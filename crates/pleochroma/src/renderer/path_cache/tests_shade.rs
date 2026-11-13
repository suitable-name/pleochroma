//! Phase B equivalence: the cache shades a position to the same mean as the path tracer.
//!
//! Each test builds the records of a few camera rays with [`TraceKit::position`], shades many
//! samples of every position through [`Shader::shade_position`], traces many ordinary samples
//! through the same rays, and compares the two means component by component with a z score.
//! Comparing per position (and not per pixel) keeps the sub-pixel quadrature of phase A out of
//! it: phase B must reproduce the position it is given, the quadrature is phase A's.

use glam::DVec3;

use super::{
    DISPERSION_GATE_BG, ExitRecord, PositionRef, Shader, TraceKit,
    tests_support::{Rig, glass},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, EnvironmentSource, LightingPreset, Ray,
            surface::SurfaceScene,
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    renderer::{frame_scene::FrameScene, whitted::CUTOFF},
};

pub(super) const SIZE: u32 = 16;
pub(super) const MAX_BOUNCES: u32 = 12;

/// Pixels of the 16 x 16 brilliant frame whose centre ray meets the stone.
pub(super) const STONE_PIXELS: [(u32, u32); 6] = [(8, 8), (5, 9), (10, 6), (7, 5), (9, 10), (6, 7)];

// The statistics helpers live in `renderer::test_stats`; the other path cache tests keep importing
// them from here.
pub(super) use crate::renderer::test_stats::{Stats, assert_z_stats, draw, smooth_environment};

/// The ray through the centre of pixel `(x, y)` of the test frame.
pub(super) fn centre_ray(rig: &Rig, (x, y): (u32, u32)) -> Ray {
    rig.camera
        .generate_ray(x as f32, y as f32, SIZE as f32, SIZE as f32, 0.0, 0.0)
}

/// The z scores (X, Y and Z of every position) of phase B against the path tracer on the stone
/// pixels of `rig`, and how many positions were compared.
pub(super) fn equivalence_zs(
    rig: &Rig,
    environment: EnvironmentSource<'_>,
    (cached_samples, traced_samples): (u32, u32),
) -> (Vec<f64>, usize) {
    let scene = FrameScene {
        environment,
        ..rig.scene(SIZE, MAX_BOUNCES)
    };
    let prepared = PreparedMaterial::new(&rig.material);
    let shader = Shader::new(&scene, &rig.soa, prepared.clone());
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (MAX_BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );
    let geom = StoneGeometry::planes_only(&rig.planes);
    let mut zs = Vec::new();
    let mut positions = 0;
    for pixel in STONE_PIXELS {
        let ray = centre_ray(rig, pixel);
        let output = kit.position(ray);
        if output.records.iter().all(ExitRecord::is_direct) {
            continue;
        }
        positions += 1;
        let reference = PositionRef {
            records: &output.records,
            residual: output.residual,
        };
        let mut cached = Stats::default();
        for i in 0..cached_samples {
            cached.add(
                shader
                    .shade_position(reference, ray, draw(i, 0xCAFE))
                    .total(),
            );
        }
        let mut traced = Stats::default();
        for i in 0..traced_samples {
            let (seed, hero) = draw(i, 0xF00D);
            traced.add(trace_spectral_ray_prepared(
                ray,
                geom,
                &rig.soa,
                &[],
                &prepared,
                Fluorescence::none(),
                MAX_BOUNCES,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            ));
        }
        for component in 0..3 {
            if let Some(z) = cached.z_against(&traced, component) {
                zs.push(z);
            }
        }
    }
    (zs, positions)
}

fn assert_equivalent(rig: &Rig, label: &str) {
    let (zs, positions) = equivalence_zs(rig, smooth_environment(), (1500, 6000));
    assert!(
        positions >= 3,
        "{label}: only {positions} positions on the stone"
    );
    assert_z_stats(&zs, 4.0, label);
}

#[test]
fn isotropic_glass_matches_the_path_tracer() {
    assert_equivalent(&Rig::brilliant(glass()), "isotropic glass");
}

#[test]
fn uniaxial_sapphire_matches_the_path_tracer() {
    let sapphire = GemMaterial::new_custom("sapphire-like", 1.768, 0.0106, 0.008, [0.0; 3]);
    assert_equivalent(&Rig::brilliant(sapphire), "uniaxial sapphire");
}

#[test]
fn dispersive_diamond_matches_the_path_tracer() {
    // Above the dispersion gate: every branch is a per-bin record.
    let diamond = GemMaterial::new_custom("diamond-like", 2.417, 0.0256, 0.0, [0.0; 3]);
    assert_equivalent(&Rig::brilliant(diamond), "dispersive diamond");
}

/// A colorless host with a base zone and a half-space zone, both with wide absorption bands.
fn zoned_stone() -> GemMaterial {
    let band = |center_nm: f32, peak_per_mm: f32| {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            center_nm,
            150.0,
            peak_per_mm,
        )]))
    };
    let mut zoning = ZonedAbsorption::new(band(520.0, 0.04));
    zoning.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: band(620.0, 0.08),
    });
    GemMaterial::new_custom("zoned probe", 1.76, 0.0, 0.0, [0.0; 3])
        .with_zoning(zoning)
        .with_absorption_path_scale(3.0)
}

