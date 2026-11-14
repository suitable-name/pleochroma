//! Pushes the Cut page: the instruction rows, the progress card and which empty state
//! applies, and keeps the row selection that drives the diagram.

use std::cell::{Cell, RefCell};

use pleochroma_mobile_core::{
    app_state::AppState,
    cutting::{self, CutRow},
    document,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::diagram;
use crate::{AppWindow, CutModel, CutRowData};

thread_local! {
    /// The rows last given to the page, so an unchanged list is not rebuilt.
    static PUSHED: RefCell<Vec<CutRow>> = const { RefCell::new(Vec::new()) };
    /// The row the diagram highlights (a step position).
    static SELECTED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The page state a push settles on.
mod kind {
    /// There are instructions.
    pub(super) const READY: i32 = 0;
    /// The design has no tiers to cut.
    pub(super) const NO_TIERS: i32 = 1;
    /// The design has no current solve.
    pub(super) const NOT_SOLVED: i32 = 2;
}

/// Writes the Cut page's properties from `state`.
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<CutModel>();
    model.set_title(document::display_title(state).into());
    model.set_solving(state.solve.is_solving());

    let rows = cutting::instruction_rows(state);
    let summary = cutting::summary(state);
    let (Ok(rows), Ok(summary)) = (rows, summary) else {
        show_empty(&model, state);
        diagram::push(window, state, None);
        return;
    };

    model.set_state_kind(kind::READY);
    model.set_state_text("".into());
    model.set_progress_text(summary.text.as_str().into());
    model.set_progress(summary.fraction);
    model.set_done_count(i32::try_from(summary.done).unwrap_or(i32::MAX));
    model.set_total_count(i32::try_from(summary.total).unwrap_or(i32::MAX));

    let selected = SELECTED.get().filter(|step| *step < rows.len());
    SELECTED.set(selected);
    model.set_selected_row(selected.map_or(-1, |step| i32::try_from(step).unwrap_or(-1)));
    let tier = selected
        .and_then(|step| rows.get(step))
        .and_then(|row| row.tier_index);

    let unchanged = PUSHED.with(|pushed| *pushed.borrow() == rows);
    if !unchanged {
        let data: Vec<CutRowData> = rows.iter().map(row_data).collect();
        model.set_rows(ModelRc::new(VecModel::from(data)));
        PUSHED.with(|pushed| *pushed.borrow_mut() = rows);
    }
    diagram::push(window, state, tier);
}

/// Highlights `step`'s tier in the diagram (a tap on a row of a wide window). Selecting
/// the selected row again clears the selection.
pub(super) fn select(window: &AppWindow, state: &AppState, step: i32) {
    let step = usize::try_from(step).ok();
    let next = if step.is_some() && step == SELECTED.get() {
        None
    } else {
        step
    };
    SELECTED.set(next);
    push(window, state);
}

/// The empty-state properties: no tiers, or not solved (with the reason when it failed).
fn show_empty(model: &CutModel<'_>, state: &AppState) {
    PUSHED.with(|pushed| pushed.borrow_mut().clear());
    model.set_rows(ModelRc::new(VecModel::from(Vec::<CutRowData>::new())));
    model.set_selected_row(-1);
    model.set_done_count(0);
    model.set_total_count(0);
    model.set_progress(0.0);
    model.set_progress_text("".into());
    if state.session.design.tiers.is_empty() {
        model.set_state_kind(kind::NO_TIERS);
        model.set_state_text("".into());
        return;
    }
    model.set_state_kind(kind::NOT_SOLVED);
    let generation = state.session.current_generation();
    let text = if state.solve.is_solving() {
        "The design is being solved. The instructions appear when it is done.".to_owned()
    } else if let Some(message) = state.solve.failure(generation) {
        format!("The design could not be solved: {message}")
    } else {
        format!("{}.", cutting::NotSolved)
    };
    model.set_state_text(text.into());
}

/// One list row for the page.
fn row_data(row: &CutRow) -> CutRowData {
    // The sheet joins the positions with `-` (`96-08-16`); the page draws one chip each.
    let chips: Vec<slint::SharedString> = row.index_text.split('-').map(Into::into).collect();
    let chip_chars = row.index_text.split('-').map(|c| c.chars().count()).max();
    CutRowData {
        step: i32::try_from(row.step).unwrap_or(i32::MAX),
        code: row.code.as_str().into(),
        angle: row.angle_text.as_str().into(),
        index_text: row.index_text.as_str().into(),
        index_chips: ModelRc::new(VecModel::from(chips)),
        chip_chars: i32::try_from(chip_chars.unwrap_or(1).max(1)).unwrap_or(1),
        name: row.name.as_str().into(),
        notes: row.notes.as_str().into(),
        done: row.done,
        changed: row.changed,
        section: row.section.into(),
        section_start: row.section_start,
        tier: row
            .tier_index
            .map_or(-1, |tier| i32::try_from(tier).unwrap_or(-1)),
    }
}

#[cfg(test)]
mod tests {
    use slint::Model;

    use super::*;

    fn row(index_text: &str) -> CutRow {
        CutRow {
            step: 3,
            code: "C1".to_owned(),
            angle_text: "41.00\u{b0}".to_owned(),
            index_text: index_text.to_owned(),
            name: "Crown Main".to_owned(),
            notes: String::new(),
            done: false,
            changed: false,
            section: "Crown",
            section_start: false,
            tier_index: Some(2),
        }
    }

    fn chips(data: &CutRowData) -> Vec<String> {
        data.index_chips
            .iter()
            .map(|chip| chip.to_string())
            .collect()
    }

    #[test]
    fn the_index_text_splits_into_one_chip_per_position() {
        let data = row_data(&row("96-08-16"));
        assert_eq!(chips(&data), ["96", "08", "16"]);
        assert_eq!(
            data.index_text.as_str(),
            "96-08-16",
            "the joined text stays"
        );
        assert_eq!(data.chip_chars, 2);
    }

    #[test]
    fn chip_chars_is_the_longest_chip() {
        let data = row_data(&row("96-8-112"));
        assert_eq!(chips(&data), ["96", "8", "112"]);
        assert_eq!(data.chip_chars, 3);
    }

    #[test]
    fn one_position_is_one_chip() {
        let data = row_data(&row("48"));
        assert_eq!(chips(&data), ["48"]);
        assert_eq!(data.chip_chars, 2);
    }

    #[test]
    fn an_empty_index_text_gives_one_empty_chip_of_width_one() {
        let data = row_data(&row(""));
        assert_eq!(chips(&data), [""]);
        assert_eq!(data.chip_chars, 1, "the chip never collapses to zero width");
        assert_eq!(data.index_text.as_str(), "");
    }

    #[test]
    fn a_concave_tier_has_no_tier_index() {
        let mut concave = row("12");
        concave.tier_index = None;
        assert_eq!(row_data(&concave).tier, -1);
        assert_eq!(row_data(&row("12")).tier, 2);
    }
}
