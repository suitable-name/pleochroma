//! Complete runner snapshots, committed by the CLI only after all payloads are durable.
//!
//! MessagePack retains NaN label masks and exact float bits. Replay is streamed separately
//! so a save does not duplicate its potentially large resident allocation. A recovery occurs
//! at an iteration boundary: work after that boundary is deliberately repeated after a crash.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use pleochroma_ai::{
    env::{GemstoneObservation, OBS_LAYOUT_VERSION, recovery::EpisodeSnapshot},
    model::{Checkpoint, GemstoneSetAttentionNetwork, Prng},
    training::buffer::ReturnNormalizer,
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
    update::InstalledUpdate,
};

const VERSION: u32 = 1;

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
            let _task = progress.task(format!(
                "restore slot {}/{} design {} (saved optical scores)",
                index + 1,
                count,
                state.entry,
            ));
            state.restore(cfg, split)
        },
    )
    .into_iter()
    .collect()
}

#[derive(Serialize, Deserialize)]
struct RunnerState {
    version: u32,
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

fn config_identity(runner: &TrainingRunner) -> String {
    let mut config = runner.cfg.clone();
    config.device = DeviceChoice::Auto; // The sole training override allowed during recovery.
    format!("{config:?}")
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
            obs_layout_version: OBS_LAYOUT_VERSION,
            environment_math: if self.execution.is_some() {
                pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
            } else {
                "cpu-env-v1"
            }
            .to_owned(),
            config: config_identity(self),
            corpus: format!("{:?}", self.split),
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
        };
        self.updater.save_training_state(directory)?;
        write_state(&directory.join("runner-state.mpk"), &state)?;
        write_state(&directory.join("replay.mpk"), &self.aggregate)
    }

    /// Restores a saved iteration boundary with its original corpus and settings.
    ///
    /// Device selection may differ. Numerical identity across different hardware/backends is
    /// not promised. Optimizer, episode and schedule state are retained; labels are rebuilt
    /// when the environment arithmetic changes, since their actions may project differently.
    ///
    /// # Errors
    /// Refuses changed data/configuration, incompatible state, or unavailable backend.
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
        if state.config != config_identity(self) || state.corpus != format!("{:?}", self.split) {
            return Err(RunnerError::Data(
                "recovery requires the original run configuration and unchanged dataset".into(),
            ));
        }
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
        let aggregate: LabelledStates = {
            let _task = self.progress.task("decode saved replay labels".into());
            read_state(&directory.join("replay.mpk"))?
        };
        {
            let _task = self.progress.task("validate saved replay labels".into());
            aggregate.validate_recovery(self.cfg.label_capacity, self.split.max_tiers)?;
        }
        let mut slots = restore_slots(state.slots, &self.cfg, &self.split, &self.progress)?;
        if let Some(execution) = &self.execution {
            for slot in &mut slots {
                slot.env.set_execution(std::sync::Arc::clone(execution));
            }
            execution.check().map_err(RunnerError::Data)?;
        }
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
        self.warnings = state.warnings;
        self.stats = state.stats;
        self.oracle_cache = state.oracle_cache;
        self.invalidate_changed_environment_labels(&state.environment_math);
        // A completed rollout was already consumed by the saved optimizer; collect_rollout
        // clears these on the next iteration as well. There is no partial PPO batch to resume.
        self.clear_rollout();
        Ok(())
    }

    fn invalidate_changed_environment_labels(&mut self, previous: &str) {
        let current = if self.execution.is_some() {
            pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
        } else {
            "cpu-env-v1"
        };
        if previous == current {
            return;
        }
        for slot in &mut self.slots {
            slot.label_cache = LabelCache::default();
        }
        self.aggregate.clear();
        self.stats.labelled_samples = 0;
        self.oracle_cache = None;
        self.warnings.push(format!(
            "Environment arithmetic changed from {previous} to {current}; model, optimizer, RNG, accepted episodes and iteration are retained. Slot labels, imitation replay and oracle cache are cleared so actions are relabelled with the new projection. Bitwise replay across revisions is not guaranteed.",
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
