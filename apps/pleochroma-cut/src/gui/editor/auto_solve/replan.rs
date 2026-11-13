//! The solid-preview replan handshake.
//!
//! It converts an already-solved mast list into viewport planes, stashing the design snapshot a
//! matching preview frame will consume, and resubmitting a follow-up replan once a partial frame
//! goes stale.

use super::runtime::RUNTIME;
use crate::{
    MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::editor::view::{ReplanSource, submit_preview_replan_for},
};
use pleochroma::geometry::{ToolPrimitive, meet_solver::SolvedTier};
use pleochroma_cut_core::{Design, design::ToolPlacements};
use pleochroma_editor::solve_policy::IDLE_REPLAN_DEBOUNCE;
use slint::ComponentHandle;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

// The solved-masts-to-viewport-planes conversion lives in
// `pleochroma_editor::solve_policy`, shared with the web app. `IDLE_REPLAN_DEBOUNCE` is
// how long a "one edit behind" frame waits before [`schedule_idle_replan_if_stale`] fires.
pub(in crate::gui::editor) use pleochroma_editor::solve_policy::design_to_gpu_planes_from_solved;

/// The concave tools, and each one's `(concave tier, placement)`, that go with
/// [`design_to_gpu_planes_from_solved`]'s planes.
///
/// `solved` is `None` when the design does not solve. A planar design returns empty
/// vectors. If the tiers do not resolve, the tools are empty too, since a flat stone is
/// accurate and a partial set of tools is not. The manufacturability warnings explain why.
#[must_use]
pub(in crate::gui::editor) fn design_to_gpu_tools_from_solved(
    design: &Design,
    solved: Option<&[SolvedTier]>,
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    if design.concave_tiers.is_empty() {
        return (Vec::new(), Vec::new());
    }
    solved
        .and_then(|solved| design.concave_tools_from_solved(solved).ok())
        .unwrap_or_default()
}

/// Records `design` and `multi_selected` with the `generation` they were snapshotted
/// at. Called by `view::submit_preview_replan` on every replan. See
/// [`take_matching_design`] for the reader.
pub(in crate::gui::editor) fn stash_current_design(
    generation: u64,
    design: Arc<Design>,
    multi_selected: BTreeSet<usize>,
) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().current_design = Some((generation, design, multi_selected));
    });
}

/// Returns the `(Design, multi_selected)` that [`stash_current_design`] recorded for
/// exactly `generation`, when a solid-preview frame naming it lands. Returns `None`
/// when an edit has moved the design past it; the caller then changes nothing.
///
/// The match is consumed (`Option::take`). Later reprojected frames repeat the same
/// generation, and a rebuild on each would be redundant. A match also drops the pending
/// auto-solve debounce, since it would only recompute the masts this frame already has.
/// Background dispatches are left alone; their own generation checks still apply.
pub(in crate::gui::editor) fn take_matching_design(
    generation: u64,
) -> Option<(Arc<Design>, BTreeSet<usize>)> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let is_match = rt
            .current_design
            .as_ref()
            .is_some_and(|(stashed_generation, ..)| *stashed_generation == generation);
        if !is_match {
            return None;
        }
        let (_, design, multi_selected) = rt.current_design.take()?;
        rt.debounce = None;
        Some((design, multi_selected))
    })
}

/// Arms a debounced full replan of `design` after a "one edit behind" (partial) preview
/// frame, so the preview catches up once edits stop. The masts are already chained
/// forward, so the worker only verifies them.
///
/// The staleness check runs at fire time, not here. `apply_matching_preview_frame`
/// calls this before `SlintSolidSink::apply` sets this frame's `stale` flag, so reading
/// it now would see the previous frame's value.
///
/// At fire time the timer checks that `current_seq` is unchanged and that no newer
/// `current_design` was stashed. A new design, a cancel or a reset bumps `current_seq`,
/// so a timer armed for design A cannot replan design B.
pub(in crate::gui::editor) fn schedule_idle_replan_if_stale(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    design: &Design,
    multi_selected: &BTreeSet<usize>,
) {
    let Some((preview_state, solid_last_solved)) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        rt.preview_state.clone().zip(rt.solid_last_solved.clone())
    }) else {
        return;
    };
    let epoch = RUNTIME.with(|cell| cell.borrow().current_seq);
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = design.clone();
    let multi_selected = multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        IDLE_REPLAN_DEBOUNCE,
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nothing_newer_since = RUNTIME.with(|cell| {
                let rt = cell.borrow();
                rt.current_design.is_none() && rt.current_seq == epoch
            });
            if !nothing_newer_since || !ui.global::<SolidPreviewModel>().get_stale() {
                return;
            }
            submit_preview_replan_for(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                ReplanSource {
                    design: &design,
                    generation,
                    multi_selected: &multi_selected,
                },
                BTreeSet::new(),
                true,
            );
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(timer));
}
