//! Internal profiling driver for real training, validation and recovery runs.

mod args;
mod capture;
mod files;
mod process;
mod scenario;
mod verify;

use std::{error::Error, fs, path::PathBuf, process::ExitCode};

use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Serialize)]
struct Summary<'a> {
    schema: u32,
    binary: PathBuf,
    database: &'a std::path::Path,
    database_bytes: u64,
    configuration: &'a args::Config,
    cases: Vec<scenario::Report>,
    status: &'static str,
    error: Option<String>,
}

pub(super) fn main(raw: &[String]) -> ExitCode {
    if matches!(raw, [help] if help == "--help" || help == "-h") {
        print!("{}", args::HELP);
        return ExitCode::SUCCESS;
    }
    let config = match args::parse(raw) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("profiling usage error: {error}\n\n{}", args::HELP);
            return ExitCode::from(2);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("profiling workload failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut config: args::Config) -> Result<()> {
    config.create_output()?;
    let binary = fs::canonicalize(std::env::current_exe()?)?;
    let summary_path = config.output_dir.join("workload-summary.json");
    let mut summary = Summary {
        schema: 1,
        binary,
        database: &config.db,
        database_bytes: fs::metadata(&config.db)?.len(),
        configuration: &config,
        cases: Vec::new(),
        status: "running",
        error: None,
    };
    files::write_json(&summary_path, &summary)?;
    let result = run_cases(&mut summary, &summary_path);
    match &result {
        Ok(()) => summary.status = "completed",
        Err(error) => {
            summary.status = "failed";
            summary.error = Some(error.to_string());
        }
    }
    files::write_json(&summary_path, &summary)?;
    result?;
    println!(
        "Completed four training/recovery cases: {}",
        summary_path.display()
    );
    Ok(())
}

fn run_cases(summary: &mut Summary<'_>, summary_path: &std::path::Path) -> Result<()> {
    let cases = [
        ("retarget", "imitation"),
        ("optimizer", "imitation"),
        ("retarget", "ppo"),
        ("optimizer", "ppo"),
    ];
    for (index, (mode, algorithm)) in cases.into_iter().enumerate() {
        let report = scenario::run(
            &summary.binary,
            summary.configuration,
            mode,
            algorithm,
            42 + index,
        )?;
        summary.cases.push(report);
        files::write_json(summary_path, summary)?;
    }
    Ok(())
}
