//! The yaw of an HDR map ([`EnvRotation`]) through every environment lookup: the miss lookups,
//! one channel and all channels at once, the NEE draw and its pdf.
//!
//! The rule under test: a world direction is rotated into the map's frame at the entry of every
//! lookup (`to_map`), a direction drawn from the map is rotated back (`to_world`), and nothing
//! else moves: texel colors and pdfs of a draw are the unrotated draw's, bit for bit.

use super::{
    EnvRotation, EnvironmentSource, LightingPreset, environment_nee_pdf,
    sample_environment_channel, sample_environment_channels, sample_environment_for_nee,
};
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;
use std::f32::consts::{PI, TAU};

const LAMBDAS: [f32; 4] = [430.0, 515.0, 610.0, 680.0];

/// A non-uniform, every-channel-distinct map whose importance distribution is not trivial.
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

/// Directions over the sphere, with both poles and the `u = 0 / 1` seam.
fn directions() -> Vec<Vec3> {
    let mut dirs = vec![Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    for i in 0..17u8 {
        for j in 0..9u8 {
            let phi = f32::from(i).mul_add(TAU / 17.0, 0.05);
            let theta = (f32::from(j) + 0.5) * (PI / 9.0);
            let (st, ct) = theta.sin_cos();
            let (sp, cp) = phi.sin_cos();
            dirs.push(Vec3::new(st * sp, ct, st * cp));
        }
    }
    dirs
}

fn bits3(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

fn rgb_bits(rgb: [f32; 3]) -> [u32; 3] {
    rgb.map(f32::to_bits)
}

/// The `(u0, u1)` grid of the NEE checks.
fn draws() -> impl Iterator<Item = (f32, f32)> {
    (0..23u8).flat_map(|column| {
        (0..17u8).map(move |row| {
            (
                (f32::from(column) + 0.5) / 23.0,
                (f32::from(row) + 0.5) / 17.0,
            )
        })
    })
}

#[test]
fn the_identity_leaves_every_map_method_bit_for_bit() {
    let map = map();
    for rotation in [EnvRotation::IDENTITY, EnvRotation::from_yaw(0.0)] {
        for dir in directions() {
            assert_eq!(
                rgb_bits(map.radiance_rgb_rotated(dir, rotation)),
                rgb_bits(map.radiance_rgb(dir))
            );
            assert_eq!(
                map.pdf_rotated(dir, rotation).to_bits(),
                map.pdf(dir).to_bits()
            );
        }
        for (u0, u1) in draws() {
            let (dir, rgb, pdf) = map.sample(u0, u1);
            let (dir_r, rgb_r, pdf_r) = map.sample_rotated(u0, u1, rotation);
            assert_eq!(bits3(dir_r), bits3(dir));
            assert_eq!(rgb_bits(rgb_r), rgb_bits(rgb));
            assert_eq!(pdf_r.to_bits(), pdf.to_bits());
        }
    }
}

#[test]
fn a_turned_lookup_reads_the_map_in_its_own_frame() {
    let map = map();
    let rotation = EnvRotation::from_yaw(0.9);
    for dir in directions() {
        let in_map = rotation.to_map(dir);
        assert_eq!(
            rgb_bits(map.radiance_rgb_rotated(dir, rotation)),
            rgb_bits(map.radiance_rgb(in_map))
        );
        assert_eq!(
            map.pdf_rotated(dir, rotation).to_bits(),
            map.pdf(in_map).to_bits()
        );
    }
    for (u0, u1) in draws() {
        let (dir, rgb, pdf) = map.sample(u0, u1);
        let (dir_r, rgb_r, pdf_r) = map.sample_rotated(u0, u1, rotation);
        assert_eq!(bits3(dir_r), bits3(rotation.to_world(dir)));
        assert_eq!(rgb_bits(rgb_r), rgb_bits(rgb));
        assert_eq!(pdf_r.to_bits(), pdf.to_bits());
    }
}

#[test]
fn the_source_builders_carry_the_rotation() {
    let map = map();
    let rotation = EnvRotation::from_yaw(1.3);
    assert!(EnvironmentSource::hdr(&map).hdr_rotation().is_identity());
    assert!(
        EnvironmentSource::hdr_at(&map, 2.0)
            .hdr_rotation()
            .is_identity()
    );
    let turned = EnvironmentSource::hdr_at(&map, 2.0).with_hdr_rotation(rotation);
    assert_eq!(turned.hdr_rotation(), rotation);
    assert_eq!(turned.exposure().to_bits(), 2.0f32.to_bits());
    // The builders that leave an HDR source alone keep its rotation too.
    let rebuilt = turned
        .with_backdrop(0.4)
        .with_surface_glare(0.2)
        .with_head_shadow(30.0)
        .with_tray_shadow(None);
    assert_eq!(rebuilt.hdr_rotation(), rotation);
    assert_eq!(rebuilt.exposure().to_bits(), 2.0f32.to_bits());
    // The studio rig has no map: it is returned as it was.
    let studio = LightingPreset::RingLights.studio(1.6, 0.85, 0.95);
    assert!(
        studio
            .with_hdr_rotation(rotation)
            .hdr_rotation()
            .is_identity()
    );
    assert!(studio.hdr_rotation().is_identity());
}

/// The miss lookups, one channel and all channels at once, rotate the direction first.
#[test]
fn the_miss_lookups_rotate_the_direction_into_the_map_frame() {
    let map = map();
    let rotation = EnvRotation::from_yaw(2.2);
    let plain = EnvironmentSource::hdr_at(&map, 0.8);
    let turned = plain.with_hdr_rotation(rotation);
    for dir in directions() {
        let in_map = rotation.to_map(dir);
        let all = sample_environment_channels(turned, dir, &LAMBDAS, None, Vec3::ZERO);
        let all_in_map = sample_environment_channels(plain, in_map, &LAMBDAS, None, Vec3::ZERO);
        for (k, lambda) in LAMBDAS.into_iter().enumerate() {
            let one = sample_environment_channel(turned, dir, lambda, None, Vec3::ZERO);
            let one_in_map = sample_environment_channel(plain, in_map, lambda, None, Vec3::ZERO);
            assert_eq!(one.to_bits(), one_in_map.to_bits(), "{dir} {lambda}");
            assert_eq!(all[k].to_bits(), all_in_map[k].to_bits(), "{dir} {lambda}");
            assert_eq!(all[k].to_bits(), one.to_bits(), "{dir} {lambda}");
        }
    }
}

/// A turned NEE draw is the unrotated draw's direction turned to the world; its color and pdf
/// are the unrotated draw's bit for bit, and the pdf lookup at the drawn direction agrees.
#[test]
fn a_turned_nee_draw_is_the_unrotated_draw_in_the_world() {
    let map = map();
    let rotation = EnvRotation::from_yaw(-1.7);
    let plain = EnvironmentSource::hdr_at(&map, 1.5);
    let turned = plain.with_hdr_rotation(rotation);
    let mut drawn = 0;
    for (u0, u1) in draws() {
        let (Some(base), Some(world)) = (
            sample_environment_for_nee(plain, u0, u1),
            sample_environment_for_nee(turned, u0, u1),
        ) else {
            continue;
        };
        drawn += 1;
        assert_eq!(bits3(world.dir), bits3(rotation.to_world(base.dir)));
        assert_eq!(world.pdf.to_bits(), base.pdf.to_bits());
        assert_eq!(rgb_bits(world.rgb), rgb_bits(base.rgb));
        let looked_up = environment_nee_pdf(turned, world.dir);
        assert!(
            (looked_up - world.pdf).abs() < 1e-3 * world.pdf.max(1.0),
            "u=({u0}, {u1}): the draw's pdf {} against the lookup {looked_up}",
            world.pdf
        );
    }
    assert!(drawn > 300, "the fixture draws almost everywhere: {drawn}");
}

/// The turned density integrates to one over the sphere (a midpoint rule on a 1 degree grid).
/// The density is piecewise constant, so its jumps limit the accuracy.
#[test]
fn the_turned_density_integrates_to_one_over_the_sphere() {
    let map = map();
    let (rows, columns) = (180u16, 360u16);
    let (d_theta, d_phi) = (PI / f32::from(rows), TAU / f32::from(columns));
    for yaw in [0.0f32, 0.7, 4.0] {
        let rotation = EnvRotation::from_yaw(yaw);
        let mut total = 0.0f64;
        for row in 0..rows {
            let theta = (f32::from(row) + 0.5) * d_theta;
            let (st, ct) = theta.sin_cos();
            for column in 0..columns {
                let (sp, cp) = ((f32::from(column) + 0.5) * d_phi).sin_cos();
                let dir = Vec3::new(st * sp, ct, st * cp);
                total += f64::from(map.pdf_rotated(dir, rotation) * st * d_theta * d_phi);
            }
        }
        assert!(
            (total - 1.0).abs() < 3e-2,
            "yaw {yaw}: the density sums to {total}"
        );
    }
}

/// Near the poles a turn keeps the density: the yaw leaves `y` exact, and the density depends
/// on the azimuth only through the texel column.
#[test]
fn the_density_near_both_poles_survives_a_turn() {
    let map = map();
    let rotation = EnvRotation::from_yaw(1.9);
    for pole in [Vec3::Y, Vec3::NEG_Y] {
        assert!(map.pdf_rotated(pole, rotation).abs() <= f32::MIN_POSITIVE);
        assert!(map.pdf(pole).abs() <= f32::MIN_POSITIVE);
    }
    for angle_deg in [0.05f32, 0.5, 5.0] {
        for phi_deg in [10.0f32, 61.0, 137.0] {
            for north in [true, false] {
                let angle = angle_deg.to_radians();
                let theta = if north { angle } else { PI - angle };
                let (st, ct) = theta.sin_cos();
                let (sp, cp) = phi_deg.to_radians().sin_cos();
                let in_map = Vec3::new(st * sp, ct, st * cp);
                let world = rotation.to_world(in_map);
                let expected = map.pdf(in_map);
                let got = map.pdf_rotated(world, rotation);
                assert!(
                    (got - expected).abs() <= 1e-3 * expected.max(1e-6),
                    "{angle_deg} deg off the {} pole at {phi_deg} deg: {got} against {expected}",
                    if north { "north" } else { "south" }
                );
            }
        }
    }
}
