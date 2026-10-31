//! Opt-in latency harness around the desktop's real progressive render stages.
//!
//! The harness serializes display processing to make phase attribution readable.
//! It deliberately does not start Slint's event loop, metrics workers or a database.
//! A completed image here is a CPU-side `SharedPixelBuffer`, not an on-screen frame.

mod cancellation;
mod config;
mod display;
mod scene;
mod session;

use anyhow::{Context, Result};
use config::Config;
use pleochroma::{optics::materials::GemMaterial, renderer::engine::EngineSpec};
use serde::Serialize;
use serde_json::{Value, json};
use session::Session;
use std::{io::Write, time::Instant};

#[derive(Clone, Copy, Serialize)]
struct Case {
    engine: EngineSpec,
    width: u32,
    height: u32,
    path_target: u32,
    denoise: bool,
    repeat: u32,
}

/// Run the headless benchmark using command-line arguments; `--help` lists options.
///
/// # Errors
/// Returns argument, material, exported-file or report-write errors. Individual
/// render errors are saved in the report before returning an unsuccessful result.
pub fn run() -> Result<()> {
    let Some(config) = Config::parse()? else {
        return Ok(());
    };
    // The phase observer emits begin/end events *inside* otherwise opaque dispatch.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            "warn,pleochroma_cut::render_bench=info,pleochroma::renderer::gpu::compute=info",
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    let material = GemMaterial::by_name(&config.material)
        .with_context(|| format!("unknown built-in material {:?}", config.material))?;
    let pipeline_cache_mode = configure_pipeline_cache(&config)?;
    let mut report = json!({
        "schema_version": 1,
        "package_version": env!("CARGO_PKG_VERSION"),
        "gpu_feature": cfg!(feature = "gpu"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "available_parallelism": std::thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        "configuration": &config,
        "pipeline_cache_mode": pipeline_cache_mode,
        "limitations": [
            "Image-ready means CPU-side SharedPixelBuffer; no UI event loop, presentation, DB loading, metrics or remote workers.",
            "Display is serialized after each pass; real desktop tracing/display overlap and cadence are excluded.",
            "Requested final resolution is measured; the desktop's reduced-resolution initial preview is excluded.",
            "Ray visibility diagnostics run after timed rendering and are reported separately; acquisition is now lazy within first-use tracing.",
            "First use is a new backend/session, not a cleared driver or disk shader cache. Later sessions share process/driver caches.",
            "Time budget is soft: checked between passes; one acquisition, trace or display may overrun it.",
            "File inputs supply geometry/tools only; the selected built-in material, default pose/lighting, polished surfaces and no effects are used.",
            "Seed is the Path absolute sample-index base; Ray uses fixed deterministic AA taps starting at zero.",
            "Adaptive PassBudget uses observed timings: identical seeds do not guarantee identical later pass sizes across runs.",
            "Stage logging adds overhead to elapsed case latency; no values claim actual on-screen latency."
        ],
        "cases": [],
        "status": "running"
    });
    save(&config, &report)?;
    let mut failures = 0;
    for case in cases(&config) {
        failures += run_sequence(&config, &material, case, &mut report)?;
    }
    report["status"] = json!(if failures == 0 {
        "complete"
    } else {
        "completed_with_errors"
    });
    save(&config, &report)?;
    anyhow::ensure!(
        failures == 0,
        "{failures} benchmark cases failed; see {}",
        config.json.display()
    );
    eprintln!("Report: {}", config.json.display());
    Ok(())
}

fn cases(config: &Config) -> Vec<Case> {
    let mut cases = Vec::new();
    for repeat in 0..config.repeats {
        for &engine in &config.engines {
            for &(width, height) in &config.resolutions {
                // Ray target/denoiser are independent of the Path quality controls.
                let targets = if engine.is_ray() {
                    &config.path_targets[..1]
                } else {
                    &config.path_targets
                };
                let denoise = if engine.is_ray() {
                    &[false][..]
                } else {
                    &config.denoise
                };
                for &path_target in targets {
                    for &denoise in denoise {
                        cases.push(Case {
                            engine,
                            width,
                            height,
                            path_target,
                            denoise,
                            repeat,
                        });
                    }
                }
            }
        }
    }
    cases
}

fn configure_pipeline_cache(config: &Config) -> Result<&'static str> {
    #[cfg(feature = "gpu")]
    if let Some(path) = &config.pipeline_cache_dir {
        anyhow::ensure!(
            pleochroma::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(path.clone()),
            "pipeline cache directory was already configured in this process"
        );
        return Ok("directory_configured_backend_may_not_support_persistence");
    }
    anyhow::ensure!(
        config.pipeline_cache_dir.is_none(),
        "pipeline cache requires gpu feature"
    );
    Ok("application_persistence_disabled_driver_cache_unchanged")
}

