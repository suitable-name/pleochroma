//! The training loop shared by the GUI worker thread and the command-line trainer.
//!
//! [`TrainingRunner`] owns the vectorized environments, the pure-Rust set-attention network that
//! validation and the checkpoints use, the Burn trainer that updates the model (and its rollout
//! copy), the labelled aggregate of an imitation run and the run statistics. Callers drive it
//! with three operations: [`TrainingRunner::iterate`] (collect a rollout, update the model),
//! [`TrainingRunner::evaluate`] / [`TrainingRunner::validate`] (episodes on held-out designs) and
//! the checkpoint methods. Everything else is configuration.
//!
//! # Overlap of rollout and update
//!
//! The Burn trainer lives on its own update thread (`runner_update.rs`). In imitation mode the
//! update of iteration `i` trains there while the main thread collects the rollout of iteration
//! `i + 1`, which acts with the weights of update `i - 1`; PPO waits for its update at once.
//! The weights each rollout sees are fixed by the iteration index alone, never by timing (the
//! module docs of `runner_update.rs` give the schedule). Anything that reads the network or the
//! trainer for another purpose goes through [`TrainingRunner::sync_update`] first.
//!
//! # Algorithms
//!
//! [`Algorithm::Imitation`] (the default) rolls the policy out with a little exploration noise,
//! asks the simulator for the finite-difference sensitivities of every `label_every`-th state
//! and trains the model on the growing aggregate of those labelled states (`DAgger`).
//! [`Algorithm::Ppo`] is the policy-gradient alternative on the same model. Both step the
//! environment with the run's line search, and both draw the objective weights of every episode
//! ([`ObjectiveSampling`]); the observation carries those weights as task context.
//!
//! # Devices
//!
//! The action of every rollout step comes from one batched forward of the rollout copy of the
//! Burn model on the training device, so rollout numbers are the device's. Validation, the
//! checkpoints export the pure-Rust twin. Evaluation uses a batched inference service on the
//! resolved training device (and the pure-Rust reference for CPU runs). CUDA runs also use
//! native environment optical scoring and meet residuals, with explicit fallback counters.
//!
//! Conventions: scores are `ObjectiveWeights::score` figures (lower is better); an episode's
//! "improvement" is `base score - final score` (never negative: the environments are greedy).
//!
//! The rest of the runner lives in child modules, which see this module's private items: the
//! configuration (`runner_config.rs`), the data types (`runner_types.rs`), the environment
//! wrappers (`runner_env.rs`), the rollout (`runner_rollout.rs`), the update thread
//! (`runner_update.rs`), the labelled
//! aggregate (`runner_imitation.rs`), the labelling schedule (`runner_teacher.rs`), the thread
//! pools (`runner_pool.rs`), the objective sampling (`runner_objective.rs`), the evaluation
//! machinery (`runner_eval.rs`), the changes to a live run (`runner_setters.rs`), the telemetry
//! and checkpoints (`runner_export.rs`) and the unit tests (`runner_tests.rs`).

// The GUI and the CLI binary both include this file; each uses a different subset of the setters.
#![allow(dead_code, reason = "shared by two binaries that each use a subset")]

use std::{fmt, sync::Arc};

use pleochroma_ai::{
    env::{EnvError, EnvExecution, GLOBAL_DIM, ObservationError, TIER_DIM},
    model::{GemstoneSetAttentionNetwork, ModelError, ModelTier, Prng, TrainingMode, WeightError},
    training::{
        BurnSetAttention, BurnTrainer, ModelShape, PpoConfig, RolloutBuffer, TrainError,
        buffer::ReturnNormalizer,
    },
};
use pleochroma_cut_core::optimize::ObjectiveFidelity;

use crate::dataset::{DatasetError, DatasetSplit};
use envs::{Slot, build_env, pick_target, sampled_source_material};
use eval::OracleCache;
use imitation::LabelledStates;
use update::{BuildArgs, InstalledUpdate, Updater};

