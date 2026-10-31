//! The two-tier sweep's heat map as pixels: one block per angle pair (no smoothing), a
//! color-blind-safe sequential scale, hatching for the pairs that give no stone, and an
//! outline round the plateau of pairs that are as good as the best one within the figure's
//! noise threshold.
//!
//! Slint-free: [`render`] returns RGBA bytes and [`cell_at`] is plain arithmetic, so both are
//! tested without a window. The marks that follow the selection and the pointer (the design's
//! own pair, the best pair, the selected pair) are drawn by the view on top of this image
//! (`ui/components/sweep_dialog/sweep_heatmap.slint`).

use pleochroma_editor::{
    metric_deltas::{DeltaMetric, Tone, YIELD_NOISE_POINTS, judge},
    sweep::SweepMetric,
};

/// Viridis at nine evenly spaced stops: dark purple for the worst value, yellow for the best.
/// It reads in grey scale and for every common kind of color blindness.
const VIRIDIS: [[u8; 3]; 9] = [
    [68, 1, 84],
    [72, 40, 120],
    [62, 73, 137],
    [49, 104, 142],
    [38, 130, 142],
    [31, 158, 137],
    [53, 183, 121],
    [110, 206, 88],
    [253, 231, 37],
];

/// A pair the sweep never reached (it was stopped), or one that has no value for the figure.
const PENDING: [u8; 3] = [24, 28, 38];

/// The ground of an invalid pair; the stripes run over it.
const HATCH_BACK: [u8; 3] = [52, 58, 70];

/// The diagonal stripes of an invalid pair.
const HATCH_STRIPE: [u8; 3] = [124, 132, 146];

/// The line round the plateau; dark, because the plateau is the bright end of the scale.
const OUTLINE: [u8; 3] = [8, 10, 16];

/// Stripes repeat every this many pixels along a diagonal, and are this wide.
const HATCH_PERIOD: usize = 8;
const HATCH_WIDTH: usize = 3;

/// The widest and tallest the map is drawn, in pixels; a cell is as large as fits.
const MAX_MAP_WIDTH: usize = 600;
const MAX_MAP_HEIGHT: usize = 400;

/// The smallest and largest block, in pixels.
const MIN_CELL: usize = 6;
const MAX_CELL: usize = 32;

/// The color scale under the map: its size in pixels.
const LEGEND_WIDTH: usize = 128;
const LEGEND_HEIGHT: usize = 8;

/// An RGBA picture, four bytes a pixel, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What one pair of the grid shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum CellFigure {
    /// The pair's figure: a block on the scale.
    Value(f64),
    /// The pair gives no stone: hatched.
    Invalid,
    /// The sweep never reached the pair, or the pair has no value for this figure.
    Pending,
}

/// The lowest and highest value among the pairs that have one; `None` when none has.
pub(super) fn value_range(cells: &[CellFigure]) -> Option<(f64, f64)> {
    cells
        .iter()
        .filter_map(|cell| match cell {
            CellFigure::Value(value) if value.is_finite() => Some(*value),
            _ => None,
        })
        .fold(None::<(f64, f64)>, |range, value| {
            Some(range.map_or((value, value), |(low, high)| {
                (low.min(value), high.max(value))
            }))
        })
}

/// Where `value` sits on the scale, 0 for the worst end and 1 for the best: flipped for a
/// figure where lower is better, so "good" is always the same end. A figure that does not
/// vary sits in the middle.
pub(super) fn normalized(value: f64, low: f64, high: f64, higher_is_better: bool) -> f64 {
    let span = high - low;
    if span <= 0.0 || !span.is_finite() {
        return 0.5;
    }
    let t = ((value - low) / span).clamp(0.0, 1.0);
    if higher_is_better { t } else { 1.0 - t }
}

/// A channel `fraction` of the way from `from` to `to`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the result lies between two u8 values"
)]
fn lerp(from: u8, to: u8, fraction: f64) -> u8 {
    let (from, to) = (f64::from(from), f64::from(to));
    (to - from).mul_add(fraction, from).round() as u8
}

/// The color at `t` (0 worst, 1 best) on the scale.
pub(super) fn scale_color(t: f64) -> [u8; 3] {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let segments = VIRIDIS.len() - 1;
    let scaled = t * segments as f64;
    let mut index = 0;
    while index + 1 < segments && scaled >= (index + 1) as f64 {
        index += 1;
    }
    let fraction = scaled - index as f64;
    let (from, to) = (VIRIDIS[index], VIRIDIS[index + 1]);
    [
        lerp(from[0], to[0], fraction),
        lerp(from[1], to[1], fraction),
        lerp(from[2], to[2], fraction),
    ]
}

