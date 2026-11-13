//! "Deep Solve"'s dispatch and completion handler.

use super::{DEEP_SOLVE_ACTIVITY_ID, LAST_DEEP_SOLVE_DELTAS, RunProvenance};
use crate::{
    EditorModel, MainWindow,
    gui::{
        editor::{
            auto_solve, deep_solve,
            stall_guard::stall_guard,
            state::EditorState,
            view::{deep_solve_tier_rows, format_deep_solve_report},
        },
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma::geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions};
use pleochroma_cut_core::Design;
use pleochroma_editor::guide::solving_events::{DEEP_SOLVE_FINISHED, DEEP_SOLVE_STARTED};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

/// "Deep Solve": the explicit, off-thread, cancellable verified-solve action.
///
/// It is a read-only diagnostic and never mutates `design` (see `deep_solve`'s module doc). The
/// busy check reads the `editor_deep_solve_running` property, since the completion handler cannot
/// clear `EditorState::deep_solve`.
///
/// `run_epoch` numbers the runs. Cancelling and starting a new run at once races the old run's
/// `on_done`, which can arrive after the new run has begun and overwrite its live status. A
/// superseded completion is therefore ignored. The counter moves only when a run starts; cancel
/// needs no guard (see [`super::deep_solve_pin::setup_deep_solve_cancel_callback`]).
pub(in crate::gui::editor) fn setup_deep_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    // The completion closure must be `Send`, so it cannot capture this `Rc`. It fetches the
    // state on the UI thread through `auto_solve::editor_state()` (the same stash as
    // `auto_solve::activity()`).
    auto_solve::stash_editor_state(&state);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>().on_deep_solve(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        stall_guard("on_deep_solve", || {
            begin_deep_solve_run(&ui, &state, &run_epoch);
        });
    });
}

/// What [`begin_deep_solve_run`] captures from `state` before dispatching the worker, so the
/// completion handler never re-reads the live state. A struct keeps that function under clippy's
/// function-length lint.
struct DeepSolveRunPrep {
    /// The design this run solves -- `deep_solve::spawn_deep_solve` derives
    /// `gear_teeth_abs`/`meet_tier_inputs` from it internally.
    design_for_run: Design,
    /// The design's plain solve, read from the rendered `solid_last_solved` cache (via
    /// `auto_solve::solid_last_solved`) when it matches this design's tier count. The completion
    /// handler uses it to show which tiers the verified repair disagreed with (see
    /// `deep_solve::tier_mast_deltas`).
    ///
    /// `None` when the cache is missing or misaligned (a tier was just added or removed). It is
    /// never filled by a `Design::solve` here, since that would block the UI thread.
    /// [`needs_background_baseline`] is `true` exactly then, and the worker computes the baseline
    /// via `SolveKind::Verified::compute_baseline`. See
    /// [`deep_solve::DeepSolveOutcome::Completed::baseline`] for how both sources are merged.
    current_masts: Option<Vec<SolvedTier>>,
    design_snapshot: Design,
    /// Whether an edit has landed since the design was loaded, created or replaced. `targets` is
    /// captured once at load and never re-derived, so a verdict describes the design as printed,
    /// not necessarily the one on screen. See `deep_solve::edited_since_load_caveat` for why this
    /// reads `History::can_undo` rather than `EditorState::is_dirty`.
    edited_since_load: bool,
    /// The design's file name, so a verdict can say where its printed figures were recorded.
    source_label: Option<String>,
    provenance: RunProvenance,
    this_run: u64,
    run_epoch_done: Arc<AtomicU64>,
}

/// Whether [`capture_deep_solve_run_prep`]'s cache read left `current_masts` usable as-is
/// (`false`), or whether the worker must compute a plain baseline solve first (`true`).
/// A pure function, so a test can call it without a real `Design` or `EditorState`. `cache` is
/// the caller's `Option<&[SolvedTier]>`, and `tier_count` is `Design::tiers.len()`.
#[must_use]
fn needs_background_baseline(cache: Option<&[SolvedTier]>, tier_count: usize) -> bool {
    cache.is_none_or(|solved| solved.len() != tier_count)
}

