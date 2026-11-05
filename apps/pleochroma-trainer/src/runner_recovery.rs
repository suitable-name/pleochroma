//! Complete runner snapshots, committed by the CLI only after all payloads are durable.
//!
//! MessagePack retains NaN label masks and exact float bits. Replay is streamed separately
//! so a save does not duplicate its potentially large resident allocation. A recovery occurs
//! at an iteration boundary: work after that boundary is deliberately repeated after a crash.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
    sync::Arc,
};

use pleochroma_ai::{
    env::{EnvExecution, GemstoneObservation, OBS_LAYOUT_VERSION, recovery::EpisodeSnapshot},
    model::{Checkpoint, GemstoneSetAttentionNetwork, Prng},
    training::buffer::ReturnNormalizer,
    training::imitation::IMITATION_OBJECTIVE_REVISION,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    DatasetSplit, DeviceChoice, MaterialLabel, RunConfig, RunnerError, RunnerStats, TrainingRunner,
    draw_weights,
    envs::{EnvKind, Slot, build_env_from_episode, pick_target, sampled_source_material},
    eval::OracleCache,
    imitation::LabelledStates,
    label_cache::{CacheSnapshot, LabelCache},
    pool,
    progress::TrainingProgress,
    recovery_identity::{self, SemanticIdentity},
    update::InstalledUpdate,
};

const VERSION: u32 = 1;
const ROLLOUT_SCHEDULER_REVISION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SlotState {
    seed: u64,
    pos: usize,
    entry: usize,
    rejected_steps: usize,
    episode: EpisodeSnapshot,
    observation: GemstoneObservation,
    label_cache: Option<CacheSnapshot>,
}

impl SlotState {
    fn capture(slot: &Slot) -> Result<Self, RunnerError> {
        let episode = match &slot.env {
            EnvKind::Optimizer(env) => env.snapshot_episode()?,
            EnvKind::Retarget(env) => env.snapshot_episode()?,
        };
        Ok(Self {
            seed: slot.seed,
            pos: slot.pos,
            entry: slot.entry,
            rejected_steps: slot.rejected_steps,
            episode,
            observation: slot.obs.clone(),
            label_cache: slot.label_cache.snapshot(slot.env.current_design())?,
        })
    }

    fn restore(self, cfg: &RunConfig, split: &DatasetSplit) -> Result<Slot, RunnerError> {
        if !split.train.contains(&self.entry)
            || self.pos >= split.train.len()
            || (cfg.corpus_mode && split.train[self.pos] != self.entry)
        {
            return Err(RunnerError::Data(
                "recovery slot is outside its training split".into(),
            ));
        }
        let item = split
            .entries
            .get(self.entry)
            .ok_or_else(|| RunnerError::Data("recovery slot refers to a missing design".into()))?;
        let design = &item.design;
        let authored = item.authored_material.as_ref();
        let target = pick_target(cfg, self.seed, design, authored)?;
        let env = build_env_from_episode(
            cfg,
            design,
            target.as_ref(),
            draw_weights(cfg.objectives, self.seed),
            self.seed,
            authored,
            &self.episode,
        )?;
        // Observations are the exact input consumed by the next forward pass, not a fresh
        // re-measurement. Checking the recomputed structure catches incompatible code/data.
        if env.build_observation() != self.observation {
            return Err(RunnerError::Data(
                "recovery observation differs from its reconstructed episode".into(),
            ));
        }
        let label_cache = LabelCache::restore(self.label_cache, env.current_design())?;
        let (source_name, _, _) = sampled_source_material(self.seed, design, authored)?;
        let src = MaterialLabel {
            name: source_name,
            n_d: env.source_material().n_d,
        };
        let tgt = target.map_or_else(
            || src.clone(),
            |target| MaterialLabel {
                name: target.name,
                n_d: target.material.n_d,
            },
        );
        Ok(Slot {
            seed: self.seed,
            env,
            pos: self.pos,
            entry: self.entry,
            src,
            tgt,
            obs: self.observation,
            rejected_steps: self.rejected_steps,
            label_cache,
        })
    }
}

