//! Real production iterations and targeted projection cases, with no recovery lifecycle.

use serde_json::{Value, json};
use std::{
    error::Error,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};

use crate::{
    dataset::{self, DatasetSplit},
    runner::{self, DeviceChoice, IterationReport, RunnerStats, TrainingRunner},
    throughput_args::{Args, Scenario},
    throughput_output::{Heartbeat, Journal, Output},
};

#[path = "throughput_metadata.rs"]
mod metadata;

type Failure = Box<dyn Error + Send + Sync>;

struct Session<'a> {
    args: &'a Args,
    journal: &'a Journal,
    heartbeat: &'a Heartbeat,
    started: Instant,
}

impl Session<'_> {
    fn can_continue(&self) -> bool {
        !self.heartbeat.failed()
            && self
                .args
                .max_seconds
                .is_none_or(|limit| self.started.elapsed().as_secs() < limit)
    }

    fn check(&self) -> Result<(), Failure> {
        if self.heartbeat.failed() {
            return Err("heartbeat logging failed".into());
        }
        if !self.can_continue() {
            return Err("soft time budget exhausted between operations".into());
        }
        Ok(())
    }
}

pub fn run(args: &Args) -> Result<(), Failure> {
    let mut output = Output::create(&args.output, &metadata::initial(args))?;
    output.journal.event(
        "run_start",
        json!({"args": args, "output": output.directory}),
    )?;
    let mut heartbeat = Heartbeat::start(output.journal.clone())?;
    let started = Instant::now();
    let session = Session {
        args,
        journal: &output.journal,
        heartbeat: &heartbeat,
        started,
    };
    let result = catch_unwind(AssertUnwindSafe(|| run_inner(&session))).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "unknown panic".to_owned());
        Err(io::Error::other(format!("benchmark panicked: {message}")).into())
    });
    let result =
        result.and_then(|summary| heartbeat.finish().map(|()| summary).map_err(Into::into));
    match result {
        Ok(mut summary) => {
            let errors = summary["case_error_count"].as_u64().unwrap_or(0);
            summary["status"] = json!(if errors == 0 {
                "complete"
            } else {
                "partial_failure"
            });
            summary["total_wall_secs"] = json!(started.elapsed().as_secs_f64());
            output.journal.event(
                if errors == 0 {
                    "run_complete"
                } else {
                    "run_partial_failure"
                },
                summary.clone(),
            )?;
            output.finish(&summary)?;
            if errors == 0 {
                Ok(())
            } else {
                Err(format!(
                    "{errors} projection cases failed; all requested case reports were retained"
                )
                .into())
            }
        }
        Err(error) => {
            let _ = heartbeat.finish();
            let summary = json!({"status": "failed", "scenario": args.scenario,
                "error": error.to_string(), "total_wall_secs": started.elapsed().as_secs_f64()});
            if let Err(logging) = output.journal.event("run_failed", summary.clone()) {
                eprintln!("also failed to record error: {logging}");
            }
            if let Err(logging) = output.finish(&summary) {
                eprintln!("also failed to save summary: {logging}");
            }
            Err(error)
        }
    }
}

fn run_inner(session: &Session<'_>) -> Result<Value, Failure> {
    if session.args.device == DeviceChoice::Cuda && !cfg!(feature = "cuda") {
        return Err("CUDA requested but this binary was built without --features cuda".into());
    }
    session
        .heartbeat
        .phase("read-only dataset loading and feature preparation", None);
    session
        .journal
        .event("dataset_start", json!({"database": session.args.database}))?;
    let loading = Instant::now();
    let records = dataset::load_db_designs_from(&session.args.database)?;
    let split = DatasetSplit::from_records(
        &records,
        dataset::DEFAULT_SPLIT_SEED,
        dataset::DEFAULT_MAX_TIERS_CAP,
    )?;
    session.journal.event("dataset_complete", json!({
        "wall_secs": loading.elapsed().as_secs_f64(), "corpus": metadata::corpus(&records, &split),
    }))?;
    session.check()?;
    match session.args.scenario {
        Scenario::Pipeline => pipeline(session, split),
        Scenario::Projection => projection(session, &split),
    }
}

