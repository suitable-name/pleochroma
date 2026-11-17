//! What the Baseline strip shows: the label, the figure cells and the sentences, and the push
//! into `BaselineModel`. Everything but [`push`] is Slint-free.

use crate::{BaselineCell, BaselineModel, MainWindow};
use pleochroma_cut_core::MaterialSelection;
use pleochroma_editor::live_deltas::{DeltaFigure, FigureDelta};
use slint::{ComponentHandle, ModelRc, VecModel};

/// One figure of the strip, as `BaselineModel.cells` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CellView {
    pub short_label: String,
    pub text: String,
    /// 0 about the same, 1 better, 2 worse.
    pub tone: i32,
    pub tooltip: String,
    /// The figure's full name ("Brilliance").
    pub name: String,
    /// The current value alone ("21.0 %").
    pub value: String,
    /// The signed change alone ("+1.8").
    pub delta: String,
    /// The value is outside the usual range (windowing above 10 %, extinction above 12 %).
    pub flag: bool,
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

/// The place of a figure in the cells: Live Render's order (brilliance, fire, scintillation,
/// windowing, extinction), then Contrast, then the rest as `DeltaFigure::ALL` has them. The
/// remaining figures rank from 10 up (index 5 and above, plus 5), so Contrast's 5 sits between.
const fn display_rank(figure: DeltaFigure) -> usize {
    match figure {
        DeltaFigure::Brilliance => 0,
        DeltaFigure::Fire => 1,
        DeltaFigure::Scintillation => 2,
        DeltaFigure::Windowing => 3,
        DeltaFigure::Extinction => 4,
        DeltaFigure::Contrast => 5,
        other => 5 + other.index(),
    }
}

/// The value and the signed change of a strip text such as `21.0 % +1.8`.
fn split_text(text: &str) -> (String, String) {
    text.rsplit_once(' ').map_or_else(
        || (text.to_owned(), String::new()),
        |(value, change)| (value.to_owned(), change.to_owned()),
    )
}

/// The cells of `deltas`, one per figure, in Live Render's figure order.
#[must_use]
pub(super) fn cells_of(deltas: &[FigureDelta]) -> Vec<CellView> {
    let mut ordered: Vec<&FigureDelta> = deltas.iter().collect();
    ordered.sort_by_key(|delta| display_rank(delta.figure));
    ordered
        .into_iter()
        .map(|delta| {
            let (value, change) = split_text(&delta.text);
            CellView {
                short_label: delta.figure.short_label().to_owned(),
                text: delta.text.clone(),
                tone: delta.tone.code(),
                tooltip: delta.tooltip.clone(),
                name: delta.figure.label().to_owned(),
                value,
                delta: change,
                flag: match delta.figure {
                    DeltaFigure::Windowing => delta.after > 10.0,
                    DeltaFigure::Extinction => delta.after > 12.0,
                    _ => false,
                },
            }
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
            name: cell.name.as_str().into(),
            value: cell.value.as_str().into(),
            delta: cell.delta.as_str().into(),
            flag: cell.flag,
        })
        .collect();
    model.set_cells(ModelRc::new(VecModel::from(cells)));
    model.set_stale(view.stale);
    model.set_busy(view.busy);
    model.set_measuring_tilt(view.measuring_tilt);
    model.set_note(view.note.as_str().into());
}
