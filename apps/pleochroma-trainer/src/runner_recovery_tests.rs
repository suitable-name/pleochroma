//! Recovery must retain the optimizer trajectory, replay masks and lagged rollout policy.

use super::*;
use crate::runner::{Algorithm, IterationReport, RunConfig, tests::fixture_runner_with};

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

fn round(runner: &mut TrainingRunner) -> IterationReport {
    runner
        .iterate(&mut || true)
        .expect("round succeeds")
        .expect("round completes")
}

/// Iteration, PPO loss bits and imitation step count and loss bits of an installed update.
type UpdateFigures = (usize, [u32; 4], Option<(usize, u32)>);

/// The timing-free part of an installed update: which iteration it trained and its losses.
fn update_figures(runner: &TrainingRunner) -> Option<UpdateFigures> {
    runner.installed_update.as_ref().map(|update| {
        let train = &update.train;
        (
            update.iteration,
            [
                train.policy_loss.to_bits(),
                train.value_loss.to_bits(),
                train.entropy.to_bits(),
                train.grad_norm.to_bits(),
            ],
            update
                .imitation
                .map(|metrics| (metrics.updates, metrics.loss.to_bits())),
        )
    })
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
    // The control never saves, so a save that perturbs the trajectory shows up as a difference.
    let mut uninterrupted = small_run(algo);
    let mut saved = small_run(algo);
    round(&mut uninterrupted);
    round(&mut saved);
    let directory = tempfile::tempdir().unwrap();
    saved.save_recovery(directory.path()).unwrap();
    let mut restored =
        TrainingRunner::new_for_recovery(saved.cfg.clone(), saved.split.clone()).unwrap();
    assert!(restored.iterate(&mut || true).is_err());
    restored.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&saved, &restored);
    assert_eq!(update_figures(&restored), update_figures(&saved));
    assert_eq!(
        restored.stats.update_of_iteration,
        saved.stats.update_of_iteration
    );
    assert_eq!(
        restored.updater.update_count().unwrap(),
        saved.updater.update_count().unwrap()
    );
    for _ in 0..2 {
        let control = round(&mut uninterrupted);
        let resumed = round(&mut restored);
        assert_eq!(resumed.update_of_iteration, control.update_of_iteration);
        assert_eq!(resumed.steps, control.steps);
        uninterrupted.sync_update().unwrap();
        restored.sync_update().unwrap();
        assert_same_learning_state(&uninterrupted, &restored);
        assert_eq!(update_figures(&restored), update_figures(&uninterrupted));
        assert_eq!(
            restored.stats.update_of_iteration,
            uninterrupted.stats.update_of_iteration
        );
        assert_eq!(
            restored.updater.update_count().unwrap(),
            uninterrupted.updater.update_count().unwrap()
        );
    }
}

#[test]
fn imitation_recovery_keeps_replay_adam_rng_and_policy_lag() {
    recovery_continues_algorithm(Algorithm::Imitation);
}

#[cfg(feature = "cuda")]
#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn restored_slots_prepare_accelerators_before_reporting_completion() {
    let runner = fixture_runner_with(|cfg| cfg.workers = 2);
    let states = runner
        .slots
        .iter()
        .map(|slot| SlotState::capture(slot).unwrap())
        .collect::<Vec<_>>();
    let expected = states
        .iter()
        .map(|state| rmp_serde::to_vec(state).unwrap())
        .collect::<Vec<_>>();
    let execution = EnvExecution::new_cuda(0).expect("CUDA execution service");
    let progress = TrainingProgress::default();
    progress.begin(1, 0);
    let restored = restore_slots(
        states,
        &runner.cfg,
        &runner.split,
        Some(&execution),
        &progress,
    )
    .expect("restored slots with accelerators");
    execution
        .check()
        .expect("accelerator preparation succeeded");

    let actual = restored
        .iter()
        .map(|slot| rmp_serde::to_vec(&SlotState::capture(slot).unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "attachment preserves saved slot state and order"
    );
    let environment = execution.snapshot();
    assert_eq!(
        environment.geometry.prepared_networks,
        restored.len() as u64
    );
    assert_eq!(
        environment.cuda_optical_requests, 0,
        "saved optics are reused"
    );
    let completed = progress.snapshot();
    assert_eq!(completed.completed_tasks, restored.len());
    assert_eq!(completed.total_tasks, restored.len());
    assert!(completed.oldest_active_task.is_none());
    assert!(
        completed
            .last_task_detail
            .as_deref()
            .is_some_and(|detail| detail.ends_with("(accelerator preparation)"))
    );
}