fn run_sequence(
    config: &Config,
    material: &GemMaterial,
    case: Case,
    report: &mut Value,
) -> Result<usize> {
    let id_base = report["cases"]
        .as_array()
        .context("report cases array")?
        .len();
    report["active_stage"] = json!({ "case_id": id_base, "stage": "session_acquire" });
    save(config, report)?;
    let (mut session, setup_ms) = measure(id_base, 0, "session_acquire", Session::new);
    let stages: &[(bool, &str)] = match config.scenarios.as_str() {
        "cold" => &[(false, "first_use_a")],
        "warm" => &[(false, "warmup_a"), (true, "warm_b"), (false, "warm_a")],
        _ => &[(false, "first_use_a"), (true, "warm_b"), (false, "warm_a")],
    };
    let mut failures = 0;
    for (index, &(second, role)) in stages.iter().enumerate() {
        let id = id_base + index;
        report["active_stage"] = json!({ "case_id": id, "role": role, "stage": "case_running" });
        save(config, report)?;
        let start = Instant::now();
        let path = if second {
            config.design_b.as_deref()
        } else {
            config.design_a.as_deref()
        };
        let (scene, prepare_ms) = measure(id, 0, "scene_load_and_geometry_prepare", || {
            scene::Scene::load(path, second)
        });
        let result =
            scene.and_then(|scene| session.run_case(config, &case, &scene, material, id, start));
        let result = match result {
            Ok(mut result) => {
                result["scene_prepare_ms"] = json!(prepare_ms);
                result["setup_acquire_ms"] = json!(if index == 0 { setup_ms } else { 0.0 });
                if index == 0 {
                    result["first_image_ready_including_acquire_ms"] =
                        result["first_image_ready_ms"]
                            .as_f64()
                            .map(|ms| ms + setup_ms)
                            .map_or(Value::Null, |ms| json!(ms));
                }
                result
            }
            Err(error) => {
                failures += 1;
                json!({ "error": format!("{error:#}") })
            }
        };
        let entry = json!({ "case_id": id, "role": role, "input": case, "result": result });
        report["cases"]
            .as_array_mut()
            .context("report cases array")?
            .push(entry);
        report["active_stage"] = Value::Null;
        save(config, report)?;
    }
    report["active_stage"] = json!({ "case_id": id_base, "stage": "drain_background_preparation" });
    save(config, report)?;
    let ((), drain_ms) = measure(id_base, 0, "drain_background_preparation", || {
        session.drain_preparation();
    });
    report["last_sequence_drain_ms"] = json!(drain_ms);
    report["active_stage"] = Value::Null;
    save(config, report)?;
    Ok(failures)
}

fn save(config: &Config, report: &Value) -> Result<()> {
    let file = std::fs::File::create(&config.json)
        .with_context(|| format!("writing {}", config.json.display()))?;
    let mut output = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut output, report)?;
    output.flush()?;
    Ok(())
}

fn measure<T>(case_id: usize, pass: usize, stage: &str, work: impl FnOnce() -> T) -> (T, f64) {
    progress(&json!({ "case_id": case_id, "pass": pass, "stage": stage, "event": "begin" }));
    let start = Instant::now();
    let result = work();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    progress(
        &json!({ "case_id": case_id, "pass": pass, "stage": stage, "event": "end", "elapsed_ms": ms }),
    );
    (result, ms)
}

fn progress(value: &Value) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{value}");
    let _ = stderr.flush();
}
