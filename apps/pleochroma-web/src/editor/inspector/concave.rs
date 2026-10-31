//! The Tier tab's concave form: seeding it from the selected concave tier, the
//! dirty-draft and stale guards, and Add / Save Concave Tier.
//!
//! Modelled on the desktop's `concave_form_tab.slint` and
//! `callbacks/tier_actions/concave_tier.rs`, with the mobile app's draft guard
//! (`pleochroma_mobile_core::concave::ConcaveDraft::check_current`). The twelve text /
//! combo / check fields, the snapshot of what they held when last seeded and the pending
//! choice live in `InspectorModel` (`c-*`, `loaded-c-*`, `concave-*`); this module decides
//! WHEN to seed, exactly as [`super::form`] does for a flat tier:
//!
//! - the subject is `EditorSession::selected_concave` (a concave row selected in the
//!   table); selecting one turns `concave-mode` on and loads the tier into a clean form,
//!   but with a dirty form only puts up the Keep Draft / Discard Draft choice;
//! - selecting a flat tier turns the mode off (again behind that choice for a dirty draft),
//!   and so does deselecting a saved concave tier; a "New Concave Tier" draft stays while
//!   nothing is selected;
//! - after an undo, redo or any edit made elsewhere, a clean form re-seeds from the design
//!   and a dirty one is left alone: its tier changing under it raises the "changed in the
//!   design" notice, and the tier count changing makes it forget which tier it was for, so
//!   a save cannot land on the wrong row;
//! - a replaced design (New, Open) leaves the mode.
//!
//! The draft's pure parts -- the tool mapping, the Keep / Discard labels, the stale guard and the
//! save itself -- are `pleochroma_web_core::concave_form`, tested natively.
//!
//! A save is `parse_concave_tier_form` -> `concave_name_clash_message` ->
//! `concave_tier_save_edit` -> `EditorSession::apply` -> [`finish_no_tier`]: no flat mast
//! moves, but the design is re-solved like after every web edit (decision D12: the
//! render's tools return with the solve).

use super::{PushCtx, finish_no_tier, form, poll::with_current};
use crate::{
    AppWindow, InspectorModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        state::WebApp,
    },
    views,
};
use pleochroma_editor::{
    loading::{ConcaveTierFormFields, concave_tier_form_fields},
    view_model::TierRowKind,
};
use pleochroma_web_core::concave_form::{
    Pending, Seed, apply_save, blank_fields, pending_label, tool_code, tool_index,
};
use slint::{ComponentHandle, SharedString};
use std::cell::RefCell;

/// What the concave selection was at a push.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Sel {
    #[default]
    None,
    Flat(usize),
    Concave(usize),
}

