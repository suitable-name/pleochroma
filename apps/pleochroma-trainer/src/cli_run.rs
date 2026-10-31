//! CLI run lifecycle and durable save boundaries.
//!
//! Recovery continues the original total iteration count. A checkpoint records whether its
//! validation or final evaluation is still pending, so restarting cannot skip that work.

use std::{
    error::Error,
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    best_record,
    cli_args::{CliArgs, build_config},
    cli_console::Console,
    cli_live_progress::{LiveProgressObserver, ProgressStatusGuard},
    cli_report::{completed_metrics, print_banner, print_progress, print_summary},
    cli_resume::{RecoverySession, ResumePhase},
    cli_run_reporting::{self, RunRecords},
    evaluate_test_split, evaluation_snapshot, load_split, periodic_checkpoint_path,
    print_eval_groups,
    runner::{self, TrainingRunner},
    save_eval_artifact,
    stats::rfc3339_utc_now,
    training_checkpoint_name, validation_checkpoint_path, warning_lines, with_heartbeat,
};

pub(super) fn run(args: &CliArgs, recovery: &mut RecoverySession) -> Result<(), Box<dyn Error>> {
    if recovery.phase() == ResumePhase::Complete {
        println!("This run is already complete; no training or evaluation was repeated.");
        return Ok(());
    }
    for note in args.ignored_flag_notes() {
        eprintln!("note: {note}");
    }
    let split = with_heartbeat("dataset loading and feature preparation (CPU)", || {
        load_split(args, recovery.database_path())
    })?;
    let counts = (split.train.len(), split.val.len(), split.test.len());
    let max_tiers = split.max_tiers;
    let mut runner = with_heartbeat("trainer initialization", || {
        if recovery.is_resumed() {
            TrainingRunner::new_for_recovery(build_config(args), split)
        } else {
            TrainingRunner::new(build_config(args), split)
        }
    })?;
    let console = Console::new();
    println!("Training backend: {}", runner.device_label());
    println!("Recovery target: {}", recovery.prefix().display());
    let progress = runner.progress();
    let (progress_observer, mut progress_status) = start_progress(&runner, args, &console);
    let mut records = recovery.take_records();
    if recovery.is_resumed() {
        console.message(&format!(
            "Restoring saved episodes and optimizer state; pending phase: {:?}",
            recovery.phase()
        ));
        recovery.restore_runner(&mut runner)?;
        console.message(&format!(
            "Restored full training state at iteration {}/{}",
            runner.stats().iteration,
            args.iterations
        ));
    } else if let Some(path) = &args.warm_start {
        runner.warm_start_checkpoint(Path::new(path))?;
        console.message(&format!(
            "Warm-started weights from {path}; this is a new run with fresh training state."
        ));
    }
    for line in warning_lines(&mut runner, &mut records.skips) {
        console.warning(&format!("warning: {line}"));
    }
    console.message_block(|| print_banner(&runner, args, counts, max_tiers));
    if !recovery.is_resumed() {
        progress.phase("save initial recovery checkpoint", 0);
        recovery.save(&mut runner, args, &records, ResumePhase::Training)?;
    }
    progress.begin(runner.stats().iteration, 0);
    progress.phase("starting training", 0);
    if recovery.phase() == ResumePhase::ValidationPending {
        console.message(
            "Completing saved pending validation before the next rollout. Policy inference follows the selected device; CUDA also accelerates optical scoring and meet residuals. Progress reports CPU fallbacks and completed cases.",
        );
        cli_run_reporting::complete_validation(
            &mut runner,
            args,
            &mut records,
            recovery,
            &console,
        )?;
    }
    train_remaining(&mut runner, args, &mut records, recovery, &console)?;
    let result = finish_run(&mut runner, args, &mut records, recovery, &console);
    if result.is_ok() {
        progress_status.complete();
    }
    drop(progress_status);
    drop(progress_observer);
    print_summary(
        &runner,
        args,
        records.training_seconds,
        records.skips.total,
        &records.history,
    );
    result
}

fn start_progress(
    runner: &TrainingRunner,
    args: &CliArgs,
    console: &Console,
) -> (LiveProgressObserver, ProgressStatusGuard) {
    let progress = runner.progress();
    progress.begin(runner.stats().iteration, 0);
    progress.phase("preparing training state", 0);
    let path = live_progress_path(args);
    console.message(&format!("Live progress: {}", path.display()));
    let observer =
        LiveProgressObserver::start(progress.clone(), path, console.clone(), args.iterations);
    (observer, ProgressStatusGuard::new(progress))
}