#[test]
fn a_zoned_stone_is_not_cached() {
    let rig = Rig::brilliant(zoned_stone());
    let scene = rig.scene(SIZE, MAX_BOUNCES);
    let refused = super::PathCacheFrame::new(
        &scene,
        &rig.soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        super::CacheOptions::default(),
    );
    assert_eq!(refused.err(), Some(super::PathCacheOff::ZonedColor));
    assert_eq!(
        super::PathCacheOff::ZonedColor.message(),
        "Path cache off: the material has color zones"
    );
}

/// The benchmark sapphire, whose ordinary band lies between two comb nodes, shades to the tracer's
/// mean at every probed position. This guards the absorption between comb nodes.
#[test]
fn the_bench_sapphire_matches_the_path_tracer() {
    use crate::renderer::test_stats::{bench_daylight, bench_sapphire};
    let rig = Rig::brilliant(bench_sapphire());
    let (zs, positions) = equivalence_zs(&rig, bench_daylight(), (6000, 24_000));
    assert!(positions >= 3, "only {positions} positions on the stone");
    assert_z_stats(&zs, 4.5, "bench sapphire, Daylight");
}

/// The same stone under the smooth test environment: the stable statistics of the other
/// equivalence tests. If this passes and the Daylight test fails just over its limit, the
/// Daylight sample counts are too small, not the absorption wrong.
#[test]
fn the_bench_sapphire_matches_the_path_tracer_under_smooth_light() {
    use crate::renderer::test_stats::bench_sapphire;
    assert_equivalent(&Rig::brilliant(bench_sapphire()), "bench sapphire, smooth");
}

/// The records of a colored stone carry depth, and their comb weights are free of absorption.
#[test]
fn colored_records_carry_depth_and_absorption_free_weights() {
    use crate::renderer::test_stats::bench_sapphire;
    let rig = Rig::brilliant(bench_sapphire());
    let prepared = PreparedMaterial::new(&rig.material);
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (MAX_BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );
    let output = kit.position(centre_ray(&rig, (8, 8)));
    for record in &output.records {
        if record.flags & super::record::FLAG_GLARE != 0 || record.is_direct() {
            assert!(
                !record.has_depth(),
                "a surface reflection crosses no material"
            );
        }
    }
    let deep: Vec<&ExitRecord> = output.records.iter().filter(|r| r.has_depth()).collect();
    assert!(!deep.is_empty(), "interior branches cross the stone");
    let mut dimmed = 0_usize;
    for record in deep {
        let smooth = record.weights();
        let absorbed = kit.comb_weights(record);
        for (k, (a, s)) in absorbed.iter().zip(&smooth).enumerate() {
            assert!(
                a <= s,
                "channel {k}: absorbed {a} above the stored weight {s}"
            );
        }
        // 560 nm sits beside the 585 nm band: absorbed in the tree, not in the stored weight.
        dimmed += usize::from(!record.is_per_bin() && absorbed[3] < 0.99 * smooth[3]);
    }
    assert!(
        dimmed > 0,
        "some merged record is visibly absorbed at 560 nm"
    );
}

/// A colorless stone carries no optical depth: the depth words of its records are all zero.
#[test]
fn a_colorless_stone_has_no_depth() {
    let rig = Rig::brilliant(glass());
    let prepared = PreparedMaterial::new(&rig.material);
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (MAX_BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );
    let output = kit.position(centre_ray(&rig, (8, 8)));
    assert!(output.records.len() > 1, "the stone has interior branches");
    assert!(output.records.iter().all(|record| !record.has_depth()));
}

#[test]
fn the_studio_rig_with_small_lights_matches_too() {
    // The light tent has a spark light a few degrees wide: rarer, brighter exits, so more samples
    // and a looser bound.
    let rig = Rig::brilliant(glass());
    let environment = LightingPreset::LightTent
        .studio(1.0, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY);
    let (zs, positions) = equivalence_zs(&rig, environment, (4000, 20_000));
    assert!(positions >= 3, "only {positions} positions on the stone");
    assert_z_stats(&zs, 5.0, "light tent");
}

#[test]
fn shading_is_deterministic() {
    let rig = Rig::brilliant(glass());
    let scene = rig.scene(SIZE, MAX_BOUNCES);
    let prepared = PreparedMaterial::new(&rig.material);
    let shader = Shader::new(&scene, &rig.soa, prepared.clone());
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (MAX_BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );
    let ray = centre_ray(&rig, (8, 8));
    let output = kit.position(ray);
    let reference = PositionRef {
        records: &output.records,
        residual: output.residual,
    };
    for i in 0..64 {
        let first = shader.shade_position(reference, ray, draw(i, 7));
        let second = shader.shade_position(reference, ray, draw(i, 7));
        assert_eq!(first, second, "sample {i}");
    }
}