#[test]
fn old_rollout_scheduler_revision_migrates_without_discarding_exact_replay() {
    let mut first = small_run(Algorithm::Imitation);
    round(&mut first);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let path = directory.path().join("runner-state.mpk");
    let mut state: RunnerState = read_state(&path).unwrap();
    state.rollout_scheduler_revision = 0;
    write_state(&path, &state).unwrap();

    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    restored.restore_recovery(directory.path()).unwrap();
    assert_eq!(
        rmp_serde::to_vec(&restored.aggregate).unwrap(),
        rmp_serde::to_vec(&first.aggregate).unwrap()
    );
    assert_eq!(restored.rng, first.rng);
    assert!(restored.warnings.iter().any(|warning| {
        warning.contains("Recovery scheduler revision 0 migrated to 1")
            && warning.contains("replay were retained")
    }));
}

#[test]
fn old_imitation_objective_explicitly_migrates_and_retains_learning_state() {
    let mut first = small_run(Algorithm::Imitation);
    round(&mut first);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let path = directory.path().join("runner-state.mpk");
    let mut state: RunnerState = read_state(&path).unwrap();
    state.imitation_objective_revision = 1;
    write_state(&path, &state).unwrap();
    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    restored.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&first, &restored);
    assert!(restored.warnings.iter().any(|warning| {
        warning.contains("objective revision 1 migrated to 2")
            && warning.contains("optimizer moments and replay were retained")
            && warning.contains("not a bitwise continuation")
    }));
    // Saving the migrated run records the new objective, so the warning is not repeated on
    // every future resume (the retained warning remains part of the run's history).
    let migrated = tempfile::tempdir().unwrap();
    restored.save_recovery(migrated.path()).unwrap();
    let saved: RunnerState = read_state(&migrated.path().join("runner-state.mpk")).unwrap();
    assert_eq!(
        saved.imitation_objective_revision,
        IMITATION_OBJECTIVE_REVISION
    );
}

#[test]
fn changed_projection_revision_rebuilds_labels_without_restarting_training() {
    assert_changed_revision_rebuilds_labels(Some("older-projection-revision"));
}

#[test]
fn previous_cuda_revision_does_not_reuse_projection_labels() {
    assert_ne!(pleochroma_ai::env::CUDA_ENVIRONMENT_MATH, "cuda-env-v3");
    assert_changed_revision_rebuilds_labels(Some("cuda-env-v3"));
}

/// Asserts that both trainers of `runner` hold exactly `net`: the update thread's, which trains
/// next, and the rollout copy, which acts next.
fn assert_trainers_hold_net(runner: &TrainingRunner) {
    let (mut reference, _, _) = crate::runner::build_trainer(
        &runner.cfg.ppo,
        runner.cfg.tier,
        runner.split.max_tiers,
        DeviceChoice::Cpu,
        runner.cfg.seed,
    )
    .unwrap();
    reference.load_weights(&runner.net).unwrap();
    let expected = reference.model.export_to_set_attention().unwrap();
    assert_eq!(runner.updater.export_weights().unwrap(), expected);
    assert_eq!(
        runner.rollout_model.export_to_set_attention().unwrap(),
        expected
    );
}

/// Makes the next iteration of `runner` fail: its first observation no longer fits the layout.
fn break_first_slot(runner: &mut TrainingRunner) {
    let tier = runner.slots[0].obs.tiers[0].clone();
    runner.slots[0]
        .obs
        .tiers
        .resize(runner.split.max_tiers + 1, tier);
}