fn select_pipeline_entries(args: &Args, split: &mut DatasetSplit) -> Result<(), Failure> {
    if let Some(indices) = &args.entries {
        for &index in indices {
            if !split.train.contains(&index) {
                return Err(format!(
                    "pipeline entry {index} is not in the original training split"
                )
                .into());
            }
        }
        split.train.clone_from(indices);
    }
    if split.train.is_empty() {
        return Err("the dataset has no training entries".into());
    }
    Ok(())
}

fn pipeline(session: &Session<'_>, mut split: DatasetSplit) -> Result<Value, Failure> {
    select_pipeline_entries(session.args, &mut split)?;
    session.journal.event("pipeline_corpus", json!({
        "representative_full_training_split": session.args.entries.is_none(),
        "train_indices": split.train,
        "selected_entries": session.args.entries.as_ref().map(|indices| indices.iter().map(|&i| metadata::entry(&split, i)).collect::<Vec<_>>()),
    }))?;
    session
        .heartbeat
        .phase("production trainer and environment initialization", None);
    let initialization = Instant::now();
    let mut runner = TrainingRunner::new(session.args.config(), split)?;
    let initialization_secs = initialization.elapsed().as_secs_f64();
    let progress = runner.progress();
    session
        .heartbeat
        .phase("production pipeline", Some(progress.clone()));
    session.journal.event(
        "backend_ready",
        json!({
            "backend": runner.device_label(), "initialization_wall_secs": initialization_secs,
            "cpu_pool_threads": runner::cpu_pool_threads(), "snapshot": progress.snapshot(),
            "warnings": runner.take_warnings(),
        }),
    )?;
    session.check()?;
    let mut iterations = Vec::new();
    let mut measured_steps = 0;
    let mut measured_start = None;
    let mut warmup_sync_secs = 0.0;
    let mut warmup_drained_update = Value::Null;
    for index in 0..session.args.warmup + session.args.iterations {
        session.check()?;
        if index == session.args.warmup {
            warmup_sync_secs = synchronize(session, &mut runner, "warmup_boundary_sync")?;
            warmup_drained_update = update_diagnostics(runner.stats());
            measured_start = Some(Instant::now());
        }
        let measured = index >= session.args.warmup;
        session.journal.event(
            "iteration_start",
            json!({"iteration": index + 1, "measured": measured, "snapshot": progress.snapshot()}),
        )?;
        session.heartbeat.phase(
            if measured {
                "measured iteration"
            } else {
                "warm-up iteration"
            },
            Some(progress.clone()),
        );
        let started = Instant::now();
        let report = runner
            .iterate(&mut || session.can_continue())?
            .ok_or("rollout interrupted by logging failure or soft time limit")?;
        let iterate_secs = started.elapsed().as_secs_f64();
        let explicit_sync_secs = if session.args.sync_every_iteration {
            synchronize(session, &mut runner, "iteration_sync")?
        } else {
            0.0
        };
        let row = iteration(
            &report,
            index + 1,
            measured,
            iterate_secs,
            explicit_sync_secs,
            session.args.sync_every_iteration.then(|| runner.stats()),
        );
        session.journal.event(
            "iteration_complete",
            json!({
                "metrics": row, "snapshot": progress.snapshot(), "warnings": runner.take_warnings(),
            }),
        )?;
        eprintln!(
            "probe: iteration {} complete: {:.3}s iterate + {:.3}s explicit sync",
            index + 1,
            iterate_secs,
            explicit_sync_secs
        );
        measured_steps += usize::from(measured) * report.steps;
        iterations.push(row);
    }
    let final_sync_secs = synchronize(session, &mut runner, "final_sync")?;
    let measured_wall_secs = measured_start
        .expect("positive measured iterations")
        .elapsed()
        .as_secs_f64();
    Ok(json!({
        "scenario": "pipeline", "backend": runner.device_label(), "initialization_wall_secs": initialization_secs,
        "warmup_sync_update_wall_secs": warmup_sync_secs, "final_sync_update_wall_secs": final_sync_secs,
        "warmup_drained_update": warmup_drained_update,
        "final_drained_update": update_diagnostics(runner.stats()),
        "measured_steps": measured_steps, "measured_window_wall_secs": measured_wall_secs,
        "measured_steps_per_second": measured_steps as f64 / measured_wall_secs.max(f64::EPSILON),
        "iterations": iterations, "final_stats": runner.stats(), "final_snapshot": progress.snapshot(),
    }))
}

