//! What the Tune view shows: the tier rows, the title, the mini sweep's chart marks and the
//! sentences, built from plain data and pushed into `TuneModel`.
//!
//! The builders take plain data and return Slint's plain structs (no window needed), so they are
//! tested directly; the `push_*` functions only copy their results into the model.

use super::{
    metrics::Visibility,
    mini_sweep::{ANGLE_EPSILON_DEG, FIGURES, best_row, plateau_rows},
    order::{Side, TierInfo},
};
use crate::{MainWindow, TuneBand, TuneModel, TuneTierRow};
use pleochroma_editor::sweep::{
    SweepRow, chart_path, chart_x_percent, format_angle_input, hover_text, nearest_row,
    series_range_text,
};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};
use std::collections::BTreeSet;

/// A position that is not on the chart (the model's "not shown").
const NOT_SHOWN: f32 = -1.0;

/// The chart's margins and plot size in percent of the viewbox: the numbers
/// `pleochroma_editor::sweep::chart_path` uses, so a mark lands on the line.
const MARGIN_X: f64 = 2.0;
const MARGIN_Y: f64 = 6.0;
const PLOT_WIDTH: f64 = 100.0 - 2.0 * MARGIN_X;
const PLOT_HEIGHT: f64 = 100.0 - 2.0 * MARGIN_Y;

/// Two chart values this close are the same height.
const FLAT_SPAN: f64 = 1e-9;

fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// `41.25°`: an angle as a person reads it (a magnitude, two decimals).
pub(super) fn format_angle(angle_deg: f64) -> String {
    format!("{:.2}\u{b0}", angle_deg.abs())
}

/// "40.75 → 41.25" when `now_deg` differs from the Baseline's angle `start_deg`, else empty.
pub(super) fn change_text(start_deg: Option<f64>, now_deg: f64) -> String {
    match start_deg {
        Some(start) if (start.abs() - now_deg.abs()).abs() >= 0.005 => {
            format!("{:.2} \u{2192} {:.2}", start.abs(), now_deg.abs())
        }
        _ => String::new(),
    }
}

/// The title over the picked tier: `PAVILION MAIN (P1) 41.25°`, or `P1 41.25°` for a tier whose
/// name is its code.
pub(super) fn title_text(info: &TierInfo) -> String {
    let angle = format_angle(info.angle_deg);
    if info.name == info.code || info.code.is_empty() {
        format!("{} {angle}", info.name.to_uppercase())
    } else {
        format!("{} ({}) {angle}", info.name.to_uppercase(), info.code)
    }
}

/// The note of a row that cannot be picked: what its angle follows, or why a girdle tier is
/// left alone.
fn note_of(info: &TierInfo) -> String {
    info.relation.as_ref().map_or_else(
        || {
            if info.side == Side::Girdle {
                "no angle to tune".to_owned()
            } else {
                String::new()
            }
        },
        |relation| format!("follows {relation}"),
    )
}

/// What the tier list needs besides the tiers.
pub(super) struct ListInput<'a> {
    /// The tiers, in design order.
    pub infos: &'a [TierInfo],
    /// Their design positions in the order to list them.
    pub order: &'a [usize],
    /// The Baseline's angle of every design tier (see `order::baseline_angles`).
    pub baseline: &'a [Option<f64>],
    pub accepted: &'a BTreeSet<usize>,
    pub selected: Option<usize>,
    /// Whether the order can be changed (the Advanced interface).
    pub reorderable: bool,
}

/// The tier list.
pub(super) fn tier_rows(input: &ListInput<'_>) -> Vec<TuneTierRow> {
    let shown: Vec<&TierInfo> = input
        .order
        .iter()
        .filter_map(|&tier| input.infos.iter().find(|info| info.index == tier))
        .collect();
    let last = shown.len().saturating_sub(1);
    shown
        .iter()
        .enumerate()
        .map(|(position, info)| {
            let selectable = info.tunable();
            let start = input.baseline.get(info.index).copied().flatten();
            TuneTierRow {
                tier: i32::try_from(info.index).unwrap_or(-1),
                code: info.code.as_str().into(),
                name: info.name.as_str().into(),
                angle: format_angle(info.angle_deg).into(),
                change: if selectable {
                    change_text(start, info.angle_deg).into()
                } else {
                    SharedString::default()
                },
                note: note_of(info).into(),
                accepted: input.accepted.contains(&info.index),
                selectable,
                selected: input.selected == Some(info.index),
                can_up: input.reorderable && position > 0,
                can_down: input.reorderable && position < last,
            }
        })
        .collect()
}