#[test]
fn a_failed_rollout_cannot_replace_a_committed_recovery() {
    let mut runner = small_run(Algorithm::Imitation);
    let committed = tempfile::tempdir().unwrap();
    runner.save_recovery(committed.path()).unwrap();
    let original = runner.slots[0].obs.clone();
    break_first_slot(&mut runner);

    assert!(runner.iterate(&mut || true).is_err());
    assert!(runner.needs_resync());
    // Training stays refused until the run is restored or re-synchronised.
    assert!(runner.iterate(&mut || true).is_err());
    let refused = tempfile::tempdir().unwrap();
    assert!(runner.save_recovery(refused.path()).is_err());
    assert_eq!(std::fs::read_dir(refused.path()).unwrap().count(), 0);

    runner.restore_recovery(committed.path()).unwrap();
    assert!(!runner.needs_resync());
    assert_eq!(runner.slots[0].obs, original);
    round(&mut runner);
}

#[test]
fn a_failed_iteration_is_trainable_again_after_resync_reset_or_weight_load() {
    let source = small_run(Algorithm::Imitation);
    let weights_dir = tempfile::tempdir().unwrap();
    let weights = weights_dir.path().join("weights.json");
    source.save_checkpoint(&weights).unwrap();

    for path in ["resync", "reset", "load weights"] {
        let mut runner = small_run(Algorithm::Imitation);
        round(&mut runner);
        let iteration = runner.stats.iteration;
        break_first_slot(&mut runner);
        assert!(runner.iterate(&mut || true).is_err(), "{path}");
        assert!(runner.needs_resync(), "{path}");
        match path {
            "resync" => runner.resync_after_failure().unwrap(),
            "reset" => runner.reset().unwrap(),
            _ => runner.load_checkpoint(&weights).unwrap(),
        }
        assert!(!runner.needs_resync(), "{path}");
        assert!(runner.updater.in_flight().is_none(), "{path}");
        assert_trainers_hold_net(&runner);
        if path == "resync" {
            assert_eq!(runner.stats.iteration, iteration, "statistics are kept");
        }
        round(&mut runner);
        let checkpoint = tempfile::tempdir().unwrap();
        runner.save_recovery(checkpoint.path()).unwrap();
    }
}

#[test]
fn resync_is_a_no_op_on_a_consistent_run() {
    let mut runner = small_run(Algorithm::Imitation);
    round(&mut runner);
    let rng = runner.rng.clone();
    runner.resync_after_failure().unwrap();
    assert_eq!(runner.rng, rng, "no environment was rebuilt");
    assert_eq!(
        runner.updater.in_flight(),
        Some(0),
        "the update in flight was left alone"
    );
}

#[test]
fn an_unwinding_rollout_cannot_be_checkpointed() {
    let mut runner = small_run(Algorithm::Imitation);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = runner.iterate(&mut || panic!("injected caller failure"));
    }));
    assert!(failure.is_err());
    assert!(!runner.recovery_safe);
    let refused = tempfile::tempdir().unwrap();
    assert!(runner.save_recovery(refused.path()).is_err());
    assert_eq!(std::fs::read_dir(refused.path()).unwrap().count(), 0);
}

#[test]
fn a_drained_cancellation_can_still_be_checkpointed() {
    let mut runner = small_run(Algorithm::Imitation);
    assert!(runner.iterate(&mut || false).unwrap().is_none());
    assert!(runner.recovery_safe);
    let directory = tempfile::tempdir().unwrap();
    runner.save_recovery(directory.path()).unwrap();
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
    let saved_labels = first.aggregate.len();
    assert!(saved_labels > 0, "the saved run carries replay labels");
    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    // Dropping the replay needs explicit permission; the refusal changes nothing.
    let refusal = restored
        .restore_recovery(directory.path())
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("--relabel"), "{refusal}");
    assert!(
        refusal.contains(&format!("{saved_labels} retained imitation labels")),
        "{refusal}"
    );
    assert!(restored.slots.is_empty() && !restored.needs_resync());

    restored.set_allow_environment_relabel(true);
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
    assert!(restored.warnings.iter().any(|warning| {
        warning.contains("relabelled")
            && warning.contains(&format!("dropped {saved_labels} retained imitation labels"))
    }));
}

