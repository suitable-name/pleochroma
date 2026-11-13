//! The tier list's "Adopt" action.

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            stall_guard::stall_guard,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The tier list's "Adopt" action: switches a pinned tier to the meet instruction the source
/// file stated for it (`ConstraintTier::imported_meet`), as `Edit::SetConstraint` like any other
/// edit. It is a no-op if the tier has nothing to adopt or the index is stale. `EditorView`
/// shows the button only when `imported_meet_text` is set, so this only guards a race.
///
/// It calls `refresh_editor_panel_stale` (with `dirty = {index}`) rather than a full `refresh_all`
/// solve. The adopted value is the one the tier already showed after the last solve, so a
/// whole-schedule solve would only re-confirm it. The panel updates from the cached solve, and
/// the background dirty-set replan confirms the tier through `resolve_dirty`, as the per-row
/// "Pin" action does (`callbacks::tier_actions::setup_pin_to_mast_callback`).
pub(in crate::gui::editor) fn setup_adopt_meet_callback(
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
    ui.global::<EditorModel>().on_adopt_meet(move |index: i32| {
        stall_guard("on_adopt_meet", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            // The lookup of the imported meet and the one `Edit::SetConstraint` live in
            // `EditorSession::adopt_imported_meet` (shared with the web app).
            match st.adopt_imported_meet(index) {
                Ok(false) => {}
                Ok(true) => {
                    let dirty: BTreeSet<usize> = std::iter::once(index).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
    });
}
