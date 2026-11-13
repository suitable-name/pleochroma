//! Wires `super::super::retarget` (the "Retarget for material" proposal
//! builder/applier) and `ui/components/retarget_dialog.slint` into the Edit tab --
//! `setup_*` functions, one per `RetargetDialog`/`EditorView` callback, mirroring
//! `solve_actions`'s Optimize callbacks in shape.
//!
//! # Where the target material comes from
//!
//! [`material::resolve_target_selection`] resolves the proposal's TARGET from
//! `RetargetModel`'s OWN `target_material_index`/`target_ri_override_text`
//! properties, NOT from `state.design.material` or the design settings panel's
//! combo: this dialog owns its target end to end, seeded from the design's current
//! material on `open()` (see [`proposal::setup_retarget_open_callback`]) and changed
//! only by this dialog's own picker. `retarget::apply` still only ever produces
//! `Edit::RetargetAngles` -- see [`apply::setup_retarget_apply_callback`] for how a
//! target material change actually reaches `design.material`.
//!
//! # Optimize is a search the cutter starts, and it runs off the UI thread
//!
//! Shift mode is pure angle arithmetic and stays synchronous. Optimize searches every crown
//! and pavilion angle in a range the cutter picks (`pleochroma_editor::retarget::search`),
//! which takes seconds to minutes, so it runs on a worker thread of [`optimize_run`] only
//! after the Search button is pressed. A picked option is copied into [`RETARGET_ASYNC`]'s
//! `pending` proposal and into [`check_run`]'s slot, so Apply and the comparison treat it
//! like a Shift result. [`RETARGET_ASYNC`] is `thread_local!` because Slint's event loop is
//! single-threaded. The worker's completion handler is `Send` and cannot capture the editor
//! state, so it hands the report back through `upgrade_in_event_loop`.
//!
//! # Shift mode's validity check runs off the UI thread too
//!
//! Judging a Shift plan (does the stone still close, keep its girdle and table) needs two
//! solves and optical traces. [`check_run`] runs that on a worker thread behind a serial
//! guard and keeps the verdict, re-anchored masts and optical table in its own
//! `thread_local!`. Apply is disabled while the verdict is `Checking...` or `Not valid`.
//! [`apply::setup_retarget_apply_callback`] applies the same gate to the Return key and the
//! compare window's "Keep after". Compare stays available for an invalid change.
//!
//! # Slint-free view-model split, for testability
//!
//! [`proposal_view::plan_view`]/[`proposal_view::candidate_row_views`]/
//! [`material::resolve_target_selection`]/[`apply::apply_pending_retarget`] take and
//! return only plain Rust types, so this group's `tests` module can exercise the
//! actual decision logic directly -- `proposal_view::push_retarget_view`/
//! `proposal_view::push_target_readout` are the only two functions that touch a
//! Slint type, thin enough not to need tests.
//!
//! # Module layout
//!
//! Split by concern: [`material`] (target material resolution/display),
//! [`proposal_view`] (the Slint-free proposal view model and its `push_*`
//! renderers), [`proposal`] (the open/proposal-changed callbacks and Shift mode's
//! own synchronous rebuild), [`check_run`] (Shift mode's off-thread validity check and
//! optical comparison), [`optimize_run`] (the Optimize search, its progress, its options
//! and the pick of one), [`apply`] (committing a pending proposal, and closing the dialog), and
//! [`snapshot`] (the "Set Baseline" and "Compare to Baseline" feature that also lives in
//! this dialog, and the held Retarget original). [`RETARGET_ASYNC`] and
//! [`apply_ghost_preview_or_revert`] are shared by several siblings, so they stay here.
//! [`compare_hooks`] is the narrow read-only surface the visual
//! before/after compare window (`gui::editor::compare`) reaches this dialog's pending
//! proposal and snapshot through -- it never commits anything itself; its "Keep
//! after" invokes `RetargetModel.apply()`, i.e. [`apply::setup_retarget_apply_callback`].

mod advanced_in_use;
mod apply;
mod check_run;
mod compare_hooks;
mod material;
mod optimize_run;
mod proposal;
mod proposal_view;
mod shift_seed;
mod snapshot;

#[cfg(test)]
mod apply_tests;
#[cfg(test)]
mod tests;

pub(in crate::gui::editor) use apply::{
    setup_retarget_apply_callback, setup_retarget_close_callback,
};
pub(in crate::gui::editor) use compare_hooks::{
    RetargetCompareInputs, RetargetKeepGuard, current_retarget_guard, discard_retarget_preview,
    retarget_compare_inputs,
};
pub(in crate::gui::editor) use proposal::{
    setup_retarget_open_callback, setup_retarget_proposal_changed_callback,
};
pub(in crate::gui::editor) use snapshot::{
    baseline_clone, baseline_marker, fill_baseline_masts, hold_reference_snapshot,
    install_baseline, original_for_compare, release_window_state as release_snapshot,
    setup_snapshot_callbacks, snapshot_for_compare,
};

