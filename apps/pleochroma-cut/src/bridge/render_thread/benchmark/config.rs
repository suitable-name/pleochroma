//! Small explicit CLI: lists expand only when requested, keeping the default short.

use crate::settings::model::LocalComputeTarget;
use anyhow::{Context, Result, bail, ensure};
use pleochroma::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
use serde::Serialize;
use std::path::PathBuf;

pub(super) const HELP: &str = "live_render_bench [options]
  --engines ray|path|both          Default ray
  --resolutions 800x600,1920x1080  Default 800x600
  --path-targets 128,1024          Default 128; Ray target is --ray-aa
  --ray-aa 1|4|8|16               Default 8, fire enabled
  --backend cpu|gpu|cpu-gpu       Default cpu-gpu (GPU may decline)
  --denoise on|off|both           Default on; Ray never denoises
  --scenarios cold|warm|both      Default both: fresh A, reused B, reused A
  --passes N                     Default 3 complete passes per case
  --case-seconds N                Default 10; soft limit checked between passes
  --full-convergence              Ignore pass count, still respect time limit
  --case-seconds 0                Disable time limit explicitly
  --repeats N                     Default 1 independent A/B/A sequences
  --seed N                        Default 0; Path absolute sample-index base
  --cancel-after-ms N             Cancel first trace pass, record drain time
  --material NAME                 Default Diamond; exact built-in material name
  --max-bounces N                 Default desktop DEFAULT_MAX_BOUNCES
  --design-a PATH                 Optional .asc/.psdd geometry instead of round
  --design-b PATH                 Optional .asc/.psdd geometry instead of emerald
  --json PATH                     Default live-render-bench.json
  --pipeline-cache-dir PATH       Opt in to persistent GPU pipeline cache (gpu build)
  --help

Geometry files supply geometry/tools only; material, frosting and effects are
controlled benchmark inputs, not restored from files. No database is opened.
First use means a fresh harness/backend, NOT an empty driver/disk shader cache.
Time limits cannot interrupt a single acquisition, trace or display operation.
Measures synchronous image-ready work, NOT an on-screen/UI-event-loop latency.
";

#[derive(Serialize)]
pub(super) struct Config {
    pub engines: Vec<EngineSpec>,
    pub resolutions: Vec<(u32, u32)>,
    pub path_targets: Vec<u32>,
    pub backend: LocalComputeTarget,
    pub denoise: Vec<bool>,
    pub scenarios: String,
    pub passes: usize,
    pub case_seconds: f64,
    pub full_convergence: bool,
    pub repeats: u32,
    pub seed: u32,
    pub cancel_after_ms: Option<u64>,
    pub material: String,
    pub max_bounces: u32,
    pub design_a: Option<PathBuf>,
    pub design_b: Option<PathBuf>,
    pub json: PathBuf,
    pub pipeline_cache_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engines: vec![EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions::default(),
            }],
            resolutions: vec![(800, 600)],
            path_targets: vec![128],
            backend: LocalComputeTarget::CpuGpu,
            denoise: vec![true],
            scenarios: "both".into(),
            passes: 3,
            case_seconds: 10.0,
            full_convergence: false,
            repeats: 1,
            seed: 0,
            cancel_after_ms: None,
            material: "Diamond".into(),
            max_bounces: pleochroma::optics::raytracer::DEFAULT_MAX_BOUNCES,
            design_a: None,
            design_b: None,
            json: "live-render-bench.json".into(),
            pipeline_cache_dir: None,
        }
    }
}

