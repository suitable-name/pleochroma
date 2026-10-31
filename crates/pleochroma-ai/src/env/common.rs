//! The machinery both environments share: scoring a design, the greedy episode core and its
//! description as an observation.
//!
//! The optimizer environment scores a design in its own material; the retarget environment
//! scores it in a target material. Everything else is identical, so both wrap one
//! [`EnvCore`] and only differ in the gem they score with and in how they describe the
//! materials in the observation.
//!
//! The core is spread over five modules that share its crate-visible fields: this one (the
//! state and scoring), [`super::observe`] (the observation of the current state),
//! [`super::moves`] (the movable set and how an action becomes a candidate),
//! [`super::stepping`] (the greedy step and its line search) and [`super::teacher`] (the
//! finite-difference labels).

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{
    error::EnvError,
    reward::{RewardConfig, RewardEvaluator},
    traits::{StepInfo, StepRejection},
};
use glam::DVec3;
use pleochroma::{
    color::metrics::FaceUpTone,
    geometry::{GpuFacetPlane, stone_metrics::measure_solid},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_cut_core::{
    BuiltinMaterials, Design, DesignSolveError, MaterialLookup, MaterialSelection,
    ObjectiveComponents, ObjectiveWeights, ResolvedMaterial, ShapeTarget, critical_angle_deg,
    optimize::ObjectiveFidelity,
};
use std::sync::Arc;

pub(crate) use super::evaluation::evaluate_design;
pub(crate) use super::moves::{MoveSpace, apply_action};

/// Default score below which an episode counts as solved: `0.0`, so ordinary positive-score
/// episodes end at the step limit. (A threshold of 5 ended the episodes of designs
/// whose base score was already below it after their first step, producing one-step
/// episodes that taught nothing.)
pub(crate) const DEFAULT_TERMINAL_SCORE: f32 = 0.0;

/// Default fraction of the base design's thinnest girdle band a candidate must keep (the
/// value the cut-core optimizer documents for "at least half the girdle thickness").
pub(crate) const DEFAULT_MIN_GIRDLE_RATIO: f32 = 0.5;

/// The solved facet planes in the GPU plane convention, shared by the environments and the
/// inference report.
pub(crate) fn to_gpu_planes(planes: &[(DVec3, f64)]) -> Vec<GpuFacetPlane> {
    // `planes_from_solved` yields n.x <= m; the GPU convention is n.x + d <= 0, so d = -m.
    planes
        .iter()
        .map(|&(normal, mast)| GpuFacetPlane::new(normal.as_vec3(), -mast as f32))
        .collect()
}

/// The material a design was authored for, as the editor and the optimizer see it.
///
/// The built-in preset named by the selection (diamond when there is none), then the
/// selection's refractive-index and colour overrides; with no recognised preset and no
/// override, the design's effective refractive index ([`Design::effective_refractive_index`],
/// so an `.asc` `I` line) as a flat index.
#[must_use]
pub fn design_source_material(design: &Design) -> ResolvedMaterial {
    let mut selection: MaterialSelection = design.material.clone();
    let looked_up = selection
        .name
        .as_deref()
        .and_then(|name| BuiltinMaterials.lookup(name));
    if looked_up.is_none() && selection.refractive_index_override.is_none() {
        selection.refractive_index_override = Some(design.effective_refractive_index());
    }
    let gem = selection.apply_overrides(looked_up.unwrap_or_else(GemMaterial::diamond));
    let n_d = selection
        .refractive_index_override
        .unwrap_or_else(|| f64::from(gem.dispersion.evaluate(589.3)));
    ResolvedMaterial {
        critical_angle_deg: critical_angle_deg(n_d),
        n_d,
        gem,
    }
}

/// Why a candidate design could not be scored.
#[derive(Debug)]
pub(crate) enum EvalFailure {
    Unsolvable(DesignSolveError),
    NotClosed,
    NonFiniteScore,
}

impl EvalFailure {
    pub(crate) const fn rejection(&self) -> StepRejection {
        match self {
            Self::Unsolvable(_) => StepRejection::Unsolvable,
            Self::NotClosed => StepRejection::NotClosed,
            Self::NonFiniteScore => StepRejection::NonFiniteScore,
        }
    }
}

impl From<EvalFailure> for EnvError {
    fn from(failure: EvalFailure) -> Self {
        match failure {
            EvalFailure::Unsolvable(error) => Self::Unsolvable(error),
            EvalFailure::NotClosed => Self::NotClosed,
            EvalFailure::NonFiniteScore => Self::NonFiniteScore,
        }
    }
}

/// Everything measured on one solved, closed design.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct DesignEval {
    pub(crate) score: f32,
    #[serde(with = "super::recovery::ComponentsWire")]
    pub(crate) components: ObjectiveComponents,
    pub(crate) tone_lighter_loss_pct: Option<f32>,
    pub(crate) tone_deeper_loss_pct: Option<f32>,
    #[serde(with = "super::recovery::ToneWire")]
    pub(crate) tone: FaceUpTone,
    pub(crate) shape_distance_pct: f32,
    /// Solved mast per tier.
    pub(crate) masts: Vec<f64>,
    pub(crate) table_percent: Option<f64>,
    pub(crate) crown_to_pavilion: Option<f64>,
    pub(crate) yield_pct: Option<f32>,
    /// Thinnest girdle gap, design units; `None` without a live girdle wall.
    pub(crate) girdle_thickness: Option<f64>,
    pub(crate) vanishing_facets: usize,
    pub(crate) undersized_facets: usize,
    pub(crate) width_axis: Option<f64>,
    pub(crate) live_tier_geometry: Vec<LiveTierGeometry>,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct LiveTierGeometry {
    pub(crate) area_fraction: f32,
    pub(crate) centroid: [f64; 3],
    pub(crate) available: bool,
}

/// The volume of `design`'s preform, `None` when it does not measure.
///
/// A pure function of `design.preform`, which neither environment ever writes (they move
/// only `angle_deg` and `constraint`), so [`EnvCore`] measures it once and hands it to
/// every [`evaluate_design`] call instead of re-measuring per step.
pub(crate) fn preform_volume(design: &Design) -> Option<f64> {
    measure_solid(&design.preform.planes()).map(|metrics| metrics.volume)
}

/// The figure the environments minimise (`0..=100`, lower is better).
///
/// The optical score ([`ObjectiveWeights::score`]), unless the objective weighs the yield
/// (`yield_weight > 0`) and the yield is measurable (`yield_pct` is `Some`): then the yield
/// loss [`yield_loss_pct`] joins it ([`ObjectiveWeights::score_with_yield`]). A stone with no
/// measurable yield is scored on its optics alone, and a zero yield weight reproduces the
/// optical score bit for bit.
#[cfg(test)]
#[must_use]
pub(crate) fn objective_score(
    weights: &ObjectiveWeights,
    components: &ObjectiveComponents,
    yield_pct: Option<f32>,
) -> f32 {
    match yield_pct {
        Some(yield_pct) if weights.yield_weight > 0.0 => {
            weights.score_with_yield(components, yield_loss_pct(yield_pct))
        }
        _ => weights.score(components),
    }
}

/// The yield loss the score and the teacher's yield component use: exactly `100 - yield_pct`,
/// matching the shared cut-core objective, including when measured yield exceeds 100 %.
#[must_use]
pub(crate) fn yield_loss_pct(yield_pct: f32) -> f32 {
    100.0 - yield_pct
}

/// Whether a candidate keeps the girdle: not lost, not a knife edge, and at least
/// `min_ratio` of the base design's thinnest gap. A base design without a girdle band has
/// nothing to keep.
fn girdle_ok(base: Option<f64>, candidate: Option<f64>, min_ratio: f32) -> bool {
    match (base, candidate) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(base), Some(candidate)) => {
            candidate >= f64::from(min_ratio) * base && (candidate > 0.0 || base <= 0.0)
        }
    }
}