#[path = "runner_bootstrap.rs"]
mod bootstrap;
#[path = "runner_config.rs"]
pub mod config;
#[cfg(test)]
#[path = "runner_device_tests.rs"]
mod device_tests;
#[path = "runner_env.rs"]
mod envs;
#[path = "runner_eval.rs"]
mod eval;
#[path = "runner_export.rs"]
mod export;
#[path = "runner_imitation.rs"]
mod imitation;
#[path = "runner_label_cache.rs"]
mod label_cache;
#[path = "runner_objective.rs"]
pub mod objective;
#[path = "runner_pool.rs"]
mod pool;
#[path = "runner_progress.rs"]
pub mod progress;
#[path = "runner_recovery.rs"]
mod recovery;
#[path = "runner_recovery_identity.rs"]
mod recovery_identity;
#[path = "runner_rollout.rs"]
mod rollout;
#[path = "runner_rollout_stream.rs"]
mod rollout_stream;
#[path = "runner_setters.rs"]
mod setters;
#[path = "runner_teacher.rs"]
mod teacher;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
#[path = "runner_types.rs"]
mod types;
#[path = "runner_update.rs"]
mod update;
#[cfg(test)]
#[path = "runner_update_tests.rs"]
mod update_tests;
#[path = "runner_validation.rs"]
mod validation;

// Only what both binaries use is re-exported; the CLI and the tests take the rest from
// `config` directly (an unused re-export is a warning in a binary crate).
pub use config::{
    DeviceChoice, Hyperparams, MAX_WORKERS, MIN_GAP_DESIGNS, RunConfig, resumed_schedule,
};
pub use objective::{Algorithm, ObjectiveSampling, draw_weights, draws_same_material};
pub use types::{
    EpisodeSummary, EvalCase, EvalFidelity, EvalPasses, EvalReport, IterationReport, MaterialLabel,
    Optics, RolloutStats, RunnerStats,
};

/// The episode of `entry` (at `pos` of the train split): its environment with the target
/// material and the objective weights drawn from `seed`, reset to its first observation.
///
/// Pure in the runner's state, so the rollout builds the next episodes of several slots in
/// parallel. The outer error is a hard one (an unknown fixed target material); the inner one
/// says the design's environment does not build, which corpus mode skips.
fn build_slot(
    cfg: &RunConfig,
    split: &DatasetSplit,
    pos: usize,
    entry: usize,
    seed: u64,
    execution: Option<&Arc<EnvExecution>>,
) -> Result<Result<Slot, EnvError>, RunnerError> {
    let item = &split.entries[entry];
    let design = &item.design;
    let authored = item.authored_material.as_ref();
    let target = pick_target(cfg, seed, design, authored)?;
    let weights = draw_weights(cfg.objectives, seed);
    let result = build_env(
        cfg,
        design,
        target.as_ref(),
        weights,
        (Some(config::training_fidelity(cfg, seed)), execution),
        seed,
        authored,
    );
    if let Some(execution) = execution {
        execution.check().map_err(RunnerError::Data)?;
    }
    let mut env = match result {
        Ok(env) => env,
        Err(error) => return Ok(Err(error)),
    };
    let obs = env.reset(seed);
    let (source_name, _, _) = sampled_source_material(seed, design, authored)?;
    let src = MaterialLabel {
        name: source_name,
        n_d: env.source_material().n_d,
    };
    let tgt = target.as_ref().map_or_else(
        || src.clone(),
        |t| MaterialLabel {
            name: t.name.clone(),
            n_d: t.material.n_d,
        },
    );
    Ok(Ok(Slot {
        seed,
        env,
        pos,
        entry,
        src,
        tgt,
        obs,
        label_cache: label_cache::LabelCache::default(),
        rejected_steps: 0,
    }))
}

/// Backend label of the GPU trainer.
pub const GPU_LABEL: &str = "GPU (WGPU)";
/// Backend label of native CUDA training.
pub const CUDA_LABEL: &str = "GPU (CUDA)";
/// Backend label of the CPU trainer.
pub const CPU_LABEL: &str = "CPU (Flex SIMD)";
/// Environment arithmetic of every run without native CUDA environments (CPU and WGPU).
const CPU_ENVIRONMENT_MATH: &str = "cpu-env-v1";

