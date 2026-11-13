//! Optimize's completion handler, its "Cancel"/"Apply" buttons, and the
//! before/after "Preview" toggle for a pending (not yet applied) result.

use super::{RunProvenance, clear_analysis_results};
use crate::{
    EditorModel, MainWindow, OptimizeModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            baseline::{self, Origin},
            optimize_panel::{self, FinishedRun},
            optimize_solve::OptimizeRunOutcome,
            state::EditorState,
            trials::{self, TrialOrigin},
            view::{
                SolidLastSolved, refresh_all_now, submit_design_ghost_preview,
                submit_preview_replan, try_build_optimize_preview_design,
            },
        },
        show_toast,
        solid_preview::{cut_slider, preview_state::SolidPreviewState},
        tutorial_events::raise,
    },
};
use pleochroma_cut_core::{Design, DesignSolveError, OptimizeOutcome};
use pleochroma_editor::{
    edit_intent::DRAIN_INTERVAL, guide::solving_events::OPTIMIZE_FINISHED,
    optimize_view::optimize_run_status,
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// The completion handler for [`super::optimize_run::setup_optimize_callback`]'s run.
///
/// It is split out to keep that callback under clippy's function-length lint. `pending_optimize`
/// is set here (by `optimize_panel::show_run`) to the best candidate, or `None` when nothing can
/// safely be applied (failed, no candidates, or stale).
///
/// `before_score` and `after_score` are both at `ObjectiveFidelity::Full`, while the search
/// compares candidates at `Fast`. So `after_score >= before_score` (both internal losses, lower
/// is better; the screen shows `100 - loss`) is a real regression. It is reported as a problem,
/// not as the success summary, since the per-tier table alone would read as success. Apply stays
/// available: a worse full-fidelity score is a warning, not a ban.
///
/// A non-stale outcome also toasts, so completion is visible when the Optimize tab is closed. If
/// a result is waiting for Apply, the inspector switches to the Optimize tab. A stale result gets
/// its own toast instead (see the `stale` branch below).
pub(super) fn handle_optimize_outcome(
    ui: &MainWindow,
    outcome: OptimizeRunOutcome,
    provenance: &RunProvenance,
    pending_optimize: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    // The design this run started from, for naming the tiers in the per-tier change
    // table. A snapshot, not the live state: this runs from a `Send` closure.
    design: &Design,
    // Wall time of the run, for the next run's time estimate.
    elapsed_secs: f32,
) {
    // Cleared first on every path, including the early return for a replaced design. Otherwise
    // the progress readout and the Optimize button gate would stay stuck mid-run.
    ui.global::<EditorModel>().set_optimize_running(false);
    ui.global::<OptimizeModel>().set_progress(0.0);
    // New / Load Selected / Open replaced the design during the run: the result describes another
    // stone and the panel is already blank, so there is nothing to show. No toast either.
    if provenance.design_replaced() {
        clear_analysis_results(ui);
        return;
    }
    match outcome {
        OptimizeRunOutcome::Failed { error } => {
            // `MissingAnchor`'s `Display` names the block(s) but not the remedy. The plain Solve
            // banner shows the same text for that failure, so the "add a tier" instruction is
            // appended for that variant only. Other `DesignSolveError`s already read as complete
            // sentences (see that type's `Display`).
            let status = if matches!(error, DesignSolveError::MissingAnchor(_)) {
                format!(
                    "Optimize failed: {error} -- add an Exact Scale Value tier there \
                     and Solve first."
                )
            } else {
                format!("Optimize failed: {error}.")
            };
            ui.global::<EditorModel>()
                .set_optimize_status(status.clone().into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(true);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            show_toast(ui, &status, "error");
        }
        OptimizeRunOutcome::Completed { result } | OptimizeRunOutcome::Cancelled { result } => {
            // Reached for an edit only (a replacement returned above). The result describes the
            // design a few edits ago: shown for reference, with Apply disabled.
            let stale = provenance.is_stale();
            // Candidates qualify only if their full-fidelity score is no worse than the start,
            // so `>=` also catches a tie.
            let no_net_improvement = result
                .candidates
                .first()
                .is_some_and(|best| best.score >= result.outcome.before_score);
            let summary = optimize_run_status(&result, Some(elapsed_secs));
            let status = if no_net_improvement {
                format!(
                    "No net improvement at full tilt fidelity -- not recommended \
                     ({summary})"
                )
            } else {
                summary
            };
            ui.global::<EditorModel>()
                .set_optimize_status(status.clone().into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(no_net_improvement);
            // Pushed now, as `view::push_stale_content` keeps doing on later edits. Otherwise the
            // flag would wait for the next edit.
            ui.global::<EditorModel>().set_optimize_stale(stale);
            // The candidate rows, the best candidate picked, its result and change tables
            // (which tiers Optimize wants to move, so a cutter can read the proposal
            // before deciding whether to apply it) and `pending_optimize`.
            let picked = optimize_panel::show_run(
                ui,
                pending_optimize,
                FinishedRun {
                    result,
                    design: design.clone(),
                    generation: provenance.started_generation,
                    stale,
                    elapsed_secs,
                },
            );
            let can_apply = picked.is_some() && !stale;
            if stale {
                show_toast(
                    ui,
                    "The design changed while Optimize was running -- its result is \
                     shown for reference but can no longer be applied. Re-run \
                     Optimize on the current design.",
                    "info",
                );
            } else {
                // A toast, and the Optimize tab opened when there is something to apply, so
                // completion is noticed even if the cutter switched tabs during the run.
                show_toast(
                    ui,
                    &format!("Optimize finished: {status}"),
                    if no_net_improvement {
                        "info"
                    } else {
                        "success"
                    },
                );
                if can_apply {
                    ui.global::<EditorModel>().set_inspector_tab(2);
                    ui.global::<EditorModel>().set_inspector_collapsed(false);
                }
                raise(ui, OPTIMIZE_FINISHED);
            }
        }
    }
}

/// "Cancel", shown only while an Optimize run is in progress. See
/// [`crate::gui::editor::optimize_solve::OptimizeSolveHandle::cancel`] for what it stops.
/// `editor_optimize_running` is left alone: the worker's own `on_done` still carries the honest
/// partial result and is what clears the flag, within one search evaluation's latency.
///
/// The status text changes at once, as Deep Solve's cancel does (see
/// [`super::deep_solve_pin::setup_deep_solve_cancel_callback`]). Clearing the running flag early
/// would let a click on "Optimize" start a second search while the first is still finishing.
///
/// The button latches locally in Slint (`cancel_requested` in
/// `ui/components/editor_inspector/optimize_tab.slint`), so a second click never reaches this
/// callback.
pub(in crate::gui::editor) fn setup_optimize_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_optimize_cancel(move || {
        if let Some(handle) = state.borrow().optimize.as_ref() {
            handle.cancel();
        }
        if let Some(ui) = ui_weak.upgrade() {
            // The search polls `cancel` mid-run, so "finishing the evaluation" is accurate.
            ui.global::<EditorModel>()
                .set_optimize_status("Cancelling -- finishing the current evaluation...".into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(false);
        }
    });
}

/// "Apply", shown only while `EditorState::pending_optimize` holds a result that is not stale.
/// It commits the held [`OptimizeOutcome`] through `optimize_panel::apply_pending` as one undo
/// step (the candidate's angles and masts, plus the tiers that follow a relation). It is the
/// only place an Optimize result becomes an edit.
///
/// The generation is checked again here as a second guard, so a stale tier index is never read
/// against a design it does not describe.
///
/// `pending_optimize` is peeked, not taken, and consumed only after the apply succeeds. Taking
/// it first would lose the result for good after a late Apply click, while the rows stay on
/// screen. A failed apply keeps it too: each apply sets absolute angles, so a retry is harmless.
pub(in crate::gui::editor) fn setup_optimize_apply_callback(
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
    ui.global::<EditorModel>().on_optimize_apply(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let snapshot = st
            .pending_optimize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some((outcome, started_generation)) = snapshot else {
            return;
        };
        if st.generation.load(AtomicOrdering::Relaxed) != started_generation {
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            // Amber ("warning"), not "error": nothing failed. The design moved on
            // before this result could be trusted.
            show_toast(
                &ui,
                "Result discarded: the design changed while Optimize was running. Re-run \
                 Optimize.",
                "warning",
            );
            return;
        }
        // The design as it is before the apply, kept as a trial once the apply went through.
        let before = trials::hold(
            &st,
            TrialOrigin::BeforeOptimize,
            Some("Before Optimize".to_owned()),
        );
        // The picked candidate: angles and masts in one undo step, with the tiers that
        // follow a relation moved along.
        match optimize_panel::apply_pending(&mut st, &outcome) {
            Ok(applied) => {
                // The design before, then the candidate now (its figures arrive with the strip's
                // next score).
                trials::record_held(&ui, &st, before);
                trials::record_after_apply(&ui, &st, TrialOrigin::OptimizeCandidate);
                // Only consumed now that the apply actually went through.
                *st.pending_optimize
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                // The candidate list described the design before this apply.
                optimize_panel::clear_results(&ui);
                // The applied design is the new Baseline, so hand-tuning starts from the result.
                baseline::install(Origin::Optimize, &st);
                // `refresh_all_now` runs a real solve (inline for a cheap design, otherwise in the
                // background), so the table and viewport read solved right after Apply. A plain
                // panel refresh would show "Not solved" with "-" masts, as if the commit were
                // still pending.
                drop(st);
                refresh_all_now(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &state,
                    false,
                );
                // The result text still describes the committed proposal, so it is prefixed to
                // show that it is no longer pending.
                let applied_status = format!(
                    "Applied: {}",
                    ui.global::<EditorModel>().get_optimize_status()
                );
                ui.global::<EditorModel>()
                    .set_optimize_status(applied_status.into());
                show_toast(
                    &ui,
                    &format!("Applied {applied} tier change(s) from Optimize."),
                    "success",
                );
            }
            Err(e) => {
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                show_toast(&ui, &e, "error");
            }
        }
    });
}

/// The "Preview" toggle: shows the pending Optimize result's candidate geometry in the solid
/// viewport before Apply, and restores the real design as soon as it is switched off or the
/// result can no longer be applied. Wired to `EditorModel.optimize_preview_toggled(bool)`.
///
/// On: the candidate is built by [`try_build_optimize_preview_design`] from the design the
/// pending result ran against, which may differ from the current design after further edits.
/// It is shown via [`submit_design_ghost_preview`], which never touches `solid_last_solved` or
/// the generation stash, so the next real edit's resolve is unaffected. The candidate solves on
/// a background worker, so the toggle does not block.
///
/// Off, or nothing pending: the real design is resubmitted through [`submit_preview_replan`].
///
/// The checkbox raises this from a Slint `changed` handler, which can run while a writer holds
/// the editor state. The state is therefore only `try_borrow`ed. A busy state is retried a few
/// ticks later ([`cut_slider::retries_after_busy`]), as the Cut slider's replan is.
pub(in crate::gui::editor) fn setup_optimize_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let preview = Rc::new(OptimizePreview {
        ui_weak: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        latest_toggle: LatestToggle::default(),
    });
    ui.global::<EditorModel>()
        .on_optimize_preview_toggled(move |enabled: bool| {
            preview.latest_toggle.set(enabled);
            run_optimize_preview(&preview, cut_slider::REPLAN_RETRIES);
        });
}

/// What one Preview toggle needs, shared by the callback and its retries.
struct OptimizePreview {
    ui_weak: slint::Weak<MainWindow>,
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    /// What the checkbox said at its newest toggle. A retry reads it when it runs, never
    /// the value the click carried when it first met a held editor state: the cutter may
    /// have switched the box the other way since, and replaying the old value would show
    /// the ghost with the box unticked (or put the real design back over a ticked box).
    latest_toggle: LatestToggle,
}

/// The Preview checkbox's newest value, written by every toggle and read by whichever
/// attempt (the click itself or a later retry) is running.
#[derive(Default)]
struct LatestToggle(Cell<bool>);

impl LatestToggle {
    fn set(&self, enabled: bool) {
        self.0.set(enabled);
    }

    const fn get(&self) -> bool {
        self.0.get()
    }
}

/// The pending Optimize result the Preview may show, or `None` for the real design.
///
/// `pending` is the result and the design generation its search ran against
/// (`EditorState::pending_optimize`). Only a switched-on toggle whose result still matches
/// `current_generation` previews it: a result from an older generation holds tier indices
/// that no longer name the same tiers.
#[must_use]
fn preview_candidate<T>(
    enabled: bool,
    pending: Option<(T, u64)>,
    current_generation: u64,
) -> Option<T> {
    pending
        .filter(|_| enabled)
        .and_then(|(outcome, started_generation)| {
            (started_generation == current_generation).then_some(outcome)
        })
}

/// One Preview toggle: shows the pending result's ghost, or puts the real design back, as
/// the checkbox's newest value says ([`OptimizePreview::latest_toggle`]). When a writer
/// holds the editor state it tries again on the next tick, until `retries_left` runs out;
/// the retry re-reads the checkbox's value then.
fn run_optimize_preview(preview: &Rc<OptimizePreview>, retries_left: u8) {
    let Some(ui) = preview.ui_weak.upgrade() else {
        return;
    };
    let Ok(st) = preview.state.try_borrow() else {
        if let Some(next) = cut_slider::retries_after_busy(retries_left) {
            let again = Rc::clone(preview);
            slint::Timer::single_shot(DRAIN_INTERVAL, move || {
                run_optimize_preview(&again, next);
            });
        }
        return;
    };
    let enabled = preview.latest_toggle.get();
    let pending = enabled
        .then(|| {
            st.pending_optimize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .flatten();
    // `started_generation` is checked: a result from an older generation holds tier indices for
    // a tier list that has since changed, so it must not preview. Apply is greyed out for the
    // same reason. `view::apply_pending_optimize_ghost` makes the same check, but it is private
    // there, so it is repeated here.
    let current_generation = st.generation.load(AtomicOrdering::Relaxed);
    if let Some(outcome) = preview_candidate(enabled, pending, current_generation) {
        let candidate = match try_build_optimize_preview_design(&st.design, &outcome) {
            Ok(candidate) => candidate,
            Err(error) => {
                crate::gui::show_toast(&ui, &error, "error");
                return;
            }
        };
        if submit_design_ghost_preview(&ui, &preview.render_ctx, &preview.preview_state, &candidate)
        {
            return;
        }
        // The ghost could not be queued -- fall through and show the real design
        // instead of leaving whatever the viewport had.
    }
    submit_preview_replan(
        &ui,
        &preview.render_ctx,
        &preview.preview_state,
        &preview.solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_switched_on_preview_of_a_current_result_shows_the_ghost() {
        // On, and the search ran against the design as it is now.
        assert_eq!(
            preview_candidate(true, Some(("result", 7)), 7),
            Some("result")
        );
        // Off: the real design comes back whatever is pending.
        assert_eq!(preview_candidate(false, Some(("result", 7)), 7), None);
        // Nothing pending.
        assert_eq!(preview_candidate::<&str>(true, None, 7), None);
        // The design changed after the search (a tier removed, an angle nudged): its
        // tier indices no longer name the same tiers.
        assert_eq!(preview_candidate(true, Some(("result", 6)), 7), None);
        assert_eq!(preview_candidate(true, Some(("result", 8)), 7), None);
    }

    /// A toggle that met a held editor state is retried a tick later. The retry shows what
    /// the checkbox says THEN: ticked, unticked and ticked again before the retry runs
    /// must never replay the first click's value.
    #[test]
    fn a_retry_follows_the_checkbox_not_the_click_it_was_scheduled_for() {
        let toggle = LatestToggle::default();
        let pending = || Some(("ghost", 7));
        let attempt = |toggle: &LatestToggle| preview_candidate(toggle.get(), pending(), 7);

        toggle.set(true);
        assert_eq!(attempt(&toggle), Some("ghost"), "the click itself");
        // The click met a held state and scheduled a retry; the cutter unticks the box
        // before the retry runs. The retry puts the real design back.
        toggle.set(false);
        assert_eq!(
            attempt(&toggle),
            None,
            "the retry after the box was unticked"
        );
        toggle.set(true);
        assert_eq!(attempt(&toggle), Some("ghost"), "ticked again");
    }
}
