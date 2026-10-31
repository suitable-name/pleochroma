//! The surface scene on the public trace entry points.
//!
//! The ideal scene changes no bit, a ground word and a frosted finish are the same facet,
//! and an abraded edge keeps next-event estimation unbiased.

use super::super::{
    super::{
        camera::Ray,
        environment::{EnvironmentSource, LightingPreset},
        intersect::build_plane_soa,
        sampling::hash_u32,
        surface::{EdgeAbrasion, FacetSurface, FacetingMachine, SurfaceScene},
    },
    inner::trace_spectral_ray_inner_geom_events_surface,
    trace_spectral_ray_attributed_soa_geom, trace_spectral_ray_attributed_soa_geom_surface,
    trace_spectral_ray_with_finish_soa_geom, trace_spectral_ray_with_finish_soa_geom_surface,
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{camera::FacetFinish, transport::PathTermination},
    },
    renderer::env_map::EnvironmentMap,
};
use glam::Vec3;

/// A uniform `[-0.5, 0.5)` draw for sample `i` and `salt`.
fn centred(i: u32, salt: u32) -> f32 {
    (hash_u32(i ^ salt) as f32) / 4_294_967_295.0 - 0.5
}

/// An oblique camera ray from above that mostly meets the stone, varied per `i`.
fn ray_for(i: u32) -> Ray {
    Ray {
        origin: Vec3::new(centred(i, 0x1111) * 1.4, 2.5, centred(i, 0x2222) * 1.4),
        dir: Vec3::new(centred(i, 0x3333) * 0.8, -1.0, centred(i, 0x4444) * 0.8).normalize(),
    }
}

fn seed_and_hero(i: u32) -> (u32, f32) {
    let seed = 31_000 + i;
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// With the ideal scene the surface entry points are the plain ones, bit for bit, for a polished
/// list and for a list with frosted facets.
#[test]
fn an_ideal_surface_scene_changes_no_bit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let mixed: Vec<FacetFinish> = (0..planes.len())
        .map(|i| {
            if i % 3 == 0 {
                FacetFinish::Frosted
            } else {
                FacetFinish::Polished
            }
        })
        .collect();
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let material = GemMaterial::by_name("Diamond").expect("built-in material");
    let fluorescence = Fluorescence::none();
    for finishes in [&[][..], &mixed[..]] {
        for i in 0..400 {
            let (seed, hero) = seed_and_hero(i);
            let plain = trace_spectral_ray_with_finish_soa_geom(
                ray_for(i),
                geom,
                &plane_soa,
                finishes,
                &material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
            );
            let with_scene = trace_spectral_ray_with_finish_soa_geom_surface(
                ray_for(i),
                geom,
                &plane_soa,
                finishes,
                &material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            );
            assert_eq!(bits(plain), bits(with_scene), "sample {i}");
            let (attributed, _) = trace_spectral_ray_attributed_soa_geom(
                ray_for(i),
                geom,
                &plane_soa,
                finishes,
                &material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
            );
            let (attributed_scene, _) = trace_spectral_ray_attributed_soa_geom_surface(
                ray_for(i),
                geom,
                &plane_soa,
                finishes,
                &material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            );
            assert_eq!(bits(attributed), bits(attributed_scene), "sample {i}");
        }
    }
}

/// A frosted word in the surface list and a frosted entry in the finish list are the same
/// facet: the traces agree bit for bit, also under next-event estimation through a scattering
/// medium, where the finish list is what the shadow probe reads.
#[test]
fn a_ground_word_is_a_frosted_finish() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let env_map = EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let hdr = EnvironmentSource::HdrMap(&env_map);
    let studio = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let clear = GemMaterial::by_name("Zircon").expect("built-in material");
    let hazy = GemMaterial::new_custom("hazy", 1.5, 0.0, 0.0, [0.0; 3]).with_scattering(1.0, 0.2);
    let words = vec![FacetSurface::frosted(); planes.len()];
    let list = vec![FacetFinish::Frosted; planes.len()];
    let fluorescence = Fluorescence::none();
    let scene = SurfaceScene {
        facet_surfaces: &words,
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion::NONE,
    };
    for (environment, material) in [(studio, &clear), (hdr, &hazy)] {
        for i in 0..300 {
            let (seed, hero) = seed_and_hero(i);
            let by_list = trace_spectral_ray_with_finish_soa_geom(
                ray_for(i),
                geom,
                &plane_soa,
                &list,
                material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
            );
            let by_word = trace_spectral_ray_with_finish_soa_geom_surface(
                ray_for(i),
                geom,
                &plane_soa,
                &[],
                material,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
                scene,
            );
            assert_eq!(bits(by_list), bits(by_word), "{} sample {i}", material.name);
        }
    }
}

/// An abraded edge keeps the estimator consistent: with next-event estimation on and off the
/// mean radiance of a lossless scattering stone under a uniform map is the same, so the shadow
/// probe and the phase-sampled continuation agree about the ground band.
#[test]
fn nee_through_an_abraded_edge_converges_to_the_phase_sampled_mean() {
    const SAMPLES: u32 = 20_000;
    const TOLERANCE: f32 = 0.05;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material =
        GemMaterial::new_custom("band probe", 1.5, 0.0, 0.0, [0.0; 3]).with_scattering(1.0, 0.2);
    let env_map = EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let scene = SurfaceScene {
        facet_surfaces: &[],
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion {
            width: 0.08,
            alpha: 0.4,
            halo_rq_nm: 0.0,
        },
    };
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
    };
    let fluorescence = Fluorescence::none();
    let trace = |seed: u32, nee: bool| {
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        let mut termination = (12, PathTermination::HitCap);
        trace_spectral_ray_inner_geom_events_surface(
            ray,
            &planes,
            &[],
            &plane_soa,
            &[],
            &material,
            fluorescence,
            12,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            nee,
            ((Some(&mut termination), None), scene),
        )
    };
    let (mut sum_on, mut sum_off) = (Vec3::ZERO, Vec3::ZERO);
    for i in 0..SAMPLES {
        let seed = 52_000 + i;
        sum_on += trace(seed, true);
        sum_off += trace(seed, false);
    }
    let mean_on = sum_on / SAMPLES as f32;
    let mean_off = sum_off / SAMPLES as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "NEE-on and NEE-off should agree through an abraded edge (mean_on={mean_on:?}, \
         mean_off={mean_off:?}, rel_err=({ex}, {ey}, {ez}))"
    );
}
