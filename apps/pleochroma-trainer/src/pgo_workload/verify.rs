//! Evidence that profiling covered completed training, updates and validation.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    Result,
    args::Config,
    capture::{Manifest, State},
    files,
};

#[derive(Deserialize)]
struct Stats {
    total_iterations: usize,
    total_steps: usize,
    device: String,
    mode: String,
    algo: String,
    metrics_history: Vec<Metric>,
}

#[derive(Deserialize)]
struct Metric {
    val_designs: usize,
    val_score: Option<f64>,
    action_samples: usize,
}

#[derive(Deserialize)]
struct Progress {
    status: String,
}

#[derive(Serialize)]
pub(super) struct Completed {
    iterations: usize,
    steps: usize,
    backend: String,
    validation_designs: usize,
    imitation_action_samples: usize,
}

pub(super) fn complete(
    directory: &Path,
    config: &Config,
    mode: &str,
    algorithm: &str,
) -> Result<Completed> {
    let stats: Stats = files::read_json(&directory.join("run.json"))?;
    let manifest: Manifest = files::read_json(&directory.join("run.latest.json"))?;
    manifest.validate()?;
    let root = directory.join("run.run");
    files::regular_directory(&root)?;
    let generation = root.join(&manifest.generation);
    files::regular_directory(&generation)?;
    let state: State = files::read_json(&generation.join("resume.json"))?;
    let progress: Progress = files::read_json(&directory.join("run.json.progress.json"))?;
    if stats.total_iterations != config.iterations
        || stats.total_steps != config.iterations * config.batch_size
        || manifest.iteration != config.iterations
        || state.iteration != config.iterations
        || state.phase != "Complete"
        || progress.status != "completed"
    {
        return Err("training did not complete its requested profiling work".into());
    }
    if stats.device != config.device.label()
        || stats.mode.to_lowercase() != mode
        || stats.algo != algorithm
    {
        return Err("actual training configuration differs from the profiling request".into());
    }
    let last = stats
        .metrics_history
        .last()
        .ok_or("missing iteration metrics")?;
    if last.val_designs == 0 || last.val_score.is_none_or(|score| !score.is_finite()) {
        return Err("no completed held-out validation; use a larger profiling corpus".into());
    }
    if algorithm == "imitation"
        && !stats
            .metrics_history
            .iter()
            .any(|point| point.action_samples > 0)
    {
        return Err(
            "no verified action labels; increase profiling corpus/workload coverage".into(),
        );
    }
    files::regular_file(&directory.join("run.ckpt.json"))?;
    Ok(Completed {
        iterations: stats.total_iterations,
        steps: stats.total_steps,
        backend: stats.device,
        validation_designs: last.val_designs,
        imitation_action_samples: last.action_samples,
    })
}
