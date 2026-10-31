use pleochroma_ai::env::{TeacherLabel, TierSensitivity, flat_len};

use super::super::imitation::{LabelledSample, LabelledStates};
use super::*;

fn sample(marker: f32) -> LabelledSample {
    let mut state = vec![0.0; flat_len(4)];
    state[0] = marker;
    let tier = TierSensitivity {
        components: [marker; N_COMPONENTS],
        score: marker,
        forward_probe: true,
    };
    let label = TeacherLabel {
        tiers: vec![Some(tier), None],
        oracle_gain: Some(marker),
        oracle_scale: Some(1.0),
        action_target: Some(vec![marker / 100.0, 0.0]),
        evaluations: 0,
    };
    LabelledSample::new(state, vec![true, false], &label)
}

#[test]
fn a_snapshot_is_a_copy_the_aggregate_can_outgrow() {
    let mut aggregate = LabelledStates::new(10);
    aggregate.push(sample(1.0));
    aggregate.push(sample(2.0));
    let snapshot = OwnedImitationBatch::snapshot(&aggregate.batch(), 31, Some(10));
    assert_eq!(snapshot.as_batch().len(), 2);

    // The live aggregate moves on; the snapshot keeps the state of the moment it was taken.
    aggregate.push(sample(3.0));
    aggregate.clear();
    let view = snapshot.as_batch();
    assert_eq!(view.oracle_gain, [1.0, 2.0]);
    assert_eq!(view.n_tiers, [2, 2]);
    assert_eq!(view.states[1][0], 2.0);
    assert_eq!(view.action_targets, [vec![0.01, 0.0], vec![0.02, 0.0]]);
    view.validate(
        pleochroma_ai::env::GLOBAL_DIM,
        pleochroma_ai::env::TIER_DIM,
        4,
    )
    .expect("a snapshot of a valid aggregate is a valid batch");
}

/// Whether two snapshots hold the same rows, NaN included: an unlabelled tier is all NaN by
/// contract and `PartialEq` says `NaN != NaN`, so `assert_eq!` could never hold.
fn same_rows(a: &OwnedImitationBatch, b: &OwnedImitationBatch) -> bool {
    let (a, b) = (a.as_batch(), b.as_batch());
    let bits = |value: &f32| value.to_bits();
    let same_rows_of = |x: &[Vec<f32>], y: &[Vec<f32>]| {
        x.len() == y.len()
            && x.iter()
                .zip(y)
                .all(|(x, y)| x.iter().map(bits).eq(y.iter().map(bits)))
    };
    a.states == b.states
        && a.n_tiers == b.n_tiers
        && a.free_tiers == b.free_tiers
        && a.sensitivities.len() == b.sensitivities.len()
        && a.sensitivities.iter().zip(b.sensitivities).all(|(x, y)| {
            x.len() == y.len()
                && x.iter()
                    .flatten()
                    .map(bits)
                    .eq(y.iter().flatten().map(bits))
        })
        && same_rows_of(a.action_targets, b.action_targets)
        && same_rows_of(a.score_slopes, b.score_slopes)
        && a.oracle_gain
            .iter()
            .map(bits)
            .eq(b.oracle_gain.iter().map(bits))
}

#[test]
fn bounded_snapshot_is_seeded_uniform_subset_not_a_prefix() {
    let mut aggregate = LabelledStates::new(256);
    for marker in 0..200 {
        aggregate.push(sample(marker as f32));
    }
    let first = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x1234, Some(32));
    let repeat = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x1234, Some(32));
    let other = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x5678, Some(32));
    // The unlabelled second tier of every sample is NaN, so the rows are compared bitwise.
    assert!(same_rows(&first, &repeat));
    assert!(!same_rows(&first, &other));
    assert_eq!(first.as_batch().len(), 32);
    let selected: Vec<f32> = first.as_batch().oracle_gain.to_vec();
    assert!(selected.iter().any(|&i| i < 50.0));
    assert!(selected.iter().any(|&i| i >= 150.0));
}

#[test]
fn full_epoch_mode_keeps_the_entire_aggregate() {
    let mut aggregate = LabelledStates::new(100);
    for marker in 0..80 {
        aggregate.push(sample(marker as f32));
    }
    let snapshot = OwnedImitationBatch::snapshot(&aggregate.batch(), 7, None);
    assert_eq!(snapshot.as_batch().len(), 80);
}