/// Thinnest girdle gap of `current` relative to `base` (`1.0` when there is nothing to
/// compare).
pub(crate) const fn girdle_ratio(base: Option<f64>, current: Option<f64>) -> f32 {
    match (base, current) {
        (Some(base), Some(current)) if base > 0.0 => (current / base) as f32,
        _ => 1.0,
    }
}

/// Silhouette drift of `candidate` from `base` in percent: `100 *`
/// [`ShapeTarget::distance`] over table size and crown-to-pavilion ratio.
pub(crate) fn shape_drift_pct(base: &DesignEval, candidate: &DesignEval) -> f32 {
    let target = ShapeTarget {
        table_percent: base.table_percent,
        crown_to_pavilion: base.crown_to_pavilion,
        weight: 1.0,
    };
    (100.0 * target.distance(candidate.table_percent, candidate.crown_to_pavilion)) as f32
}

/// Episode settings the core needs (the part of the two config types they share).
#[derive(Debug, Clone)]
pub(crate) struct CoreSettings {
    pub(crate) max_steps: usize,
    pub(crate) max_step_deg: f64,
    pub(crate) weights: ObjectiveWeights,
    pub(crate) fidelity: ObjectiveFidelity,
    pub(crate) terminal_score: f32,
    pub(crate) min_girdle_ratio: f32,
    pub(crate) vary_anchored: bool,
    pub(crate) task_kind: f32,
    pub(crate) lighting: LightingPreset,
    pub(crate) shape_target: Option<ShapeTarget>,
    pub(crate) probe_scale_deg: f64,
    pub(crate) only_tiers: Option<std::collections::BTreeSet<usize>>,
    pub(crate) angle_bounds: std::collections::BTreeMap<usize, (f64, f64)>,
}

