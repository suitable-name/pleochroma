//! Validation/checkpoint messages between CLI iterations; all output shares the live console.

use std::collections::BTreeMap;

use crate::{
    cli_args::CliArgs,
    cli_console::Console,
    cli_report::completed_metrics,
    cli_resume::{RecoverySession, ResumePhase},
    cli_run::{StopRequest, save_best_validation, save_progress, save_telemetry},
    evaluation_snapshot, print_eval_groups,
    runner::{Algorithm, TrainingRunner},
    stats::{EvaluationSnapshot, IterationMetricPoint, rfc3339_utc_now},
    validate_with_progress,
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct RunRecords {
    pub(super) history: Vec<IterationMetricPoint>,
    pub(super) logs: Vec<(String, String, String)>,
    pub(super) evaluations: Vec<EvaluationSnapshot>,
    /// Best validation score of the current validation identity.
    pub(super) best_validation_score: Option<f32>,
    #[serde(default)]
    pub(super) validation_identity: Option<String>,
    /// Best scores of the identities this run left, by [`identity_tag`]. Returning to one (a
    /// device switched back, say) compares against its saved best instead of replacing that
    /// identity's file with a worse model.
    #[serde(default)]
    retired_validation_bests: BTreeMap<String, f32>,
    /// Whether the current identity's best weights use the fixed file name of runs recorded
    /// before per-identity names existed (the field is absent there, and the default `true`
    /// keeps their file), until that identity changes.
    #[serde(default = "fixed_best_name")]
    best_validation_fixed_name: bool,
    pub(super) skips: crate::SkipLog,
    pub(super) training_seconds: f32,
}

const fn fixed_best_name() -> bool {
    true
}

/// Short stable name of a validation identity (FNV-1a 64 in hex) for file names and records.
fn identity_tag(identity: &str) -> String {
    let hash = identity
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("{hash:016x}")
}

impl RunRecords {
    /// Compares validation scores within `identity` from now on; returns whether it changed.
    /// The best score of the identity left behind is retired, not discarded.
    fn enter_validation_identity(&mut self, identity: String) -> bool {
        if self.best_validation_score.is_none() {
            // The fixed name can only belong to an identity that saved a best before
            // per-identity names existed; without a best, its file is another identity's.
            self.best_validation_fixed_name = false;
        }
        if self.validation_identity.as_ref() == Some(&identity) {
            return false;
        }
        if let (Some(previous), Some(score)) =
            (&self.validation_identity, self.best_validation_score)
        {
            self.retired_validation_bests
                .insert(identity_tag(previous), score);
        }
        self.best_validation_fixed_name = false;
        self.best_validation_score = self
            .retired_validation_bests
            .remove(&identity_tag(&identity));
        self.validation_identity = Some(identity);
        true
    }

    /// The tag in the current identity's best-validation file name; `None` for the fixed name
    /// an older run already uses for this identity.
    pub(super) fn best_validation_tag(&self) -> Option<String> {
        if self.best_validation_fixed_name {
            return None;
        }
        self.validation_identity.as_deref().map(identity_tag)
    }
}

/// Saves and validates after an iteration as scheduled. Returns whether the iteration's state
/// is committed, so a stop needs no further save. On a stop request a due validation is
/// committed as pending instead of run.
pub(super) fn after_iteration(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    iter: usize,
    console: &Console,
    stop: &StopRequest,
) -> Result<bool, Box<dyn std::error::Error>> {
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
        if stop.is_requested() {
            console.message(&format!(
                "Stop requested: the validation of iteration {iter} stays pending and runs first on --resume."
            ));
            return Ok(true);
        }
        complete_validation(runner, args, records, recovery, console)?;
        Ok(true)
    } else if args.save_every > 0 && iter.is_multiple_of(args.save_every) {
        save_progress(
            runner,
            args,
            records,
            recovery,
            ResumePhase::Training,
            console,
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn complete_validation(
    runner: &mut TrainingRunner,
    args: &CliArgs,
    records: &mut RunRecords,
    recovery: &mut RecoverySession,
    console: &Console,
) -> Result<(), Box<dyn std::error::Error>> {
    let iter = runner.stats().iteration;
    let identity = runner.validation_identity(runner.config().validation_designs);
    let had_best = records.best_validation_score.is_some();
    if records.enter_validation_identity(identity) && had_best {
        console.message("Validation cohort, physics or backend changed: the previous identity keeps its best-validation weights file; this identity compares against its own best. Prior validation records are retained.");
    }
    console.message(&format!(
        "iteration {iter}/{}: validation in progress",
        args.iterations
    ));
    let directory = recovery.validation_directory(iter, runner.device_label());
    console.message(&format!(
        "Validation cases, passes, steps and teacher progress are saved in {}; model-independent references are retained beside this directory.",
        directory.display()
    ));
    if let Some((line, report)) = validate_with_progress(runner, args, &directory)? {
        console.message(&format!("  {line}"));
        console.message(&format!(
            "  coverage: {} entries / {} families / {} successful objective cases; zero-mean exploration drop: {}{}",
            report.distinct_entries, report.distinct_families, report.designs,
            report.mean_improvement_noise.map_or_else(|| "not evaluated".to_owned(), |v| format!("{v:.3}")),
            if report.noise_cached { " (cached reference)" } else { "" },
        ));
        print_eval_groups(&report, console);
        records
            .logs
            .push((rfc3339_utc_now(), "VAL".to_owned(), line));
        records
            .evaluations
            .push(evaluation_snapshot(iter, "validation", &report));
        refresh_validation_figures(runner, records, console);
        save_best_validation(runner, args, records, console)?;
    }
    runner.progress().phase("save validation telemetry", 0);
    save_telemetry(runner, args, records)?;
    recovery.save(runner, args, records, ResumePhase::Training)?;
    for warning in recovery.prune_validation_journals(iter) {
        console.warning(&format!("warning: {warning}"));
    }
    Ok(())
}

/// Copies the figures a validation pass changes into the iteration's recorded point and shows
/// them. The update figures stay those the iteration reported: the save before validation
/// synchronizes the next update early, and that update belongs to the next iteration's point.
fn refresh_validation_figures(
    runner: &TrainingRunner,
    records: &mut RunRecords,
    console: &Console,
) {
    let iteration = runner.stats().iteration;
    if let Some(last) = records
        .history
        .last_mut()
        .filter(|last| last.iteration == iteration)
    {
        copy_validation_figures(last, &runner.metric_point(last.steps_per_sec));
        console.metrics(completed_metrics(
            last,
            runner.config().algo == Algorithm::Imitation,
        ));
    }
}

const fn copy_validation_figures(
    recorded: &mut IterationMetricPoint,
    fresh: &IterationMetricPoint,
) {
    recorded.val_score = fresh.val_score;
    recorded.val_brilliance = fresh.val_brilliance;
    recorded.val_windowing = fresh.val_windowing;
    recorded.val_improvement = fresh.val_improvement;
    recorded.val_improvement_sampled = fresh.val_improvement_sampled;
    recorded.val_improvement_oracle = fresh.val_improvement_oracle;
    recorded.val_evaluations_per_step = fresh.val_evaluations_per_step;
    recorded.val_designs = fresh.val_designs;
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

    fn point(update_of: usize, loss: f32, val_score: Option<f32>) -> IterationMetricPoint {
        serde_json::from_value(serde_json::json!({
            "iteration": 20, "episode": 3, "total_steps": 512, "steps_per_sec": 9.0,
            "mean_step_reward": 0.1, "policy_loss": 0.0, "value_loss": 0.0, "entropy": 0.0,
            "clip_fraction": 0.0, "solid_closed": true, "brilliance": 50.0, "windowing": 5.0,
            "extinction": 10.0, "yield_pct": 40.0, "update_of_iteration": update_of,
            "imitation_loss": loss, "val_score": val_score, "val_designs": 0,
        }))
        .unwrap()
    }

    #[test]
    fn a_validation_refresh_keeps_the_update_the_iteration_reported() {
        // Iteration 20 reported the update of rollout 18; the save before validation already
        // installed the one of rollout 19, which belongs to iteration 21's point.
        let mut recorded = point(18, 0.5, None);
        let mut fresh = point(19, 0.25, Some(41.5));
        fresh.val_designs = 16;
        copy_validation_figures(&mut recorded, &fresh);
        assert_eq!(recorded.update_of_iteration, Some(18));
        assert_eq!(recorded.imitation_loss, 0.5);
        assert_eq!(recorded.val_score, Some(41.5));
        assert_eq!(recorded.val_designs, 16);
    }

    #[test]
    fn identities_keep_their_own_best_and_never_reuse_another_identitys_file() {
        let mut records = RunRecords::default();
        assert!(records.enter_validation_identity("cuda:A".into()));
        assert!(!records.enter_validation_identity("cuda:A".into()));
        records.best_validation_score = Some(5.0);
        let first = records.best_validation_tag().unwrap();
        assert_eq!(first, identity_tag("cuda:A"));
        assert!(records.enter_validation_identity("cpu:A".into()));
        assert_eq!(records.best_validation_score, None);
        let second = records.best_validation_tag().unwrap();
        assert_ne!(first, second);
        records.best_validation_score = Some(7.0);
        // Back on the first backend: its own best (and file) again, the other one retired.
        assert!(records.enter_validation_identity("cuda:A".into()));
        assert_eq!(records.best_validation_score, Some(5.0));
        assert_eq!(records.best_validation_tag(), Some(first));
        assert_eq!(records.retired_validation_bests.get(&second), Some(&7.0));
    }

    #[test]
    fn runs_recorded_before_tags_keep_their_fixed_best_file_until_the_identity_changes() {
        let legacy = |best: Option<f32>| -> RunRecords {
            serde_json::from_value(serde_json::json!({
                "history": [], "logs": [], "evaluations": [], "best_validation_score": best,
                "validation_identity": "cuda:A", "skips": {"total": 0, "pending": 0},
                "training_seconds": 0.0,
            }))
            .unwrap()
        };
        let mut records = legacy(Some(3.0));
        assert!(!records.enter_validation_identity("cuda:A".into()));
        assert_eq!(
            records.best_validation_tag(),
            None,
            "the old fixed file is this identity's"
        );
        assert!(records.enter_validation_identity("cpu:A".into()));
        assert_eq!(records.best_validation_tag(), Some(identity_tag("cpu:A")));
        assert_eq!(
            records
                .retired_validation_bests
                .get(&identity_tag("cuda:A")),
            Some(&3.0)
        );
        // Without a saved best, the fixed file belongs to an earlier identity: never reuse it.
        let mut records = legacy(None);
        assert!(!records.enter_validation_identity("cuda:A".into()));
        assert_eq!(records.best_validation_tag(), Some(identity_tag("cuda:A")));
        // A new run never uses the fixed name.
        assert!(!RunRecords::default().best_validation_fixed_name);
    }
}
