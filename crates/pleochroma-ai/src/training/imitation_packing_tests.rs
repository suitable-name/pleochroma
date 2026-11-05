use super::*;

// Packing

#[test]
fn label_rows_follow_the_masking_rules() {
    let fx = hand_built();
    let rows = label_rows(&fx.batch(), &[0, 1], 4);

    // Sample 0: tier 1 is not free (masked although finite), tier 2 lacks one component, tier 3
    // is padding. Sample 1: tier 0 has no label at all, tiers 2 and 3 are padding. A missing
    // label (NaN, including the three unmeasured components of every row) is a zero target
    // under a zero mask.
    let o = 0.0_f32;
    let zeros = [o; N_COMPONENTS];
    let labelled = |values: [f32; 4]| -> [f32; N_COMPONENTS] {
        let mut row = zeros;
        row[..4].copy_from_slice(&values);
        row
    };
    let want_target = [
        labelled([1.0, 2.0, 3.0, 4.0]).as_slice(),
        zeros.as_slice(),
        labelled([0.5, 0.0, -1.5, 2.5]).as_slice(),
        zeros.as_slice(),
        zeros.as_slice(),
        labelled([0.1, 0.2, 0.3, 0.4]).as_slice(),
        zeros.as_slice(),
        zeros.as_slice(),
    ]
    .concat();
    assert_eq!(rows.sens_target, want_target);
    let mut want_mask = vec![0.0; 2 * 4 * N_COMPONENTS];
    want_mask[..4].fill(1.0);
    want_mask[2 * N_COMPONENTS] = 1.0;
    want_mask[2 * N_COMPONENTS + 2..2 * N_COMPONENTS + 4].fill(1.0);
    want_mask[5 * N_COMPONENTS..5 * N_COMPONENTS + 4].fill(1.0);
    assert_eq!(rows.sens_mask, want_mask);

    // Verified action rows supervise every actionable coordinate, including zero-valued ones.
    let want_action = [
        1.0_f32.tanh(),
        o,
        (-0.5_f32).tanh(),
        o,
        o,
        (-1.0_f32).tanh(),
        o,
        o,
    ];
    assert_close(&rows.action_target, &want_action, 1e-5);
    assert_eq!(
        rows.action_mask,
        vec![1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0]
    );
    assert_eq!(rows.gain_target, vec![3.5, 0.0]);
    assert_eq!(rows.gain_mask, vec![1.0, 0.0]);
    assert!(rows.sample_has_labels.iter().any(|&has| has));
}

#[test]
fn label_rows_of_an_unlabelled_batch_report_no_labels() {
    let fx = Fixture::blank(4, MAX_TIERS);
    let rows = label_rows(&fx.batch(), &[0, 1, 2, 3], MAX_TIERS);
    assert!(!rows.sample_has_labels.iter().any(|&has| has));
    assert!(rows.sens_target.iter().all(|&v| v == 0.0));
    assert!(rows.action_target.iter().all(|&v| v == 0.0));
    assert!(rows.gain_target.iter().all(|&v| v == 0.0));
}

#[test]
fn infinite_labels_are_treated_as_missing() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.sensitivities[0][0] = component_row([f32::INFINITY, 1.0, f32::NEG_INFINITY, f32::NAN]);
    fx.score_slopes[0][0] = f32::INFINITY;
    fx.oracle_gain[0] = f32::INFINITY;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.sens_mask.iter().sum::<f32>(), 1.0);
    assert_eq!(rows.sens_target[1], 1.0);
    assert_eq!(rows.action_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(rows.gain_mask, vec![0.0]);
}

#[test]
fn mobility_masks_component_support_and_teacher_direction_labels() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.free_tiers[0].fill(true);
    fx.sensitivities[0][0] = component_row([1.0, 2.0, 3.0, 4.0]);
    fx.score_slopes[0][0] = 2.0;
    fx.states[0][GLOBAL_DIM + crate::env::MOBILITY_INDEX] = 0.0;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.sens_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(rows.action_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(
        count_component_support(&fx.batch(), &[0]),
        [0; N_COMPONENTS]
    );
}

#[test]
fn labelled_tiers_count_real_free_tiers_with_a_finite_component() {
    // Sample 0: tier 0 (all four) and tier 2 (three of four) count, tier 1 is not free.
    // Sample 1: tier 0 has no label, tier 1 counts.
    assert_eq!(count_labelled_tiers(&hand_built().batch(), &[0, 1]), 3);
    assert_eq!(
        count_labelled_tiers(&Fixture::blank(5, MAX_TIERS).batch(), &[0, 1, 2, 3, 4]),
        0
    );
}

