//! The optimizer scores the stone WITH its frosted facets and frosted girdle, and an unfrosted
//! design is scored on exactly the polished stone, bit for bit as before.

use super::{
    super::{
        CANONICAL_LIGHTING_PRESET, ObjectiveFidelity, ObjectiveWeights, OptimizeConfig,
        ScoringSetup, SearchHooks, candidate, evaluate_objective_finished_under,
        evaluate_objective_under, objective, optimize_design, score_finished_design,
    },
    fixtures::rbc_445,
};
use crate::{
    design::{Design, TierFrosting},
    edit::Edit,
};
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::meet_solver::SolvedTier,
    optics::{materials::GemMaterial, raytracer::FacetFinish},
};

/// The first tier of RBC-445 is its pavilion main: a tier whose facets are real stone faces.
const PAVILION_MAIN: usize = 0;

/// RBC-445 with its pavilion mains frosted.
fn frosted_rbc() -> Design {
    let mut design = rbc_445();
    design
        .apply_edit(Edit::SetTierFrosting {
            index: PAVILION_MAIN,
            frosting: Some(TierFrosting::Tier),
        })
        .expect("the tier exists");
    assert!(design.has_frosting());
    design
}

fn solved_and_planes(design: &Design) -> (Vec<SolvedTier>, Vec<(glam::DVec3, f64)>) {
    let solved = design.solve().expect("RBC-445 must solve");
    let planes = design.planes_from_solved(&solved);
    (solved, planes)
}

fn bits(components: &objective::ObjectiveComponents) -> [u32; 3] {
    [
        components.windowing_pct.to_bits(),
        components.extinction_pct.to_bits(),
        components.tilt_brilliance_pct.to_bits(),
    ]
}

