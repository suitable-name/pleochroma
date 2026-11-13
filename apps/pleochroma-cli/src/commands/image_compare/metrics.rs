//! The numbers `image-compare` reports, measured on two or three display images.
//!
//! Everything is a pure function of the pixels: sums run in pixel order, percentiles are
//! nearest-rank on sorted values, and no hash map is iterated.

use super::display::{DisplayImage, parallel_map};
use pleochroma::color::delta_e_2000;

/// The edge of a tile of `worst_tile_de`, in pixels; tiles at the right and bottom edges may be
/// smaller.
pub(super) const TILE: usize = 16;

/// The lowest threshold of `changed_area_pct`: a Delta E of 1 is about one just-noticeable step.
const MIN_CHANGED_DE: f64 = 1.0;

/// A glint is at least this many times brighter than the median of its neighbourhood.
const GLINT_RATIO: f32 = 4.0;

/// The glint percentile of the display luminance, as a fraction (999 / 1000).
const GLINT_PERCENTILE: (usize, usize) = (999, 1000);

/// The 99th percentile, as a fraction (99 / 100).
const P99: (usize, usize) = (99, 100);

/// Half the side of the glint neighbourhood (a 5 x 5 window).
const WINDOW_RADIUS: usize = 2;

/// The number of pixels in the glint neighbourhood.
const WINDOW_LEN: usize = (2 * WINDOW_RADIUS + 1) * (2 * WINDOW_RADIUS + 1);

/// The Delta E between two references of the same scene: what pure noise looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NoiseFloor {
    /// The mean Delta E over all pixels.
    pub mean: f64,
    /// The 99th percentile of the Delta E.
    pub p99: f64,
}

/// Everything `image-compare` reports.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Metrics {
    /// Reference against second reference; `None` without `--ref2`.
    pub noise_floor: Option<NoiseFloor>,
    /// The Delta E above which a pixel counts as changed.
    pub changed_threshold_de: f64,
    /// The percentage of pixels whose Delta E (reference, run) is above the threshold.
    pub changed_area_pct: f64,
    /// The largest tile-mean Delta E (reference, run).
    pub worst_tile_de: f64,
    /// The share of the reference's glint pixels that are glint pixels in the run; `None` when the
    /// reference has no glints.
    pub glint_recall: Option<f64>,
    /// The sum of the run's per-pixel sample counts.
    pub samples_used: u64,
    /// The largest per-pixel sample count of the run.
    pub max_spp: u32,
    /// One minus the used samples over `width x height x max_spp`.
    pub samples_saving: f64,
    /// The mean Delta E (reference, run).
    pub mean_de: f64,
    /// The 99th percentile of the Delta E (reference, run).
    pub p99_de: f64,
}

/// The metrics and the per-pixel Delta E (reference, run) they were taken from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Comparison {
    /// The reported numbers.
    pub metrics: Metrics,
    /// Delta E of each pixel, row-major, for the error map.
    pub error_map: Vec<f32>,
}

/// Compares `run` with `reference` (and `reference` with `ref2`, when given).
///
/// All images have the same size; `run_counts` holds the run's per-pixel sample counts.
pub(super) fn compare(
    reference: &DisplayImage,
    run: &DisplayImage,
    ref2: Option<&DisplayImage>,
    run_counts: &[u32],
) -> Comparison {
    let error_map = delta_e_map(reference, run);
    let noise_floor = ref2.map(|second| {
        let floor = delta_e_map(reference, second);
        NoiseFloor {
            mean: mean(&floor),
            p99: f64::from(nearest_rank(&sorted(&floor), P99)),
        }
    });
    let changed_threshold_de =
        noise_floor.map_or(MIN_CHANGED_DE, |floor| MIN_CHANGED_DE.max(floor.p99));
    let changed = error_map
        .iter()
        .filter(|&&de| f64::from(de) > changed_threshold_de)
        .count();
    let (samples_used, max_spp, samples_saving) = sample_figures(run_counts, reference);
    let metrics = Metrics {
        noise_floor,
        changed_threshold_de,
        changed_area_pct: percent(changed, error_map.len()),
        worst_tile_de: worst_tile_mean(&error_map, reference.width, reference.height),
        glint_recall: glint_recall(reference, run),
        samples_used,
        max_spp,
        samples_saving,
        mean_de: mean(&error_map),
        p99_de: f64::from(nearest_rank(&sorted(&error_map), P99)),
    };
    Comparison { metrics, error_map }
}

