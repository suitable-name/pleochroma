//! Per-facet and tier-wide index-wheel editing: gear-tooth highlight, per-facet
//! Remove/Detach/Add, and tier-wide Rotate/Mirror.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma_cut_core::EditError;
use slint::ComponentHandle;

use super::facet_overlay::{facet_map_from_aligned_solve, resubmit_facet_overlay};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// Highlights every facet on a clicked index-wheel tooth. A negative tooth (nothing hit) clears
/// the highlight. This is the reverse of the forward lookup in
/// `setup_toggle_multi_select_callback`, which maps tier indices to facet ids via
/// `FacetMap::facets_of_tier`. Here one gear tooth maps to every facet whose
/// `FacetMap::index_on_gear` matches, found by scanning `0..FacetMap::facet_count()`, since
/// `facet_map.rs` has no tooth-to-facets index. The tint reuses
/// [`FacetOverlay::multi_selected`], which avoids changing `facet_map.rs` or `preview_state.rs`.
///
/// Wired to `EditorModel.highlight_tooth(int)` (declared in `ui/models/editor.slint`). The
/// `tracked_clicked_tooth` mirror in `editor_tier_table.slint` calls it when
/// `SolidPreviewModel.diagram_clicked_tooth` changes.
///
/// `pub(super)` since [`super::tier_crud::setup_toggle_detach_callback`] is the one call site
/// that registers it.
pub(super) fn setup_highlight_tooth_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_highlight_tooth(move |tooth: i32| {
            let Some(preview_state) = auto_solve::preview_state() else {
                return;
            };
            let Ok(tooth) = u32::try_from(tooth) else {
                resubmit_facet_overlay(&preview_state, |overlay| overlay.multi_selected.clear());
                return;
            };
            let st = state.borrow();
            let facet_map = facet_map_from_aligned_solve(&st.design);
            let facet_count =
                u32::try_from(facet_map.facet_count()).expect("facet count fits in u32");
            let facet_ids: Vec<u32> = (0..facet_count)
                .filter(|&facet_id| facet_map.index_on_gear(facet_id as usize) == tooth)
                .collect();
            resubmit_facet_overlay(&preview_state, |overlay| {
                overlay.multi_selected = facet_ids;
            });
        });
}

/// The toast text for a per-facet Remove, Detach or Reattach that [`Design`] refused. A missing
/// position reports the same [`EditError`] shape as an out-of-range tier, but with an index that
/// is in range. Shown as is, that would read "tier index 0 out of range", so this names the tier
/// and the position instead. A real out-of-range tier keeps the error's own text.
///
/// [`Design`]: pleochroma_cut_core::Design
fn facet_edit_error_text(error: &EditError, position: f64) -> String {
    if error.index < error.tier_count {
        format!(
            "Tier #{} has no facet at index position {position}.",
            error.index + 1
        )
    } else {
        error.to_string()
    }
}

/// Per-facet "Remove" deletes one index-wheel occurrence from a tier via
/// [`Design::remove_orbit_member`]. Removing a member of a complete, non-detached orbit also
/// removes the rest of that orbit, so `symmetry_order` never misdescribes `indices`.
///
/// Wired to `EditorModel.facet_remove(int, float)` (declared in `ui/models/editor.slint`; tier
/// index, index-wheel position). The per-facet chips in `editor_inspector.slint` call it.
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`].
pub(super) fn setup_facet_remove_callback(
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
        .on_facet_remove(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let position = f64::from(position);
            let mut st = state.borrow_mut();
            let edit = match st.design.remove_orbit_member(tier_index, position) {
                Ok(edit) => edit,
                Err(e) => {
                    show_toast(&ui, &facet_edit_error_text(&e, position), "error");
                    return;
                }
            };
            match st.apply(edit) {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet "Detach"/"Reattach" toggle for ONE index-wheel occurrence. It is the per-facet
/// counterpart of `setup_toggle_detach_callback`'s whole-tier button, via
/// [`Design::detach_orbit_member`] and [`Design::reattach_orbit_member`]. The direction is
/// picked by whether `position` is already in the tier's `detached` list.
///
/// Wired to `EditorModel.facet_toggle_detach(int, float)` (declared in `ui/models/editor.slint`).
/// The per-facet chips in `editor_inspector.slint` call it, as they call
/// [`setup_facet_remove_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`].
pub(super) fn setup_facet_toggle_detach_callback(
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
        .on_facet_toggle_detach(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let position = f64::from(position);
            let mut st = state.borrow_mut();
            let Some(tier) = st.design.tiers.get(tier_index) else {
                return;
            };
            let edit_result = if tier.detached.contains(&position) {
                st.design.reattach_orbit_member(tier_index, position)
            } else {
                st.design.detach_orbit_member(tier_index, position)
            };
            let edit = match edit_result {
                Ok(edit) => edit,
                Err(e) => {
                    show_toast(&ui, &facet_edit_error_text(&e, position), "error");
                    return;
                }
            };
            match st.apply(edit) {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet "Add" adds one index-wheel occurrence to a tier via
/// [`Design::add_orbit_member`]. The addition expands to the full symmetry orbit, so no
/// half-populated orbit unit is left behind.
///
/// Wired to `EditorModel.facet_add(int, float)` (declared in `ui/models/editor.slint`). The
/// per-facet chips in `editor_inspector.slint` call it, as they call
/// [`setup_facet_remove_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_facet_add_callback(
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
        .on_facet_add(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .add_orbit_member(tier_index, f64::from(position))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide "Rotate" rotates every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) by `k_teeth` around the
/// gear, via [`Design::rotate_indices`].
///
/// Wired to `EditorModel.tier_rotate_indices(int, float)` (declared in
/// `ui/models/editor.slint`; tier index, teeth to rotate by) -- the inspector's
/// "rotate this tier" stepper in `editor_inspector.slint` calls it.
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_tier_rotate_indices_callback(
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
        .on_tier_rotate_indices(move |tier_index: i32, k_teeth: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .rotate_indices(tier_index, f64::from(k_teeth))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide "Mirror" mirrors every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) to the other side of the
/// symmetry axis, via [`Design::mirror_indices`].
///
/// Wired to `EditorModel.tier_mirror_indices(int)` (declared in
/// `ui/models/editor.slint`) -- called from the same inspector control in
/// `editor_inspector.slint` as [`setup_tier_rotate_indices_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_tier_mirror_indices_callback(
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
        .on_tier_mirror_indices(move |tier_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            // Goes through the shared session (`EditorSession::mirror_indices`).
            match st.mirror_indices(tier_index) {
                Ok(_) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_position_names_the_tier_and_the_position() {
        let error = EditError {
            index: 0,
            tier_count: 8,
        };
        assert_eq!(
            facet_edit_error_text(&error, 24.0),
            "Tier #1 has no facet at index position 24."
        );
    }

    #[test]
    fn an_out_of_range_tier_keeps_the_errors_own_text() {
        let error = EditError {
            index: 9,
            tier_count: 8,
        };
        assert_eq!(
            facet_edit_error_text(&error, 24.0),
            "tier index 9 out of range (schedule has 8 tier(s))"
        );
    }
}
