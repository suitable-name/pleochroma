//! Exact episode snapshots for recovery at a completed training-iteration boundary.
//!
//! The caller rebuilds the immutable episode from the original design, settings and seed,
//! then restores its accepted geometry and measured values. Reconstruction reuses the saved
//! baseline evaluation; optical measurements and actions are not replayed. The identity guard
//! deliberately refuses changed material/catalogue code.
//!
//! Two identities guard a snapshot. The legacy one is the `Debug` text of the inputs, so any
//! change to those types' layout refuses it. The field-level one (`EpisodeIdentity`) names the
//! episode settings explicitly and digests the design, material and reward evaluator; a
//! snapshot restores when either matches, and a refusal names the fields that differ.

use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
};

use pleochroma::{
    color::metrics::FaceUpTone, geometry::meet_solver::MeetConstraint,
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{Design, ObjectiveComponents, optimize::ObjectiveFidelity};
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
    /// Field-level identity of the inputs; absent in snapshots written before it existed.
    #[serde(default)]
    semantic_identity: Option<EpisodeIdentity>,
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

/// Revision of the field list of [`EpisodeIdentity`]: bump it when a listed field changes
/// meaning. A newly listed field instead goes into [`ADDED_EPISODE_FIELDS`].
const EPISODE_IDENTITY_REVISION: u32 = 1;

/// Episode fields listed after revision 1, each with the value that reproduces the behaviour of
/// snapshots saved before it was listed.
const ADDED_EPISODE_FIELDS: &[(&str, &str)] = &[];

/// The inputs a snapshot's saved measurements depend on, by stable field name.
///
/// Every episode setting is destructured under a fixed name, so a new setting does not compile
/// until it is listed (with its legacy value in [`ADDED_EPISODE_FIELDS`]) or ignored, and a
/// renamed one keeps its recorded name. The design, the material and the reward evaluator are
/// compared through digests of their complete values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct EpisodeIdentity {
    revision: u32,
    fields: BTreeMap<String, String>,
}

impl EpisodeIdentity {
    fn of(
        design: &Design,
        gem: &GemMaterial,
        settings: &CoreSettings,
        reward: &RewardEvaluator,
    ) -> Self {
        let CoreSettings {
            max_steps,
            max_step_deg,
            weights,
            fidelity,
            terminal_score,
            min_girdle_ratio,
            vary_anchored,
            task_kind,
            lighting,
            shape_target,
            probe_scale_deg,
            only_tiers,
            angle_bounds,
        } = settings;
        let shape = shape_target.map_or_else(
            || "none".to_owned(),
            |target| {
                format!(
                    "table={:?},crown={:?},weight={:?}",
                    target.table_percent, target.crown_to_pavilion, target.weight
                )
            },
        );
        let fields = [
            ("design", debug_digest(design)),
            ("material", debug_digest(gem)),
            ("reward", debug_digest(reward)),
            ("settings.max_steps", max_steps.to_string()),
            ("settings.max_step_deg", format!("{max_step_deg:?}")),
            (
                "settings.weights.windowing",
                format!("{:?}", weights.windowing),
            ),
            (
                "settings.weights.extinction",
                format!("{:?}", weights.extinction),
            ),
            (
                "settings.weights.tilt_brilliance",
                format!("{:?}", weights.tilt_brilliance),
            ),
            (
                "settings.weights.yield",
                format!("{:?}", weights.yield_weight),
            ),
            (
                "settings.weights.tone",
                format!("{:?}", weights.tone_weight),
            ),
            (
                "settings.weights.tone_goal",
                format!("{:?}", weights.tone_goal),
            ),
            (
                "settings.fidelity",
                match fidelity {
                    ObjectiveFidelity::Fast => "fast",
                    ObjectiveFidelity::Full => "full",
                }
                .to_owned(),
            ),
            ("settings.terminal_score", format!("{terminal_score:?}")),
            ("settings.min_girdle_ratio", format!("{min_girdle_ratio:?}")),
            ("settings.vary_anchored", vary_anchored.to_string()),
            ("settings.task_kind", format!("{task_kind:?}")),
            ("settings.lighting", format!("{lighting:?}")),
            ("settings.shape_target", shape),
            ("settings.probe_scale_deg", format!("{probe_scale_deg:?}")),
            ("settings.only_tiers", format!("{only_tiers:?}")),
            ("settings.angle_bounds", format!("{angle_bounds:?}")),
        ];
        Self {
            revision: EPISODE_IDENTITY_REVISION,
            fields: fields
                .into_iter()
                .map(|(field, value)| (field.to_owned(), value))
                .collect(),
        }
    }

