//! What the Trials view shows: the rows with their figures, and the push into `TrialsModel`.
//! Everything but [`push`] and [`push_static`] is Slint-free.

use super::record::{axis_part, material_part};
use crate::{MainWindow, TrialCellItem, TrialRow, TrialsModel};
use pleochroma_editor::{
    live_deltas::DeltaFigure,
    sweep::RowMetrics,
    trials::{TrialCell, TrialRowView, TrialSort},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// The figure columns of the table, in order.
pub(super) const COLUMNS: [DeltaFigure; 5] = [
    DeltaFigure::Brilliance,
    DeltaFigure::Windowing,
    DeltaFigure::Extinction,
    DeltaFigure::Fire,
    DeltaFigure::Yield,
];

/// The orders the table can take, as the drop-down names them.
pub(super) const SORTS: [(&str, TrialSort); 6] = [
    ("Number", TrialSort::Number),
    ("Brilliance", TrialSort::Figure(DeltaFigure::Brilliance)),
    ("Windowing", TrialSort::Figure(DeltaFigure::Windowing)),
    ("Extinction", TrialSort::Figure(DeltaFigure::Extinction)),
    ("Fire", TrialSort::Figure(DeltaFigure::Fire)),
    ("Yield", TrialSort::Figure(DeltaFigure::Yield)),
];

/// The tooltip of a figure that was scored in another material than the design now has.
pub(super) const OTHER_MATERIAL: &str =
    "Scored in another material \u{2014} press Re-score Trials.";

/// The tooltip note of a figure scored in the same material under another Crystal Axis than the
/// design has now: the figure is real (a trial is scored in its own orientation), so it keeps
/// its tone and is not greyed.
pub(super) const OTHER_AXIS: &str = "Other Crystal Axis \u{2014} scored in its own orientation.";

/// The tooltip of a figure that has not been scored yet.
pub(super) const NOT_SCORED: &str = "Not scored yet.";

/// What a figure shows where there is none.
const DASH: &str = "\u{2014}";

/// The sentence that replaces the table while there are no trials.
pub(super) const EMPTY_TEXT: &str = "No trials yet. Press Keep as Trial (Ctrl+Shift+K) to keep the design as it is. Every sweep angle you use, every Optimize candidate you apply and every Retarget you apply is kept here too.";

/// The order for the drop-down's entry `index`; the first for one that does not exist.
#[must_use]
pub(super) fn sort_for(index: i32) -> TrialSort {
    usize::try_from(index)
        .ok()
        .and_then(|index| SORTS.get(index))
        .map_or(TrialSort::Number, |&(_, sort)| sort)
}

/// One figure of a row, as `TrialsModel` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CellView {
    pub text: String,
    pub change_text: String,
    /// 0 about the same, 1 better, 2 worse.
    pub tone: i32,
    pub best: bool,
    pub tooltip: String,
}

/// One row of the table (or the Baseline's).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct RowView {
    pub number: i32,
    pub label: String,
    pub note: String,
    pub origin: String,
    pub starred: bool,
    pub has_metrics: bool,
    /// One cell per entry of [`COLUMNS`].
    pub cells: Vec<CellView>,
}

/// Everything the view shows at one moment; pushed only where it differs from the last push.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct View {
    pub rows: Vec<RowView>,
    pub baseline: RowView,
    /// The selected trial's number, or -1.
    pub selected: i32,
    pub selected_label: String,
    pub selected_note: String,
    pub empty_text: String,
    pub design_open: bool,
    pub rescore_needed: bool,
    pub rescoring: bool,
}

/// A figure that is not there: a dash, with the reason as its tooltip.
fn missing_cell(tooltip: &str) -> CellView {
    CellView {
        text: DASH.to_owned(),
        change_text: String::new(),
        tone: 0,
        best: false,
        tooltip: tooltip.to_owned(),
    }
}

/// The cell for a figure the trial has. A figure scored in another material shows grey, with no
/// change and no best mark, until it is re-scored.
fn cell_of(cell: &TrialCell, elsewhere: bool, other_axis: bool) -> CellView {
    if elsewhere {
        return CellView {
            text: cell.text.clone(),
            change_text: String::new(),
            tone: 0,
            best: false,
            tooltip: OTHER_MATERIAL.to_owned(),
        };
    }
    let mut tooltip = match (cell.tooltip.is_empty(), cell.best) {
        (true, true) => "Best of the list.".to_owned(),
        (false, true) => format!("{} Best of the list.", cell.tooltip),
        _ => cell.tooltip.clone(),
    };
    if other_axis {
        tooltip = if tooltip.is_empty() {
            OTHER_AXIS.to_owned()
        } else {
            format!("{OTHER_AXIS} {tooltip}")
        };
    }
    CellView {
        text: cell.text.clone(),
        change_text: cell.change_text.clone(),
        tone: cell.tone.code(),
        best: cell.best,
        tooltip,
    }
}

