//! The adaptive-sampling error metric (guide section 6.3).
//!
//! Each pixel holds the summed samples (`sum`) and the summed odd-index samples (`half_b`). The
//! other half is `A = sum - half_b`. Both half means are tone-mapped with the render's own
//! sRGB tone-map, converted to CIE L\*a\*b\* (D65) and compared with CIEDE2000; half of that
//! difference estimates the error of the full mean. A tile's error is the **maximum** pixel
//! error, so one glinting pixel keeps the whole tile alive.
//!
//! The display step is the export's sRGB conversion **before** its 8-bit rounding
//! (`tonemap_pixel_unquantized`): rounding both halves to display codes would read noise below
//! one code value as zero and stop dark or flat tiles too early.

use glam::Vec3;

use super::{LiveTiles, NoiseMetric};
use crate::{
    color::body_color::{delta_e_2000, srgb_to_lab},
    renderer::tonemap::tonemap_pixel_unquantized,
};

/// Lowest mean `L*` the relative metric divides by, so black areas do not divide by zero.
pub const L_FLOOR: f32 = 5.0;

/// Live tiles per thread below which the convergence check stays on the calling thread: a thread
/// costs more than the tiles it would measure.
const PARALLEL_CHECK_MIN_TILES: usize = 64;

/// What one tile measured: its worst pixel error and its mean lightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileMeasure {
    /// Largest pixel error (`Delta E / 2`) in the tile; infinite when a half has no samples.
    pub max_pixel_error: f32,
    /// Mean `L*` of the tile's pixels, from the average of the two half means.
    pub mean_lightness: f32,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the display conversion works in f32; the f64 value is a Delta E or L* of normal size"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

#[expect(
    clippy::cast_precision_loss,
    reason = "sample counts are far below 2^24, where f32 is exact"
)]
pub(super) const fn count_f32(count: u32) -> f32 {
    count as f32
}

/// `L*a*b*` of a mean XYZ value after the render's tone-map (`scale` multiplies it first).
fn display_lab(mean: Vec3, scale: f32) -> [f64; 3] {
    let [r, g, b] = tonemap_pixel_unquantized(mean, scale);
    srgb_to_lab([f64::from(r), f64::from(g), f64::from(b)])
}

/// Error and lightness of one pixel; `None` when either half has no samples.
///
/// `total` is the pixel's summed XYZ over `n` samples, `half_b` the sum of the `n_b` samples with
/// an odd absolute index, and `scale` the display multiplier applied to the means before the
/// tone-map.
#[must_use]
pub fn pixel_error(total: Vec3, half_b: Vec3, n: u32, n_b: u32, scale: f32) -> Option<(f32, f32)> {
    pixel_error_with_offset(total, half_b, (n, n_b), scale, None)
}

/// [`pixel_error`] with a constant part added to both half means (beam guide section 12.4).
///
/// `offset` is the pixel's beam part: it is the same in every sample, so it is added once to each
/// half **mean** before the tone-map, `offset + mean_a` against `offset + mean_b`, and the noise
/// check judges the total. With `None` the result is [`pixel_error`]'s, bit for bit. `counts` is
/// `(n, n_b)` as in [`live_tile_noises`].
#[must_use]
pub fn pixel_error_with_offset(
    total: Vec3,
    half_b: Vec3,
    counts: (u32, u32),
    scale: f32,
    offset: Option<Vec3>,
) -> Option<(f32, f32)> {
    let (n, n_b) = counts;
    let n_a = n.saturating_sub(n_b);
    if n_a == 0 || n_b == 0 {
        return None;
    }
    let mut mean_a = (total - half_b) / count_f32(n_a);
    let mut mean_b = half_b / count_f32(n_b);
    if let Some(beam) = offset {
        mean_a += beam;
        mean_b += beam;
    }
    let lab_a = display_lab(mean_a, scale);
    let lab_b = display_lab(mean_b, scale);
    let error = delta_e_2000(lab_a, lab_b) / 2.0;
    let lightness = f64::midpoint(lab_a[0], lab_b[0]);
    Some((to_f32(error), to_f32(lightness)))
}

/// Measures one tile: the maximum pixel error and the mean lightness.
///
/// `rect` is the tile's pixel rectangle `(x0, y0, x1, y1)` (end-exclusive) in a frame `width`
/// pixels wide; `sum` and `half_b` are the full-frame buffers. A pixel the buffers do not cover
/// makes the tile unmeasurable (infinite error), so it never converges by accident.
#[must_use]
pub fn tile_measure(
    sum: &[Vec3],
    half_b: &[Vec3],
    width: u32,
    rect: (u32, u32, u32, u32),
    n: u32,
    n_b: u32,
    scale: f32,
) -> TileMeasure {
    tile_measure_with_offset(sum, half_b, None, width, rect, (n, n_b), scale)
}