use super::super::{
    auto_solve,
    retarget::{self, RetargetProposal},
    view,
};
use crate::{
    CompareModel, MainWindow, RetargetModel, bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::SolidPreviewState,
};
use pleochroma_cut_core::Design;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

thread_local! {
    /// The dialog's async Optimize run tracker. See the module doc comment.
    static RETARGET_ASYNC: RefCell<RetargetAsyncRun> = RefCell::new(RetargetAsyncRun::default());
}

/// [`RETARGET_ASYNC`]'s payload.
#[derive(Default)]
struct RetargetAsyncRun {
    /// Cancels the in-flight worker and reads its progress -- `None` whenever nothing is
    /// running.
    handle: Option<optimize_run::SearchHandle>,
    /// Bumped every time a run starts, is cancelled, or is superseded by a newer
    /// one, so a worker that finishes after being superseded can tell (by comparing
    /// the `run_id` it was launched with) and discard its own result instead of
    /// overwriting a newer run's.
    run_id: u64,
    /// The Optimize option the cutter picked, with the design generation it was built
    /// against. Same shape as `EditorState::pending_retarget`.
    pending: Option<(RetargetProposal, u64)>,
    /// Status-strip activity of the running search. Finished by
    /// [`Self::cancel_and_supersede`] so no stale chip is left behind.
    activity_id: Option<u64>,
}

impl RetargetAsyncRun {
    /// Cancels whatever is running (if anything), forgets the options a finished search
    /// offered and bumps `run_id`, so any result still in flight is discarded on arrival --
    /// shared by every place that starts a new run, cancels one outright, changes an input
    /// the options were computed from, or closes the dialog.
    fn cancel_and_supersede(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.run_id = self.run_id.wrapping_add(1);
        self.pending = None;
        optimize_run::forget_results();
        if let (Some(activity), Some(id)) = (auto_solve::activity(), self.activity_id.take()) {
            activity.finish(id);
        }
    }
}

/// Points the dialog's embedded comparison pane (`MainWindow`'s own `CompareModel`
/// instance, handled in `gui::editor::compare`) at the proposal just built:
/// `solved` re-opens the embedded session on it, otherwise the session is dropped so
/// the pane never shows a proposal that was dropped. The handlers read
/// the editor state, so callers must not hold a borrow of it across this call.
fn sync_embedded_comparison(ui: &MainWindow, solved: bool) {
    let model = ui.global::<CompareModel>();
    if solved {
        model.invoke_open_retarget_embedded();
    } else {
        model.invoke_close_embedded();
    }
}

/// Shows `design` retargeted by `proposal` as a ghost overlay in the shared solid viewport,
/// when `RetargetModel.preview_enabled` is on and `proposal` is `Some`. Returns whether the
/// candidate was queued on the solve worker. When it was not, the caller resubmits the live
/// design through [`view::submit_preview_replan`].
///
/// The candidate comes from [`Design::apply_edit`] on a clone, not `EditorState::apply`, so
/// `History`, `is_dirty` and the generation stay untouched.
fn apply_ghost_preview_or_revert(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
    proposal: Option<&RetargetProposal>,
) -> bool {
    if !ui.global::<RetargetModel>().get_preview_enabled() {
        return false;
    }
    if proposal.is_none() {
        return false;
    }
    let Some(candidate) = build_retarget_candidate(design) else {
        return false;
    };
    view::submit_design_ghost_preview(ui, render_ctx, preview_state, &candidate)
}

/// `design` with the held proposal's angle retarget and re-anchored masts applied, on a clone.
/// This is the candidate shown by both the viewport ghost and the compare window's "after"
/// side. `None` when the edit does not apply (a stale tier index) or no finished check
/// holds one.
///
/// The edit is the one the finished check validated, so the comparison shows what Apply
/// would keep. The target material is attached by the caller when it matters. Related tiers
/// are brought into line afterwards, as Apply does; a relation that cannot be satisfied is
/// left as the edit put it, and the verdict reports it.
#[must_use]
pub(in crate::gui::editor) fn build_retarget_candidate(design: &Design) -> Option<Design> {
    let mut candidate = design.clone();
    let edit = check_run::validated_candidate_edit(design).ok()?;
    candidate.apply_edit(edit).ok()?;
    retarget::anchors::fold_relations(&mut candidate).ok()?;
    Some(candidate)
}