/// Reward history length kept for the bar chart.
const REWARD_HISTORY_LEN: usize = 60;
/// Finished episodes the rolling improvement is averaged over.
const IMPROVEMENT_WINDOW: usize = 64;
/// Smoothing of the per-step reward average.
const REWARD_EMA: f32 = 0.05;
/// Prefix of the warning raised for a design whose environment cannot be built; the CLI uses it
/// to cap the number of skip lines it prints.
pub const SKIPPED_PREFIX: &str = "skipping design ";

/// Anything that can stop the run.
#[derive(Debug)]
pub enum RunnerError {
    /// A configuration value is out of range.
    Config(String),
    /// The data does not allow the requested operation.
    Data(String),
    /// Data set or material error.
    Dataset(DatasetError),
    /// An environment could not be built.
    Env(EnvError),
    /// The network rejected an input.
    Model(ModelError),
    /// The trainer failed.
    Train(TrainError),
    /// A checkpoint could not be written, read or validated.
    Weights(WeightError),
    /// A design has more tiers than the observation layout holds.
    Observation(ObservationError),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) | Self::Data(m) => f.write_str(m),
            Self::Dataset(e) => write!(f, "{e}"),
            Self::Env(e) => write!(f, "{e}"),
            Self::Model(e) => write!(f, "{e}"),
            Self::Train(e) => write!(f, "{e}"),
            Self::Weights(e) => write!(f, "{e}"),
            Self::Observation(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RunnerError {}

macro_rules! runner_from {
    ($($from:ty => $variant:ident),* $(,)?) => {
        $(impl From<$from> for RunnerError {
            fn from(e: $from) -> Self { Self::$variant(e) }
        })*
    };
}
runner_from!(
    DatasetError => Dataset,
    EnvError => Env,
    ModelError => Model,
    TrainError => Train,
    WeightError => Weights,
    ObservationError => Observation,
);

/// The file name a checkpoint gets when the caller names none.
#[must_use]
pub fn checkpoint_file_name(mode: TrainingMode, tier: ModelTier, iteration: usize) -> String {
    format!(
        "pleochroma-{mode}-{}-iter{iteration}.json",
        tier.code_str().to_ascii_lowercase()
    )
}

/// Threads [`TrainingRunner::evaluate`] spreads `designs` designs over (0 for no designs): the
/// core count, but never more than the designs.
#[must_use]
pub fn evaluation_threads(designs: usize) -> usize {
    pool::pass_threads(designs)
}

/// Persistent environment worker pool width (serial fallback reports one).
#[must_use]
pub fn cpu_pool_threads() -> usize {
    pool::pool_threads()
}

fn build_trainer(
    ppo: &PpoConfig,
    tier: ModelTier,
    max_tiers: usize,
    device: DeviceChoice,
    seed: u64,
) -> Result<(BurnTrainer, &'static str, Option<String>), RunnerError> {
    let (d_model, num_heads, num_layers) = tier.config();
    let shape = ModelShape {
        d_tier: TIER_DIM,
        d_global: GLOBAL_DIM,
        d_model,
        num_heads,
        num_layers,
        max_tiers,
    };
    let (trainer, label, warning) = match device {
        DeviceChoice::Cpu => (BurnTrainer::new_cpu(shape, ppo.clone())?, CPU_LABEL, None),
        DeviceChoice::Gpu => (BurnTrainer::new_gpu(shape, ppo.clone())?, GPU_LABEL, None),
        DeviceChoice::Cuda => (BurnTrainer::new_cuda(shape, ppo.clone())?, CUDA_LABEL, None),
        DeviceChoice::Auto => {
            let (trainer, skipped) = BurnTrainer::new_auto(shape, ppo.clone())?;
            match skipped {
                None => (trainer, GPU_LABEL, None),
                Some(reason) => (
                    trainer,
                    CPU_LABEL,
                    Some(format!("GPU unavailable, training on the CPU: {reason}")),
                ),
            }
        }
    };
    Ok((trainer.with_seed(seed), label, warning))
}

/// The explicit device choice that reproduces the backend `label` names (what an `Auto` choice
/// resolved to), so the rollout trainer lands on the same device as the update thread's.
fn device_of_label(label: &str) -> DeviceChoice {
    match label {
        GPU_LABEL => DeviceChoice::Gpu,
        CUDA_LABEL => DeviceChoice::Cuda,
        _ => DeviceChoice::Cpu,
    }
}

/// The training run: vectorized environments, a model and its trainer, statistics.
pub struct TrainingRunner {
    cfg: RunConfig,
    split: DatasetSplit,
    /// The update thread, which owns the Burn trainer that updates the model.
    updater: Updater,
    /// A second, never-trained trainer on the same device, kept only to turn `net` into the
    /// rollout copy (`rollout_model`) on the main thread, so no Burn object crosses threads.
    rollout_trainer: BurnTrainer,
    /// The pure-Rust twin of the model as of the last installed update: validation, `evaluate`
    /// and the checkpoints read it (call [`Self::sync_update`] first to include the update in
    /// flight).
    net: GemstoneSetAttentionNetwork,
    /// The inference copy of the Burn model the rollout acts with (one batched forward per
    /// pass on the training device). Refreshed at the wait point of every iteration and after
    /// every weight change, never by a bare [`Self::sync_update`].
    rollout_model: BurnSetAttention,
    /// The update installed since the last report (its figures go into the next one).
    installed_update: Option<InstalledUpdate>,
    slots: Vec<Slot>,
    /// PPO only: the rollout's samples.
    buffer: RolloutBuffer,
    /// PPO only: per sample of the rollout, the real tier count.
    n_tiers: Vec<usize>,
    /// PPO only: per sample of the rollout, which real tiers the environment will move.
    free_masks: Vec<Vec<bool>>,
    /// Imitation only: labelled states retained by full recovery, but not weight-only exports.
    aggregate: LabelledStates,
    /// Rollout passes finished so far; rotates which slots a pass labels.
    label_phase: usize,
    /// Scale of the discounted return the rewards are divided by (review TQ-15). It persists
    /// across iterations and is saved by full recovery; weight-only warm starts re-estimate it.
    normalizer: ReturnNormalizer,
    rng: Prng,
    focus_entry: usize,
    warnings: Vec<String>,
    stats: RunnerStats,
    /// The oracle's validation drop, kept while nothing it depends on changes.
    oracle_cache: Option<OracleCache>,
    device_label: &'static str,
    execution: Option<Arc<EnvExecution>>,
    /// Shared read-only snapshots for a CLI observer or other caller.
    progress: progress::TrainingProgress,
    /// False while an iteration or recovery install can leave partially mutated state.
    /// Errors and unwinding retain this latch until a complete recovery is restored or the run
    /// is re-synchronised ([`Self::resync_after_failure`], a reset, a weight load).
    recovery_safe: bool,
    /// Whether [`Self::restore_recovery`] may discard labels made with other environment
    /// arithmetic (another device or revision) and relabel; refused by default.
    allow_environment_relabel: bool,
}

impl TrainingRunner {
    /// Builds the run: trainer on the requested device, seeded network, one environment per
    /// worker. Warnings (for example a GPU fallback) are available from
    /// [`Self::take_warnings`].
    ///
    /// # Errors
    ///
    /// [`RunnerError`] for an invalid configuration, an unavailable explicit device, or when no
    /// training design builds an environment.
    pub fn new(cfg: RunConfig, split: DatasetSplit) -> Result<Self, RunnerError> {
        let mut runner = Self::new_for_recovery(cfg, split)?;
        runner.rebuild_slots()?;
        Ok(runner)
    }

    /// Builds the device and model owners without constructing throwaway initial episodes.
    ///
    /// Call [`Self::restore_recovery`] before iterating. The recovered slots each reconstruct
    /// their immutable geometry once and reuse saved optical measurements and material features.
    ///
    /// # Errors
    /// As [`Self::new`], excluding environment construction (deferred to recovery).
    pub fn new_for_recovery(cfg: RunConfig, split: DatasetSplit) -> Result<Self, RunnerError> {
        cfg.validate()?;
        if split.train.is_empty() {
            return Err(RunnerError::Data("the train split is empty".into()));
        }
        let net = cfg.tier.init_network(cfg.seed);
        let (updater, built) = Updater::spawn(BuildArgs {
            ppo: cfg.ppo.clone(),
            tier: cfg.tier,
            max_tiers: split.max_tiers,
            device: cfg.device,
            seed: cfg.seed,
            net: net.clone(),
        })?;
        let (device_label, warning) = (built.label, built.warning);
        let execution = if device_label == CUDA_LABEL {
            Some(EnvExecution::new_cuda(0).map_err(RunnerError::Data)?)
        } else {
            None
        };
        let progress = progress::TrainingProgress::default();
        progress.set_execution(execution.clone());
        let (mut rollout_trainer, _, _) = build_trainer(
            &cfg.ppo,
            cfg.tier,
            split.max_tiers,
            device_of_label(device_label),
            cfg.seed,
        )?;
        rollout_trainer.load_weights(&net)?;
        let focus_entry = split.train[0];
        let rollout_model = rollout_trainer.rollout_model();
        let runner = Self {
            rng: Prng::from_stream(cfg.seed, 1),
            aggregate: LabelledStates::with_budget(
                cfg.label_capacity,
                cfg.label_memory_mib * 1024 * 1024,
            ),
            cfg,
            split,
            updater,
            rollout_trainer,
            net,
            rollout_model,
            installed_update: None,
            slots: Vec::new(),
            buffer: RolloutBuffer::default(),
            n_tiers: Vec::new(),
            free_masks: Vec::new(),
            label_phase: 0,
            normalizer: ReturnNormalizer::new(),
            focus_entry,
            warnings: warning.into_iter().collect(),
            stats: RunnerStats::new(),
            oracle_cache: None,
            device_label,
            execution,
            progress,
            recovery_safe: true,
            allow_environment_relabel: false,
        };
        Ok(runner)
    }

    /// The configuration in effect.
    #[must_use]
    pub const fn config(&self) -> &RunConfig {
        &self.cfg
    }

    /// The data set.
    #[must_use]
    pub const fn split(&self) -> &DatasetSplit {
        &self.split
    }

    /// Cumulative statistics.
    #[must_use]
    pub const fn stats(&self) -> &RunnerStats {
        &self.stats
    }

    /// Backend label of the trainer in use.
    #[must_use]
    pub const fn device_label(&self) -> &'static str {
        self.device_label
    }

    /// A cloneable observer handle for live progress snapshots.
    #[must_use]
    pub fn progress(&self) -> progress::TrainingProgress {
        self.progress.clone()
    }

    /// The step limit in effect, degrees per step and tier: the configured override, or the
    /// environments' own default (read from the displayed environment).
    #[must_use]
    pub fn max_step_deg(&self) -> f64 {
        self.slots
            .first()
            .map(|s| s.env.max_step_deg())
            .or(self.cfg.max_step_deg)
            .unwrap_or(0.0)
    }

    /// Source and target material of the displayed stone (worker 0).
    #[must_use]
    pub fn display_materials(&self) -> Option<(&MaterialLabel, &MaterialLabel)> {
        self.slots.first().map(|s| (&s.src, &s.tgt))
    }

    /// Data-set entry the displayed stone (worker 0) trains on.
    #[must_use]
    pub fn display_entry(&self) -> Option<usize> {
        self.slots.first().map(|s| s.entry)
    }

    /// The objective weights the displayed stone (worker 0) is scored with.
    #[must_use]
    pub fn display_weights(&self) -> Option<&pleochroma_cut_core::ObjectiveWeights> {
        self.slots.first().map(|s| s.env.weights())
    }

    /// Warnings raised since the last call (skipped designs, device fallback).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Whether a failed iteration, a failed update or an interrupted recovery install left the
    /// run inconsistent. [`Self::iterate`] and [`Self::save_recovery`] refuse such a run until
    /// [`Self::restore_recovery`], [`Self::resync_after_failure`], [`Self::reset`] or
    /// [`Self::load_checkpoint`] makes it consistent again.
    #[must_use]
    pub const fn needs_resync(&self) -> bool {
        !self.recovery_safe
    }

    /// Makes a failed run trainable again without a recovery bundle (the GUI keeps none).
    ///
    /// The update in flight is drained (a failed one is dropped with a warning), the update
    /// thread's trainer is reloaded from `net`, the weights of the last installed update (steps
    /// a failed update applied are dropped and the Adam moments start over), the rollout copy
    /// follows, and fresh environments replace the episodes the failed iteration may have left
    /// half-stepped. Statistics, iteration count and the labelled aggregate (every label is
    /// exact for its state) are kept. A no-op on a consistent run.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when the trainers cannot be reloaded or no environment builds (a latched
    /// accelerator failure, for example); the run then stays failed.
    pub fn resync_after_failure(&mut self) -> Result<(), RunnerError> {
        if self.recovery_safe {
            return Ok(());
        }
        self.resync_trainers()?;
        self.rebuild_slots()?;
        self.recovery_safe = true;
        Ok(())
    }

    /// The arithmetic the environments score with: native CUDA math, or the CPU math that the
    /// CPU and WGPU trainers share. Labels made with one are never reused with another.
    const fn environment_math(&self) -> &'static str {
        if self.execution.is_some() {
            pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
        } else {
            CPU_ENVIRONMENT_MATH
        }
    }

    /// The editable hyperparameters in effect (the imitation settings in imitation mode).
    #[must_use]
    pub const fn hyperparams(&self) -> Hyperparams {
        let (mini_batch, ppo_epochs, learning_rate) = match self.cfg.algo {
            Algorithm::Ppo => (
                self.cfg.ppo.mini_batch_size,
                self.cfg.ppo.epochs_per_batch,
                self.cfg.ppo.learning_rate,
            ),
            Algorithm::Imitation => (
                self.cfg.imitation.mini_batch_size,
                self.cfg.imitation.epochs_per_round,
                self.cfg.imitation.learning_rate,
            ),
        };
        Hyperparams {
            workers: self.cfg.workers,
            rollout_size: self.cfg.rollout_size,
            mini_batch,
            ppo_epochs,
            learning_rate,
            clip_epsilon: self.cfg.ppo.clip_epsilon,
            gae_lambda: self.cfg.ppo.gae_lambda,
        }
    }

    // ---- environments -------------------------------------------------------------------

    /// First usable (position, entry) at or after `start` in the train split, or the focus
    /// stone in single-stone mode.
    fn choose(&self, start: usize) -> Result<(usize, usize), RunnerError> {
        if !self.cfg.corpus_mode {
            return Ok((0, self.focus_entry));
        }
        let n = self.split.train.len();
        if n == 0 {
            return Err(RunnerError::Data("the train split is empty".to_owned()));
        }
        let pos = start % n;
        Ok((pos, self.split.train[pos]))
    }

    /// Starts an episode: a design from the train split at or after `start`, its target
    /// material and its objective weights, all drawn from `seed`. A design whose environment
    /// does not build is skipped ([`Self::skip_entry`]) and the next one is tried.
    fn spawn_slot(&mut self, start: usize, seed: u64) -> Result<Slot, RunnerError> {
        let mut start = start;
        let mut last_error = None;
        self.progress
            .phase("serial respawn retry", self.split.train.len());
        for _ in 0..self.split.train.len() {
            let (pos, entry) = self.choose(start)?;
            let attempt = {
                let _task = self
                    .progress
                    .task(format!("serial slot retry entry {entry}"));
                build_slot(
                    &self.cfg,
                    &self.split,
                    pos,
                    entry,
                    seed,
                    self.execution.as_ref(),
                )?
            };
            match attempt {
                Ok(slot) => return Ok(slot),
                Err(error) => {
                    last_error = Some(error.to_string());
                    start = self.skip_entry(pos, entry, &error)?;
                }
            }
        }
        Err(RunnerError::Data(format!(
            "no sampled training design builds an environment for this target/objective: {}",
            last_error.unwrap_or_else(|| "empty train split".to_owned())
        )))
    }

    /// Records that the environment of `entry` (at `pos` of the train split) failed to build
    /// with `error` and returns the position the search for a design continues from. In
    /// single-stone mode there is nothing to continue with: the error is hard.
    fn skip_entry(
        &mut self,
        pos: usize,
        entry: usize,
        error: &dyn fmt::Display,
    ) -> Result<usize, RunnerError> {
        let title = self.split.entries[entry].title.clone();
        if !self.cfg.corpus_mode {
            return Err(RunnerError::Data(format!(
                "the selected design '{title}' cannot be trained: {error}"
            )));
        }
        self.warnings
            .push(format!("{SKIPPED_PREFIX}'{title}': {error}"));
        Ok(pos + 1)
    }

    fn install_slot(&mut self, w: usize, slot: Slot) {
        if w == 0 {
            self.stats.baseline = Some(Optics::from_global(&slot.obs.global));
            self.stats.current = self.stats.baseline;
        }
        if w < self.slots.len() {
            self.slots[w] = slot;
        } else {
            self.slots.push(slot);
        }
    }

    /// Builds a fresh environment per worker. The new slots replace the old ones only when every
    /// one built, so a failure leaves the run exactly as it was.
    fn rebuild_slots(&mut self) -> Result<(), RunnerError> {
        let n = self.cfg.effective_workers();
        let starts = (0..n)
            .map(|w| {
                let seed = self.rng.next_u64();
                let (pos, entry) = self.choose(w)?;
                Ok((pos, entry, seed))
            })
            .collect::<Result<Vec<_>, RunnerError>>()?;
        let cfg = &self.cfg;
        let split = &self.split;
        let execution = self.execution.as_ref();
        let first_attempts = pool::run_indexed(n, pool::pass_threads(n), |i| {
            let (pos, entry, seed) = starts[i];
            (
                pos,
                entry,
                seed,
                build_slot(cfg, split, pos, entry, seed, execution),
            )
        });
        let mut fresh = Vec::with_capacity(n);
        for (pos, entry, seed, attempt) in first_attempts {
            match attempt? {
                Ok(slot) => fresh.push(slot),
                Err(error) => {
                    let next = self.skip_entry(pos, entry, &error)?;
                    fresh.push(self.spawn_slot(next, seed)?);
                }
            }
        }
        self.slots = fresh;
        self.clear_rollout();
        // The in-flight episodes are gone and the next rewards come from other environments (new
        // task, targets, worker set, model): the scale estimated on them no longer applies.
        self.normalizer.reset();
        if let Some(first) = self.slots.first() {
            self.stats.baseline = Some(Optics::from_global(&first.obs.global));
            self.stats.current = self.stats.baseline;
        }
        Ok(())
    }

    /// Empties the PPO rollout buffers (the imitation aggregate is a different thing: it
    /// outlives rollouts).
    fn clear_rollout(&mut self) {
        self.buffer.clear();
        self.n_tiers.clear();
        self.free_masks.clear();
    }

    /// Points the rollout copy of the model at the weights of `net` (the installed update).
    fn refresh_rollout_model(&mut self) -> Result<(), RunnerError> {
        self.progress.phase("rollout model refresh", 1);
        let _refresh = self.progress.task("install rollout weights".into());
        self.rollout_trainer.load_weights(&self.net)?;
        self.rollout_model = self.rollout_trainer.rollout_model();
        Ok(())
    }

    /// Drains the update thread and points both trainers at `net`. A failed update in flight is
    /// consumed; its error becomes a warning, since `net` never received its weights.
    fn resync_trainers(&mut self) -> Result<(), RunnerError> {
        if let Err(error) = self.sync_update() {
            self.warnings.push(format!(
                "the update in flight failed and was discarded: {error}"
            ));
        }
        self.updater.load_weights(self.net.clone())?;
        self.refresh_rollout_model()
    }

    /// Rebuilds the environments, then starts the statistics over and empties the labelled
    /// aggregate. The update in flight finishes first and is installed (the weights are kept);
    /// its figures belong to the statistics that start over and are dropped. When no
    /// environment can be built nothing changes (statistics and aggregate included).
    ///
    /// After a failure ([`Self::needs_resync`]) the trainers are re-synchronised first (a
    /// failed update in flight is dropped, the update thread is reloaded from `net` and its
    /// Adam moments start over), and a successful restart makes the run trainable again. When
    /// the environments then fail to build, the reloaded trainers stay and the run stays failed.
    fn restart_run(&mut self) -> Result<(), RunnerError> {
        if self.recovery_safe {
            self.sync_update()?;
            self.refresh_rollout_model()?;
        } else {
            self.resync_trainers()?;
        }
        let previous = std::mem::replace(&mut self.stats, RunnerStats::new());
        self.rebuild_slots().inspect_err(|_| {
            self.stats = previous;
        })?;
        self.aggregate.clear();
        self.installed_update = None;
        self.recovery_safe = true;
        Ok(())
    }
}
