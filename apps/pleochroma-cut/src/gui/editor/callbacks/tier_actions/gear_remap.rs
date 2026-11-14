//! The design settings panel's gear-remap Apply/Confirm/Cancel flow.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
};

use pleochroma_cut_core::{Edit, RemapRounding};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    EditorModel, GearRemapRow, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::{
                EditorState, PendingGearRemap, gear_choice_to_teeth, gear_index_from_teeth,
                gear_remap_preview,
            },
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

thread_local! {
    /// The design generation (`EditorState::generation`) when [`setup_gear_apply_callback`]
    /// built the pending remap's preview. [`setup_gear_remap_confirm_callback`] refuses a Confirm
    /// whose generation has moved on. Kept beside `PendingGearRemap`, not inside it, and cleared
    /// whenever nothing is pending.
    static PENDING_GEAR_REMAP_GENERATION: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The design settings panel's gear combo "Apply" -- computes a real dry-run preview
/// ([`gear_remap_preview`]) and opens the confirmation panel rather than applying
/// anything directly; see [`setup_gear_remap_confirm_callback`]/
/// [`setup_gear_remap_cancel_callback`] for how that panel closes. A no-op (just a
/// toast) when the chosen gear is already the design's current one.
pub(in crate::gui::editor) fn setup_gear_apply_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state_apply = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_gear_apply(
        move |gear_preset_index: i32, gear_custom_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let to_gear = match gear_choice_to_teeth(gear_preset_index, &gear_custom_text) {
                Ok(t) => t,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let mut st = state_apply.borrow_mut();
            let from_gear = st.design.meta.gear_teeth;
            if to_gear == from_gear {
                show_toast(&ui, "Already using this gear.", "info");
                return;
            }
            let rounding = RemapRounding::Nearest;
            let rows: Vec<GearRemapRow> =
                gear_remap_preview(&st.design, from_gear, to_gear, rounding);
            st.pending_gear_remap = Some(PendingGearRemap {
                from_gear,
                to_gear,
                symmetry_order: st.design.meta.symmetry_order,
                mirror: st.design.meta.mirror,
                rounding,
            });
            // Records the generation the preview was built against; see
            // `PENDING_GEAR_REMAP_GENERATION`.
            PENDING_GEAR_REMAP_GENERATION
                .with(|cell| cell.set(Some(st.generation.load(Ordering::Relaxed))));
            drop(st);
            ui.global::<EditorModel>()
                .set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
            ui.global::<EditorModel>().set_gear_remap_open(true);
        },
    );

    // `EditorModel.gear_remap_set_rounding` (`ui/models/editor.slint`) is registered beside
    // `on_gear_apply`, which shares its preview code. It stores the chosen rounding in
    // `PendingGearRemap` and recomputes the same dry-run preview, so the preview rows show
    // what Confirm will actually do.
    let state_rounding = Rc::clone(state);
    let ui_weak_rounding = ui.as_weak();
    ui.global::<EditorModel>()
        .on_gear_remap_set_rounding(move |rounding_index: i32| {
            let Some(ui) = ui_weak_rounding.upgrade() else {
                return;
            };
            let rounding = match rounding_index {
                1 => RemapRounding::Floor,
                2 => RemapRounding::Ceil,
                _ => RemapRounding::Nearest,
            };
            let mut st = state_rounding.borrow_mut();
            let Some(pending) = st.pending_gear_remap.as_mut() else {
                // Defensive only: the rounding selector only shows while a remap
                // is pending, same as Confirm's own no-op guard.
                return;
            };
            pending.rounding = rounding;
            let from_gear = pending.from_gear;
            let to_gear = pending.to_gear;
            let rows: Vec<GearRemapRow> =
                gear_remap_preview(&st.design, from_gear, to_gear, rounding);
            drop(st);
            ui.global::<EditorModel>()
                .set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
        });
}

/// The gear-remap confirmation panel's "Apply": commits [`EditorState::pending_gear_remap`] as
/// one undoable `History` step, an [`Edit::Batch`] of [`Edit::RemapIndices`] then
/// [`Edit::SetSchedule`]. Two separate steps would let a single Undo leave the indices remapped
/// for the new gear while the schedule still named the old one. With nothing pending it only
/// closes the panel, which is defensive: the button shows only while a remap is pending.
pub(in crate::gui::editor) fn setup_gear_remap_confirm_callback(
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
    ui.global::<EditorModel>().on_gear_remap_confirm(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let Some(pending) = st.pending_gear_remap.take() else {
            ui.global::<EditorModel>().set_gear_remap_open(false);
            return;
        };
        // Refuses a Confirm whose preview no longer describes the live design: an edit, an
        // Undo or another Apply landed while the panel was open, so the pending gears may no
        // longer match what the batch will do.
        let started_generation = PENDING_GEAR_REMAP_GENERATION.with(Cell::take);
        if started_generation.is_some_and(|g| g != st.generation.load(Ordering::Relaxed)) {
            ui.global::<EditorModel>().set_gear_remap_open(false);
            show_toast(
                &ui,
                "The design changed while the gear change was open, so this preview no \
                 longer matches it. Pick the gear again to remap against the current \
                 design.",
                "warning",
            );
            return;
        }
        let batch_result = st.apply(Edit::Batch(vec![
            Edit::RemapIndices {
                from_gear: pending.from_gear,
                to_gear: pending.to_gear,
                rounding: pending.rounding,
            },
            Edit::SetSchedule {
                gear_teeth: pending.to_gear,
                symmetry_order: pending.symmetry_order,
                mirror: pending.mirror,
            },
        ]));
        ui.global::<EditorModel>().set_gear_remap_open(false);
        match batch_result {
            Ok(()) => {
                // A gear remap rewrites every tier's index-wheel position -- force a
                // full solve rather than guessing a `dirty` set, and trust no cached
                // mast either.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Err(e) => show_toast(&ui, &e.to_string(), "error"),
        }
    });
}

/// The gear-remap confirmation panel's "Cancel" -- discards the pending remap
/// without touching `Design`; the gear combo's display is restored to the design's
/// real current gear on the next refresh.
pub(in crate::gui::editor) fn setup_gear_remap_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_gear_remap_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let gear_teeth = {
            let mut st = state.borrow_mut();
            st.pending_gear_remap = None;
            st.design.meta.gear_teeth
        };
        // Clears the recorded generation together with the pending remap.
        PENDING_GEAR_REMAP_GENERATION.with(|cell| cell.set(None));
        let editor = ui.global::<EditorModel>();
        editor.set_gear_remap_open(false);
        // The Design sheet's gear segments follow `gear_index` at once, so a cancelled pick
        // must not leave the segment on the gear that was not applied.
        editor.set_gear_index(gear_index_from_teeth(gear_teeth));
        editor.set_gear_custom_text(gear_teeth.to_string().into());
    });
}
