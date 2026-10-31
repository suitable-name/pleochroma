//! What the Baseline strip shows: the label, the figure cells and the sentences, and the push
//! into `BaselineModel`. Everything but [`push`] is Slint-free.

use crate::{BaselineCell, BaselineModel, MainWindow};
use pleochroma_cut_core::MaterialSelection;
use pleochroma_editor::live_deltas::FigureDelta;
use slint::{ComponentHandle, ModelRc, VecModel};

/// One figure of the strip, as `BaselineModel.cells` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CellView {
    pub short_label: String,
    pub text: String,
    /// 0 about the same, 1 better, 2 worse.
    pub tone: i32,
    pub tooltip: String,
}

/// Everything the strip shows at one moment; pushed only when it differs from the last push.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a view model: one flag per piece of the strip the window shows"
)]
pub(super) struct View {
    pub has_baseline: bool,
    pub label: String,
    pub cells: Vec<CellView>,
    pub stale: bool,
    pub busy: bool,
    pub measuring_tilt: bool,
    pub note: String,
}

/// How long ago, in words: `just now`, `4 min ago`, `2 h ago`.
#[must_use]
pub(super) fn age_text(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", seconds / 60),
        _ => format!("{} h ago", seconds / 3600),
    }
}

/// The strip's label: `Baseline: opened 4 min ago`.
#[must_use]
pub(super) fn label_text(origin: &str, seconds: u64) -> String {
    format!("Baseline: {origin} {}", age_text(seconds))
}

/// The cells of `deltas`, one per figure.
#[must_use]
pub(super) fn cells_of(deltas: &[FigureDelta]) -> Vec<CellView> {
    deltas
        .iter()
        .map(|delta| CellView {
            short_label: delta.figure.short_label().to_owned(),
            text: delta.text.clone(),
            tone: delta.tone.code(),
            tooltip: delta.tooltip.clone(),
        })
        .collect()
}

/// A material as a sentence names it: the picked name, `Custom RI`, or the design's own index.
#[must_use]
pub(super) fn material_label(selection: &MaterialSelection) -> String {
    match (&selection.name, selection.refractive_index_override) {
        (Some(name), _) => name.clone(),
        (None, Some(_)) => "Custom RI".to_owned(),
        (None, None) => "the design's refractive index".to_owned(),
    }
}

/// Puts `view` into `BaselineModel`.
pub(super) fn push(ui: &MainWindow, view: &View) {
    let model = ui.global::<BaselineModel>();
    model.set_has_baseline(view.has_baseline);
    model.set_label(view.label.as_str().into());
    let cells: Vec<BaselineCell> = view
        .cells
        .iter()
        .map(|cell| BaselineCell {
            short_label: cell.short_label.as_str().into(),
            text: cell.text.as_str().into(),
            tone: cell.tone,
            tooltip: cell.tooltip.as_str().into(),
        })
        .collect();
    model.set_cells(ModelRc::new(VecModel::from(cells)));
    model.set_stale(view.stale);
    model.set_busy(view.busy);
    model.set_measuring_tilt(view.measuring_tilt);
    model.set_note(view.note.as_str().into());
}