/// How many pixels a block is: as large as fits `MAX_MAP_WIDTH` by `MAX_MAP_HEIGHT`, between
/// `MIN_CELL` and `MAX_CELL`.
fn cell_size(columns: usize, rows: usize) -> usize {
    (MAX_MAP_WIDTH / columns.max(1))
        .min(MAX_MAP_HEIGHT / rows.max(1))
        .clamp(MIN_CELL, MAX_CELL)
}

/// The color of a pixel of an invalid pair: dark with light diagonal stripes.
const fn hatch(x: usize, y: usize) -> [u8; 3] {
    if (x + y) % HATCH_PERIOD < HATCH_WIDTH {
        HATCH_STRIPE
    } else {
        HATCH_BACK
    }
}

/// Whether the pixel `(lx, ly)` of the block `(col, row)` lies on an edge of the plateau:
/// a pixel of a plateau block next to a block that is not on the plateau. The edge of the
/// map itself is not an edge of the plateau.
fn on_outline(
    (col, row): (usize, usize),
    (lx, ly): (usize, usize),
    size: usize,
    (columns, rows): (usize, usize),
    plateau: &[bool],
) -> bool {
    let inside = |c: usize, r: usize| plateau.get(r * columns + c).copied().unwrap_or(false);
    if !inside(col, row) {
        return false;
    }
    let edge = (size / 8).clamp(2, size / 2);
    (lx < edge && col > 0 && !inside(col - 1, row))
        || (lx >= size - edge && col + 1 < columns && !inside(col + 1, row))
        || (ly < edge && row > 0 && !inside(col, row - 1))
        || (ly >= size - edge && row + 1 < rows && !inside(col, row + 1))
}

/// Draws the grid: `cells` in row-major order (`columns` by `rows`), the best end of the
/// scale being the high values when `higher_is_better`, the pairs of `plateau` outlined.
/// An empty grid gives an empty picture.
pub(super) fn render(
    (columns, rows): (usize, usize),
    cells: &[CellFigure],
    higher_is_better: bool,
    plateau: &[bool],
) -> Pixels {
    if columns == 0 || rows == 0 {
        return Pixels {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
    }
    let size = cell_size(columns, rows);
    let (width, height) = (columns * size, rows * size);
    let (low, high) = value_range(cells).unwrap_or((0.0, 0.0));
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let (col, row) = (x / size, y / size);
            let figure = cells
                .get(row * columns + col)
                .copied()
                .unwrap_or(CellFigure::Pending);
            let color = if on_outline(
                (col, row),
                (x % size, y % size),
                size,
                (columns, rows),
                plateau,
            ) {
                OUTLINE
            } else {
                match figure {
                    CellFigure::Value(value) if value.is_finite() => {
                        scale_color(normalized(value, low, high, higher_is_better))
                    }
                    CellFigure::Value(_) | CellFigure::Pending => PENDING,
                    CellFigure::Invalid => hatch(x, y),
                }
            };
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    Pixels {
        width,
        height,
        rgba,
    }
}

/// The color scale as a strip: the worst end on the left, the best on the right.
pub(super) fn legend_pixels() -> Pixels {
    let mut rgba = Vec::with_capacity(LEGEND_WIDTH * LEGEND_HEIGHT * 4);
    for _ in 0..LEGEND_HEIGHT {
        for x in 0..LEGEND_WIDTH {
            let color = scale_color(x as f64 / (LEGEND_WIDTH - 1) as f64);
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    Pixels {
        width: LEGEND_WIDTH,
        height: LEGEND_HEIGHT,
        rgba,
    }
}

/// The pair under a pointer at `(x, y)`, each a fraction of the image (0 at the top left,
/// 1 at the bottom right), as `(column, row)`; `None` outside the image or for an empty grid.
pub(super) fn cell_at(columns: usize, rows: usize, x: f32, y: f32) -> Option<(usize, usize)> {
    if columns == 0 || rows == 0 || !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
        return None;
    }
    Some((index_in(x, columns), index_in(y, rows)))
}

/// The block of `count` blocks that `fraction` (0 up to, not including, 1) falls in.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is in 0..1, so the product is a small non-negative number"
)]
fn index_in(fraction: f32, count: usize) -> usize {
    ((fraction * count as f32) as usize).min(count - 1)
}

