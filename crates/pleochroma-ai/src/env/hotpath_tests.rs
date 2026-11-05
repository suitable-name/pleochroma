//! Regressions for valid identity proposals and adaptive search scheduling.

use super::*;
use crate::env::{teacher::DEFAULT_LINE_SEARCH, test_fixtures::fixture_env, traits::GemstoneRlEnv};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn unchanged_valid_stone_pays_step_cost_and_reports_its_known_score() {
    let mut env = fixture_env();
    let before = env.current_design().clone();
    let score = env.current_score();
    let action = ContinuousTierAction::new(vec![0.0; before.tiers.len()]);
    let step = env.step_scaled(&action, &DEFAULT_LINE_SEARCH);
    assert_eq!(step.info.rejection, Some(StepRejection::NotImproved));
    assert_eq!(step.info.evaluations, 0);
    assert!(step.info.unchanged);
    assert_eq!(step.info.candidate_score, Some(score));
    assert!(step.info.is_closed);
    assert!(step.info.manufacturability_ok);
    assert_eq!(step.reward, env.config().reward_config.step_cost);
    assert_eq!(env.current_design(), &before);
}

#[test]
fn failed_proposals_are_not_reported_as_preserved_identity_candidates() {
    let mut env = fixture_env();
    let step = env.step(&ContinuousTierAction::new(Vec::new()));
    assert!(!step.info.unchanged);
    assert!(!step.info.is_closed);
    assert_eq!(step.info.candidate_score, None);
}

#[test]
fn identity_reuses_the_score_but_still_applies_the_original_girdle_gate() {
    let mut env = fixture_env();
    env.core.base_eval.girdle_thickness = Some(-1.0);
    env.core.current_eval.girdle_thickness = Some(-1.0);
    let action = ContinuousTierAction::new(vec![0.0; env.base_design().tiers.len()]);
    let step = env.step(&action);
    assert_eq!(step.info.rejection, Some(StepRejection::GirdleViolation));
    assert!(step.info.unchanged);
    assert!(step.info.is_closed);
    assert_eq!(step.info.evaluations, 0);
}

#[test]
fn old_serialized_step_info_defaults_to_no_known_identity() {
    let mut value = serde_json::to_value(StepInfo::default()).expect("serialize step");
    value
        .as_object_mut()
        .expect("step object")
        .remove("unchanged");
    let restored: StepInfo = serde_json::from_value(value).expect("legacy step info");
    assert!(!restored.unchanged);
}

#[test]
fn unchanged_diagnostic_describes_the_first_proposal_when_no_scale_is_accepted() {
    let env = fixture_env();
    let zero = ContinuousTierAction::new(vec![0.0; env.base_design().tiers.len()]);
    let invalid = ContinuousTierAction::new(Vec::new());
    let search = fold_search(
        (1.0, env.core.try_candidate(&invalid, 1.0)),
        std::iter::once((0.5, env.core.try_candidate(&zero, 0.5))),
    );
    assert!(!search.outcome.unchanged);
    let search = fold_search(
        (1.0, env.core.try_candidate(&zero, 1.0)),
        std::iter::once((0.5, env.core.try_candidate(&invalid, 0.5))),
    );
    assert!(search.outcome.unchanged);
    assert_eq!(search.accepted_scale, None);
}

#[test]
fn adaptive_search_never_speculates_even_with_spare_pool_capacity() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("test pool");
    for spare in [true, false] {
        let mut sequential = fixture_env();
        let mut adaptive = fixture_env();
        let action = ContinuousTierAction::new(vec![0.0; sequential.base_design().tiers.len()]);
        let expected = sequential.step_scaled(&action, &DEFAULT_LINE_SEARCH);
        let checks = AtomicUsize::new(0);
        let actual = pool.install(|| {
            adaptive.step_scaled_adaptive(&action, &DEFAULT_LINE_SEARCH, &|| {
                checks.fetch_add(1, Ordering::Relaxed);
                spare
            })
        });
        assert_eq!(actual, expected);
        // Smaller scales run only after a rejection, never on spare capacity.
        assert_eq!(checks.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn adaptive_oracle_is_the_serial_search_without_consulting_capacity() {
    let env = fixture_env();
    let slopes = vec![Some(0.0); env.base_design().tiers.len()];
    let expected = env.oracle_outcome(&slopes, &DEFAULT_LINE_SEARCH);
    let checks = AtomicUsize::new(0);
    let actual = env.oracle_outcome_adaptive(&slopes, &DEFAULT_LINE_SEARCH, &|| {
        checks.fetch_add(1, Ordering::Relaxed);
        true
    });
    assert_eq!(actual, expected);
    assert_eq!(checks.load(Ordering::Relaxed), 0);
}
