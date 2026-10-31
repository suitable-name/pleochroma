//! What the Angle Sweep dialog's Two Tiers mode shows: the heat map and its color scale, the
//! readout of the pair under the pointer or the selected pair, the axis labels and the status
//! lines, built from the engine's cells, and the pushes into `SweepModel`.
//!
//! The builders take plain data and return plain values (no window needed), so they are
//! tested directly; the `push_*` functions only copy their results into the model.

use super::{
    heatmap::{self, CellFigure, Pixels},
    state2d::TwoTier,
};
use crate::{MainWindow, SweepModel, bridge::pixel_buffer::rgba_image};
use pleochroma_editor::{
    live_deltas::{DeltaFigure, figure_deltas},
    sweep::{RowMetrics, Sweep2dOutcome, SweepMetric},
};
use slint::{ComponentHandle, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::fmt::Write as _;

/// The readout before the pointer or a click has picked a pair.
const READOUT_HINT: &str = "Move the pointer over the map, or click a pair.";

/// A position that is not on the map (the model's "not shown").
const NOT_SHOWN: f32 = -1.0;

fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// The name of a figure in the combo.
pub(super) const fn figure_label(metric: SweepMetric) -> &'static str {
    match metric {
        SweepMetric::Brilliance => "Brilliance",
        SweepMetric::Windowing => "Windowing",
        SweepMetric::Extinction => "Extinction",
        SweepMetric::Fire => "Fire",
        SweepMetric::Scintillation => "Scintillation",
        SweepMetric::Yield => "Yield",
        SweepMetric::TiltBrilliance => "Tilt Brilliance",
        SweepMetric::TiltWindowing => "Tilt Windowing",
        SweepMetric::TiltExtinction => "Tilt Extinction",
    }
}

/// `1 angle pair`, `21 angle pairs`.
fn pair_count(count: usize) -> String {
    format!("{count} angle pair{}", if count == 1 { "" } else { "s" })
}

/// `figure` of every cell of the grid, row-major: a block, hatched (no stone), or nothing
/// (the sweep never reached the cell, or the cell has no value for this figure).
pub(super) fn cell_figures(outcome: &Sweep2dOutcome, metric: SweepMetric) -> Vec<CellFigure> {
    let mut figures = vec![CellFigure::Pending; outcome.plan.len()];
    for cell in &outcome.cells {
        let index = outcome.plan.index_of(cell.col, cell.row);
        let Some(slot) = figures.get_mut(index) else {
            continue;
        };
        *slot = match (&cell.metrics, metric.value_of(cell.metrics.as_ref())) {
            (None, _) => CellFigure::Invalid,
            (Some(_), Some(value)) => CellFigure::Value(value),
            (Some(_), None) => CellFigure::Pending,
        };
    }
    figures
}

/// A figure as the legend shows it: one decimal and the unit.
fn value_text(metric: SweepMetric, value: f64) -> String {
    format!("{value:.1}{}", metric.unit())
}

/// The values at the dark and the bright end of the scale, in the figure's units; empty
/// texts when no cell has a value.
pub(super) fn legend_texts(metric: SweepMetric, range: Option<(f64, f64)>) -> (String, String) {
    let Some((low, high)) = range else {
        return (String::new(), String::new());
    };
    let (worst, best) = if metric.higher_is_better() {
        (low, high)
    } else {
        (high, low)
    };
    (value_text(metric, worst), value_text(metric, best))
}

/// The sentence under the color scale.
fn legend_note(contour: bool) -> String {
    let mut note = "Bright cells are the best. Hatched cells give no stone. A ring marks the \
        design's own pair and a star the best pair."
        .to_owned();
    if contour {
        note.push_str(
            " The outline marks the pairs that are as good as the best one, within the noise \
             of the figure.",
        );
    }
    note
}

/// Everything the map and its scale show for one figure.
pub(super) struct HeatView {
    pub heat: Pixels,
    pub legend: Pixels,
    pub legend_low: String,
    pub legend_high: String,
    pub legend_note: String,
    /// The best pair, `(column, row)`.
    pub best: Option<(usize, usize)>,
}

