//! The environments of a run: one wrapper over the two environment kinds, the target material of
//! a retarget episode and the slot that holds one running episode.

use pleochroma::optics::raytracer::LightingPreset;
use pleochroma_ai::{
    env::{
        ContinuousTierAction, EnvError, GemstoneObservation, GemstoneRlEnv, OptimizerEnv,
        OptimizerEnvConfig, OracleOutcome, RetargetEnv, RetargetEnvConfig, Step, StepWork,
        TeacherLabel, TeacherLabelWork, TierProbe, TierProbeWork, recovery::EpisodeSnapshot,
    },
    model::TrainingMode,
};
use pleochroma_cut_core::{
    Design, ObjectiveWeights, ResolvedMaterial, ShapeTarget, optimize::ObjectiveFidelity,
};

use super::{MaterialLabel, RunConfig, RunnerError, draws_same_material};
use crate::dataset::{MINERAL_SPECTRUM, find_spectrum_entry, resolve_entry, sample_target};

/// One environment of either kind.
pub(super) enum EnvKind {
    // Boxed: the environments are large and differ in size.
    Optimizer(Box<OptimizerEnv>),
    Retarget(Box<RetargetEnv>),
}

impl EnvKind {
    pub(super) fn set_execution(
        &mut self,
        execution: std::sync::Arc<pleochroma_ai::env::EnvExecution>,
    ) {
        match self {
            Self::Optimizer(env) => env.set_execution(execution),
            Self::Retarget(env) => env.set_execution(execution),
        }
    }

    pub(super) fn reset(&mut self, seed: u64) -> GemstoneObservation {
        match self {
            Self::Optimizer(e) => e.reset(seed),
            Self::Retarget(e) => e.reset(seed),
        }
    }

    /// One step along a line search (see `GemstoneRlEnv::step_scaled`).
    pub(super) fn step_scaled(
        &mut self,
        action: &ContinuousTierAction,
        scales: &[f64],
    ) -> Step<GemstoneObservation> {
        match self {
            Self::Optimizer(e) => e.step_scaled(action, scales),
            Self::Retarget(e) => e.step_scaled(action, scales),
        }
    }

    /// The finite-difference label of the current design (see `RetargetEnv::teacher_label`);
    /// pure, it does not move the environment. An empty `scales` asks for no oracle step.
    pub(super) fn teacher_label(&self, h_deg: f64, scales: &[f64]) -> TeacherLabel {
        match self {
            Self::Optimizer(e) => e.teacher_label(h_deg, scales),
            Self::Retarget(e) => e.teacher_label(h_deg, scales),
        }
    }

    /// The probe of one free tier of the current design (phase 1 of a labelled pass); pure.
    pub(super) fn probe_tier(&self, tier: usize, h_deg: f64) -> TierProbe {
        match self {
            Self::Optimizer(e) => e.probe_tier(tier, h_deg),
            Self::Retarget(e) => e.probe_tier(tier, h_deg),
        }
    }

    pub(super) fn prepare_tier_probe_with(
        &self,
        tier: usize,
        h_deg: f64,
        parallel: bool,
    ) -> TierProbeWork {
        match self {
            Self::Optimizer(e) => e.prepare_tier_probe_with(tier, h_deg, parallel),
            Self::Retarget(e) => e.prepare_tier_probe_with(tier, h_deg, parallel),
        }
    }

    pub(super) fn complete_tier_probe(&self, work: TierProbeWork) -> TierProbe {
        match self {
            Self::Optimizer(e) => e.complete_tier_probe(work),
            Self::Retarget(e) => e.complete_tier_probe(work),
        }
    }