fn restore_slots(
    states: Vec<SlotState>,
    cfg: &RunConfig,
    split: &DatasetSplit,
    execution: Option<&Arc<EnvExecution>>,
    progress: &TrainingProgress,
) -> Result<Vec<Slot>, RunnerError> {
    let count = states.len();
    let costs = states
        .iter()
        .map(|state| {
            split
                .entries
                .get(state.entry)
                .map_or(1, |entry| entry.design.tiers.len() as u64)
        })
        .collect::<Vec<_>>();
    let mut pending = states.into_iter().map(Some).collect::<Vec<_>>();
    progress.phase("restore episode geometry", count);
    pool::run_items_ranked(
        &mut pending,
        &costs,
        pool::pass_threads(count),
        |index, state, _| {
            let state = state
                .take()
                .expect("each saved slot is restored exactly once");
            let task = progress.task(format!(
                "restore slot {}/{} design {} (saved optical scores)",
                index + 1,
                count,
                state.entry,
            ));
            let mut slot = state.restore(cfg, split)?;
            if let Some(execution) = execution {
                // Preparation stays in the bounded worker and inside this slot's task lifetime.
                task.update_detail(format!(
                    "restore slot {}/{} design {} (accelerator preparation)",
                    index + 1,
                    count,
                    slot.entry,
                ));
                slot.env.set_execution(Arc::clone(execution));
            }
            Ok(slot)
        },
    )
    .into_iter()
    .collect()
}

#[derive(Serialize, Deserialize)]
struct RunnerState {
    version: u32,
    #[serde(default)]
    rollout_scheduler_revision: u32,
    #[serde(default = "legacy_imitation_objective_revision")]
    imitation_objective_revision: u32,
    obs_layout_version: u32,
    #[serde(default = "legacy_environment_math")]
    environment_math: String,
    config: String,
    corpus: String,
    network: GemstoneSetAttentionNetwork,
    rollout_network: GemstoneSetAttentionNetwork,
    installed_update: Option<InstalledUpdate>,
    slots: Vec<SlotState>,
    label_phase: usize,
    normalizer: ReturnNormalizer,
    rng: Prng,
    focus_entry: usize,
    warnings: Vec<String>,
    stats: RunnerStats,
    oracle_cache: Option<OracleCache>,
    /// Field-level configuration identity; absent in bundles written before it existed.
    #[serde(default)]
    semantic_config: Option<SemanticIdentity>,
    /// Field-level corpus identity; absent in bundles written before it existed.
    #[serde(default)]
    semantic_corpus: Option<SemanticIdentity>,
}

/// The refusal of a changed corpus, before any field-level detail.
const CHANGED_CORPUS: &str = "recovery requires an unchanged dataset, including effective physical sizes and source-material sampling. Native files whose missing values are now augmented need a fresh run with a new output prefix; use --warm-start with the model checkpoint to retain learned weights without reusing incompatible episodes or replay";

/// The refusal of a changed configuration, before any field-level detail.
const CHANGED_CONFIG: &str = "recovery requires the original run configuration";

const fn legacy_imitation_objective_revision() -> u32 {
    1
}

fn legacy_environment_math() -> String {
    // Old snapshots did not record the environment backend. Do not assume their
    // labels were produced by CPU arithmetic when resuming on a CPU device.
    "legacy-unknown".to_owned()
}

fn error(context: &str, error: impl std::fmt::Display) -> RunnerError {
    RunnerError::Data(format!("{context}: {error}"))
}

fn write_state(path: &Path, value: &impl Serialize) -> Result<(), RunnerError> {
    let mut output =
        BufWriter::new(File::create(path).map_err(|e| error("create recovery payload", e))?);
    value
        .serialize(&mut rmp_serde::Serializer::new(&mut output).with_struct_map())
        .map_err(|e| error("serialize recovery payload", e))?;
    output
        .flush()
        .map_err(|e| error("flush recovery payload", e))?;
    output
        .get_ref()
        .sync_all()
        .map_err(|e| error("sync recovery payload", e))
}