/// CIEDE2000 between the same pixels of `a` and `b`.
fn delta_e_map(a: &DisplayImage, b: &DisplayImage) -> Vec<f32> {
    parallel_map(a.lab.len(), |index| {
        let de = delta_e_2000(a.lab[index].map(f64::from), b.lab[index].map(f64::from));
        narrow(de)
    })
}

/// `value` in single precision.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a Delta E is a small number; the maps are stored in f32"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

/// `part` of `whole` in percent; 0 for an empty image.
const fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// The mean of `values`, summed in order; 0 for none.
fn mean(values: &[f32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().map(|&value| f64::from(value)).sum();
    sum / values.len() as f64
}

/// `values` in ascending order.
fn sorted(values: &[f32]) -> Vec<f32> {
    let mut copy = values.to_vec();
    copy.sort_unstable_by(f32::total_cmp);
    copy
}

/// The nearest-rank percentile `fraction.0 / fraction.1` of ascending `sorted`: the value at
/// rank `ceil(p x n)`. 0 for no values.
pub(super) fn nearest_rank(sorted: &[f32], fraction: (usize, usize)) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (sorted.len() * fraction.0)
        .div_ceil(fraction.1)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}

/// The total, the largest and the saving of the run's sample counts.
fn sample_figures(run_counts: &[u32], reference: &DisplayImage) -> (u64, u32, f64) {
    let used: u64 = run_counts.iter().map(|&count| u64::from(count)).sum();
    let max_spp = run_counts.iter().copied().max().unwrap_or(0);
    let budget = (reference.width * reference.height) as f64 * f64::from(max_spp);
    let saving = if budget > 0.0 {
        1.0 - used as f64 / budget
    } else {
        0.0
    };
    (used, max_spp, saving)
}

/// The largest mean over the 16 x 16 tiles of `map`, edge tiles included.
fn worst_tile_mean(map: &[f32], width: usize, height: usize) -> f64 {
    let mut worst = 0.0_f64;
    for top in (0..height).step_by(TILE) {
        for left in (0..width).step_by(TILE) {
            let right = (left + TILE).min(width);
            let bottom = (top + TILE).min(height);
            let mut sum = 0.0_f64;
            for row in top..bottom {
                for &value in &map[row * width + left..row * width + right] {
                    sum += f64::from(value);
                }
            }
            let pixels = (right - left) * (bottom - top);
            worst = worst.max(sum / pixels as f64);
        }
    }
    worst
}

/// The share of `reference`'s glint pixels that are glint pixels in `run`; `None` without glints.
///
/// A reference without glints (for example one clipped to white, where the 99.9th percentile is
/// the image maximum and no pixel is strictly above it) has nothing to recall.
fn glint_recall(reference: &DisplayImage, run: &DisplayImage) -> Option<f64> {
    let in_reference = glint_mask(reference);
    let total = in_reference.iter().filter(|&&glint| glint).count();
    if total == 0 {
        return None;
    }
    let in_run = glint_mask(run);
    let kept = in_reference
        .iter()
        .zip(&in_run)
        .filter(|&(&was, &is)| was && is)
        .count();
    Some(kept as f64 / total as f64)
}

/// Which pixels of `image` are glints: above the image's own 99.9th percentile of display
/// luminance and at least four times the median of their 5 x 5 neighbourhood.
fn glint_mask(image: &DisplayImage) -> Vec<bool> {
    let luminance = &image.luminance;
    let threshold = nearest_rank(&sorted(luminance), GLINT_PERCENTILE);
    let (width, height) = (image.width, image.height);
    let mut mask = vec![false; luminance.len()];
    for y in 0..height {
        for x in 0..width {
            let value = luminance[y * width + x];
            if value > threshold
                && value >= GLINT_RATIO * neighbourhood_median(luminance, width, height, (x, y))
            {
                mask[y * width + x] = true;
            }
        }
    }
    mask
}

/// The median of the 5 x 5 window around `at`; the window repeats the edge pixels at the borders.
fn neighbourhood_median(luminance: &[f32], width: usize, height: usize, at: (usize, usize)) -> f32 {
    let mut window = [0.0_f32; WINDOW_LEN];
    let mut next = 0;
    for dy in 0..=2 * WINDOW_RADIUS {
        let y = (at.1 + dy).saturating_sub(WINDOW_RADIUS).min(height - 1);
        for dx in 0..=2 * WINDOW_RADIUS {
            let x = (at.0 + dx).saturating_sub(WINDOW_RADIUS).min(width - 1);
            window[next] = luminance[y * width + x];
            next += 1;
        }
    }
    let (_, median, _) = window.select_nth_unstable_by(WINDOW_LEN / 2, f32::total_cmp);
    *median
}