impl CoreSettings {
    fn validate(&self) -> Result<(), EnvError> {
        if self.max_steps == 0 {
            return Err(EnvError::InvalidConfig("max_steps must be at least 1"));
        }
        if !(self.max_step_deg.is_finite() && self.max_step_deg > 0.0) {
            return Err(EnvError::InvalidConfig(
                "max_step_deg must be finite and positive",
            ));
        }
        if !self.terminal_score.is_finite() {
            return Err(EnvError::InvalidConfig("terminal_score must be finite"));
        }
        if !(self.min_girdle_ratio.is_finite() && (0.0..=1.0).contains(&self.min_girdle_ratio)) {
            return Err(EnvError::InvalidConfig(
                "min_girdle_ratio must lie in 0..=1",
            ));
        }
        if !self.probe_scale_deg.is_finite() || self.probe_scale_deg <= 0.0 {
            return Err(EnvError::InvalidConfig(
                "probe_scale_deg must be finite and positive",
            ));
        }
        if [
            self.weights.windowing,
            self.weights.extinction,
            self.weights.tilt_brilliance,
            self.weights.yield_weight,
            self.weights.tone_weight,
        ]
        .iter()
        .any(|w| !w.is_finite() || *w < 0.0)
        {
            return Err(EnvError::InvalidConfig(
                "objective weights must be finite and non-negative",
            ));
        }
        if self.shape_target.is_some_and(|target| {
            !target.weight.is_finite()
                || target.weight < 0.0
                || target.table_percent.is_some_and(|value| !value.is_finite())
                || target
                    .crown_to_pavilion
                    .is_some_and(|value| !value.is_finite())
        }) {
            return Err(EnvError::InvalidConfig(
                "shape target values must be finite and its weight non-negative",
            ));
        }
        if self
            .angle_bounds
            .values()
            .any(|(lo, hi)| !lo.is_finite() || !hi.is_finite() || lo > hi)
        {
            return Err(EnvError::InvalidConfig(
                "angle bounds must be finite inclusive ranges",
            ));
        }
        Ok(())
    }
}

