//! Command-line trainer for the Pleochroma RL policies.
//!
//! Headless counterpart of the GUI: the same `TrainingRunner` (supervised imitation of the
//! finite-difference teacher by default, PPO on request; vectorized environments, held-out
//! validation with a model, an oracle and a sampled pass, schema-versioned checkpoints), driven by
//! flags and reporting on stdout. Exports the telemetry (`TrainingRunStats`) the GUI can import.
//!
//! The flags live in `cli_args.rs` and everything the run prints in `cli_report.rs`.
//!
//! Exit status: `0` on success, `1` when the run fails, `2` for a usage error, `130` when
//! SIGINT/SIGTERM stopped the run (after committing it on the first signal, at once on the
//! second; `--resume` continues it either way).

mod cli_args;
mod cli_console;
mod cli_dataset;
mod cli_live_progress;
mod cli_report;
mod cli_resume;
mod cli_run;
mod cli_run_reporting;
mod dataset;
mod pgo_workload;
#[path = "runner.rs"]
mod runner;
mod stats;

use std::{
    collections::BTreeMap,
    error::Error,
    path::Path,
    process::ExitCode,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use cli_args::{CliArgs, HELP, Parsed, parse_args};
use cli_console::Console;
use dataset::{DEFAULT_SPLIT_SEED, DatasetSplit, load_db_designs_from};
use pleochroma_update::{NO_UPDATE_CHECK_ENV, cli_notice, take_update_check};
use runner::{EvalFidelity, EvalPasses, SKIPPED_PREFIX, TrainingRunner, evaluation_threads};
use stats::{EvaluationSnapshot, describe_drops, rfc3339_utc_now, save_evaluations};

/// Skip warnings printed in full before the console switches to one summary line per
/// [`SKIP_SUMMARY_EVERY`] further skips (a library holds hundreds of dead stones).
const SKIP_LINES_FULL: usize = 10;
/// Further skips summarised by one console line.
const SKIP_SUMMARY_EVERY: usize = 50;
fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--pgo") {
        return pgo_workload::main(&args[1..]);
    }
    // The new-version check is the very first output: one `info:` line when a newer release
    // exists, nothing when up to date, switched off or on any failure.
    let skip = std::env::var_os(NO_UPDATE_CHECK_ENV);
    if take_update_check(&mut args, skip.as_deref())
        && let Some(line) = cli_notice("pleochroma-trainer-cli", env!("CARGO_PKG_VERSION"))
    {
        println!("info: {line}");
    }
    match parse_args(&args) {
        Ok(Parsed::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run(parsed)) => match with_heartbeat("recovery checkpoint verification", || {
            cli_resume::prepare(*parsed, &args)
        })
        .and_then(|(args, mut recovery)| cli_run::run(&args, &mut recovery))
        {
            Ok(cli_run::RunOutcome::Completed) => ExitCode::SUCCESS,
            Ok(cli_run::RunOutcome::Stopped) => ExitCode::from(cli_run::STOPPED_EXIT_STATUS),
            Err(e) => {
                eprintln!("\nerror: {e}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("error: {message}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

/// Evaluates the held-out test split and prints the before/after figures. The oracle pass costs
/// a probe per free tier and step, so it runs at fast fidelity only.
fn evaluate_test_split(
    runner: &TrainingRunner,
    args: &CliArgs,
    console: &Console,
) -> Result<runner::EvalReport, Box<dyn Error>> {
    let entries = &runner.split().test;
    let fast = args.eval_fidelity == EvalFidelity::Fast;
    console.message(&format!(
        "\nTest split: {} designs, {} optics{}",
        entries.len(),
        if fast { "fast" } else { "full-sweep" },
        if fast {
            ""
        } else {
            " (model and sampled pass; no oracle pass)"
        }
    ));
    // The designs are played in parallel on the CPU (the solver, mesh and tracer behind every
    // environment step are CPU code, the network forward is negligible), one pass per design.
    let started = Instant::now();
    let passes = EvalPasses {
        sampled: true,
        oracle: fast,
        noise: runner.config().algo == runner::Algorithm::Imitation,
    };
    let report = runner.evaluate(entries, args.eval_fidelity, args.seed, passes)?;
    console.message(&format!(
        "  evaluated in {:.1}s on {} threads",
        started.elapsed().as_secs_f32(),
        evaluation_threads(entries.len())
    ));
    if report.designs == 0 {
        console.message(&format!(
            "  no design could be evaluated ({})",
            report.first_failure.as_deref().unwrap_or_default()
        ));
    } else {
        console.message(&format!(
            "  score before {:.2} -> after {:.2}, {}, {:.2} evals/step",
            report.mean_base_score,
            report.mean_final_score,
            describe_drops(
                report.mean_improvement,
                report.mean_improvement_oracle,
                report.mean_improvement_sampled
            ),
            report.mean_evaluations_per_step
        ));
        console.message(&format!(
            "  brilliance {:.1}% -> {:.1}%, windowing {:.1}% -> {:.1}%, extinction {:.1}% -> {:.1}%",
            report.base_optics.brilliance,
            report.final_optics.brilliance,
            report.base_optics.windowing,
            report.final_optics.windowing,
            report.base_optics.extinction,
            report.final_optics.extinction
        ));
        if report.failed > 0 {
            console.message(&format!(
                "  {} designs could not be evaluated",
                report.failed
            ));
        }
    }
    Ok(report)
}

/// Caps the console noise of skipped designs: the first [`SKIP_LINES_FULL`] skips are shown in
/// full, every [`SKIP_SUMMARY_EVERY`] further ones as one summary line naming the latest. The
/// total is kept for the end-of-run summary.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct SkipLog {
    /// Skip warnings seen so far.
    total: usize,
    /// Skips since the last summary line.
    pending: usize,
}

impl SkipLog {
    /// The line to show for the skip warning with `detail` (`'<title>': <reason>`), if any.
    fn line(&mut self, detail: &str) -> Option<String> {
        self.total += 1;
        if self.total <= SKIP_LINES_FULL {
            return Some(format!("{SKIPPED_PREFIX}{detail}"));
        }
        self.pending += 1;
        if self.pending < SKIP_SUMMARY_EVERY {
            return None;
        }
        self.pending = 0;
        Some(format!(
            "… {SKIP_SUMMARY_EVERY} more designs skipped (last: {detail})"
        ))
    }
}

/// Drains the runner's warnings and returns the lines to show: device fallbacks and the like
/// as they are, skipped designs through the cap of `skips`.
fn warning_lines(runner: &mut TrainingRunner, skips: &mut SkipLog) -> Vec<String> {
    runner
        .take_warnings()
        .into_iter()
        .filter_map(|warning| {
            if let Some(detail) = warning.strip_prefix(SKIPPED_PREFIX) {
                return skips.line(detail);
            }
            Some(warning)
        })
        .collect()
}

/// The data set the run trains on; designs skipped at load time are listed on stderr.
fn load_split(args: &CliArgs, database: Option<&Path>) -> Result<DatasetSplit, Box<dyn Error>> {
    let split = if args.fixture {
        DatasetSplit::fixture()
    } else {
        let mut records = if args.design_dir.is_none() || args.include_library {
            load_db_designs_from(database.ok_or("the run has no saved database path")?)?
        } else {
            Vec::new()
        };
        if let Some(directory) = &args.design_dir {
            records.extend(cli_dataset::load_design_directory(Path::new(directory))?);
        }
        DatasetSplit::from_records(&records, DEFAULT_SPLIT_SEED, args.max_tiers)?
    };
    if !split.skipped.is_empty() {
        eprintln!(
            "warning: {} designs skipped at load time",
            split.skipped.len()
        );
        for s in split.skipped.iter().take(5) {
            eprintln!("  '{}': {}", s.title, s.reason);
        }
    }
    let coverage = split.coverage();
    eprintln!(
        "corpus: {} ASC / {} native, {} families, {} with relations, {} concave",
        coverage.asc,
        coverage.native,
        coverage.geometric_families,
        coverage.relations,
        coverage.concave
    );
    eprintln!(
        "corpus context: {} authored widths / {} sampled widths; {} sources eligible for material sampling",
        coverage.authored_physical_size,
        coverage.augmented_physical_size,
        coverage.augmented_source_materials,
    );
    if coverage.native == 0 {
        eprintln!(
            "warning: no native designs; add --design-dir to teach authored relations, dimensional targets and custom materials"
        );
    }
    for note in &coverage.analytical_scope_notes {
        eprintln!("analytical scope: {note}");
    }
    let path = Path::new(&args.stats_path).with_extension("coverage.json");
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&coverage)?)?;
    eprintln!("corpus details: {}", path.display());
    Ok(split)
}

/// Runs the validation pass and returns its report line; `None` when nothing could be
/// validated (the split is empty or no design evaluated).
fn run_validation(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    recovery_directory: &Path,
) -> Result<Option<(String, runner::EvalReport)>, Box<dyn Error>> {
    let designs = runner.config().validation_designs;
    let Some(v) = runner
        .validate_with_recovery(designs, recovery_directory)?
        .filter(|v| !v.cases.is_empty())
    else {
        return Ok(None);
    };
    let gap = runner.generalization_gap().map_or_else(
        || format!("n/a (needs {} designs)", runner::MIN_GAP_DESIGNS),
        |g| format!("{g:+.2}"),
    );
    let failed = if v.failed > 0 {
        format!(", {} designs failed", v.failed)
    } else {
        String::new()
    };
    let cohort_kind = if args.fixture {
        "smoke fixture"
    } else {
        "fixed held-out cohort"
    };
    let score = if v.designs > 0 {
        format!("{:.2}", v.mean_final_score)
    } else {
        "n/a".to_owned()
    };
    let line = format!(
        "validation {cohort_kind}: {} attempted ({} evaluated, {} failed), score {score}, {}, {:.2} evals/step, generalization gap \
         {gap}, brilliance {:.1}%, windowing {:.1}%{failed}",
        v.cases.len(),
        v.designs,
        v.failed,
        describe_drops(
            v.mean_improvement,
            v.mean_improvement_oracle,
            v.mean_improvement_sampled
        ),
        v.mean_evaluations_per_step,
        v.final_optics.brilliance,
        v.final_optics.windowing
    );
    Ok(Some((line, v)))
}

fn with_heartbeat<T>(phase: &'static str, work: impl FnOnce() -> T) -> T {
    let (stop, stopped) = mpsc::channel();
    let heartbeat = thread::spawn(move || {
        while stopped.recv_timeout(Duration::from_secs(10)) == Err(mpsc::RecvTimeoutError::Timeout)
        {
            eprintln!("{phase} still running");
        }
    });
    let result = work();
    let _ = stop.send(());
    let _ = heartbeat.join();
    result
}

fn validate_with_progress(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    recovery_directory: &Path,
) -> Result<Option<(String, runner::EvalReport)>, Box<dyn Error>> {
    runner.progress().phase("validation", 0);
    let result = run_validation(runner, args, recovery_directory);
    if result.is_ok() {
        runner.progress().phase("validation complete", 0);
    }
    result
}

/// The saved record of one evaluation pass.
fn evaluation_snapshot(
    iteration: usize,
    kind: &str,
    report: &runner::EvalReport,
) -> EvaluationSnapshot {
    EvaluationSnapshot {
        iteration,
        kind: kind.to_owned(),
        fidelity: if report.full_fidelity { "full" } else { "fast" }.to_owned(),
        evaluation_revision: 3,
        distinct_entries: report.distinct_entries,
        distinct_families: report.distinct_families,
        noise_drop: report.mean_improvement_noise,
        noise_cached: report.noise_cached,
        cases: report.cases.clone(),
    }
}

/// The log line of a new largest score drop at iteration `iter`.
fn best_record(iter: usize, best: &runner::EpisodeSummary) -> (String, String, String) {
    (
        rfc3339_utc_now(),
        "BEST".to_owned(),
        format!(
            "iteration {iter}: score drop {:.2} (from {:.2} to {:.2})",
            best.improvement(),
            best.base_score,
            best.final_score
        ),
    )
}

fn training_checkpoint_stem(args: &CliArgs) -> String {
    let algorithm = match args.algo {
        runner::Algorithm::Imitation => "imitation",
        runner::Algorithm::Ppo => "ppo",
    };
    format!(
        "pleochroma-{}-{algorithm}-{}",
        args.mode,
        args.tier.code_str().to_ascii_lowercase()
    )
}

fn evaluation_artifact_path(args: &CliArgs) -> std::path::PathBuf {
    let path = Path::new(&args.stats_path);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("training_stats");
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{name}-evaluations.json"))
}

fn save_eval_artifact(args: &CliArgs, evaluations: &[EvaluationSnapshot]) -> std::io::Result<()> {
    save_evaluations(&evaluation_artifact_path(args), evaluations)
}

/// Prints how much of the preset and spectrum space an evaluation covered.
fn print_coverage(report: &runner::EvalReport, group_count: usize, console: &Console) {
    console.message(&format!(
        "  evaluation coverage: {} objective/material groups, {} cases, {} failed",
        group_count,
        report.cases.len(),
        report.failed
    ));
    let objectives: std::collections::BTreeSet<_> = report
        .cases
        .iter()
        .map(|case| case.objective.as_str())
        .collect();
    let materials: std::collections::BTreeSet<_> = report
        .cases
        .iter()
        .map(|case| {
            case.target_material
                .split(" [")
                .next()
                .unwrap_or(&case.target_material)
        })
        .collect();
    let missing_objectives: Vec<_> = pleochroma_cut_core::ObjectivePreset::ALL
        .into_iter()
        .map(|preset| format!("{preset:?}"))
        .filter(|name| !objectives.contains(name.as_str()))
        .collect();
    let missing_materials: Vec<_> = dataset::MINERAL_SPECTRUM
        .iter()
        .map(|entry| entry.name)
        .filter(|name| !materials.contains(name))
        .collect();
    console.message(&format!(
        "    preset coverage {}/7; missing: {}",
        7 - missing_objectives.len(),
        if missing_objectives.is_empty() {
            "none".to_owned()
        } else {
            missing_objectives.join(", ")
        }
    ));
    console.message(&format!(
        "    spectrum coverage {}/{}; missing: {}",
        dataset::MINERAL_SPECTRUM.len() - missing_materials.len(),
        dataset::MINERAL_SPECTRUM.len(),
        if missing_materials.is_empty() {
            "none".to_owned()
        } else {
            missing_materials.join(", ")
        }
    ));
}

fn print_eval_groups(report: &runner::EvalReport, console: &Console) {
    let mut groups = BTreeMap::<(String, String), (usize, usize, f32, usize, f32, f32)>::new();
    let mut classical = BTreeMap::<(String, String), (usize, f32, usize, f32, usize)>::new();
    for case in &report.cases {
        let key = (case.objective.clone(), case.target_material.clone());
        let group = groups.entry(key.clone()).or_default();
        group.0 += 1;
        if let Some(drop) = case.score_drop {
            group.1 += 1;
            group.2 += drop;
        }
        group.3 += case.evaluations;
        group.4 += case.model_seconds.unwrap_or(0.0);
        group.5 += case.classical_seconds.unwrap_or(0.0);
        if let (Some(before), Some(after)) = (case.classical_base_score, case.classical_final_score)
        {
            let comparison = classical.entry(key).or_default();
            comparison.0 += 1;
            comparison.1 += before - after;
            comparison.2 += case.classical_evaluations;
            comparison.3 += case.classical_seconds.unwrap_or(0.0);
        } else if case.classical_failure.is_some() {
            classical.entry(key).or_default().4 += 1;
        }
    }
    print_coverage(report, groups.len(), console);
    for ((objective, material), (total, scored, drops, evaluations, elapsed, classical_secs)) in
        groups
    {
        let mean = if scored == 0 {
            0.0
        } else {
            drops / scored as f32
        };
        let throughput = if elapsed > 0.0 {
            evaluations as f32 / elapsed
        } else {
            0.0
        };
        let classical = if classical_secs > 0.0 {
            format!(", classical {classical_secs:.1}s")
        } else {
            String::new()
        };
        console.message(&format!(
            "    {objective} / {material}: {scored}/{total} scored, mean drop {mean:.2}, {throughput:.1} model eval/s{classical}"
        ));
    }
    for ((objective, material), (count, drops, evaluations, seconds, failures)) in classical {
        if count > 0 {
            console.message(&format!(
                "    classical {objective} / {material}: {count} cases, mean drop {:.2}, {} candidate evals, {:.1}s{}",
                drops / count as f32,
                evaluations,
                seconds,
                if failures > 0 {
                    format!(", {failures} failed")
                } else {
                    String::new()
                }
            ));
        }
    }
}

fn training_checkpoint_name(args: &CliArgs, iteration: usize) -> String {
    format!("{}-iter{iteration}.json", training_checkpoint_stem(args))
}

fn periodic_checkpoint_path(args: &CliArgs, iteration: usize) -> String {
    if let Some(path) = &args.weights_path {
        let path = Path::new(path);
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("weights");
        let name = format!("{stem}-iter{iteration}.json");
        return path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(name)
            .to_string_lossy()
            .into_owned();
    }
    training_checkpoint_name(args, iteration)
}

/// The best-validation weights of one validation identity: `<stem>-best-validation-v3.<tag>.json`,
/// or the fixed `<stem>-best-validation-v3.json` (`tag` `None`) that runs recorded before
/// per-identity names keep using for their identity.
fn validation_checkpoint_path(args: &CliArgs, tag: Option<&str>) -> String {
    let name = |stem: &str| {
        tag.map_or_else(
            || format!("{stem}-best-validation-v3.json"),
            |tag| format!("{stem}-best-validation-v3.{tag}.json"),
        )
    };
    if let Some(path) = &args.weights_path {
        let path = Path::new(path);
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("weights");
        return path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(name(stem))
            .to_string_lossy()
            .into_owned();
    }
    name(&training_checkpoint_stem(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_warnings_are_capped_to_ten_then_one_line_per_fifty() {
        let mut skips = SkipLog::default();
        for n in 1..=10 {
            let line = skips.line(&format!("'d{n}': dead stone")).unwrap();
            assert_eq!(line, format!("{SKIPPED_PREFIX}'d{n}': dead stone"));
        }
        // Skips 11 to 59 are silent; the 60th closes the first block of fifty.
        for n in 11..=59 {
            assert!(
                skips.line(&format!("'d{n}': dead stone")).is_none(),
                "skip {n}"
            );
        }
        assert_eq!(
            skips.line("'d60': dead stone").as_deref(),
            Some("… 50 more designs skipped (last: 'd60': dead stone)")
        );
        assert_eq!(skips.total, 60);
        for _ in 61..=109 {
            assert!(skips.line("'x': dead stone").is_none());
        }
        assert!(skips.line("'d110': dead stone").is_some());
        assert_eq!(skips.total, 110);
    }
}
