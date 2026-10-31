//! Parallel evaluation of the model on a set of designs, in up to three passes (review TQ-16).
//!
//! Every design is played from its base state by the same machinery, [`EvalJob::play`], with a
//! different [`Pilot`] choosing the action of each step:
//!
//! - the **model** (always): the policy mean, projected, stepped with the run's line search;
//! - the **sampled** pass: one stochastic episode, the exploration policy of the run (the lower
//!   bound a trained model must beat);
//! - the **oracle** pass: the teacher's verified best action among coordinate probes,
//!   gradient line search and bounded rescue probes, replayed at its measured scale. This
//!   finite search is a reference policy, not a guaranteed upper bound.
//!
//! All three score the same stone against the same target under the same objective weights, so
//! `model / oracle` is the figure to watch. The end-of-run test evaluation and every in-training
//! validation pass previously played designs one after another on one core. Designs are
//! independent, so they are handed to the persistent bounded CPU pool (a design with many free
//! tiers costs many times a small one) and every outcome is written into the slot of its entry;
//! the report is then reduced serially in entry order. GPU/CUDA policy forwards share one
//! bounded device owner which batches ready cases. CPU evaluation keeps the pure network;
//! GPU floating point values may vary across backends and batch shapes.

use std::{sync::Arc, time::Instant};

#[path = "runner_eval_dispatch.rs"]
mod dispatch;
#[path = "runner_eval_inference.rs"]
mod policy_inference;
pub(super) use dispatch::evaluate_designs;
#[cfg(test)]
use dispatch::reduce;

#[path = "runner_eval_recovery.rs"]
mod recovery;
#[path = "runner_eval_work.rs"]
mod work;

use pleochroma::{geometry::stone_metrics::measure_solid, optics::materials::GemMaterial};
use pleochroma_ai::env::material_features::material_for_design;
use pleochroma_ai::inference::score_design_at_fidelity;
use pleochroma_ai::{
    env::{ContinuousTierAction, EnvExecution, GemstoneObservation, action_from_direction},
    model::{GaussianPolicy, GemstoneSetAttentionNetwork, InferenceScratch, Prng, TrainingMode},
};
use pleochroma_cut_core::{
    Design, ObjectiveWeights, ResolvedMaterial, ShapeTarget,
    design::tier_hinges,
    meet_preservation::PreservationPolicy,
    optimize::{
        ObjectiveFidelity, OptimizeConfig, OptimizeOptions, SearchHooks, optimize_design_with,
    },
};

use super::{
    Algorithm, EvalCase, EvalFidelity, EvalPasses, EvalReport, ObjectiveSampling, Optics,
    RunConfig, RunnerError, build_env, draw_weights,
    envs::{EnvKind, Target, sampled_context, sampled_source_material},
    imitation::explore_raw,
    pick_target, pool,
    progress::{TaskGuard, TrainingProgress},
};
use crate::dataset::{DatasetSplit, DesignEntry};

/// Everything the evaluation threads read. All of it is shared immutably, so the job is `Copy`
/// and `Send`; the trainer, the optimizer state and the environments of the rollout never enter.
#[derive(Clone, Copy)]
pub(super) struct EvalJob<'a> {
    /// The network that plays (its `forward` is pure).
    pub(super) net: &'a GemstoneSetAttentionNetwork,
    /// Resolved training backend, also used by model/sample evaluation.
    pub(super) backend: &'static str,
    /// Owned device worker; absent for the pure CPU reference and restored cases.
    pub(super) inference: Option<&'a policy_inference::Client>,
    /// Optional accelerated environment, shared by the model and reference passes.
    pub(super) execution: Option<&'a Arc<EnvExecution>>,
    /// The data set the entry indices refer to.
    pub(super) split: &'a DatasetSplit,
    /// Task, target selection, objective sampling, line search and step-limit override.
    pub(super) cfg: &'a RunConfig,
    /// Scoring fidelity of the reported figures.
    pub(super) fidelity: EvalFidelity,
    /// Run seed: fixes the targets, the weights and the stochastic episodes.
    pub(super) seed: u64,
    /// The passes besides the model's own.
    pub(super) passes: EvalPasses,
    /// Diagnostic progress only; never part of policy or scoring decisions.
    pub(super) progress: &'a TrainingProgress,
}