    /// What differs between this saved identity and `current`, one `field: saved -> current`
    /// line per field; empty when they match.
    fn changes(&self, current: &Self) -> Vec<String> {
        if self.revision != current.revision {
            return vec![format!(
                "identity revision: {} -> {}",
                self.revision, current.revision
            )];
        }
        let mut changes = Vec::new();
        for (field, value) in &current.fields {
            match self.fields.get(field) {
                Some(saved) if saved == value => {}
                Some(saved) => changes.push(format!("{field}: {saved} -> {value}")),
                None if ADDED_EPISODE_FIELDS
                    .iter()
                    .any(|&(name, default)| name == field && default == value) => {}
                None => changes.push(format!("{field}: (not recorded) -> {value}")),
            }
        }
        for (field, saved) in &self.fields {
            if !current.fields.contains_key(field) {
                changes.push(format!("{field}: {saved} -> (no longer recorded)"));
            }
        }
        changes
    }
}

/// 64-bit FNV-1a over everything written to it, without keeping the text.
struct DigestWriter {
    hash: u64,
    bytes: usize,
}

impl fmt::Write for DigestWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.hash ^= u64::from(byte);
            self.hash = self.hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.bytes += text.len();
        Ok(())
    }
}

/// A compact, deterministic digest of `value`'s `Debug` text: equal values, equal digests.
fn debug_digest(value: &impl fmt::Debug) -> String {
    let mut digest = DigestWriter {
        hash: 0xcbf2_9ce4_8422_2325,
        bytes: 0,
    };
    // The writer never fails; only a faulty `Debug` impl could, and its partial digest still
    // differs from a complete one.
    let _ = write!(digest, "{value:?}");
    format!("fnv1a64:{:016x}:{}", digest.hash, digest.bytes)
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
            semantic_identity: Some(EpisodeIdentity::of(
                &core.base_design,
                &core.gem,
                &core.settings,
                &core.reward,
            )),
        })
    }

    /// Checks the original inputs before saved values can replace optical scoring: either the
    /// legacy identity or the field-level one must match.
    pub(super) fn validated_baseline(
        &self,
        design: &Design,
        gem: &GemMaterial,
        settings: &CoreSettings,
        reward: &RewardEvaluator,
    ) -> Result<(Option<f64>, DesignEval), EnvError> {
        if self.identity != baseline_identity(design, gem, settings, reward) {
            let Some(saved) = &self.semantic_identity else {
                return Err(EnvError::InvalidConfig(
                    "recovery episode baseline, settings or material changed",
                ));
            };
            let changes = saved.changes(&EpisodeIdentity::of(design, gem, settings, reward));
            if !changes.is_empty() {
                return Err(EnvError::UnsupportedDesign(format!(
                    "recovery episode baseline, settings or material changed ({})",
                    changes.join("; ")
                )));
            }
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

    #[test]
    fn a_changed_debug_layout_restores_through_the_field_identity() {
        let original = fixture_env();
        let rebuild = |saved: &EpisodeSnapshot, config| {
            RetargetEnv::from_episode(original.base_design().clone(), config, saved)
        };
        // An older build's legacy text no longer matches, the field-level identity still does.
        let mut saved = original.snapshot_episode().unwrap();
        saved.identity.push_str("\nfield added by a later build");
        let Ok(restored) = rebuild(&saved, original.config().clone()) else {
            panic!("the field-level identity matches");
        };
        assert_eq!(restored.build_observation(), original.build_observation());
        // Snapshots without it keep the exact legacy comparison.
        saved.semantic_identity = None;
        assert!(rebuild(&saved, original.config().clone()).is_err());

        // A real change is refused under both identities, and the field is named.
        let fresh = original.snapshot_episode().unwrap();
        let mut config = original.config().clone();
        config.max_step_deg *= 0.5;
        let Err(error) = rebuild(&fresh, config) else {
            panic!("a changed step limit must be refused");
        };
        assert!(
            error.to_string().contains("settings.max_step_deg"),
            "{error}"
        );
    }
}
