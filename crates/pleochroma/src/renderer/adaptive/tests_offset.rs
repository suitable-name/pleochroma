//! Tests of the beam part `offset` in the adaptive metric and of `AdaptiveRun::restricted_to`
//! (beam guide section 12.4), with synthetic samples instead of a path tracer.

use glam::Vec3;

use super::{
    AdaptiveRun, AdaptiveSettings, ConvergenceMap, LiveTiles, NoiseMetric, Pass,
    metric::{
        live_tile_noises, live_tile_noises_with_offset, pixel_error, pixel_error_with_offset,
        tile_measure, tile_measure_with_offset,
    },
    tests_metric::unit,
};

/// Frame of three 16-pixel tiles in a row.
const WIDTH: u32 = 48;
/// Frame height.
const HEIGHT: u32 = 16;
/// Pixels of the frame.
const PIXELS: usize = (WIDTH * HEIGHT) as usize;
/// Samples per pixel of the metric tests.
const SAMPLES: u32 = 32;
/// The odd-index samples among them.
const ODD: u32 = SAMPLES / 2;
/// The tolerance of the "offset equals a shifted total" comparison.
const SHIFT_TOLERANCE: f32 = 1.0e-3;

/// Absolute-metric settings: 8-sample passes, a minimum of 16, a cap of 64.
const fn settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.05,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: 16,
        max_spp: 64,
        pass_spp: 8,
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

/// Noise in every tile.
fn noisy_everywhere(x: u32, y: u32, sample: u32) -> Vec3 {
    Vec3::new(
        unit(y * WIDTH + x, sample),
        unit(y * WIDTH + x, sample + 1000),
        unit(y * WIDTH + x, sample + 2000),
    )
}

/// Traces `pass` with `value(x, y, sample)` into the buffers, as the pass contract says.
fn trace_pass(
    pass: &Pass,
    value: &dyn Fn(u32, u32, u32) -> Vec3,
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
) {
    for &tile in pass.live.indices() {
        let (x0, y0, x1, y1) = pass.live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = (y * WIDTH + x) as usize;
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

/// `SAMPLES` noisy samples of every pixel of the frame: `(sum, half_b)`.
fn noisy_buffers() -> (Vec<Vec3>, Vec<Vec3>) {
    let mut sum = vec![Vec3::ZERO; PIXELS];
    let mut half_b = vec![Vec3::ZERO; PIXELS];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let pixel = (y * WIDTH + x) as usize;
            for sample in 0..SAMPLES {
                let v = noisy_everywhere(x, y, sample);
                sum[pixel] += v;
                if !sample.is_multiple_of(2) {
                    half_b[pixel] += v;
                }
            }
        }
    }
    (sum, half_b)
}

/// The bit patterns of a list of floats.
fn float_bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The bit patterns of an image.
fn vec_bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

#[test]
fn a_pixel_error_without_offset_is_the_shipped_one_bit_for_bit() {
    for pixel in 0..64_u32 {
        let total = Vec3::new(
            4.0 * unit(pixel, 1),
            4.0 * unit(pixel, 2),
            4.0 * unit(pixel, 3),
        );
        let half = total * 0.5 * 0.2_f32.mul_add(unit(pixel, 4), 0.9);
        let old = pixel_error(total, half, SAMPLES, ODD, 1.0);
        let new = pixel_error_with_offset(total, half, (SAMPLES, ODD), 1.0, None);
        assert_eq!(
            old.map(|(e, l)| (e.to_bits(), l.to_bits())),
            new.map(|(e, l)| (e.to_bits(), l.to_bits()))
        );
        let zero = pixel_error_with_offset(total, half, (SAMPLES, ODD), 1.0, Some(Vec3::ZERO));
        assert_eq!(
            old.map(|(e, l)| (e.to_bits(), l.to_bits())),
            zero.map(|(e, l)| (e.to_bits(), l.to_bits()))
        );
    }
    assert_eq!(
        pixel_error_with_offset(Vec3::ONE, Vec3::ONE, (4, 4), 1.0, Some(Vec3::ONE)),
        None
    );
}

