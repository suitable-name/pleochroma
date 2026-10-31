//! The baseline scoring: the start design is solved twice in all (the capture and the
//! validating gate, whose solve it keeps), and a cancellation that arrives before the `Full`
//! sweep stops the baseline instead of paying for the sweep.

use super::*;
use crate::{
    design::{ConstraintTier, ScheduleMeta},
    optimize::objective::full_sweep_runs,
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::SolveError;
use std::sync::atomic::{AtomicBool, Ordering};

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// `baseline_report` of the brilliant with a cancellation that rises as the baseline's `Full`
/// scoring is announced (after the solves, before the sweep): its result, how many solves it
/// ran and how many `Full` sweeps it started.
fn cancelled_at_the_baseline_scoring() -> (Result<Baseline, DesignSolveError>, usize, usize) {
    let design = brilliant();
    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(false);
    let on_progress = |_: usize, stage: SearchStage| {
        if stage == SearchStage::BaselineFull {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: Some(&on_progress),
        on_start: None,
    };
    let solves = Design::solve_call_count();
    let sweeps = full_sweep_runs::count();
    let result = baseline_report(
        &design,
        &material,
        &OptimizeConfig::default(),
        &OptimizeOptions::default(),
        &hooks,
    );
    (
        result,
        Design::solve_call_count() - solves,
        full_sweep_runs::count() - sweeps,
    )
}

#[test]
fn the_baseline_solves_the_start_design_twice_not_three_times() {
    let (_, solves, _) = cancelled_at_the_baseline_scoring();
    assert_eq!(
        solves, 2,
        "the capture and the validating gate (three before the reuse)"
    );
}

#[test]
fn a_cancellation_before_the_baseline_sweep_is_a_cancelled_baseline_that_ran_no_sweep() {
    let (result, _, sweeps) = cancelled_at_the_baseline_scoring();
    assert!(matches!(
        result,
        Err(DesignSolveError::Solve(SolveError::Cancelled))
    ));
    assert_eq!(sweeps, 0, "the Full sweep never ran");
}
