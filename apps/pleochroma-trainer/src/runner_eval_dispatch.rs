//! Recovery-aware evaluation dispatch; only unfinished cases request device inference.

use std::path::Path;

use super::{
    CaseProgress, DesignOutcome, DesignResult, EvalFidelity, EvalJob, EvalPasses, EvalReport,
    ObjectiveWeights, Optics, RunnerError, TrainingProgress, policy_inference, pool, recovery,
};

pub(in super::super) fn evaluate_designs(
    job: EvalJob<'_>,
    entries: &[usize],
    directory: Option<&Path>,
) -> Result<EvalReport, RunnerError> {
    job.check_execution()?;
    let tasks: Vec<_> = if job.cfg.objective_sweep {
        entries
            .iter()
            .flat_map(|&entry| {
                pleochroma_cut_core::ObjectivePreset::ALL
                    .into_iter()
                    .map(move |preset| (entry, Some(preset.weights())))
            })
            .collect()
    } else {
        entries.iter().map(|&entry| (entry, None)).collect()
    };
    job.progress
        .phase("validation recovery journal", tasks.len());
    let journal = directory
        .map(|directory| recovery::Journal::open(directory, job, entries, &tasks))
        .transpose()?;
    // Restore before touching the GPU. A fully completed journal needs no device calls.
    let tasks = tasks
        .into_iter()
        .enumerate()
        .map(|(i, (entry, weights))| {
            let restored = journal
                .as_ref()
                .map(|journal| journal.load(i, entry, weights))
                .transpose()?
                .flatten();
            Ok((entry, weights, restored))
        })
        .collect::<Result<Vec<_>, RunnerError>>()?;
    let policy_pending = tasks.iter().enumerate().try_fold(
        0usize,
        |count, (index, (entry, weights, restored))| -> Result<usize, RunnerError> {
            if restored.is_some() {
                return Ok(count);
            }
            let needs_policy = if let Some(journal) = &journal {
                super::episode::needs_policy(
                    &journal.case(index, *entry, *weights),
                    job.passes.sampled,
                )?
            } else {
                true
            };
            Ok(count + usize::from(needs_policy))
        },
    )?;
    let service = if policy_pending > 0 && job.backend != super::super::CPU_LABEL {
        job.progress
            .phase("initialize validation GPU policy", policy_pending);
        Some(policy_inference::Service::start(
            job.net.clone(),
            job.cfg.clone(),
            job.backend,
            job.split.max_tiers,
            policy_pending,
        )?)
    } else {
        None
    };
    // Reborrow immutable job inputs for the worker lifetime instead of extending its borrow.
    let job = EvalJob {
        inference: service.as_ref().map(policy_inference::Service::client),
        ..job
    };
    let phase = match job.backend {
        super::super::CUDA_LABEL if job.execution.is_some() => {
            "validation: CUDA policy/physics; host search control"
        }
        super::super::CUDA_LABEL => "validation: CUDA policy; CPU physics/search",
        super::super::GPU_LABEL => "validation: WGPU policy; CPU physics/search",
        _ => "validation: CPU policy/physics/search",
    };
    job.progress.phase(phase, tasks.len());
    let results = run_pending_cases(
        tasks,
        job.progress,
        |entry| job.split.entries[entry].design.tiers.len() as u64,
        |index, entry, weights, progress| {
            progress.stage("initialize material on CPU");
            let parts = journal
                .as_ref()
                .map(|journal| journal.case(index, entry, weights));
            let references = journal
                .as_ref()
                .map(|journal| journal.reference_case(job, entry, weights))
                .transpose()?;
            let result = job.evaluate_design_with_recovery(
                entry,
                weights,
                progress,
                parts.as_ref(),
                references.as_ref(),
            )?;
            if let Some(journal) = &journal {
                progress.stage("commit completed validation case");
                journal.save(index, entry, weights, &result)?;
            }
            Ok(result)
        },
    );
    job.check_execution()?;
    let mut report = reduce(job.fidelity, job.passes, results)?;
    report.distinct_entries = entries
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    report.distinct_families = job.split.distinct_families(entries);
    Ok(report)
}

