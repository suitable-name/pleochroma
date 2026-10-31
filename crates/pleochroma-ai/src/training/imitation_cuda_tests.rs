//! CUDA-specific imitation/export contract.

use super::*;

#[test]
#[ignore = "requires an NVIDIA CUDA device at hardware index 0"]
fn cuda_imitation_update_and_packed_export_match_pure_inference() {
    let mut trainer = BurnTrainer::new_cuda(shape(MAX_TIERS), PpoConfig::default()).unwrap();
    trainer.load_weights(&network(5)).unwrap();
    let before = export(&trainer);
    let fixture = Fixture::from_teacher(6, MAX_TIERS, 11);
    let config = ImitationConfig {
        mini_batch_size: 4,
        updates_per_round: 2,
        ..ImitationConfig::default()
    };
    let metrics = trainer
        .train_imitation_epochs(&fixture.batch(), &config)
        .unwrap();
    assert_eq!(metrics.updates, 2);
    assert_eq!(trainer.update_count(), 2);
    assert!(
        [
            metrics.loss,
            metrics.sens_mse,
            metrics.cosine,
            metrics.action_loss,
            metrics.gain_loss,
            metrics.grad_norm,
            metrics.packing_ms,
            metrics.update_ms,
        ]
        .into_iter()
        .all(f32::is_finite),
        "non-finite imitation metrics: {metrics:?}"
    );

    let exported = export(&trainer);
    assert_ne!(exported, before, "imitation updates did not change weights");
    let rollout = trainer
        .rollout_model()
        .forward_host(&fixture.states, &fixture.n_tiers)
        .unwrap();
    for (index, (state, &n_tiers)) in fixture.states.iter().zip(&fixture.n_tiers).enumerate() {
        let tokens: Vec<Vec<f32>> = state[GLOBAL_DIM..]
            .chunks_exact(TIER_DIM)
            .take(n_tiers)
            .map(<[f32]>::to_vec)
            .collect();
        let (policy, value) = exported.forward(&tokens, &state[..GLOBAL_DIM]).unwrap();
        for tier in 0..n_tiers {
            assert!(
                (policy.mean[tier] - rollout.means[index][tier]).abs() < 1e-4,
                "mean {index}/{tier}"
            );
            assert!(
                (policy.log_std[tier] - rollout.log_stds[index][tier]).abs() < 1e-4,
                "log std {index}/{tier}"
            );
        }
        assert!(
            (value - rollout.values[index]).abs() < 1e-4,
            "value {index}"
        );
    }
}