/// The map of `metric` over the cells of `outcome`, with the plateau outlined when `contour`.
pub(super) fn heat_view(outcome: &Sweep2dOutcome, metric: SweepMetric, contour: bool) -> HeatView {
    let (columns, rows) = (outcome.plan.columns(), outcome.plan.rows());
    let figures = cell_figures(outcome, metric);
    let higher_is_better = metric.higher_is_better();
    let plateau = if contour {
        heatmap::plateau(metric, &figures)
    } else {
        vec![false; figures.len()]
    };
    let (legend_low, legend_high) = legend_texts(metric, heatmap::value_range(&figures));
    HeatView {
        heat: heatmap::render((columns, rows), &figures, higher_is_better, &plateau),
        legend: heatmap::legend_pixels(),
        legend_low,
        legend_high,
        legend_note: legend_note(contour),
        best: heatmap::best_index(&figures, higher_is_better)
            .map(|index| (index % columns.max(1), index / columns.max(1))),
    }
}

/// `Crown Main, 32.50° to 36.50°`: a tier's name and the range of its angles (magnitudes,
/// flattest first).
pub(super) fn axis_label(name: &str, angles: &[f64]) -> String {
    match (angles.first(), angles.last()) {
        (Some(first), Some(last)) => format!(
            "{name}, {:.2}\u{b0} to {:.2}\u{b0}",
            first.abs(),
            last.abs()
        ),
        _ => name.to_owned(),
    }
}

/// The title over the results: the two tiers and how many pairs came back.
pub(super) fn result_title(outcome: &Sweep2dOutcome) -> String {
    let mut title = format!(
        "{} and {}: {}, {} valid",
        outcome.plan.tier_a_name,
        outcome.plan.tier_b_name,
        pair_count(outcome.cells.len()),
        outcome.valid_count()
    );
    if outcome.cancelled {
        let _ = write!(title, " (stopped, {} were planned)", outcome.requested);
    }
    title
}

/// The status line when a sweep is over.
pub(super) fn finished_status(outcome: &Sweep2dOutcome) -> String {
    let done = outcome.cells.len();
    if outcome.cancelled {
        format!("Stopped: {done} of {} done.", pair_count(outcome.requested))
    } else if outcome.valid_count() == 0 {
        format!(
            "Done: {}, but none gives a stone. Hover over a hatched pair to see why.",
            pair_count(done)
        )
    } else {
        format!(
            "Done: {}, {} valid.",
            pair_count(done),
            outcome.valid_count()
        )
    }
}

/// The status line while a sweep runs.
pub(super) fn running_status(done: usize, total: usize) -> String {
    format!("{done} of {} done", pair_count(total))
}

/// The readout of one pair: its angles, its figures with the change against the design's
/// own pair, and notes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Readout {
    /// `Crown Main 34.75°, Pavilion Main 41.25°`.
    pub head: String,
    /// Each figure with its change, and its tone: 0 about the same, 1 better, 2 worse.
    pub items: Vec<(String, i32)>,
    /// Why the pair gives no stone, or what to watch at it; empty for none.
    pub notes: String,
}

/// The figures of a pair when there is no pair of the design to compare with.
fn plain_items(figures: &RowMetrics) -> Vec<(String, i32)> {
    DeltaFigure::ALL
        .into_iter()
        .filter_map(|figure| {
            let value = figure.value(figures)?;
            // The warning count is a whole number; none is not worth a word.
            if figure == DeltaFigure::Warnings && value < 0.5 {
                return None;
            }
            let unit = if figure.is_percent() { " %" } else { "" };
            Some((
                format!("{} {}{unit}", figure.short_label(), figure.cell_text(value)),
                0,
            ))
        })
        .collect()
}

