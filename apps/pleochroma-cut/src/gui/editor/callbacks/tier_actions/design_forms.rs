//! Design-wide form applies: preform, yield inputs, preform Y-offset, cheater
//! offset, tier note, and design meta.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma_cut_core::Edit;
use pleochroma_editor::{
    loading::{NumberExprError, eval_number},
    slider_ranges::{format_slider_number, preform_spec_data},
};
use slint::{ComponentHandle, SharedString};

use super::{FIXED_CYLINDER_PREFORM_SIDES, tier_form::slider_spec_from};
use crate::{
    EditorModel, MainWindow, SliderModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve, guide, loading,
            stall_guard::stall_guard,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// A design-wide number field (the preform Y-offset, the gear reference angle): a plain number
/// or arithmetic over numbers (`1.5 - 0.25`, see `pleochroma_editor::loading::eval_number`).
/// `Err` is the toast text: "<label> '<text>' is not a number." for text that is no number,
/// "... cannot be calculated: <reason>." for a broken calculation, and "<label> must be a
/// finite number." for `inf` or `NaN`.
fn finite_number_field(label: &str, text: &str) -> Result<f64, String> {
    let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{label} must be a finite number."))
    }
}

/// The cheater-offset field: blank clears the offset (`Ok(None)`), otherwise a number or
/// arithmetic (`12 + 0.5`). `Err` is the toast text; a value that is not finite reads "...
/// is not a number." like any other text that is no number.
fn cheater_offset_from_text(text: &str) -> Result<Option<f64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match eval_number(trimmed, None) {
        Ok(value) if value.is_finite() => Ok(Some(value)),
        Ok(_) => Err(NumberExprError::NotANumber.message("Cheater offset", trimmed)),
        Err(error) => Err(error.message("Cheater offset", trimmed)),
    }
}

/// Answers the Preform tab's sliders (`SliderModel`'s two Preform callbacks): `preform_spec`
/// says whether a field's slider works and gives its range and value, `preform_text` is the
/// number a drag writes into the field. Both are pure functions of their arguments (the
/// Y-offset's range is half the Depth in millimetres, so the Depth and Girdle Diameter texts
/// come along); the work is done in `pleochroma_editor::slider_ranges`. A slider only fills
/// its field in: "Apply Preform" applies it.
fn setup_preform_slider_callbacks(ui: &MainWindow) {
    let model = ui.global::<SliderModel>();
    model.on_preform_spec(|field, text, depth_text, girdle_diameter_text| {
        slider_spec_from(preform_spec_data(
            &field,
            &text,
            &depth_text,
            &girdle_diameter_text,
        ))
    });
    model.on_preform_text(|value| format_slider_number(f64::from(value)).into());
}