/// The module's memory between pushes (single-threaded, like the rest of the page).
#[derive(Default)]
struct State {
    seed: Option<Seed>,
    pending: Option<Pending>,
    /// The selection of the last push.
    selection: Sel,
    /// The concave tier count of the last push.
    concave_count: usize,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Writes `fields` into the form AND its clean snapshot.
fn show_fields(model: &InspectorModel<'_>, fields: &ConcaveTierFormFields) {
    model.set_c_name(fields.name.as_str().into());
    model.set_c_angle(fields.angle_deg.as_str().into());
    model.set_c_indices(fields.indices.as_str().into());
    model.set_c_instructions(fields.instructions.as_str().into());
    model.set_c_tool(tool_index(&fields.tool));
    model.set_c_theta(fields.tool_azimuth_deg.as_str().into());
    model.set_c_x(fields.x.as_str().into());
    model.set_c_y(fields.y.as_str().into());
    model.set_c_z(fields.z.as_str().into());
    model.set_c_diameter(fields.diameter_ratio.as_str().into());
    model.set_c_tool_angle(fields.tool_angle_deg.as_str().into());
    model.set_c_reciprocating(fields.reciprocating);
    snapshot_fields(model);
}

/// Records the fields' current values as the clean baseline `concave-dirty` compares with.
fn snapshot_fields(model: &InspectorModel<'_>) {
    model.set_loaded_c_name(model.get_c_name());
    model.set_loaded_c_angle(model.get_c_angle());
    model.set_loaded_c_indices(model.get_c_indices());
    model.set_loaded_c_instructions(model.get_c_instructions());
    model.set_loaded_c_tool(model.get_c_tool());
    model.set_loaded_c_theta(model.get_c_theta());
    model.set_loaded_c_x(model.get_c_x());
    model.set_loaded_c_y(model.get_c_y());
    model.set_loaded_c_z(model.get_c_z());
    model.set_loaded_c_diameter(model.get_c_diameter());
    model.set_loaded_c_tool_angle(model.get_c_tool_angle());
    model.set_loaded_c_reciprocating(model.get_c_reciprocating());
}

/// The form's fields as the user left them.
fn fields_from_model(model: &InspectorModel<'_>) -> ConcaveTierFormFields {
    ConcaveTierFormFields {
        name: model.get_c_name().to_string(),
        angle_deg: model.get_c_angle().to_string(),
        indices: model.get_c_indices().to_string(),
        instructions: model.get_c_instructions().to_string(),
        tool: tool_code(model.get_c_tool()).to_owned(),
        tool_azimuth_deg: model.get_c_theta().to_string(),
        x: model.get_c_x().to_string(),
        y: model.get_c_y().to_string(),
        z: model.get_c_z().to_string(),
        diameter_ratio: model.get_c_diameter().to_string(),
        tool_angle_deg: model.get_c_tool_angle().to_string(),
        reciprocating: model.get_c_reciprocating(),
    }
}

/// Clears the error line, the notices and the pending choice.
fn reset_notes(model: &InspectorModel<'_>) {
    model.set_concave_error(SharedString::new());
    model.set_concave_error_field(SharedString::new());
    model.set_concave_show_dirty_warning(false);
    model.set_concave_pending_label(SharedString::new());
    model.set_concave_external_change(false);
}

/// Shows `fields` as the form of `seed` and turns the mode on.
fn open_form(model: &InspectorModel<'_>, fields: &ConcaveTierFormFields, seed: Seed) {
    reset_notes(model);
    show_fields(model, fields);
    model.set_loaded_concave_index(
        seed.index
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
    );
    model.set_concave_mode(true);
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.seed = Some(seed);
        state.pending = None;
    });
}

/// Seeds the form from concave tier `index` (the full-precision strings, so a Save with
/// nothing retyped round-trips bit for bit).
fn apply_load(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, index: usize) {
    let Some(tier) = pcx.design.concave_tiers.get(index) else {
        return;
    };
    let fields = concave_tier_form_fields(tier);
    let seed = Seed {
        index: Some(index),
        generation: pcx.design_state.session.current_generation(),
        baseline: fields.clone(),
        acknowledged: false,
    };
    open_form(model, &fields, seed);
}

/// Opens a blank "Add Concave Tier" draft.
fn open_blank(model: &InspectorModel<'_>, generation: u64) {
    let fields = blank_fields();
    let seed = Seed {
        index: None,
        generation,
        baseline: fields.clone(),
        acknowledged: false,
    };
    open_form(model, &fields, seed);
}

/// Turns the mode off and blanks the form and its notes.
fn leave_mode(model: &InspectorModel<'_>) {
    reset_notes(model);
    show_fields(model, &blank_fields());
    model.set_loaded_concave_index(-1);
    model.set_concave_mode(false);
    model.set_concave_sheet_visible(false);
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.seed = None;
        state.pending = None;
    });
}

/// Everything blank: no design is loaded.
pub(super) fn clear_all(ui: &AppWindow) {
    leave_mode(&ui.global::<InspectorModel>());
    STATE.with(|state| *state.borrow_mut() = State::default());
}

/// Puts up the Keep Draft / Discard Draft choice for a selection that moved to `pending`.
fn warn(model: &InspectorModel<'_>, pending: Pending) {
    STATE.with(|state| state.borrow_mut().pending = Some(pending));
    model.set_concave_pending_label(pending_label(pending).into());
    model.set_concave_show_dirty_warning(true);
}