/// [`tile_measure`] with the beam part `offset` inside the pixel error (beam guide section 12.4).
///
/// `offset`, when given, is laid out like `sum` (one entry per pixel of the same `width`-wide
/// buffer); a pixel it does not cover makes the tile unmeasurable. `counts` is `(n, n_b)`. With
/// `None` the result is [`tile_measure`]'s, bit for bit.
#[must_use]
pub fn tile_measure_with_offset(
    sum: &[Vec3],
    half_b: &[Vec3],
    offset: Option<&[Vec3]>,
    width: u32,
    rect: (u32, u32, u32, u32),
    counts: (u32, u32),
    scale: f32,
) -> TileMeasure {
    let unmeasurable = TileMeasure {
        max_pixel_error: f32::INFINITY,
        mean_lightness: 0.0,
    };
    let (x0, y0, x1, y1) = rect;
    let mut max_error = 0.0_f32;
    let mut lightness_sum = 0.0_f64;
    let mut pixels = 0_u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let index = y as usize * width as usize + x as usize;
            let (Some(&total), Some(&half)) = (sum.get(index), half_b.get(index)) else {
                return unmeasurable;
            };
            let beam = match offset {
                None => None,
                Some(values) => match values.get(index) {
                    Some(&value) => Some(value),
                    None => return unmeasurable,
                },
            };
            let Some((error, lightness)) =
                pixel_error_with_offset(total, half, counts, scale, beam)
            else {
                return unmeasurable;
            };
            max_error = max_error.max(error);
            lightness_sum += f64::from(lightness);
            pixels += 1;
        }
    }
    if pixels == 0 {
        return unmeasurable;
    }
    TileMeasure {
        max_pixel_error: max_error,
        mean_lightness: to_f32(lightness_sum / f64::from(pixels)),
    }
}

/// The noise of every live tile of `live` after a pass, in the order of `live.indices()`.
///
/// `counts` is `(n, n_b)`: the samples every live pixel holds and how many of them had an odd
/// absolute index. `sum` and `half_b` are the full-frame buffers. This is the CPU reference of
/// the GPU reduction (`adaptive_tile_error.wgsl`), and what [`super::AdaptiveRun::finish_pass`]
/// feeds its decision with.
#[must_use]
pub fn live_tile_noises(
    sum: &[Vec3],
    half_b: &[Vec3],
    live: &LiveTiles,
    counts: (u32, u32),
    scale: f32,
    metric: NoiseMetric,
) -> Vec<f32> {
    live_tile_noises_with_offset(sum, half_b, None, live, counts, scale, metric)
}

/// [`live_tile_noises`] with the beam part `offset` inside the metric (beam guide section 12.4).
///
/// `offset`, when given, is laid out like `sum` and `half_b` (full-frame buffers). With `None`
/// every noise is [`live_tile_noises`]'s, bit for bit.
#[must_use]
pub fn live_tile_noises_with_offset(
    sum: &[Vec3],
    half_b: &[Vec3],
    offset: Option<&[Vec3]>,
    live: &LiveTiles,
    counts: (u32, u32),
    scale: f32,
    metric: NoiseMetric,
) -> Vec<f32> {
    let indices = live.indices();
    let noise_of = |index: u32| {
        let rect = live.tile_rect(index);
        let measure =
            tile_measure_with_offset(sum, half_b, offset, live.width(), rect, counts, scale);
        tile_noise(&measure, metric)
    };
    // A tile's noise depends on its own pixels alone, so the tiles are measured on several threads
    // and written to their own slots: the result is the serial one, bit for bit. Without threads
    // (wasm32 reports no parallelism) or with few tiles the calling thread does it all.
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(indices.len() / PARALLEL_CHECK_MIN_TILES)
        .max(1);
    if threads == 1 {
        return indices.iter().map(|&index| noise_of(index)).collect();
    }
    let per_thread = indices.len().div_ceil(threads);
    let mut noises = vec![0.0_f32; indices.len()];
    let noise_of = &noise_of;
    std::thread::scope(|scope| {
        for (tiles, slots) in indices
            .chunks(per_thread)
            .zip(noises.chunks_mut(per_thread))
        {
            scope.spawn(move || {
                for (slot, &index) in slots.iter_mut().zip(tiles) {
                    *slot = noise_of(index);
                }
            });
        }
    });
    noises
}

/// The tile's error in the unit of `metric`, to be compared with the target noise.
///
/// The relative metric divides by `max(mean L*, L_FLOOR)`; the absolute one is the worst pixel
/// error itself.
#[must_use]
pub fn tile_noise(measure: &TileMeasure, metric: NoiseMetric) -> f32 {
    match metric {
        NoiseMetric::RelativeLstar => measure.max_pixel_error / measure.mean_lightness.max(L_FLOOR),
        NoiseMetric::AbsoluteDeltaE => measure.max_pixel_error,
    }
}