/// The marks of the mini sweep's chart, in percent of the plot (the 0..100 viewbox).
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct ChartMarks {
    /// The line of the chosen figure.
    pub commands: String,
    /// Left edge and width of every stretch as good as the best.
    pub plateau: Vec<(f64, f64)>,
    /// The tier's own angle, on the line.
    pub current: Option<(f64, f64)>,
    /// The best angle of the figure.
    pub best: Option<(f64, f64)>,
    pub axis_low: String,
    pub axis_high: String,
    /// What the line's height means.
    pub range_text: String,
}

/// The lowest and the highest magnitude among the rows' angles.
fn angle_extent(rows: &[SweepRow]) -> Option<(f64, f64)> {
    let low = rows
        .iter()
        .map(|row| row.angle_deg.abs())
        .reduce(f64::min)?;
    let high = rows
        .iter()
        .map(|row| row.angle_deg.abs())
        .reduce(f64::max)?;
    Some((low, high))
}

/// Where `angle_deg` sits on the x axis of the chart, or `None` outside the swept range.
fn angle_x(rows: &[SweepRow], angle_deg: f64) -> Option<f64> {
    let (low, high) = angle_extent(rows)?;
    let magnitude = angle_deg.abs();
    if magnitude < low - 1e-9 || magnitude > high + 1e-9 {
        return None;
    }
    if high - low < FLAT_SPAN {
        return Some(50.0);
    }
    Some(((magnitude - low) / (high - low)).mul_add(PLOT_WIDTH, MARGIN_X))
}

/// The lowest and the highest value of `figure` over the valid rows.
fn value_extent(rows: &[SweepRow], figure: usize) -> Option<(f64, f64)> {
    let (metric, _) = FIGURES.get(figure)?;
    let mut values = rows.iter().filter_map(|row| metric.value(row));
    let first = values.next()?;
    Some(values.fold((first, first), |(low, high), value| {
        (low.min(value), high.max(value))
    }))
}

/// Where `value` sits on the chart's y axis (a larger value is higher up, so a smaller y).
fn value_y(value: f64, (low, high): (f64, f64)) -> f64 {
    if high - low < FLAT_SPAN {
        50.0
    } else {
        ((value - low) / (high - low)).mul_add(-PLOT_HEIGHT, 100.0 - MARGIN_Y)
    }
}

/// The figure at `angle_deg`, read off the line between its two neighbouring valid rows (or the
/// nearest valid row at an end).
fn value_at(rows: &[SweepRow], figure: usize, angle_deg: f64) -> Option<f64> {
    let (metric, _) = FIGURES.get(figure)?;
    let magnitude = angle_deg.abs();
    let valid: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|row| Some((row.angle_deg.abs(), metric.value(row)?)))
        .collect();
    let before = valid
        .iter()
        .rev()
        .find(|(angle, _)| *angle <= magnitude + 1e-9);
    let after = valid.iter().find(|(angle, _)| *angle >= magnitude - 1e-9);
    match (before, after) {
        (Some(&(a0, v0)), Some(&(a1, v1))) if (a1 - a0).abs() > 1e-9 => {
            Some(v0 + (v1 - v0) * (magnitude - a0) / (a1 - a0))
        }
        (Some(&(_, value)), _) | (None, Some(&(_, value))) => Some(value),
        (None, None) => None,
    }
}

/// The stretches of the x axis the plateau rows cover: a stretch per run of neighbouring rows,
/// reaching half a step beyond its first and last row, as `(left, width)`.
fn plateau_bands(rows: &[SweepRow], plateau: &[usize]) -> Vec<(f64, f64)> {
    let half = if rows.len() > 1 {
        PLOT_WIDTH / (rows.len() - 1) as f64 / 2.0
    } else {
        PLOT_WIDTH / 2.0
    };
    let mut bands = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    let mut close = |run: (usize, usize)| {
        let left = (chart_x_percent(rows, run.0) - half).max(0.0);
        let right = (chart_x_percent(rows, run.1) + half).min(100.0);
        bands.push((left, right - left));
    };
    for &index in plateau {
        run = match run {
            Some((first, last)) if index == last + 1 => Some((first, index)),
            Some(finished) => {
                close(finished);
                Some((index, index))
            }
            None => Some((index, index)),
        };
    }
    if let Some(finished) = run {
        close(finished);
    }
    bands
}