/// What the cached oracle drop depends on: the validation designs, the seed that draws their
/// targets and weights, and every setting of the run that shapes the oracle episode. The oracle
/// does not depend on the model, so while the key is unchanged the drop is too.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct OracleKey {
    entries: Vec<usize>,
    seed: u64,
    max_step_deg: f64,
    mode: TrainingMode,
    universal_spectrum: bool,
    fixed_target: String,
    line_search: Vec<f64>,
    label_h_deg: Option<f64>,
    objectives: ObjectiveSampling,
    same_material_frac: f32,
    #[serde(with = "super::recovery::fidelity")]
    train_fidelity: ObjectiveFidelity,
    validation_fidelity: EvalFidelity,
    objective_sweep: bool,
    #[serde(default = "cpu_environment_math")]
    environment_math: String,
}

fn cpu_environment_math() -> String {
    "cpu-env-v1".to_owned()
}

impl OracleKey {
    /// The key of an oracle pass over `entries` with `seed`, under the step limit `max_step_deg`
    /// that is in effect.
    pub(super) fn new(cfg: &RunConfig, entries: &[usize], seed: u64, max_step_deg: f64) -> Self {
        Self {
            entries: entries.to_vec(),
            seed,
            max_step_deg,
            mode: cfg.mode,
            universal_spectrum: cfg.universal_spectrum,
            fixed_target: cfg.fixed_target.clone(),
            line_search: cfg.line_search.clone(),
            label_h_deg: cfg.label_h_deg,
            objectives: cfg.objectives,
            same_material_frac: cfg.same_material_frac,
            train_fidelity: cfg.train_fidelity,
            validation_fidelity: cfg.validation_fidelity,
            objective_sweep: cfg.objective_sweep,
            environment_math: cpu_environment_math(),
        }
    }

    pub(super) fn with_environment(mut self, accelerated: bool) -> Self {
        if accelerated {
            self.environment_math = pleochroma_ai::env::CUDA_ENVIRONMENT_MATH.to_owned();
        }
        self
    }
}

/// The oracle's mean score drop on the validation designs, kept for as long as its [`OracleKey`]
/// matches.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct OracleCache {
    pub(super) key: OracleKey,
    pub(super) drop: f32,
}

/// Figures of one evaluated design.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct DesignFigures {
    case: EvalCase,
    /// Score before the episode.
    base_score: f32,
    /// Score after the model's episode.
    final_score: f32,
    /// Optics before the episode.
    base: Optics,
    /// Optics after the model's episode.
    fin: Optics,
    /// Score after the stochastic episode (same fidelity); `None` when it was not played.
    sampled_final_score: Option<f32>,
    /// Score after the oracle episode (same fidelity); `None` when it was not played.
    oracle_final_score: Option<f32>,
    /// Candidate evaluations the model's episode spent.
    model_evaluations: usize,
    /// Environment steps the model's episode took.
    model_steps: usize,
}

/// What one design came to: figures, or the reason it could not be evaluated.
#[derive(serde::Serialize, serde::Deserialize)]
enum DesignResult {
    Figures(DesignFigures),
    Failed(EvalCase),
}

/// A design's outcome: its result, or the error that stops the evaluation.
type DesignOutcome = Result<DesignResult, RunnerError>;

/// Who chooses the action of each step of an episode.
enum Pilot {
    /// The policy mean.
    Model,
    /// One draw per step. `explore_std` is the exploration noise around the mean (imitation
    /// runs); `None` draws from the policy's own distribution (PPO runs).
    Sampled { rng: Prng, explore_std: Option<f32> },
    /// The finite-difference oracle.
    Oracle,
}

impl Pilot {
    fn name(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Sampled { .. } => "sampled",
            Self::Oracle => "oracle",
        }
    }
}

struct CaseProgress<'a> {
    task: TaskGuard,
    case: usize,
    entry: usize,
    tail: &'a pool::Tail,
}

impl CaseProgress<'_> {
    fn stage(&self, detail: &str) {
        self.task
            .update_detail(format!("case {} entry {}: {detail}", self.case, self.entry));
    }
}

