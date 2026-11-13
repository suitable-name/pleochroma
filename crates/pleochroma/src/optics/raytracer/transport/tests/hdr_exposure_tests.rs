//! The scene exposure under an HDR map, end to end through the spectral tracer.
//!
//! The estimator is linear in the environment's radiance: the random decisions (Fresnel
//! branch, scattering, roulette) read the path's throughput, never the light, and the MIS
//! weights read pdfs, which do not depend on the exposure. So a path traced at exposure 2
//! with the same seed returns twice the XYZ of the same path at exposure 1 -- through the
//! miss lookups, the exit-split probes, the Henyey-Greenstein NEE (a scattering stone) and
//! the frosted-exterior NEE (a frosted stone). Compared with a rounding tolerance rather than
//! bit for bit, since a deep path's tiny throughput may round differently once doubled.

use super::super::{
    super::{
        camera::{FacetFinish, Ray},
        environment::EnvironmentSource,
        intersect::build_plane_soa,
        sampling::hash_u32,
    },
    trace_spectral_ray_with_finish_soa,
};
use crate::{
    geometry::cuts::StandardGemCuts, optics::materials::GemMaterial,
    renderer::env_map::EnvironmentMap,
};
use glam::Vec3;

const SAMPLES: u32 = 256;

/// A bright upper sky with a hot patch and a dim, tinted ground, so every lookup and NEE
/// draw sees a different radiance.
fn sky() -> EnvironmentMap {
    let (width, height) = (16, 8);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                if y < height / 2 {
                    let hot: f32 = if (5..8).contains(&x) && y == 1 {
                        6.0
                    } else {
                        1.0
                    };
                    [0.9 * hot, hot, 1.2 * hot]
                } else {
                    [0.12, 0.08, 0.05]
                }
            })
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
}

/// Every sample of one oblique ray at the table, at `exposure`, same seeds every call.
fn samples(material: &GemMaterial, finishes: &[FacetFinish], exposure: f32) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let map = sky();
    let environment = EnvironmentSource::hdr_at(&map, exposure);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    (0..SAMPLES)
        .map(|i| {
            let seed = 9100 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            trace_spectral_ray_with_finish_soa(
                ray,
                &planes,
                &plane_soa,
                finishes,
                material,
                12,
                environment,
                seed,
                hero_rand,
                None,
            )
        })
        .collect()
}

/// `scaled` is `factor` times `base`, sample by sample, to within a few roundings.
fn assert_scaled(label: &str, base: &[Vec3], scaled: &[Vec3], factor: f32) {
    let mut lit = 0;
    for (i, (b, s)) in base.iter().zip(scaled).enumerate() {
        let expected = *b * factor;
        let error = (*s - expected).abs().max_element();
        assert!(
            error <= 1e-5 * expected.abs().max_element().max(1e-12),
            "{label}: sample {i} gives {s} at the scaled exposure, {expected} expected"
        );
        if b.max_element() > 0.0 {
            lit += 1;
        }
    }
    assert!(lit > SAMPLES / 4, "{label}: only {lit} lit samples");
}

#[test]
fn an_hdr_scene_at_exposure_two_returns_twice_the_radiance() {
    let polished: [FacetFinish; 0] = [];
    let frosted = vec![FacetFinish::Frosted; StandardGemCuts::standard_round_brilliant().len()];
    let diamond = GemMaterial::diamond();
    let hazy = GemMaterial::new_custom("haze probe", 1.6, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(1.0, 0.2);
    for (label, material, finishes) in [
        ("polished diamond", &diamond, &polished[..]),
        ("scattering stone (HG NEE)", &hazy, &polished[..]),
        ("frosted diamond (frosted NEE)", &diamond, &frosted[..]),
    ] {
        let base = samples(material, finishes, 1.0);
        assert_scaled(label, &base, &samples(material, finishes, 2.0), 2.0);
        assert_scaled(label, &base, &samples(material, finishes, 0.35), 0.35);
    }
}

/// `hdr(map)` and `hdr_at(map, 1.0)` are the same source, so the default traces bit for bit
/// like an explicit exposure of one.
#[test]
fn exposure_one_is_the_default_bit_for_bit() {
    let diamond = GemMaterial::diamond();
    let map = sky();
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    for seed in 0..64u32 {
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        let trace = |environment| {
            trace_spectral_ray_with_finish_soa(
                ray,
                &planes,
                &plane_soa,
                &[],
                &diamond,
                12,
                environment,
                seed,
                hero_rand,
                None,
            )
        };
        let a = trace(EnvironmentSource::hdr(&map));
        let b = trace(EnvironmentSource::hdr_at(&map, 1.0));
        assert_eq!(
            a.to_array().map(f32::to_bits),
            b.to_array().map(f32::to_bits)
        );
    }
}
