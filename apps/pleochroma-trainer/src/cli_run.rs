//! CLI run lifecycle and durable save boundaries.
//!
//! Recovery continues the original total iteration count. A checkpoint records whether its
//! validation or final evaluation is still pending, so restarting cannot skip that work.
//!
//! A first SIGINT or SIGTERM (Ctrl-C) stops the run at the next iteration boundary: the
//! iteration in progress finishes, its state is committed through the ordinary save path and
//! the CLI exits with [`STOPPED_EXIT_STATUS`]. The rollout is deliberately not cancelled: a
//! cancelled rollout keeps the episode progress and replay labels it collected and refreshes
//! the rollout weights early, so a run resumed from it would depend on when the signal arrived
//! instead of on the iteration index alone. A validation or test evaluation due at that
//! boundary stays pending and runs first on `--resume`. A second signal exits at once; the
//! last committed recovery stays valid.

use std::{
    error::Error,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
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

/// Exit status of a run a signal stopped (128 + SIGINT, as shells report an interrupt), both
/// after the clean stop and on the second signal.
pub(super) const STOPPED_EXIT_STATUS: u8 = 130;

/// How often the stop notice thread looks at the request flag.
const STOP_POLL: Duration = Duration::from_millis(200);

const STOP_NOTICE: &str = "Stop requested: the iteration in progress finishes and is committed, then the run stops (continue with --resume). Press Ctrl-C again to exit at once from the last committed recovery.";

/// How a run ended without an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunOutcome {
    Completed,
    /// A signal stopped it at a committed boundary; `--resume` continues it.
    Stopped,
}

/// The first SIGINT/SIGTERM requests a stop at the next committed boundary; the second exits
/// at once with [`STOPPED_EXIT_STATUS`].
pub(super) struct StopRequest {
    requested: Arc<AtomicBool>,
    notice: Option<(mpsc::Sender<()>, thread::JoinHandle<()>)>,
}

impl StopRequest {
    fn install(console: &Console) -> Self {
        let requested = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            // Registered first, so it acts only once the flag below is already set, that is on
            // the second signal.
            let installed = signal_hook::flag::register_conditional_shutdown(
                signal,
                i32::from(STOPPED_EXIT_STATUS),
                Arc::clone(&requested),
            )
            .and_then(|shutdown| {
                signal_hook::flag::register(signal, Arc::clone(&requested)).inspect_err(|_| {
                    // Without the flag the conditional exit would swallow every signal.
                    let _ = signal_hook::low_level::unregister(shutdown);
                })
            });
            if let Err(error) = installed {
                console.warning(&format!(
                    "warning: no clean stop on signal {signal} ({error}); it ends the run at once, which resumes from the last committed recovery"
                ));
            }
        }
        let notice = Self::start_notice(&requested, console);
        Self { requested, notice }
    }

    /// Announces the request as soon as it arrives, also during a long rollout or validation.
    fn start_notice(
        requested: &Arc<AtomicBool>,
        console: &Console,
    ) -> Option<(mpsc::Sender<()>, thread::JoinHandle<()>)> {
        let (stop, stopped) = mpsc::channel();
        let requested = Arc::clone(requested);
        let console = console.clone();
        thread::Builder::new()
            .name("trainer-stop-notice".into())
            .spawn(move || {
                while stopped.recv_timeout(STOP_POLL) == Err(mpsc::RecvTimeoutError::Timeout) {
                    if requested.load(Ordering::SeqCst) {
                        console.warning(STOP_NOTICE);
                        return;
                    }
                }
            })
            .ok()
            .map(|handle| (stop, handle))
    }

    pub(super) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
}

impl Drop for StopRequest {
    fn drop(&mut self) {
        if let Some((stop, handle)) = self.notice.take() {
            let _ = stop.send(());
            let _ = handle.join();
        }
    }
}

