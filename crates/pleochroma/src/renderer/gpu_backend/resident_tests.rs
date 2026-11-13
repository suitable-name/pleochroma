//! GPU backend tests of GPU-resident adaptive runs (`gpu` feature).
//!
//! Like the other hardware tests in this module they skip with a printed note when no adapter is
//! present (`PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a failure).
//!
//! The contract under test: a resident run adds exactly what the host live pass adds (same
//! kernels, same reduce order, so the sums agree within the GPU-vs-GPU golden tolerance), the GPU
//! tile noise is the CPU metric (`adaptive::metric`) evaluated on the same buffers within a
//! stated tolerance, and a run that fails is dead rather than half-right.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    GpuAccumulate, GpuBackend, GpuSceneRef,
    tests::{acquire_or_skip, bits},
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, surface::SurfaceScene},
    },
    renderer::{
        adaptive::{LiveTiles, NoiseMetric, Pass, PendingCheck, metric::live_tile_noises},
        gpu::{event_wavefront::compare::compare_frames, frame::live_pixel_indices},
    },
};

const MAX_BOUNCES: u32 = 6;

/// 40 x 36 at tile 16 is 3 x 3 tiles, the right column 8 pixels wide and the bottom row 4 high.
const FRAME: (u32, u32) = (40, 36);
const TILE: u32 = 16;

/// An interior tile and the clipped right, bottom and corner tiles.
const PARTIAL: [u32; 4] = [1, 5, 7, 8];

/// How far a GPU tile noise may differ from the CPU metric's, as a fraction of the CPU value
/// (plus [`NOISE_ABS_TOL`]) for most tiles. The GPU works in f32 where the metric works in f64,
/// through `pow`, `atan2`, a 32-step gamut walk and CIEDE2000. This is an estimate, not a
/// measured budget. The test prints the worst case.
const NOISE_REL_TOL: f32 = 0.02;

/// The absolute slack added to [`NOISE_REL_TOL`] (noise values are of order 0.01 to 10).
const NOISE_ABS_TOL: f32 = 2.0e-3;

/// A tile may exceed [`NOISE_REL_TOL`] when its worst pixel sits next to a discontinuity of the
/// gamut walk; no tile may exceed this loose bound, and at most one in ten the tight one.
const NOISE_LOOSE_REL_TOL: f32 = 0.25;

fn camera() -> Camera {
    Camera::new(0.35, 0.28, 5.0, 18.0)
}

const fn frame_len() -> usize {
    (FRAME.0 * FRAME.1) as usize
}

const fn gpu_scene<'a>(
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
) -> GpuSceneRef<'a> {
    GpuSceneRef {
        camera,
        width: FRAME.0,
        height: FRAME.1,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        estimator: None,
    }
}

fn partial_tiles() -> LiveTiles {
    LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, PARTIAL.to_vec())
}

fn all_tiles() -> LiveTiles {
    LiveTiles::all(FRAME.0, FRAME.1, TILE)
}

/// How many of the absolute sample indices `first..first + spp` are odd.
fn odd_count(first: u32, spp: u32) -> u32 {
    u32::try_from(
        (first..first + spp)
            .filter(|s| !s.is_multiple_of(2))
            .count(),
    )
    .unwrap_or(0)
}

#[test]
fn a_disabled_backend_declines_a_resident_run() {
    let backend = GpuBackend::disabled();
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let run = backend.try_adaptive_resident(
        &gpu_scene(&camera, &planes, &material),
        &[],
        &SurfaceScene::IDEAL,
        16,
    );
    assert!(run.is_none());
}