/// The position of the best value (the first one on a tie), row-major; `None` when no pair
/// has a value.
pub(super) fn best_index(cells: &[CellFigure], higher_is_better: bool) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, cell) in cells.iter().enumerate() {
        let CellFigure::Value(value) = *cell else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let better = best.is_none_or(|(_, held)| {
            if higher_is_better {
                value > held
            } else {
                value < held
            }
        });
        if better {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

/// How the engine's noise rule sees `metric`: the figure it judges and whether the tilt
/// threshold applies. The yield has its own rule.
const fn judged_as(metric: SweepMetric) -> Option<(DeltaMetric, bool)> {
    match metric {
        SweepMetric::Brilliance => Some((DeltaMetric::Brilliance, false)),
        SweepMetric::Windowing => Some((DeltaMetric::Windowing, false)),
        SweepMetric::Extinction => Some((DeltaMetric::Extinction, false)),
        SweepMetric::Fire => Some((DeltaMetric::Fire, false)),
        SweepMetric::Scintillation => Some((DeltaMetric::Scintillation, false)),
        SweepMetric::Yield => None,
        SweepMetric::TiltBrilliance => Some((DeltaMetric::Brilliance, true)),
        SweepMetric::TiltWindowing => Some((DeltaMetric::Windowing, true)),
        SweepMetric::TiltExtinction => Some((DeltaMetric::Extinction, true)),
    }
}

/// Whether `value` is within the figure's noise threshold of `best`: the rule the Live
/// Deltas and the compare window use ([`judge`] says "about the same").
#[expect(
    clippy::cast_possible_truncation,
    reason = "the engine's figures are f32, widened for the table; narrowing gives them back"
)]
fn within_noise(metric: SweepMetric, best: f64, value: f64) -> bool {
    match judged_as(metric) {
        Some((judged, tilt)) => judge(judged, best as f32, value as f32, tilt)
            .is_some_and(|change| change.tone == Tone::Same),
        None => (best - value).abs() <= YIELD_NOISE_POINTS,
    }
}

