//! Pushes [`EditorState`](super::state::EditorState) into the editor panels and the viewport.
//!
//! Solving runs on an explicit action, not on every edit: [`refresh_editor_panel_stale`]
//! shows an edited design without solving it.
//!
//! The submodules: [`panel`] (the solved panel and its proportion and yield texts),
//! [`panel_stale`] (the unsolved counterpart and the Deep Solve and Optimize availability),
//! [`inspector`] (the design settings, the viewport material link and the Tier tab's facet
//! chips), [`viewport`] (the shared GPU and solid-preview viewport), [`solve_results`]
//! (per-tier Deep Solve and Optimize tables) and [`optimize_apply`] (the Optimize summary,
//! its ghost preview and the weight parser). This file re-exports what the rest of
//! `gui::editor` reaches through `view::*`.

mod design_label;
mod inspector;
mod optimize_apply;
mod panel;
mod panel_stale;
mod solve_results;
mod viewport;

use super::state;

// Re-exported so sibling `callbacks::*` modules can spell this `view::SolidLastSolved`
// (matching how they already reach every other `view::*` helper) rather than reaching
// past this module into `solid_preview::preview_state` directly.
pub(in crate::gui::editor) use crate::gui::solid_preview::preview_state::SolidLastSolved;

// The one name every place that shows the design (title, Baseline, Retarget, Compare) uses.
pub(in crate::gui::editor) use design_label::design_label;
pub(in crate::gui::editor) use inspector::{
    push_selected_tier_chips, sync_viewport_material_link, traced_c_axis_available,
};
// The Stone tab's Gem Data card, also pushed for the material a library design is shown in.
pub(in crate::gui) use inspector::push_gem_card;
pub(in crate::gui::editor) use optimize_apply::{
    optimize_result_rows, result_row, submit_design_ghost_preview,
    try_build_optimize_preview_design,
};
// Test-only paths to these helpers.
#[cfg(test)]
pub(in crate::gui::editor) use optimize_apply::{
    build_optimize_preview_design, optimize_status_text, parse_optimize_weights,
};
pub(in crate::gui::editor) use panel::{
    girdle_and_ratio_texts_from_solved, preform_mm_texts_from_solved,
    proportions_texts_from_solved, push_proportion_verdicts_from_solved, refresh_editor_panel,
};
pub(in crate::gui::editor) use panel_stale::refresh_editor_panel_stale;
pub(in crate::gui::editor) use solve_results::{
    deep_solve_tier_rows, facet_count_from_solved, format_deep_solve_report, optimize_change_rows,
};
pub(in crate::gui::editor) use viewport::{
    ReplanSource, planned_frame_size, push_has_design, reclaim_viewport_for_editor, refresh_all,
    refresh_all_now, submit_preview_replan, submit_preview_replan_chained,
    submit_preview_replan_for,
};
// `push_solved_preview` is also used by `editor::apply_matching_preview_frame`.
// `push_stale_content` stays `pub(super)` in `panel_stale`.
pub(in crate::gui::editor) use viewport::{push_late_findings, push_solved_preview};