fn synchronize(
    session: &Session<'_>,
    runner: &mut TrainingRunner,
    kind: &str,
) -> Result<f64, Failure> {
    session.heartbeat.phase(kind, Some(runner.progress()));
    session.journal.event(
        "sync_start",
        json!({"kind": kind, "iteration": runner.stats().iteration}),
    )?;
    let started = Instant::now();
    runner.sync_update()?;
    let elapsed = started.elapsed().as_secs_f64();
    session.journal.event(
        "sync_complete",
        json!({"kind": kind, "wall_secs": elapsed, "snapshot": runner.progress().snapshot(),
            "installed_update": update_diagnostics(runner.stats())}),
    )?;
    Ok(elapsed)
}

fn iteration(
    report: &IterationReport,
    number: usize,
    measured: bool,
    iterate: f64,
    sync: f64,
    synchronized: Option<&RunnerStats>,
) -> Value {
    let mut row = json!({
        "iteration": number, "measured": measured, "iterate_wall_secs": iterate,
        "explicit_sync_update_wall_secs": sync, "steps": report.steps,
        "rollout_secs": report.rollout_secs, "label_secs": report.label_secs,
        "rollout_forward_ms": report.rollout_forward_ms, "reported_update_secs": report.update_secs,
        "reported_update_of_iteration_zero_based": report.update_of_iteration, "internal_update_wait_secs": report.update_wait_secs,
        "packing_ms": report.packing_ms, "training_host_ms": report.training_host_ms,
        "weight_export_ms": report.weight_export_ms, "evaluations": report.evaluations_this_round,
        "labels": report.labels_this_round, "accepted_frac": report.accepted_frac,
        "mean_step_reward": report.mean_step_reward, "oracle_gain_mean": report.oracle_gain_mean,
        "diagnostics": report.diagnostics, "imitation": report.imitation,
        "rollout_scheduling": report.rollout_scheduling,
        "update_attribution": "production_install_lag",
    });
    if let Some(stats) = synchronized {
        // `iterate` intentionally reports the previously installed update in imitation mode.
        // An explicit drain completes this rollout's own update after that report was built.
        row["production_reported_update"] = json!({
            "iteration_zero_based": report.update_of_iteration,
            "wall_secs": report.update_secs, "packing_ms": report.packing_ms,
            "training_host_ms": report.training_host_ms,
            "weight_export_ms": report.weight_export_ms, "imitation": report.imitation,
        });
        let update = update_diagnostics(stats);
        row["reported_update_secs"] = update["wall_secs"].clone();
        row["reported_update_of_iteration_zero_based"] = update["iteration_zero_based"].clone();
        row["packing_ms"] = update["packing_ms"].clone();
        row["training_host_ms"] = update["training_host_ms"].clone();
        row["weight_export_ms"] = update["weight_export_ms"].clone();
        row["imitation"] = update["imitation"].clone();
        row["update_attribution"] = json!("explicit_same_iteration");
    }
    row
}

fn update_diagnostics(stats: &RunnerStats) -> Value {
    let Some(iteration) = stats.update_of_iteration else {
        return Value::Null;
    };
    json!({
        "iteration_zero_based": iteration, "wall_secs": stats.update_secs,
        "weight_export_ms": stats.weight_export_secs * 1000.0,
        "packing_ms": stats.last_imitation.map_or(stats.last_train.packing_ms, |m| m.packing_ms),
        "training_host_ms": stats.last_imitation.map_or(stats.last_train.update_ms, |m| m.update_ms),
        "train": stats.last_train, "imitation": stats.last_imitation,
    })
}

