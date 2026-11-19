//! The RI range field of the Design Settings sheet's MATERIAL section.
//!
//! The sheet's "RI range" row picks who states the range the cutting sheet prints:
//!
//! - Off: the design's own index, no range (the default when the file states none);
//! - From File: the range the file itself states (`.psdd` `[meta]`, or a `.gcs` `ri_min` /
//!   `ri_max` the importer carried in), offered only when it states one;
//! - Pleochroma estimate: the traced range ([`super::latest`]), labelled as an estimate;
//! - Type: the user's own statement, printed plain, with an optional high end.
//!
//! The open design holds the choice (`Design::ri_range_choice`), so it is an ordinary undoable edit
//! ([`Edit::SetRiRangeChoice`]): Ctrl+Z reverts it and Ctrl+Y applies it again. The file keeps it
//! in the `[meta]` table (the three keys `ri_min`, `ri_max`, `ri_range_source`): opening a design
//! copies it into the design (`EditorState::replace_wholesale`) and every write copies it back
//! (`EditorState::file_extras_for_write`). The estimate is stored as the choice only, never as a
//! number: its number is traced when the sheet is written ([`super::export_wait`]). There is no
//! automatic prefill: the field is Off until a person picks another source.
//!
//! Setting the field is an edit that moves no geometry: a solve or an Optimize result that was
//! current stays current (`EditorState::keep_results_current`), and no re-solve runs
//! (`resolve_after_edit` treats it like a design-details edit). The preview line under the row
//! shows what the sheet's "RI range" row will print.
//!
//! [`setup`] registers every `RiFieldModel` callback. The sheet asks Rust to write the stored
//! choice back into the model (`refresh_requested`) when it opens, when Esc drops an open edit and
//! when another design is underneath. Undo, Redo and a jump along the history call [`refresh`]
//! too, so an open sheet follows them.

use super::{export_wait, latest, when_ready};
use crate::{
    EditorModel, MainWindow, RiFieldModel, UndoRedoLabels,
    gui::editor::state::{EditorState, coalesce_timestamp},
};
use pleochroma_cut_core::{Edit, native::RiRangeChoice};
use pleochroma_editor::{
    material_lookup::material_for_refractive_index,
    ri_range::{RiSource, RiStatement, range_text},
};
use slint::{ComponentHandle as _, SharedString};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

/// `RiFieldModel.source_index` for Off.
const SOURCE_OFF: i32 = 0;
/// `RiFieldModel.source_index` for From File.
const SOURCE_FILE: i32 = 1;
/// `RiFieldModel.source_index` for the Pleochroma estimate.
const SOURCE_ESTIMATE: i32 = 2;
/// `RiFieldModel.source_index` for Type.
const SOURCE_TYPED: i32 = 3;

/// The lowest index a typed range may state (exclusive): a refractive index is above 1.
const LOWEST_TYPED_RI: f64 = 1.0;
/// The highest index a typed range may state: above diamond, below any gem.
const HIGHEST_TYPED_RI: f64 = 4.0;

/// The coalescing key of the typed boxes: every edit of them within the history's window is one
/// undo step. Spells "RI range" in ASCII.
const TYPED_COALESCE_KEY: u64 = 0x5249_2052_414E_4745;

/// The range a file states, as `(low, high)`.
type StatedRange = (f64, Option<f64>);

thread_local! {
    /// The editor state, for the refresh that runs when the traced range lands. A weak handle:
    /// the window's own callbacks hold the strong ones.
    static STATE: RefCell<Weak<RefCell<EditorState>>> = const { RefCell::new(Weak::new()) };
    /// Whether a refresh already waits for the traced range.
    static WAITING: Cell<bool> = const { Cell::new(false) };
}

/// What the preview says about the Pleochroma estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Estimate {
    /// The worker has no result for the design yet.
    Computing,
    /// The trace found no range.
    NoRange,
    /// The traced range as the sheet would print it.
    Found(RiStatement),
}

