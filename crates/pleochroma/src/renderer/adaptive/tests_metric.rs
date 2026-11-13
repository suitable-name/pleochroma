//! Tests of the adaptive-sampling error metric.

use glam::Vec3;

use super::{
    LiveTiles, NoiseMetric,
    metric::{L_FLOOR, TileMeasure, live_tile_noises, pixel_error, tile_measure, tile_noise},
};
use crate::{
    color::body_color::{delta_e_2000, srgb_to_lab},
    renderer::tonemap::{tonemap_pixel, tonemap_pixel_unquantized},
};

/// A deterministic value in `[0, 1)` for `(pixel, sample)`.
pub(super) fn unit(pixel: u32, sample: u32) -> f32 {
    let mut h = ((u64::from(pixel) << 32) | u64::from(sample)).wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    f32::from(u16::try_from(h >> 48).unwrap_or(0)) / 65_536.0
}

/// The Lab of a mean after the display tone-map, computed here independently of the metric.
fn lab(mean: Vec3) -> [f64; 3] {
    let [r, g, b] = tonemap_pixel_unquantized(mean, 1.0);
    srgb_to_lab([f64::from(r), f64::from(g), f64::from(b)])
}

#[test]
fn the_unquantized_tone_map_rounds_to_the_display_bytes() {
    for index in 0..512_u32 {
        let mean = Vec3::new(
            4.0 * unit(index, 0),
            4.0 * unit(index, 1),
            4.0 * unit(index, 2),
        );
        let bytes = tonemap_pixel(mean, 1.0);
        let float = tonemap_pixel_unquantized(mean, 1.0);
        for channel in 0..3 {
            let rounded = float[channel].mul_add(255.0, 0.5).floor();
            assert!(
                (rounded - f32::from(bytes[channel])).abs() < 0.5,
                "mean {mean:?} channel {channel}"
            );
        }
    }
}

#[test]
fn noise_below_one_display_step_still_counts() {
    // Two half means one hundredth of a display step apart in a dark gray.
    let mean_a = Vec3::splat(0.02);
    let mean_b = mean_a * 1.0005;
    let (error, _) = pixel_error(mean_a + mean_b, mean_b, 2, 1, 1.0).expect("both halves");
    assert_eq!(tonemap_pixel(mean_a, 1.0), tonemap_pixel(mean_b, 1.0));
    assert!(error > 0.0, "error {error}");
}

/// `n` noisy samples of every pixel of a `pixels`-pixel frame: `(sum, half_b)` with `half_b`
/// holding the odd sample indices.
fn noisy_buffers(pixels: u32, n: u32) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut sum = vec![Vec3::ZERO; pixels as usize];
    let mut half = vec![Vec3::ZERO; pixels as usize];
    for pixel in 0..pixels {
        for sample in 0..n {
            let value = Vec3::new(0.8, 1.0, 0.9) * (0.5 + unit(pixel, sample));
            sum[pixel as usize] += value;
            if !sample.is_multiple_of(2) {
                half[pixel as usize] += value;
            }
        }
    }
    (sum, half)
}

#[test]
fn the_threaded_check_equals_the_serial_loop_bit_for_bit() {
    // 1024 tiles, enough for the check to leave the calling thread on a multi-core machine.
    let (width, height, tile) = (512_u32, 512_u32, 16_u32);
    let (sum, half) = noisy_buffers(width * height, 4);
    let live = LiveTiles::all(width, height, tile);
    for metric in [NoiseMetric::RelativeLstar, NoiseMetric::AbsoluteDeltaE] {
        let threaded = live_tile_noises(&sum, &half, &live, (4, 2), 1.0, metric);
        let serial: Vec<f32> = live
            .indices()
            .iter()
            .map(|&index| {
                let rect = live.tile_rect(index);
                tile_noise(&tile_measure(&sum, &half, width, rect, 4, 2, 1.0), metric)
            })
            .collect();
        assert_eq!(threaded.len(), serial.len());
        for (slot, (a, b)) in threaded.iter().zip(&serial).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "tile slot {slot}: {a} vs {b}");
        }
    }
}

#[test]
fn the_pixel_error_is_half_the_delta_e_of_the_two_half_means() {
    // Dyadic values, so the sums and divisions are exact and the display rounding agrees.
    let mean_a = Vec3::new(0.5, 0.5, 0.25);
    let mean_b = Vec3::new(0.75, 0.5, 0.5);
    // Even and odd counts: A holds `n - n_b` samples.
    for (n, n_b, a_samples, b_samples) in [(8_u32, 4_u32, 4.0_f32, 4.0_f32), (7, 3, 4.0, 3.0)] {
        let half_b = mean_b * b_samples;
        let total = mean_a * a_samples + half_b;
        let (error, lightness) = pixel_error(total, half_b, n, n_b, 1.0).expect("both halves");
        let expected = delta_e_2000(lab(mean_a), lab(mean_b)) / 2.0;
        assert!(
            (f64::from(error) - expected).abs() < 1e-3,
            "n {n}: {error} vs {expected}"
        );
        let expected_l = f64::midpoint(lab(mean_a)[0], lab(mean_b)[0]);
        assert!((f64::from(lightness) - expected_l).abs() < 1e-3);
    }
}

