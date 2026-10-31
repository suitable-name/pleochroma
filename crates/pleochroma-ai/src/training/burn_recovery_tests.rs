//! Recovery checks exercise both algorithms on CPU; none require the design library.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;
use crate::{
    env::{GLOBAL_DIM, MOBILITY_INDEX, N_COMPONENTS, TIER_DIM, WEIGHT_FRACTION_INDEX},
    model::GemstoneSetAttentionNetwork,
    training::{ImitationBatch, ImitationConfig, LrSchedule, TrainBatch},
};

const TIERS: usize = 2;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pleochroma-ai-recovery-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn trainer(seed: u64) -> BurnTrainer {
    let shape = ModelShape {
        d_tier: TIER_DIM,
        d_global: GLOBAL_DIM,
        d_model: 8,
        num_heads: 2,
        num_layers: 1,
        max_tiers: TIERS,
    };
    let mut trainer = BurnTrainer::new_cpu(
        shape,
        PpoConfig {
            learning_rate: 0.001,
            mini_batch_size: 2,
            epochs_per_batch: 2,
            lr_schedule: LrSchedule::Linear { total_updates: 100 },
            ..PpoConfig::default()
        },
    )
    .unwrap()
    .with_seed(seed);
    let mut net = GemstoneSetAttentionNetwork::new(TIER_DIM, GLOBAL_DIM, 8, 2, 1, seed).unwrap();
    // Epsilons are not tensor parameters: restore must retain them separately.
    net.pre_attn_norms[0].eps = 0.002;
    net.post_attn_norms[0].eps = 0.003;
    net.final_norm.eps = 0.004;
    trainer.load_weights(&net).unwrap();
    trainer
}

fn states() -> Vec<Vec<f32>> {
    (0..4)
        .map(|row| {
            let mut rng = Prng::new(row);
            let mut state: Vec<_> = (0..GLOBAL_DIM + TIER_DIM * TIERS)
                .map(|_| rng.next_signed_f32())
                .collect();
            state[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS].fill(0.0);
            state[WEIGHT_FRACTION_INDEX] = 1.0;
            for tier in 0..TIERS {
                state[GLOBAL_DIM + tier * TIER_DIM + MOBILITY_INDEX] = 1.0;
            }
            state
        })
        .collect()
}

fn assert_same_state(left: &BurnTrainer, right: &BurnTrainer) {
    assert_eq!(
        left.model.export_to_set_attention().unwrap(),
        right.model.export_to_set_attention().unwrap()
    );
    assert_eq!(parameter_ids(&left.model), parameter_ids(&right.model));
    assert_eq!(left.config, right.config);
    assert_eq!(left.updates, right.updates);
    assert_eq!(left.shuffle, right.shuffle);
    assert_eq!(
        left.optimizer.to_record().len(),
        right.optimizer.to_record().len()
    );
}

#[test]
fn imitation_continues_the_identical_next_update_with_moments_and_shuffle() {
    let rows = states();
    let counts = vec![TIERS; rows.len()];
    let masks = vec![vec![true; TIERS]; rows.len()];
    let sensitivities = vec![vec![[0.2; N_COMPONENTS]; TIERS]; rows.len()];
    let slopes = vec![vec![0.2; TIERS]; rows.len()];
    let targets = vec![vec![-0.4; TIERS]; rows.len()];
    let gains = vec![0.25; rows.len()];
    let batch = ImitationBatch {
        states: &rows,
        n_tiers: &counts,
        free_tiers: &masks,
        sensitivities: &sensitivities,
        score_slopes: &slopes,
        action_targets: &targets,
        oracle_gain: &gains,
    };
    let config = ImitationConfig {
        updates_per_round: 3,
        mini_batch_size: 2,
        ..ImitationConfig::default()
    };
    let mut uninterrupted = trainer(31);
    uninterrupted
        .train_imitation_epochs(&batch, &config)
        .unwrap();
    let scratch = Scratch::new();
    uninterrupted.save_training_state(&scratch.0).unwrap();
    let mut restored = trainer(99);
    restored.restore_training_state(&scratch.0).unwrap();
    assert_same_state(&uninterrupted, &restored);
    uninterrupted
        .train_imitation_epochs(&batch, &config)
        .unwrap();
    restored.train_imitation_epochs(&batch, &config).unwrap();
    assert_same_state(&uninterrupted, &restored);
}

#[test]
fn ppo_continues_the_identical_next_update_and_learning_rate_position() {
    let rows = states();
    let actions = vec![vec![0.1; TIERS]; rows.len()];
    let old_log_probs = vec![0.0; rows.len()];
    let advantages = vec![-0.3, 0.7, 0.2, -0.6];
    let returns = vec![0.2, -0.4, 0.6, 0.3];
    let counts = vec![TIERS; rows.len()];
    let masks = vec![vec![true; TIERS]; rows.len()];
    let batch = TrainBatch {
        states: &rows,
        actions: &actions,
        old_log_probs: &old_log_probs,
        advantages: &advantages,
        returns: &returns,
        n_tiers: &counts,
        free_tiers: &masks,
    };
    let mut uninterrupted = trainer(17);
    uninterrupted.train_rollout_epochs(&batch).unwrap();
    let scratch = Scratch::new();
    uninterrupted.save_training_state(&scratch.0).unwrap();
    let mut restored = trainer(91);
    restored.restore_training_state(&scratch.0).unwrap();
    assert_same_state(&uninterrupted, &restored);
    uninterrupted.train_rollout_epochs(&batch).unwrap();
    restored.train_rollout_epochs(&batch).unwrap();
    assert_same_state(&uninterrupted, &restored);
}

#[test]
fn incomplete_or_incompatible_state_leaves_the_trainer_untouched() {
    let original = trainer(4);
    let scratch = Scratch::new();
    original.save_training_state(&scratch.0).unwrap();
    let mut restored = trainer(8);
    let before = restored.model.export_to_set_attention().unwrap();
    let mut state: TrainerState =
        serde_json::from_reader(File::open(scratch.0.join(META)).unwrap()).unwrap();
    state.version += 1;
    serde_json::to_writer(File::create(scratch.0.join(META)).unwrap(), &state).unwrap();
    assert!(restored.restore_training_state(&scratch.0).is_err());
    assert_eq!(before, restored.model.export_to_set_attention().unwrap());
    original.save_training_state(&scratch.0).unwrap();
    std::fs::remove_file(scratch.0.join(OPTIMIZER)).unwrap();
    assert!(restored.restore_training_state(&scratch.0).is_err());
    assert_eq!(before, restored.model.export_to_set_attention().unwrap());
    assert_eq!(restored.updates, 0);
}