#[test]
fn an_offset_is_added_to_both_half_means() {
    // offset + mean equals the mean of the total shifted by offset * n (half_b by offset * n_b).
    let offset = Vec3::new(0.3, 0.2, 0.1);
    for pixel in 0..32_u32 {
        let total = Vec3::new(unit(pixel, 1), unit(pixel, 2), unit(pixel, 3)) * 8.0;
        let half = total * 0.5 * 0.2_f32.mul_add(unit(pixel, 4), 0.9);
        let with = pixel_error_with_offset(total, half, (SAMPLES, ODD), 1.0, Some(offset))
            .expect("both halves");
        let shifted = pixel_error(
            total + offset * SAMPLES as f32,
            half + offset * ODD as f32,
            SAMPLES,
            ODD,
            1.0,
        )
        .expect("both halves");
        assert!(
            (with.0 - shifted.0).abs() < SHIFT_TOLERANCE,
            "{with:?} {shifted:?}"
        );
        assert!(
            (with.1 - shifted.1).abs() < SHIFT_TOLERANCE,
            "{with:?} {shifted:?}"
        );
    }
}

#[test]
fn a_tile_measure_without_offset_or_with_zeros_is_the_shipped_one() {
    let (sum, half_b) = noisy_buffers();
    let zeros = vec![Vec3::ZERO; PIXELS];
    for rect in [
        (0, 0, 16, 16),
        (16, 0, 32, 16),
        (32, 0, 48, 16),
        (5, 3, 20, 9),
    ] {
        let old = tile_measure(&sum, &half_b, WIDTH, rect, SAMPLES, ODD, 1.0);
        let none = tile_measure_with_offset(&sum, &half_b, None, WIDTH, rect, (SAMPLES, ODD), 1.0);
        let zero = tile_measure_with_offset(
            &sum,
            &half_b,
            Some(&zeros),
            WIDTH,
            rect,
            (SAMPLES, ODD),
            1.0,
        );
        assert_eq!(
            old.max_pixel_error.to_bits(),
            none.max_pixel_error.to_bits()
        );
        assert_eq!(old.mean_lightness.to_bits(), none.mean_lightness.to_bits());
        assert_eq!(
            old.max_pixel_error.to_bits(),
            zero.max_pixel_error.to_bits()
        );
        assert_eq!(old.mean_lightness.to_bits(), zero.mean_lightness.to_bits());
    }
}

#[test]
fn an_offset_slice_that_does_not_cover_the_tile_makes_it_unmeasurable() {
    let (sum, half_b) = noisy_buffers();
    let short = vec![Vec3::ZERO; 10];
    let measure = tile_measure_with_offset(
        &sum,
        &half_b,
        Some(&short),
        WIDTH,
        (0, 0, 16, 16),
        (SAMPLES, ODD),
        1.0,
    );
    assert_eq!(measure.max_pixel_error, f32::INFINITY);
}

#[test]
fn the_live_tile_noises_without_offset_or_with_zeros_are_the_shipped_ones() {
    let (sum, half_b) = noisy_buffers();
    let zeros = vec![Vec3::ZERO; PIXELS];
    let live = LiveTiles::all(WIDTH, HEIGHT, 16);
    for metric in [NoiseMetric::AbsoluteDeltaE, NoiseMetric::RelativeLstar] {
        let old = live_tile_noises(&sum, &half_b, &live, (SAMPLES, ODD), 1.0, metric);
        let none =
            live_tile_noises_with_offset(&sum, &half_b, None, &live, (SAMPLES, ODD), 1.0, metric);
        let zero = live_tile_noises_with_offset(
            &sum,
            &half_b,
            Some(&zeros),
            &live,
            (SAMPLES, ODD),
            1.0,
            metric,
        );
        assert_eq!(old.len(), 3);
        assert_eq!(float_bits(&old), float_bits(&none));
        assert_eq!(float_bits(&old), float_bits(&zero));
    }
}