/// Two resident passes (an odd offset, partial tiles) hold what two host live passes add, and the
/// pixels outside the live tiles are exactly zero.
#[test]
fn a_resident_run_adds_what_the_host_live_pass_adds() {
    let Some(backend) = acquire_or_skip("a_resident_run_adds_what_the_host_live_pass_adds") else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let live = partial_tiles();
    let passes = [(5_u32, 24_u32), (29, 8)];
    let never_cancel = AtomicBool::new(false);

    let mut run = backend
        .try_adaptive_resident(
            &gpu_scene(&camera, &planes, &material),
            &[],
            &SurfaceScene::IDEAL,
            24,
        )
        .expect("a GPU backend starts a resident run on a small frame");
    for &(first_sample, spp) in &passes {
        let pass = Pass {
            first_sample,
            spp,
            live: live.clone(),
        };
        assert_eq!(run.pass(&pass, &never_cancel), GpuAccumulate::Done);
    }
    let (sum, half_b) = run.finish().expect("the run delivers its sums");
    assert_eq!((sum.len(), half_b.len()), (frame_len(), frame_len()));

    let scene = gpu_scene(&camera, &planes, &material);
    let mut host_sum = vec![Vec3::ZERO; frame_len()];
    let mut host_half = vec![Vec3::ZERO; frame_len()];
    for &(sample_offset, spp) in &passes {
        let outcome = backend.try_accumulate_geom_surface_live_cancellable(
            &scene,
            &[],
            &SurfaceScene::IDEAL,
            sample_offset,
            spp,
            &live,
            &mut host_sum,
            &mut host_half,
            &never_cancel,
        );
        assert_eq!(outcome, GpuAccumulate::Done);
    }

    let total_spp = passes.iter().map(|p| p.1).sum::<u32>();
    for (label, resident, host) in [("sum", &sum, &host_sum), ("half B", &half_b, &host_half)] {
        let diff = compare_frames(resident, host, total_spp);
        println!("{label}, resident vs host live pass: {diff:?}");
        assert!(
            diff.tol_passed(resident.len()),
            "{label}: the resident sums differ from the host live pass: {diff:?}"
        );
    }
    let inside: std::collections::BTreeSet<u32> = live_pixel_indices(&live).into_iter().collect();
    for (pixel, (a, b)) in sum.iter().zip(&half_b).enumerate() {
        let index = u32::try_from(pixel).expect("a test frame is small");
        if !inside.contains(&index) {
            assert_eq!(bits(&[*a, *b]), bits(&[Vec3::ZERO; 2]), "pixel {pixel}");
        }
    }
}

/// The GPU tile noise is the CPU metric on the same buffers, for both metrics and for passes that
/// start at an even and an odd absolute sample.
#[test]
fn the_gpu_tile_noise_matches_the_cpu_metric_on_the_same_buffers() {
    let Some(backend) =
        acquire_or_skip("the_gpu_tile_noise_matches_the_cpu_metric_on_the_same_buffers")
    else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let never_cancel = AtomicBool::new(false);
    let spp = 16;
    for first_sample in [0_u32, 3] {
        let live = all_tiles();
        let mut run = backend
            .try_adaptive_resident(
                &gpu_scene(&camera, &planes, &material),
                &[],
                &SurfaceScene::IDEAL,
                spp,
            )
            .expect("a GPU backend starts a resident run on a small frame");
        let pass = Pass {
            first_sample,
            spp,
            live: live.clone(),
        };
        assert_eq!(run.pass(&pass, &never_cancel), GpuAccumulate::Done);
        let check = PendingCheck {
            live: live.clone(),
            samples: spp,
            odd_samples: odd_count(first_sample, spp),
        };
        let relative = run
            .tile_errors(&check, 1.0, NoiseMetric::RelativeLstar)
            .expect("the GPU measures the tiles");
        let absolute = run
            .tile_errors(&check, 1.0, NoiseMetric::AbsoluteDeltaE)
            .expect("the GPU measures the tiles");
        let (sum, half_b) = run.finish().expect("the run delivers its sums");

        for (metric, gpu) in [
            (NoiseMetric::RelativeLstar, &relative),
            (NoiseMetric::AbsoluteDeltaE, &absolute),
        ] {
            let cpu = live_tile_noises(
                &sum,
                &half_b,
                &live,
                (check.samples, check.odd_samples),
                1.0,
                metric,
            );
            assert_eq!(gpu.len(), cpu.len());
            let mut tight = 0_usize;
            let mut worst = 0.0_f32;
            for (slot, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
                let allowed = |rel: f32| rel.mul_add(c.abs(), NOISE_ABS_TOL);
                let rel = (g - c).abs() / c.abs().max(NOISE_ABS_TOL);
                worst = worst.max(rel);
                println!(
                    "offset {first_sample} {metric:?}: tile {slot}: GPU {g:.5} CPU {c:.5} rel {rel:.4}"
                );
                assert!(
                    (g - c).abs() <= allowed(NOISE_LOOSE_REL_TOL),
                    "offset {first_sample} {metric:?}: tile {slot}: GPU {g} vs CPU {c}"
                );
                if (g - c).abs() <= allowed(NOISE_REL_TOL) {
                    tight += 1;
                }
            }
            println!("offset {first_sample} {metric:?}: worst relative difference {worst:.4}");
            assert!(
                tight * 10 >= cpu.len() * 9,
                "offset {first_sample} {metric:?}: only {tight} of {} tiles are within {NOISE_REL_TOL}",
                cpu.len()
            );
        }
    }
}

