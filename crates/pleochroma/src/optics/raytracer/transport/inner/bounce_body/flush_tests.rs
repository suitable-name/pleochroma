//! The flush at a bounce off the glossy tray (`PathState::flush_radiance_to_nee`): the radiance
//! gathered so far, staged exit-split contributions included, is integrated at that moment with
//! that moment's `path_pdf`/`compat` and moved into `nee_xyz`, and both accumulators restart at
//! zero. Driven on a `PathState` directly, since its fields are private to the bounce loop.

use super::*;
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            environment::LightingPreset, intersect::build_plane_soa, refraction::DirectionMatch,
        },
    },
};

/// Builds a `PathState` over a round brilliant Diamond with exit splitting `exit_splitting`,
/// loads the accumulators with known values, flushes and compares with an independent
/// integration of the same sums.
fn check_flush(exit_splitting: bool) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::diamond();
    let environment = LightingPreset::RingLights
        .studio(1.0, 0.0, 0.85)
        .with_backdrop(0.23);
    let scene = TraceScene {
        planes: &planes,
        tools: &[],
        plane_soa: &plane_soa,
        facet_finishes: &[],
        material: &diamond,
        fluorescence: Fluorescence::none(),
        max_bounces: 4,
        environment,
        surface_glare: 1.0,
        surface: crate::optics::raytracer::surface::SurfaceScene::IDEAL,
        zone_kernels: None,
        tables: None,
    };
    let switches = TraceSwitches {
        internal_mode_coupling: false,
        exit_splitting,
        nee: false,
        direction_match: DirectionMatch::default(),
        estimator: Estimator::Branching,
    };
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::NEG_Y,
    };
    let mut split_radiance = [0.0f32; NUM_CHANNELS];
    let mut path = PathState::new(scene, ray, 11, 0.37, switches, &mut split_radiance);

    path.radiance = [0.5, 0.25, 0.0, 1.0, 0.125, 0.75, 0.0, 0.3];
    path.exit_split_ctx.split_radiance[2] = 0.25;
    path.exit_split_ctx.split_radiance[7] = 0.5;
    path.path_pdf = [1.0, 0.8, 0.6, 1.0, 0.5, 0.9, 1.0, 0.7];
    // Every channel keeps its own bit and the hero's, as `compat` always does.
    path.exit_split_ctx.compat = [0xFF, 0x7F, 0xFD, 0xFF, 0x11, 0xEF, 0xFF, 0xBF];
    path.nee_xyz = Vec3::new(0.01, 0.02, 0.03);

    // The same sums, integrated the way `finish_with_mask` integrates them.
    let mut summed = path.radiance;
    for (sum, &staged) in summed
        .iter_mut()
        .zip(path.exit_split_ctx.split_radiance.iter())
    {
        *sum += staged;
    }
    let integrated = if exit_splitting {
        integrate_channels_to_xyz_families(
            &summed,
            &path.camera_lambdas,
            &path.path_pdf,
            HERO_IDX,
            path.exit_split_ctx.compat,
        )
    } else {
        integrate_channels_to_xyz(&summed, &path.camera_lambdas, &path.path_pdf, HERO_IDX)
    };
    assert!(
        integrated.max_element() > 0.0,
        "the loaded radiance must integrate to something"
    );
    let before = path.nee_xyz;

    path.flush_radiance_to_nee();

    assert_eq!(
        path.nee_xyz.to_array().map(f32::to_bits),
        (before + integrated).to_array().map(f32::to_bits),
        "exit_splitting {exit_splitting}: the integrated sum joins nee_xyz"
    );
    assert_eq!(
        path.radiance, [0.0f32; NUM_CHANNELS],
        "radiance restarts at zero"
    );
    assert_eq!(
        *path.exit_split_ctx.split_radiance, [0.0f32; NUM_CHANNELS],
        "the staged exit-split contributions restart at zero"
    );
    assert!(!path.path_escaped, "a floor bounce is not an escape");
}

/// The flush with exit splitting on (family integration) and off (hero comb).
#[test]
fn the_flush_moves_the_integrated_radiance_to_nee_and_zeroes_both_accumulators() {
    check_flush(true);
    check_flush(false);
}