/// Whether two stated ranges are the same numbers.
fn same_range(a: StatedRange, b: StatedRange) -> bool {
    let same_high = match (a.1, b.1) {
        (None, None) => true,
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => false,
    };
    (a.0 - b.0).abs() < 1e-9 && same_high
}

/// The segment shown as picked for the stored `choice`.
///
/// `current` is the segment shown now, or `None` when the model is written afresh (the sheet
/// opened, another design). While Type is shown it stays shown: its boxes are the user's to fill
/// even when nothing is stored yet, and a typed range equal to the file's stays Type.
fn source_for(choice: RiRangeChoice, file: Option<StatedRange>, current: Option<i32>) -> i32 {
    let typing = current == Some(SOURCE_TYPED);
    match choice {
        RiRangeChoice::Off if typing => SOURCE_TYPED,
        RiRangeChoice::Off => SOURCE_OFF,
        RiRangeChoice::Estimate => SOURCE_ESTIMATE,
        RiRangeChoice::Designer { low, high } => {
            if !typing && file.is_some_and(|stated| same_range(stated, (low, high))) {
                SOURCE_FILE
            } else {
                SOURCE_TYPED
            }
        }
    }
}

/// A number with at most four decimals and no trailing zeros (`1.63`, `2`).
fn number_text(value: f64) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The two typed boxes for the stored `choice`: the designer's range, else empty.
fn typed_texts(choice: RiRangeChoice) -> (String, String) {
    match choice {
        RiRangeChoice::Designer { low, high } => {
            (number_text(low), high.map_or_else(String::new, number_text))
        }
        RiRangeChoice::Off | RiRangeChoice::Estimate => (String::new(), String::new()),
    }
}

/// One typed index: a number between [`LOWEST_TYPED_RI`] and [`HIGHEST_TYPED_RI`]. A comma works as
/// the decimal mark.
fn parse_index(what: &str, text: &str) -> Result<f64, String> {
    text.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > LOWEST_TYPED_RI && *value <= HIGHEST_TYPED_RI)
        .ok_or_else(|| format!("{what} must be a number between 1.00 and 4.00, such as 1.63."))
}

/// What the two typed boxes state.
///
/// Both empty is [`RiRangeChoice::Off`] (Type is picked, nothing typed yet). The high end is
/// optional; one below the low end is refused.
fn parse_typed(low: &str, high: &str) -> Result<RiRangeChoice, String> {
    let (low, high) = (low.trim(), high.trim());
    if low.is_empty() && high.is_empty() {
        return Ok(RiRangeChoice::Off);
    }
    if low.is_empty() {
        return Err("Type the lowest RI too, or clear the highest.".to_string());
    }
    let low = parse_index("Lowest RI", low)?;
    let high = if high.is_empty() {
        None
    } else {
        Some(parse_index("Highest RI", high)?)
    };
    if high.is_some_and(|high| high < low) {
        return Err("The highest RI cannot be below the lowest.".to_string());
    }
    Ok(RiRangeChoice::Designer { low, high })
}

/// What picking segment `index` stores, given what the file states and what is typed.
fn choice_for_source(
    index: i32,
    file: Option<StatedRange>,
    typed: (&str, &str),
) -> Result<RiRangeChoice, String> {
    match index {
        SOURCE_OFF => Ok(RiRangeChoice::Off),
        SOURCE_FILE => file
            .map(|(low, high)| RiRangeChoice::Designer { low, high })
            .ok_or_else(|| "This design's file states no RI range.".to_string()),
        SOURCE_ESTIMATE => Ok(RiRangeChoice::Estimate),
        SOURCE_TYPED => parse_typed(typed.0, typed.1),
        other => Err(format!("RI range source {other} does not exist.")),
    }
}

/// The material a sheet line names in brackets: the design's own material, else the nearest
/// built-in one with a question mark (a guess, never a fact), else `None`.
///
/// The rule of the sheet's own "RI range" row, so the preview and the printed row agree.
fn material_label(name: Option<&str>, n_d: Option<f64>) -> Option<String> {
    if let Some(name) = name {
        return Some(name.to_string());
    }
    let (guess, _) = material_for_refractive_index(n_d?)?;
    Some(format!("{guess}?"))
}