/// What one core step produced (the environment adds the observation).
pub(crate) struct CoreStep {
    pub(crate) reward: f32,
    pub(crate) terminated: bool,
    pub(crate) truncated: bool,
    pub(crate) info: StepInfo,
}

/// The greedy episode: a base design, the current accepted design, and the scoring gem.
///
/// The fields are crate-visible because the step ([`super::stepping`]) and the teacher
/// ([`super::teacher`]) are further `impl` blocks of this type.
pub(crate) struct EnvCore {
    pub(crate) execution: Option<Arc<super::runtime::EnvExecution>>,
    pub(crate) settings: CoreSettings,
    pub(crate) gem: GemMaterial,
    pub(crate) reward: RewardEvaluator,
    pub(crate) base_design: Design,
    /// [`preform_volume`] of the base design, measured once: the base design is fixed for
    /// the core's life (a new design is a new core), so it never goes stale.
    pub(crate) preform_volume: Option<f64>,
    pub(crate) current_design: Design,
    pub(crate) space: MoveSpace,
    pub(crate) base_eval: DesignEval,
    pub(crate) current_eval: DesignEval,
    pub(crate) best_score: f32,
    pub(crate) step_count: usize,
    pub(crate) last_components: Option<ObjectiveComponents>,
}

impl EnvCore {
    /// Builds the core for `design` scored in `gem`.
    ///
    /// # Errors
    ///
    /// [`EnvError::InvalidConfig`] for an out-of-range setting; [`EnvError::Unsolvable`],
    /// [`EnvError::NotClosed`] or [`EnvError::NonFiniteScore`] when the base design cannot be
    /// scored; [`EnvError::NoMovableTiers`]
    /// when no tier can move.
    pub(crate) fn new(
        design: Design,
        gem: GemMaterial,
        settings: CoreSettings,
        reward_config: RewardConfig,
    ) -> Result<Self, EnvError> {
        Self::build(design, gem, settings, reward_config, None, None)
    }

    /// Rebuilds immutable geometry while reusing validated saved optical measurements.
    pub(crate) fn from_episode(
        design: Design,
        gem: GemMaterial,
        settings: CoreSettings,
        reward_config: RewardConfig,
        snapshot: &super::recovery::EpisodeSnapshot,
    ) -> Result<Self, EnvError> {
        Self::build(design, gem, settings, reward_config, Some(snapshot), None)
    }

    pub(super) fn build(
        design: Design,
        gem: GemMaterial,
        settings: CoreSettings,
        reward_config: RewardConfig,
        snapshot: Option<&super::recovery::EpisodeSnapshot>,
        execution: Option<Arc<super::runtime::EnvExecution>>,
    ) -> Result<Self, EnvError> {
        settings.validate()?;
        super::design_features::validate_design_representation(&design)
            .map_err(EnvError::UnsupportedDesign)?;
        reward_config.validate().map_err(EnvError::InvalidConfig)?;
        if settings
            .only_tiers
            .as_ref()
            .is_some_and(|tiers| tiers.iter().any(|&i| i >= design.tiers.len()))
            || settings.angle_bounds.iter().any(|(&i, &(lo, hi))| {
                design
                    .tiers
                    .get(i)
                    .is_none_or(|tier| tier.angle_deg < lo || tier.angle_deg > hi)
            })
        {
            return Err(EnvError::InvalidConfig(
                "tier scope or angle bounds do not contain the design",
            ));
        }
        let reward = RewardEvaluator::new(reward_config);
        let (preform_volume, base_eval) = if let Some(snapshot) = snapshot {
            snapshot.validated_baseline(&design, &gem, &settings, &reward)?
        } else {
            let volume = preform_volume(&design);
            let evaluated = super::evaluation::evaluate_configured(
                &design,
                &gem,
                &settings,
                volume,
                execution.as_ref(),
            )?;
            (volume, evaluated)
        };
        let policy = if settings.task_kind > 0.5 {
            pleochroma_cut_core::meet_preservation::PreservationPolicy::RETARGET
        } else {
            pleochroma_cut_core::meet_preservation::PreservationPolicy::OPTIMIZE
        };
        let space = MoveSpace::measure_with(
            &design,
            settings.vary_anchored,
            settings.only_tiers.as_ref(),
            policy,
            &settings.angle_bounds,
        )?;
        let mut core = Self {
            execution: None,
            best_score: base_eval.score,
            current_eval: base_eval.clone(),
            base_eval,
            settings,
            gem,
            reward,
            base_design: design.clone(),
            preform_volume,
            current_design: design,
            space,
            step_count: 0,
            last_components: None,
        };
        if let Some(execution) = execution {
            core.set_execution(execution);
        }
        if let Some(snapshot) = snapshot {
            snapshot.restore(&mut core)?;
        }
        Ok(core)
    }