/// The selection of this push: a concave tier only when no flat tier is selected.
fn current_selection(pcx: &PushCtx<'_>) -> Sel {
    pcx.selected()
        .map(Sel::Flat)
        .or_else(|| pcx.selected_concave().map(Sel::Concave))
        .unwrap_or(Sel::None)
}

/// The guarded entry point for a selection change.
fn on_selection_changed(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, selection: Sel) {
    let in_mode = model.get_concave_mode();
    let dirty = in_mode && model.get_concave_dirty();
    let loaded = model.get_loaded_concave_index();
    match selection {
        Sel::Concave(index) => {
            if !dirty {
                apply_load(pcx, model, index);
            } else if i32::try_from(index) == Ok(loaded) {
                // The tier the draft is for was selected again: keep the draft.
                model.set_concave_show_dirty_warning(false);
            } else {
                warn(model, Pending::Concave(index));
            }
        }
        Sel::Flat(index) => {
            if dirty {
                warn(model, Pending::Flat(index));
            } else if in_mode {
                leave_mode(model);
            }
        }
        Sel::None => {
            if dirty {
                warn(model, Pending::Blank);
            } else if in_mode && loaded >= 0 {
                // A saved tier was deselected: back to the flat form. A blank "Add" draft
                // stays; nothing selected is its natural state.
                leave_mode(model);
            }
        }
    }
}

/// A dirty draft after the design changed (an edit, undo / redo, or a remove / duplicate /
/// move that renumbered the concave list), whether or not the selection moved with it.
///
/// The tier count changing, or the draft's index running out of range, makes the draft
/// forget which tier it was for; the tier at the index reading differently from when the
/// draft started raises the "changed in the design" notice (a save is refused until the
/// user chooses). Either way a save can never land on a different tier silently.
fn guard_dirty_draft(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, previous_count: usize) {
    let count = pcx.design.concave_tiers.len();
    let loaded = usize::try_from(model.get_loaded_concave_index()).ok();
    let in_range = loaded.filter(|&index| index < count);
    // The draft's tier is gone: an index was loaded and it is past the end of the list.
    let loaded_row_vanished = loaded.is_some() && in_range.is_none();
    if count != previous_count || loaded_row_vanished {
        // The draft may now describe a different row: forget which tier it was for
        // rather than risk a save landing on the wrong one.
        forget_index(model);
        return;
    }
    let generation = pcx.design_state.session.current_generation();
    let stale = STATE.with(|state| {
        state
            .borrow()
            .seed
            .as_ref()
            .is_some_and(|seed| !seed.acknowledged && seed.is_stale(pcx.design, generation))
    });
    if stale {
        model.set_concave_external_change(true);
    }
}

/// After an edit made elsewhere or an undo / redo that left the selection alone: a clean
/// form re-seeds (or closes when its tier is gone); a dirty one is [`guard_dirty_draft`]'s.
fn reseed_after_external_change(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let count = pcx.design.concave_tiers.len();
    let loaded = usize::try_from(model.get_loaded_concave_index()).ok();
    let in_range = loaded.filter(|&index| index < count);
    if model.get_concave_dirty() {
        return;
    }
    match in_range {
        Some(index) => apply_load(pcx, model, index),
        None if loaded.is_some() => leave_mode(model),
        None => {}
    }
}

/// The dirty draft becomes an "Add" draft: it no longer names a tier.
fn forget_index(model: &InspectorModel<'_>) {
    model.set_loaded_concave_index(-1);
    model.set_concave_external_change(false);
    STATE.with(|state| {
        if let Some(seed) = state.borrow_mut().seed.as_mut() {
            seed.index = None;
        }
    });
}