#[test]
fn a_real_offset_changes_the_noise() {
    let (sum, half_b) = noisy_buffers();
    let bright = vec![Vec3::splat(0.5); PIXELS];
    let live = LiveTiles::all(WIDTH, HEIGHT, 16);
    let plain = live_tile_noises(
        &sum,
        &half_b,
        &live,
        (SAMPLES, ODD),
        1.0,
        NoiseMetric::AbsoluteDeltaE,
    );
    let shifted = live_tile_noises_with_offset(
        &sum,
        &half_b,
        Some(&bright),
        &live,
        (SAMPLES, ODD),
        1.0,
        NoiseMetric::AbsoluteDeltaE,
    );
    assert_ne!(float_bits(&plain), float_bits(&shifted));
}

/// The closure that closes one pass of a driven run.
/// (Borrowing closures welcome: the alias carries a lifetime, so `dyn` is not `'static`.)
type FinishPass<'a> = dyn Fn(&mut AdaptiveRun, &[Vec3], &[Vec3]) + 'a;

/// A whole run of `value` over the frame, with `finish` closing each pass; returns the final
/// buffers and the run.
fn drive(
    mut run: AdaptiveRun,
    value: &dyn Fn(u32, u32, u32) -> Vec3,
    finish: &FinishPass<'_>,
) -> (AdaptiveRun, Vec<Vec3>, Vec<Vec3>) {
    let mut sum = vec![Vec3::ZERO; PIXELS];
    let mut half_b = vec![Vec3::ZERO; PIXELS];
    while let Some(pass) = run.next_pass() {
        trace_pass(&pass, value, &mut sum, &mut half_b);
        finish(&mut run, &sum, &half_b);
    }
    (run, sum, half_b)
}

#[test]
fn finish_pass_with_no_offset_or_zeros_decides_like_finish_pass() {
    let zeros = vec![Vec3::ZERO; PIXELS];
    let start = || AdaptiveRun::new(settings(), WIDTH, HEIGHT).expect("valid");
    let plain = drive(start(), &noisy_everywhere, &|run, sum, half| {
        run.finish_pass(sum, half, 1.0);
    });
    let none = drive(start(), &noisy_everywhere, &|run, sum, half| {
        run.finish_pass_with_offset(sum, half, 1.0, None);
    });
    let zero = drive(start(), &noisy_everywhere, &|run, sum, half| {
        run.finish_pass_with_offset(sum, half, 1.0, Some(&zeros));
    });
    for other in [&none, &zero] {
        assert_eq!(plain.0.map(), other.0.map());
        assert_eq!(plain.0.samples_done(), other.0.samples_done());
        assert_eq!(vec_bits(&plain.1), vec_bits(&other.1));
        assert_eq!(vec_bits(&plain.2), vec_bits(&other.2));
    }
}

#[test]
fn restricting_to_every_tile_is_the_frame_run() {
    let start = || AdaptiveRun::new(settings(), WIDTH, HEIGHT).expect("valid");
    let close =
        |run: &mut AdaptiveRun, sum: &[Vec3], half: &[Vec3]| run.finish_pass(sum, half, 1.0);
    let frame = drive(start(), &clean_left_noisy_right, &close);
    let all = LiveTiles::all(WIDTH, HEIGHT, 16);
    assert_eq!(start().restricted_to(&all).map(), start().map());
    let restricted = drive(start().restricted_to(&all), &clean_left_noisy_right, &close);
    assert_eq!(frame.0.map(), restricted.0.map());
    assert_eq!(frame.0.pixel_counts(), restricted.0.pixel_counts());
    assert_eq!(frame.0.samples_traced(), restricted.0.samples_traced());
    assert_eq!(vec_bits(&frame.1), vec_bits(&restricted.1));
    assert_eq!(vec_bits(&frame.2), vec_bits(&restricted.2));
}

