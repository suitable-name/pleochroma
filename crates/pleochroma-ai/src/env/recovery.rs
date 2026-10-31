//! Exact episode snapshots for recovery at a completed training-iteration boundary.
//!
//! The caller rebuilds the immutable episode from the original design, settings and seed,
//! then restores its accepted geometry and measured values. Reconstruction reuses the saved
//! baseline evaluation; optical measurements and actions are not replayed. The identity guard
//! deliberately refuses changed material/catalogue code.

use pleochroma::{
    color::metrics::FaceUpTone, geometry::meet_solver::MeetConstraint,
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{Design, ObjectiveComponents};
use serde::{Deserialize, Serialize};

use super::{
    OptimizerEnv, RetargetEnv,
    common::{CoreSettings, DesignEval, EnvCore},
    error::EnvError,
    material_features::MaterialFeatures,
    reward::RewardEvaluator,
};

/// The only authored fields the environments are allowed to change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesignState {
    tiers: Vec<(f64, ConstraintState)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ConstraintState {
    Existing,
    Named(Vec<String>),
    Scale(f64),
}

impl DesignState {
    /// Captures the mutable geometry, refusing any change outside the episode's action space.
    ///
    /// # Errors
    /// Returns an error when `current` differs from `base` outside angles and constraints.
    pub fn capture(base: &Design, current: &Design) -> Result<Self, EnvError> {
        let state = Self {
            tiers: current
                .tiers
                .iter()
                .map(|tier| {
                    let constraint = match &tier.constraint {
                        MeetConstraint::MeetExisting => ConstraintState::Existing,
                        MeetConstraint::MeetNamed(names) => ConstraintState::Named(names.clone()),
                        MeetConstraint::ScaleReference(mast) => ConstraintState::Scale(*mast),
                    };
                    (tier.angle_deg, constraint)
                })
                .collect(),
        };
        if state.apply(base)? != *current {
            return Err(EnvError::InvalidConfig(
                "episode changed fields outside its action space",
            ));
        }
        Ok(state)
    }

    /// Restores the mutable geometry onto the original authored design.
    ///
    /// # Errors
    /// Refuses a changed tier count or non-finite saved geometry.
    pub fn apply(&self, base: &Design) -> Result<Design, EnvError> {
        if self.tiers.len() != base.tiers.len() {
            return Err(EnvError::InvalidConfig(
                "recovery tier count differs from its baseline",
            ));
        }
        let mut design = base.clone();
        for (tier, (angle, constraint)) in design.tiers.iter_mut().zip(&self.tiers) {
            if !angle.is_finite() {
                return Err(EnvError::InvalidConfig("recovery angle is not finite"));
            }
            tier.angle_deg = *angle;
            tier.constraint = match constraint {
                ConstraintState::Existing => MeetConstraint::MeetExisting,
                ConstraintState::Named(names) => MeetConstraint::MeetNamed(names.clone()),
                ConstraintState::Scale(mast) if mast.is_finite() => {
                    MeetConstraint::ScaleReference(*mast)
                }
                ConstraintState::Scale(_) => {
                    return Err(EnvError::InvalidConfig("recovery mast is not finite"));
                }
            };
        }
        Ok(design)
    }
}

/// Saved accepted episode state; all floating-point measurements must round-trip exactly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpisodeSnapshot {
    identity: String,
    current: DesignState,
    base_eval: DesignEval,
    current_eval: DesignEval,
    preform_volume: Option<f64>,
    best_score: f32,
    step_count: usize,
    last_components: Option<[f32; 3]>,
    source_features: MaterialFeatures,
    target_features: MaterialFeatures,
}

fn identity(core: &EnvCore) -> String {
    baseline_identity(&core.base_design, &core.gem, &core.settings, &core.reward)
}

fn baseline_identity(
    design: &Design,
    gem: &GemMaterial,
    settings: &CoreSettings,
    reward: &RewardEvaluator,
) -> String {
    format!("{:?}\n{:?}\n{:?}\n{:?}", design, settings, gem, reward)
}