#[test]
fn an_unfrosted_design_has_no_finishes_and_scores_the_polished_stone_bit_for_bit() {
    let design = rbc_445();
    let material = GemMaterial::diamond();
    let (solved, planes) = solved_and_planes(&design);
    let gpu = objective::to_gpu_planes(&planes);
    assert!(
        design.facet_finishes_from_solved(&solved, &gpu).is_empty(),
        "an unfrosted design is the documented all-polished list"
    );

    let stone = candidate::ScoredStone::of(&design, &solved, &planes);
    assert!(stone.finished().finishes.is_empty());
    let (via_stone, _, _) = candidate::measure_with_tone(
        stone.finished(),
        &material,
        &ObjectiveWeights::default(),
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    let direct = evaluate_objective_under(
        &gpu,
        &material,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    assert_eq!(bits(&via_stone), bits(&direct));
}

#[test]
fn the_finishes_of_a_frosted_design_mark_exactly_the_frosted_planes() {
    let design = frosted_rbc();
    let (solved, planes) = solved_and_planes(&design);
    let gpu = objective::to_gpu_planes(&planes);
    let finishes = design.facet_finishes_from_solved(&solved, &gpu);
    assert_eq!(finishes.len(), gpu.len(), "one finish per plane");

    let frosted = design.frosted_planes_from_solved(&solved);
    assert!(!frosted.is_empty(), "the pavilion mains frost real planes");
    for (index, finish) in finishes.iter().enumerate() {
        let expected = if frosted.contains(&u32::try_from(index).expect("small")) {
            FacetFinish::Frosted
        } else {
            FacetFinish::Polished
        };
        assert_eq!(*finish, expected, "plane {index}");
    }
}

#[test]
fn the_frosted_girdle_flag_frosts_the_girdle_band_on_top_of_the_facets() {
    let mut design = rbc_445();
    let (solved, planes) = solved_and_planes(&design);
    let gpu = objective::to_gpu_planes(&planes);
    design.render_effects.girdle_frosted = true;
    let girdle_only = design.facet_finishes_from_solved(&solved, &gpu);
    assert!(girdle_only.contains(&FacetFinish::Frosted));
    assert!(girdle_only.contains(&FacetFinish::Polished));

    let mut both = frosted_rbc();
    both.render_effects.girdle_frosted = true;
    let with_both = both.facet_finishes_from_solved(&solved, &gpu);
    let frosted_count =
        |list: &[FacetFinish]| list.iter().filter(|f| **f == FacetFinish::Frosted).count();
    assert!(frosted_count(&with_both) > frosted_count(&girdle_only));
}

#[test]
fn a_frosted_design_is_scored_differently_from_the_polished_one_and_repeatably() {
    let polished = rbc_445();
    let frosted = frosted_rbc();
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let score = |design: &Design| {
        let (solved, planes) = solved_and_planes(design);
        score_finished_design(
            design,
            &solved,
            &planes,
            &material,
            ScoringSetup {
                weights: &weights,
                fidelity: ObjectiveFidelity::Fast,
                lighting: CANONICAL_LIGHTING_PRESET,
            },
            None,
        )
    };
    let polished_score = score(&polished);
    let frosted_score = score(&frosted);
    assert_ne!(
        bits(&polished_score.after),
        bits(&frosted_score.after),
        "frosted facets scatter the traced light"
    );
    assert_eq!(
        bits(&score(&frosted).after),
        bits(&frosted_score.after),
        "a frosted score repeats exactly"
    );
}

#[test]
fn the_design_score_equals_the_direct_finished_stone_call() {
    let design = frosted_rbc();
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let (solved, planes) = solved_and_planes(&design);
    let gpu = objective::to_gpu_planes(&planes);
    let finishes = design.facet_finishes_from_solved(&solved, &gpu);
    let tools = design.tools_for_scoring(&planes);
    let geom = pleochroma::geometry::StoneGeometry {
        planes: &gpu,
        tools: &tools,
    };
    let direct = evaluate_objective_finished_under(
        FinishedStone::new(geom, &finishes),
        &material,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    let scored = score_finished_design(
        &design,
        &solved,
        &planes,
        &material,
        ScoringSetup {
            weights: &weights,
            fidelity: ObjectiveFidelity::Fast,
            lighting: CANONICAL_LIGHTING_PRESET,
        },
        None,
    );
    assert_eq!(bits(&scored.after), bits(&direct));
}

#[test]
fn a_candidate_of_a_frosted_design_scores_the_frosted_stone() {
    let polished = rbc_445();
    let frosted = frosted_rbc();
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let score_of = |design: &Design| {
        let (solved, _) = solved_and_planes(design);
        let baseline = candidate::BaselineWarningCounts::count(
            &crate::manufacturability::check_manufacturability(
                design,
                &solved,
                crate::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
            ),
        );
        match candidate::evaluate_candidate(
            design,
            &material,
            &weights,
            &baseline,
            CANONICAL_LIGHTING_PRESET,
            None,
            None,
            candidate::preform_volume(design),
        ) {
            candidate::CandidateOutcome::Accepted { score, .. } => score,
            candidate::CandidateOutcome::Rejected => panic!("the fixture must be accepted"),
        }
    };
    let frosted_score = score_of(&frosted);
    assert_ne!(frosted_score.to_bits(), score_of(&polished).to_bits());
    assert_eq!(frosted_score.to_bits(), score_of(&frosted).to_bits());
}

/// A whole run on a frosted design: deterministic, never worse than its start, and the
/// design it hands back keeps its frosting. Two `Full` sweeps of a frosted stone are slow
/// unoptimised, so this is the same `--release --ignored` class as the polished acceptance gate.
#[test]
#[ignore = "frosted Full sweeps, see doc comment -- run with --release --ignored"]
fn an_optimizer_run_on_a_frosted_design_is_deterministic() {
    let design = frosted_rbc();
    let material = GemMaterial::diamond();
    let config = OptimizeConfig {
        seed: 42,
        max_evaluations: 12,
        ..OptimizeConfig::default()
    };
    let run = || {
        optimize_design(&design, &material, &config, &SearchHooks::default())
            .expect("the frosted design solves")
    };
    let first = run();
    let second = run();
    assert_eq!(first.before_score.to_bits(), second.before_score.to_bits());
    assert_eq!(first.after_score.to_bits(), second.after_score.to_bits());
    assert_eq!(first.evaluations, second.evaluations);
    assert!(first.after_score <= first.before_score + 1e-4);
}