#[test]
fn an_unchanged_environment_never_needs_relabel_permission() {
    let mut first = small_run(Algorithm::Imitation);
    round(&mut first);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    restored.restore_recovery(directory.path()).unwrap();
    assert_eq!(restored.aggregate.len(), first.aggregate.len());
    assert!(
        !restored
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
fn device_and_imitation_step_size_are_the_only_ignored_config_fields() {
    let mut runner = small_run(Algorithm::Imitation);
    let saved = config_identity(&runner);
    let semantic = recovery_identity::config_identity(&runner.cfg);
    let accepted = |cfg: &RunConfig| {
        matches_legacy_config(&saved, cfg)
            && semantic
                .changes(
                    &recovery_identity::config_identity(cfg),
                    recovery_identity::ADDED_CONFIG_FIELDS,
                )
                .is_empty()
    };
    runner.cfg.device = DeviceChoice::Cuda;
    runner.cfg.imitation.learning_rate *= 0.5;
    runner.cfg.imitation.max_grad_norm *= 4.0;
    assert!(accepted(&runner.cfg));
    let changes: [fn(&mut RunConfig); 3] = [
        |cfg| cfg.seed += 1,
        |cfg| cfg.ppo.learning_rate *= 0.5,
        |cfg| cfg.imitation.mini_batch_size += 1,
    ];
    for change in changes {
        let mut changed = runner.cfg.clone();
        change(&mut changed);
        assert!(!matches_legacy_config(&saved, &changed));
        assert!(!accepted(&changed));
    }
}

#[test]
fn a_bundle_whose_legacy_identities_no_longer_match_resumes_by_its_field_identities() {
    let mut first = small_run(Algorithm::Imitation);
    round(&mut first);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let path = directory.path().join("runner-state.mpk");
    let mut state: RunnerState = read_state(&path).unwrap();
    // What an older build wrote before a field was added or a skip reason was reworded.
    state.config = state
        .config
        .replacen("RunConfig {", "RunConfig { retired_field: 1,", 1);
    state.corpus.push_str(" (older layout)");
    write_state(&path, &state).unwrap();

    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    restored.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&first, &restored);

    // A real change is refused under both forms, and the message names the field.
    let mut changed =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    changed.cfg.label_every += 1;
    let error = changed
        .restore_recovery(directory.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("original run configuration"), "{error}");
    assert!(error.contains("label_every: 1 -> 2"), "{error}");
    let mut renamed =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    renamed.split.entries[0].title.push_str(" renamed");
    let error = renamed
        .restore_recovery(directory.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("unchanged dataset"), "{error}");
    assert!(error.contains("entries[0].title"), "{error}");
}

#[test]
fn restoring_keeps_the_new_process_warnings_beside_the_saved_ones() {
    let mut first = small_run(Algorithm::Imitation);
    first.warnings = vec!["saved warning".to_owned()];
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let mut restored =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    let fallback = "GPU unavailable, training on the CPU: no adapter".to_owned();
    restored.warnings = vec![fallback.clone(), "saved warning".to_owned()];
    restored.restore_recovery(directory.path()).unwrap();
    assert_eq!(
        restored.warnings,
        vec!["saved warning".to_owned(), fallback]
    );
}

#[test]
fn corpus_reporting_metadata_can_resume_but_changed_sampling_inputs_cannot() {
    let mut first = small_run(Algorithm::Imitation);
    let directory = tempfile::tempdir().unwrap();
    first.save_recovery(directory.path()).unwrap();
    let mut same =
        TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
    same.split.entries[0].physical_size_augmented = !same.split.entries[0].physical_size_augmented;
    same.restore_recovery(directory.path()).unwrap();
    assert_same_learning_state(&first, &same);

    for material_change in [false, true] {
        let mut changed =
            TrainingRunner::new_for_recovery(first.cfg.clone(), first.split.clone()).unwrap();
        let entry = &mut changed.split.entries[0];
        if material_change {
            entry.authored_material =
                Some(pleochroma_ai::env::design_source_material(&entry.design));
        } else {
            entry.design.girdle_diameter_mm =
                Some(entry.design.girdle_diameter_mm.unwrap_or(7.0) + 1.0);
        }
        let error = changed
            .restore_recovery(directory.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("unchanged dataset"));
        assert!(error.contains("--warm-start"));
        assert!(
            changed.slots.is_empty(),
            "incompatible inputs cannot restore saved episodes"
        );
    }
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