/// The "RI range" row of the cutting sheet on one line: the design's own index with its material
/// when no range is stated, else the range with its source and the material in brackets.
///
/// The sheet prints the second part on a second line; the preview joins the two with a space.
fn row_text(material: Option<&str>, n_d: Option<f64>, statement: Option<RiStatement>) -> String {
    let own = match (material, n_d) {
        (Some(material), Some(n)) => format!("{material} {n:.3}"),
        (None, Some(n)) => format!("{n:.3}"),
        (Some(material), None) => material.to_string(),
        (None, None) => String::new(),
    };
    let Some(statement) = statement else {
        return match (material, n_d) {
            (Some(material), Some(n)) => format!("{n:.3} ({material})"),
            _ if own.is_empty() => "-".to_string(),
            _ => own,
        };
    };
    let range = range_text(&statement);
    match (statement.source, own.is_empty()) {
        (RiSource::Designer, true) => range,
        (RiSource::Designer, false) => format!("{range} ({own})"),
        (RiSource::Estimate, true) => format!("{range} (Pleochroma estimate)"),
        (RiSource::Estimate, false) => format!("{range} (Pleochroma estimate; {own})"),
    }
}

/// The preview line: what the sheet's "RI range" row will print for the stored `choice`.
fn preview_text(
    choice: RiRangeChoice,
    estimate: Estimate,
    material: Option<&str>,
    n_d: Option<f64>,
) -> String {
    match choice {
        RiRangeChoice::Off => row_text(material, n_d, None),
        RiRangeChoice::Designer { low, high } => row_text(
            material,
            n_d,
            Some(RiStatement {
                low,
                high,
                source: RiSource::Designer,
            }),
        ),
        RiRangeChoice::Estimate => match estimate {
            Estimate::Found(statement) => row_text(material, n_d, Some(statement)),
            Estimate::Computing => "Pleochroma estimate (computing\u{2026})".to_string(),
            Estimate::NoRange => "Pleochroma estimate (no range found)".to_string(),
        },
    }
}

/// Applies `choice` to the design as one undoable edit. `false` when the design already holds it
/// (no history step is made) or the edit is refused.
///
/// The edit moves the generation, so the design reads as unsaved and the autosave sees it. A
/// Deep Solve verdict or Optimize result that was current stays current
/// ([`EditorState::keep_results_current`]): the range changes no geometry.
fn apply_choice(st: &mut EditorState, choice: RiRangeChoice) -> bool {
    apply_edit_of(st, choice, None)
}

/// [`apply_choice`] for a choice typed into the two boxes: the keystrokes of one typing run merge
/// into one undo step ([`TYPED_COALESCE_KEY`]), so Ctrl+Z takes back the whole range at once.
fn apply_typed_choice(st: &mut EditorState, choice: RiRangeChoice) -> bool {
    apply_edit_of(st, choice, Some(TYPED_COALESCE_KEY))
}

/// The edit behind [`apply_choice`] and [`apply_typed_choice`]; `coalesce_key` merges it into the
/// previous step of the same key when that step is recent enough.
fn apply_edit_of(st: &mut EditorState, choice: RiRangeChoice, coalesce_key: Option<u64>) -> bool {
    if st.design.ri_range_choice == choice {
        return false;
    }
    let generation_before = st.current_generation();
    let edit = Edit::SetRiRangeChoice { choice };
    let applied = match coalesce_key {
        Some(key) => st.session.apply_coalescing(edit, key, coalesce_timestamp()),
        None => st.session.apply(edit),
    };
    if applied.is_err() {
        return false;
    }
    st.keep_results_current(generation_before);
    true
}