impl Config {
    pub fn parse() -> Result<Option<Self>> {
        let mut config = Self::default();
        let mut args = std::env::args().skip(1);
        let mut ray_aa = 8;
        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                print!("{HELP}");
                return Ok(None);
            }
            if flag == "--full-convergence" {
                config.full_convergence = true;
                continue;
            }
            let value = args
                .next()
                .with_context(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--engines" => config.engines = parse_engines(&value)?,
                "--resolutions" => config.resolutions = parse_resolutions(&value)?,
                "--path-targets" => {
                    config.path_targets =
                        value.split(',').map(str::parse).collect::<Result<_, _>>()?;
                }
                "--ray-aa" => ray_aa = value.parse()?,
                "--backend" => config.backend = parse_backend(&value)?,
                "--denoise" => {
                    config.denoise = match value.as_str() {
                        "on" => vec![true],
                        "off" => vec![false],
                        "both" => vec![false, true],
                        _ => bail!("--denoise must be on, off or both"),
                    };
                }
                "--scenarios" => {
                    ensure!(
                        ["cold", "warm", "both"].contains(&value.as_str()),
                        "invalid scenarios"
                    );
                    config.scenarios = value;
                }
                "--passes" => config.passes = value.parse()?,
                "--case-seconds" => config.case_seconds = value.parse()?,
                "--repeats" => config.repeats = value.parse()?,
                "--seed" => config.seed = value.parse()?,
                "--cancel-after-ms" => config.cancel_after_ms = Some(value.parse()?),
                "--material" => config.material = value,
                "--max-bounces" => config.max_bounces = value.parse()?,
                "--design-a" => config.design_a = Some(value.into()),
                "--design-b" => config.design_b = Some(value.into()),
                "--json" => config.json = value.into(),
                "--pipeline-cache-dir" => config.pipeline_cache_dir = Some(value.into()),
                _ => bail!("unknown option {flag}; use --help"),
            }
        }
        ensure!([1, 4, 8, 16].contains(&ray_aa), "--ray-aa must be 1,4,8,16");
        for engine in &mut config.engines {
            engine.ray.aa_taps = ray_aa;
        }
        ensure!(
            config.passes > 0 && config.repeats > 0,
            "passes and repeats must be positive"
        );
        ensure!(config.max_bounces > 0, "max bounces must be positive");
        ensure!(
            cfg!(feature = "gpu") || config.pipeline_cache_dir.is_none(),
            "--pipeline-cache-dir requires the gpu feature"
        );
        ensure!(
            config.case_seconds.is_finite() && config.case_seconds >= 0.0,
            "invalid case seconds"
        );
        for &target in &config.path_targets {
            ensure!(
                target > 0 && config.seed.checked_add(target).is_some(),
                "invalid target or seed overflow"
            );
        }
        Ok(Some(config))
    }
}

fn parse_engines(value: &str) -> Result<Vec<EngineSpec>> {
    let engines = match value {
        "ray" => vec![RenderEngine::RayTraced],
        "path" => vec![RenderEngine::PathTraced],
        "both" => vec![RenderEngine::RayTraced, RenderEngine::PathTraced],
        _ => bail!("--engines must be ray, path or both"),
    };
    Ok(engines
        .into_iter()
        .map(|engine| EngineSpec {
            engine,
            ray: RayOptions::default(),
        })
        .collect())
}

fn parse_backend(value: &str) -> Result<LocalComputeTarget> {
    match value {
        "cpu" => Ok(LocalComputeTarget::Cpu),
        "gpu" => Ok(LocalComputeTarget::Gpu),
        "cpu-gpu" => Ok(LocalComputeTarget::CpuGpu),
        _ => bail!("--backend must be cpu, gpu or cpu-gpu"),
    }
}

fn parse_resolutions(value: &str) -> Result<Vec<(u32, u32)>> {
    value
        .split(',')
        .map(|item| {
            let (width, height) = item
                .split_once('x')
                .context("resolution must be WIDTHxHEIGHT")?;
            let (width, height): (u32, u32) = (width.parse()?, height.parse()?);
            ensure!(
                width > 0 && height > 0 && width.checked_mul(height).is_some(),
                "invalid dimensions"
            );
            let count = usize::try_from(u64::from(width) * u64::from(height))?;
            ensure!(
                count.checked_mul(64).is_some(),
                "resolution buffers overflow address space"
            );
            Ok((width, height))
        })
        .collect()
}