    pub(crate) fn reset(&mut self) {
        self.current_design = self.base_design.clone();
        self.current_eval = self.base_eval.clone();
        self.best_score = self.base_eval.score;
        self.step_count = 0;
        self.last_components = None;
    }

    pub(crate) fn free_tiers(&self) -> &[usize] {
        self.space.free()
    }

    pub(crate) const fn base_design(&self) -> &Design {
        &self.base_design
    }

    pub(crate) const fn current_design(&self) -> &Design {
        &self.current_design
    }

    /// The objective weights the core scores with.
    pub(crate) const fn weights(&self) -> &ObjectiveWeights {
        &self.settings.weights
    }

    pub(crate) const fn base_score(&self) -> f32 {
        self.base_eval.score
    }

    pub(crate) const fn current_score(&self) -> f32 {
        self.current_eval.score
    }

    pub(crate) fn preservation_facts(
        &self,
    ) -> Result<pleochroma_cut_core::meet_preservation::PreservationFacts, String> {
        self.space.certify(&self.current_design)
    }

    pub(crate) fn certify_design(
        &self,
        design: &Design,
    ) -> Result<pleochroma_cut_core::meet_preservation::PreservationFacts, String> {
        self.space.certify(design)
    }

    pub(crate) fn preservation_baseline(
        &self,
    ) -> Arc<pleochroma_cut_core::meet_preservation::MeetPreservationBaseline> {
        self.space.baseline()
    }

    pub(crate) const fn best_score(&self) -> f32 {
        self.best_score
    }

    pub(crate) const fn base_components(&self) -> ObjectiveComponents {
        self.base_eval.components
    }

    pub(crate) const fn current_components(&self) -> ObjectiveComponents {
        self.current_eval.components
    }

    pub(crate) const fn last_components(&self) -> Option<ObjectiveComponents> {
        self.last_components
    }

    pub(crate) const fn step_count(&self) -> usize {
        self.step_count
    }

    /// Whether `eval` keeps the girdle of the base design (see [`girdle_ok`]); the gate the
    /// step and the teacher probes both apply.
    pub(crate) fn girdle_gate(&self, eval: &DesignEval) -> bool {
        girdle_ok(
            self.base_eval.girdle_thickness,
            eval.girdle_thickness,
            self.settings.min_girdle_ratio,
        )
    }

    /// Whether `eval` has no more vanishing or undersized facets than the base design.
    pub(crate) const fn manufacturing_gate(&self, eval: &DesignEval) -> bool {
        eval.vanishing_facets <= self.base_eval.vanishing_facets
            && eval.undersized_facets <= self.base_eval.undersized_facets
    }
}

#[cfg(test)]
#[path = "common_tests.rs"]
mod tests;
