//! Scoring a result's states: a state is solved once, and a rival whose `Full` sweep a
//! cancellation stops scores nothing while the accepted end point is still finished.

use super::*;
use crate::{
    design::{ConstraintTier, ScheduleMeta},
    manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability},
    meet_preservation::MeetPreservationBaseline,
    optimize::{
        candidate::{BaselineWarningCounts, preform_volume},
        constrained::FeasibleSpace,
        objective::{CANONICAL_LIGHTING_PRESET, ObjectiveWeights, full_sweep_runs},
        options::OptimizeOptions,
        space::SearchSpace,
    },
    preform::PreformSpec,
};
use pleochroma::optics::materials::GemMaterial;
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// What `score_state` and `build_result` borrow: a baseline captured on the brilliant and the
/// fixed settings of a default run.
struct Scene {
    design: Design,
    preservation: Arc<MeetPreservationBaseline>,
    material: GemMaterial,
    weights: ObjectiveWeights,
    warnings: BaselineWarningCounts,
    space: SearchSpace,
}

impl Scene {
    fn new() -> Self {
        let design = brilliant();
        let options = OptimizeOptions::default();
        let preservation =
            Arc::new(MeetPreservationBaseline::capture(&design).expect("the brilliant captures"));
        let solved = design.solve().expect("the brilliant solves");
        let warnings = BaselineWarningCounts::count(&check_manufacturability(
            &design,
            &solved,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ));
        let mut space = SearchSpace::new(&options, BTreeMap::new());
        space.feasible = Some(FeasibleSpace {
            baseline: preservation.clone(),
            free: Vec::new(),
            masts: Vec::new(),
            policy: options.preservation_policy,
            turn_request: None,
        });
        Self {
            design,
            preservation,
            material: GemMaterial::diamond(),
            weights: ObjectiveWeights::default(),
            warnings,
            space,
        }
    }

    fn context(&self) -> SearchContext<'_> {
        SearchContext {
            cancel: None,
            material: &self.material,
            weights: &self.weights,
            baseline_warnings: &self.warnings,
            lighting: CANONICAL_LIGHTING_PRESET,
            space: &self.space,
            guard: None,
            shape_target: None,
            preform_volume: preform_volume(&self.design),
        }
    }

    /// A baseline whose score every state passes (nothing here is scored against it).
    fn baseline(&self) -> Baseline {
        Baseline {
            preservation: self.preservation.clone(),
            before: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            before_score: f32::MAX,
            before_score_fast: f32::MAX,
            before_yield_loss_pct: 0.0,
            tone_before: None,
            warnings: self.warnings,
            free: Vec::new(),
            anchors: BTreeMap::new(),
            guard: None,
        }
    }
}

#[test]
fn scoring_a_state_solves_it_once_and_a_cancelled_sweep_scores_nothing() {
    let scene = Scene::new();
    let ctx = scene.context();
    let cancel = AtomicBool::new(true);
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: None,
        on_start: None,
    };

    let solves = Design::solve_call_count();
    let sweeps = full_sweep_runs::count();
    let scored = score_state(&scene.design, &ctx, &hooks, 0, true);
    assert!(scored.is_none(), "a cancelled sweep scores nothing");
    assert_eq!(
        Design::solve_call_count() - solves,
        1,
        "the validating solve is the only solve (two before the reuse)"
    );
    assert_eq!(full_sweep_runs::count(), sweeps, "no sweep ran");
}

#[test]
fn a_rival_stopped_by_a_cancellation_is_dropped_while_the_end_point_is_still_scored() {
    let scene = Scene::new();
    let ctx = scene.context();
    let baseline = scene.baseline();
    let summary = SearchRunSummary {
        evaluations: 0,
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
        starts_run: 1,
        best_start: 0,
    };

    // The flag rises as the rival's scoring starts (its `FinalFull` report is the second):
    // the end point's own scoring has finished by then.
    let cancel = AtomicBool::new(false);
    let reports = Cell::new(0_usize);
    let on_progress = |_: usize, stage: SearchStage| {
        if stage == SearchStage::FinalFull {
            reports.set(reports.get() + 1);
            if reports.get() == 2 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    };
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: Some(&on_progress),
        on_start: None,
    };

    // The end point changes nothing, so it never becomes a candidate and the rival is tried.
    let rivals = [scene.design.clone()];
    let sweeps = full_sweep_runs::count();
    let result = build_result(
        &scene.design,
        &scene.design,
        &rivals,
        1,
        &ctx,
        &baseline,
        &hooks,
        &summary,
    );
    assert_eq!(
        full_sweep_runs::count() - sweeps,
        1,
        "exactly the end point's Full scoring ran"
    );
    assert_eq!(reports.get(), 2, "the rival's scoring did start");
    assert!(result.outcome.cancelled);
    assert!(result.candidates.is_empty());
}