fn read_state<T: DeserializeOwned>(path: &Path) -> Result<T, RunnerError> {
    let file = File::open(path).map_err(|e| error("open recovery payload", e))?;
    rmp_serde::from_read(BufReader::new(file)).map_err(|e| error("decode recovery payload", e))
}

/// The legacy `Debug` identity a bundle records. The device is normalised: a resume may move to
/// other hardware.
fn config_identity(runner: &TrainingRunner) -> String {
    let mut config = runner.cfg.clone();
    config.device = DeviceChoice::Auto;
    format!("{config:?}")
}

/// The legacy comparison with the explicit resume overrides ignored: besides the device, the
/// imitation learning rate and gradient-norm cap may differ, so the current configuration is
/// rendered with the values the saved identity recorded for those two before comparing.
fn matches_legacy_config(saved: &str, cfg: &RunConfig) -> bool {
    let mut config = cfg.clone();
    config.device = DeviceChoice::Auto;
    let (learning_rate, max_grad_norm) = recovery_identity::legacy_imitation_step_size(saved);
    if let Some(rate) = learning_rate {
        config.imitation.learning_rate = rate;
    }
    if let Some(norm) = max_grad_norm {
        config.imitation.max_grad_norm = norm;
    }
    matches_config_identity(saved, &format!("{config:?}"), cfg.label_memory_mib)
}

/// Accepts a saved identity when the legacy form matches or the field-level form has no
/// changes; otherwise the refusal names the changed fields when the bundle recorded them.
fn check_identity(
    legacy_matches: bool,
    saved: Option<&SemanticIdentity>,
    current: impl FnOnce() -> SemanticIdentity,
    added: &[(&str, &str)],
    refusal: &str,
) -> Result<(), RunnerError> {
    if legacy_matches {
        return Ok(());
    }
    let Some(saved) = saved else {
        return Err(RunnerError::Data(refusal.to_owned()));
    };
    let changes = saved.changes(&current(), added);
    if changes.is_empty() {
        return Ok(());
    }
    Err(RunnerError::Data(format!(
        "{refusal}. Changed: {}",
        recovery_identity::describe_changes(&changes)
    )))
}

fn matches_config_identity(saved: &str, current: &str, label_memory_mib: usize) -> bool {
    if saved == current {
        return true;
    }
    // Before the host budget existed, its effective value was always 512 MiB.
    // Remove precisely that one field from the current identity; all other fields
    // remain part of the exact comparison and non-default budgets cannot migrate.
    let field = ", label_memory_mib: 512";
    label_memory_mib == 512
        && current.contains(field)
        && !saved.contains(", label_memory_mib:")
        && current.replacen(field, "", 1) == saved
}

impl TrainingRunner {
    /// Saves a complete, quiescent iteration boundary in a fresh unpublished directory.
    ///
    /// The caller must publish the directory transactionally after every file is durable.
    /// This includes Adam/RNG, retained labels, running episodes and the lagged rollout model.
    ///
    /// # Errors
    /// Returns an error on synchronization, export or IO failure; the bundle must not be published.
    pub fn save_recovery(&mut self, directory: &Path) -> Result<(), RunnerError> {
        if !self.recovery_safe {
            return Err(RunnerError::Data(
                "cannot checkpoint a failed iteration or recovery install; keep the last committed recovery"
                    .into(),
            ));
        }
        // A fatal device error can interrupt a rollout after environment mutation but
        // before its observation is committed. Keep the previous durable generation.
        if let Some(execution) = &self.execution {
            execution.check().map_err(RunnerError::Data)?;
        }
        if self.slots.is_empty() {
            return Err(RunnerError::Data(
                "cannot save before recovery restores the episodes".into(),
            ));
        }
        self.sync_update()?;
        let state = RunnerState {
            version: VERSION,
            rollout_scheduler_revision: ROLLOUT_SCHEDULER_REVISION,
            imitation_objective_revision: IMITATION_OBJECTIVE_REVISION,
            obs_layout_version: OBS_LAYOUT_VERSION,
            environment_math: self.environment_math().to_owned(),
            config: config_identity(self),
            corpus: self.split.recovery_identity(),
            network: self.net.clone(),
            rollout_network: self.rollout_model.export_to_set_attention()?,
            installed_update: self.installed_update.clone(),
            slots: self
                .slots
                .iter()
                .map(SlotState::capture)
                .collect::<Result<_, _>>()?,
            label_phase: self.label_phase,
            normalizer: self.normalizer.clone(),
            rng: self.rng.clone(),
            focus_entry: self.focus_entry,
            warnings: self.warnings.clone(),
            stats: self.stats.clone(),
            oracle_cache: self.oracle_cache.clone(),
            semantic_config: Some(recovery_identity::config_identity(&self.cfg)),
            semantic_corpus: Some(recovery_identity::corpus_identity(&self.split)),
        };
        self.updater.save_training_state(directory)?;
        write_state(&directory.join("runner-state.mpk"), &state)?;
        write_state(&directory.join("replay.mpk"), &self.aggregate)
    }