/// How an episode went.
struct Played {
    /// The observation after the last step.
    obs: GemstoneObservation,
    /// Environment steps taken.
    steps: usize,
    /// Candidates the steps and any teacher search scored (not reset/endpoint scoring).
    evaluations: usize,
}

struct ReplayScore {
    score: f32,
    evaluations: usize,
    steps: usize,
}

/// What every pass of one design shares: the target it is judged against and the objective
/// weights it is scored with.
struct DesignContext {
    target: Option<Target>,
    source: ResolvedMaterial,
    weights: ObjectiveWeights,
    context_seed: u64,
}

impl EvalJob<'_> {
    fn check_execution(&self) -> Result<(), RunnerError> {
        if let Some(execution) = self.execution {
            execution.check().map_err(RunnerError::Data)?;
        }
        Ok(())
    }

    /// The exploration noise of the sampled pass: the run's `explore_std` in imitation mode,
    /// `None` (the policy's own distribution) in PPO mode.
    fn explore_std(&self) -> Option<f32> {
        (self.cfg.algo == Algorithm::Imitation).then_some(self.cfg.explore_std)
    }

    /// Replays the teacher's verified action; an unsuccessful search proposes no motion.
    fn oracle_action(
        &self,
        env: &EnvKind,
        max_step: f64,
        progress: &CaseProgress<'_>,
    ) -> (ContinuousTierAction, usize) {
        let h = self.cfg.label_h_deg.unwrap_or(max_step);
        let label = work::oracle(env, h, &self.cfg.line_search, progress.tail);
        let raw = label
            .action_target
            .unwrap_or_else(|| vec![0.0; label.tiers.len()]);
        (action_from_direction(&raw, max_step), label.evaluations)
    }

    /// The (unprojected) action and teacher evaluations used to choose it.
    fn propose(
        &self,
        env: &EnvKind,
        obs: &GemstoneObservation,
        pilot: &mut Pilot,
        scratch: &mut InferenceScratch,
        progress: &CaseProgress<'_>,
    ) -> Result<(ContinuousTierAction, usize), RunnerError> {
        let max_step = env.max_step_deg();
        let raw = match pilot {
            Pilot::Oracle => return Ok(self.oracle_action(env, max_step, progress)),
            Pilot::Model => self.policy(obs, scratch)?.mean,
            Pilot::Sampled { rng, explore_std } => {
                let policy = self.policy(obs, scratch)?;
                let free = obs.free_tier_mask();
                match *explore_std {
                    Some(std) => explore_raw(&policy.mean, &free, std, rng),
                    None => policy.sample_raw_masked(rng, &free).0,
                }
            }
        };
        Ok((GaussianPolicy::squash(&raw, max_step), 0))
    }

    /// Policy numbers come from the selected backend; RNG and action handling stay per case.
    fn policy(
        &self,
        obs: &GemstoneObservation,
        scratch: &mut InferenceScratch,
    ) -> Result<GaussianPolicy, RunnerError> {
        if let Some(inference) = self.inference {
            inference.policy(obs)
        } else if self.backend == super::CPU_LABEL {
            Ok(self
                .net
                .forward_with_scratch(&obs.tier_tokens(), &obs.global_token(), scratch)?
                .0)
        } else {
            Err(RunnerError::Data(format!(
                "validation policy on {} has no inference worker",
                self.backend,
            )))
        }
    }

    /// Plays one episode from `obs` to its end and returns how it went: every step is proposed
    /// by `pilot`; model actions use rollout projection and line search. Oracle actions replay
    /// the exact measured input at unit scale. Counts include the oracle's teacher search.
    ///
    /// The oracle stops as soon as a step is not accepted: the environment is greedy and
    /// deterministic, a refused step leaves the design as it was, and the oracle's action
    /// depends on the design alone, so every later step would repeat the refusal. The final
    /// score is the same as if the episode were played out, at a fraction of the cost.
    fn play(
        &self,
        env: &mut EnvKind,
        mut obs: GemstoneObservation,
        mut pilot: Pilot,
        scratch: &mut InferenceScratch,
        progress: &CaseProgress<'_>,
    ) -> Result<Played, RunnerError> {
        let (mut steps, mut evaluations) = (0, 0);
        loop {
            self.check_execution()?;
            let max_step = env.max_step_deg();
            let backend = if matches!(pilot, Pilot::Oracle) {
                "CPU oracle"
            } else {
                self.backend
            };
            progress.stage(&format!(
                "{} step {}: propose on {backend}",
                pilot.name(),
                steps + 1
            ));
            let (mut action, teacher_evaluations) =
                self.propose(env, &obs, &mut pilot, scratch, progress)?;
            self.check_execution()?;
            let oracle = matches!(pilot, Pilot::Oracle);
            if !oracle {
                GemstoneSetAttentionNetwork::project_actions(
                    &mut action,
                    &obs.tiers,
                    obs.global.target_critical_angle_deg,
                    max_step,
                );
            }
            // The teacher already selected the scale and evaluated environment projection.
            // Re-scaling or projecting through rounded observation bounds changes its action.
            let scales: &[f64] = if oracle {
                &[1.0]
            } else {
                &self.cfg.line_search
            };
            let environment_backend = if self.execution.is_some() {
                "CUDA"
            } else {
                "CPU"
            };
            progress.stage(&format!(
                "{} step {}: geometry and optical scoring on {environment_backend}",
                pilot.name(),
                steps + 1
            ));
            let step = work::step(env, &action, scales, progress.tail);
            self.check_execution()?;
            steps += 1;
            evaluations += teacher_evaluations + usize::from(step.info.evaluations);
            let done = step.terminated || step.truncated;
            let stalled = matches!(pilot, Pilot::Oracle) && step.info.scale.is_none();
            obs = step.observation;
            if done || stalled {
                return Ok(Played {
                    obs,
                    steps,
                    evaluations,
                });
            }
        }
    }

    /// Plays `pilot` from the base design again and returns the final score.
    ///
    /// Every step, observation and acceptance decision uses this job's evaluation fidelity.
    fn replay(
        &self,
        env: &mut EnvKind,
        pilot: Pilot,
        scratch: &mut InferenceScratch,
        progress: &CaseProgress<'_>,
    ) -> Result<ReplayScore, RunnerError> {
        progress.stage(&format!("{}: reset episode", pilot.name()));
        let obs = env.reset(self.seed);
        let played = self.play(env, obs, pilot, scratch, progress)?;
        Ok(ReplayScore {
            score: env.current_score(),
            evaluations: played.evaluations,
            steps: played.steps,
        })
    }

    /// Evaluates the model and requested reference policies at the job's fidelity.
    ///
    /// An environment that cannot be built produces a [`DesignResult::Failed`]; a material
    /// lookup or network failure stops the evaluation instead.
    fn evaluate_design(
        &self,
        entry: usize,
        objective_override: Option<ObjectiveWeights>,
        progress: &CaseProgress<'_>,
    ) -> DesignOutcome {
        self.check_execution()?;
        let started = Instant::now();
        let item = &self.split.entries[entry];
        let salted = self.seed ^ (entry as u64).wrapping_mul(0x9E37_79B9);
        let target = pick_target(
            self.cfg,
            salted,
            &item.design,
            item.authored_material.as_ref(),
        )?;
        let weights =
            objective_override.unwrap_or_else(|| draw_weights(self.cfg.objectives, salted));
        let mut case = new_case(item, target.as_ref(), weights, salted)?;
        progress.stage("build environment and score baseline on selected physics backend");
        let result = build_env(
            self.cfg,
            &item.design,
            target.as_ref(),
            weights,
            (Some(objective_fidelity(self.fidelity)), self.execution),
            salted,
            item.authored_material.as_ref(),
        );
        self.check_execution()?;
        let mut env = match result {
            Ok(env) => env,
            Err(e) => {
                case.failure = Some(format!("'{}': {e}", item.title));
                return Ok(DesignResult::Failed(case));
            }
        };
        let ctx = DesignContext {
            target,
            source: env.source_material().clone(),
            weights,
            context_seed: salted,
        };
        let mut scratch = InferenceScratch::default();

        progress.stage("model: reset episode");
        let obs = env.reset(self.seed);
        let base_optics = Optics::from_global(&obs.global);
        let model_started = Instant::now();
        let model = self.play(&mut env, obs, Pilot::Model, &mut scratch, progress)?;
        let final_optics = Optics::from_global(&model.obs.global);
        let (base_score, final_score, base, fin) = (
            env.base_score(),
            env.current_score(),
            base_optics,
            final_optics,
        );
        let mut figures = DesignFigures {
            case: case.clone(),
            base_score,
            final_score,
            base,
            fin,
            sampled_final_score: None,
            oracle_final_score: None,
            model_evaluations: model.evaluations,
            model_steps: model.steps,
        };
        figures.case.model_seconds = Some(model_started.elapsed().as_secs_f32());

        // The same environment, back at the base design: every extra pass starts from the very
        // stone, target and weights the model's episode did.
        if self.passes.sampled {
            let pilot = Pilot::Sampled {
                rng: Prng::new(salted),
                explore_std: self.explore_std(),
            };
            let replay = self.replay(&mut env, pilot, &mut scratch, progress)?;
            figures.sampled_final_score = Some(replay.score);
        }
        if self.passes.oracle {
            let oracle_started = Instant::now();
            let replay = self.replay(&mut env, Pilot::Oracle, &mut scratch, progress)?;
            figures.oracle_final_score = Some(replay.score);
            figures.case.oracle_final_score = Some(replay.score);
            figures.case.oracle_evaluations = Some(replay.evaluations);
            figures.case.oracle_steps = Some(replay.steps);
            figures.case.oracle_seconds = Some(oracle_started.elapsed().as_secs_f32());
        }
        figures.case.base_score = Some(figures.base_score);
        figures.case.final_score = Some(figures.final_score);
        figures.case.score_drop = Some(figures.base_score - figures.final_score);
        figures.case.accepted = Some(figures.final_score < figures.base_score);
        figures.case.evaluations = figures.model_evaluations;
        figures.case.steps = figures.model_steps;
        // Keep model throughput separate from the optional comparator's CPU time.
        figures.case.elapsed_seconds = Some(started.elapsed().as_secs_f32());
        if let Some(budget) = self.cfg.classical_budget {
            progress.stage("classical optimizer comparison on CPU");
            self.classical_comparison(item, &ctx, budget, &mut figures.case);
        }
        Ok(DesignResult::Figures(figures))
    }

    /// Runs the core optimizer in the episode's material, weights, lighting and shape context.
    ///
    /// The core search keeps a baseline-sized material and chooses its own scoring stages.
    /// Endpoint scores below use the AI scorer with each candidate's actual size instead.
    fn classical_comparison(
        &self,
        item: &DesignEntry,
        ctx: &DesignContext,
        budget: usize,
        case: &mut EvalCase,
    ) {
        let salted = ctx.context_seed;
        let weights = ctx.weights;
        let (shape_target, lighting) = sampled_context(salted);
        let started = Instant::now();
        case.classical_context = Some(
            "Core Fast search with a Full final gate; material orientation and absorption scale \
             are prepared at the baseline width. Reported endpoints use the AI scorer at the \
             requested evaluation fidelity with each endpoint's own width."
                .to_owned(),
        );
        let material = if let Some(target) = &ctx.target {
            target.material.clone()
        } else {
            ctx.source.clone()
        };
        let (prepared, options) =
            match classical_inputs(&item.design, &material.gem, shape_target, self.cfg.mode) {
                Ok(inputs) => inputs,
                Err(error) => {
                    case.classical_failure = Some(error);
                    case.classical_seconds = Some(started.elapsed().as_secs_f32());
                    return;
                }
            };
        let optimizer_config = OptimizeConfig {
            weights,
            seed: salted,
            max_evaluations: budget,
            polish_start_step_deg: None,
            lighting,
            ..OptimizeConfig::default()
        };
        match optimize_design_with(
            &item.design,
            &prepared,
            &optimizer_config,
            &options,
            &SearchHooks::default(),
        ) {
            Ok(result) => {
                let report_fidelity = objective_fidelity(self.fidelity);
                let baseline = score_design_at_fidelity(
                    &item.design,
                    &material.gem,
                    &weights,
                    report_fidelity,
                    lighting,
                    shape_target,
                )
                .map_err(|error| error.to_string());
                let candidate = if result.outcome.changes.is_empty()
                    && result.mast_changes.is_empty()
                {
                    Ok(item.design.clone())
                } else {
                    pleochroma_cut_core::optimize::materialize_outcome(&item.design, &result.outcome)
                        .map(|(design, _, _)| design)
                        .map_err(|error| error.to_string())
                };
                match (baseline, candidate) {
                    (Ok(baseline), Ok(design)) => match score_design_at_fidelity(
                        &design,
                        &material.gem,
                        &weights,
                        report_fidelity,
                        lighting,
                        shape_target,
                    ) {
                        Ok(final_score) => {
                            case.classical_base_score = Some(baseline.score);
                            case.classical_final_score = Some(final_score.score);
                        }
                        Err(error) => case.classical_failure = Some(error.to_string()),
                    },
                    (Err(error), _) | (_, Err(error)) => case.classical_failure = Some(error),
                }
                case.classical_evaluations = result.outcome.evaluations;
            }
            Err(error) => case.classical_failure = Some(error.to_string()),
        }
        case.classical_seconds = Some(started.elapsed().as_secs_f32());
    }
}

