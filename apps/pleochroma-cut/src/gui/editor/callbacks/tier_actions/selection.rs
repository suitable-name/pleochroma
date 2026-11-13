//! The tier list's multi-select: Ctrl+click toggle and Shift+click range-select.

use std::{cell::RefCell, rc::Rc};

use slint::{ComponentHandle, Model};

use super::facet_overlay::{facet_map_from_aligned_solve, resubmit_facet_overlay};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    gui::{
        editor::{
            auto_solve,
            state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
        },
        tutorial_events::raise,
    },
};
use pleochroma_editor::guide::TIERS_MULTI_SELECTED;

/// The tier list's Ctrl+click: toggles one row in or out of [`EditorState::multi_selected`]
/// and patches `EditorTierItem::multi_selected` into the already-pushed `editor_tiers` model.
/// It does not call [`refresh_editor_panel_stale`]: a highlight changes nothing in `Design`,
/// so the validation banner must not turn "Not solved".
pub(in crate::gui::editor) fn setup_toggle_multi_select_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    // Wired from this call site, which already receives `state` (see
    // `setup_toggle_detach_callback`).
    setup_select_tier_range_callback(ui, state);

    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_multi_select(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            if !st.multi_selected.remove(&index) {
                st.multi_selected.insert(index);
            }
            let multi_selected = st.multi_selected.clone();
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Resolves every multi-selected TIER to its member FACET ids so the
            // group is visible in 3D too, not only as a one-pixel row border --
            // `FacetOverlay::multi_selected` is a flat facet id list, built against
            // the last rendered frame's masts the same way the hover/click
            // callbacks already build one.
            if let Some(preview_state) = auto_solve::preview_state() {
                let facet_map = facet_map_from_aligned_solve(&st.design);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
            drop(st);
            report_group(&ui, multi_selected.len());
        });
}

/// Tells the running guide that a group of `count` tiers is now ticked, once it holds two or
/// more (the multi-select tutorial waits for that). Called with the editor state released, so
/// the guide can re-check its step straight away.
fn report_group(ui: &MainWindow, count: usize) {
    if count >= 2 {
        raise(ui, TIERS_MULTI_SELECTED);
    }
}

/// The tier list's Shift+click: replaces [`EditorState::multi_selected`] with every tier index
/// from the selection anchor (`EditorModel.selected_tier_index`) to `index`, both included.
/// Unlike Ctrl+click, which flips one row, Shift+click always replaces the whole set, as in
/// spreadsheets. The row handler (`tier_table_row.slint`) checks Shift before Ctrl.
///
/// No anchor yet (`selected_tier_index < 0` -- a brand-new design, before any row
/// has ever been selected) falls back to a single-row selection of `index`: there
/// is nothing sensible to range from.
///
/// Shares the row-patch, count-push and facet-overlay tail of
/// [`setup_toggle_multi_select_callback`], and is registered from that callback.
fn setup_select_tier_range_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_select_tier_range(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            // A selected CONCAVE row (a position past the flat tiers) is no anchor: the
            // multi-select group is a flat-tier selection, so the range starts at the
            // clicked row instead of sweeping across rows it cannot contain.
            let flat_count = state.borrow().design.tiers.len();
            let anchor = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
                .ok()
                .filter(|&selected| selected < flat_count)
                .unwrap_or(index);
            let (lo, hi) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            let mut st = state.borrow_mut();
            st.multi_selected = (lo..=hi).collect();
            let multi_selected = st.multi_selected.clone();
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Same facet-overlay resubmit `setup_toggle_multi_select_callback` uses
            // -- resolves every multi-selected TIER to its member FACET ids so the
            // range is visible in 3D too, not only as a row border.
            if let Some(preview_state) = auto_solve::preview_state() {
                let facet_map = facet_map_from_aligned_solve(&st.design);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
            drop(st);
            report_group(&ui, multi_selected.len());
        });
}