    /// Lets [`Self::restore_recovery`] continue a run whose environment arithmetic changed (a
    /// resume on another device, or a new CUDA math revision): its retained labels are then
    /// dropped and relabelled, with a warning naming how many. Off by default, so such a resume
    /// is refused rather than silently training the rest of the run on a handful of fresh
    /// labels while the optimizer step count and learning-rate schedule carry on.
    pub const fn set_allow_environment_relabel(&mut self, allow: bool) {
        self.allow_environment_relabel = allow;
    }

    /// Restores a saved iteration boundary with its original corpus and settings.
    ///
    /// Device selection may differ, and the imitation learning rate and gradient-norm cap may
    /// be overridden explicitly. Numerical identity across different hardware/backends is not
    /// promised. Optimizer, episode and schedule state are retained. Labels made with other
    /// environment arithmetic project actions differently: a bundle that carries any is refused
    /// unless [`Self::set_allow_environment_relabel`] allowed dropping them.
    ///
    /// The configuration and the corpus must match either the bundle's legacy identities or its
    /// field-level ones; a refusal names the differing fields when the bundle recorded them.
    ///
    /// # Errors
    /// Refuses changed data/configuration, a change of environment arithmetic that would drop
    /// labels without permission, incompatible state, or an unavailable backend.
    pub fn restore_recovery(&mut self, directory: &Path) -> Result<(), RunnerError> {
        self.progress.phase("decode recovery state", 1);
        let state: RunnerState = {
            let _task = self.progress.task("read runner-state.mpk".into());
            read_state(&directory.join("runner-state.mpk"))?
        };
        self.progress.phase("validate recovery inputs", 1);
        let validation = self
            .progress
            .task("compare original dataset, configuration and models".into());
        if state.version != VERSION || state.obs_layout_version != OBS_LAYOUT_VERSION {
            return Err(RunnerError::Data(
                "unsupported runner recovery schema or observation layout".into(),
            ));
        }
        if state.rollout_scheduler_revision > ROLLOUT_SCHEDULER_REVISION {
            return Err(RunnerError::Data(format!(
                "unsupported rollout scheduler revision {}",
                state.rollout_scheduler_revision
            )));
        }
        if self.cfg.algo == super::Algorithm::Imitation
            && state.imitation_objective_revision > IMITATION_OBJECTIVE_REVISION
        {
            return Err(RunnerError::Data(format!(
                "unsupported imitation objective revision {}",
                state.imitation_objective_revision
            )));
        }
        check_identity(
            matches_legacy_config(&state.config, &self.cfg),
            state.semantic_config.as_ref(),
            || recovery_identity::config_identity(&self.cfg),
            recovery_identity::ADDED_CONFIG_FIELDS,
            CHANGED_CONFIG,
        )?;
        check_identity(
            state.corpus == self.split.recovery_identity(),
            state.semantic_corpus.as_ref(),
            || recovery_identity::corpus_identity(&self.split),
            recovery_identity::ADDED_CORPUS_FIELDS,
            CHANGED_CORPUS,
        )?;
        if state.slots.len() != self.cfg.effective_workers()
            || !self.split.train.contains(&state.focus_entry)
            || state
                .installed_update
                .as_ref()
                .is_some_and(|u| u.iteration >= state.stats.iteration)
        {
            return Err(RunnerError::Data(
                "invalid recovery slot count, focus or update iteration".into(),
            ));
        }
        state.network.validate()?;
        state.rollout_network.validate()?;
        drop(validation);
        self.progress.begin(state.stats.iteration, 0);
        self.progress.phase("restore replay", 2);
        let mut aggregate: LabelledStates = {
            let _task = self.progress.task("decode saved replay labels".into());
            read_state(&directory.join("replay.mpk"))?
        };
        {
            let _task = self.progress.task("validate saved replay labels".into());
            aggregate.validate_recovery(
                self.cfg.label_capacity,
                self.split.max_tiers,
                self.cfg.label_memory_mib * 1024 * 1024,
            )?;
            aggregate.enforce_budget();
        }
        let cached_labels = state
            .slots
            .iter()
            .filter(|slot| slot.label_cache.is_some())
            .count();
        self.check_environment_relabel(&state.environment_math, aggregate.len(), cached_labels)?;
        let slots = restore_slots(
            state.slots,
            &self.cfg,
            &self.split,
            self.execution.as_ref(),
            &self.progress,
        )?;
        if let Some(execution) = &self.execution {
            execution.check().map_err(RunnerError::Data)?;
        }
        self.recovery_safe = false;
        self.sync_update()?;
        self.progress.phase("restore training device", 2);
        {
            let _task = self.progress.task("install saved rollout weights".into());
            self.rollout_trainer.load_weights(&state.rollout_network)?;
        }
        {
            let _task = self
                .progress
                .task("restore optimizer state on training device".into());
            self.updater.restore_training_state(directory)?;
        }
        self.rollout_model = self.rollout_trainer.rollout_model();
        self.net = state.network;
        self.installed_update = state.installed_update;
        self.slots = slots;
        self.aggregate = aggregate;
        self.label_phase = state.label_phase;
        self.normalizer = state.normalizer;
        self.rng = state.rng;
        self.focus_entry = state.focus_entry;
        // The saved warnings come first; this process's own (a GPU fallback, for example) stay.
        let fresh = std::mem::replace(&mut self.warnings, state.warnings);
        for warning in fresh {
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
        if self.cfg.algo == super::Algorithm::Imitation
            && state.imitation_objective_revision < IMITATION_OBJECTIVE_REVISION
        {
            self.warnings.push(format!(
                "Recovery imitation objective revision {} migrated to {}; subsequent gradients use robust component regression, balanced action Huber and bounded direction loss. Learned weights, optimizer moments and replay were retained as intentional continued training. This is not a bitwise continuation of the old objective's optimizer trajectory.",
                state.imitation_objective_revision,
                IMITATION_OBJECTIVE_REVISION,
            ));
        }
        if state.rollout_scheduler_revision < ROLLOUT_SCHEDULER_REVISION
            && self.cfg.algo == super::Algorithm::Imitation
        {
            self.warnings.push(format!(
                "Recovery scheduler revision {} migrated to {}; imitation rollout streams now use deterministic per-slot RNG and fixed quotas. Existing labels and replay were retained; bitwise action replay across this scheduler change is not promised.",
                state.rollout_scheduler_revision,
                ROLLOUT_SCHEDULER_REVISION,
            ));
        }
        self.stats = state.stats;
        self.oracle_cache = state.oracle_cache;
        self.invalidate_changed_environment_labels(&state.environment_math, cached_labels);
        // A completed rollout was already consumed by the saved optimizer; collect_rollout
        // clears these on the next iteration as well. There is no partial PPO batch to resume.
        self.clear_rollout();
        self.recovery_safe = true;
        Ok(())
    }

    /// Refuses, before anything changes, a resume whose environment arithmetic differs from
    /// the saved one when that would drop `replay_labels` retained labels or `cached_labels`
    /// slot caches and [`Self::set_allow_environment_relabel`] did not allow it.
    fn check_environment_relabel(
        &self,
        previous: &str,
        replay_labels: usize,
        cached_labels: usize,
    ) -> Result<(), RunnerError> {
        let current = self.environment_math();
        if previous == current
            || self.allow_environment_relabel
            || (replay_labels == 0 && cached_labels == 0)
        {
            return Ok(());
        }
        Err(RunnerError::Data(format!(
            "recovery was saved with environment arithmetic '{previous}', but this run on {} \
             computes '{current}'. Resuming would discard {replay_labels} retained imitation \
             labels and {cached_labels} cached slot labels while the optimizer step count and \
             learning-rate schedule continue, so the rest of the run would train on a few fresh \
             labels at an already decayed learning rate. Resume on the original device, or pass \
             --relabel to accept dropping and relabelling them",
            self.device_label,
        )))
    }

    /// Drops every label made with other environment arithmetic (permitted by
    /// [`Self::check_environment_relabel`]) and says how many were dropped.
    fn invalidate_changed_environment_labels(&mut self, previous: &str, cached_labels: usize) {
        let current = self.environment_math();
        if previous == current {
            return;
        }
        let replay_labels = self.aggregate.len();
        for slot in &mut self.slots {
            slot.label_cache = LabelCache::default();
        }
        self.aggregate.clear();
        self.stats.labelled_samples = 0;
        self.oracle_cache = None;
        self.warnings.push(format!(
            "Environment arithmetic changed from {previous} to {current}: dropped {replay_labels} retained imitation labels and {cached_labels} cached slot labels, which are relabelled with the new projection; the oracle cache is cleared too. Model, optimizer, RNG, accepted episodes, iteration and learning-rate schedule are retained, so the remaining iterations train on fresh labels only. Bitwise replay across revisions is not guaranteed.",
        ));
    }

    /// Starts a new run from compatible model weights, without inheriting run progress.
    ///
    /// # Errors
    /// Requires a fresh runner, a readable checkpoint and compatible task/model dimensions.
    pub fn warm_start_checkpoint(&mut self, path: &Path) -> Result<(), RunnerError> {
        if self.stats.iteration != 0 || self.updater.in_flight().is_some() {
            return Err(RunnerError::Data(
                "warm start requires a fresh runner".into(),
            ));
        }
        let checkpoint = Checkpoint::load(path, OBS_LAYOUT_VERSION)?;
        checkpoint.ensure_mode(self.cfg.mode)?;
        if checkpoint.tier != self.cfg.tier {
            return Err(RunnerError::Data(
                "warm-start model tier differs from this run".into(),
            ));
        }
        self.updater.load_weights(checkpoint.network.clone())?;
        self.net = checkpoint.network;
        self.refresh_rollout_model()?;
        Ok(())
    }
}

pub(super) mod fidelity {
    use pleochroma_cut_core::optimize::ObjectiveFidelity;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &ObjectiveFidelity,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(*value == ObjectiveFidelity::Full)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ObjectiveFidelity, D::Error> {
        bool::deserialize(deserializer).map(|full| {
            if full {
                ObjectiveFidelity::Full
            } else {
                ObjectiveFidelity::Fast
            }
        })
    }
}

#[cfg(test)]
#[path = "runner_recovery_tests.rs"]
mod tests;

#[cfg(test)]
mod config_identity_migration_tests {
    use super::matches_config_identity;

    #[test]
    fn legacy_budget_migration_keeps_all_other_config_checks() {
        let current =
            "RunConfig { label_capacity: 50000, label_memory_mib: 512, explore_std: 0.3 }";
        let legacy = "RunConfig { label_capacity: 50000, explore_std: 0.3 }";
        assert!(matches_config_identity(legacy, current, 512));
        assert!(!matches_config_identity(legacy, current, 4096));
        assert!(!matches_config_identity(
            &legacy.replace("50000", "50001"),
            current,
            512
        ));
        assert!(!matches_config_identity(
            &current.replace("512", "513"),
            current,
            512
        ));
    }
}
