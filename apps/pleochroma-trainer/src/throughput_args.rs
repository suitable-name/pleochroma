//! Standalone diagnostic arguments. Defaults match the reported medium-model training run.

use std::{collections::BTreeSet, path::PathBuf};

use pleochroma_ai::{
    model::{ModelTier, TrainingMode},
    training::{ImitationConfig, LrSchedule},
};
use serde::Serialize;

use crate::runner::{
    Algorithm, DeviceChoice, ObjectiveSampling, RunConfig, config::default_workers,
};

pub const HELP: &str = "pleochroma-throughput-probe --database PATH --output DIR [options]
  --scenario pipeline|projection   Default pipeline; never runs validation
  --device cpu|cuda                Strict backend, default cuda; no automatic fallback
  --workers N                      Environment slots, default 3 x available CPU threads
  --warmup N                       Pipeline warm-up iterations, default 1
  --iterations N                   Measured pipeline iterations, default 2
  --sync-every-iteration           Time an explicit update wait after every iteration
  --entries I,J,...                Original loaded corpus indices; pipeline requires train indices
  --repeats N                      Projection repeats per action, default 2
  --max-step-deg X                 Projection step limit, default 0.5
  --seed N                         Default 42
  --batch-size N                   Environment steps per iteration, default 256
  --mini-batch N                   Neural mini-batch rows, default 128
  --updates-per-round N            Optimizer steps per iteration, default 16
  --label-every N                  Default 2
  --max-seconds N                  Optional soft limit checked between operations
The output directory must be new; metadata.json, events.jsonl and summary.json never overwrite.
Every heartbeat and completed case is flushed. Timings are host wall time, not GPU utilization.
The soft limit cannot interrupt an already running projection or initialization.\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Pipeline,
    Projection,
}

#[derive(Clone, Debug, Serialize)]
pub struct Args {
    pub database: PathBuf,
    pub output: PathBuf,
    pub scenario: Scenario,
    pub device: DeviceChoice,
    pub workers: usize,
    pub warmup: usize,
    pub iterations: usize,
    pub sync_every_iteration: bool,
    pub entries: Option<Vec<usize>>,
    pub repeats: usize,
    pub max_step_deg: f64,
    pub seed: u64,
    pub batch_size: usize,
    pub mini_batch: usize,
    pub updates_per_round: usize,
    pub label_every: usize,
    pub max_seconds: Option<u64>,
}

impl Args {
    pub fn config(&self) -> RunConfig {
        let mut cfg = RunConfig::new(TrainingMode::Retarget, ModelTier::Medium);
        cfg.device = self.device;
        cfg.workers = self.workers;
        cfg.seed = self.seed;
        cfg.rollout_size = self.batch_size;
        cfg.algo = Algorithm::Imitation;
        cfg.corpus_mode = true;
        cfg.universal_spectrum = true;
        cfg.fixed_target = "Diamond".to_owned();
        cfg.train_fidelity = pleochroma_cut_core::optimize::ObjectiveFidelity::Fast;
        cfg.full_fidelity_frac = 0.0;
        cfg.objectives = ObjectiveSampling::Mixed;
        cfg.same_material_frac = 0.25;
        cfg.explore_std = 0.3;
        cfg.max_rejected_steps = 8;
        cfg.max_step_deg = None;
        cfg.label_h_deg = None;
        cfg.label_capacity = 50_000;
        cfg.line_search = vec![1.0, 0.5, 0.25];
        cfg.objective_sweep = false;
        cfg.classical_budget = None;
        cfg.imitation = ImitationConfig {
            learning_rate: 3e-4,
            // Match the reported 300-iteration run, independent of diagnostic length.
            lr_schedule: LrSchedule::Linear {
                total_updates: 300_u64.saturating_mul(self.updates_per_round as u64),
            },
            mini_batch_size: self.mini_batch,
            epochs_per_round: 2,
            updates_per_round: self.updates_per_round,
            max_grad_norm: 1.0,
            cosine_coeff: 0.5,
            action_coeff: 1.0,
            gain_coeff: 0.1,
        };
        cfg.label_every = self.label_every;
        // RunConfig requires positive validation fields. This driver never calls evaluate.
        cfg.validation_every = usize::MAX;
        cfg.validation_designs = 16;
        cfg.validation_fidelity = crate::runner::EvalFidelity::Fast;
        cfg
    }
}

pub fn parse(args: &[String]) -> Result<Option<Args>, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(None);
    }
    let mut parsed = Args {
        database: PathBuf::new(),
        output: PathBuf::new(),
        scenario: Scenario::Pipeline,
        device: DeviceChoice::Cuda,
        workers: default_workers(),
        warmup: 1,
        iterations: 2,
        sync_every_iteration: false,
        entries: None,
        repeats: 2,
        max_step_deg: 0.5,
        seed: 42,
        batch_size: 256,
        mini_batch: 128,
        updates_per_round: 16,
        label_every: 2,
        max_seconds: None,
    };
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        index += 1;
        if flag == "--sync-every-iteration" {
            parsed.sync_every_iteration = true;
            continue;
        }
        let value = args
            .get(index)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        index += 1;
        assign(&mut parsed, flag, value)?;
    }
    validate(&parsed)?;
    Ok(Some(parsed))
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag}: {value}"))
}