#[test]
fn equal_halves_have_no_error() {
    let half_b = Vec3::splat(2.0);
    let total = Vec3::splat(4.0);
    let (error, _) = pixel_error(total, half_b, 8, 4, 1.0).expect("both halves");
    assert!(error.abs() < f32::EPSILON);
}

#[test]
fn a_pixel_with_an_empty_half_has_no_error_value() {
    assert!(pixel_error(Vec3::ONE, Vec3::ZERO, 1, 0, 1.0).is_none());
    assert!(pixel_error(Vec3::ONE, Vec3::ONE, 1, 1, 1.0).is_none());
    // A tile with such pixels cannot converge.
    let measure = tile_measure(&[Vec3::ONE], &[Vec3::ZERO], 1, (0, 0, 1, 1), 1, 0, 1.0);
    assert!(measure.max_pixel_error.is_infinite());
    assert!(tile_noise(&measure, NoiseMetric::AbsoluteDeltaE).is_infinite());
}

#[test]
fn the_tile_error_is_the_maximum_pixel_error_not_the_mean() {
    // A 16 x 16 tile of agreeing halves with one pixel whose halves disagree.
    let (mut sum, mut half) = (vec![Vec3::splat(8.0); 256], vec![Vec3::splat(4.0); 256]);
    let glint = 5 * 16 + 9;
    half[glint] = Vec3::splat(3.0);
    sum[glint] = Vec3::splat(8.0);
    let (glint_error, _) = pixel_error(sum[glint], half[glint], 8, 4, 1.0).expect("both halves");
    assert!(glint_error > 1.0, "the premise: a visible disagreement");

    let measure = tile_measure(&sum, &half, 16, (0, 0, 16, 16), 8, 4, 1.0);
    assert_eq!(measure.max_pixel_error.to_bits(), glint_error.to_bits());
    // The mean over the tile would be 1/256 of that.
    assert!(measure.max_pixel_error > 100.0 * (glint_error / 256.0));
}

#[test]
fn the_tile_rectangle_limits_the_measurement() {
    let (mut sum, mut half) = (vec![Vec3::splat(8.0); 64], vec![Vec3::splat(4.0); 64]);
    half[7 * 8 + 7] = Vec3::splat(3.0);
    sum[7 * 8 + 7] = Vec3::splat(8.0);
    let clean = tile_measure(&sum, &half, 8, (0, 0, 4, 4), 8, 4, 1.0);
    let dirty = tile_measure(&sum, &half, 8, (4, 4, 8, 8), 8, 4, 1.0);
    assert!(clean.max_pixel_error.abs() < f32::EPSILON);
    assert!(dirty.max_pixel_error > 0.5);
}

#[test]
fn the_relative_metric_floors_the_lightness_so_black_does_not_divide_by_zero() {
    let black = tile_measure(
        &[Vec3::ZERO; 4],
        &[Vec3::ZERO; 4],
        2,
        (0, 0, 2, 2),
        8,
        4,
        1.0,
    );
    assert!(black.mean_lightness.abs() < 1e-3);
    assert!(black.max_pixel_error.abs() < f32::EPSILON);
    let noise = tile_noise(&black, NoiseMetric::RelativeLstar);
    assert!(noise.is_finite());
    assert!(noise.abs() < f32::EPSILON);

    let dark = TileMeasure {
        max_pixel_error: 0.1,
        mean_lightness: 0.5,
    };
    assert_eq!(
        tile_noise(&dark, NoiseMetric::RelativeLstar).to_bits(),
        (0.1_f32 / L_FLOOR).to_bits()
    );
    let bright = TileMeasure {
        max_pixel_error: 0.1,
        mean_lightness: 50.0,
    };
    assert_eq!(
        tile_noise(&bright, NoiseMetric::RelativeLstar).to_bits(),
        (0.1_f32 / 50.0).to_bits()
    );
}

#[test]
fn the_absolute_metric_is_the_worst_pixel_error() {
    let measure = TileMeasure {
        max_pixel_error: 1.25,
        mean_lightness: 40.0,
    };
    assert_eq!(
        tile_noise(&measure, NoiseMetric::AbsoluteDeltaE).to_bits(),
        1.25_f32.to_bits()
    );
}

#[test]
fn the_half_buffer_error_falls_with_the_square_root_of_the_samples() {
    // Seeded noise of known spread: the error of a 4x larger sample set is about half.
    let rect = (0, 0, 16, 16);
    let (sum_small, half_small) = noisy_buffers(256, 64);
    let (sum_large, half_large) = noisy_buffers(256, 256);
    let small = tile_measure(&sum_small, &half_small, 16, rect, 64, 32, 1.0);
    let large = tile_measure(&sum_large, &half_large, 16, rect, 256, 128, 1.0);
    let ratio = small.max_pixel_error / large.max_pixel_error;
    assert!(
        (1.5..2.7).contains(&ratio),
        "ratio {ratio} (small {}, large {})",
        small.max_pixel_error,
        large.max_pixel_error
    );
    // Both look at the same mean lightness.
    assert!((small.mean_lightness - large.mean_lightness).abs() < 3.0);
}