fn train_remaining(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    let mut window_start = Instant::now();
    let mut window_steps = 0usize;
    let mut steps_per_sec = 0.0f32;
    while runner.stats().iteration < args.iterations {
        let iteration_start = Instant::now();
        let report = runner
            .iterate(&mut || true)?
            .ok_or("the rollout ended early")?;
        let iter = runner.stats().iteration;
        records.training_seconds += iteration_start.elapsed().as_secs_f32();
        for line in warning_lines(runner, &mut records.skips) {
            console.warning(&format!("warning: {line}"));
            records
                .logs
                .push((rfc3339_utc_now(), "WARN".to_owned(), line));
        }
        window_steps += report.steps;
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_millis(500) || steps_per_sec == 0.0 {
            steps_per_sec = window_steps as f32 / elapsed.as_secs_f32().max(1e-3);
            window_steps = 0;
            window_start = Instant::now();
        }
        if let Some(best) = &report.new_best_improvement {
            records.logs.push(best_record(iter, best));
        }
        let point = runner.metric_point(steps_per_sec);
        print_progress(console, runner, &point, iter, args.iterations);
        records.history.push(point);
        cli_run_reporting::after_iteration(runner, args, records, recovery, iter, console)?;
    }
    Ok(())
}

fn finish_run(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    runner.sync_update()?;
    runner.progress().phase("final telemetry and checkpoint", 0);
    refresh_last_metric(runner, records, console);
    save_telemetry(runner, args, records)?;
    console.message(&format!("\nTelemetry written to {}", args.stats_path));
    let weights_path = args
        .weights_path
        .clone()
        .unwrap_or_else(|| training_checkpoint_name(args, runner.stats().iteration));
    runner.save_checkpoint(Path::new(&weights_path))?;
    console.message(&format!("Checkpoint written to {weights_path}"));
    if args.eval_test {
        recovery.save(runner, args, records, ResumePhase::FinalEvaluationPending)?;
        runner.progress().phase("final test evaluation", 0);
        let report = evaluate_test_split(runner, args, console)?;
        print_eval_groups(&report, console);
        records.evaluations.push(evaluation_snapshot(
            runner.stats().iteration,
            "test",
            &report,
        ));
        runner.progress().phase("save final test report", 0);
        save_eval_artifact(args, &records.evaluations)?;
    }
    recovery.save(runner, args, records, ResumePhase::Complete)?;
    console.message("Import the telemetry in the GUI with 'Import Run'.");
    Ok(())
}

pub(super) fn refresh_last_metric(
    runner: &TrainingRunner,
    records: &mut RunRecords,
    console: &Console,
) {
    if let Some(last) = records.history.last_mut() {
        *last = runner.metric_point(last.steps_per_sec);
        console.metrics(completed_metrics(
            last,
            runner.config().algo == runner::Algorithm::Imitation,
        ));
    }
}

pub(super) fn save_progress(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    phase: ResumePhase,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    runner.sync_update()?;
    runner.progress().phase("save telemetry and checkpoint", 0);
    refresh_last_metric(runner, records, console);
    save_telemetry(runner, args, records)?;
    let iter = runner.stats().iteration;
    let path = periodic_checkpoint_path(args, iter);
    runner.save_checkpoint(Path::new(&path))?;
    recovery.save(runner, args, records, phase)?;
    console.message(&format!(
        "\nSaved iteration {iter}: {} and {path}; latest recovery committed",
        args.stats_path
    ));
    Ok(())
}

pub(super) fn save_telemetry(
    runner: &TrainingRunner,
    args: &CliArgs,
    records: &RunRecords,
) -> Result<(), Box<dyn Error>> {
    runner
        .export_stats(&records.history, &records.logs)
        .save_to_file(&args.stats_path)?;
    save_eval_artifact(args, &records.evaluations)?;
    Ok(())
}

pub(super) fn save_best_validation(
    runner: &TrainingRunner,
    args: &CliArgs,
    best_score: &mut Option<f32>,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    let Some(validation) = runner.stats().last_validation.as_ref() else {
        return Ok(());
    };
    if args.fixture
        || !validation.mean_final_score.is_finite()
        || validation.designs == 0
        || validation.failed > 0
        || best_score.is_some_and(|score| validation.mean_final_score >= score)
    {
        return Ok(());
    }
    *best_score = Some(validation.mean_final_score);
    let path = validation_checkpoint_path(args);
    runner.save_checkpoint(Path::new(&path))?;
    console.message(&format!(
        "Validation checkpoint at iteration {}: {path}",
        runner.stats().iteration
    ));
    Ok(())
}

fn live_progress_path(args: &CliArgs) -> std::path::PathBuf {
    let mut path = std::ffi::OsString::from(&args.stats_path);
    path.push(".progress.json");
    path.into()
}
