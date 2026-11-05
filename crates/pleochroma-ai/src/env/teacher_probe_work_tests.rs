use super::*;
use crate::env::{common::apply_action, test_fixtures::fixture_env};

fn assert_probe(actual: TierProbe, expected: TierProbe) {
    assert_eq!(actual.evaluations, expected.evaluations);
    assert_eq!(actual.best_action, expected.best_action);
    match (actual.sensitivity, expected.sensitivity) {
        (Some(actual), Some(expected)) => {
            assert_eq!(actual.forward_probe, expected.forward_probe);
            assert_eq!(actual.score.to_bits(), expected.score.to_bits());
            assert_eq!(
                actual.components.map(f32::to_bits),
                expected.components.map(f32::to_bits)
            );
        }
        (None, None) => {}
        pair => panic!("different sensitivity availability: {pair:?}"),
    }
}

#[test]
fn staged_probes_preserve_ordered_secants_winners_and_counts() {
    let env = fixture_env();
    let tiers = env.core.free_tiers().to_vec();
    for h in [0.25, 1e-30, 5.0] {
        let expected: Vec<_> = tiers.iter().map(|&tier| env.probe_tier(tier, h)).collect();
        let mut prepared: Vec<_> = tiers
            .iter()
            .enumerate()
            .map(|(index, &tier)| env.prepare_tier_probe_with(tier, h, index % 2 == 0))
            .collect();
        // Exercise readiness in reverse order; assembly still has its original identity.
        for work in prepared.iter_mut().rev() {
            assert!(work.poll());
        }
        for (work, expected) in prepared.into_iter().zip(expected) {
            assert_probe(env.complete_tier_probe(work), expected);
        }
    }
    for (tier, h) in [(usize::MAX, 0.25), (0, f64::NAN), (0, 0.0)] {
        let mut work = env.prepare_tier_probe(tier, h);
        assert!(work.poll());
        assert_probe(env.complete_tier_probe(work), env.probe_tier(tier, h));
    }
}

#[test]
fn pool_workers_share_both_probe_sides_without_changing_results() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("test pool");
    let env = fixture_env();
    let tiers = env.core.free_tiers().to_vec();
    let h = 0.25;
    let expected: Vec<_> = tiers.iter().map(|&tier| env.probe_tier(tier, h)).collect();
    // Unrequested sharing on a pool worker: the staged and the direct probe both join.
    let (staged, direct): (Vec<_>, Vec<_>) = pool.install(|| {
        tiers
            .iter()
            .map(|&tier| {
                let mut work = env.prepare_tier_probe_with(tier, h, false);
                assert!(work.poll());
                (env.complete_tier_probe(work), env.probe_tier(tier, h))
            })
            .unzip()
    });
    for ((staged, direct), expected) in staged.into_iter().zip(direct).zip(expected) {
        assert_probe(staged, expected);
        assert_probe(direct, expected);
    }
}

#[test]
fn candidate_completion_preserves_full_environment_measurements_and_gates() {
    let env = fixture_env();
    let core = &env.core;
    for magnitude in [0.0, -0.2, 0.2, f64::NAN] {
        let action = ContinuousTierAction::new(vec![magnitude; core.current_design.tiers.len()]);
        let expected = core.try_candidate(&action, 1.0);
        let mut prepared = core.prepare_candidate(&action, 1.0);
        assert!(prepared.poll());
        let actual = core.complete_candidate(prepared);
        assert_eq!(actual.rejection, expected.rejection);
        assert_eq!(actual.evaluated, expected.evaluated);
        assert_eq!(actual.unchanged, expected.unchanged);
        assert_eq!(actual.girdle_gate, expected.girdle_gate);
        assert_eq!(actual.manufacturing_gate, expected.manufacturing_gate);
        assert_eq!(
            actual.shape_penalty.to_bits(),
            expected.shape_penalty.to_bits()
        );
        match (actual.scored, expected.scored) {
            (Some((design, actual)), Some((expected_design, expected))) => {
                assert_eq!(design, expected_design);
                assert_eq!(actual.score.to_bits(), expected.score.to_bits());
                assert_eq!(
                    serde_json::to_value(actual).expect("actual measurements"),
                    serde_json::to_value(expected).expect("expected measurements")
                );
            }
            (None, None) => {}
            _ => panic!("different candidate score availability"),
        }
    }
}

#[test]
fn prepared_assembly_matches_direct_scoring_of_actual_projected_design() {
    let env = fixture_env();
    let core = &env.core;
    let tier = core.free_tiers()[0];
    let mut deltas = vec![0.0; core.current_design.tiers.len()];
    deltas[tier] = 0.1;
    let candidate = apply_action(
        &core.space,
        &core.current_design,
        &ContinuousTierAction::new(deltas),
        core.settings.max_step_deg,
    )
    .expect("projected design");
    let expected = core
        .evaluate_candidate_design(&candidate)
        .expect("reference score");
    let mut prepared = core
        .prepare_candidate_design(&candidate)
        .expect("prepared scoring");
    assert!(prepared.poll());
    let actual = prepared.complete().expect("completed scoring");
    assert_eq!(
        serde_json::to_value(actual).expect("actual measurements"),
        serde_json::to_value(expected).expect("reference measurements")
    );
}