/// The chart of `rows` for `figure`, with the tier's angle now `current_deg`.
pub(super) fn chart_marks(rows: &[SweepRow], figure: usize, current_deg: f64) -> ChartMarks {
    let Some(&(metric, _)) = FIGURES.get(figure) else {
        return ChartMarks::default();
    };
    let extent = value_extent(rows, figure);
    let best = best_row(rows, figure);
    let (axis_low, axis_high) = angle_extent(rows).map_or_else(
        || (String::new(), String::new()),
        |(low, high)| (format!("{low:.2}\u{b0}"), format!("{high:.2}\u{b0}")),
    );
    ChartMarks {
        commands: chart_path(rows, metric),
        plateau: plateau_bands(rows, &plateau_rows(rows, figure)),
        current: extent.and_then(|extent| {
            let x = angle_x(rows, current_deg)?;
            let value = value_at(rows, figure, current_deg)?;
            Some((x, value_y(value, extent)))
        }),
        best: extent.and_then(|extent| {
            let row = rows.get(best?)?;
            let value = metric.value(row)?;
            Some((chart_x_percent(rows, best?), value_y(value, extent)))
        }),
        axis_low,
        axis_high,
        range_text: series_range_text(rows, metric),
    }
}

/// The sentence under the chart: the angle under the pointer, else the best angle.
pub(super) fn readout(
    rows: &[SweepRow],
    figure: usize,
    hover: Option<usize>,
    visible: Visibility,
) -> String {
    let Some(&(metric, _)) = FIGURES.get(figure) else {
        return String::new();
    };
    let selected = visible.selected();
    if let Some(index) = hover.filter(|&index| index < rows.len()) {
        return hover_text(rows, index, &selected);
    }
    best_row(rows, figure).map_or_else(String::new, |best| {
        format!(
            "Best ({}): {}",
            metric.label(),
            hover_text(rows, best, &selected)
        )
    })
}

/// The row under the pointer at `fraction` across the plot.
pub(super) fn row_at(rows: &[SweepRow], fraction: f32) -> Option<usize> {
    nearest_row(rows, f64::from(fraction))
}

/// The figure combo's entries.
pub(super) fn figure_names() -> Vec<SharedString> {
    FIGURES
        .iter()
        .map(|(metric, _)| SharedString::from(metric.label()))
        .collect()
}

/// A float for a Slint property.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the Slint property precision; a chart position is 0..=100"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// The angle text a nudge or Jump to Best hands to the tier: the magnitude at full precision, no
/// float noise.
pub(super) fn angle_field(angle_deg: f64) -> String {
    format_angle_input(angle_deg)
}

/// What the angle box SHOWS for `angle_deg`: two decimals, like the tier table and the title
/// ("40.10", not "40.100000001"). A tier whose angle really has more digits ("41.2367") keeps
/// them, so pressing Enter in the box never moves the angle.
pub(super) fn angle_display(angle_deg: f64) -> String {
    let magnitude = angle_deg.abs();
    let two_decimals = format!("{magnitude:.2}");
    let rounded = two_decimals.parse::<f64>().unwrap_or(magnitude);
    if (rounded - magnitude).abs() <= ANGLE_EPSILON_DEG {
        two_decimals
    } else {
        format_angle_input(angle_deg)
    }
}

/// Copies the tier list into the model.
pub(super) fn push_tiers(ui: &MainWindow, rows: Vec<TuneTierRow>) {
    ui.global::<TuneModel>().set_tiers(model_of(rows));
}

/// Copies the chart's marks into the model.
pub(super) fn push_chart(ui: &MainWindow, marks: &ChartMarks) {
    let model = ui.global::<TuneModel>();
    model.set_chart_commands(marks.commands.as_str().into());
    model.set_plateau(model_of(
        marks
            .plateau
            .iter()
            .map(|&(x, w)| TuneBand {
                x: to_f32(x),
                w: to_f32(w),
            })
            .collect(),
    ));
    let mark = |mark: Option<(f64, f64)>| {
        mark.map_or((NOT_SHOWN, NOT_SHOWN), |(x, y)| (to_f32(x), to_f32(y)))
    };
    let (current_x, current_y) = mark(marks.current);
    model.set_current_x(current_x);
    model.set_current_y(current_y);
    let (best_x, best_y) = mark(marks.best);
    model.set_best_x(best_x);
    model.set_best_y(best_y);
    model.set_axis_low(marks.axis_low.as_str().into());
    model.set_axis_high(marks.axis_high.as_str().into());
    model.set_range_text(marks.range_text.as_str().into());
}

/// Empties the chart (nothing drawn).
pub(super) fn clear_chart(ui: &MainWindow, visible: Visibility) {
    push_chart(ui, &ChartMarks::default());
    super::metrics::push(ui, &[], visible);
    let model = ui.global::<TuneModel>();
    model.set_readout_text("".into());
    model.set_hover_x(NOT_SHOWN);
    model.set_can_jump(false);
}

/// Puts the pointer's guide on the chart (`None` hides it).
pub(super) fn push_hover(ui: &MainWindow, rows: &[SweepRow], hover: Option<usize>) {
    let x = hover
        .filter(|&index| index < rows.len())
        .map_or(NOT_SHOWN, |index| to_f32(chart_x_percent(rows, index)));
    ui.global::<TuneModel>().set_hover_x(x);
}