/// Writes the unsaved mark and the Undo and Redo state of `st` into the window, after an edit
/// of the choice: the Edit menu and the hover hints read them.
fn push_history_state(ui: &MainWindow, st: &EditorState) {
    let editor = ui.global::<EditorModel>();
    editor.set_is_dirty(st.is_dirty());
    editor.set_can_undo(st.history.can_undo());
    editor.set_can_redo(st.history.can_redo());
    let labels = ui.global::<UndoRedoLabels>();
    labels.set_undo_label(st.undo_hint().into());
    labels.set_redo_label(st.redo_hint().into());
}

/// Writes the choice of `st` afresh into the model: the picked segment, the typed boxes, no error.
///
/// What Undo, Redo and a jump along the history call once the design has changed, so an open
/// Design Settings sheet shows the choice the design holds now.
pub(in crate::gui::editor) fn refresh(ui: &MainWindow, st: &EditorState) {
    push(ui, st, true);
}

/// The design's effective refractive index as the Design sheet shows it.
fn effective_index(ui: &MainWindow) -> Option<f64> {
    ui.global::<EditorModel>()
        .get_effective_ri_text()
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
}

/// What the worker has for the design now.
fn estimate_now() -> Estimate {
    latest().map_or(Estimate::Computing, |range| {
        range.statement().map_or(Estimate::NoRange, Estimate::Found)
    })
}

/// Writes the stored choice of `st` into the model.
///
/// `reset` writes everything afresh: the picked segment, the typed boxes, no error. Without it the
/// segment on show stays (Type with empty boxes), and the boxes and the error are the user's.
fn push(ui: &MainWindow, st: &EditorState, reset: bool) {
    let model = ui.global::<RiFieldModel>();
    let choice = st.design.ri_range_choice;
    let file = st.file_extras.file_ri_range;
    let current = (!reset).then(|| model.get_source_index());
    model.set_source_index(source_for(choice, file, current));
    model.set_file_states_range(file.is_some());
    if reset {
        let (low, high) = typed_texts(choice);
        model.set_typed_low(low.into());
        model.set_typed_high(high.into());
        model.set_error(SharedString::new());
    }
    let n_d = effective_index(ui);
    let material = material_label(st.design.material.name.as_deref(), n_d);
    let estimate = estimate_now();
    model.set_preview(preview_text(choice, estimate, material.as_deref(), n_d).into());
    if choice == RiRangeChoice::Estimate && estimate == Estimate::Computing {
        refresh_when_traced(ui);
    }
}

/// Refreshes the preview once the worker lands the range, so "(computing...)" turns into the
/// number while the sheet is open. One waiter at a time.
fn refresh_when_traced(ui: &MainWindow) {
    if WAITING.with(|waiting| waiting.replace(true)) {
        return;
    }
    when_ready(ui, |ui, _range| {
        WAITING.with(|waiting| waiting.set(false));
        let Some(state) = STATE.with(|slot| slot.borrow().upgrade()) else {
            return;
        };
        if let Ok(st) = state.try_borrow() {
            push(ui, &st, false);
        }
    });
}

/// Answers a pick of segment `index`.
fn on_source_chosen(ui: &MainWindow, state: &Rc<RefCell<EditorState>>, index: i32) {
    let Ok(mut st) = state.try_borrow_mut() else {
        return;
    };
    let model = ui.global::<RiFieldModel>();
    // Switching to Type shows the range the design states now, to edit.
    if index == SOURCE_TYPED
        && model.get_typed_low().is_empty()
        && model.get_typed_high().is_empty()
    {
        let (low, high) = typed_texts(st.design.ri_range_choice);
        model.set_typed_low(low.into());
        model.set_typed_high(high.into());
    }
    let (low, high) = (model.get_typed_low(), model.get_typed_high());
    let typed = (low.as_str(), high.as_str());
    match choice_for_source(index, st.file_extras.file_ri_range, typed) {
        Ok(choice) => {
            model.set_error(SharedString::new());
            model.set_source_index(index);
            if apply_choice(&mut st, choice) {
                push_history_state(ui, &st);
            }
            push(ui, &st, false);
        }
        Err(message) => {
            model.set_error(message.into());
            if index == SOURCE_TYPED {
                model.set_source_index(index);
            }
        }
    }
}

