//! Tests of the adaptive pass driver, with synthetic samples instead of a path tracer.

use glam::Vec3;

use super::{
    AdaptiveRun, AdaptiveSettings, AdaptiveSettingsError, NoiseMetric, Pass, tests_metric::unit,
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

/// Samples that are exactly 0.5 in the left two tiles and wild noise in the right one.
fn clean_left_noisy_right(x: u32, y: u32, sample: u32) -> Vec3 {
    if x < 32 {
        Vec3::splat(0.5)
    } else {
        Vec3::splat(unit(y * WIDTH + x, sample) * 4.0)
    }
}

/// Traces `pass` with `value(x, y, sample)` into the buffers, as the pass contract says.
fn trace_pass(
    pass: &Pass,
    width: u32,
    value: &dyn Fn(u32, u32, u32) -> Vec3,
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
) {
    for &tile in pass.live.indices() {
        let (x0, y0, x1, y1) = pass.live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = (y * width + x) as usize;
                for sample in pass.first_sample..pass.first_sample + pass.spp {
                    let v = value(x, y, sample);
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
fn an_invalid_setting_is_rejected_when_the_run_starts() {
    let bad = settings(64, 32, 128, 0.05);
    assert!(matches!(
        AdaptiveRun::new(bad, WIDTH, HEIGHT),
        Err(AdaptiveSettingsError::SampleCounts { .. })
    ));
}

#[test]
fn the_last_pass_is_shortened_so_the_total_never_exceeds_the_cap() {
    // One tile of noise that never meets the target; max 100 is not a multiple of 32.
    let mut run = AdaptiveRun::new(settings(32, 32, 100, 1e-6), 16, 16).expect("valid");
    let value = |x: u32, y: u32, s: u32| Vec3::splat(unit(y * 16 + x, s));
    let (mut sum, mut half) = (vec![Vec3::ZERO; 256], vec![Vec3::ZERO; 256]);
    let mut seen = Vec::new();
    while let Some(pass) = run.next_pass() {
        seen.push((pass.first_sample, pass.spp));
        trace_pass(&pass, 16, &value, &mut sum, &mut half);
        run.finish_pass(&sum, &half, 1.0);
    }
    assert_eq!(seen, vec![(0, 32), (32, 32), (64, 32), (96, 4)]);
    assert_eq!(run.samples_done(), 100);
    assert_eq!(run.pixel_spp(5, 5), 100);
    assert_eq!(run.samples_traced(), 100 * 256);
    assert!(run.next_pass().is_none());
}

#[test]
fn nothing_is_checked_before_the_minimum_samples() {
    // Exactly equal halves would stop at the first check; the first check is at min_spp.
    let mut run = AdaptiveRun::new(settings(8, 32, 64, 0.05), 16, 16).expect("valid");
    let value = |_: u32, _: u32, _: u32| Vec3::splat(0.5);
    let (mut sum, mut half) = (vec![Vec3::ZERO; 256], vec![Vec3::ZERO; 256]);
    let mut live_after_pass = Vec::new();
    while let Some(pass) = run.next_pass() {
        trace_pass(&pass, 16, &value, &mut sum, &mut half);
        run.finish_pass(&sum, &half, 1.0);
        live_after_pass.push(run.map().live_count());
    }
    assert_eq!(live_after_pass, vec![1, 1, 1, 0]);
    assert_eq!(run.samples_done(), 32);
}

#[test]
fn a_run_with_an_odd_first_sample_still_pairs_the_halves_correctly() {
    // Constant samples: the halves agree only if the driver counts the odd absolute indices
    // of `5..5 + n` right (it is `ceil(n / 2)` when the run starts on an odd index and n is
    // odd, here n = 15 with 8 odd indices).
    let run = AdaptiveRun::new(settings(5, 15, 60, 0.05), 16, 16)
        .expect("valid")
        .with_sample_offset(5);
    let value = |_: u32, _: u32, _: u32| Vec3::splat(0.5);
    let image = run.run_with(1.0, |pass, sum, half| {
        trace_pass(pass, 16, &value, sum, half);
    });
    assert!(image.counts.iter().all(|&c| c == 15), "stops at min_spp");
}

#[test]
fn asking_twice_gives_the_same_pass_and_finishing_without_a_pass_does_nothing() {
    let mut run = AdaptiveRun::new(settings(16, 16, 64, 0.05), 16, 16).expect("valid");
    let (sum, half) = (vec![Vec3::ZERO; 256], vec![Vec3::ZERO; 256]);
    run.finish_pass(&sum, &half, 1.0);
    assert_eq!(run.samples_done(), 0);
    let first = run.next_pass().expect("a pass");
    assert_eq!(run.next_pass(), Some(first));
    assert_eq!(run.samples_done(), 0);
}

#[test]
fn stopped_tiles_leave_the_passes_and_the_counts_follow() {
    let mut run = AdaptiveRun::new(settings(16, 16, 64, 0.05), WIDTH, HEIGHT).expect("valid");
    let pixels = (WIDTH * HEIGHT) as usize;
    let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    let mut live_per_pass = Vec::new();
    while let Some(pass) = run.next_pass() {
        live_per_pass.push(pass.live.indices().to_vec());
        trace_pass(&pass, WIDTH, &clean_left_noisy_right, &mut sum, &mut half);
        run.finish_pass(&sum, &half, 1.0);
    }
    // Tile 0 stops at the first check (16 samples); tile 1 is held by its noisy neighbour 2.
    assert_eq!(
        live_per_pass,
        vec![vec![0, 1, 2], vec![1, 2], vec![1, 2], vec![1, 2]]
    );

    let counts = run.pixel_counts();
    assert_eq!(counts[0], 16);
    assert_eq!(counts[15 + 15 * WIDTH as usize], 16);
    assert_eq!(counts[16], 64);
    assert_eq!(counts[47], 64);
    assert_eq!(run.samples_traced(), 16 * 256 + 64 * 512);
    assert_eq!(
        run.samples_traced(),
        counts.iter().map(|&c| u64::from(c)).sum::<u64>()
    );
    // The stopped tile's pixels were not touched after it stopped.
    assert_eq!(sum[0].to_array(), [8.0; 3]);
}

#[test]
fn resolve_divides_each_pixel_by_its_own_count() {
    let mut run = AdaptiveRun::new(settings(16, 16, 64, 0.05), WIDTH, HEIGHT).expect("valid");
    let pixels = (WIDTH * HEIGHT) as usize;
    let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    while let Some(pass) = run.next_pass() {
        trace_pass(&pass, WIDTH, &clean_left_noisy_right, &mut sum, &mut half);
        run.finish_pass(&sum, &half, 1.0);
    }
    let resolved = run.resolve(&sum);
    assert_eq!(resolved.len(), pixels);
    // Stopped, clean: the mean of its 16 samples, exactly.
    assert_eq!(resolved[3].to_array(), [0.5; 3]);
    // Still live: the mean of its own 64 samples (not of 16, and not of a frame-wide count).
    let (x, y) = (40_u32, 3_u32);
    let expected: f32 = (0..64).map(|s| unit(y * WIDTH + x, s) * 4.0).sum::<f32>() / 64.0;
    let got = resolved[(y * WIDTH + x) as usize].x;
    assert!((got - expected).abs() < 1e-4, "{got} vs {expected}");
}

#[test]
fn run_with_returns_the_image_the_manual_loop_builds() {
    let image = AdaptiveRun::new(settings(16, 16, 64, 0.05), WIDTH, HEIGHT)
        .expect("valid")
        .run_with(1.0, |pass, sum, half| {
            trace_pass(pass, WIDTH, &clean_left_noisy_right, sum, half);
        });
    assert_eq!((image.width, image.height), (WIDTH, HEIGHT));
    assert_eq!(image.counts[0], 16);
    assert_eq!(image.counts[47], 64);
    assert_eq!(image.samples_traced, 16 * 256 + 64 * 512);
    assert_eq!(image.pixels[3].to_array(), [0.5; 3]);
    // The odd-index half of a noisy pixel is a strict subset of its sum.
    let noisy = (3 * WIDTH + 40) as usize;
    let odd: f32 = (0_u32..64)
        .filter(|s| !s.is_multiple_of(2))
        .map(|s| unit(3 * WIDTH + 40, s) * 4.0)
        .sum();
    assert!((image.half_b[noisy].x - odd).abs() < 1e-3);
    assert!(image.half_b[noisy].x < image.sum[noisy].x);
}

#[test]
fn a_run_is_deterministic() {
    let render = || {
        AdaptiveRun::new(settings(16, 16, 64, 0.05), WIDTH, HEIGHT)
            .expect("valid")
            .run_with(1.0, |pass, sum, half| {
                trace_pass(pass, WIDTH, &clean_left_noisy_right, sum, half);
            })
    };
    assert_eq!(render(), render());
}

/// The pass ends of a run that never converges (one noisy tile), with or without growing passes.
fn pass_ends(settings: AdaptiveSettings, growing: bool) -> Vec<u32> {
    let run = AdaptiveRun::new(settings, 16, 16).expect("valid");
    let mut run = if growing {
        run.with_growing_passes()
    } else {
        run
    };
    let value = |x: u32, y: u32, s: u32| Vec3::splat(unit(y * 16 + x, s));
    let (mut sum, mut half) = (vec![Vec3::ZERO; 256], vec![Vec3::ZERO; 256]);
    let mut ends = Vec::new();
    while let Some(pass) = run.next_pass() {
        assert!(pass.spp <= run.largest_pass(), "pass {} too long", pass.spp);
        trace_pass(&pass, 16, &value, &mut sum, &mut half);
        run.finish_pass(&sum, &half, 1.0);
        ends.push(run.samples_done());
    }
    ends
}

#[test]
fn growing_passes_double_to_the_minimum_then_take_an_eighth_of_an_octave() {
    let mut expected: Vec<u32> = vec![16, 32, 64, 128, 256, 512];
    let mut done = 512_u32;
    for (step, until) in [(64, 1024), (128, 2048), (256, 4096), (512, 8192)] {
        while done < until {
            done += step;
            expected.push(done);
        }
    }
    let ends = pass_ends(settings(16, 512, 8192, 1e-6), true);
    assert_eq!(ends.len(), 38);
    assert_eq!(ends, expected);
}

#[test]
fn growing_passes_with_a_pass_as_long_as_the_minimum() {
    let mut expected: Vec<u32> = (1..=16).map(|k| k * 64).collect();
    for (step, until) in [(128, 2048), (256, 4096), (512, 8192)] {
        let mut done = *expected.last().expect("not empty");
        while done < until {
            done += step;
            expected.push(done);
        }
    }
    assert_eq!(pass_ends(settings(64, 64, 8192, 1e-6), true), expected);
}

#[test]
fn growing_passes_stay_multiples_of_an_odd_pass_size_and_end_at_the_cap() {
    let ends = pass_ends(settings(5, 15, 60, 1e-6), true);
    let mut before = 0;
    for &end in &ends {
        assert_eq!((end - before) % 5, 0, "pass ending at {end}");
        before = end;
    }
    assert_eq!(ends.last(), Some(&60));
}

#[test]
fn without_growing_passes_every_pass_is_pass_spp_long() {
    let ends = pass_ends(settings(16, 512, 8192, 1e-6), false);
    assert_eq!(ends.len(), 512);
    assert!(ends.iter().zip(1_u32..).all(|(&end, k)| end == 16 * k));
}