fn assign(args: &mut Args, flag: &str, value: &str) -> Result<(), String> {
    match flag {
        "--database" => args.database = value.into(),
        "--output" => args.output = value.into(),
        "--scenario" => {
            args.scenario = match value {
                "pipeline" => Scenario::Pipeline,
                "projection" => Scenario::Projection,
                _ => return Err("--scenario must be pipeline or projection".into()),
            }
        }
        "--device" => {
            args.device = match value {
                "cpu" => DeviceChoice::Cpu,
                "cuda" => DeviceChoice::Cuda,
                _ => {
                    return Err(
                        "--device must be cpu or cuda; auto is intentionally unsupported".into(),
                    );
                }
            }
        }
        "--workers" => args.workers = number(flag, value)?,
        "--warmup" => args.warmup = number(flag, value)?,
        "--iterations" => args.iterations = number(flag, value)?,
        "--repeats" => args.repeats = number(flag, value)?,
        "--max-step-deg" => args.max_step_deg = number(flag, value)?,
        "--seed" => args.seed = number(flag, value)?,
        "--batch-size" => args.batch_size = number(flag, value)?,
        "--mini-batch" => args.mini_batch = number(flag, value)?,
        "--updates-per-round" => args.updates_per_round = number(flag, value)?,
        "--label-every" => args.label_every = number(flag, value)?,
        "--max-seconds" => args.max_seconds = Some(number(flag, value)?),
        "--entries" => {
            let entries: Vec<usize> = value
                .split(',')
                .map(|v| number(flag, v))
                .collect::<Result<_, _>>()?;
            if entries.is_empty()
                || entries.iter().copied().collect::<BTreeSet<_>>().len() != entries.len()
            {
                return Err("--entries must contain distinct non-negative indices".into());
            }
            args.entries = Some(entries);
        }
        _ => return Err(format!("unknown argument {flag}")),
    }
    Ok(())
}

fn validate(args: &Args) -> Result<(), String> {
    if args.database.as_os_str().is_empty() || args.output.as_os_str().is_empty() {
        return Err("--database and --output are required".into());
    }
    if !(1..=crate::runner::MAX_WORKERS).contains(&args.workers)
        || args.iterations == 0
        || !(1..=16).contains(&args.repeats)
        || args.batch_size == 0
        || args.mini_batch == 0
        || args.mini_batch > args.batch_size
        || args.updates_per_round == 0
        || !(1..=64).contains(&args.label_every)
        || args.max_seconds == Some(0)
        || !(args.max_step_deg.is_finite() && args.max_step_deg > 0.0)
        || args.warmup.checked_add(args.iterations).is_none()
    {
        return Err("invalid limits: counts must be positive (warmup may be zero), repeats <=16, mini-batch <= batch-size, workers <=1024, label-every <=64, finite positive step".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(extra: &[&str]) -> Vec<String> {
        ["--database", "library.sqlite", "--output", "new-output"]
            .into_iter()
            .chain(extra.iter().copied())
            .map(str::to_owned)
            .collect()
    }
    #[test]
    fn defaults_match_the_production_case_without_validation() {
        let parsed = parse(&args(&[])).unwrap().unwrap();
        let cfg = parsed.config();
        assert_eq!(
            (parsed.warmup, parsed.iterations, cfg.rollout_size),
            (1, 2, 256)
        );
        assert_eq!(
            (
                cfg.imitation.mini_batch_size,
                cfg.imitation.updates_per_round
            ),
            (128, 16)
        );
        assert_eq!((cfg.seed, cfg.label_every), (42, 2));
        assert_eq!(cfg.device, DeviceChoice::Cuda);
        assert_eq!(
            cfg.imitation.lr_schedule,
            LrSchedule::Linear {
                total_updates: 4800
            }
        );
        assert_eq!(cfg.algo, Algorithm::Imitation);
        assert_eq!(cfg.objectives, ObjectiveSampling::Mixed);
        assert!(!parsed.sync_every_iteration);
    }
    #[test]
    fn strict_backend_and_invalid_or_duplicate_inputs_are_rejected() {
        for flags in [
            vec!["--device", "auto"],
            vec!["--entries", "2,2"],
            vec!["--max-step-deg", "NaN"],
            vec!["--iterations", "0"],
            vec!["--repeats", "17"],
        ] {
            assert!(parse(&args(&flags)).is_err());
        }
        let parsed = parse(&args(&[
            "--device",
            "cpu",
            "--scenario",
            "projection",
            "--entries",
            "69,215,283",
            "--sync-every-iteration",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.entries, Some(vec![69, 215, 283]));
        assert!(parsed.sync_every_iteration);
    }
}