/// [`begin_deep_solve_run`]'s prelude -- see [`DeepSolveRunPrep`]'s own doc
/// comment. `st` is the already-borrowed `EditorState` (read-only: nothing here
/// mutates it); `run_epoch` is bumped here, the one side effect this otherwise
/// pure capture has.
fn capture_deep_solve_run_prep(st: &EditorState, run_epoch: &Arc<AtomicU64>) -> DeepSolveRunPrep {
    let cached = auto_solve::solid_last_solved().and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    // The cache is generation-tagged (`(u64, Vec<SolvedTier>)`); only the masts matter here.
    let current_masts = cached.and_then(|(_, solved)| {
        (!needs_background_baseline(Some(solved.as_slice()), st.design.tiers.len()))
            .then_some(solved)
    });
    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    DeepSolveRunPrep {
        design_for_run: st.design.clone(),
        current_masts,
        design_snapshot: st.design.clone(),
        edited_since_load: st.history.can_undo(),
        source_label: st.asc_filename.clone(),
        provenance: RunProvenance::capture(st),
        this_run,
        run_epoch_done: Arc::clone(run_epoch),
    }
}

/// The `on_deep_solve` handler's body, split out of [`setup_deep_solve_callback`] to keep that
/// function under clippy's function-length lint. See that function for `run_epoch`.
fn begin_deep_solve_run(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    run_epoch: &Arc<AtomicU64>,
) {
    // Also rejects a Solve or Optimize already running. All three read the `*_running` flags
    // directly, not the derived `EditorModel.busy_action`
    // (see `tier_actions::setup_solve_callback`).
    let model = ui.global::<EditorModel>();
    if model.get_deep_solve_running() || model.get_solve_running() || model.get_optimize_running() {
        return;
    }
    let mut st = state.borrow_mut();
    let Some(targets) = st.printed_proportions else {
        show_toast(
            ui,
            "Deep Solve needs this design's printed proportions; none are available.",
            "error",
        );
        return;
    };
    let DeepSolveRunPrep {
        design_for_run,
        current_masts,
        design_snapshot,
        edited_since_load,
        source_label,
        provenance,
        this_run,
        run_epoch_done,
    } = capture_deep_solve_run_prep(&st, run_epoch);
    // `current_masts` is `None` only when the cache was missing or misaligned. It is never
    // re-solved here, since that would be a UI-thread solve. The worker computes it instead.
    let need_baseline = current_masts.is_none();
    // A new run's result must not be pinned against the previous run's deltas (see
    // `LAST_DEEP_SOLVE_DELTAS`).
    LAST_DEEP_SOLVE_DELTAS.with(|cell| cell.borrow_mut().clear());

    ui.global::<EditorModel>().set_deep_solve_running(true);
    ui.global::<EditorModel>()
        .set_deep_solve_status("Deep solving... 0.0s elapsed".into());
    ui.global::<EditorModel>()
        .set_deep_solve_status_is_problem(false);
    raise(ui, DEEP_SOLVE_STARTED);

    // Registered before the worker spawns, so the status strip lists it from the first frame.
    // `auto_solve::activity()` is `None` only if this fires before `auto_solve::init`, which
    // cannot happen in practice; the run then simply has no activity entry. The cancel closure
    // goes through `state`, so it always cancels the current `EditorState::deep_solve`.
    let activity = auto_solve::activity();
    let activity_id = activity.as_ref().map(|a| {
        a.start(
            "deep_solve",
            "Deep Solve",
            Some({
                let state = Rc::clone(state);
                Box::new(move || {
                    if let Some(handle) = state.borrow().deep_solve.as_ref() {
                        handle.cancel();
                    }
                })
            }),
        )
    });
    if activity_id.is_some() {
        DEEP_SOLVE_ACTIVITY_ID.with(|cell| *cell.borrow_mut() = activity_id);
    }

    let ui_weak = ui.as_weak();
    let handle = deep_solve::spawn_deep_solve(
        ui_weak,
        design_for_run,
        targets,
        need_baseline,
        |ui: &MainWindow, progress: deep_solve::DeepSolveProgress| {
            ui.global::<EditorModel>().set_deep_solve_status(
                format!(
                    "Deep solving... {:.1}s elapsed",
                    progress.elapsed.as_secs_f32()
                )
                .into(),
            );
        },
        move |ui: &MainWindow, outcome: deep_solve::DeepSolveOutcome| {
            // This run is finishing on its own (as opposed to having already
            // been finished early by a Cancel click, see
            // `setup_deep_solve_cancel_callback`) -- `ActivityRegistry::finish`
            // is a harmless no-op if that already happened, per its own doc
            // comment.
            // Fetched here, on the UI thread, rather than captured: this
            // closure must be `Send`, and `ActivityRegistry` is an `Rc`.
            if let (Some(activity), Some(id)) = (auto_solve::activity(), activity_id) {
                activity.finish(id);
            }
            DEEP_SOLVE_ACTIVITY_ID.with(|cell| {
                if *cell.borrow() == activity_id {
                    *cell.borrow_mut() = None;
                }
            });
            // A superseded run (cancelled, then a fresh Deep Solve already
            // started before this one's background thread finished) -- ignore
            // it entirely rather than clobbering the current run's live status.
            if run_epoch_done.load(AtomicOrdering::Relaxed) != this_run {
                return;
            }
            // The design was replaced (New / Load Selected / Open) while this run was in flight.
            // Return before touching the busy flag or the panel: a fresh Deep Solve may already
            // be running on the new design, and this stale completion would overwrite its state.
            // `clear_analysis_results` already ran at replacement; see `apply_deep_solve_outcome`.
            if provenance.design_replaced() {
                return;
            }
            // Cleared on every path below. The status is the live elapsed readout and the flag
            // gates the Deep Solve button, so skipping either would freeze the strip.
            ui.global::<EditorModel>().set_deep_solve_running(false);
            // Fetched on the UI thread, not captured, because the closure must be `Send`.
            // It is stashed by this `setup_*` call, so `None` is unreachable; it is still handled.
            let Some(state_for_done) = auto_solve::editor_state() else {
                tracing::warn!("deep solve finished before the editor state was stashed");
                return;
            };
            apply_deep_solve_outcome(
                ui,
                outcome,
                &DeepSolveRunContext {
                    provenance: &provenance,
                    targets,
                    source_label: source_label.as_deref(),
                    edited_since_load,
                    current_masts: current_masts.as_deref(),
                    design_snapshot: &design_snapshot,
                    state: &state_for_done,
                },
            );
        },
    );
    st.deep_solve = Some(handle);
}