fn projection(session: &Session<'_>, split: &DatasetSplit) -> Result<Value, Failure> {
    use pleochroma_ai::{env::ProjectionProbeConfig, gpu_geometry::GpuGeometry};
    let indices = session
        .args
        .entries
        .clone()
        .unwrap_or_else(|| split.train.iter().copied().take(3).collect());
    if indices.is_empty() {
        return Err("projection needs at least one corpus entry".into());
    }
    if let Some(index) = indices.iter().find(|&&index| index >= split.entries.len()) {
        return Err(format!(
            "entry index {index} is out of range ({} loaded entries)",
            split.entries.len()
        )
        .into());
    }
    session
        .heartbeat
        .phase("projection backend initialization", None);
    let initialization = Instant::now();
    let geometry = if session.args.device == DeviceChoice::Cuda {
        Some(GpuGeometry::new_cuda(0).map_err(io::Error::other)?)
    } else {
        None
    };
    session.journal.event(
        "backend_ready",
        json!({
            "scenario": "projection", "backend": session.args.device,
            "initialization_wall_secs": initialization.elapsed().as_secs_f64(),
            "geometry": geometry.as_ref().map(|geometry| geometry.diagnostics()),
        }),
    )?;
    let config = ProjectionProbeConfig {
        max_step_deg: session.args.max_step_deg,
        repeats: session.args.repeats,
        preservation_policy: pleochroma_cut_core::meet_preservation::PreservationPolicy::RETARGET,
        ..ProjectionProbeConfig::default()
    };
    session.journal.event(
        "projection_config",
        json!({"config": config, "entries": indices}),
    )?;
    let mut cases = Vec::new();
    for index in indices {
        session.check()?;
        cases.push(projection_case(
            session,
            split,
            index,
            &config,
            geometry.clone(),
        )?);
    }
    let errors = cases
        .iter()
        .filter(|case| case["status"] == "failed")
        .count();
    Ok(
        json!({"scenario": "projection", "cases": cases, "case_error_count": errors,
        "geometry": geometry.as_ref().map(|geometry| geometry.diagnostics())}),
    )
}

fn projection_case(
    session: &Session<'_>,
    split: &DatasetSplit,
    index: usize,
    config: &pleochroma_ai::env::ProjectionProbeConfig,
    geometry: Option<std::sync::Arc<pleochroma_ai::gpu_geometry::GpuGeometry>>,
) -> Result<Value, Failure> {
    let entry = metadata::entry(split, index);
    session
        .heartbeat
        .phase(&format!("projection entry {index}"), None);
    session
        .journal
        .event("case_start", json!({"entry": entry}))?;
    let started = Instant::now();
    let mut logging_error = None;
    let report = pleochroma_ai::env::run_projection_probe_with_events(
        &split.entries[index].design,
        &[],
        config,
        geometry.clone(),
        |event| {
            session.heartbeat.phase(
                &format!(
                    "projection entry {index} action {} {} {}",
                    event.action_index, event.phase, event.state,
                ),
                None,
            );
            if logging_error.is_none() {
                logging_error = session
                    .journal
                    .event(
                        "projection_phase",
                        json!({
                            "entry_index": index, "detail": event,
                        }),
                    )
                    .err();
            }
        },
    );
    if let Some(error) = logging_error {
        return Err(error.into());
    }
    let case = match report {
        Ok(report) => {
            let failures = projection_failures(&report);
            json!({"status": if failures.is_empty() { "complete" } else { "failed" },
                "entry": entry, "wall_secs": started.elapsed().as_secs_f64(),
                "validation_failures": failures, "report": report})
        }
        Err(error) => json!({"status": "failed", "entry": entry,
            "wall_secs": started.elapsed().as_secs_f64(), "error": error}),
    };
    session.journal.event(
        if case["status"] == "failed" {
            "case_failed"
        } else {
            "case_complete"
        },
        case.clone(),
    )?;
    if let Some(geometry) = geometry {
        geometry.check().map_err(io::Error::other)?;
    }
    Ok(case)
}

