//! "Deep Solve": an explicit, cancellable diagnostic that runs next to the ordinary "Solve" button
//! (which only calls the cheap `solve_meet_points`).
//!
//! It calls `solve_meet_points_verified_with` instead, which scores candidate mast configurations
//! against the design's printed proportions rather than trusting every self-consistent meet vertex.
//!
//! # Read-only diagnostic
//!
//! The repair search overrides individual tiers' vertex-level picks. `MeetConstraint` cannot
//! express that, so the result is only ever shown and never written back to a `ConstraintTier`.
//! This module never calls `History::apply` or `Design::apply_edit` and holds no reference to
//! the live `EditorState`, which keeps `History` the only mutator of `Design`.
//! [`DeepSolveOutcome::Completed`] carries the searched masts (`solved`) out for display only,
//! as a per-tier comparison with the design's last plain solve (see [`tier_mast_deltas`]).
//!
//! # Off the UI thread
//!
//! A run is about 68 pipeline runs per design, which takes minutes on a large design and would
//! freeze the UI thread. It runs on a [`super::solve_service::SolveService`] worker. Each run
//! gets its own service, because each run has its own `on_progress`/`on_done` closures.
//!
//! # `adjustable_anchors` is always empty
//!
//! Passing recorded-mast anchors as `adjustable_anchors` is wrong, and measurably weaker (24%
//! pass rate against 90% for the fixed-anchor mode used here). Every `ScaleReference` tier is
//! a recorded mast or an authored dimension, so [`super::solve_service::SolveKind::Verified`]
//! always passes `&[]`.
//!
//! # No progress fraction
//!
//! The search has no caller-visible loop, so [`DeepSolveProgress`] carries only elapsed time.
//! A fraction would be invented.
//!
//! # Cancellation is a mid-search checkpoint
//!
//! [`DeepSolveHandle::cancel`] sets the `SolveControl::with_cancel` flag that the search checks
//! inside each pipeline run. A cancel therefore takes effect within milliseconds, not when the
//! run finishes. A cancelled run's [`DeepSolveOutcome::Cancelled`] carries nothing; see
//! [`outcome_for`].

use super::solve_service::{
    SolveKind, SolveOutcome, SolveRequest, SolveResult, SolveService, VerifiedSolve,
};
use pleochroma::geometry::{
    meet_solver::{SolveError, SolvedTier, VerifiedSolveReport},
    stone_metrics::ExternalProportions,
};
use pleochroma_cut_core::Design;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// Handle to a running Deep Solve, returned by [`spawn_deep_solve`].
pub struct DeepSolveHandle {
    handle: super::solve_service::SolveHandle,
    /// Owns this run's [`SolveService`], so its worker lives while the handle can cancel it.
    /// Dropping it stops the worker (see `SolveService`'s `Drop`).
    _service: SolveService,
}

impl DeepSolveHandle {
    /// Requests cancellation; the running search stops at its next checkpoint.
    pub fn cancel(&self) {
        self.handle.cancel();
    }
}

/// One elapsed-time tick while a deep solve runs. It carries no completion fraction.
#[derive(Debug, Clone, Copy)]
pub struct DeepSolveProgress {
    pub elapsed: Duration,
}

/// What a deep solve produced, or that the UI gave up waiting on it.
///
/// `Completed` carries the [`VerifiedSolveReport`] to display and the [`SolvedTier`]s the search
/// settled on. Those masts are never applied to a `Design`. They feed the per-tier comparison
/// [`tier_mast_deltas`], which the deep-solve status line summarises with
/// [`format_tier_mast_deltas`]. A per-tier table would need a display slot on `EditorModel` and a
/// Slint model, so the status line keeps the one-line summary.
pub enum DeepSolveOutcome {
    Completed {
        /// The searched masts, in the same tier order as `Design::solve`. See
        /// [`tier_mast_deltas`].
        solved: Vec<SolvedTier>,
        /// The aggregate verdict to display, formatted by
        /// `pleochroma_editor::deep_solve::format_deep_solve_report`.
        report: VerifiedSolveReport,
        /// The plain solve of this design, computed on the worker when the caller had no valid
        /// cached one. `None` when the caller already had one, or when the plain solve failed.
        /// `apply_deep_solve_outcome` prefers its own cache and reads this only when that is
        /// missing.
        baseline: Option<Vec<SolvedTier>>,
    },
    /// The user cancelled before the search returned -- it may still be running in
    /// the background, but its eventual result is never delivered anywhere.
    Cancelled,
}