/// A check with no live tile reads nothing and answers an empty list; a half with no samples is
/// unmeasurable and never converges.
#[test]
fn degenerate_checks_are_handled() {
    let Some(backend) = acquire_or_skip("degenerate_checks_are_handled") else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let never_cancel = AtomicBool::new(false);
    let mut run = backend
        .try_adaptive_resident(
            &gpu_scene(&camera, &planes, &material),
            &[],
            &SurfaceScene::IDEAL,
            4,
        )
        .expect("a GPU backend starts a resident run on a small frame");
    let pass = Pass {
        first_sample: 0,
        spp: 4,
        live: partial_tiles(),
    };
    assert_eq!(run.pass(&pass, &never_cancel), GpuAccumulate::Done);

    let none = PendingCheck {
        live: LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, Vec::new()),
        samples: 4,
        odd_samples: 2,
    };
    assert_eq!(
        run.tile_errors(&none, 1.0, NoiseMetric::RelativeLstar),
        Some(Vec::new())
    );

    // One sample at an even index: half-buffer B holds nothing, so nothing can be compared.
    let one_sided = PendingCheck {
        live: partial_tiles(),
        samples: 4,
        odd_samples: 0,
    };
    let noises = run
        .tile_errors(&one_sided, 1.0, NoiseMetric::AbsoluteDeltaE)
        .expect("the GPU answers");
    assert_eq!(noises.len(), PARTIAL.len());
    assert!(noises.iter().all(|&n| n > 1.0e30), "{noises:?}");
}

/// Only one run holds the resident frame at a time; dropping it frees the frame.
#[test]
fn one_run_holds_the_resident_frame_at_a_time() {
    let Some(backend) = acquire_or_skip("one_run_holds_the_resident_frame_at_a_time") else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let start = || {
        backend.try_adaptive_resident(
            &gpu_scene(&camera, &planes, &material),
            &[],
            &SurfaceScene::IDEAL,
            4,
        )
    };
    let first = start().expect("the first run starts");
    assert!(start().is_none(), "a second run must decline, not share");
    drop(first);
    assert!(start().is_some(), "the frame is free again after a drop");
}

/// A cancelled pass ends the run: later calls decline and no sums are delivered.
#[test]
fn a_cancelled_pass_ends_the_run() {
    let Some(backend) = acquire_or_skip("a_cancelled_pass_ends_the_run") else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let mut run = backend
        .try_adaptive_resident(
            &gpu_scene(&camera, &planes, &material),
            &[],
            &SurfaceScene::IDEAL,
            8,
        )
        .expect("a GPU backend starts a resident run on a small frame");
    let pass = Pass {
        first_sample: 0,
        spp: 8,
        live: all_tiles(),
    };
    let cancelled = AtomicBool::new(true);
    assert_eq!(run.pass(&pass, &cancelled), GpuAccumulate::Cancelled);
    let never_cancel = AtomicBool::new(false);
    assert_eq!(run.pass(&pass, &never_cancel), GpuAccumulate::Declined);
    assert!(run.finish().is_none());
}
