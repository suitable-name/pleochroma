//! Regression contracts for verified behavior targets, independent of derivative labels.

use super::*;
use crate::env::test_fixtures::{fixture_env, fixture_env_with};
use crate::model::GemstoneSetAttentionNetwork;

#[test]
fn a_symmetric_improvement_is_not_erased_by_its_zero_central_derivative() {
    // F(x) = 10 - x² has a zero central derivative at zero, yet either side improves.
    let derivative = super::super::nonuniform_secant(10.0, 9.0, 9.0, 1.0, 1.0);
    assert_eq!(derivative, 0.0);
    let mut best = None;
    for raw in [MEAN_BOUND, -MEAN_BOUND] {
        offer(
            &mut best,
            coordinate_winner(
                CoordinateProbe {
                    tier: 1,
                    raw,
                    gain: 1.0,
                },
                3,
            )
            .expect("measured coordinate"),
        );
    }
    let best = best.expect("a verified action survives the derivative cancellation");
    assert_eq!(best.raw, [0.0, MEAN_BOUND, 0.0]);
    assert_eq!(best.gain, 1.0);
    assert_eq!(direction_from_slopes(&[Some(derivative)]), [0.0]);
}

#[test]
fn a_worse_joint_direction_cannot_replace_an_improving_coordinate() {
    let coordinate = coordinate_winner(
        CoordinateProbe {
            tier: 0,
            raw: 0.5,
            gain: 0.25,
        },
        2,
    )
    .expect("coordinate");
    let mut best = Some(coordinate.clone());
    for gain in [-0.5, 0.0, f32::NAN, 0.1, 0.25] {
        offer(
            &mut best,
            Winner {
                raw: vec![0.5, 0.5],
                gain,
                scale: 1.0,
            },
        );
        assert_eq!(best, Some(coordinate.clone()));
    }
    offer(
        &mut best,
        Winner {
            raw: vec![0.5, 0.5],
            gain: 0.5,
            scale: 0.5,
        },
    );
    assert_eq!(best.expect("joint winner").gain, 0.5);
}

#[test]
fn the_teacher_measures_the_finite_endpoint_the_policy_can_replay() {
    let env = fixture_env();
    let step = env.config().max_step_deg;
    for &tier in env.movable_tier_indices() {
        for requested in [-step, -step * 0.5, step * 0.5, step] {
            let (raw, measured_delta) = env.core.canonical_probe(tier, requested);
            let mut target = vec![0.0; env.base_design().tiers.len()];
            target[tier] = raw;
            let replay = action_from_direction(&target, step);
            assert_eq!(replay.deltas_deg[tier].to_bits(), measured_delta.to_bits());
            let observation = env.build_observation();
            let mut projected = replay.clone();
            GemstoneSetAttentionNetwork::project_actions(
                &mut projected,
                &observation.tiers,
                observation.global.target_critical_angle_deg,
                step,
            );
            assert_eq!(projected, replay);
            assert!(raw.abs() <= MEAN_BOUND);
            assert!(measured_delta.abs() <= step * f64::from(MEAN_BOUND.tanh()));
        }
    }
    assert_eq!(raw_for_fraction(1.0), raw_for_fraction(0.95));
    assert_eq!(raw_for_fraction(f64::NAN), 0.0);
}

#[test]
fn tight_bounds_keep_canonical_teacher_actions_unchanged_by_policy_projection() {
    let design = pleochroma_cut_core::Design::concave_fixture();
    let tier = 2;
    let angle = design.tiers[tier].angle_deg;
    let env = fixture_env_with(|config| {
        config
            .angle_bounds
            .insert(tier, (angle - 0.000_013, angle + 0.000_017));
    });
    let observation = env.build_observation();
    let step = env.config().max_step_deg;
    for requested in [-step, step] {
        let (raw, delta) = env.core.canonical_probe(tier, requested);
        assert_ne!(raw, 0.0, "narrow bounds still contain represented movement");
        let mut target = vec![0.0; observation.tiers.len()];
        target[tier] = raw;
        let original = action_from_direction(&target, step);
        let mut projected = original.clone();
        GemstoneSetAttentionNetwork::project_actions(
            &mut projected,
            &observation.tiers,
            observation.global.target_critical_angle_deg,
            step,
        );
        assert_eq!(projected, original);
        assert_eq!(delta.to_bits(), original.deltas_deg[tier].to_bits());
        assert!((angle - 0.000_013..=angle + 0.000_017).contains(&(angle + delta)));
    }
}