/// "Apply Preform": parses the form (see `loading::parse_preform_form`) and, on
/// success, applies it through `EditorState::apply` as a [`Edit::SetPreform`] -- the
/// only `Edit` variant this callback ever constructs. The Preform tab's slider callbacks
/// are wired from here too ([`setup_preform_slider_callbacks`]).
pub(in crate::gui::editor) fn setup_apply_preform_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    setup_preform_slider_callbacks(ui);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_preform(
        move |shape_index: i32,
              half_width: SharedString,
              length_over_width: SharedString,
              depth: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match loading::parse_preform_form(
                shape_index,
                &half_width,
                &length_over_width,
                &depth,
                FIXED_CYLINDER_PREFORM_SIDES,
            ) {
                // `SetPreform` never fails (it names no tier index), so the only
                // `Err` path here is this function's own parse failure, already reported.
                Ok(preform) => {
                    let _ = st.apply(Edit::SetPreform { preform });
                    // The preform reshapes the bounding planes but never moves a
                    // tier's own mast -- no tier is dirty.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Apply Yield Inputs": parses the form and applies both halves through `EditorState::apply`
/// as ONE [`Edit::Batch`] of [`Edit::SetGirdleDiameterMm`] then [`Edit::SetMaterial`]. Two
/// separate edits would need two undo steps for one click, and could be undone out of order.
pub(in crate::gui::editor) fn setup_apply_yield_inputs_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_yield_inputs(
        move |girdle_diameter_mm: SharedString,
              material_index: i32,
              specific_gravity_override: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match crate::gui::editor::state::parse_yield_form(
                &girdle_diameter_mm,
                material_index,
                &specific_gravity_override,
                &st.design.material,
            ) {
                Ok((girdle_diameter_mm, material)) => {
                    // Neither `SetGirdleDiameterMm` nor `SetMaterial` names a tier
                    // index (both design-wide), so -- like `Edit::SetPreform`/
                    // `Edit::SetMeta` elsewhere in this module -- `EditorState::apply`
                    // cannot fail on this `Batch` in practice; kept `let _ =`.
                    let _ = st.apply(Edit::Batch(vec![
                        Edit::SetGirdleDiameterMm { girdle_diameter_mm },
                        Edit::SetMaterial {
                            material: Box::new(material),
                        },
                    ]));
                    // Girdle diameter and material alone never move a tier's own
                    // mast -- no tier is dirty.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    // The guide's optional yield step completes here.
                    guide::check_progress(&ui, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Apply Y-Offset". `Edit::SetPreformYOffset` is a mast-preserving edit. The field is typed in
/// millimetres, the cutter's own measurement, not model units (see
/// `EditorModel.preform_y_offset_mm` in `ui/models/editor.slint`). It is converted through the
/// design's mm-per-unit factor (`Design::yield_report(&solved).mm_per_unit`, as
/// `state::preform_mm_texts` reads it). It toasts instead of applying when the field does not
/// parse, or when no girdle diameter or solve has set that factor yet.
pub(in crate::gui::editor) fn setup_apply_preform_y_offset_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_preform_y_offset(move |mm_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            stall_guard("on_apply_preform_y_offset", || {
                // A number or arithmetic (`1.5 - 0.25`); see `finite_number_field`.
                let mm = match finite_number_field("Y-offset", &mm_text) {
                    Ok(mm) => mm,
                    Err(message) => {
                        show_toast(&ui, &message, "error");
                        return;
                    }
                };
                let mut st = state.borrow_mut();
                // The UI thread never solves here. This reuses the cached masts of the last
                // solve instead of a fresh `Design::solve()` while the state is borrowed. If no
                // cached solve matches the current tier count, the cutter is asked to solve first.
                let Some(solved) = auto_solve::solid_last_solved()
                    .and_then(|cache| {
                        cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    // The cache is generation-tagged; only the masts matter here.
                    .filter(|(_, solved)| solved.len() == st.design.tiers.len())
                    .map(|(_, solved)| solved)
                else {
                    show_toast(&ui, "Solve first, then set a Y-offset.", "error");
                    return;
                };
                let Some(mm_per_unit) = st.design.yield_report(&solved).mm_per_unit else {
                    show_toast(
                        &ui,
                        "Set a girdle diameter in the Yield tab before setting a Y-offset in \
                     millimetres.",
                        "error",
                    );
                    return;
                };
                let y_offset = mm / mm_per_unit;
                // Mast-preserving, so no tier is dirty, as in `setup_apply_preform_callback`.
                // The edit names no tier index, so `apply` can fail here only on an internal
                // invariant violation. The error is still shown rather than assumed impossible.
                if let Err(e) = st.apply(Edit::SetPreformYOffset { y_offset }) {
                    show_toast(&ui, &e.to_string(), "error");
                    return;
                }
                refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    false,
                );
            });
        });
}

/// "Apply Cheater Offset". Sets, or for a blank field clears, one tier's cheater/azimuth offset
/// via [`Edit::SetCheaterOffset`]. It is a cutting-sheet annotation with no geometric effect, so
/// unlike the other tier edits here it never calls [`submit_preview_replan`].
pub(in crate::gui::editor) fn setup_apply_cheater_offset_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_cheater_offset(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            // Blank clears the offset; otherwise a number or arithmetic (`12 + 0.5`).
            let offset_deg = match cheater_offset_from_text(&text) {
                Ok(offset_deg) => offset_deg,
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            // `index` names a real row when the field was focused, but the tier list
            // can change while a cutter is still typing in this field -- surfaced
            // rather than silently dropping a stale-index edit.
            if let Err(e) = st.apply(Edit::SetCheaterOffset { index, offset_deg }) {
                show_toast(&ui, &e.to_string(), "error");
                return;
            }
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
        });
}

/// "Apply Tier Note". Sets, or for a blank field clears, one tier's free-text note via
/// [`Edit::SetTierNote`]. It is a cutting-sheet annotation with no geometric effect, so like
/// [`setup_apply_cheater_offset_callback`] it never calls [`submit_preview_replan`]. Any text is
/// a valid note, so there is no error branch: blank just clears it.
pub(in crate::gui::editor) fn setup_apply_tier_note_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_tier_note(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let trimmed = text.trim();
            let note = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            let mut st = state.borrow_mut();
            // Same stale-index reasoning as `setup_apply_cheater_offset_callback` just above.
            if let Err(e) = st.apply(Edit::SetTierNote { index, note }) {
                show_toast(&ui, &e.to_string(), "error");
                return;
            }
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
        });
}

/// Replaces the design's title (the first `H` header), any further header lines, the footnotes
/// and the index wheel's zero-tooth reference angle as ONE undoable [`Edit::SetMeta`], as in
/// [`setup_apply_yield_inputs_callback`]. `title`, `extra_headers` and `footnotes` are each split
/// on `';'` into lines. The extra headers follow the title as further `H` lines (see
/// `EditorModel.design_title` in `ui/models/editor.slint`). An unparseable `gear_ref` toasts
/// instead of applying anything.
pub(in crate::gui::editor) fn setup_apply_design_meta_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_design_meta(
        move |title: SharedString,
              extra_headers: SharedString,
              footnotes: SharedString,
              gear_ref: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let split_lines = |text: &str| -> Vec<String> {
                text.split(';')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            };
            let trimmed_title = title.trim();
            let mut headers = Vec::new();
            if !trimmed_title.is_empty() {
                headers.push(trimmed_title.to_string());
            }
            headers.extend(split_lines(&extra_headers));
            let footnotes = split_lines(&footnotes);
            let gear_reference_angle = match finite_number_field("Gear reference angle", &gear_ref)
            {
                Ok(angle) => angle,
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            // Mast-preserving, so no tier is dirty. The gear reference angle can rotate the
            // rendered index wheel, so the solid preview still needs a replan. `SetMeta` names no
            // tier index, so `apply` cannot fail on it in practice, and the result is ignored.
            let _ = st.apply(Edit::SetMeta {
                headers,
                footnotes,
                gear_reference_angle,
            });
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
            submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_number_reads_as_before_and_arithmetic_is_worked_out() {
        assert_eq!(finite_number_field("Y-offset", " 1.5 "), Ok(1.5));
        assert_eq!(finite_number_field("Y-offset", "-0.25"), Ok(-0.25));
        let sum = finite_number_field("Gear reference angle", "1.5 - 0.25").unwrap();
        assert_eq!(sum.to_bits(), 1.25_f64.to_bits());
        assert_eq!(cheater_offset_from_text("12 + 0.5"), Ok(Some(12.5)));
        assert_eq!(cheater_offset_from_text("3"), Ok(Some(3.0)));
    }

    #[test]
    fn a_blank_cheater_offset_clears_it() {
        assert_eq!(cheater_offset_from_text(""), Ok(None));
        assert_eq!(cheater_offset_from_text("   "), Ok(None));
    }

    #[test]
    fn text_that_is_no_number_keeps_the_old_wording() {
        assert_eq!(
            finite_number_field("Y-offset", " abc "),
            Err("Y-offset 'abc' is not a number.".to_owned())
        );
        assert_eq!(
            cheater_offset_from_text("abc"),
            Err("Cheater offset 'abc' is not a number.".to_owned())
        );
        assert_eq!(
            finite_number_field("Gear reference angle", ""),
            Err("Gear reference angle '' is not a number.".to_owned())
        );
    }

    #[test]
    fn a_value_that_is_not_finite_is_refused() {
        assert_eq!(
            finite_number_field("Y-offset", "inf"),
            Err("Y-offset must be a finite number.".to_owned())
        );
        assert_eq!(
            finite_number_field("Gear reference angle", "NaN"),
            Err("Gear reference angle must be a finite number.".to_owned())
        );
        assert_eq!(
            cheater_offset_from_text("inf"),
            Err("Cheater offset 'inf' is not a number.".to_owned())
        );
    }

    #[test]
    fn a_broken_calculation_says_why_and_names_the_field() {
        let message = finite_number_field("Y-offset", "1 +").unwrap_err();
        assert!(
            message.starts_with("Y-offset '1 +' cannot be calculated:"),
            "{message}"
        );
        let message = cheater_offset_from_text("(2 + 3").unwrap_err();
        assert!(
            message.starts_with("Cheater offset '(2 + 3' cannot be calculated:"),
            "{message}"
        );
        // A relation is for a tier's angle only.
        assert!(finite_number_field("Y-offset", "=P1").is_err());
    }
}
