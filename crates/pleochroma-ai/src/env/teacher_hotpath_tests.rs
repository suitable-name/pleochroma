//! Teacher labels must describe the same preserved candidate map as real steps.

use super::*;
use crate::env::test_fixtures::fixture_env;

#[test]
fn projected_identity_is_a_known_zero_response_without_rescoring() {
    let env = fixture_env();
    let core = &env.core;
    let tier = core.free_tiers()[0];
    // A nonzero input below representable angle precision still asks the candidate map
    // a valid question: its response is exactly the already-measured current design.
    let (sample, evaluated) = core.probe(tier, 1e-30);
    let sample = sample.expect("identity remains a feasible response");
    assert!(!evaluated);
    assert_eq!(sample.score, core.current_score());
    let sensitivity = core.sensitivity(&sample, 1e-30, true);
    assert_eq!(sensitivity.score, 0.0);
    assert!(
        sensitivity
            .components
            .iter()
            .all(|slope| slope.is_nan() || *slope == 0.0)
    );
}

#[test]
fn a_probe_smaller_than_the_step_limit_uses_the_same_candidate_as_a_step() {
    let env = fixture_env();
    let core = &env.core;
    let h = core.settings.max_step_deg / 5.0;
    for &tier in core.free_tiers() {
        for delta in [h, -h] {
            let mut deltas = vec![0.0; core.current_design.tiers.len()];
            deltas[tier] = delta;
            let actual_step = core.try_candidate(&ContinuousTierAction::new(deltas), 1.0);
            let (probe, evaluated) = core.probe(tier, delta);
            assert_eq!(evaluated, actual_step.evaluated);
            let expected = actual_step
                .scored
                .as_ref()
                .filter(|_| actual_step.girdle_gate && actual_step.manufacturing_gate);
            assert_eq!(
                probe.as_ref().map(|eval| eval.score),
                expected.map(|(_, eval)| eval.score)
            );
        }
    }
}