#[test]
fn a_restricted_run_only_ever_sees_its_tiles_live() {
    let batch = LiveTiles::from_indices(WIDTH, HEIGHT, 16, vec![1]);
    let mut run = AdaptiveRun::new(settings(), WIDTH, HEIGHT)
        .expect("valid")
        .restricted_to(&batch);
    let mut seen = Vec::new();
    let (mut sum, mut half_b) = (vec![Vec3::ZERO; PIXELS], vec![Vec3::ZERO; PIXELS]);
    while let Some(pass) = run.next_pass() {
        seen.push(pass.live.indices().to_vec());
        trace_pass(&pass, &clean_left_noisy_right, &mut sum, &mut half_b);
        run.finish_pass(&sum, &half_b, 1.0);
    }
    assert_ne!(seen.len(), 0);
    assert!(seen.iter().all(|live| live == &[1]));
    // The other tiles never got a sample, and tile 1 stopped at the minimum.
    assert_eq!(run.pixel_spp(2, 2), 0);
    assert_eq!(run.pixel_spp(40, 2), 0);
    assert_eq!(run.pixel_spp(20, 2), settings().min_spp);
    assert_eq!(run.samples_traced(), 256 * u64::from(settings().min_spp));
    assert!(sum[..16].iter().all(|v| *v == Vec3::ZERO));
}

#[test]
fn dilation_in_a_restricted_run_reads_only_the_batchs_own_errors() {
    // Tiles 0 and 1 are clean, tile 2 is wild. In the frame run tile 1 is held back by tile 2's
    // error; the batch {0, 1} does not see tile 2, so both stop at the minimum.
    let close =
        |run: &mut AdaptiveRun, sum: &[Vec3], half: &[Vec3]| run.finish_pass(sum, half, 1.0);
    let frame = drive(
        AdaptiveRun::new(settings(), WIDTH, HEIGHT).expect("valid"),
        &clean_left_noisy_right,
        &close,
    );
    assert_eq!(
        frame.0.pixel_spp(20, 2),
        settings().max_spp,
        "held back by the neighbour"
    );
    let batch = LiveTiles::from_indices(WIDTH, HEIGHT, 16, vec![0, 1]);
    let restricted = drive(
        AdaptiveRun::new(settings(), WIDTH, HEIGHT)
            .expect("valid")
            .restricted_to(&batch),
        &clean_left_noisy_right,
        &close,
    );
    assert_eq!(restricted.0.pixel_spp(2, 2), settings().min_spp);
    assert_eq!(restricted.0.pixel_spp(20, 2), settings().min_spp);
    assert_eq!(restricted.0.pixel_spp(40, 2), 0);
    assert!(restricted.0.map().is_done());
}

#[test]
fn a_noisy_tile_of_the_batch_still_holds_its_batch_neighbour() {
    // Dilation inside the batch works as in the frame: tile 1 (clean) waits for tile 2 (noisy).
    let batch = LiveTiles::from_indices(WIDTH, HEIGHT, 16, vec![1, 2]);
    let restricted = drive(
        AdaptiveRun::new(settings(), WIDTH, HEIGHT)
            .expect("valid")
            .restricted_to(&batch),
        &clean_left_noisy_right,
        &|run, sum, half| run.finish_pass(sum, half, 1.0),
    );
    assert_eq!(restricted.0.pixel_spp(20, 2), settings().max_spp);
    assert_eq!(restricted.0.pixel_spp(2, 2), 0);
}

#[test]
fn live_tiles_of_another_frame_leave_nothing_live() {
    let other_frame = LiveTiles::all(WIDTH + 16, HEIGHT, 16);
    let mut run = AdaptiveRun::new(settings(), WIDTH, HEIGHT)
        .expect("valid")
        .restricted_to(&other_frame);
    assert!(run.next_pass().is_none());
    let other_tile = LiveTiles::all(WIDTH, HEIGHT, 8);
    let mut run = AdaptiveRun::new(settings(), WIDTH, HEIGHT)
        .expect("valid")
        .restricted_to(&other_tile);
    assert!(run.next_pass().is_none());
    let empty = LiveTiles::from_indices(WIDTH, HEIGHT, 16, Vec::new());
    let mut run = AdaptiveRun::new(settings(), WIDTH, HEIGHT)
        .expect("valid")
        .restricted_to(&empty);
    assert!(run.next_pass().is_none());
}