#[test]
fn plateau_rescue_is_bounded_distinct_and_can_expand_beyond_the_probe_radius() {
    let env = fixture_env();
    let count = env.base_design().tiers.len();
    let probes = vec![TierProbe::default(); count];
    let h = env.config().max_step_deg * 0.25;
    let candidates = env.core.rescue_candidates(h, &probes, &[]);
    assert!(!candidates.is_empty());
    assert!(candidates.len() <= RESCUE_ATTEMPTS);
    assert!(candidates.iter().any(|candidate| {
        action_from_direction(&candidate.raw, env.config().max_step_deg)
            .deltas_deg
            .iter()
            .any(|delta| delta.abs() > h)
    }));
    for (index, candidate) in candidates.iter().enumerate() {
        assert!(
            !candidates[..index]
                .iter()
                .any(|old| old.raw == candidate.raw)
        );
        let moving = candidate.raw.iter().filter(|&&raw| raw != 0.0).count();
        assert!((1..=2).contains(&moving));
        for (tier, &raw) in candidate
            .raw
            .iter()
            .enumerate()
            .filter(|(_, raw)| **raw != 0.0)
        {
            assert!(env.movable_tier_indices().contains(&tier));
            assert!(env.core.policy_coordinate(tier).mobile);
            assert!(raw.is_finite() && raw.abs() <= MEAN_BOUND);
            if moving == 1 {
                assert_ne!(raw, env.core.canonical_probe(tier, h).0);
                assert_ne!(raw, env.core.canonical_probe(tier, -h).0);
            }
        }
    }
    assert!(
        candidates
            .iter()
            .any(|c| c.raw.iter().filter(|&&raw| raw != 0.0).count() == 2)
    );
}

#[test]
fn a_gradient_equal_to_a_coordinate_probe_is_not_scored_again() {
    let env = fixture_env();
    let mut slopes = vec![None; env.base_design().tiers.len()];
    slopes[env.movable_tier_indices()[0]] = Some(-1.0);
    let h = env.config().max_step_deg;
    assert!(env.core.gradient_actions(h, &slopes, &[1.0]).is_empty());
}

#[test]
fn paired_rescue_uses_the_inward_direction_at_upper_bounds() {
    let design = pleochroma_cut_core::Design::concave_fixture();
    let env = fixture_env_with(|config| {
        for tier in [1, 2] {
            let angle = design.tiers[tier].angle_deg;
            config.angle_bounds.insert(tier, (angle - 0.1, angle));
        }
    });
    let probes = vec![TierProbe::default(); design.tiers.len()];
    let candidates = env.core.rescue_candidates(0.25, &probes, &[]);
    assert!(candidates.len() <= RESCUE_ATTEMPTS);
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.raw[1] < 0.0 && candidate.raw[2] < 0.0)
    );
}

#[test]
fn malformed_probe_measurements_do_not_become_behavior_labels() {
    let env = fixture_env();
    let count = env.base_design().tiers.len();
    let tier = env.movable_tier_indices()[0];
    let mut cases = vec![vec![TierProbe::default(); count - 1]];
    for (raw, gain) in [(f32::NAN, 1.0), (0.5, f32::NAN), (0.0, 1.0)] {
        let mut probes = vec![TierProbe::default(); count];
        probes[tier].best_action = Some(CoordinateProbe { tier, raw, gain });
        cases.push(probes);
    }
    for probes in cases {
        let label = env.teacher_label_from_probes(0.25, &probes, &[1.0]);
        assert!(label.tiers.iter().all(Option::is_none));
        assert_eq!(label.action_target, None);
        assert_eq!(label.oracle_gain, None);
        assert_eq!(label.evaluations, 0);
    }
}

#[test]
fn rescue_omits_measured_gradient_actions_without_refilling_its_budget() {
    let env = fixture_env();
    let count = env.base_design().tiers.len();
    let h = env.config().max_step_deg;
    let probes = vec![TierProbe::default(); count];
    let mut slopes = vec![None; count];
    slopes[env.movable_tier_indices()[0]] = Some(-1.0);
    let measured = env.core.gradient_actions(h, &slopes, &[0.5, 0.25]);
    let original = env.core.rescue_candidates(h, &probes, &[]);
    let fresh = env.core.rescue_candidates(h, &probes, &measured);
    assert!(original.iter().any(|candidate| {
        measured.iter().any(|old| old.raw == candidate.raw)
    }));
    let expected: Vec<_> = original
        .into_iter()
        .filter(|candidate| !measured.iter().any(|old| old.raw == candidate.raw))
        .collect();
    assert_eq!(fresh, expected, "keep the original ordered candidate subset");
    assert!(fresh.len() < RESCUE_ATTEMPTS);
    for old in &measured {
        let replay = action_from_direction(&old.raw, h);
        assert!(fresh.iter().all(|candidate| {
            action_from_direction(&candidate.raw, h) != replay
        }));
    }
}

#[test]
fn rescue_keeps_the_same_winner_when_spare_pool_threads_are_used() {
    let env = fixture_env();
    let probes = vec![TierProbe::default(); env.base_design().tiers.len()];
    let h = env.config().max_step_deg;
    let mut serial = None;
    let mut parallel = None;
    let serial_evaluations = env
        .core
        .rescue_actions(h, &probes, &[], &|| false, &mut serial);
    let parallel_evaluations = env
        .core
        .rescue_actions(h, &probes, &[], &|| true, &mut parallel);
    assert_eq!(parallel_evaluations, serial_evaluations);
    assert_eq!(parallel, serial);
}
