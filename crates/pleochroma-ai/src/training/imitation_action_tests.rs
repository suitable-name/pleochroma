//! Verified-action supervision must not hallucinate actions from a failed local gradient.

use super::*;

#[test]
fn failed_teacher_actions_never_fall_back_to_score_slopes() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.score_slopes[0].fill(-4.0);
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.action_mask, vec![0.0; MAX_TIERS]);
    assert_eq!(rows.sample_has_labels, vec![false]);
}

#[test]
fn action_targets_require_complete_finite_rows_or_explicit_absence() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.action_targets[0].fill(0.0);
    assert!(fx.batch().validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS).is_ok());
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        fx.action_targets[0][0] = invalid;
        let result = fx.batch().validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS);
        assert!(
            matches!(result, Err(TrainError::NonFinite(ref text)) if text.contains("action_targets")),
            "{result:?}"
        );
    }
    fx.action_targets[0].fill(f32::NAN);
    assert!(fx.batch().validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS).is_ok());
}

#[test]
fn verified_zero_actions_are_supervised_and_immobile_actions_are_masked() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.action_targets[0].fill(0.0);
    fx.action_targets[0][0] = 2.0;
    fx.states[0][GLOBAL_DIM + MOBILITY_INDEX] = 0.0;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.action_target, vec![0.0; MAX_TIERS]);
    assert_eq!(rows.action_mask, vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(rows.sample_has_labels, vec![true]);
}

#[test]
fn action_regression_penalizes_drift_on_verified_zero_coordinates() {
    let target = matrix(vec![0.8, 0.0, 0.0], 1, 3);
    let predicted = matrix(vec![0.8, 0.6, 0.9], 1, 3);
    let mask = matrix(vec![1.0, 1.0, 0.0], 1, 3);
    let loss = host_scalar(masked_huber(predicted, target, mask)).unwrap();
    assert!((loss - 0.09).abs() < 1e-6, "{loss}");
}

#[test]
fn verified_actions_train_even_when_all_component_probes_are_unavailable() {
    let mut fx = Fixture::blank(4, MAX_TIERS);
    for actions in &mut fx.action_targets {
        actions.fill(0.0);
        actions[0] = 0.75;
    }
    let mut student = trainer(MAX_TIERS, 1);
    let before = export(&student);
    let metrics = student
        .train_imitation_epochs(&fx.batch(), &single_step(4))
        .unwrap();
    assert_eq!(metrics.updates, 1);
    assert_eq!(metrics.labelled_tiers, 0);
    assert_eq!(metrics.component_support, [0; N_COMPONENTS]);
    assert_eq!(metrics.action_samples, 4);
    assert_eq!(metrics.sens_mse, 0.0);
    assert_eq!(metrics.gain_loss, 0.0);
    assert!(metrics.action_loss > 0.0 && all_finite(&metrics));
    let after = export(&student);
    assert_ne!(after.action_head, before.action_head);
}

#[test]
fn action_support_counts_selected_samples_with_mobile_labels_including_zeros() {
    let mut fx = Fixture::blank(5, MAX_TIERS);
    for row in &mut fx.action_targets[1..] {
        row.fill(0.0);
    }
    fx.free_tiers[2].fill(false);
    for k in 0..fx.n_tiers[3] {
        fx.states[3][GLOBAL_DIM + k * TIER_DIM + MOBILITY_INDEX] = 0.0;
    }
    assert_eq!(count_action_samples(&fx.batch(), &[0, 1, 2, 3, 4]), 2);
    assert_eq!(count_action_samples(&fx.batch(), &[0, 2, 3]), 0);
    assert_eq!(count_action_samples(&fx.batch(), &[1]), 1);
}

#[test]
fn repeated_optimizer_updates_count_action_samples_only_once() {
    let mut fx = Fixture::blank(3, MAX_TIERS);
    for row in &mut fx.action_targets {
        row.fill(0.0);
    }
    let mut student = trainer(MAX_TIERS, 1);
    let metrics = student
        .train_imitation_epochs(&fx.batch(), &bounded(3, 3))
        .unwrap();
    assert_eq!(metrics.updates, 3);
    assert_eq!(metrics.action_samples, 3);
}