pub use pleochroma_editor::deep_solve::{TierMastDelta, format_tier_mast_deltas, tier_mast_deltas};

/// The printed proportions Deep Solve verified against, formatted for the status line.
///
/// Each field is optional, and only the ones the catalogue printed are listed, so a missing `C/W`
/// is omitted rather than shown as a placeholder. The result is empty only when every field is
/// `None`. The caller already requires `Some(targets)` to start a run, so that is defensive.
///
/// `source` is `EditorState::asc_filename` at launch: the printed figures come from that file.
/// `None` prints no source clause.
#[must_use]
pub fn format_verification_targets(targets: &ExternalProportions, source: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(v) = targets.vol_w3 {
        parts.push(format!("Vol/W3 {v:.4}"));
    }
    if let Some(v) = targets.lw {
        parts.push(format!("L/W {v:.4}"));
    }
    if let Some(v) = targets.cw {
        parts.push(format!("C/W {v:.4}"));
    }
    if let Some(v) = targets.pw {
        parts.push(format!("P/W {v:.4}"));
    }
    if let Some(v) = targets.hw {
        parts.push(format!("H/W {v:.4}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    let source_clause = source.map_or_else(String::new, |name| format!(" (from \"{name}\")"));
    format!(
        " Verified against the printed proportions{source_clause}: {}.",
        parts.join(", ")
    )
}

/// The printed targets describe the design as it was printed, not as it is after edits.
///
/// `edited_since_load` is the caller's `EditorState::history.can_undo()` at launch: a fresh
/// `History` is made on every load, so "can undo" means an edit has landed since load. It is
/// deliberately not `EditorState::is_dirty`, which saving clears while the targets still describe
/// the printed design.
#[must_use]
pub const fn edited_since_load_caveat(edited_since_load: bool) -> &'static str {
    if edited_since_load {
        " Note: this design has been edited since it was loaded -- these targets \
         may no longer describe the design you are holding."
    } else {
        ""
    }
}

/// Spawns a dedicated [`SolveService`] and submits one `Verified` request to it.
///
/// `on_progress` runs on the UI event loop at the service's ~10 Hz throttle. `on_done` runs exactly
/// once, with the final outcome.
///
/// `need_baseline` says whether the caller still lacks a valid plain solve of this design. When
/// it does, the worker computes one alongside the verified search, so the UI thread never runs a
/// synchronous `Design::solve`. See [`DeepSolveOutcome::Completed::baseline`].
pub fn spawn_deep_solve<T, P, D>(
    ui_weak: Weak<T>,
    design: Design,
    targets: ExternalProportions,
    need_baseline: bool,
    on_progress: P,
    on_done: D,
) -> DeepSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, DeepSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, DeepSolveOutcome) + Send + 'static,
{
    let start = Instant::now();
    // `SolveService::new` takes a `Fn`, but `on_done` is `FnOnce`. The `Mutex<Option<_>>`
    // lets the single completion `take()` it; a second call would do nothing rather than panic.
    let on_done = Arc::new(Mutex::new(Some(on_done)));
    let service = SolveService::new(
        ui_weak,
        move |ui: &T, report: super::solve_service::SolveProgressReport| {
            debug_assert_eq!(
                report.generation, 0,
                "this module always submits generation 0"
            );
            // Traces the pipeline phase and sweep: the only place this module sees the raw
            // `SolveProgress`.
            tracing::trace!(
                phase = ?report.progress.phase,
                sweep = report.progress.sweep,
                "deep solve progress"
            );
            on_progress(
                ui,
                DeepSolveProgress {
                    elapsed: start.elapsed(),
                },
            );
        },
        move |ui: &T, result: SolveResult| {
            debug_assert_eq!(
                result.generation, 0,
                "this module always submits generation 0"
            );
            tracing::info!(
                elapsed_ms = result.elapsed.as_millis(),
                "deep solve finished"
            );
            let SolveOutcome::Verified(verified) = result.outcome else {
                unreachable!("spawn_deep_solve only ever submits SolveKind::Verified");
            };
            let outcome = outcome_for(verified);
            let done = on_done
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(done) = done {
                done(ui, outcome);
            }
        },
    );
    // `adjustable_anchors` is always `&[]` (see the module doc).
    let handle = service.submit(SolveRequest {
        design: Arc::new(design),
        generation: 0,
        kind: SolveKind::Verified {
            targets,
            compute_baseline: need_baseline,
        },
    });
    debug_assert_eq!(
        handle.generation, 0,
        "this module always submits generation 0"
    );
    DeepSolveHandle {
        handle,
        _service: service,
    }
}

