//! Recovery-aware evaluation dispatch; only unfinished cases request device inference.

use std::path::Path;

use super::{
    CaseProgress, DesignOutcome, DesignResult, EvalFidelity, EvalJob, EvalPasses, EvalReport,
    Optics, RunnerError, policy_inference, pool, recovery,
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
    let mut pending = 0;
    let mut tasks = tasks
        .into_iter()
        .enumerate()
        .map(|(i, (entry, weights))| {
            let restored = journal
                .as_ref()
                .map(|journal| journal.load(i, entry, weights))
                .transpose()?
                .flatten();
            pending += usize::from(restored.is_none());
            Ok((entry, weights, restored))
        })
        .collect::<Result<Vec<_>, RunnerError>>()?;
    let service = if pending > 0 && job.backend != super::super::CPU_LABEL {
        job.progress
            .phase("initialize validation GPU policy", pending);
        Some(policy_inference::Service::start(
            job.net.clone(),
            job.cfg.clone(),
            job.backend,
            job.split.max_tiers,
            pending,
        )?)
    } else {
        None
    };
    // Reborrow immutable job inputs for the worker lifetime instead of extending its borrow.
    let job = EvalJob {
        inference: service.as_ref().map(policy_inference::Service::client),
        ..job
    };
    let costs: Vec<_> = tasks
        .iter()
        .map(|(entry, _, restored)| {
            if restored.is_some() {
                0
            } else {
                job.split.entries[*entry].design.tiers.len() as u64
            }
        })
        .collect();
    let phase = match job.backend {
        super::super::CUDA_LABEL if job.execution.is_some() => {
            "CUDA policy/physics; CPU oracle search"
        }
        super::super::CUDA_LABEL => "CUDA policy; CPU geometry/oracle",
        super::super::GPU_LABEL => "WGPU policy; CPU geometry/oracle",
        _ => "CPU evaluation",
    };
    job.progress.phase(phase, tasks.len());
    let threads = pool::pass_threads(pending);
    let results = pool::run_items_ranked(&mut tasks, &costs, threads, |i, task, tail| {
        let (entry, weights, restored) = task;
        let progress = CaseProgress {
            task: job
                .progress
                .task(format!("case {} entry {entry}: initialize material", i + 1)),
            case: i + 1,
            entry: *entry,
            tail,
        };
        if let Some(result) = restored.take() {
            progress.stage("restored completed validation case (no policy call)");
            return Ok(result);
        }
        progress.stage("initialize material on CPU");
        let result = job.evaluate_design(*entry, *weights, &progress)?;
        if let Some(journal) = &journal {
            progress.stage("commit completed validation case");
            journal.save(i, *entry, *weights, &result)?;
        }
        Ok(result)
    });
    job.check_execution()?;
    reduce(job.fidelity, job.passes, results)
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
                oracle_drop += f.oracle_final_score.map_or(0.0, |s| f.base_score - s);
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
        report.mean_improvement_oracle = passes.oracle.then_some(oracle_drop / n);
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