/// The read-only sheet line (and warning) of the loaded saved tier.
fn push_sheet(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let loaded = usize::try_from(model.get_loaded_concave_index()).ok();
    let row = loaded.and_then(|index| {
        pcx.rows().iter().find(|row| {
            row.kind == TierRowKind::Concave && usize::try_from(row.index).ok() == Some(index)
        })
    });
    model.set_concave_sheet_visible(row.is_some());
    model.set_concave_sheet_line(
        row.map_or_else(SharedString::new, |r| r.tool_line.as_str().into()),
    );
    model.set_concave_orbit_status(
        row.map_or_else(SharedString::new, |r| r.orbit_status.as_str().into()),
    );
    model.set_concave_orbit_incomplete(row.is_some_and(|r| r.orbit_incomplete));
    model.set_concave_warning(
        row.map_or_else(SharedString::new, |r| r.warning_text.as_str().into()),
    );
}

/// One refresh of the concave form.
pub(super) fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<InspectorModel>();
    if pcx.replaced {
        leave_mode(&model);
        STATE.with(|state| *state.borrow_mut() = State::default());
    }
    let selection = current_selection(pcx);
    let count = pcx.design.concave_tiers.len();
    let generation = pcx.design_state.session.current_generation();
    let (previous, previous_count) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let before = (state.selection, state.concave_count);
        state.selection = selection;
        state.concave_count = count;
        before
    });
    let moved = pcx.previous_generation.is_some_and(|g| g != generation);
    if selection != previous {
        on_selection_changed(pcx, &model, selection);
    } else if moved && model.get_concave_mode() {
        reseed_after_external_change(pcx, &model);
    }
    // A remove / duplicate / move moves the selection AND renumbers the list: a draft that
    // survived the selection change must not keep pointing at the old numbering.
    if moved && model.get_concave_mode() && model.get_concave_dirty() {
        guard_dirty_draft(pcx, &model, previous_count);
    }
    if model.get_concave_mode() {
        push_sheet(pcx, &model);
    } else {
        model.set_concave_sheet_visible(false);
    }
}

/// Makes concave tier `index` the one selection: no flat tier, no multi-selection.
fn select_concave(app: &mut WebApp, index: usize) {
    app.selected_tier = None;
    if let Some(design) = app.design.as_mut() {
        design.session.multi_selected.clear();
        design.session.selected_concave = Some(index);
    }
}

/// Add / Save Concave Tier (and Enter in a form field).
fn save_concave(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let fields = fields_from_model(&model);
    let index = usize::try_from(model.get_loaded_concave_index()).ok();
    let seed = STATE.with(|state| state.borrow().seed.clone());
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let outcome = apply_save(&mut design_state.session, &fields, index, seed.as_ref());
        if let Ok(saved) = &outcome {
            select_concave(&mut app, saved.index);
        }
        outcome
    };
    match outcome {
        Ok(saved) => {
            model.set_concave_error(SharedString::new());
            model.set_concave_error_field(SharedString::new());
            // Optimistic: the values were just committed, so the form reads clean against
            // them; the refresh below re-seeds it from the design.
            snapshot_fields(&model);
            finish_no_tier(ctx);
            if saved.added {
                let label = if saved.name.is_empty() {
                    "concave tier".to_owned()
                } else {
                    format!("concave tier {}", saved.name)
                };
                show_message(ctx, MessageKind::Info, &format!("Added {label}"));
            }
        }
        Err(error) => {
            model.set_concave_error(error.message.as_str().into());
            model.set_concave_error_field(error.field.into());
            show_message(ctx, MessageKind::Error, &error.message);
        }
    }
}

/// New Concave Tier: a blank draft with the selection cleared, unless a saved tier's
/// draft would be lost, and the cursor in the Name field.
fn new_concave(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    model.set_tab(0);
    model.set_collapsed(false);
    if model.get_concave_mode()
        && model.get_concave_dirty()
        && model.get_loaded_concave_index() != -1
    {
        warn(&model, Pending::Blank);
        return;
    }
    let generation = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        design_state.session.selected_concave = None;
        design_state.session.multi_selected.clear();
        let generation = design_state.session.current_generation();
        app.selected_tier = None;
        generation
    };
    open_blank(&model, generation);
    // The selection the push will see is the one just made: no "selection changed".
    STATE.with(|state| state.borrow_mut().selection = Sel::None);
    views::request_refresh(ctx);
    form::request_focus(&model, "concave-name");
    super::refresh(ctx);
}

