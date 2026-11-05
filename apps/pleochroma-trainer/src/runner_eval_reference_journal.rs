//! Model-independent validation reference identities, separate from iteration checkpoints.

use pleochroma_ai::env::{OptimizerEnvConfig, RetargetEnvConfig};
use serde::Serialize;

use super::{
    EvalJob, Journal, ObjectiveWeights, Path, PathBuf, RunnerError, encode, failure, stamp,
};

const REFERENCE_VERSION: u32 = 1;

#[derive(Serialize)]
struct ReferenceIdentity {
    version: u32,
    trainer_version: &'static str,
    observation_layout: u32,
    environment_math: &'static str,
    teacher_search: &'static str,
    noise_policy_math: &'static str,
    entry: usize,
    seed: u64,
    context_seed: u64,
    design: String,
    environment_config: String,
    line_search: Vec<f64>,
    noise_std_bits: u32,
}

pub(super) fn open_corpus(directory: &Path, corpus: &str) -> Result<PathBuf, RunnerError> {
    let parent = directory
        .parent()
        .ok_or_else(|| failure("reference directory", "missing run directory"))?;
    let root = parent
        .join("reference-cases-v1")
        .join(format!("corpus-{:016x}", stamp(corpus.as_bytes())));
    // No model weights or training hyperparameters enter this namespace. A corpus digest
    // collision is refused by the full identity comparison, never treated as a cache hit.
    Journal::open_identity(&root, corpus.as_bytes())?;
    Ok(root)
}

/// Build the same effective environment configuration as `runner_env::build_env`, without
/// constructing geometry or scoring the baseline. Debug uses round-trippable float formatting
/// and includes the complete resolved materials, rather than a catalogue name alone.
pub(super) fn identity(
    job: EvalJob<'_>,
    entry: usize,
    weights: Option<ObjectiveWeights>,
) -> Result<Vec<u8>, RunnerError> {
    let item = job
        .split
        .entries
        .get(entry)
        .ok_or_else(|| failure("reference identity", "entry is outside the corpus"))?;
    let salted = job.seed ^ (entry as u64).wrapping_mul(0x9E37_79B9);
    let target = super::super::pick_target(
        job.cfg,
        salted,
        &item.design,
        item.authored_material.as_ref(),
    )?;
    let weights = weights.unwrap_or_else(|| super::super::draw_weights(job.cfg.objectives, salted));
    let source = super::super::sampled_source_material(
        salted,
        &item.design,
        item.authored_material.as_ref(),
    )?
    .1;
    let (shape_target, lighting) = super::super::sampled_context(salted);
    let fidelity = super::super::objective_fidelity(job.fidelity);
    let environment_config = if let Some(target) = target {
        let mut config = RetargetEnvConfig::new_with_target(target.material);
        config.weights = weights;
        config.source_material_override = Some(source);
        config.shape_target = shape_target;
        config.lighting = lighting;
        config.fidelity = fidelity;
        if let Some(h) = job.cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = job.cfg.label_h_deg.unwrap_or(config.max_step_deg);
        format!("{config:?}")
    } else {
        let mut config = OptimizerEnvConfig {
            weights,
            shape_target,
            lighting,
            source_material_override: Some(source),
            fidelity,
            ..OptimizerEnvConfig::default()
        };
        if let Some(h) = job.cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = job.cfg.label_h_deg.unwrap_or(config.max_step_deg);
        format!("{config:?}")
    };
    encode(&ReferenceIdentity {
        version: REFERENCE_VERSION,
        trainer_version: env!("CARGO_PKG_VERSION"),
        observation_layout: pleochroma_ai::env::OBS_LAYOUT_VERSION,
        environment_math: if job.execution.is_some() {
            pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
        } else {
            "cpu-env-v1"
        },
        teacher_search: pleochroma_ai::env::TEACHER_SEARCH_RECOVERY_REVISION,
        noise_policy_math: super::POLICY_MATH,
        entry,
        seed: job.seed,
        context_seed: salted,
        design: format!("{:?}", item.design),
        environment_config,
        line_search: job.cfg.line_search.clone(),
        noise_std_bits: job.cfg.explore_std.to_bits(),
    })
}

#[cfg(test)]
#[path = "runner_eval_reference_journal_tests.rs"]
mod tests;