/// Reference for a restricted run: the full map seeded with the batch members.
fn seeded_full_map(width: u32, height: u32, members: &[u32], target: f32) -> ConvergenceMap {
    let mut map = ConvergenceMap::new(width, height, 16);
    let seeds: Vec<(u32, f32)> = (0_u32..)
        .take(map.states().len())
        .map(|index| {
            let member = members.binary_search(&index).is_ok();
            (index, if member { 2.0 * target } else { 0.0 })
        })
        .collect();
    map.record_errors(&seeds);
    map.converge(0, target);
    let reset: Vec<(u32, f32)> = members
        .iter()
        .map(|&index| (index, f32::INFINITY))
        .collect();
    map.record_errors(&reset);
    map
}

#[test]
fn a_sparse_restriction_equals_the_full_map_seeding() {
    // Six by four tiles; the batch has a corner, an edge pair, an interior pair and a gap.
    let (width, height) = (96, 64);
    let members = [0_u32, 1, 7, 8, 11, 23];
    let target = settings().target_noise;
    let mut full = seeded_full_map(width, height, &members, target);
    let batch = LiveTiles::from_indices(width, height, 16, members.to_vec());
    let by_new = AdaptiveRun::new_restricted(settings(), width, height, &batch).expect("valid");
    let by_restricted = AdaptiveRun::new(settings(), width, height)
        .expect("valid")
        .restricted_to(&batch);
    let mut sparse = by_new.map().clone();
    assert_eq!(by_new.map(), by_restricted.map());
    assert_eq!(&sparse, &full);
    assert_eq!(sparse.stored_tiles(), members.len());
    let rounds: [[f32; 6]; 3] = [
        [0.03, 0.04, 0.03, 0.2, 0.04, 0.03],
        [0.03, 0.04, 0.03, 0.04, 0.04, 0.3],
        [0.01, 0.01, 0.01, 0.01, 0.2, 0.01],
    ];
    for errors in rounds {
        for map in [&mut full, &mut sparse] {
            map.add_samples(8);
            let measured: Vec<(u32, f32)> = members.iter().copied().zip(errors).collect();
            map.record_errors(&measured);
            map.converge(8, target);
        }
        assert_eq!(&sparse, &full);
        assert_eq!(sparse.live_tiles(), full.live_tiles());
        assert_eq!(sparse.live_count(), full.live_count());
        assert_eq!(sparse.samples_traced(), full.samples_traced());
        for y in (0..height).step_by(5) {
            for x in (0..width).step_by(7) {
                assert_eq!(sparse.pixel_spp(x, y), full.pixel_spp(x, y));
            }
        }
    }
}

#[test]
fn a_restricted_run_over_a_large_frame_stores_only_its_batch() {
    // An 8K frame has 129 600 tiles of 16 px; one batch of 64 stores 64 states, not the frame.
    let (width, height) = (7680, 4320);
    let first = 1000_u32;
    let members: Vec<u32> = (first..first + 64).collect();
    let batch = LiveTiles::from_indices(width, height, 16, members);
    let mut run = AdaptiveRun::new_restricted(settings(), width, height, &batch).expect("valid");
    assert_eq!(run.map().tiles_x() * run.map().tiles_y(), 129_600);
    assert_eq!(run.map().stored_tiles(), 64);
    assert_eq!(run.map().live_count(), 64);
    let pass = run.next_pass().expect("the batch is live");
    assert_eq!(pass.live, batch);
    assert_eq!(pass.first_sample, 0);
    // A tile beside the batch and a far one are stopped and unsampled.
    assert_eq!(run.pixel_spp(0, 0), 0);
    assert_eq!(
        run.map().state(first + 100).map(|state| state.live),
        Some(false)
    );
    assert_eq!(run.map().state(129_600), None);
}
