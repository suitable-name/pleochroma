//! Committed run snapshots and a stable name for unattended restart.
//!
//! Payloads live in immutable generation directories. The small `latest.json` manifest is
//! replaced last, after every file is flushed. A crashed writer therefore never publishes
//! half a model with a different optimizer/replay state. Keep the latest two generations.

use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{cli_args::CliArgs, cli_run_reporting::RunRecords, dataset, runner::TrainingRunner};

#[path = "cli_resume_io.rs"]
mod storage;
#[cfg(test)]
#[path = "cli_resume_tests.rs"]
mod tests;

type ResumeResult<T> = Result<T, Box<dyn Error>>;
const RESUME_SCHEMA: u32 = 1;

/// Work to perform after restoring the last completed training iteration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum ResumePhase {
    Training,
    ValidationPending,
    FinalEvaluationPending,
    Complete,
}

#[derive(Serialize, Deserialize)]
struct RunState {
    schema: u32,
    trainer_version: String,
    iteration: usize,
    phase: ResumePhase,
    args: CliArgs,
    database: Option<PathBuf>,
    records: RunRecords,
}

/// Owns the run lock until the CLI exits, including during initialization/evaluation.
pub(super) struct RecoverySession {
    prefix: PathBuf,
    root: PathBuf,
    manifest: PathBuf,
    database: Option<PathBuf>,
    restored: Option<PathBuf>,
    restored_iteration: Option<usize>,
    phase: ResumePhase,
    records: RunRecords,
    _lock: File,
}

impl RecoverySession {
    pub(super) fn database_path(&self) -> Option<&Path> {
        self.database.as_deref()
    }

    pub(super) fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// Mutable evaluation journal, separate from immutable training generations.
    pub(super) fn validation_directory(&self, iteration: usize, backend: &str) -> PathBuf {
        // An upgraded physics/policy route must never mix completed cases with an
        // older journal. Retain old directories and start a separate namespace.
        let route = match backend {
            crate::runner::CUDA_LABEL => pleochroma_ai::env::CUDA_ENVIRONMENT_MATH,
            crate::runner::GPU_LABEL => "wgpu-cpu-env-v1",
            _ => "cpu-env-v1",
        };
        self.root
            .join(format!("validation-v2-{route}-{iteration:08}"))
    }

    pub(super) const fn is_resumed(&self) -> bool {
        self.restored.is_some()
    }

    pub(super) const fn phase(&self) -> ResumePhase {
        self.phase
    }

    pub(super) fn take_records(&mut self) -> RunRecords {
        std::mem::take(&mut self.records)
    }

    pub(super) fn restore_runner(&self, runner: &mut TrainingRunner) -> ResumeResult<()> {
        if let Some(directory) = &self.restored {
            runner.restore_recovery(directory)?;
            if Some(runner.stats().iteration) != self.restored_iteration {
                return Err("runner snapshot and resume manifest disagree on iteration".into());
            }
        }
        Ok(())
    }

    /// Commits a synchronized iteration boundary, then advances the stable resume target.
    pub(super) fn save(
        &mut self,
        runner: &mut TrainingRunner,
        args: &CliArgs,
        records: &RunRecords,
        phase: ResumePhase,
    ) -> ResumeResult<()> {
        runner.sync_update()?;
        let iteration = runner.stats().iteration;
        let pending = tempfile::Builder::new()
            .prefix(&format!("generation-{iteration:08}-"))
            .tempdir_in(&self.root)?;
        runner.save_recovery(pending.path())?;
        runner.save_checkpoint(&pending.path().join("model.ckpt.json"))?;
        let state = RunState {
            schema: RESUME_SCHEMA,
            trainer_version: env!("CARGO_PKG_VERSION").to_owned(),
            iteration,
            phase,
            args: args.clone(),
            database: self.database.clone(),
            records: records.clone(),
        };
        storage::write_json(&pending.path().join("resume.json"), &state)?;
        storage::publish(pending, &self.root, &self.manifest, iteration)?;
        self.phase = phase;
        Ok(())
    }
}