/// The readout of the cell `(column, row)` (`hovering` for the pair under the pointer, else
/// it is the selected one), or the hint for none.
pub(super) fn readout(
    outcome: &Sweep2dOutcome,
    cell: Option<(usize, usize)>,
    hovering: bool,
) -> Readout {
    let Some(found) = cell.and_then(|(col, row)| outcome.cell_at(col, row)) else {
        return Readout {
            head: READOUT_HINT.to_owned(),
            items: Vec::new(),
            notes: String::new(),
        };
    };
    let plan = &outcome.plan;
    let mut head = format!(
        "{}{} {:.2}\u{b0}, {} {:.2}\u{b0}",
        if hovering { "" } else { "Selected: " },
        plan.tier_a_name,
        found.angle_a.abs(),
        plan.tier_b_name,
        found.angle_b.abs()
    );
    if found.is_current {
        head.push_str(" (the design's own pair)");
    }
    let current = outcome.current_cell().and_then(|own| own.metrics);
    let items = match (found.metrics, current) {
        (Some(now), Some(was)) => figure_deltas(&was, &now)
            .into_iter()
            .filter(|delta| {
                // No warnings before and after is not worth a word.
                delta.figure != DeltaFigure::Warnings
                    || delta.after >= 0.5
                    || delta.change.abs() >= 0.5
            })
            .map(|delta| {
                // `delta.text` is "<value> <signed change>": the change goes in brackets so
                // it reads as the change against the design's own pair, not a second figure.
                let text = delta.text.rsplit_once(' ').map_or_else(
                    || delta.text.clone(),
                    |(value, change)| format!("{value} ({change})"),
                );
                (
                    format!("{} {text}", delta.figure.short_label()),
                    delta.tone.code(),
                )
            })
            .collect(),
        (Some(now), None) => plain_items(&now),
        (None, _) => Vec::new(),
    };
    let notes = match (found.metrics.is_some(), found.notes.is_empty()) {
        (true, true) => String::new(),
        (true, false) => found.notes_text(),
        (false, true) => "This pair gives no stone.".to_owned(),
        (false, false) => format!("This pair gives no stone: {}.", found.notes_text()),
    };
    Readout { head, items, notes }
}

/// The file name the CSV is offered under: `<design> <tier> and <tier> angle sweep.csv`.
pub(super) fn csv_file_name_2d(design_title: &str, tier_a: &str, tier_b: &str) -> String {
    let stem = format!("{design_title} {tier_a} and {tier_b} angle sweep");
    format!(
        "{}.csv",
        crate::gui::library::local::sanitize_filename(&stem)
    )
}

/// An image of `pixels`; an empty one gives the empty image.
fn image_of(pixels: &Pixels) -> slint::Image {
    let (Ok(width), Ok(height)) = (u32::try_from(pixels.width), u32::try_from(pixels.height))
    else {
        return slint::Image::default();
    };
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() == pixels.rgba.len() {
        bytes.copy_from_slice(&pixels.rgba);
    }
    rgba_image(buffer)
}