/// "New Flat Tier Instead": leaves the concave form for the flat one.
///
/// `true` when the form was left; `false` when an unsaved draft of a saved tier raised the
/// Keep / Discard choice instead.
pub(super) fn leave_for_new_flat(ctx: &Ctx, model: &InspectorModel<'_>) -> bool {
    if model.get_concave_dirty() && model.get_loaded_concave_index() != -1 {
        warn(model, Pending::NewFlat);
        return false;
    }
    {
        let mut app = ctx.state.borrow_mut();
        if let Some(design_state) = app.design.as_mut() {
            design_state.session.selected_concave = None;
        }
    }
    leave_mode(model);
    STATE.with(|state| state.borrow_mut().selection = Sel::None);
    true
}

/// Opens the concave form: on a blank draft for `None` (the "+ Add Concave Tier" path),
/// on concave tier `index` for `Some` (a concave row was selected, duplicated or moved).
///
/// Idempotent, and it never throws a draft away: for `Some` it makes the tier the one
/// selection and refreshes the panels, whose selection rules load a clean form or put up
/// the Keep / Discard choice for a dirty one. The tab is left alone. An `index` that is not
/// a concave tier (any more) does nothing. Call with no `RefCell` borrow held.
pub fn open(ctx: &Ctx, index: Option<usize>) {
    let Some(index) = index else {
        new_concave(ctx);
        return;
    };
    {
        let mut app = ctx.state.borrow_mut();
        let in_range = app
            .design
            .as_ref()
            .is_some_and(|d| index < d.session.design.concave_tiers.len());
        if !in_range {
            return;
        }
        select_concave(&mut app, index);
    }
    views::request_refresh(ctx);
    super::refresh(ctx);
}

/// Keep Draft: the form keeps its draft and the choice goes away.
fn keep_draft(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    model.set_concave_show_dirty_warning(false);
    model.set_concave_pending_label(SharedString::new());
    STATE.with(|state| state.borrow_mut().pending = None);
}

/// Discard Draft: goes where the selection moved to.
fn discard_draft(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let pending = STATE
        .with(|state| state.borrow_mut().pending.take())
        .unwrap_or(Pending::Blank);
    model.set_concave_show_dirty_warning(false);
    model.set_concave_pending_label(SharedString::new());
    match pending {
        Pending::Concave(index) => {
            with_current(ctx, |pcx| apply_load(pcx, &model, index));
        }
        // The flat form loaded the selected tier on its own push.
        Pending::Flat(_) => leave_mode(&model),
        Pending::Blank => {
            with_current(ctx, |pcx| {
                open_blank(&model, pcx.design_state.session.current_generation());
            });
        }
        Pending::NewFlat => {
            leave_mode(&model);
            form::new_tier(ctx);
        }
    }
}

/// Reload From Design: drops the draft and shows the tier as the design has it now.
fn reload(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let loaded = usize::try_from(model.get_loaded_concave_index()).ok();
    with_current(ctx, |pcx| match loaded {
        Some(index) if index < pcx.design.concave_tiers.len() => apply_load(pcx, &model, index),
        _ => leave_mode(&model),
    });
}

/// Keep Draft on the "changed in the design" notice: a save may now overwrite the change.
fn dismiss_external(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    ui.global::<InspectorModel>()
        .set_concave_external_change(false);
    STATE.with(|state| {
        if let Some(seed) = state.borrow_mut().seed.as_mut() {
            seed.acknowledged = true;
        }
    });
}

/// Registers the concave form's callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<InspectorModel>();
    let c = ctx.clone();
    model.on_save_concave(move || save_concave(&c));
    let c = ctx.clone();
    model.on_new_concave(move || new_concave(&c));
    let c = ctx.clone();
    model.on_concave_keep_draft(move || keep_draft(&c));
    let c = ctx.clone();
    model.on_concave_discard_draft(move || discard_draft(&c));
    let c = ctx.clone();
    model.on_concave_reload(move || reload(&c));
    let c = ctx.clone();
    model.on_concave_dismiss_external(move || dismiss_external(&c));
}
