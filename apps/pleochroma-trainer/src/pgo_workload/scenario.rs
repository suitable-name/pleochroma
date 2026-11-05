//! Fresh runs and recovery continuations using the same executable's normal CLI.

use std::{fs, path::Path};

use serde::Serialize;

use super::{Result, args::Config, capture::Capture, files, process, verify};

#[derive(Serialize)]
pub(super) struct Report {
    mode: String,
    algorithm: String,
    fresh: verify::Completed,
    fresh_wall_seconds: f64,
    resumed: verify::Completed,
    resume_wall_seconds: f64,
    restored_iteration: usize,
    resumed_iterations: usize,
}

pub(super) fn run(
    binary: &Path,
    config: &Config,
    mode: &str,
    algorithm: &str,
    seed: usize,
) -> Result<Report> {
    let directory = config.output_dir.join(format!("{mode}-{algorithm}"));
    fs::create_dir(&directory)?;
    let mut capture = Capture::new(&directory, config.iterations);
    let fresh_arguments = arguments(config, &directory, mode, algorithm, seed)?;
    let fresh_wall_seconds = process::invoke(
        binary,
        &fresh_arguments,
        &directory,
        "fresh",
        config,
        Some(&mut capture),
    )?;
    let fresh = verify::complete(&directory, config, mode, algorithm)?;
    files::archive_json(&directory)?;
    let restored_iteration = capture.restore()?;
    let resume_arguments = vec![
        "--device".to_owned(),
        config.device.argument().to_owned(),
        "--resume".to_owned(),
        files::text(&directory.join("run"))?,
    ];
    let resume_wall_seconds = process::invoke(
        binary,
        &resume_arguments,
        &directory,
        "resume",
        config,
        None,
    )?;
    let resumed = verify::complete(&directory, config, mode, algorithm)?;
    let log = fs::read_to_string(directory.join("resume.log"))?;
    let restored_line = format!("Restored full training state at iteration {restored_iteration}/");
    if !log.contains(&restored_line) {
        return Err("resume did not report restoration of the captured training state".into());
    }
    Ok(Report {
        mode: mode.to_owned(),
        algorithm: algorithm.to_owned(),
        fresh,
        fresh_wall_seconds,
        resumed,
        resume_wall_seconds,
        restored_iteration,
        resumed_iterations: config.iterations - restored_iteration,
    })
}

fn arguments(
    config: &Config,
    directory: &Path,
    mode: &str,
    algorithm: &str,
    seed: usize,
) -> Result<Vec<String>> {
    let fidelity = if config.full_fidelity { "full" } else { "fast" };
    let label_every = if algorithm == "imitation" { "2" } else { "0" };
    let mut args: Vec<String> = [
        "--no-update-check",
        "--device",
        config.device.argument(),
        "--mode",
        mode,
        "--algo",
        algorithm,
        "--tier",
        "m",
        "--epochs",
        "2",
        "--updates-per-round",
        "4",
        "--label-capacity",
        "1024",
        "--label-every",
        label_every,
        "--max-rejected-steps",
        "8",
        "--objectives",
        "mixed",
        "--same-material-frac",
        "0.25",
        "--train-fidelity",
        fidelity,
        "--full-fidelity-frac",
        "0",
        "--validation-fidelity",
        fidelity,
        "--save-every",
        "1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (flag, value) in [
        ("--iterations", config.iterations),
        ("--workers", config.workers),
        ("--batch-size", config.batch_size),
        ("--mini-batch", config.mini_batch),
        ("--max-tiers", config.max_tiers),
        ("--seed", seed),
        ("--validation-designs", config.validation_designs),
        ("--validation-every", config.iterations.min(1000)),
    ] {
        args.extend([flag.to_owned(), value.to_string()]);
    }
    args.extend([
        "--save-stats".to_owned(),
        files::text(&directory.join("run.json"))?,
        "--save-weights".to_owned(),
        files::text(&directory.join("run.ckpt.json"))?,
    ]);
    if let Some(design_dir) = &config.design_dir {
        args.extend([
            "--design-dir".to_owned(),
            files::text(design_dir)?,
            "--include-library".to_owned(),
        ]);
    }
    Ok(args)
}