/// The row for `row`, judged for the design whose trial key is `current_key`: a figure of
/// another material greys; one of the same material under another Crystal Axis says so.
#[must_use]
pub(super) fn row_of(row: &TrialRowView, current_key: &str) -> RowView {
    let judged = row.has_metrics && !current_key.is_empty();
    let elsewhere = judged && material_part(&row.material_key) != material_part(current_key);
    let other_axis = judged && !elsewhere && axis_part(&row.material_key) != axis_part(current_key);
    let cells = COLUMNS
        .iter()
        .map(
            |&figure| match row.cells.iter().find(|cell| cell.figure == figure) {
                Some(cell) => cell_of(cell, elsewhere, other_axis),
                None if row.has_metrics => missing_cell("This figure was not measured."),
                None => missing_cell(NOT_SCORED),
            },
        )
        .collect();
    RowView {
        number: i32::try_from(row.number).unwrap_or(i32::MAX),
        label: row.label.clone(),
        note: row.note.clone(),
        origin: row.origin.to_owned(),
        starred: row.starred,
        has_metrics: row.has_metrics,
        cells,
    }
}

/// The Baseline as the table's first line: its own figures, no change.
#[must_use]
pub(super) fn baseline_row_of(label: &str, metrics: Option<&RowMetrics>) -> RowView {
    let cells = COLUMNS
        .iter()
        .map(|&figure| {
            metrics
                .and_then(|metrics| figure.value(metrics))
                .map_or_else(
                    || missing_cell(NOT_SCORED),
                    |value| CellView {
                        text: figure.cell_text(value),
                        change_text: String::new(),
                        tone: 0,
                        best: false,
                        tooltip: "The Baseline's value.".to_owned(),
                    },
                )
        })
        .collect();
    RowView {
        number: 0,
        label: label.to_owned(),
        note: String::new(),
        origin: DASH.to_owned(),
        starred: false,
        has_metrics: metrics.is_some(),
        cells,
    }
}

fn to_slint(row: &RowView) -> TrialRow {
    let cells: Vec<TrialCellItem> = row
        .cells
        .iter()
        .map(|cell| TrialCellItem {
            text: cell.text.as_str().into(),
            change_text: cell.change_text.as_str().into(),
            tone: cell.tone,
            best: cell.best,
            tooltip: cell.tooltip.as_str().into(),
        })
        .collect();
    TrialRow {
        number: row.number,
        label: row.label.as_str().into(),
        note: row.note.as_str().into(),
        origin: row.origin.as_str().into(),
        starred: row.starred,
        has_metrics: row.has_metrics,
        cells: ModelRc::new(VecModel::from(cells)),
    }
}

fn strings(items: impl IntoIterator<Item = &'static str>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// The parts of `TrialsModel` that never change: the column names and the sort entries.
pub(super) fn push_static(ui: &MainWindow) {
    let model = ui.global::<TrialsModel>();
    model.set_columns(strings(COLUMNS.iter().map(|figure| figure.short_label())));
    model.set_sort_options(strings(SORTS.iter().map(|&(name, _)| name)));
}

/// Puts `view` into `TrialsModel`, setting only what differs from `last` (replacing the rows
/// would send the list back to the top).
pub(super) fn push(ui: &MainWindow, view: &View, last: Option<&View>) {
    let model = ui.global::<TrialsModel>();
    if last.is_none_or(|last| last.rows != view.rows) {
        let rows: Vec<TrialRow> = view.rows.iter().map(to_slint).collect();
        model.set_rows(ModelRc::new(VecModel::from(rows)));
    }
    if last.is_none_or(|last| last.baseline != view.baseline) {
        model.set_baseline_row(to_slint(&view.baseline));
    }
    if last.is_none_or(|last| last.selected != view.selected) {
        model.set_selected(view.selected);
    }
    if last.is_none_or(|last| last.selected_label != view.selected_label) {
        model.set_selected_label(view.selected_label.as_str().into());
    }
    if last.is_none_or(|last| last.selected_note != view.selected_note) {
        model.set_selected_note(view.selected_note.as_str().into());
    }
    if last.is_none_or(|last| last.empty_text != view.empty_text) {
        model.set_empty_text(view.empty_text.as_str().into());
    }
    if last.is_none_or(|last| last.design_open != view.design_open) {
        model.set_design_open(view.design_open);
    }
    if last.is_none_or(|last| last.rescore_needed != view.rescore_needed) {
        model.set_rescore_needed(view.rescore_needed);
    }
    if last.is_none_or(|last| last.rescoring != view.rescoring) {
        model.set_rescoring(view.rescoring);
    }
}