fn projection_failures(report: &pleochroma_ai::env::ProjectionProbeReport) -> Vec<String> {
    let mut failures = Vec::new();
    for (index, action) in report.actions.iter().enumerate() {
        for (route, attempt) in [
            ("cpu_reference", &action.cpu_reference),
            ("candidate_first", &action.candidate_first),
        ] {
            if attempt
                .as_ref()
                .is_some_and(|attempt| attempt.validation_passed == Some(false))
            {
                failures.push(format!(
                    "action {index} {route}: accepted result failed independent validation"
                ));
            }
        }
        for (repeat, attempt) in action.candidate_repeats.iter().enumerate() {
            if attempt.validation_passed == Some(false) {
                failures.push(format!(
                    "action {index} repeat {repeat}: accepted result failed independent validation"
                ));
            }
        }
        for (repeat, &matches) in action.repeat_matches_first.iter().enumerate() {
            if !matches {
                failures.push(format!(
                    "action {index} repeat {repeat}: outcome changed for the identical request"
                ));
            }
        }
        // CPU/CUDA acceptance or numeric differences are comparisons, not validation failures.
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_sync_reports_its_own_update_and_retains_production_lag() {
        let input = ["--database", "unused.sqlite", "--output", "unused-output"].map(str::to_owned);
        let args = crate::throughput_args::parse(&input).unwrap().unwrap();
        let mut config = args.config();
        config.device = DeviceChoice::Cpu;
        config.workers = 1;
        config.rollout_size = 4;
        config.label_every = 1;
        config.imitation.mini_batch_size = 4;
        config.imitation.updates_per_round = 1;
        let mut runner = TrainingRunner::new(config, DatasetSplit::fixture()).unwrap();
        for number in 1..=2 {
            let report = runner.iterate(&mut || true).unwrap().unwrap();
            let lagged = iteration(&report, number, true, 1.0, 0.0, None);
            assert_eq!(lagged["update_attribution"], "production_install_lag");
            assert_eq!(
                lagged["reported_update_of_iteration_zero_based"],
                json!(report.update_of_iteration)
            );
            runner.sync_update().unwrap();
            let row = iteration(&report, number, true, 1.0, 0.5, Some(runner.stats()));
            assert_eq!(row["update_attribution"], "explicit_same_iteration");
            assert_eq!(
                row["reported_update_of_iteration_zero_based"],
                json!(number - 1)
            );
            assert_eq!(
                row["production_reported_update"]["iteration_zero_based"],
                json!(report.update_of_iteration)
            );
            assert_eq!(row["imitation"], json!(runner.stats().last_imitation));
            // A final no-op drain must retain the last completed update's diagnostics.
            let final_update = update_diagnostics(runner.stats());
            runner.sync_update().unwrap();
            assert_eq!(update_diagnostics(runner.stats()), final_update);
            assert_eq!(final_update["iteration_zero_based"], json!(number - 1));
        }
    }
    #[test]
    fn targeted_pipeline_keeps_global_padding_and_rejects_nontraining_entries() {
        let input = ["--database", "unused.sqlite", "--output", "unused-output"].map(str::to_owned);
        let mut args = crate::throughput_args::parse(&input).unwrap().unwrap();
        let mut split = DatasetSplit::fixture();
        let padding = split.max_tiers;
        let chosen = split.train[0];
        args.entries = Some(vec![chosen]);
        select_pipeline_entries(&args, &mut split).unwrap();
        assert_eq!(split.train, [chosen]);
        assert_eq!(split.max_tiers, padding);
        let outside = split.entries.len();
        split.entries.push(split.entries[chosen].clone());
        args.entries = Some(vec![outside]);
        assert!(select_pipeline_entries(&args, &mut split).is_err());
        assert_eq!(split.train, [chosen]);
    }

    #[test]
    fn projection_route_differences_are_diagnostic_but_invalid_results_and_unstable_repeats_fail() {
        use pleochroma_ai::env::{
            ContinuousTierAction, ProjectionActionReport, ProjectionAttemptReport,
            ProjectionComparison, ProjectionProbeConfig, ProjectionProbeReport,
        };
        let mut report = ProjectionProbeReport {
            config: ProjectionProbeConfig::default(),
            tier_count: 1,
            candidate_route: "cuda_geometry",
            move_space_setup_ms: 0.0,
            cuda_prepare_ms: 0.0,
            geometry_before: None,
            geometry_after: None,
            actions: vec![ProjectionActionReport {
                action: ContinuousTierAction::new(vec![0.1]),
                proposal_error: None,
                cpu_reference: None,
                candidate_first: None,
                candidate_repeats: Vec::new(),
                comparison: Some(ProjectionComparison {
                    same_acceptance: false,
                    exact_output_equal: None,
                    finite_differences: None,
                    max_angle_difference_deg: None,
                    max_pinned_mast_difference: None,
                }),
                repeat_matches_first: vec![true],
            }],
        };
        assert!(projection_failures(&report).is_empty());
        report.actions[0].repeat_matches_first[0] = false;
        assert_eq!(projection_failures(&report).len(), 1);
        report.actions[0].candidate_first = Some(ProjectionAttemptReport {
            wall_ms: 0.0,
            accepted: true,
            returned_noop: false,
            refusal_kind: None,
            refusal: None,
            verification_wall_ms: 0.0,
            original_gate_accepts: Some(true),
            original_gate_refusal_kind: None,
            original_gate_refusal: None,
            finite_authored_parameters: Some(false),
            requested_bounds_accepts: Some(true),
            step_limit_accepts: Some(true),
            validation_passed: Some(false),
            geometry_after: None,
        });
        assert_eq!(projection_failures(&report).len(), 2);
    }
}
