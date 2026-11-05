//! Hidden profiling options, independent of ordinary training configuration.

use std::{fs, path::PathBuf};

use serde::Serialize;

use super::Result;

pub(super) const HELP: &str = "\
Trainer profiling workload (internal build tooling)

USAGE:
  pleochroma-trainer-cli --pgo --db PATH --output-dir NEW_DIRECTORY [OPTIONS]

  --device cuda|cpu|gpu       Actual training backend (default: cuda)
  --workers N                Rollout slots, 1-1024 (default: 4)
  --batch-size N             Steps per rollout, 16-4096 (default: 32)
  --mini-batch N             Update rows, 8-1024, at most batch size (default: 16)
  --iterations N             Iterations per case, 2-1000000 (default: 2)
  --validation-designs N     Held-out designs, 1-64 (default: 1)
  --max-tiers N              Design tier cap, 1-128 (default: 128)
  --design-dir PATH          Also load authored/native designs
  --full-fidelity            Profile Full training/validation (much slower)
  --help                    Show this internal help

Runs four actual CLI training cases and their recovery continuations. Outputs must
go to a new directory. Work is finite, but has no wall-clock deadline. Compiler
instrumentation environment is inherited by children. No existing run is resumed.
";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Device {
    Cpu,
    Cuda,
    Gpu,
}

impl Device {
    pub(super) const fn argument(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Gpu => "gpu",
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Cpu => crate::runner::CPU_LABEL,
            Self::Cuda => crate::runner::CUDA_LABEL,
            Self::Gpu => crate::runner::GPU_LABEL,
        }
    }
}

#[derive(Serialize)]
pub(super) struct Config {
    pub(super) db: PathBuf,
    pub(super) output_dir: PathBuf,
    pub(super) device: Device,
    pub(super) design_dir: Option<PathBuf>,
    pub(super) workers: usize,
    pub(super) batch_size: usize,
    pub(super) mini_batch: usize,
    pub(super) iterations: usize,
    pub(super) validation_designs: usize,
    pub(super) max_tiers: usize,
    pub(super) full_fidelity: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db: PathBuf::new(),
            output_dir: PathBuf::new(),
            device: Device::Cuda,
            design_dir: None,
            workers: 4,
            batch_size: 32,
            mini_batch: 16,
            iterations: 2,
            validation_designs: 1,
            max_tiers: 128,
            full_fidelity: false,
        }
    }
}

pub(super) fn parse(raw: &[String]) -> Result<Config> {
    let mut config = Config::default();
    let mut args = raw.iter();
    while let Some(flag) = args.next() {
        if flag == "--full-fidelity" {
            config.full_fidelity = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        match flag.as_str() {
            "--db" => config.db = PathBuf::from(value),
            "--output-dir" => config.output_dir = PathBuf::from(value),
            "--design-dir" => config.design_dir = Some(PathBuf::from(value)),
            "--device" => {
                config.device = match value.as_str() {
                    "cpu" => Device::Cpu,
                    "cuda" => Device::Cuda,
                    "gpu" => Device::Gpu,
                    _ => return Err("--device must be cuda, cpu or gpu".into()),
                };
            }
            "--workers" => config.workers = number(flag, value, 1, 1024)?,
            "--batch-size" => config.batch_size = number(flag, value, 16, 4096)?,
            "--mini-batch" => config.mini_batch = number(flag, value, 8, 1024)?,
            "--iterations" => config.iterations = number(flag, value, 2, 1_000_000)?,
            "--validation-designs" => config.validation_designs = number(flag, value, 1, 64)?,
            "--max-tiers" => config.max_tiers = number(flag, value, 1, 128)?,
            _ => return Err(format!("Unknown profiling option: {flag}").into()),
        }
    }
    config.validate_inputs()?;
    Ok(config)
}

fn number(flag: &str, text: &str, low: usize, high: usize) -> Result<usize> {
    let value: usize = text.parse()?;
    if !(low..=high).contains(&value) {
        return Err(format!("{flag} must be between {low} and {high}").into());
    }
    Ok(value)
}

impl Config {
    fn validate_inputs(&mut self) -> Result<()> {
        if self.db.as_os_str().is_empty() || self.output_dir.as_os_str().is_empty() {
            return Err("--db and --output-dir are required".into());
        }
        if self.mini_batch > self.batch_size {
            return Err("--mini-batch must not exceed --batch-size".into());
        }
        self.iterations
            .checked_mul(self.batch_size)
            .ok_or("step count overflow")?;
        self.db = fs::canonicalize(&self.db)?;
        if !self.db.is_file() {
            return Err("--db must name a regular database file".into());
        }
        if let Some(directory) = &mut self.design_dir {
            *directory = fs::canonicalize(&*directory)?;
            if !directory.is_dir() {
                return Err("--design-dir must name a directory".into());
            }
        }
        self.output_dir = std::path::absolute(&self.output_dir)?;
        Ok(())
    }

    pub(super) fn create_output(&mut self) -> Result<()> {
        if let Some(parent) = self.output_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        // This rejects existing directories and symlinks, even if apparently empty.
        fs::create_dir(&self.output_dir)?;
        self.output_dir = fs::canonicalize(&self.output_dir)?;
        Ok(())
    }
}