/// Completed cases occupy their original result slots, but never count as unfinished pool work.
/// Otherwise their zero-cost tasks wait behind pending cases and suppress spare-worker sharing.
fn run_pending_cases(
    tasks: Vec<(usize, Option<ObjectiveWeights>, Option<DesignResult>)>,
    progress: &TrainingProgress,
    cost: impl Fn(usize) -> u64,
    evaluate: impl Fn(usize, usize, Option<ObjectiveWeights>, &CaseProgress<'_>) -> DesignOutcome + Sync,
) -> Vec<DesignOutcome> {
    let mut results = Vec::with_capacity(tasks.len());
    let mut pending = Vec::new();
    for (index, (entry, weights, restored)) in tasks.into_iter().enumerate() {
        if let Some(mut result) = restored {
            if let DesignResult::Figures(figures) = &mut result {
                figures.case.oracle_reference_cached =
                    figures.oracle_final_score.is_some_and(f32::is_finite);
                figures.case.noise_reference_cached =
                    figures.case.noise_final_score.is_some_and(f32::is_finite);
                figures.case.references_cached =
                    figures.case.oracle_reference_cached || figures.case.noise_reference_cached;
            }
            results.push(Some(Ok(result)));
            drop(progress.task(format!(
                "case {} entry {entry}: restored completed validation case (no policy call)",
                index + 1,
            )));
        } else {
            results.push(None);
            pending.push((index, entry, weights));
        }
    }
    let costs: Vec<_> = pending.iter().map(|(_, entry, _)| cost(*entry)).collect();
    let threads = pool::pass_threads(pending.len());
    let completed = pool::run_items_ranked(&mut pending, &costs, threads, |_, task, tail| {
        let (index, entry, weights) = task;
        let progress = CaseProgress {
            task: progress.task(format!(
                "case {} entry {entry}: initialize material",
                *index + 1,
            )),
            case: *index + 1,
            entry: *entry,
            tail,
        };
        evaluate(*index, *entry, *weights, &progress)
    });
    for ((index, _, _), result) in pending.into_iter().zip(completed) {
        results[index] = Some(result);
    }
    results
        .into_iter()
        .map(|result| result.expect("every validation case is restored or evaluated"))
        .collect()
}

/// Adds the readings of `v` to the running sums in `acc`.
fn add_optics(acc: &mut Optics, v: Optics) {
    acc.brilliance += v.brilliance;
    acc.windowing += v.windowing;
    acc.extinction += v.extinction;
    acc.yield_pct += v.yield_pct;
}

/// The sums in `o` divided by the design count `n`.
fn mean_optics(o: Optics, n: f32) -> Optics {
    Optics {
        brilliance: o.brilliance / n,
        windowing: o.windowing / n,
        extinction: o.extinction / n,
        yield_pct: o.yield_pct / n,
    }
}

/// Serial reduction in entry order: sums, failure count and the first failure (the first failing
/// entry), or the first hard error. The order is the entry order whatever the thread count, so
/// the sums round identically.
pub(super) fn reduce(
    fidelity: EvalFidelity,
    passes: EvalPasses,
    results: Vec<DesignOutcome>,
) -> Result<EvalReport, RunnerError> {
    let mut report = EvalReport {
        full_fidelity: fidelity == EvalFidelity::Full,
        ..EvalReport::default()
    };
    let (mut base_score, mut final_score) = (0.0f32, 0.0f32);
    let (mut sampled_drop, mut oracle_drop) = (0.0f32, 0.0f32);
    let (mut noise_drop, mut noise_count) = (0.0f32, 0usize);
    let (mut oracle_count, mut cached_oracles, mut cached_noise) = (0usize, 0usize, 0usize);
    let (mut model_evaluations, mut model_steps) = (0usize, 0usize);
    let (mut base, mut fin) = (Optics::default(), Optics::default());
    for result in results {
        match result? {
            DesignResult::Failed(case) => {
                report.failed += 1;
                report
                    .first_failure
                    .get_or_insert_with(|| case.failure.clone().unwrap_or_default());
                report.cases.push(case);
            }
            DesignResult::Figures(f) => {
                report.designs += 1;
                base_score += f.base_score;
                final_score += f.final_score;
                sampled_drop += f.sampled_final_score.map_or(0.0, |s| f.base_score - s);
                if let Some(score) = f.oracle_final_score.filter(|score| score.is_finite()) {
                    oracle_drop += f.base_score - score;
                    oracle_count += 1;
                    cached_oracles += usize::from(f.case.oracle_reference_cached);
                }
                if let Some(score) = f.case.noise_final_score.filter(|score| score.is_finite()) {
                    noise_drop += f.base_score - score;
                    noise_count += 1;
                    cached_noise += usize::from(f.case.noise_reference_cached);
                }
                model_evaluations += f.model_evaluations;
                model_steps += f.model_steps;
                add_optics(&mut base, f.base);
                add_optics(&mut fin, f.fin);
                report.cases.push(f.case);
            }
        }
    }
    if report.designs > 0 {
        let n = report.designs as f32;
        report.mean_base_score = base_score / n;
        report.mean_final_score = final_score / n;
        report.mean_improvement = report.mean_base_score - report.mean_final_score;
        report.mean_improvement_sampled = passes.sampled.then_some(sampled_drop / n);
        report.mean_improvement_oracle =
            (passes.oracle && oracle_count == report.designs).then_some(oracle_drop / n);
        report.mean_improvement_noise =
            (passes.noise && noise_count == report.designs).then_some(noise_drop / n);
        report.oracle_cached =
            report.mean_improvement_oracle.is_some() && cached_oracles == report.designs;
        report.noise_cached =
            report.mean_improvement_noise.is_some() && cached_noise == report.designs;
        report.mean_evaluations_per_step = if model_steps == 0 {
            0.0
        } else {
            model_evaluations as f32 / model_steps as f32
        };
        report.base_optics = mean_optics(base, n);
        report.final_optics = mean_optics(fin, n);
    }
    Ok(report)
}

#[cfg(test)]
#[path = "runner_eval_dispatch_tests.rs"]
mod tests;