/// The pairs that are as good as the best one within the figure's noise threshold: the
/// plateau a cutter can choose from. Row-major like `cells`; empty of `true` when no pair has
/// a value.
pub(super) fn plateau(metric: SweepMetric, cells: &[CellFigure]) -> Vec<bool> {
    let best =
        best_index(cells, metric.higher_is_better()).and_then(|index| match cells.get(index) {
            Some(CellFigure::Value(value)) => Some(*value),
            _ => None,
        });
    cells
        .iter()
        .map(|cell| match (best, cell) {
            (Some(best), CellFigure::Value(value)) => within_noise(metric, best, *value),
            _ => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(figures: &[f64]) -> Vec<CellFigure> {
        figures.iter().map(|&v| CellFigure::Value(v)).collect()
    }

    fn pixel(pixels: &Pixels, x: usize, y: usize) -> [u8; 3] {
        let at = (y * pixels.width + x) * 4;
        [pixels.rgba[at], pixels.rgba[at + 1], pixels.rgba[at + 2]]
    }

    #[test]
    fn the_scale_runs_from_dark_purple_to_yellow() {
        assert_eq!(scale_color(0.0), [68, 1, 84]);
        assert_eq!(scale_color(1.0), [253, 231, 37]);
        assert_eq!(scale_color(0.5), [38, 130, 142]);
        assert_eq!(scale_color(-3.0), scale_color(0.0));
        assert_eq!(scale_color(f64::NAN), scale_color(0.0));
    }

    #[test]
    fn a_lower_is_better_figure_has_its_good_end_flipped() {
        assert!((normalized(10.0, 10.0, 20.0, true) - 0.0).abs() < 1e-12);
        assert!((normalized(10.0, 10.0, 20.0, false) - 1.0).abs() < 1e-12);
        assert!((normalized(20.0, 10.0, 20.0, false) - 0.0).abs() < 1e-12);
        assert!((normalized(5.0, 5.0, 5.0, true) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn the_range_skips_pairs_without_a_value() {
        let cells = [
            CellFigure::Invalid,
            CellFigure::Value(4.0),
            CellFigure::Pending,
            CellFigure::Value(9.0),
        ];
        assert_eq!(value_range(&cells), Some((4.0, 9.0)));
        assert_eq!(value_range(&[CellFigure::Invalid]), None);
    }

    #[test]
    fn a_pointer_finds_its_pair_and_none_outside_the_image() {
        assert_eq!(cell_at(4, 2, 0.0, 0.0), Some((0, 0)));
        assert_eq!(cell_at(4, 2, 0.26, 0.6), Some((1, 1)));
        assert_eq!(cell_at(4, 2, 0.999, 0.999), Some((3, 1)));
        assert_eq!(cell_at(4, 2, 1.0, 0.5), None);
        assert_eq!(cell_at(4, 2, -1.0, -1.0), None);
        assert_eq!(cell_at(4, 2, f32::NAN, 0.5), None);
        assert_eq!(cell_at(0, 2, 0.5, 0.5), None);
    }

    #[test]
    fn the_picture_has_a_block_per_pair() {
        let cells = values(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let pixels = render((3, 2), &cells, true, &[false; 6]);
        let size = cell_size(3, 2);
        assert_eq!((pixels.width, pixels.height), (3 * size, 2 * size));
        assert_eq!(pixels.rgba.len(), pixels.width * pixels.height * 4);
        // The lowest value is the dark end and the highest the bright end; a block is flat.
        assert_eq!(pixel(&pixels, 0, 0), scale_color(0.0));
        assert_eq!(pixel(&pixels, size - 1, size - 1), scale_color(0.0));
        assert_eq!(pixel(&pixels, 3 * size - 1, 2 * size - 1), scale_color(1.0));
        // Flipped for a figure where lower is better.
        let flipped = render((3, 2), &cells, false, &[false; 6]);
        assert_eq!(pixel(&flipped, 0, 0), scale_color(1.0));
    }

    #[test]
    fn an_invalid_pair_is_hatched_and_a_pending_one_is_flat() {
        let cells = [
            CellFigure::Value(1.0),
            CellFigure::Invalid,
            CellFigure::Pending,
            CellFigure::Value(2.0),
        ];
        let pixels = render((2, 2), &cells, true, &[false; 4]);
        let size = cell_size(2, 2);
        let colors: std::collections::HashSet<[u8; 3]> = (0..size)
            .flat_map(|y| (0..size).map(move |x| (size + x, y)))
            .map(|(x, y)| pixel(&pixels, x, y))
            .collect();
        assert_eq!(colors.len(), 2, "stripes and ground");
        assert!(colors.contains(&HATCH_STRIPE) && colors.contains(&HATCH_BACK));
        assert_eq!(pixel(&pixels, 0, size), PENDING);
    }

    #[test]
    fn the_plateau_is_outlined_inside_and_not_at_the_map_edge() {
        let cells = values(&[1.0; 4]);
        // Only the first pair is on the plateau.
        let pixels = render((2, 2), &cells, true, &[true, false, false, false]);
        let size = cell_size(2, 2);
        // Its right and bottom edges face pairs off the plateau; its left and top edges are
        // the map's edge and stay plain.
        assert_eq!(pixel(&pixels, size - 1, size / 2), OUTLINE);
        assert_eq!(pixel(&pixels, size / 2, size - 1), OUTLINE);
        assert_ne!(pixel(&pixels, 0, size / 2), OUTLINE);
        assert_ne!(pixel(&pixels, size / 2, 0), OUTLINE);
        // The pairs off the plateau carry no line.
        assert_ne!(pixel(&pixels, size, size / 2), OUTLINE);
    }

    #[test]
    fn the_best_pair_is_the_first_extreme() {
        let cells = [
            CellFigure::Invalid,
            CellFigure::Value(3.0),
            CellFigure::Value(7.0),
            CellFigure::Value(7.0),
            CellFigure::Value(1.0),
        ];
        assert_eq!(best_index(&cells, true), Some(2));
        assert_eq!(best_index(&cells, false), Some(4));
        assert_eq!(best_index(&[CellFigure::Pending], true), None);
    }

    #[test]
    fn the_plateau_follows_the_figures_noise_threshold() {
        // Brilliance: within its noise of the best (80) is on the plateau, 70 is not.
        let cells = [
            CellFigure::Value(80.0),
            CellFigure::Value(79.95),
            CellFigure::Value(70.0),
            CellFigure::Invalid,
        ];
        assert_eq!(
            plateau(SweepMetric::Brilliance, &cells),
            vec![true, true, false, false]
        );
        // Windowing: lower is better, so the best is 5.
        let windowing = values(&[5.0, 5.01, 30.0]);
        assert_eq!(
            plateau(SweepMetric::Windowing, &windowing),
            vec![true, true, false]
        );
        // The yield has its own rule: 0.1 points.
        let yields = values(&[40.0, 39.95, 39.0]);
        assert_eq!(
            plateau(SweepMetric::Yield, &yields),
            vec![true, true, false]
        );
        assert_eq!(plateau(SweepMetric::Fire, &[]), Vec::<bool>::new());
    }

    #[test]
    fn the_legend_goes_from_the_worst_to_the_best_end() {
        let legend = legend_pixels();
        assert_eq!(legend.rgba.len(), legend.width * legend.height * 4);
        assert_eq!(pixel(&legend, 0, 0), scale_color(0.0));
        assert_eq!(pixel(&legend, legend.width - 1, 0), scale_color(1.0));
    }

    #[test]
    fn an_empty_grid_draws_nothing() {
        let pixels = render((0, 3), &[], true, &[]);
        assert_eq!((pixels.width, pixels.height), (0, 0));
        assert_eq!(pixels.rgba.len(), 0);
    }
}