/// Resolves paths/configuration before loading a corpus or initializing a compute device.
pub(super) fn prepare(
    mut parsed: CliArgs,
    raw: &[String],
) -> ResumeResult<(CliArgs, RecoverySession)> {
    if let Some(target) = &parsed.resume {
        validate_resume_flags(raw)?;
        let prefix = resume_prefix(Path::new(target))?;
        let mut session = open_session(prefix)?;
        let bundle = storage::read_committed(&session.root, &session.manifest)?;
        let state: RunState = storage::read_json(&bundle.join("resume.json"))?;
        validate_state(&state)?;
        let mut args = state.args;
        // A hardware choice is the sole training override. Omission keeps the recorded backend.
        if raw.iter().any(|flag| flag == "--device") {
            args.device = parsed.device;
        }
        args.resume = Some(session.prefix.to_string_lossy().into_owned());
        args.warm_start = None;
        session.database = state.database;
        session.phase = state.phase;
        session.records = state.records;
        session.restored = Some(bundle);
        session.restored_iteration = Some(state.iteration);
        return Ok((args, session));
    }

    normalize_paths(&mut parsed)?;
    let prefix = new_prefix(&parsed)?;
    let mut session = open_session(prefix)?;
    if session.manifest.exists() {
        return Err(format!(
            "run {} already has a recovery checkpoint; use --resume {} or a new --save-weights name",
            session.prefix.display(), session.prefix.display()
        ).into());
    }
    if !parsed.fixture && (parsed.design_dir.is_none() || parsed.include_library) {
        session.database = Some(fs::canonicalize(dataset::find_db_path()?)?);
    }
    Ok((parsed, session))
}

fn validate_state(state: &RunState) -> ResumeResult<()> {
    if state.schema != RESUME_SCHEMA {
        return Err(format!("unsupported recovery schema {}", state.schema).into());
    }
    if state.trainer_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "recovery belongs to trainer {}, this binary is {}; use the matching binary or --warm-start a weight checkpoint",
            state.trainer_version, env!("CARGO_PKG_VERSION")
        ).into());
    }
    if state.iteration > state.args.iterations {
        return Err("recovery iteration exceeds the recorded run length".into());
    }
    if state
        .records
        .history
        .last()
        .is_some_and(|point| point.iteration != state.iteration)
        || state
            .records
            .evaluations
            .iter()
            .any(|evaluation| evaluation.iteration > state.iteration)
    {
        return Err("saved telemetry and recovery iteration disagree".into());
    }
    validate_saved_args(&state.args)?;
    match state.phase {
        ResumePhase::Complete | ResumePhase::FinalEvaluationPending
            if state.iteration != state.args.iterations =>
        {
            return Err("final recovery phase precedes the end of training".into());
        }
        ResumePhase::FinalEvaluationPending if !state.args.eval_test => {
            return Err("pending test evaluation was not enabled in the saved run".into());
        }
        ResumePhase::ValidationPending
            if state.iteration == 0
                || !(state.iteration.is_multiple_of(state.args.validation_every)
                    || state.iteration == state.args.iterations) =>
        {
            return Err("pending validation does not match the saved schedule".into());
        }
        _ => {}
    }
    Ok(())
}

fn validate_resume_flags(raw: &[String]) -> ResumeResult<()> {
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--resume" | "--device" => index += 2,
            "--no-update-check" => index += 1,
            other => return Err(format!(
                "{other} cannot override a recovered run; use only --resume <run> and optionally --device. Use --warm-start to start a new configuration from weights."
            ).into()),
        }
    }
    Ok(())
}

fn open_session(prefix: PathBuf) -> ResumeResult<RecoverySession> {
    let root = suffixed(&prefix, ".run");
    fs::create_dir_all(&root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("writer.lock"))?;
    lock.try_lock().map_err(|error| {
        format!(
            "cannot lock run {} (another trainer may own it): {error}",
            prefix.display()
        )
    })?;
    Ok(RecoverySession {
        manifest: suffixed(&prefix, ".latest.json"),
        prefix,
        root,
        database: None,
        restored: None,
        restored_iteration: None,
        phase: ResumePhase::Training,
        records: RunRecords::default(),
        _lock: lock,
    })
}