#[test]
fn pack_imitation_uploads_states_masks_and_labels_in_index_order() {
    let fx = hand_built();
    let batch = fx.batch();
    let trainer = trainer(4, 0);
    let packed = trainer.pack_imitation(&batch, &[1, 0]);
    assert!(packed.row_has_labels.iter().any(|&has| has));
    // Three real tiers round up to the reusable four-tier bucket.
    let width = 4;
    assert_eq!(packed.tiers.dims(), [2, width, TIER_DIM]);
    assert_eq!(packed.globals.dims(), [2, GLOBAL_DIM]);

    let globals = host_vec(packed.globals).unwrap();
    assert_eq!(globals[..GLOBAL_DIM], fx.states[1][..GLOBAL_DIM]);
    assert_eq!(globals[GLOBAL_DIM..], fx.states[0][..GLOBAL_DIM]);
    let tiers = host_vec(packed.tiers).unwrap();
    for (row, &sample) in [1, 0].iter().enumerate() {
        let n = fx.n_tiers[sample];
        let start = row * width * TIER_DIM;
        assert_eq!(
            tiers[start..start + n * TIER_DIM],
            fx.states[sample][GLOBAL_DIM..GLOBAL_DIM + n * TIER_DIM]
        );
        assert!(
            tiers[start + n * TIER_DIM..start + width * TIER_DIM]
                .iter()
                .all(|&value| value == 0.0)
        );
    }
    // Sample 1 has 2 real tiers, sample 0 has 3.
    assert_eq!(
        host_vec(packed.mask).unwrap(),
        vec![1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0]
    );

    let rows = label_rows(&batch, &[1, 0], width);
    assert_eq!(packed.labels.sens_mask.dims(), [2, width, N_COMPONENTS]);
    assert_eq!(host_vec(packed.labels.sens_mask).unwrap(), rows.sens_mask);
    assert_eq!(
        host_vec(packed.labels.sens_target).unwrap(),
        rows.sens_target
    );
    assert_eq!(
        host_vec(packed.labels.action_mask).unwrap(),
        rows.action_mask
    );
    assert_eq!(
        host_vec(packed.labels.action_target).unwrap(),
        rows.action_target
    );
    assert_eq!(host_vec(packed.labels.gain_mask).unwrap(), rows.gain_mask);
    assert_eq!(
        host_vec(packed.labels.gain_target).unwrap(),
        rows.gain_target
    );
    // Only sample 0, now the second row, has a gain.
    assert_eq!(rows.gain_mask, vec![0.0, 1.0]);
}

#[test]
fn compact_replay_rows_pack_identically_to_padded_observations() {
    let mut fx = hand_built();
    let trainer = trainer(4, 0);
    let padded = trainer.pack_imitation(&fx.batch(), &[1, 0]);
    for (state, &real) in fx.states.iter_mut().zip(&fx.n_tiers) {
        state.truncate(GLOBAL_DIM + real * TIER_DIM);
    }
    assert!(fx.batch().validate(GLOBAL_DIM, TIER_DIM, 4).is_ok());
    let compact = trainer.pack_imitation(&fx.batch(), &[1, 0]);
    assert_eq!(padded.row_has_labels, compact.row_has_labels);
    assert_eq!(padded.tiers.dims(), compact.tiers.dims());
    assert_eq!(
        host_vec(padded.tiers).unwrap(),
        host_vec(compact.tiers).unwrap()
    );
    for (padded, compact) in [
        (padded.globals, compact.globals),
        (padded.mask, compact.mask),
        (padded.labels.action_target, compact.labels.action_target),
        (padded.labels.action_mask, compact.labels.action_mask),
        (padded.labels.gain_target, compact.labels.gain_target),
        (padded.labels.gain_mask, compact.labels.gain_mask),
    ] {
        assert_eq!(padded.dims(), compact.dims());
        assert_eq!(host_vec(padded).unwrap(), host_vec(compact).unwrap());
    }
    assert_eq!(
        host_vec(padded.labels.sens_target).unwrap(),
        host_vec(compact.labels.sens_target).unwrap()
    );
    assert_eq!(
        host_vec(padded.labels.sens_mask).unwrap(),
        host_vec(compact.labels.sens_mask).unwrap()
    );
}

#[test]
fn compact_rows_still_reject_missing_tokens_partial_padding_and_nonfinite_states() {
    for invalid_len in [GLOBAL_DIM + TIER_DIM, GLOBAL_DIM + 2 * TIER_DIM + 1] {
        let mut fx = hand_built();
        fx.states[1].truncate(invalid_len);
        assert!(matches!(
            fx.batch().validate(GLOBAL_DIM, TIER_DIM, 4),
            Err(TrainError::ShapeMismatch(ref message))
                if message.contains("sample 1") && message.contains("states")
        ));
    }
    let mut fx = hand_built();
    fx.states[1].truncate(GLOBAL_DIM + 2 * TIER_DIM);
    fx.states[1][GLOBAL_DIM] = f32::NAN;
    assert!(matches!(
        fx.batch().validate(GLOBAL_DIM, TIER_DIM, 4),
        Err(TrainError::NonFinite(ref message)) if message.contains("sample 1")
    ));
}

