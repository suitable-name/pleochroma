//! The scene exposure under an HDR map: [`hdr_rgb_at_exposure`] and every lookup that applies
//! it (the miss lookups, one channel and all channels at once, and the NEE draw).
//!
//! Exposure `2.0` is checked bit for bit. Scaling by a power of two commutes with the rounding
//! of the spectral lift (a chain of `max`, `min`, `-`, `*` and `fma` that is homogeneous in the
//! texel), so the doubled lookup is exactly twice the plain one wherever values stay normal.

use super::{
    EnvironmentSource, LightingPreset, environment_nee_pdf, hdr_rgb_at_exposure,
    sample_environment_channel, sample_environment_channels, sample_environment_for_nee,
};
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;

/// A non-uniform, every-channel-distinct map (so the lift takes both its neutral and its
/// chroma part, and the importance distribution is not trivial).
fn map() -> EnvironmentMap {
    let (width, height) = (12, 6);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let u = x as f32 / width as f32;
                let v = y as f32 / height as f32;
                [
                    1.7f32.mul_add(u, 0.15),
                    0.9f32.mul_add(v, 0.2),
                    u.mul_add(v, 0.05) * 2.5,
                ]
            })
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
}

const DIRS: [Vec3; 5] = [
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.6, 0.3, -0.74),
    Vec3::new(-0.2, -0.9, 0.39),
    Vec3::new(0.95, -0.05, 0.3),
    Vec3::new(-0.5, 0.5, 0.707),
];
const LAMBDAS: [f32; 8] = [385.0, 430.0, 470.0, 515.0, 560.0, 610.0, 680.0, 775.0];

#[test]
fn unit_exposure_returns_the_texel_and_others_scale_each_channel() {
    for rgb in [
        [0.25, 1.5, 3.0],
        [0.0, -0.0, f32::MIN_POSITIVE],
        [7.0e5, 1.0e-30, 0.1],
    ] {
        assert_eq!(
            hdr_rgb_at_exposure(rgb, 1.0).map(f32::to_bits),
            rgb.map(f32::to_bits),
            "exposure 1.0 is the texel itself"
        );
        assert_eq!(
            hdr_rgb_at_exposure(rgb, 0.37).map(f32::to_bits),
            rgb.map(|c| (c * 0.37).to_bits()),
        );
    }
    assert_eq!(hdr_rgb_at_exposure([1.0, 2.0, 4.0], 0.0), [0.0; 3]);
}

#[test]
fn the_hdr_constructors_and_accessors_carry_the_exposure() {
    let map = map();
    let plain = EnvironmentSource::hdr(&map);
    assert_eq!(plain.exposure().to_bits(), 1.0f32.to_bits());
    assert!(
        plain
            .hdr_map()
            .is_some_and(|m| std::ptr::eq(m, &raw const map))
    );
    let exposed = EnvironmentSource::hdr_at(&map, 3.25);
    assert_eq!(exposed.exposure().to_bits(), 3.25f32.to_bits());
    // The studio-only builders return an HDR source unchanged, exposure included.
    let rebuilt = exposed
        .with_backdrop(0.4)
        .with_surface_glare(0.2)
        .with_head_shadow(30.0);
    assert_eq!(rebuilt.exposure().to_bits(), 3.25f32.to_bits());
    let studio = LightingPreset::RingLights.studio(1.6, 0.85, 0.95);
    assert_eq!(studio.exposure().to_bits(), 1.6f32.to_bits());
    assert!(studio.hdr_map().is_none());
}

/// At exposure 1 the miss lookup is `EnvironmentMap::radiance_at` bit for bit, and the
/// all-channels form is the one-channel form bit for bit, at every exposure.
#[test]
fn an_unexposed_map_is_its_own_radiance_and_both_lookups_agree() {
    let map = map();
    for dir in DIRS {
        let dir = dir.normalize();
        for exposure in [1.0, 2.0, 0.6] {
            let env = EnvironmentSource::hdr_at(&map, exposure);
            let all = sample_environment_channels(env, dir, &LAMBDAS, None, Vec3::ZERO);
            for (k, lambda) in LAMBDAS.into_iter().enumerate() {
                let one = sample_environment_channel(env, dir, lambda, None, Vec3::ZERO);
                assert_eq!(
                    all[k].to_bits(),
                    one.to_bits(),
                    "{dir} {lambda} x{exposure}"
                );
                if exposure == 1.0 {
                    assert_eq!(one.to_bits(), map.radiance_at(dir, lambda).to_bits());
                }
            }
        }
    }
}

/// Exposure 2 doubles every miss lookup exactly; any other exposure scales it to within a
/// few rounding steps.
#[test]
fn the_exposure_scales_every_miss_lookup() {
    let map = map();
    let plain = EnvironmentSource::hdr(&map);
    for dir in DIRS {
        let dir = dir.normalize();
        for lambda in LAMBDAS {
            let base = sample_environment_channel(plain, dir, lambda, None, Vec3::ZERO);
            assert!(
                base > 0.0,
                "the fixture lights every direction: {dir} {lambda}"
            );
            let doubled = sample_environment_channel(
                EnvironmentSource::hdr_at(&map, 2.0),
                dir,
                lambda,
                None,
                Vec3::ZERO,
            );
            assert_eq!(doubled.to_bits(), (base * 2.0).to_bits(), "{dir} {lambda}");
            let dimmed = sample_environment_channel(
                EnvironmentSource::hdr_at(&map, 0.3),
                dir,
                lambda,
                None,
                Vec3::ZERO,
            );
            assert!(
                base.mul_add(-0.3, dimmed).abs() <= 4e-6 * base,
                "{dimmed} vs {} at {dir} {lambda}",
                base * 0.3
            );
        }
    }
}

/// An NEE draw keeps its direction and its pdf (the distribution does not depend on the
/// exposure) and carries the exposed radiance; the pdf lookup ignores the exposure too.
#[test]
fn the_exposure_scales_an_nee_draw_but_not_its_pdf() {
    let map = map();
    let plain = EnvironmentSource::hdr(&map);
    let doubled = EnvironmentSource::hdr_at(&map, 2.0);
    let mut drawn = 0;
    for column in 0..23u16 {
        for row in 0..17u16 {
            let (u0, u1) = (
                (f32::from(column) + 0.5) / 23.0,
                (f32::from(row) + 0.5) / 17.0,
            );
            let (Some(base), Some(lit)) = (
                sample_environment_for_nee(plain, u0, u1),
                sample_environment_for_nee(doubled, u0, u1),
            ) else {
                continue;
            };
            drawn += 1;
            assert_eq!(
                base.dir.to_array().map(f32::to_bits),
                lit.dir.to_array().map(f32::to_bits)
            );
            assert_eq!(base.pdf.to_bits(), lit.pdf.to_bits());
            assert_eq!(
                environment_nee_pdf(plain, base.dir).to_bits(),
                environment_nee_pdf(doubled, base.dir).to_bits()
            );
            for lambda in LAMBDAS {
                assert_eq!(
                    lit.radiance(lambda).to_bits(),
                    (base.radiance(lambda) * 2.0).to_bits(),
                    "u=({u0}, {u1}) {lambda}"
                );
            }
        }
    }
    assert!(
        drawn > 300,
        "the fixture's distribution draws almost everywhere: {drawn}"
    );
}