fn normalize_paths(args: &mut CliArgs) -> ResumeResult<()> {
    args.stats_path = absolute(Path::new(&args.stats_path))?
        .to_string_lossy()
        .into_owned();
    if args.weights_path.is_none() {
        args.weights_path = Some(
            suffixed(
                &prefix_from_output(Path::new(&args.stats_path))?,
                ".ckpt.json",
            )
            .to_string_lossy()
            .into_owned(),
        );
    }
    for path in [
        &mut args.weights_path,
        &mut args.design_dir,
        &mut args.warm_start,
    ]
    .into_iter()
    .flatten()
    {
        *path = absolute(Path::new(path))?.to_string_lossy().into_owned();
    }
    Ok(())
}

/// Deserialized settings bypass the CLI parser, so check its bounds again before allocating.
fn validate_saved_args(args: &CliArgs) -> ResumeResult<()> {
    args.validate()?;
    for (name, value, minimum, maximum) in [
        ("iterations", Some(args.iterations), 1, usize::MAX),
        ("workers", Some(args.workers), 1, crate::runner::MAX_WORKERS),
        ("batch-size", Some(args.batch_size), 16, 4096),
        ("mini-batch", args.mini_batch, 8, 1024),
        ("epochs", args.epochs, 1, 16),
        ("updates-per-round", args.updates_per_round, 0, 100_000),
        ("validation-designs", Some(args.validation_designs), 1, 64),
        ("validation-every", Some(args.validation_every), 1, 1000),
        ("label-every", args.label_every, 0, 64),
        ("label-capacity", args.label_capacity, 1, 1_000_000),
        ("max-rejected-steps", args.max_rejected_steps, 0, 100_000),
        (
            "max-tiers",
            Some(args.max_tiers),
            1,
            pleochroma_ai::training::MAX_TIERS_LIMIT,
        ),
        ("save-every", Some(args.save_every), 0, 1_000_000),
        ("classical-baseline", args.classical_budget, 1, 4096),
    ] {
        if value.is_some_and(|value| !(minimum..=maximum).contains(&value)) {
            return Err(format!("saved {name} is outside {minimum}..={maximum}").into());
        }
    }
    let config = crate::cli_args::build_config(args);
    config.ppo.validate()?;
    config.imitation.validate()?;
    for (name, value) in [
        ("step limit", config.max_step_deg),
        ("probe size", config.label_h_deg),
    ] {
        if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(format!("saved {name} must be finite and positive").into());
        }
    }
    if !config.explore_std.is_finite()
        || config.explore_std < 0.0
        || !(0.0..=1.0).contains(&config.same_material_frac)
        || !(0.0..=1.0).contains(&config.full_fidelity_frac)
        || config.line_search.is_empty()
        || config
            .line_search
            .iter()
            .any(|scale| !scale.is_finite() || *scale <= 0.0)
    {
        return Err("saved exploration, sampling fraction or line search is invalid".into());
    }
    Ok(())
}

fn new_prefix(args: &CliArgs) -> ResumeResult<PathBuf> {
    let output = args.weights_path.as_deref().unwrap_or(&args.stats_path);
    prefix_from_output(Path::new(output))
}

fn resume_prefix(target: &Path) -> ResumeResult<PathBuf> {
    let path = absolute(target)?;
    let text = path.to_string_lossy();
    if let Some(prefix) = text
        .strip_suffix(".latest.json")
        .or_else(|| text.strip_suffix(".run"))
    {
        return Ok(PathBuf::from(prefix));
    }
    // An exact existing manifest prefix wins before interpreting an output-file suffix.
    if suffixed(&path, ".latest.json").is_file() {
        return Ok(path);
    }
    let prefix = prefix_from_output(&path)?;
    if suffixed(&prefix, ".latest.json").is_file() {
        return Ok(prefix);
    }
    if path.is_file() {
        return Err(format!(
            "{} is a weight-only checkpoint, not a recovery state. Its optimizer/replay cannot be recovered; use --warm-start {} with the original training flags",
            path.display(), path.display()
        ).into());
    }
    Err(format!(
        "no latest recovery checkpoint for {} (expected {})",
        path.display(),
        suffixed(&prefix, ".latest.json").display()
    )
    .into())
}

fn prefix_from_output(path: &Path) -> ResumeResult<PathBuf> {
    let path = absolute(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("run path needs a UTF-8 file name")?;
    let stem = name
        .strip_suffix(".ckpt.json")
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(name);
    if stem.is_empty() || stem == "." || stem == ".." {
        return Err("run name is empty".into());
    }
    Ok(path.with_file_name(stem))
}

fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    std::path::absolute(path)
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}
