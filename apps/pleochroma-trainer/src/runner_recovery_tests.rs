//! Recovery must retain the optimizer trajectory, replay masks and lagged rollout policy.

use super::*;
use crate::runner::{Algorithm, RunConfig, tests::fixture_runner_with};

fn small_run(algo: Algorithm) -> TrainingRunner {
    fixture_runner_with(|cfg| {
        cfg.algo = algo;
        cfg.workers = 1;
        cfg.rollout_size = 2;
        cfg.label_every = 1;
        cfg.imitation.mini_batch_size = 2;
        cfg.imitation.updates_per_round = 1;
        cfg.ppo.mini_batch_size = 2;
        cfg.ppo.epochs_per_batch = 1;
    })
}

fn round(runner: &mut TrainingRunner) {
    runner
        .iterate(&mut || true)
        .expect("round succeeds")
        .expect("round completes");
}

fn assert_same_learning_state(a: &TrainingRunner, b: &TrainingRunner) {
    assert_eq!(a.net, b.net);
    assert_eq!(a.rng, b.rng);
    assert_eq!(a.label_phase, b.label_phase);
    assert_eq!(a.stats.iteration, b.stats.iteration);
    assert_eq!(a.stats.total_steps, b.stats.total_steps);
    assert_eq!(a.stats.episodes, b.stats.episodes);
    assert_eq!(a.stats.diagnostics, b.stats.diagnostics);
    assert_eq!(
        rmp_serde::to_vec(&a.normalizer).unwrap(),
        rmp_serde::to_vec(&b.normalizer).unwrap()
    );
    assert_eq!(
        rmp_serde::to_vec(&a.aggregate).unwrap(),
        rmp_serde::to_vec(&b.aggregate).unwrap()
    );
    assert_eq!(
        a.rollout_model.export_to_set_attention().unwrap(),
        b.rollout_model.export_to_set_attention().unwrap()
    );
    for (left, right) in a.slots.iter().zip(&b.slots) {
        assert_eq!(left.obs, right.obs);
        assert_eq!(left.rejected_steps, right.rejected_steps);
        assert_eq!(left.seed, right.seed);
        assert_eq!(left.env.current_design(), right.env.current_design());
        let cached = |slot: &Slot| {
            rmp_serde::to_vec(
                &slot
                    .label_cache
                    .snapshot(slot.env.current_design())
                    .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(cached(left), cached(right));
    }
}

fn recovery_continues_algorithm(algo: Algorithm) {
    let mut uninterrupted = small_run(algo);
    round(&mut uninterrupted);
    let directory = tempfile::tempdir().unwrap();
    uninterrupted.save_recovery(directory.path()).unwrap();
    let mut restored =
        TrainingRunner::new_for_recovery(uninterrupted.cfg.clone(), uninterrupted.split.clone())
            .unwrap();
    assert!(restored.iterate(&mut || true).is_err());
    restored.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&uninterrupted, &restored);
    for _ in 0..2 {
        round(&mut uninterrupted);
        round(&mut restored);
        uninterrupted.sync_update().unwrap();
        restored.sync_update().unwrap();
        assert_same_learning_state(&uninterrupted, &restored);
    }
}

#[test]
fn imitation_recovery_keeps_replay_adam_rng_and_policy_lag() {
    recovery_continues_algorithm(Algorithm::Imitation);
}

#[test]
fn changed_projection_revision_rebuilds_labels_without_restarting_training() {
    assert_changed_revision_rebuilds_labels(Some("older-projection-revision"));
}

#[test]
fn a_legacy_snapshot_with_unknown_backend_rebuilds_labels_on_cpu() {
    assert_changed_revision_rebuilds_labels(None);
}

/// Reproduce the old wire format without converting NaN label masks through JSON.
struct LegacyRunnerState<'a>(&'a RunnerState);