impl EpisodeSnapshot {
    fn capture(
        core: &EnvCore,
        source: &MaterialFeatures,
        target: &MaterialFeatures,
    ) -> Result<Self, EnvError> {
        Ok(Self {
            identity: identity(core),
            current: DesignState::capture(&core.base_design, &core.current_design)?,
            base_eval: core.base_eval.clone(),
            current_eval: core.current_eval.clone(),
            preform_volume: core.preform_volume,
            best_score: core.best_score,
            step_count: core.step_count,
            last_components: core
                .last_components
                .map(|c| [c.windowing_pct, c.extinction_pct, c.tilt_brilliance_pct]),
            source_features: source.clone(),
            target_features: target.clone(),
        })
    }

    /// Checks the original inputs before saved values can replace optical scoring.
    pub(super) fn validated_baseline(
        &self,
        design: &Design,
        gem: &GemMaterial,
        settings: &CoreSettings,
        reward: &RewardEvaluator,
    ) -> Result<(Option<f64>, DesignEval), EnvError> {
        if self.identity != baseline_identity(design, gem, settings, reward) {
            return Err(EnvError::InvalidConfig(
                "recovery episode baseline, settings or material changed",
            ));
        }
        if self.step_count > settings.max_steps
            || !self.best_score.is_finite()
            || self
                .preform_volume
                .is_some_and(|v| !v.is_finite() || v <= 0.0)
            || self
                .last_components
                .is_some_and(|c| c.iter().any(|v| !v.is_finite()))
            || !valid_evaluation(&self.base_eval, design, settings)
            || !valid_evaluation(&self.current_eval, design, settings)
        {
            return Err(EnvError::InvalidConfig(
                "invalid saved episode measurements",
            ));
        }
        self.source_features
            .validate()
            .map_err(EnvError::InvalidConfig)?;
        self.target_features
            .validate()
            .map_err(EnvError::InvalidConfig)?;
        self.current.apply(design)?;
        Ok((self.preform_volume, self.base_eval.clone()))
    }

    pub(super) fn material_features(&self) -> (&MaterialFeatures, &MaterialFeatures) {
        (&self.source_features, &self.target_features)
    }

    pub(super) fn restore(&self, core: &mut EnvCore) -> Result<(), EnvError> {
        self.validated_baseline(&core.base_design, &core.gem, &core.settings, &core.reward)?;
        let current = self.current.apply(&core.base_design)?;
        core.current_design = current;
        core.base_eval = self.base_eval.clone();
        core.current_eval = self.current_eval.clone();
        core.preform_volume = self.preform_volume;
        core.best_score = self.best_score;
        core.step_count = self.step_count;
        core.last_components =
            self.last_components
                .map(
                    |[windowing_pct, extinction_pct, tilt_brilliance_pct]| ObjectiveComponents {
                        windowing_pct,
                        extinction_pct,
                        tilt_brilliance_pct,
                    },
                );
        Ok(())
    }
}

fn valid_evaluation(eval: &DesignEval, design: &Design, settings: &CoreSettings) -> bool {
    let finite = [
        eval.score,
        eval.components.windowing_pct,
        eval.components.extinction_pct,
        eval.components.tilt_brilliance_pct,
        eval.tone.l_star,
        eval.tone.chroma,
        eval.tone.hue_deg,
        eval.tone.mean_path_units,
        eval.tone.returned_fraction,
    ]
    .into_iter()
    .all(f32::is_finite);
    finite
        && eval.masts.len() == design.tiers.len()
        && eval.masts.iter().all(|v| v.is_finite())
        && eval.live_tier_geometry.len() == design.tiers.len()
        && eval.live_tier_geometry.iter().all(|live| {
            live.area_fraction.is_finite() && live.centroid.iter().all(|v| v.is_finite())
        })
        && [eval.tone_lighter_loss_pct, eval.tone_deeper_loss_pct, eval.yield_pct]
            .into_iter().flatten().all(f32::is_finite)
        && [eval.table_percent, eval.crown_to_pavilion, eval.girdle_thickness, eval.width_axis]
            .into_iter().flatten().all(f64::is_finite)
        // An absent shape target is deliberately represented by NaN, not a measurement.
        && if settings.shape_target.is_some() {
            eval.shape_distance_pct.is_finite()
        } else {
            eval.shape_distance_pct.is_nan()
        }
}

