//! Validation/checkpoint messages between CLI iterations; all output shares the live console.

use crate::{
    cli_args::CliArgs,
    cli_console::Console,
    cli_resume::{RecoverySession, ResumePhase},
    cli_run::{refresh_last_metric, save_best_validation, save_progress, save_telemetry},
    evaluation_snapshot, print_eval_groups,
    runner::TrainingRunner,
    stats::{EvaluationSnapshot, IterationMetricPoint, rfc3339_utc_now},
    validate_with_progress,
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct RunRecords {
    pub(super) history: Vec<IterationMetricPoint>,
    pub(super) logs: Vec<(String, String, String)>,
    pub(super) evaluations: Vec<EvaluationSnapshot>,
    pub(super) best_validation_score: Option<f32>,
    pub(super) skips: crate::SkipLog,
    pub(super) training_seconds: f32,
}

pub(super) fn after_iteration(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    iter: usize,
    console: &Console,
) -> Result<(), Box<dyn std::error::Error>> {
    if validation_due(iter, args.iterations, runner.config().validation_every) {
        // Commit the update before potentially long validation. Recovery retries this phase
        // before collecting another rollout, instead of losing the scheduled evaluation.
        save_progress(
            runner,
            args,
            records,
            recovery,
            ResumePhase::ValidationPending,
            console,
        )?;
        complete_validation(runner, args, records, recovery, console)?;
    } else if args.save_every > 0 && iter.is_multiple_of(args.save_every) {
        save_progress(
            runner,
            args,
            records,
            recovery,
            ResumePhase::Training,
            console,
        )?;
    }
    Ok(())
}

pub(super) fn complete_validation(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    console: &Console,
) -> Result<(), Box<dyn std::error::Error>> {
    let iter = runner.stats().iteration;
    console.message(&format!(
        "iteration {iter}/{}: validation in progress",
        args.iterations
    ));
    let directory = recovery.validation_directory(iter, runner.device_label());
    console.message(&format!(
        "Completed validation cases are saved in {} and reused after recovery.",
        directory.display()
    ));
    if let Some((line, report)) = validate_with_progress(runner, args, &directory)? {
        console.message(&format!("  {line}"));
        print_eval_groups(&report, console);
        records
            .logs
            .push((rfc3339_utc_now(), "VAL".to_owned(), line));
        records
            .evaluations
            .push(evaluation_snapshot(iter, "validation", &report));
        refresh_last_metric(runner, records, console);
        save_best_validation(runner, args, &mut records.best_validation_score, console)?;
    }
    runner.progress().phase("save validation telemetry", 0);
    save_telemetry(runner, args, records)?;
    recovery.save(runner, args, records, ResumePhase::Training)?;
    Ok(())
}

const fn validation_due(iteration: usize, total: usize, cadence: usize) -> bool {
    iteration == total || iteration.is_multiple_of(cadence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_cadence_uses_absolute_iteration_after_recovery() {
        assert!(!validation_due(21, 300, 20));
        assert!(validation_due(40, 300, 20));
        assert!(validation_due(299, 299, 20));
    }
}