pub(super) fn run(
    args: &CliArgs,
    recovery: &mut RecoverySession,
) -> Result<RunOutcome, Box<dyn Error>> {
    if recovery.phase() == ResumePhase::Complete {
        println!("This run is already complete; no training or evaluation was repeated.");
        return Ok(RunOutcome::Completed);
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
    restore_or_warm_start(&mut runner, args, recovery, &console)?;
    for line in warning_lines(&mut runner, &mut records.skips) {
        console.warning(&format!("warning: {line}"));
    }
    console.message_block(|| print_banner(&runner, args, counts, max_tiers));
    if !recovery.is_resumed() {
        progress.phase("save initial recovery checkpoint", 0);
        recovery.save(&mut runner, args, &records, ResumePhase::Training)?;
    }
    // From here on a committed state exists, so an interruption can stop cleanly.
    let stop = StopRequest::install(&console);
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
    let trained = train_remaining(&mut runner, args, &mut records, recovery, &console, &stop)?;
    let result = match trained {
        RunOutcome::Completed => {
            finish_run(&mut runner, args, &mut records, recovery, &console, &stop)
        }
        RunOutcome::Stopped => Ok(RunOutcome::Stopped),
    };
    let stopped = matches!(result, Ok(RunOutcome::Stopped));
    if matches!(result, Ok(RunOutcome::Completed)) {
        progress_status.complete();
    }
    drop(progress_status);
    if stopped {
        // After the guard, which marks every unfinished run as failed.
        runner.progress().finish("stopped");
    }
    drop(progress_observer);
    drop(stop);
    if trained == RunOutcome::Completed {
        print_summary(
            &runner,
            args,
            records.training_seconds,
            records.skips.total,
            &records.history,
        );
    }
    if stopped {
        report_stop(&runner, args, recovery, &console);
    }
    result
}

/// Restores the full state of a resumed run, or the weights a new run warm-starts from.
fn restore_or_warm_start(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    recovery: &RecoverySession,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    if recovery.is_resumed() {
        console.message(&format!(
            "Restoring saved episodes and optimizer state; pending phase: {:?}",
            recovery.phase()
        ));
        // Without --relabel a changed environment backend refuses the restore.
        runner.set_allow_environment_relabel(args.relabel);
        recovery.restore_runner(runner)?;
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
    Ok(())
}

fn report_stop(
    runner: &TrainingRunner,
    args: &CliArgs,
    recovery: &RecoverySession,
    console: &Console,
) {
    let pending = match recovery.phase() {
        ResumePhase::ValidationPending => "; its pending validation runs first",
        ResumePhase::FinalEvaluationPending => "; its pending test evaluation runs first",
        ResumePhase::Training | ResumePhase::Complete => "",
    };
    console.message(&format!(
        "\nStopped by signal at iteration {}/{}; recovery committed. Continue with --resume {}{pending}.",
        runner.stats().iteration,
        args.iterations,
        recovery.prefix().display()
    ));
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
    stop: &StopRequest,
) -> Result<RunOutcome, Box<dyn Error>> {
    let mut window_start = Instant::now();
    let mut window_steps = 0usize;
    let mut steps_per_sec = 0.0f32;
    // The restored, the initial and every saved state are committed; a stop then saves nothing.
    let mut committed = true;
    loop {
        // Before the end check: a stop during the last iteration keeps its validation pending.
        if stop.is_requested() {
            if !committed {
                save_progress(
                    runner,
                    args,
                    records,
                    recovery,
                    ResumePhase::Training,
                    console,
                )?;
            }
            return Ok(RunOutcome::Stopped);
        }
        if runner.stats().iteration >= args.iterations {
            return Ok(RunOutcome::Completed);
        }
        let iteration_start = Instant::now();
        // Never cancelled, see the module docs; a stop waits for the iteration boundary.
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
        committed = cli_run_reporting::after_iteration(
            runner, args, records, recovery, iter, console, stop,
        )?;
    }
}

fn finish_run(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    console: &Console,
    stop: &StopRequest,
) -> Result<RunOutcome, Box<dyn Error>> {
    runner.sync_update()?;
    runner.progress().phase("final telemetry and checkpoint", 0);
    record_final_update(runner, records, console);
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
        if stop.is_requested() {
            return Ok(RunOutcome::Stopped);
        }
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
    Ok(RunOutcome::Completed)
}

/// Every iteration's point shows the update installed during that iteration (an imitation
/// update trains during the next rollout). The final synchronization installs one more update,
/// which no later point can show; it is logged once (an `UPDATE` record) instead of replacing
/// the last point's figures, so every update appears exactly once in the telemetry.
fn record_final_update(runner: &TrainingRunner, records: &mut RunRecords, console: &Console) {
    let point = runner.metric_point(0.0);
    let reported = records
        .history
        .last()
        .and_then(|last| last.update_of_iteration);
    if point.update_of_iteration.is_none() || point.update_of_iteration == reported {
        return;
    }
    let lines = completed_metrics(&point, runner.config().algo == runner::Algorithm::Imitation);
    let Some(line) = lines.first().cloned() else {
        return;
    };
    console.metrics(lines);
    // A resumed pending final evaluation installs the same update again.
    if !records
        .logs
        .iter()
        .any(|(_, kind, text)| kind == "UPDATE" && *text == line)
    {
        records
            .logs
            .push((rfc3339_utc_now(), "UPDATE".to_owned(), line));
    }
}

/// Commits a synchronized iteration boundary: telemetry, a numbered checkpoint and the
/// recovery bundle. The recorded points keep the update figures their iterations reported; the
/// update this synchronization installs belongs to the next iteration's point.
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
    let mut stats = runner.export_stats(&records.history, &records.logs);
    // `export_stats` rebuilds the current iteration's point from the runner, which after a
    // save's synchronization already shows the next iteration's update. The recorded points
    // (validation figures refreshed) keep every update exactly once.
    stats.metrics_history.clone_from(&records.history);
    stats.save_to_file(&args.stats_path)?;
    save_eval_artifact(args, &records.evaluations)?;
    Ok(())
}

/// Saves the weights of a new best validation score of the current validation identity. Every
/// identity has its own file ([`RunRecords::best_validation_tag`]), so a changed cohort,
/// physics or backend never overwrites another identity's best.
pub(super) fn save_best_validation(
    runner: &TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    console: &Console,
) -> Result<(), Box<dyn Error>> {
    let Some(validation) = runner.stats().last_validation.as_ref() else {
        return Ok(());
    };
    if args.fixture
        || !validation.mean_final_score.is_finite()
        || validation.designs == 0
        || validation.failed > 0
        || records
            .best_validation_score
            .is_some_and(|score| validation.mean_final_score >= score)
    {
        return Ok(());
    }
    records.best_validation_score = Some(validation.mean_final_score);
    let path = validation_checkpoint_path(args, records.best_validation_tag().as_deref());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_signal_requests_a_stop_without_ending_the_process() {
        let stop = StopRequest::install(&Console::new());
        assert!(!stop.is_requested());
        signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !stop.is_requested() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(stop.is_requested());
    }
}
