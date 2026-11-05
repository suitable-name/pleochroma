use super::*;

#[test]
fn sensitivity_mse_is_the_masked_mean_over_labelled_entries() {
    let mut hat_values = vec![1.0; 2 * N_COMPONENTS];
    hat_values[N_COMPONENTS + 2] = 7.0;
    let mut target_values = vec![0.0; 2 * N_COMPONENTS];
    target_values[0] = 0.5;
    target_values[N_COMPONENTS + 2] = 4.0;
    let mut mask_values = vec![0.0; 2 * N_COMPONENTS];
    mask_values[0] = 1.0;
    mask_values[N_COMPONENTS + 2] = 1.0;
    let hat = cube(hat_values, [1, 2, N_COMPONENTS]);
    let target = cube(target_values, [1, 2, N_COMPONENTS]);
    let mask = cube(mask_values, [1, 2, N_COMPONENTS]);
    let mse = host_scalar(masked_sens_mse(hat.clone(), target.clone(), mask)).unwrap();
    assert!((mse - 4.625).abs() < 1e-6, "{mse}");

    let none = masked_sens_mse(
        hat,
        target,
        cube(vec![0.0; 2 * N_COMPONENTS], [1, 2, N_COMPONENTS]),
    );
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}

#[test]
fn cosine_averages_over_the_samples_that_have_a_direction() {
    let hat = matrix(vec![1.0, 0.0, 99.0, 5.0, 5.0, 5.0, 2.0, -2.0, 0.0], 3, 3);
    let target = matrix(vec![1.0, 1.0, 7.0, 1.0, 2.0, 3.0, 1.0, -1.0, 0.0], 3, 3);
    let mask = matrix(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0], 3, 3);
    let (cosine, loss) = masked_cosine(hat, target, mask);
    let want = f32::midpoint(FRAC_1_SQRT_2, 1.0);
    let (cosine, loss) = (host_scalar(cosine).unwrap(), host_scalar(loss).unwrap());
    assert!((cosine - want).abs() < 1e-5, "{cosine} vs {want}");
    assert!((loss - (1.0 - want)).abs() < 1e-5, "{loss}");
}

#[test]
fn cosine_of_opposite_directions_is_minus_one_and_an_empty_mask_is_exactly_zero() {
    let target = matrix(vec![1.0, 2.0], 1, 2);
    let opposite = matrix(vec![-1.0, -2.0], 1, 2);
    let both = matrix(vec![1.0; 2], 1, 2);
    let (cosine, loss) = masked_cosine(opposite.clone(), target.clone(), both);
    assert!((host_scalar(cosine).unwrap() + 1.0).abs() < 1e-5);
    assert!((host_scalar(loss).unwrap() - 2.0).abs() < 1e-5);
    let (cosine, loss) = masked_cosine(opposite, target, matrix(vec![0.0; 2], 1, 2));
    assert_eq!(host_scalar(cosine).unwrap(), 0.0);
    assert_eq!(host_scalar(loss).unwrap(), 0.0);
}

#[test]
fn huber_is_quadratic_below_the_knee_linear_above_and_masked() {
    let hat = matrix(vec![0.5, 3.0, 10.0, 7.0, -1.5], 5, 1);
    let target = matrix(vec![0.0, 0.5, 2.0, 0.0, 0.0], 5, 1);
    let mask = matrix(vec![1.0, 1.0, 0.0, 1.0, 1.0], 5, 1);
    let loss = host_scalar(masked_huber(hat, target, mask)).unwrap();
    assert!((loss - 2.406_25).abs() < 1e-5, "{loss}");
    let one = |v: f32| matrix(vec![v], 1, 1);
    let knee = masked_huber(one(1.0), one(0.0), one(1.0));
    assert!((host_scalar(knee).unwrap() - 0.5).abs() < 1e-6);
    let none = masked_huber(one(4.0), one(0.0), one(0.0));
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}

#[test]
fn adding_stationary_tiers_cannot_make_a_missed_sparse_move_cheap() {
    for width in [2, 8, 128] {
        let mut target = vec![0.0; width];
        target[0] = 0.8;
        let loss = balanced_action_huber(
            matrix(vec![0.0; width], 1, width),
            matrix(target, 1, width),
            matrix(vec![1.0; width], 1, width),
        );
        // Half the sample budget fits the actual move: 0.5 * H(0.8) = 0.16,
        // independently of how many correctly stationary tiers surround it.
        assert!((host_scalar(loss).unwrap() - 0.16).abs() < 1e-6);
    }
}

