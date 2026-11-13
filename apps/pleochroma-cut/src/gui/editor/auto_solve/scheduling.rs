//! Deciding whether a solve should run synchronously, be auto-solved after a debounce,
//! or be skipped, plus recording/reading this design's last measured solve time.

use super::{dispatch::dispatch_background_solve, runtime::RUNTIME};
use crate::{
    EditorModel, MainWindow, bridge::render_thread::RenderContext, gui::editor::state::EditorState,
};
use pleochroma_editor::solve_policy::AUTO_SOLVE_DEBOUNCE;
use slint::ComponentHandle;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

// The pure policy (synchronous-solve cutoffs, auto-solve eligibility and its "off for
// this design" note) lives in `pleochroma_editor::solve_policy`, shared with the web app.
pub(super) use pleochroma_editor::solve_policy::{auto_solve_off_note, should_schedule_auto_solve};

/// Records a just-completed solve's wall time (background or synchronous) as this
/// design's new estimate for [`should_schedule_auto_solve`]'s next decision.
pub(in crate::gui::editor) fn record_solve_duration(elapsed: Duration) {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = Some(elapsed));
}

/// The loaded design's last measured solve time, if any. `view::refresh_all` passes it to
/// the synchronous-solve decision for every caller that keeps the loaded design. Replacing
/// the design wholesale clears it first, via [`reset_for_new_design`].
#[must_use]
pub(in crate::gui::editor) fn last_solve() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// Test-only view of the measured solve time, for asserting on [`record_solve_duration`]
/// and [`reset_for_new_design`]. Same value as [`last_solve`].
#[cfg(test)]
#[must_use]
pub(super) fn last_measured_solve_duration() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// Resets the scheduling state when `EditorState` is replaced wholesale (New, Load
/// Selected, Open). Clears the measured solve time, the pending debounce and idle-replan
/// timers, the stash and any queued dispatch, and sets `frame_pending`.
///
/// `current_seq` is bumped, so a background solve still running for the old design is
/// retired at once. Its completion then fails `is_current` and cannot touch the banner.
/// `generation` is also passed to the solid preview as a floor, so a frame queued for the
/// old design is not applied.
///
/// `solve_in_flight` is not reset: the old worker is still running and clears it itself
/// in `free_in_flight_slot`. Resetting it here would allow a second worker to start
/// while the first is still active.
pub(in crate::gui::editor) fn reset_for_new_design(generation: u64) {
    if let Some(preview_state) = super::runtime::preview_state() {
        preview_state.bump_generation_floor(generation);
    }
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.last_solve = None;
        rt.current_seq += 1;
        rt.debounce = None;
        rt.idle_replan = None;
        rt.current_design = None;
        rt.pending_dispatch = None;
        rt.frame_pending = true;
    });
}

/// Whether the design just loaded wholesale still waits to be framed in the picture (set by
/// [`reset_for_new_design`], cleared by [`clear_frame_pending`]): the BACKGROUND solve's
/// first result frames it when the cutter has not zoomed.
#[must_use]
pub(in crate::gui::editor) fn frame_pending() -> bool {
    RUNTIME.with(|cell| cell.borrow().frame_pending)
}

/// The new design has been framed (or the cutter's own zoom was kept): later results of the
/// same design leave the camera alone.
pub(in crate::gui::editor) fn clear_frame_pending() {
    RUNTIME.with(|cell| cell.borrow_mut().frame_pending = false);
}

/// Whether a solid-preview frame may push the design's solve status (the strip's
/// "Solved", the tier table's masts, the warnings, the yield figures) given the
/// Auto-Solve budget and the status the design shows right now.
///
/// With Auto-Solve Off (`budget_ms <= 0`) only pressing Solve (or New/Load/Open, which
/// solve through `view::refresh_all`) may mark the design solved: a design the last edit
/// left `"stale"` stays "Not solved" even though the preview worker re-solved it for
/// the picture. Every other budget, and every other state, is unchanged: a design that
/// shows "solved"/"failed"/"solving" already got there through a real solve.
#[must_use]
pub(in crate::gui::editor) fn preview_frame_may_push_status(
    budget_ms: i32,
    solve_state: &str,
) -> bool {
    !(budget_ms <= 0 && solve_state == "stale")
}

/// [`preview_frame_may_push_status`] for the live `EditorModel`: the gate
/// `editor::apply_matching_preview_frame` applies before it pushes a frame's status.
#[must_use]
pub(in crate::gui::editor) fn preview_may_push_status(ui: &MainWindow) -> bool {
    let model = ui.global::<EditorModel>();
    preview_frame_may_push_status(
        model.get_auto_solve_budget_ms(),
        model.get_solve_state().as_str(),
    )
}

/// Called at the end of `super::super::view::refresh_editor_panel_stale` -- every edit
/// callback's stale-refresh path calls that function with a fixed, already-established
/// signature, so this hook reads everything it needs (the budget property, `state`'s
/// design/generation) rather than asking for new parameters. See
/// [`should_schedule_auto_solve`] for the eligibility rule.
pub(in crate::gui::editor) fn on_edit(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let budget_ms =
        u64::try_from(ui.global::<EditorModel>().get_auto_solve_budget_ms()).unwrap_or(0);
    let budget = Duration::from_millis(budget_ms);
    let last_solve = RUNTIME.with(|cell| cell.borrow().last_solve);

    if !should_schedule_auto_solve(last_solve, budget) {
        // Only worth a dedicated note when THIS design is the reason (its own last
        // solve exceeded a real, nonzero budget) -- a `0` budget is a deliberate,
        // global "auto-solve off" choice that the ordinary stale banner already
        // covers without needing an extra explanation.
        if !budget.is_zero()
            && let Some(last) = last_solve
        {
            ui.global::<EditorModel>()
                .set_status_text(auto_solve_off_note(last).into());
            ui.global::<EditorModel>().set_status_is_problem(true);
        }
        // A queued `PendingDispatch` names an older design, so it must not replay once
        // auto-solve is not eligible. The next eligible edit queues a fresh one.
        RUNTIME.with(|cell| cell.borrow_mut().pending_dispatch = None);
        return;
    }

    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = state.design.clone();
    let generation = Arc::clone(&state.generation);
    // Captured with the `design` snapshot. See `dispatch_background_solve`.
    let started_generation = generation.load(Ordering::Relaxed);
    let multi_selected = state.multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        AUTO_SOLVE_DEBOUNCE,
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                dispatch_background_solve(
                    &ui,
                    &render_ctx,
                    design.clone(),
                    &generation,
                    multi_selected.clone(),
                    started_generation,
                );
            }
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(timer));
}