impl OptimizerEnv {
    /// Captures this episode without replaying its optical evaluations.
    ///
    /// # Errors
    /// Refuses unexpected changes outside angles and constraints.
    pub fn snapshot_episode(&self) -> Result<EpisodeSnapshot, EnvError> {
        EpisodeSnapshot::capture(&self.core, &self.source_features, &self.source_features)
    }

    /// Restores a snapshot into an environment built from its original baseline and config.
    ///
    /// # Errors
    /// Refuses a different baseline, context or malformed snapshot.
    pub fn restore_episode(&mut self, snapshot: &EpisodeSnapshot) -> Result<(), EnvError> {
        snapshot.restore(&mut self.core)?;
        self.source_features = snapshot.source_features.clone();
        Ok(())
    }
}

impl RetargetEnv {
    /// Captures this episode without replaying its optical evaluations.
    ///
    /// # Errors
    /// Refuses unexpected changes outside angles and constraints.
    pub fn snapshot_episode(&self) -> Result<EpisodeSnapshot, EnvError> {
        EpisodeSnapshot::capture(&self.core, &self.source_features, &self.target_features)
    }

    /// Restores a snapshot into an environment built from its original baseline and config.
    ///
    /// # Errors
    /// Refuses a different baseline, context or malformed snapshot.
    pub fn restore_episode(&mut self, snapshot: &EpisodeSnapshot) -> Result<(), EnvError> {
        snapshot.restore(&mut self.core)?;
        self.source_features = snapshot.source_features.clone();
        self.target_features = snapshot.target_features.clone();
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "ObjectiveComponents")]
pub(crate) struct ComponentsWire {
    pub windowing_pct: f32,
    pub extinction_pct: f32,
    pub tilt_brilliance_pct: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "FaceUpTone")]
pub(crate) struct ToneWire {
    pub l_star: f32,
    pub chroma: f32,
    pub hue_deg: f32,
    pub srgb: [u8; 3],
    pub mean_path_units: f32,
    pub returned_fraction: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{ContinuousTierAction, GemstoneRlEnv, test_fixtures::fixture_env};

    #[test]
    fn reconstructed_episode_keeps_the_accepted_state_and_next_observation() {
        let mut original = fixture_env();
        let action = ContinuousTierAction::new(vec![0.05; original.base_design().tiers.len()]);
        let _ = original.step(&action);
        let saved = original.snapshot_episode().unwrap();
        let restored = RetargetEnv::from_episode(
            original.base_design().clone(),
            original.config().clone(),
            &saved,
        )
        .unwrap();
        assert_eq!(restored.build_observation(), original.build_observation());
        assert_eq!(restored.current_design(), original.current_design());
        assert_eq!(restored.step_count(), original.step_count());
        assert_eq!(
            restored.current_score().to_bits(),
            original.current_score().to_bits()
        );
    }

    #[test]
    fn saved_scores_require_the_original_inputs_and_complete_measurements() {
        let original = fixture_env();
        let saved = original.snapshot_episode().unwrap();
        let rebuild = |saved: &EpisodeSnapshot| {
            RetargetEnv::from_episode(
                original.base_design().clone(),
                original.config().clone(),
                saved,
            )
        };
        let mut changed = saved.clone();
        assert!(changed.base_eval.masts.pop().is_some());
        assert!(rebuild(&changed).is_err());
        changed = saved.clone();
        assert!(changed.current_eval.live_tier_geometry.pop().is_some());
        assert!(rebuild(&changed).is_err());
        changed = saved.clone();
        changed.base_eval.score = f32::NAN;
        assert!(rebuild(&changed).is_err());
        let mut config = original.config().clone();
        config.max_step_deg *= 0.5;
        assert!(RetargetEnv::from_episode(original.base_design().clone(), config, &saved).is_err());
    }
}