#[path = "imitation_loss_tests.rs"]
mod loss_tests;

#[test]
fn tier_buckets_are_bounded_and_keep_small_designs_small() {
    use crate::training::burn_model_host::tier_bucket;
    assert_eq!(
        (1..=9).map(|n| tier_bucket(n, 9)).collect::<Vec<_>>(),
        vec![1, 2, 4, 4, 8, 8, 8, 8, 9]
    );
    assert_eq!(tier_bucket(1, 64), 1);
}

#[test]
fn sentinel_padding_keeps_all_label_masks_zero_and_real_row_order() {
    let fx = hand_built();
    let trainer = trainer(4, 0);
    let prepared = trainer.pack_imitation_padded(&fx.batch(), &[1, 0], 4);
    let gathered = prepared.gather_rows(&[1, 0, 1], &trainer.device);
    assert_eq!(gathered.row_has_labels.len(), 4);
    assert!(!gathered.row_has_labels[3]);
    let globals = host_vec(gathered.globals).unwrap();
    assert_eq!(&globals[..GLOBAL_DIM], &fx.states[0][..GLOBAL_DIM]);
    assert_eq!(
        &globals[GLOBAL_DIM..2 * GLOBAL_DIM],
        &fx.states[1][..GLOBAL_DIM]
    );
    assert!(globals[3 * GLOBAL_DIM..].iter().all(|&v| v == 0.0));
    let mt = gathered.mask.dims()[1];
    assert!(
        host_vec(gathered.mask).unwrap()[3 * mt..]
            .iter()
            .all(|&v| v == 0.0)
    );
    assert!(
        host_vec(gathered.labels.sens_mask).unwrap()[3 * mt * N_COMPONENTS..]
            .iter()
            .all(|&v| v == 0.0)
    );
    assert!(
        host_vec(gathered.labels.action_mask).unwrap()[3 * mt..]
            .iter()
            .all(|&v| v == 0.0)
    );
    assert_eq!(host_vec(gathered.labels.gain_mask).unwrap()[3], 0.0);
}

#[test]
fn masked_dummy_rows_preserve_every_loss_and_finite_optimizer_step() {
    let fx = hand_built();
    let mut exact = trainer(4, 17);
    let mut padded = trainer(4, 17);
    let indices = [0, 1, 0];
    let plain = exact.pack_imitation(&fx.batch(), &indices);
    let bucket = padded.pack_imitation_padded(&fx.batch(), &indices, 4);
    let config = single_step(3);
    let a = exact
        .imitation_update(&plain, &[0, 1, 2], &config)
        .unwrap()
        .stepped()
        .unwrap();
    let b = padded
        .imitation_update(&bucket, &[0, 1, 2], &config)
        .unwrap()
        .stepped()
        .unwrap();
    for (a, b) in [
        (a.loss, b.loss),
        (a.sens_mse, b.sens_mse),
        (a.cosine, b.cosine),
        (a.action_loss, b.action_loss),
        (a.gain_loss, b.gain_loss),
        (a.grad_norm, b.grad_norm),
    ] {
        assert!(a.is_finite() && b.is_finite());
        assert!((a - b).abs() <= 1.0e-4 * a.abs().max(1.0), "{a} vs {b}");
    }
    assert_eq!(padded.update_count(), 1);
    assert_eq!(b.stages.max_minibatch_rows, 4);
}

#[test]
fn growing_small_replays_use_finite_small_row_buckets() {
    for count in [1usize, 2, 3, 4, 5, 7, 8, 9] {
        let fixture = Fixture::from_teacher(count, MAX_TIERS, 11);
        let mut trainer = trainer(MAX_TIERS, 5);
        let metrics = trainer
            .train_imitation_epochs(&fixture.batch(), &single_step(256))
            .unwrap();
        assert_eq!(metrics.updates, 1);
        assert_eq!(metrics.stages.max_minibatch_rows, count.next_power_of_two());
        assert_eq!(metrics.stages.max_prepared_rows, count.next_power_of_two());
        assert_eq!(metrics.stages.prepared_windows, 1);
        assert!(metrics.stages.max_minibatch_rows < count * 2 + 1);
    }
}

#[test]
fn prepared_padding_never_pushes_a_fitting_minibatch_over_the_payload_budget() {
    for (rows, dummy, capacity, expected) in [
        (600, true, 1000, 600),
        (512, true, 512, 512),
        (511, true, 512, 512),
        (500, false, 1000, 512),
        (1200, true, 1000, 1200),
    ] {
        assert_eq!(prepared_row_count(rows, dummy, capacity), expected);
        if rows <= capacity {
            assert!(prepared_row_count(rows, dummy, capacity) <= capacity);
        }
    }
}
