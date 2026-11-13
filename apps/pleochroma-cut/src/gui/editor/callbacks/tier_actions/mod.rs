//! The Solve/New/Load-Selected/Undo/Redo/tier/preform/yield-input edit callbacks -- one `setup_*`
//! function per Slint callback.
//!
//! Every edit to the open design goes through `EditorState::apply`, so `History` is the only thing
//! that mutates it.
//!
//! Split into one file per user-facing concern: design lifecycle
//! ([`lifecycle`]/[`new_design`]/[`material_suggestion`]), undo/redo ([`history`]), design-wide
//! forms ([`design_forms`]), the Save Tier form ([`tier_form`]), angle nudging
//! ([`nudge`]), tier CRUD ([`tier_crud`]), generated tier series
//! ([`tier_generation`]), multi-select ([`selection`]), Adopt/Pin ([`adopt`]),
//! concave tiers ([`concave_tier`]), per-facet index editing ([`facet_editing`]),
//! material/symmetry settings
//! ([`materials_symmetry`]), gear remap ([`gear_remap`]), viewport material linking
//! ([`viewport_material`]), solid-view facet picking ([`solid_picking`]), frosting
//! ([`frosting`]), surface finish ([`surface`]), the Rough Planner's stones
//! ([`planned_stone`]), and small
//! shared helpers ([`misc`]) -- plus the shared facet-overlay state
//! ([`facet_overlay`]) every selection/picking path resubmits through.

mod adopt;
mod concave_tier;
mod design_forms;
mod facet_editing;
mod facet_overlay;
mod frosting;
mod gear_remap;
mod history;
mod lifecycle;
mod material_suggestion;
mod materials_symmetry;
mod misc;
mod new_design;
mod nudge;
// The Rough Planner's "Open in Editor" and "Use Color" load a design here and apply a planned
// stone to it; reached from `gui::rough_plan` as `gui::editor::planned_stone`.
pub(in crate::gui) mod planned_stone;
mod selection;
mod solid_picking;
mod surface;
#[cfg(test)]
mod tests;
mod tier_crud;
mod tier_form;
mod tier_generation;
mod viewport_material;

// Items that only these files share are reached as `super::<module>::<item>` and need no
// re-export. Only the callbacks that `callbacks::mod`'s hub imports are re-exported below.
pub(in crate::gui::editor) use design_forms::{
    setup_apply_cheater_offset_callback, setup_apply_design_meta_callback,
    setup_apply_preform_callback, setup_apply_preform_y_offset_callback,
    setup_apply_tier_note_callback, setup_apply_yield_inputs_callback,
};
pub(in crate::gui::editor) use gear_remap::{
    setup_gear_apply_callback, setup_gear_remap_cancel_callback, setup_gear_remap_confirm_callback,
};
pub(in crate::gui::editor) use history::{
    HistoryMove, refresh_after_history_move, refresh_after_renumbering_move, setup_redo_callback,
    setup_undo_callback,
};
pub(in crate::gui::editor) use lifecycle::{setup_load_selected_callback, setup_solve_callback};
pub(in crate::gui::editor) use material_suggestion::{
    setup_material_suggestion_accept_callback, setup_material_suggestion_dismiss_callback,
};
pub(in crate::gui::editor) use materials_symmetry::{
    setup_apply_design_material_callback, setup_apply_symmetry_callback,
    setup_material_guess_set_callback,
};
pub(in crate::gui::editor) use misc::{
    setup_anchor_explainer_dismiss_callback, setup_angle_live_preview_callback,
    setup_tier_filter_callback,
};
pub(in crate::gui::editor) use new_design::setup_new_design_create_callback;
pub(in crate::gui::editor) use nudge::{
    setup_inline_set_angle_callback, setup_nudge_angle_callback,
};
pub(in crate::gui::editor) use selection::setup_toggle_multi_select_callback;
// The facet overlay's merge-then-resubmit primitive, shared with the manipulation
// module (`gui::editor::manipulate`), which outlines the tiers a handle drag moves.
pub(in crate::gui::editor) use facet_overlay::resubmit_facet_overlay;
pub(in crate::gui::editor) use solid_picking::{
    clear_selected_facet, map_to_pick_coordinates, pick_margin, pick_pixels_to_logical,
    selected_facet_id, setup_solid_facet_click_callback, setup_solid_facet_hover_callback,
    setup_solid_selected_tier_changed_callback,
};
// The pick-frame arithmetic the manipulation module's tests round-trip.
#[cfg(test)]
pub(in crate::gui::editor) use solid_picking::{letterbox_margin, logical_to_pick_pixels};
// The inspector's Surface Finish state, pushed with the other selected-tier state.
pub(in crate::gui::editor) use surface::publish_selected_tier;
pub(in crate::gui::editor) use tier_crud::{
    setup_duplicate_tier_callback, setup_remove_tier_callback, setup_toggle_detach_callback,
};
pub(in crate::gui::editor) use tier_form::setup_save_tier_callback;
pub(in crate::gui::editor) use viewport_material::{
    setup_printed_proportions_callback, setup_stone_tab_callbacks,
};

/// The side count of a cylindrical preform. It is fixed, not tied to the index gear: the preform
/// is a piece of rough, so a count derived from the gear would be left behind by a gear change.
/// 96 keeps the rough smooth-looking at every zoom level. `loading::default_preform_for_schedule`
/// does not use this constant.
///
/// `pub(in crate::gui::editor)`, so the New Design form and the design forms share one value.
pub(in crate::gui::editor) const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;