impl Serialize for LegacyRunnerState<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        macro_rules! fields {
            ($($field:ident),+ $(,)?) => {{
                let mut map = serializer.serialize_map(Some(15))?;
                $(map.serialize_entry(stringify!($field), &self.0.$field)?;)+
                map.end()
            }};
        }
        fields!(
            version,
            obs_layout_version,
            config,
            corpus,
            network,
            rollout_network,
            installed_update,
            slots,
            label_phase,
            normalizer,
            rng,
            focus_entry,
            warnings,
            stats,
            oracle_cache,
        )
    }
}

fn assert_changed_revision_rebuilds_labels(revision: Option<&str>) {
    let mut first = small_run(Algorithm::Imitation);
    round(&mut first);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let path = directory.path().join("runner-state.mpk");
    let mut state: RunnerState = read_state(&path).unwrap();
    if let Some(revision) = revision {
        state.environment_math = revision.to_owned();
        write_state(&path, &state).unwrap();
    } else {
        write_state(&path, &LegacyRunnerState(&state)).unwrap();
        let decoded: RunnerState = read_state(&path).unwrap();
        assert_eq!(decoded.environment_math, "legacy-unknown");
    }
    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    restored.restore_recovery(directory.path()).unwrap();
    assert_eq!(restored.net, first.net);
    assert_eq!(restored.rng, first.rng);
    assert_eq!(restored.stats.iteration, first.stats.iteration);
    assert_eq!(restored.stats.total_steps, first.stats.total_steps);
    assert_eq!(
        restored.slots[0].env.current_design(),
        first.slots[0].env.current_design()
    );
    assert!(restored.aggregate.is_empty());
    assert!(restored.oracle_cache.is_none());
    assert!(
        restored.slots[0]
            .label_cache
            .snapshot(restored.slots[0].env.current_design())
            .unwrap()
            .is_none()
    );
    assert!(
        restored
            .warnings
            .iter()
            .any(|warning| warning.contains("relabelled"))
    );
}

#[test]
fn ppo_recovery_keeps_adam_rng_and_return_normalization() {
    recovery_continues_algorithm(Algorithm::Ppo);
}

#[test]
fn a_fresh_run_can_be_saved_before_the_first_optimizer_step() {
    let mut first = small_run(Algorithm::Imitation);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let mut second = small_run(Algorithm::Imitation);
    second.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&first, &second);
}

#[test]
fn changed_data_or_hyperparameters_are_refused_before_resume() {
    let mut first = small_run(Algorithm::Imitation);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let mut changed = small_run(Algorithm::Imitation);
    changed.cfg.label_every += 1;
    assert!(
        changed
            .restore_recovery(directory.path())
            .unwrap_err()
            .to_string()
            .contains("configuration")
    );
    changed.cfg = first.cfg.clone();
    changed.split.entries[0].title.push_str(" changed");
    assert!(
        changed
            .restore_recovery(directory.path())
            .unwrap_err()
            .to_string()
            .contains("dataset")
    );
}

#[test]
fn device_is_the_only_ignored_config_identity_field() {
    let mut runner = small_run(Algorithm::Imitation);
    let initial = config_identity(&runner);
    runner.cfg.device = DeviceChoice::Cuda;
    assert_eq!(config_identity(&runner), initial);
    runner.cfg.seed += 1;
    assert_ne!(config_identity(&runner), initial);
}

#[test]
fn warm_start_ignores_legacy_iteration_and_preserves_new_schedule() {
    let source = small_run(Algorithm::Ppo);
    let mut checkpoint = source.checkpoint();
    checkpoint.iteration = 999;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("weights.json");
    checkpoint.save(&path).unwrap();
    let mut target = small_run(Algorithm::Ppo);
    let original_config: RunConfig = target.cfg.clone();
    let original_rng = target.rng.clone();
    target.warm_start_checkpoint(&path).unwrap();
    assert_eq!(target.stats.iteration, 0);
    assert_eq!(target.rng, original_rng);
    assert_eq!(target.cfg.ppo.lr_schedule, original_config.ppo.lr_schedule);
    assert_eq!(target.net, source.net);
}
