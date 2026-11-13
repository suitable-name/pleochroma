//! Tests of the tile-error entry of the adaptive pass driver (`pending_check`,
//! `finish_pass_with_tile_errors`), which the GPU-resident run reports through.
//!
//! No GPU is needed: the noises are the CPU's own, or synthetic.

use glam::Vec3;

use super::{
    AdaptiveRun, AdaptiveSettings, NoiseMetric, Pass, metric::live_tile_noises, tests_metric::unit,
};

/// Frame of three 16-pixel tiles in a row.
const WIDTH: u32 = 48;
const HEIGHT: u32 = 16;

/// Absolute-metric settings with the given sample counts.
const fn settings(
    pass_spp: u32,
    min_spp: u32,
    max_spp: u32,
    target_noise: f32,
) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp,
        max_spp,
        pass_spp,
        tile: 16,
    }
}

/// Exactly 0.5 in the left two tiles, wild noise in the right one.
fn clean_left_noisy_right(x: u32, y: u32, sample: u32) -> Vec3 {
    if x < 32 {
        Vec3::splat(0.5)
    } else {
        Vec3::splat(unit(y * WIDTH + x, sample) * 4.0)
    }
}

/// Adds `pass` into the full-frame buffers as the pass contract says.
fn trace_pass(pass: &Pass, sum: &mut [Vec3], half_b: &mut [Vec3]) {
    for &tile in pass.live.indices() {
        let (x0, y0, x1, y1) = pass.live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = (y * WIDTH + x) as usize;
                for sample in pass.first_sample..pass.first_sample + pass.spp {
                    let v = clean_left_noisy_right(x, y, sample);
                    sum[pixel] += v;
                    if !sample.is_multiple_of(2) {
                        half_b[pixel] += v;
                    }
                }
            }
        }
    }
}

#[test]
fn feeding_the_cpu_tile_errors_equals_finish_pass() {
    for offset in [0_u32, 1, 5] {
        let make = || {
            AdaptiveRun::new(settings(8, 16, 64, 0.05), WIDTH, HEIGHT)
                .expect("valid")
                .with_sample_offset(offset)
        };
        let (mut by_buffers, mut by_errors) = (make(), make());
        let pixels = (WIDTH * HEIGHT) as usize;
        let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
        let mut passes = 0;
        loop {
            let a = by_buffers.next_pass();
            let b = by_errors.next_pass();
            assert_eq!(a, b, "offset {offset}, pass {passes}");
            let Some(pass) = a else { break };
            trace_pass(&pass, &mut sum, &mut half);

            let noises = by_errors.pending_check().map(|check| {
                live_tile_noises(
                    &sum,
                    &half,
                    &check.live,
                    (check.samples, check.odd_samples),
                    1.0,
                    NoiseMetric::AbsoluteDeltaE,
                )
            });
            by_buffers.finish_pass(&sum, &half, 1.0);
            by_errors.finish_pass_with_tile_errors(&noises.unwrap_or_default());

            assert_eq!(
                by_buffers.map(),
                by_errors.map(),
                "offset {offset}, pass {passes}"
            );
            assert_eq!(by_buffers.samples_done(), by_errors.samples_done());
            passes += 1;
        }
        assert!(passes > 2, "the run takes several passes");
        assert!(
            by_buffers.map().live_count() < 3,
            "some tile stopped, so the comparison covers a decision"
        );
        assert_eq!(by_buffers.pixel_counts(), by_errors.pixel_counts());
        assert_eq!(by_buffers.samples_traced(), by_errors.samples_traced());
    }
}

#[test]
fn no_check_is_pending_below_the_minimum_and_the_counts_follow_the_offset() {
    let mut run = AdaptiveRun::new(settings(8, 24, 64, 0.3), WIDTH, HEIGHT)
        .expect("valid")
        .with_sample_offset(3);
    assert_eq!(run.pending_check(), None, "no pass is pending yet");

    let mut checks = Vec::new();
    while run.next_pass().is_some() {
        checks.push(
            run.pending_check()
                .map(|c| (c.live.indices().len(), c.samples, c.odd_samples)),
        );
        run.finish_pass_with_tile_errors(&[1.0e9, 1.0e9, 1.0e9]);
    }
    // Passes of 8 samples from the absolute index 3: the odd indices among 3..11 are
    // 3, 5, 7, 9 (four), and so on; the check starts at 24 samples.
    assert_eq!(
        checks,
        vec![
            None,
            None,
            Some((3, 24, 12)),
            Some((3, 32, 16)),
            Some((3, 40, 20)),
            Some((3, 48, 24)),
            Some((3, 56, 28)),
            Some((3, 64, 32)),
        ]
    );
}

#[test]
fn a_pass_closed_without_errors_below_the_minimum_changes_nothing_else() {
    let mut run = AdaptiveRun::new(settings(8, 16, 64, 0.3), WIDTH, HEIGHT).expect("valid");
    assert!(run.next_pass().is_some());
    run.finish_pass_with_tile_errors(&[]);
    assert_eq!(run.samples_done(), 8);
    assert_eq!(run.map().live_count(), 3);
    // Nothing is pending any more: a second report is a no-op.
    run.finish_pass_with_tile_errors(&[0.0, 0.0, 0.0]);
    assert_eq!(run.samples_done(), 8);
}

#[test]
fn synthetic_tile_errors_stop_tiles_by_the_shared_decision() {
    let mut run = AdaptiveRun::new(settings(8, 8, 64, 0.05), WIDTH, HEIGHT).expect("valid");

    // Pass 1: the middle tile is noisy, so its neighbours (2x target = 0.1 exceeded) stay.
    assert!(run.next_pass().is_some());
    run.finish_pass_with_tile_errors(&[0.01, 0.2, 0.01]);
    assert_eq!(run.map().live_count(), 3);

    // Pass 2: everything is calm, every tile stops.
    assert!(run.next_pass().is_some());
    run.finish_pass_with_tile_errors(&[0.01, 0.01, 0.01]);
    assert_eq!(run.map().live_count(), 0);
    assert!(run.next_pass().is_none());
    assert_eq!(run.samples_done(), 16);
}

#[test]
fn a_missing_error_counts_as_unmeasured_and_never_converges() {
    let mut run = AdaptiveRun::new(settings(8, 8, 64, 0.05), WIDTH, HEIGHT).expect("valid");
    assert!(run.next_pass().is_some());
    // Only the first tile reports; the other two are unmeasured and keep everything alive
    // (the first tile's neighbour is unmeasured).
    run.finish_pass_with_tile_errors(&[0.0]);
    assert_eq!(run.map().live_count(), 3);
}

#[test]
fn the_cpu_reference_noises_follow_the_live_tile_order() {
    let live = super::LiveTiles::from_indices(WIDTH, HEIGHT, 16, vec![2, 0]);
    let pixels = (WIDTH * HEIGHT) as usize;
    let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    let pass = Pass {
        first_sample: 0,
        spp: 8,
        live: live.clone(),
    };
    trace_pass(&pass, &mut sum, &mut half);
    let noises = live_tile_noises(&sum, &half, &live, (8, 4), 1.0, NoiseMetric::AbsoluteDeltaE);
    assert_eq!(noises.len(), 2);
    assert!(
        noises[0] < 1e-3,
        "tile 0 is constant, so its halves agree: {noises:?}"
    );
    assert!(noises[1] > 0.05, "tile 2 is noisy: {noises:?}");
}