#[test]
fn stationary_drift_keeps_half_the_action_budget_and_padding_has_no_weight() {
    for width in [2, 8, 128] {
        let mut target = vec![0.0; width + 1];
        target[0] = 0.8;
        let mut predicted = vec![0.6; width + 1];
        predicted[0] = 0.8;
        predicted[width] = -100.0;
        let mut mask = vec![1.0; width + 1];
        mask[width] = 0.0;
        let loss = balanced_action_huber(
            matrix(predicted, 1, width + 1),
            matrix(target, 1, width + 1),
            matrix(mask, 1, width + 1),
        );
        assert!((host_scalar(loss).unwrap() - 0.09).abs() < 1e-6);
    }
}

#[test]
fn verified_samples_get_equal_action_weight_despite_different_mobile_tier_counts() {
    let loss = balanced_action_huber(
        matrix(vec![0.0; 12], 3, 4),
        matrix(
            vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 9.0, 9.0, 9.0, 9.0],
            3,
            4,
        ),
        matrix(
            vec![1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            3,
            4,
        ),
    );
    // The first sample loses 0.25, the second 0.125; the unlabelled third is omitted.
    assert!((host_scalar(loss).unwrap() - 0.1875).abs() < 1e-6);
    let empty = balanced_action_huber(
        matrix(vec![7.0; 4], 1, 4),
        matrix(vec![0.0; 4], 1, 4),
        matrix(vec![0.0; 4], 1, 4),
    );
    assert_eq!(host_scalar(empty).unwrap(), 0.0);
}

#[test]
fn bounded_direction_loss_matches_teacher_and_orders_partial_zero_and_opposite_moves() {
    let loss_at = |predicted: f32| {
        host_scalar(bounded_direction_loss(
            matrix(vec![predicted], 1, 1),
            matrix(vec![0.8], 1, 1),
            matrix(vec![1.0], 1, 1),
        ))
        .unwrap()
    };
    assert_eq!(loss_at(0.8), 0.0);
    assert!((loss_at(0.4) - 0.16).abs() < 1e-6);
    assert!((loss_at(0.0) - 0.64).abs() < 1e-6);
    assert!((loss_at(-0.8) - 2.0).abs() < 1e-6);
    // A useful gradient exists even at the collapsed policy, without the cosine singularity.
    let slope = (loss_at(0.001) - loss_at(-0.001)) / 0.002;
    assert!((slope + 1.6).abs() < 1e-3, "{slope}");
}

#[test]
fn tiny_verified_directions_do_not_become_unit_targets_or_large_gradients() {
    let loss_at = |predicted: f32| {
        host_scalar(bounded_direction_loss(
            matrix(vec![predicted], 1, 1),
            matrix(vec![0.0001], 1, 1),
            matrix(vec![1.0], 1, 1),
        ))
        .unwrap()
    };
    assert!((loss_at(0.0) - 1e-8).abs() < 1e-12);
    let slope = (loss_at(0.0001) - loss_at(-0.0001)) / 0.0002;
    assert!((slope + 0.0002).abs() < 1e-7, "{slope}");
    let loss = balanced_action_huber(
        matrix(vec![0.0, 0.0], 1, 2),
        matrix(vec![0.0001, 0.0], 1, 2),
        matrix(vec![1.0, 1.0], 1, 2),
    );
    assert!((host_scalar(loss).unwrap() - 2.5e-9).abs() < 1e-12);
}

#[test]
fn normalized_direction_branch_retains_action_scale_supervision() {
    let loss_at_scale = |scale: f32| {
        host_scalar(bounded_direction_loss(
            matrix(vec![0.8 * scale; 2], 1, 2),
            matrix(vec![0.8; 2], 1, 2),
            matrix(vec![1.0; 2], 1, 2),
        ))
        .unwrap()
    };
    // Even when the norm denominator exceeds one, scaling a direction is not a null mode.
    assert!(loss_at_scale(0.5) > loss_at_scale(0.75));
    assert_eq!(loss_at_scale(1.0), 0.0);
    assert!(loss_at_scale(1.1) > 0.0);
    let slope = (loss_at_scale(0.501) - loss_at_scale(0.499)) / 0.002;
    assert!((slope + 0.96).abs() < 1e-3, "{slope}");
}

#[test]
fn unsupported_direction_rows_and_masked_coordinates_have_no_vector_loss_weight() {
    let loss = bounded_direction_loss(
        matrix(vec![0.4, 90.0, 8.0, 8.0, 4.0, 4.0], 3, 2),
        matrix(vec![0.8, -90.0, 0.0, 0.0, 3.0, 3.0], 3, 2),
        matrix(vec![1.0, 0.0, 1.0, 1.0, 0.0, 0.0], 3, 2),
    );
    assert!((host_scalar(loss).unwrap() - 0.16).abs() < 1e-6);
    let empty = bounded_direction_loss(
        matrix(vec![7.0; 4], 1, 4),
        matrix(vec![0.0; 4], 1, 4),
        matrix(vec![1.0; 4], 1, 4),
    );
    assert_eq!(host_scalar(empty).unwrap(), 0.0);
}