/// The core optimizer accepts a resolved material, not design orientation or size overrides.
///
/// Its API keeps this material constant during search. Preserve the raw material separately
/// for `score_design_at_fidelity`, which applies each endpoint's own measured width.
fn classical_inputs(
    design: &Design,
    material: &GemMaterial,
    shape_target: Option<ShapeTarget>,
    mode: TrainingMode,
) -> Result<(GemMaterial, OptimizeOptions), String> {
    let solved = design.solve().map_err(|error| error.to_string())?;
    let planes = design.planes_from_solved(&solved);
    let width = measure_solid(&planes).map(|metrics| metrics.width_axis);
    let options = OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: tier_hinges(design, &solved)
            .into_iter()
            .map(|(index, hinge)| (index, hinge.point))
            .collect(),
        shape_target,
        preservation_policy: if mode == TrainingMode::Retarget {
            PreservationPolicy::RETARGET
        } else {
            PreservationPolicy::OPTIMIZE
        },
        min_girdle_fraction: Some(0.5),
        min_girdle_thinnest_fraction: Some(0.5),
        ..OptimizeOptions::default()
    };
    Ok((material_for_design(design, material, width), options))
}

/// The report row of `item` before it is played: which material, objective and context it
/// draws.
fn new_case(
    item: &DesignEntry,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    salted: u64,
) -> Result<EvalCase, RunnerError> {
    let (shape_target, lighting) = sampled_context(salted);
    let (source_name, _, augmentation) =
        sampled_source_material(salted, &item.design, item.authored_material.as_ref())?;
    Ok(EvalCase {
        design_id: item.id,
        design_title: item.title.clone(),
        objective: objective_name(weights),
        target_material: target.map_or_else(|| source_name.clone(), |target| target.name.clone()),
        source_material: source_name,
        seed: salted,
        material_augmentation: augmentation,
        target_augmentation: target
            .map_or_else(|| "none".to_owned(), |target| target.augmentation.clone()),
        lighting: format!("{lighting:?}"),
        shape_target: shape_target.map_or_else(
            || "none".to_owned(),
            |shape| {
                format!(
                    "table={:?}, crown/pavilion={:?}, weight={:.2}",
                    shape.table_percent, shape.crown_to_pavilion, shape.weight
                )
            },
        ),
        ..EvalCase::default()
    })
}

fn objective_name(weights: ObjectiveWeights) -> String {
    pleochroma_cut_core::ObjectivePreset::ALL
        .into_iter()
        .find(|preset| preset.weights() == weights)
        .map_or_else(|| "weighted".to_owned(), |preset| format!("{preset:?}"))
}

const fn objective_fidelity(fidelity: EvalFidelity) -> ObjectiveFidelity {
    match fidelity {
        EvalFidelity::Fast => ObjectiveFidelity::Fast,
        EvalFidelity::Full => ObjectiveFidelity::Full,
    }
}

#[cfg(test)]
#[path = "runner_eval_tests.rs"]
mod tests;