/// Answers an edit of one of the two typed boxes.
fn on_typed_edited(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let model = ui.global::<RiFieldModel>();
    if model.get_source_index() != SOURCE_TYPED {
        return;
    }
    let Ok(mut st) = state.try_borrow_mut() else {
        return;
    };
    let (low, high) = (model.get_typed_low(), model.get_typed_high());
    match parse_typed(low.as_str(), high.as_str()) {
        Ok(choice) => {
            model.set_error(SharedString::new());
            if apply_typed_choice(&mut st, choice) {
                push_history_state(ui, &st);
            }
            push(ui, &st, false);
        }
        Err(message) => model.set_error(message.into()),
    }
}

/// Registers every `RiFieldModel` callback against the editor `state`.
///
/// Called once from the editor's own set-up, which owns the state. The contract's common
/// `setup(ui, db, render_ctx, settings_store)` form cannot reach it.
pub(in crate::gui::editor) fn setup(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    STATE.with(|slot| *slot.borrow_mut() = Rc::downgrade(state));
    let model = ui.global::<RiFieldModel>();

    let (ui_weak, state_chosen) = (ui.as_weak(), Rc::clone(state));
    model.on_source_chosen(move |index| {
        if let Some(ui) = ui_weak.upgrade() {
            on_source_chosen(&ui, &state_chosen, index);
        }
    });

    let (ui_weak, state_typed) = (ui.as_weak(), Rc::clone(state));
    model.on_typed_edited(move || {
        if let Some(ui) = ui_weak.upgrade() {
            on_typed_edited(&ui, &state_typed);
        }
    });

    let (ui_weak, state_refresh) = (ui.as_weak(), Rc::clone(state));
    model.on_refresh_requested(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if let Ok(st) = state_refresh.try_borrow() {
            push(&ui, &st, true);
        }
    });

    let ui_weak = ui.as_weak();
    model.on_cancel_export_wait(move || {
        if let Some(ui) = ui_weak.upgrade() {
            export_wait::cancel(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: Option<StatedRange> = Some((1.7, Some(2.15)));

    fn designer(low: f64, high: Option<f64>) -> RiRangeChoice {
        RiRangeChoice::Designer { low, high }
    }

    fn designer_statement(low: f64, high: Option<f64>) -> RiStatement {
        RiStatement {
            low,
            high,
            source: RiSource::Designer,
        }
    }

    fn estimate_statement(low: f64, high: Option<f64>) -> RiStatement {
        RiStatement {
            low,
            high,
            source: RiSource::Estimate,
        }
    }

    #[test]
    fn the_stored_choice_picks_the_segment() {
        assert_eq!(source_for(RiRangeChoice::Off, None, None), SOURCE_OFF);
        assert_eq!(
            source_for(RiRangeChoice::Estimate, None, None),
            SOURCE_ESTIMATE
        );
        // The file's own range is From File, any other designer range is Type.
        assert_eq!(
            source_for(designer(1.7, Some(2.15)), FILE, None),
            SOURCE_FILE
        );
        assert_eq!(source_for(designer(1.63, None), FILE, None), SOURCE_TYPED);
        assert_eq!(
            source_for(designer(1.7, Some(2.15)), None, None),
            SOURCE_TYPED
        );
        assert_eq!(source_for(designer(1.7, None), FILE, None), SOURCE_TYPED);
    }

    #[test]
    fn type_stays_shown_while_the_boxes_are_empty_or_hold_the_files_numbers() {
        let typing = Some(SOURCE_TYPED);
        assert_eq!(source_for(RiRangeChoice::Off, FILE, typing), SOURCE_TYPED);
        assert_eq!(
            source_for(designer(1.7, Some(2.15)), FILE, typing),
            SOURCE_TYPED
        );
        // Any other segment on show follows the stored choice.
        assert_eq!(
            source_for(RiRangeChoice::Off, FILE, Some(SOURCE_ESTIMATE)),
            SOURCE_OFF
        );
        assert_eq!(
            source_for(RiRangeChoice::Estimate, FILE, typing),
            SOURCE_ESTIMATE
        );
    }

    #[test]
    fn typed_boxes_hold_only_a_designers_range() {
        assert_eq!(
            typed_texts(designer(1.63, None)),
            ("1.63".to_string(), String::new())
        );
        assert_eq!(
            typed_texts(designer(1.7, Some(2.0))),
            ("1.7".to_string(), "2".to_string())
        );
        assert_eq!(
            typed_texts(RiRangeChoice::Off),
            (String::new(), String::new())
        );
        assert_eq!(
            typed_texts(RiRangeChoice::Estimate),
            (String::new(), String::new())
        );
    }

    #[test]
    fn a_low_end_alone_is_an_open_range() {
        assert_eq!(parse_typed("1.63", ""), Ok(designer(1.63, None)));
        assert_eq!(parse_typed(" 1.63 ", "  "), Ok(designer(1.63, None)));
    }

    #[test]
    fn a_range_with_both_ends_is_kept_as_typed() {
        assert_eq!(parse_typed("1.54", "1.80"), Ok(designer(1.54, Some(1.80))));
        // A comma is a decimal mark; equal ends are one index stated twice.
        assert_eq!(parse_typed("1,54", "1,54"), Ok(designer(1.54, Some(1.54))));
    }

    #[test]
    fn nothing_typed_is_off_with_type_still_shown() {
        assert_eq!(parse_typed("", ""), Ok(RiRangeChoice::Off));
        assert_eq!(parse_typed("  ", " "), Ok(RiRangeChoice::Off));
    }

    #[test]
    fn typed_text_that_is_no_index_is_refused_with_a_reason() {
        for (low, high) in [("abc", ""), ("1.6x", ""), ("1.63", "high"), ("", "2.1")] {
            assert!(parse_typed(low, high).is_err(), "{low:?} {high:?}");
        }
        for out_of_range in ["0", "1", "0.5", "-1.6", "4.5", "inf", "NaN"] {
            let message = parse_typed(out_of_range, "").unwrap_err();
            assert!(
                message.starts_with("Lowest RI must be a number"),
                "{message}"
            );
        }
        assert_eq!(
            parse_typed("1.63", "9").unwrap_err(),
            "Highest RI must be a number between 1.00 and 4.00, such as 1.63."
        );
        assert_eq!(
            parse_typed("1.90", "1.60").unwrap_err(),
            "The highest RI cannot be below the lowest."
        );
        assert_eq!(
            parse_typed("", "2.1").unwrap_err(),
            "Type the lowest RI too, or clear the highest."
        );
    }

    #[test]
    fn each_segment_stores_what_it_names() {
        let typed = ("1.63", "");
        assert_eq!(
            choice_for_source(SOURCE_OFF, FILE, typed),
            Ok(RiRangeChoice::Off)
        );
        assert_eq!(
            choice_for_source(SOURCE_ESTIMATE, None, typed),
            Ok(RiRangeChoice::Estimate)
        );
        assert_eq!(
            choice_for_source(SOURCE_FILE, FILE, typed),
            Ok(designer(1.7, Some(2.15)))
        );
        assert_eq!(
            choice_for_source(SOURCE_TYPED, FILE, typed),
            Ok(designer(1.63, None))
        );
        // From File without a stated range is refused, and nothing invents one.
        assert!(choice_for_source(SOURCE_FILE, None, typed).is_err());
        assert!(choice_for_source(7, FILE, typed).is_err());
    }

    #[test]
    fn the_estimate_is_never_a_number_in_the_choice() {
        // Whatever the worker holds, picking the estimate stores the choice alone.
        let choice = choice_for_source(SOURCE_ESTIMATE, FILE, ("1.63", "2.31")).unwrap();
        assert_eq!(choice, RiRangeChoice::Estimate);
        assert_eq!(typed_texts(choice), (String::new(), String::new()));
    }

    #[test]
    fn the_preview_reads_as_the_sheet_row_does() {
        let sapphire = Some("Sapphire");
        let n = Some(1.768);
        assert_eq!(
            preview_text(RiRangeChoice::Off, Estimate::Computing, sapphire, n),
            "1.768 (Sapphire)"
        );
        assert_eq!(
            preview_text(designer(1.63, None), Estimate::Computing, sapphire, n),
            "1.63 and up (Sapphire 1.768)"
        );
        assert_eq!(
            preview_text(designer(1.54, Some(1.80)), Estimate::Computing, sapphire, n),
            "1.54 to 1.80 (Sapphire 1.768)"
        );
        assert_eq!(
            preview_text(designer(1.70, Some(2.15)), Estimate::Computing, sapphire, n),
            "1.70 to 2.15 (Sapphire 1.768)"
        );
    }

    #[test]
    fn the_estimate_is_labelled_and_waits_for_the_worker() {
        let sapphire = Some("Sapphire");
        let n = Some(1.768);
        assert_eq!(
            preview_text(RiRangeChoice::Estimate, Estimate::Computing, sapphire, n),
            "Pleochroma estimate (computing\u{2026})"
        );
        assert_eq!(
            preview_text(
                RiRangeChoice::Estimate,
                Estimate::Found(estimate_statement(1.63, None)),
                sapphire,
                n
            ),
            "1.63 and up (Pleochroma estimate; Sapphire 1.768)"
        );
        // The calibrated form: both ends.
        assert_eq!(
            preview_text(
                RiRangeChoice::Estimate,
                Estimate::Found(estimate_statement(1.63, Some(2.31))),
                sapphire,
                n
            ),
            "1.63 to 2.31 (Pleochroma estimate; Sapphire 1.768)"
        );
        assert_eq!(
            preview_text(RiRangeChoice::Estimate, Estimate::NoRange, sapphire, n),
            "Pleochroma estimate (no range found)"
        );
    }

    #[test]
    fn a_guessed_material_keeps_its_question_mark_and_a_missing_one_is_left_out() {
        assert_eq!(
            row_text(Some("Quartz?"), Some(1.54), None),
            "1.540 (Quartz?)"
        );
        assert_eq!(
            row_text(
                Some("Quartz?"),
                Some(1.54),
                Some(designer_statement(1.5, None))
            ),
            "1.50 and up (Quartz? 1.540)"
        );
        assert_eq!(row_text(None, Some(1.54), None), "1.540");
        assert_eq!(
            row_text(None, Some(1.54), Some(estimate_statement(1.5, None))),
            "1.50 and up (Pleochroma estimate; 1.540)"
        );
        assert_eq!(row_text(None, None, None), "-");
    }

    #[test]
    fn the_design_material_names_the_row_and_a_guess_is_marked() {
        assert_eq!(
            material_label(Some("Sapphire"), Some(1.768)),
            Some("Sapphire".to_string())
        );
        assert_eq!(
            material_label(Some("Custom Glass"), None),
            Some("Custom Glass".to_string())
        );
        assert_eq!(material_label(None, None), None);
        if let Some(guess) = material_label(None, Some(1.544)) {
            assert!(guess.ends_with('?'), "{guess}");
        }
    }

    #[test]
    fn setting_the_choice_is_an_undoable_edit_that_marks_the_design_unsaved() {
        let mut st = EditorState::fresh();
        assert!(!st.is_dirty());

        assert!(apply_choice(&mut st, RiRangeChoice::Estimate));
        assert!(st.is_dirty());
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Estimate);
        assert!(st.history.can_undo());

        // The same choice again changes nothing, and adds no history step.
        st.mark_saved();
        assert!(!apply_choice(&mut st, RiRangeChoice::Estimate));
        assert!(!st.is_dirty());

        // Another choice after a save is unsaved again.
        assert!(apply_choice(&mut st, RiRangeChoice::Off));
        assert!(st.is_dirty());
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Off);

        // Ctrl+Z and Ctrl+Y move the choice both ways.
        assert!(st.undo().expect("the undo replays"));
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Estimate);
        assert!(st.undo().expect("the undo replays"));
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Off);
        assert!(st.redo().expect("the redo replays"));
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Estimate);
    }

    #[test]
    fn the_keystrokes_of_one_typing_run_are_one_undo_step() {
        let mut st = EditorState::fresh();
        assert!(apply_typed_choice(&mut st, designer(1.6, None)));
        assert!(apply_typed_choice(&mut st, designer(1.63, None)));
        assert!(apply_typed_choice(&mut st, designer(1.63, Some(2.1))));
        assert_eq!(st.design.ri_range_choice, designer(1.63, Some(2.1)));

        assert!(st.undo().expect("the undo replays"));
        assert_eq!(st.design.ri_range_choice, RiRangeChoice::Off);
        assert!(!st.history.can_undo(), "the run was one step");
    }

    #[test]
    fn a_range_edit_keeps_a_current_solve_and_optimize_result_current() {
        let mut st = EditorState::fresh();
        let before = st.current_generation();
        st.deep_solve_result_generation = Some(before);
        let components = pleochroma_cut_core::ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        };
        let outcome = pleochroma_cut_core::OptimizeOutcome {
            provenance: None,
            before: components,
            before_score: 10.0,
            before_yield_loss_pct: 0.0,
            after: components,
            after_score: 10.0,
            after_yield_loss_pct: 0.0,
            evaluations: 4,
            changes: Vec::new(),
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        };
        *st.pending_optimize.lock().unwrap() = Some((outcome, before));

        assert!(apply_choice(&mut st, RiRangeChoice::Estimate));
        let now = st.current_generation();
        assert_ne!(now, before, "the edit is a step in the history");
        assert_eq!(st.deep_solve_result_generation, Some(now));
        let pending = st.pending_optimize.lock().unwrap();
        assert_eq!(pending.as_ref().map(|(_, started)| *started), Some(now));
        drop(pending);

        // A result that was already stale stays stale.
        st.deep_solve_result_generation = Some(before);
        assert!(apply_choice(&mut st, RiRangeChoice::Off));
        assert_eq!(st.deep_solve_result_generation, Some(before));
    }

    #[test]
    fn the_estimate_is_stored_as_the_choice_only() {
        let mut st = EditorState::fresh();
        assert!(apply_choice(&mut st, RiRangeChoice::Estimate));
        let extras = st.file_extras_for_write();
        let metadata = &extras.metadata;
        assert!(!metadata.unknown.contains_key("ri_min"));
        assert!(!metadata.unknown.contains_key("ri_max"));
        assert_eq!(
            metadata
                .unknown
                .get("ri_range_source")
                .and_then(toml::Value::as_str),
            Some("estimate")
        );
    }

    #[test]
    fn choosing_off_does_not_lose_the_range_the_file_states() {
        let mut opened = EditorState::fresh();
        opened.file_extras = crate::gui::editor::state::DesignFileExtras::new(
            {
                let mut metadata = pleochroma_cut_core::native::DesignMetadata::default();
                metadata.set_ri_range_choice(designer(1.7, Some(2.15)));
                metadata
            },
            Vec::new(),
        );
        // Opening a design takes the stated range into the design.
        let mut st = EditorState::fresh();
        st.replace_wholesale(opened);
        assert_eq!(st.design.ri_range_choice, designer(1.7, Some(2.15)));
        assert_eq!(st.file_extras.file_ri_range, FILE);
        assert!(apply_choice(&mut st, RiRangeChoice::Off));
        assert_eq!(
            st.file_extras.file_ri_range, FILE,
            "From File stays offered"
        );
        assert!(apply_choice(&mut st, designer(1.7, Some(2.15))));
        assert_eq!(
            source_for(
                st.design.ri_range_choice,
                st.file_extras.file_ri_range,
                None
            ),
            SOURCE_FILE
        );
    }
}