    pub(super) fn prepare_teacher_label_adaptive(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> TeacherLabelWork {
        match self {
            Self::Optimizer(e) => e.prepare_teacher_label_adaptive(h_deg, probes, scales, parallel),
            Self::Retarget(e) => e.prepare_teacher_label_adaptive(h_deg, probes, scales, parallel),
        }
    }

    pub(super) fn advance_teacher_label_adaptive(
        &self,
        work: &mut TeacherLabelWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        match self {
            Self::Optimizer(e) => e.advance_teacher_label_adaptive(work, parallel),
            Self::Retarget(e) => e.advance_teacher_label_adaptive(work, parallel),
        }
    }

    pub(super) fn complete_teacher_label(&self, work: TeacherLabelWork) -> TeacherLabel {
        match self {
            Self::Optimizer(e) => e.complete_teacher_label(work),
            Self::Retarget(e) => e.complete_teacher_label(work),
        }
    }

    pub(super) fn prepare_step_scaled_adaptive(
        &self,
        action: &ContinuousTierAction,
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> StepWork {
        match self {
            Self::Optimizer(e) => e.prepare_step_scaled_adaptive(action, scales, parallel),
            Self::Retarget(e) => e.prepare_step_scaled_adaptive(action, scales, parallel),
        }
    }

    pub(super) fn advance_step_adaptive(
        &self,
        work: &mut StepWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        match self {
            Self::Optimizer(e) => e.advance_step_adaptive(work, parallel),
            Self::Retarget(e) => e.advance_step_adaptive(work, parallel),
        }
    }

    pub(super) fn complete_step(&mut self, work: StepWork) -> Step<GemstoneObservation> {
        match self {
            Self::Optimizer(e) => e.complete_step(work),
            Self::Retarget(e) => e.complete_step(work),
        }
    }

    /// The oracle step along the score `slopes` of the current design (phase 2 of a labelled
    /// pass); pure.
    pub(super) fn oracle_outcome(&self, slopes: &[Option<f32>], scales: &[f64]) -> OracleOutcome {
        match self {
            Self::Optimizer(e) => e.oracle_outcome(slopes, scales),
            Self::Retarget(e) => e.oracle_outcome(slopes, scales),
        }
    }

    /// The tiers the environment moves.
    pub(super) fn free_tiers(&self) -> &[usize] {
        match self {
            Self::Optimizer(e) => e.free_tier_indices(),
            Self::Retarget(e) => e.movable_tier_indices(),
        }
    }

    /// The objective weights the environment scores with.
    pub(super) fn weights(&self) -> &ObjectiveWeights {
        match self {
            Self::Optimizer(e) => e.weights(),
            Self::Retarget(e) => e.weights(),
        }
    }

    pub(super) fn build_observation(&self) -> GemstoneObservation {
        match self {
            Self::Optimizer(e) => e.build_observation(),
            Self::Retarget(e) => e.build_observation(),
        }
    }

    pub(super) fn snapshot_episode(&self) -> Result<EpisodeSnapshot, EnvError> {
        match self {
            Self::Optimizer(env) => env.snapshot_episode(),
            Self::Retarget(env) => env.snapshot_episode(),
        }
    }

    pub(super) fn restore_episode(&mut self, snapshot: &EpisodeSnapshot) -> Result<(), EnvError> {
        match self {
            Self::Optimizer(env) => env.restore_episode(snapshot),
            Self::Retarget(env) => env.restore_episode(snapshot),
        }
    }

    pub(super) fn step_count(&self) -> usize {
        match self {
            Self::Optimizer(env) => env.step_count(),
            Self::Retarget(env) => env.step_count(),
        }
    }

    pub(super) fn base_score(&self) -> f32 {
        match self {
            Self::Optimizer(e) => e.base_score(),
            Self::Retarget(e) => e.base_score(),
        }
    }

    pub(super) fn current_score(&self) -> f32 {
        match self {
            Self::Optimizer(e) => e.current_score(),
            Self::Retarget(e) => e.current_score(),
        }
    }

    pub(super) fn max_step_deg(&self) -> f64 {
        match self {
            Self::Optimizer(e) => e.config().max_step_deg,
            Self::Retarget(e) => e.config().max_step_deg,
        }
    }

    pub(super) fn current_design(&self) -> &Design {
        match self {
            Self::Optimizer(e) => e.current_design(),
            Self::Retarget(e) => e.current_design(),
        }
    }

    pub(super) fn source_material(&self) -> &ResolvedMaterial {
        match self {
            Self::Optimizer(e) => e.source_material(),
            Self::Retarget(e) => e.source_material(),
        }
    }
}

/// The target material of a retarget episode.
pub(super) struct Target {
    pub(super) name: String,
    pub(super) material: ResolvedMaterial,
    pub(super) augmentation: String,
}

/// The target of the retarget episode of `design` drawn for `seed` (`None` for the optimizer
/// task).
///
/// A share of the episodes (`cfg.same_material_frac`) is judged in the source material as a
/// robustness subset; in the others the target is sampled from the spectrum or fixed material.
pub(super) fn pick_target(
    cfg: &RunConfig,
    seed: u64,
    design: &Design,
    authored: Option<&ResolvedMaterial>,
) -> Result<Option<Target>, RunnerError> {
    if cfg.mode != TrainingMode::Retarget {
        return Ok(None);
    }
    if draws_same_material(cfg.same_material_frac, seed) {
        let (source_name, material, augmentation) =
            sampled_source_material(seed, design, authored)?;
        return Ok(Some(Target {
            name: source_name,
            material,
            augmentation,
        }));
    }
    let entry = if cfg.universal_spectrum {
        sample_target(seed)
    } else {
        find_spectrum_entry(&cfg.fixed_target)?
    };
    let (material, mut augmentation) = sampled_material(resolve_entry(&entry)?, seed);
    if design.render_effects.c_axis_override_enabled {
        augmentation = "[authored stone frame overrides sampled target frame]".to_owned();
    }
    Ok(Some(Target {
        name: entry.name.to_owned(),
        material,
        augmentation,
    }))
}

/// Deterministic crystal-frame augmentation for materials with no authored orientation.
/// Scattering is not fabricated: the analytical objective cannot label its transport effects.
pub(super) fn sampled_material(
    mut material: ResolvedMaterial,
    seed: u64,
) -> (ResolvedMaterial, String) {
    if material.gem.optical_character == pleochroma::optics::materials::OpticalCharacter::Isotropic
    {
        return (material, "[catalogue preset]".to_owned());
    }
    let mut rng = pleochroma_ai::model::Prng::from_stream(seed, 0x4D41_5445_5249_414C);
    // Axes are unoriented lines. Uniform cos(tilt) on a hemisphere, uniform azimuth and
    // independent biaxial roll cover frames without over-weighting near-table orientations.
    let tilt = f64::from(rng.next_f32()).acos().to_degrees();
    let azimuth = f64::from(rng.next_f32()) * 360.0;
    let roll = f64::from(rng.next_f32()) * 180.0;
    let orientation = pleochroma_cut_core::crystal_axis::CrystalOrientation::from_angles(
        tilt,
        azimuth,
        Some(roll),
    );
    material.gem =
        pleochroma_cut_core::material::with_default_orientation(material.gem, Some(orientation));
    (
        material,
        format!("[sampled crystal frame {tilt:.1}/{azimuth:.1}/{roll:.1} deg]"),
    )
}

/// Explicitly authored materials and material-dependent context are retained unchanged.
///
/// Entries without either (ASC or native) use their recorded index in one quarter of episodes
/// and catalogue materials in the remainder. The loader decides authoring from actual fields,
/// not from the file format; these sampled properties are never written into the source file.
pub(super) fn sampled_source_material(
    seed: u64,
    design: &Design,
    authored: Option<&ResolvedMaterial>,
) -> Result<(String, ResolvedMaterial, String), crate::dataset::DatasetError> {
    if let Some(material) = authored {
        return Ok((
            design
                .material
                .name
                .clone()
                .unwrap_or_else(|| "Authored index".to_owned()),
            material.clone(),
            "[authored native material]".to_owned(),
        ));
    }
    let mut rng = pleochroma_ai::model::Prng::from_stream(seed, 0x534F_5552_4345_4D41);
    if rng.next_index(4) == 0 {
        let material = pleochroma_ai::env::design_source_material(design);
        return Ok((
            design
                .material
                .name
                .clone()
                .unwrap_or_else(|| format!("Imported n={:.4}", material.n_d)),
            material,
            "[authored imported index]".to_owned(),
        ));
    }
    let entry = &MINERAL_SPECTRUM[rng.next_index(MINERAL_SPECTRUM.len())];
    let (material, augmentation) = sampled_material(resolve_entry(entry)?, seed ^ 0x534F_5552_4345);
    Ok((entry.name.to_owned(), material, augmentation))
}
/// Builds the environment the task of `cfg` asks for around `design`, scoring with `weights`;
/// `fidelity` overrides the scoring fidelity and `cfg.max_step_deg` the step limit of either
/// environment kind.
pub(super) fn build_env(
    cfg: &RunConfig,
    design: &Design,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    evaluation: (
        Option<ObjectiveFidelity>,
        Option<&std::sync::Arc<pleochroma_ai::env::EnvExecution>>,
    ),
    context_seed: u64,
    authored: Option<&ResolvedMaterial>,
) -> Result<EnvKind, EnvError> {
    build_env_with_start(
        cfg,
        design,
        target,
        weights,
        context_seed,
        authored,
        EpisodeStart::Fresh(evaluation.0, evaluation.1),
    )
}

/// Reconstructs a saved episode's immutable state without evaluating optics again.
pub(super) fn build_env_from_episode(
    cfg: &RunConfig,
    design: &Design,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    context_seed: u64,
    authored: Option<&ResolvedMaterial>,
    snapshot: &EpisodeSnapshot,
) -> Result<EnvKind, EnvError> {
    build_env_with_start(
        cfg,
        design,
        target,
        weights,
        context_seed,
        authored,
        EpisodeStart::Recovery(snapshot, None),
    )
}

/// Reconstructs validation with its own fidelity, retaining saved baseline measurements.
/// Training's per-episode fidelity mixture must never select validation's scoring settings.
pub(super) fn build_eval_env_from_episode(
    cfg: &RunConfig,
    design: &Design,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    context_seed: u64,
    authored: Option<&ResolvedMaterial>,
    snapshot: &EpisodeSnapshot,
    evaluation: (
        ObjectiveFidelity,
        Option<&std::sync::Arc<pleochroma_ai::env::EnvExecution>>,
    ),
) -> Result<EnvKind, EnvError> {
    let mut env = build_env_with_start(
        cfg,
        design,
        target,
        weights,
        context_seed,
        authored,
        EpisodeStart::Recovery(snapshot, Some(evaluation.0)),
    )?;
    if let Some(execution) = evaluation.1 {
        env.set_execution(std::sync::Arc::clone(execution));
    }
    Ok(env)
}

#[derive(Clone, Copy)]
enum EpisodeStart<'a> {
    Fresh(
        Option<ObjectiveFidelity>,
        Option<&'a std::sync::Arc<pleochroma_ai::env::EnvExecution>>,
    ),
    Recovery(&'a EpisodeSnapshot, Option<ObjectiveFidelity>),
}

fn build_env_with_start(
    cfg: &RunConfig,
    design: &Design,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    context_seed: u64,
    authored: Option<&ResolvedMaterial>,
    start: EpisodeStart<'_>,
) -> Result<EnvKind, EnvError> {
    let fidelity = match start {
        EpisodeStart::Fresh(fidelity, _) => fidelity.unwrap_or(cfg.train_fidelity),
        EpisodeStart::Recovery(_, fidelity) => {
            fidelity.unwrap_or_else(|| super::config::training_fidelity(cfg, context_seed))
        }
    };
    let (shape_target, lighting) = sampled_context(context_seed);
    if let (TrainingMode::Retarget, Some(target)) = (cfg.mode, target) {
        let mut config = RetargetEnvConfig::new_with_target(target.material.clone());
        config.weights = weights;
        config.source_material_override = Some(
            sampled_source_material(context_seed, design, authored)
                .map_err(|_| {
                    EnvError::InvalidConfig("trainer source material could not be resolved")
                })?
                .1,
        );
        config.shape_target = shape_target;
        config.lighting = lighting;
        config.fidelity = fidelity;
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = cfg.label_h_deg.unwrap_or(config.max_step_deg);
        match start {
            EpisodeStart::Fresh(_, execution) => {
                RetargetEnv::new_with_execution(design.clone(), config, execution.cloned())
            }
            EpisodeStart::Recovery(saved, _) => {
                RetargetEnv::from_episode(design.clone(), config, saved)
            }
        }
        .map(|e| EnvKind::Retarget(Box::new(e)))
    } else {
        let mut config = OptimizerEnvConfig {
            weights,
            shape_target,
            lighting,
            source_material_override: Some(
                sampled_source_material(context_seed, design, authored)
                    .map_err(|_| {
                        EnvError::InvalidConfig("trainer source material could not be resolved")
                    })?
                    .1,
            ),
            ..OptimizerEnvConfig::default()
        };
        config.fidelity = fidelity;
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = cfg.label_h_deg.unwrap_or(config.max_step_deg);
        match start {
            EpisodeStart::Fresh(_, execution) => {
                OptimizerEnv::new_with_execution(design.clone(), config, execution.cloned())
            }
            EpisodeStart::Recovery(saved, _) => {
                OptimizerEnv::from_episode(design.clone(), config, saved)
            }
        }
        .map(|e| EnvKind::Optimizer(Box::new(e)))
    }
}

/// Episode context is deterministic and shared by rollout, validation passes and endpoint rescoring.
pub(super) fn sampled_context(seed: u64) -> (Option<ShapeTarget>, LightingPreset) {
    const LIGHTING: [LightingPreset; 7] = [
        LightingPreset::IsoHemisphere,
        LightingPreset::Daylight,
        LightingPreset::Incandescent,
        LightingPreset::RingLights,
        LightingPreset::ShopLights,
        LightingPreset::WhiteTray,
        LightingPreset::Aset,
    ];
    let mut shape_rng = pleochroma_ai::model::Prng::from_stream(seed, 0x5348_4150_455f_5447);
    let table = 35.0 + f64::from(shape_rng.next_f32()) * 40.0;
    let ratio = 0.15 + f64::from(shape_rng.next_f32()) * 0.65;
    let weight = 0.1 + shape_rng.next_f32() * 1.9;
    let shape_target = match shape_rng.next_index(4) {
        0 => None,
        1 => Some(ShapeTarget {
            table_percent: Some(table),
            crown_to_pavilion: None,
            weight,
        }),
        2 => Some(ShapeTarget {
            table_percent: None,
            crown_to_pavilion: Some(ratio),
            weight,
        }),
        _ => Some(ShapeTarget {
            table_percent: Some(table),
            crown_to_pavilion: Some(ratio),
            weight,
        }),
    };
    let mut lighting_rng = pleochroma_ai::model::Prng::from_stream(seed, 0x4C49_4748_5449_4E47);
    (
        shape_target,
        LIGHTING[lighting_rng.next_index(LIGHTING.len())],
    )
}

/// One vectorized environment slot: a running episode.
pub(super) struct Slot {
    /// Seed of this episode, retaining its exact sampled material and scoring context.
    pub(super) seed: u64,
    pub(super) env: EnvKind,
    /// Consecutive rejected training steps, reset by an accepted move or a new episode.
    pub(super) rejected_steps: usize,
    /// Exact label of the most recently scored state in this episode, if any.
    pub(super) label_cache: super::label_cache::LabelCache,
    /// Position in the train split (corpus mode cycling).
    pub(super) pos: usize,
    /// Index into the data set's `entries`.
    pub(super) entry: usize,
    pub(super) src: MaterialLabel,
    pub(super) tgt: MaterialLabel,
    pub(super) obs: GemstoneObservation,
}

#[cfg(test)]
#[path = "runner_env_tests.rs"]
mod tests;