/// What [`apply_deep_solve_outcome`] needs beyond `ui` and `outcome`, bundled to satisfy clippy's
/// `too_many_arguments`. Each field is what the dispatch closure captured.
#[derive(Clone, Copy)]
struct DeepSolveRunContext<'a> {
    provenance: &'a RunProvenance,
    targets: ExternalProportions,
    source_label: Option<&'a str>,
    edited_since_load: bool,
    current_masts: Option<&'a [SolvedTier]>,
    design_snapshot: &'a Design,
    /// Lets the `Completed` arm stamp [`EditorState::deep_solve_result_generation`]. This is a
    /// separate `Rc` clone from the one the dispatch body still borrows when this is built.
    state: &'a Rc<RefCell<EditorState>>,
}

/// The Deep Solve completion handler, split out of the closure passed to
/// [`deep_solve::spawn_deep_solve`] to keep that function under clippy's function-length lint.
/// `ctx` holds what the closure captured at dispatch.
fn apply_deep_solve_outcome(
    ui: &MainWindow,
    outcome: deep_solve::DeepSolveOutcome,
    ctx: &DeepSolveRunContext<'_>,
) {
    let DeepSolveRunContext {
        provenance,
        targets,
        source_label,
        edited_since_load,
        current_masts,
        design_snapshot,
        state,
    } = *ctx;
    // The caller filters out a replaced design before this point, so this function has no
    // effect on one.
    debug_assert!(
        !provenance.design_replaced(),
        "the caller must filter out a replaced design before calling this"
    );
    match outcome {
        deep_solve::DeepSolveOutcome::Cancelled => {
            ui.global::<EditorModel>()
                .set_deep_solve_status("Deep Solve cancelled.".into());
            ui.global::<EditorModel>()
                .set_deep_solve_status_is_problem(false);
        }
        deep_solve::DeepSolveOutcome::Completed {
            solved,
            report,
            baseline,
        } => {
            // Reached only for an edit, so the caveat is that a few edits have landed since.
            let mut status =
                format_deep_solve_report(&report, provenance.is_stale(), design_snapshot);
            // Names which printed figures the verdict used and where they came from, and warns
            // when edits since load are not reflected in them.
            status.push_str(&deep_solve::format_verification_targets(
                &targets,
                source_label,
            ));
            status.push_str(deep_solve::edited_since_load_caveat(edited_since_load));
            // The per-tier breakdown behind the verdict. The Log popup's row model below is the
            // fuller view. `current_masts` (the cache read at dispatch) and `baseline` (the
            // worker's solve, computed only when that cache was unusable) are mutually exclusive.
            // If neither is available (an unsolvable design) the table is omitted.
            let current_masts = current_masts.map(<[SolvedTier]>::to_vec).or(baseline);
            if let Some(current) = current_masts.as_deref() {
                let deltas = deep_solve::tier_mast_deltas(current, &solved);
                status.push_str(&deep_solve::format_tier_mast_deltas(&deltas));
                // The same disagreement as a real per-tier table, for the
                // Log popup -- the status line only has room for a summary.
                ui.global::<EditorModel>()
                    .set_deep_solve_tier_rows(ModelRc::new(VecModel::from(deep_solve_tier_rows(
                        &deltas,
                        design_snapshot,
                    ))));
                // Kept so `setup_deep_solve_pin_callback` can look a row's mast up by tier index
                // without the report holding an `EditorState` reference.
                LAST_DEEP_SOLVE_DELTAS.with(|cell| cell.borrow_mut().clone_from(&deltas));
            }
            ui.global::<EditorModel>()
                .set_deep_solve_status(status.into());
            ui.global::<EditorModel>()
                .set_deep_solve_status_is_problem(!report.accepted);
            // Stamps the generation this verdict describes (`started_generation`, not the live
            // counter), so later edits can mark it stale (`state::result_is_stale`). The stale
            // flag is also set now, so a run that went stale mid-search shows it immediately.
            state.borrow_mut().deep_solve_result_generation = Some(provenance.started_generation);
            ui.global::<EditorModel>()
                .set_deep_solve_stale(provenance.is_stale());
            raise(ui, DEEP_SOLVE_FINISHED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- needs_background_baseline: the UI-thread solve guard ---
    //
    // The pure decision in `capture_deep_solve_run_prep`: is the cached plain solve usable?
    // It must never fall back to a synchronous `Design::solve()` on the UI thread. The worker
    // computes a missing baseline instead (`SolveKind::Verified::compute_baseline`).

    fn dummy_solved_tier(mast: f64) -> pleochroma::geometry::meet_solver::SolvedTier {
        pleochroma::geometry::meet_solver::SolvedTier {
            mast,
            strategy: pleochroma::geometry::meet_solver::SolveStrategy::DependencyOrder,
            detail: String::new(),
        }
    }

    #[test]
    fn needs_background_baseline_is_false_when_the_cache_matches_the_tier_count() {
        let cache = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        assert!(!needs_background_baseline(Some(&cache), 2));
    }

    #[test]
    fn needs_background_baseline_is_true_when_the_cache_is_missing() {
        assert!(needs_background_baseline(None, 2));
    }

    #[test]
    fn needs_background_baseline_is_true_when_the_cache_is_the_wrong_tier_count() {
        // A tier was just added or removed and the background solve has not caught up yet.
        let cache = vec![dummy_solved_tier(1.0)];
        assert!(needs_background_baseline(Some(&cache), 2));
    }
}