/// A count as the model's `int`.
fn int_of(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// The center of the cell `(column, row)` as fractions of the image, or "not shown".
fn center_of(cell: Option<(usize, usize)>, columns: usize, rows: usize) -> (f32, f32) {
    match cell {
        Some((col, row)) if col < columns && row < rows => (
            (col as f32 + 0.5) / columns as f32,
            (row as f32 + 0.5) / rows as f32,
        ),
        _ => (NOT_SHOWN, NOT_SHOWN),
    }
}

/// Copies everything the finished sweep shows into the model: the figure choice, the map and
/// its scale, the axes, the title and the marks; or clears them when there is no sweep.
pub(super) fn push_results(ui: &MainWindow, state: &TwoTier) {
    let model = ui.global::<SweepModel>();
    let Some(outcome) = &state.outcome else {
        model.set_has_results_2d(false);
        model.set_heat_figure_options(model_of(Vec::new()));
        model.set_heatmap_image(slint::Image::default());
        model.set_legend_image(slint::Image::default());
        model.set_heat_cols(0);
        model.set_heat_rows(0);
        model.set_heat_best_x(NOT_SHOWN);
        model.set_heat_best_y(NOT_SHOWN);
        push_marks(ui, state);
        return;
    };
    let metric = state.metric();
    let view = heat_view(outcome, metric, state.contour);
    let (columns, rows) = (outcome.plan.columns(), outcome.plan.rows());
    model.set_has_results_2d(true);
    model.set_result_title_2d(result_title(outcome).into());
    let options: Vec<SharedString> = state
        .figures()
        .into_iter()
        .map(|figure| figure_label(figure).into())
        .collect();
    model.set_heat_figure_options(model_of(options));
    model.set_heat_figure_index(int_of(state.figure_position()));
    model.set_contour(state.contour);
    model.set_heatmap_image(image_of(&view.heat));
    model.set_legend_image(image_of(&view.legend));
    model.set_heat_aspect(if view.heat.height == 0 {
        1.0
    } else {
        view.heat.width as f32 / view.heat.height as f32
    });
    model.set_heat_cols(int_of(columns));
    model.set_heat_rows(int_of(rows));
    model.set_heat_cell_w(1.0 / columns.max(1) as f32);
    model.set_heat_cell_h(1.0 / rows.max(1) as f32);
    model.set_legend_low(view.legend_low.into());
    model.set_legend_high(view.legend_high.into());
    model.set_legend_note(view.legend_note.into());
    model.set_across_label(axis_label(&outcome.plan.tier_a_name, &outcome.plan.angles_a).into());
    model.set_down_label(axis_label(&outcome.plan.tier_b_name, &outcome.plan.angles_b).into());
    let (best_x, best_y) = center_of(view.best, columns, rows);
    model.set_heat_best_x(best_x);
    model.set_heat_best_y(best_y);
    push_marks(ui, state);
}

/// Copies the marks that follow the pointer and the selection, the readout and the state of
/// "Use These Angles" and "Refine Around This".
pub(super) fn push_marks(ui: &MainWindow, state: &TwoTier) {
    let model = ui.global::<SweepModel>();
    let Some(outcome) = &state.outcome else {
        model.set_heat_current_x(NOT_SHOWN);
        model.set_heat_current_y(NOT_SHOWN);
        model.set_heat_selected_x(NOT_SHOWN);
        model.set_heat_selected_y(NOT_SHOWN);
        model.set_heat_hover_x(NOT_SHOWN);
        model.set_heat_hover_y(NOT_SHOWN);
        model.set_selected_cell_text("".into());
        model.set_readout_items(model_of(Vec::new()));
        model.set_readout_tones(model_of(Vec::new()));
        model.set_readout_notes("".into());
        model.set_cells_usable(false);
        model.set_cell_selected(false);
        return;
    };
    let (columns, rows) = (outcome.plan.columns(), outcome.plan.rows());
    let own = outcome.current_cell().map(|cell| (cell.col, cell.row));
    let (x, y) = center_of(own, columns, rows);
    model.set_heat_current_x(x);
    model.set_heat_current_y(y);
    let (x, y) = center_of(state.selected, columns, rows);
    model.set_heat_selected_x(x);
    model.set_heat_selected_y(y);
    let (x, y) = center_of(state.hover, columns, rows);
    model.set_heat_hover_x(x);
    model.set_heat_hover_y(y);
    let (cell, hovering) = match (state.hover, state.selected) {
        (Some(hover), _) => (Some(hover), true),
        (None, selected) => (selected, false),
    };
    let readout = readout(outcome, cell, hovering);
    model.set_selected_cell_text(readout.head.into());
    let (items, tones): (Vec<SharedString>, Vec<i32>) = readout
        .items
        .into_iter()
        .map(|(text, tone)| (SharedString::from(text), tone))
        .unzip();
    model.set_readout_items(model_of(items));
    model.set_readout_tones(model_of(tones));
    model.set_readout_notes(readout.notes.into());
    model.set_cells_usable(state.pick().is_some());
    model.set_cell_selected(state.selected.is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::sweep::{Sweep2dCell, Sweep2dPlan};

    fn figures(brilliance_pct: f32, windowing_pct: f32) -> RowMetrics {
        RowMetrics {
            brilliance_pct,
            windowing_pct,
            extinction_pct: 5.0,
            fire_index: 1.0,
            scintillation_pct: 20.0,
            yield_pct: None,
            warning_count: 0,
            has_girdle: true,
            tilt: None,
        }
    }

    /// Two columns (crown 34.5, 35.0) by two rows (pavilion -40.0, -41.0); the design's own
    /// pair is column 0, row 1; the pair at column 1, row 0 gives no stone.
    fn outcome() -> Sweep2dOutcome {
        let plan = Sweep2dPlan {
            tier_a: 2,
            tier_a_name: "Crown Main".to_owned(),
            current_a_deg: 34.5,
            angles_a: vec![34.5, 35.0],
            current_col: 0,
            tier_b: 5,
            tier_b_name: "Pavilion Main".to_owned(),
            current_b_deg: -41.0,
            angles_b: vec![-40.0, -41.0],
            current_row: 1,
        };
        let cell =
            |col: usize, row: usize, metrics: Option<RowMetrics>, notes: &[&str]| Sweep2dCell {
                col,
                row,
                angle_a: plan.angles_a[col],
                angle_b: plan.angles_b[row],
                metrics,
                notes: notes.iter().map(|note| (*note).to_owned()).collect(),
                is_current: col == 0 && row == 1,
            };
        let cells = vec![
            cell(0, 0, Some(figures(70.0, 20.0)), &[]),
            cell(1, 0, None, &["The girdle vanishes"]),
            cell(0, 1, Some(figures(75.0, 10.0)), &[]),
            cell(1, 1, Some(figures(80.0, 4.0)), &["A facet is very small"]),
        ];
        Sweep2dOutcome {
            plan,
            requested: 4,
            cancelled: false,
            tilt_average: false,
            cells,
        }
    }

    #[test]
    fn a_figure_has_a_title_case_name() {
        assert_eq!(figure_label(SweepMetric::Brilliance), "Brilliance");
        assert_eq!(figure_label(SweepMetric::TiltExtinction), "Tilt Extinction");
    }

    #[test]
    fn the_cells_become_blocks_hatches_and_gaps() {
        let mut stopped = outcome();
        stopped.cells.pop();
        stopped.cancelled = true;
        let figures = cell_figures(&stopped, SweepMetric::Brilliance);
        assert_eq!(
            figures,
            vec![
                CellFigure::Value(70.0),
                CellFigure::Invalid,
                CellFigure::Value(75.0),
                CellFigure::Pending,
            ]
        );
        // A valid cell without a yield has no value for it.
        assert_eq!(
            cell_figures(&stopped, SweepMetric::Yield)[0],
            CellFigure::Pending
        );
    }

    #[test]
    fn the_legend_names_the_dark_and_the_bright_end_in_the_figures_units() {
        assert_eq!(
            legend_texts(SweepMetric::Brilliance, Some((70.0, 80.0))),
            ("70.0 %".to_owned(), "80.0 %".to_owned())
        );
        // Lower is better: the bright end is the low value.
        assert_eq!(
            legend_texts(SweepMetric::Windowing, Some((4.0, 20.0))),
            ("20.0 %".to_owned(), "4.0 %".to_owned())
        );
        assert_eq!(
            legend_texts(SweepMetric::Fire, Some((1.0, 2.0))),
            ("1.0".to_owned(), "2.0".to_owned())
        );
        assert_eq!(
            legend_texts(SweepMetric::Fire, None),
            (String::new(), String::new())
        );
    }

    #[test]
    fn the_view_marks_the_best_pair_of_the_figure() {
        let outcome = outcome();
        let brilliance = heat_view(&outcome, SweepMetric::Brilliance, true);
        assert_eq!(brilliance.best, Some((1, 1)));
        let windowing = heat_view(&outcome, SweepMetric::Windowing, false);
        assert_eq!(windowing.best, Some((1, 1)), "lowest windowing");
        assert!(brilliance.heat.width > 0 && brilliance.heat.width == brilliance.heat.height);
        assert!(brilliance.legend_note.contains("outline"));
        assert!(!windowing.legend_note.contains("outline"));
    }

    #[test]
    fn an_axis_names_the_tier_and_its_range_as_magnitudes() {
        assert_eq!(
            axis_label("Pavilion Main", &[-40.0, -41.0, -42.5]),
            "Pavilion Main, 40.00\u{b0} to 42.50\u{b0}"
        );
        assert_eq!(axis_label("Crown Main", &[]), "Crown Main");
    }

    #[test]
    fn the_title_and_status_lines_count_the_pairs() {
        let mut outcome = outcome();
        assert_eq!(
            result_title(&outcome),
            "Crown Main and Pavilion Main: 4 angle pairs, 3 valid"
        );
        assert_eq!(finished_status(&outcome), "Done: 4 angle pairs, 3 valid.");
        assert_eq!(running_status(1, 4), "1 of 4 angle pairs done");
        outcome.cancelled = true;
        outcome.cells.truncate(2);
        assert_eq!(
            finished_status(&outcome),
            "Stopped: 2 of 4 angle pairs done."
        );
        assert!(result_title(&outcome).ends_with("(stopped, 4 were planned)"));
    }

    #[test]
    fn the_readout_gives_the_change_against_the_design_s_own_pair() {
        let outcome = outcome();
        let better = readout(&outcome, Some((1, 1)), false);
        assert_eq!(
            better.head,
            "Selected: Crown Main 35.00\u{b0}, Pavilion Main 41.00\u{b0}"
        );
        let brilliance = &better.items[0];
        assert_eq!(brilliance.0, "Bril. 80.0 % (+5.0)");
        assert_eq!(brilliance.1, 1, "5 points more is better");
        let windowing = better
            .items
            .iter()
            .find(|(text, _)| text.starts_with("Wind."))
            .expect("a windowing item");
        assert_eq!(windowing.0, "Wind. 4.0 % (\u{2212}6.0)");
        assert_eq!(windowing.1, 1, "less windowing is better");
        assert_eq!(better.notes, "A facet is very small");
        let worse = readout(&outcome, Some((0, 0)), true);
        assert!(worse.head.starts_with("Crown Main 34.50"), "{}", worse.head);
        assert_eq!(worse.items[0].1, 2, "5 points less is worse");
    }

    #[test]
    fn the_readout_of_the_own_pair_and_of_an_invalid_pair() {
        let outcome = outcome();
        let own = readout(&outcome, Some((0, 1)), true);
        assert!(
            own.head.ends_with("(the design's own pair)"),
            "{}",
            own.head
        );
        assert!(own.items.iter().all(|(_, tone)| *tone == 0));
        let invalid = readout(&outcome, Some((1, 0)), false);
        assert_eq!(invalid.items.len(), 0);
        assert_eq!(
            invalid.notes,
            "This pair gives no stone: The girdle vanishes."
        );
        let none = readout(&outcome, None, false);
        assert_eq!(none.head, READOUT_HINT);
        assert!(none.items.is_empty() && none.notes.is_empty());
    }

    #[test]
    fn the_cells_are_centered_as_fractions_of_the_image() {
        assert_eq!(center_of(Some((0, 0)), 4, 2), (0.125, 0.25));
        assert_eq!(center_of(Some((3, 1)), 4, 2), (0.875, 0.75));
        assert_eq!(center_of(Some((4, 0)), 4, 2), (NOT_SHOWN, NOT_SHOWN));
        assert_eq!(center_of(None, 4, 2), (NOT_SHOWN, NOT_SHOWN));
    }

    #[test]
    fn the_csv_is_offered_under_the_tiers_names() {
        let name = csv_file_name_2d("Fancy", "Crown Main", "Pavilion Main");
        assert!(
            std::path::Path::new(&name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
        );
        assert!(
            name.contains("Crown Main") && name.contains("Pavilion Main"),
            "{name}"
        );
    }
}
