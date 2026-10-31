//! A candidate the projection certified is solved once, not twice, and scores exactly what it
//! scored when the evaluation solved it itself; the mesh and the measures it is scored on come
//! from one arrangement and equal the separate calls' figures bit for bit.

use super::*;
use crate::{
    design::{ConstraintTier, ScheduleMeta},
    manufacturability::check_manufacturability,
    meet_preservation::MeetPreservationBaseline,
    optimize::constrained::FeasibleSpace,
    preform::PreformSpec,
};
use std::{collections::BTreeMap, sync::Arc};

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// Everything one evaluation borrows: a feasible space captured on the brilliant, and the fixed
/// settings of a default run.
struct Fixture {
    design: Design,
    material: GemMaterial,
    weights: ObjectiveWeights,
    warnings: BaselineWarningCounts,
    space: SearchSpace,
}

impl Fixture {
    fn new() -> Self {
        let design = brilliant();
        let options = OptimizeOptions::default();
        let baseline =
            Arc::new(MeetPreservationBaseline::capture(&design).expect("the brilliant captures"));
        let solved = design.solve().expect("the brilliant solves");
        let warnings = BaselineWarningCounts::count(&check_manufacturability(
            &design,
            &solved,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ));
        let mut space = SearchSpace::new(&options, BTreeMap::new());
        space.feasible = Some(FeasibleSpace {
            baseline,
            free: free_tier_indices(&design),
            masts: Vec::new(),
            policy: options.preservation_policy,
            turn_request: None,
        });
        Self {
            design,
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
            lighting: LightingPreset::IsoHemisphere,
            space: &self.space,
            guard: None,
            shape_target: None,
            preform_volume: preform_volume(&self.design),
        }
    }

    fn feasible(&self) -> &FeasibleSpace {
        self.space.feasible.as_ref().expect("a feasible space")
    }

    /// [`evaluate_candidate_controlled`] of `design` with the pre-solve `presolved`.
    fn evaluate(&self, design: &Design, presolved: Option<Vec<SolvedTier>>) -> CandidateOutcome {
        evaluate_candidate_controlled(
            design,
            &self.material,
            &self.weights,
            &self.warnings,
            LightingPreset::IsoHemisphere,
            None,
            None,
            preform_volume(design),
            presolved,
            None,
            None,
        )
    }
}

fn accepted_score(outcome: &CandidateOutcome) -> f32 {
    match outcome {
        CandidateOutcome::Accepted { score, .. } => *score,
        CandidateOutcome::Rejected => panic!("the brilliant's own candidate must be accepted"),
    }
}

#[test]
fn a_projected_candidate_is_not_solved_again() {
    let fixture = Fixture::new();
    let ctx = fixture.context();

    let before = Design::solve_call_count();
    let (projected, tiers) = fixture
        .feasible()
        .project_solved(&fixture.design, &fixture.space, None)
        .expect("the brilliant's own seed is feasible");
    let projection = Design::solve_call_count() - before;
    assert_eq!(projection, 1, "the accepting assessment solves once");
    assert_eq!(tiers.len(), projected.tiers.len());

    let before = Design::solve_call_count();
    let outcome = evaluate_in_context(&fixture.design, &ctx);
    let evaluation = Design::solve_call_count() - before;
    assert!(matches!(outcome, CandidateOutcome::Accepted { .. }));
    assert_eq!(
        evaluation, 1,
        "a certified candidate is solved by the projection alone (two solves before the reuse)"
    );
}

#[test]
fn the_reused_solve_scores_the_bits_a_fresh_solve_scored() {
    let fixture = Fixture::new();
    let ctx = fixture.context();
    let (projected, tiers) = fixture
        .feasible()
        .project_solved(&fixture.design, &fixture.space, None)
        .expect("the brilliant's own seed is feasible");

    let fresh = accepted_score(&fixture.evaluate(&projected, None));
    let reused = accepted_score(&fixture.evaluate(&projected, Some(tiers)));
    let in_context = accepted_score(&evaluate_in_context(&fixture.design, &ctx));
    assert_eq!(reused.to_bits(), fresh.to_bits());
    assert_eq!(in_context.to_bits(), fresh.to_bits());
}

#[test]
fn the_shared_arrangement_scores_what_the_separate_calls_scored() {
    let fixture = Fixture::new();
    let design = &fixture.design;
    let solved = design.solve().expect("the brilliant solves");
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(_) = build_solid_mesh(&planes) else {
        panic!("the brilliant closes");
    };
    let metrics = measure_solid(&planes);
    let stone = ScoredStone::of(design, &solved, &planes);
    let (components, _, tone_loss) = measure_with_tone(
        stone.finished(),
        &fixture.material,
        &fixture.weights,
        ObjectiveFidelity::Fast,
        LightingPreset::IsoHemisphere,
    );
    let expected = fixture.weights.score_with_tone(
        &components,
        yield_loss_pct_with_preform(
            preform_volume(design),
            stone.yield_metrics(&planes, metrics.as_ref()).as_ref(),
        ),
        tone_loss,
    );
    let got = accepted_score(&fixture.evaluate(design, None));
    assert_eq!(got.to_bits(), expected.to_bits());
}