/// What a finished run becomes for the UI. A cancelled run is `Cancelled` whatever the search
/// returned, since the user has already been told the editor stopped waiting. Kept apart from
/// [`spawn_deep_solve`] so a test can call it directly.
fn outcome_for(result: Result<VerifiedSolve, SolveError>) -> DeepSolveOutcome {
    match result {
        Ok(VerifiedSolve {
            solved,
            report,
            baseline,
        }) => DeepSolveOutcome::Completed {
            solved,
            report,
            baseline,
        },
        // `TooManyPlanes` and `NonFiniteInput` cannot occur here: the ordinary Solve already
        // accepted this design (its plane count is within `MAX_PLANES`, no tier is NaN or
        // infinite), and the verified search adds no planes. They map to `Cancelled` rather
        // than a third variant.
        Err(
            SolveError::Cancelled
            | SolveError::TooManyPlanes { .. }
            | SolveError::NonFiniteInput { .. },
        ) => DeepSolveOutcome::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::SolveStrategy;

    fn dummy_report(accepted: bool) -> VerifiedSolveReport {
        VerifiedSolveReport {
            initial_score: 0.5,
            score_after_calibration: 0.3,
            final_score: 0.1,
            accepted,
            overrides_applied: 2,
            anchor_moves_applied: 1,
            pipeline_runs: 12,
        }
    }

    fn dummy_solved_tier(mast: f64) -> SolvedTier {
        SolvedTier {
            mast,
            strategy: SolveStrategy::DependencyOrder,
            detail: String::new(),
        }
    }

    // `DeepSolveHandle::cancel` delegates to `solve_service::SolveHandle::cancel` --
    // see `solve_service::tests::handle_cancel_sets_the_flag_a_worker_would_read`
    // for the underlying flag behavior. `DeepSolveHandle` itself can only be
    // constructed by `spawn_deep_solve` (its `handle`/`_service` fields require a
    // real `SolveService`), so there is nothing left to test bare here.

    // --- outcome_for: the cancellation decision ---

    #[test]
    fn outcome_for_reports_completed_with_the_report_when_ok() {
        let report = dummy_report(false);
        let solved = vec![dummy_solved_tier(1.0)];
        let outcome = outcome_for(Ok(VerifiedSolve {
            solved,
            report,
            baseline: None,
        }));
        match outcome {
            DeepSolveOutcome::Completed {
                solved: s,
                report: r,
                baseline,
            } => {
                assert_eq!(r.accepted, report.accepted);
                assert_eq!(r.pipeline_runs, report.pipeline_runs);
                assert_eq!(r.final_score, report.final_score);
                assert_eq!(s.len(), 1);
                assert!(baseline.is_none());
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    // The baseline computed on the background worker must reach `DeepSolveOutcome::Completed`
    // unchanged, so the completion handler can use it in place of a missing cache.
    #[test]
    fn outcome_for_carries_a_worker_computed_baseline_through_to_completed() {
        let report = dummy_report(true);
        let solved = vec![dummy_solved_tier(1.0)];
        let baseline = vec![dummy_solved_tier(0.9)];
        let outcome = outcome_for(Ok(VerifiedSolve {
            solved,
            report,
            baseline: Some(baseline.clone()),
        }));
        match outcome {
            DeepSolveOutcome::Completed {
                baseline: Some(b), ..
            } => {
                assert_eq!(b.len(), baseline.len());
                assert!((b[0].mast - baseline[0].mast).abs() < 1e-9);
            }
            DeepSolveOutcome::Completed { baseline: None, .. } => {
                panic!("expected the worker-computed baseline to survive");
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    #[test]
    fn outcome_for_reports_cancelled_on_a_cancelled_solve_error() {
        let outcome = outcome_for(Err(SolveError::Cancelled));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    #[test]
    fn outcome_for_reports_cancelled_on_the_unreachable_in_practice_too_many_planes_case() {
        // See `outcome_for`'s own doc comment for why this cannot happen from
        // `spawn_deep_solve`'s one call site, and why `Cancelled` (nothing to
        // show) is the honest fallback rather than a panic.
        let outcome = outcome_for(Err(SolveError::TooManyPlanes {
            planes: 999,
            max: 400,
        }));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    // --- tier_mast_deltas ---

    #[test]
    fn tier_mast_deltas_keeps_only_tiers_that_actually_moved() {
        let current = vec![
            dummy_solved_tier(1.0),
            dummy_solved_tier(2.0),
            dummy_solved_tier(3.0),
        ];
        let verified = vec![
            dummy_solved_tier(1.0),
            dummy_solved_tier(2.5),
            dummy_solved_tier(3.0),
        ];
        let deltas = tier_mast_deltas(&current, &verified);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].tier_index, 1);
        assert_eq!(deltas[0].before_mast, 2.0);
        assert_eq!(deltas[0].after_mast, 2.5);
        assert!((deltas[0].delta() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tier_mast_deltas_is_empty_when_every_tier_matches() {
        let masts = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        let deltas = tier_mast_deltas(&masts, &masts.clone());
        assert_eq!(
            deltas.len(),
            0,
            "identical solves must report no disagreement"
        );
    }

    #[test]
    fn tier_mast_deltas_stops_at_the_shorter_slice_instead_of_panicking() {
        let current = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        let verified = vec![dummy_solved_tier(9.0)];
        let deltas = tier_mast_deltas(&current, &verified);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].tier_index, 0);
    }

    // --- format_tier_mast_deltas ---

    #[test]
    fn format_tier_mast_deltas_is_empty_string_when_no_tiers_disagree() {
        assert_eq!(format_tier_mast_deltas(&[]), "");
    }

    #[test]
    fn format_tier_mast_deltas_names_the_tier_and_signed_movement() {
        let deltas = vec![TierMastDelta {
            tier_index: 3,
            before_mast: 1.0,
            after_mast: 1.25,
        }];
        let text = format_tier_mast_deltas(&deltas);
        assert!(text.contains("tier 3"));
        assert!(text.contains("+0.2500"));
    }

    // --- format_verification_targets ---

    #[test]
    fn format_verification_targets_names_only_the_fields_that_are_set() {
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            lw: None,
            cw: Some(0.1489),
            pw: None,
            hw: None,
        };
        let text = format_verification_targets(&targets, None);
        assert!(text.contains("Vol/W3 0.6013"));
        assert!(text.contains("C/W 0.1489"));
        assert!(!text.contains("L/W"));
        assert!(!text.contains("P/W"));
        assert!(!text.contains("H/W"));
    }

    #[test]
    fn format_verification_targets_is_empty_when_nothing_is_set() {
        assert_eq!(
            format_verification_targets(&ExternalProportions::default(), Some("RBC-445.asc")),
            "",
            "no source clause should print when there are no targets to report at all"
        );
    }

    #[test]
    fn format_verification_targets_names_the_source_when_given_one() {
        // The verdict must say where the printed figures it verified against came
        // from.
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            ..ExternalProportions::default()
        };
        let text = format_verification_targets(&targets, Some("RBC-445.asc"));
        assert!(text.contains("RBC-445.asc"), "got: {text}");
    }

    #[test]
    fn format_verification_targets_omits_the_source_clause_when_none() {
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            ..ExternalProportions::default()
        };
        let text = format_verification_targets(&targets, None);
        assert!(!text.contains("(from"), "got: {text}");
    }

    // --- edited_since_load_caveat ---

    #[test]
    fn edited_since_load_caveat_is_empty_when_not_edited() {
        assert_eq!(edited_since_load_caveat(false), "");
    }

    #[test]
    fn edited_since_load_caveat_warns_when_edited() {
        let text = edited_since_load_caveat(true);
        assert!(text.contains("edited since it was loaded"));
    }

    // --- DeepSolveProgress ---

    #[test]
    fn progress_carries_the_elapsed_duration_it_was_built_with() {
        let progress = DeepSolveProgress {
            elapsed: Duration::from_millis(1500),
        };
        assert_eq!(progress.elapsed, Duration::from_millis(1500));
    }

    #[test]
    fn progress_is_copy_not_just_clone() {
        // The ticker thread hands each tick a fresh `DeepSolveProgress` by value
        // across the `upgrade_in_event_loop` boundary -- `Copy` is load-bearing.
        let progress = DeepSolveProgress {
            elapsed: Duration::from_secs(3),
        };
        let copied = progress;
        assert_eq!(copied.elapsed, progress.elapsed);
    }
}
