//! Schema-versioned weight exports for inference and warm starts of new runs.
//!
//! A [`Checkpoint`] wraps the trained [`GemstoneSetAttentionNetwork`] with the metadata needed
//! to detect mismatches at load time: the training mode and model tier, the observation layout
//! it was trained against, and the training progress. This weight-only format omits Adam,
//! replay and episode state. Full training recovery uses
//! [`crate::training::BurnTrainer::save_training_state`] together with the trainer app's
//! runner-state bundle; its model and optimiser records have distinct base names.
//!
//! # Schema
//!
//! Schema 6 has the same network and omits the training probe displacement. It loads with
//! the default probe rule (use the rollout step limit). Earlier schemas are refused with
//! [`WeightError::UnsupportedSchema`] before the network is parsed, because their heads and
//! input embeddings differ from the current network. The network holds per-tier sensitivity and
//! residual action heads plus a gain head. The policy mean is composed from the sensitivities and
//! objective weights.

use super::{
    actor_critic::{GemstoneSetAttentionNetwork, ModelTier},
    weights::WeightError,
};
use crate::env::{GLOBAL_DIM, N_COMPONENTS, TIER_DIM, WEIGHT_FRACTION_INDEX};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

/// Schema version written by [`Checkpoint::save`] and required by [`Checkpoint::load`].
pub const CHECKPOINT_SCHEMA_VERSION: u32 = 7;

/// Read before the rest of the file, so a file of another schema is refused by its version
/// rather than by a missing network field.
#[derive(Deserialize)]
struct SchemaProbe {
    schema_version: u32,
}

/// Which task a model was trained for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrainingMode {
    /// Angle optimisation within one material.
    Optimizer,
    /// Retargeting a design from one material to another.
    Retarget,
}

impl fmt::Display for TrainingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Optimizer => "optimizer",
            Self::Retarget => "retarget",
        })
    }
}

/// A trained network plus the metadata needed to load it safely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Always [`CHECKPOINT_SCHEMA_VERSION`] for files written by this build.
    pub schema_version: u32,
    /// Task the network was trained for.
    pub mode: TrainingMode,
    /// Size tier the network was built from.
    pub tier: ModelTier,
    /// Observation layout version (`OBS_LAYOUT_VERSION`) the network was trained against.
    pub obs_layout_version: u32,
    /// Number of tier slots the batched training pass was padded to.
    pub max_tiers: usize,
    /// Completed trainer iterations when the checkpoint was written (rollout/training rounds for
    /// either PPO or imitation; this is not an Adam-step count).
    pub iteration: u64,
    /// Seed of the training run.
    pub seed: u64,
    /// Probe displacement observed during training, in degrees. Without recorded metadata,
    /// inference uses the rollout step limit unless the caller supplies a probe displacement.
    #[serde(default)]
    pub probe_scale_deg: Option<f64>,
    /// The trained network.
    pub network: GemstoneSetAttentionNetwork,
}

impl Checkpoint {
    /// Assemble a checkpoint at the current schema version.
    #[must_use]
    pub const fn new(
        mode: TrainingMode,
        tier: ModelTier,
        obs_layout_version: u32,
        max_tiers: usize,
        iteration: u64,
        seed: u64,
        network: GemstoneSetAttentionNetwork,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            mode,
            tier,
            obs_layout_version,
            max_tiers,
            iteration,
            seed,
            probe_scale_deg: None,
            network,
        }
    }

    /// Record the effective displacement represented in training observations.
    #[must_use]
    pub const fn with_probe_scale_deg(mut self, degrees: f64) -> Self {
        self.probe_scale_deg = Some(degrees);
        self
    }

    pub(crate) fn validate_probe_scale(&self) -> Result<(), WeightError> {
        if self.probe_scale_deg.is_some_and(|degrees| !crate::env::is_observable_displacement(degrees)) {
            return Err(WeightError::Invalid(
                "invalid checkpoint probe displacement".into(),
            ));
        }
        Ok(())
    }

    /// Write the checkpoint as compact JSON to `<path>.tmp` and then rename it over `path`, so an
    /// interrupted save never leaves a truncated checkpoint behind.
    ///
    /// # Errors
    /// [`WeightError::Io`] / [`WeightError::Json`]; the temporary file is removed on failure.
    pub fn save(&self, path: &Path) -> Result<(), WeightError> {
        self.validate_probe_scale()?;
        let mut tmp_name: OsString = path
            .file_name()
            .ok_or_else(|| WeightError::Invalid("checkpoint path has no file name".to_owned()))?
            .to_owned();
        tmp_name.push(".tmp");
        let tmp_path = path.with_file_name(tmp_name);

        let write = || -> Result<(), WeightError> {
            let file = File::create(&tmp_path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, self)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            // Best effort: the original error is the one worth reporting.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        Ok(())
    }

    /// Load and validate a checkpoint.
    ///
    /// Checks the schema version (before the network is parsed), the observation layout, the
    /// network ([`GemstoneSetAttentionNetwork::validate`], covering every head), a global token
    /// wide enough for the weight fractions, and agreement with the recorded tier.
    ///
    /// # Errors
    /// [`WeightError::UnsupportedSchema`], [`WeightError::ObsLayoutMismatch`], validation errors,
    /// or IO / JSON errors.
    pub fn load(path: &Path, expected_obs_layout_version: u32) -> Result<Self, WeightError> {
        let bytes = std::fs::read(path)?;
        let probe: SchemaProbe = serde_json::from_slice(&bytes)?;
        if !matches!(probe.schema_version, 6 | CHECKPOINT_SCHEMA_VERSION) {
            return Err(WeightError::UnsupportedSchema {
                expected: CHECKPOINT_SCHEMA_VERSION,
                got: probe.schema_version,
            });
        }
        let mut checkpoint: Self = serde_json::from_slice(&bytes)?;
        checkpoint.schema_version = CHECKPOINT_SCHEMA_VERSION;
        checkpoint.validate_probe_scale()?;
        if checkpoint.obs_layout_version != expected_obs_layout_version {
            return Err(WeightError::ObsLayoutMismatch {
                expected: expected_obs_layout_version,
                got: checkpoint.obs_layout_version,
            });
        }
        checkpoint.network.validate()?;
        if checkpoint.network.d_tier != TIER_DIM || checkpoint.network.d_global != GLOBAL_DIM {
            return Err(WeightError::Invalid(format!(
                "network observation width {}x{} does not match layout {}x{}",
                checkpoint.network.d_tier, checkpoint.network.d_global, TIER_DIM, GLOBAL_DIM
            )));
        }

        let (d_model, num_heads, num_layers) = checkpoint.tier.config();
        let net = &checkpoint.network;
        // The composed mean reads the weight fractions from the global token: a network too
        // narrow to carry them can never run, whatever its layout tag says.
        let needed_global = WEIGHT_FRACTION_INDEX + N_COMPONENTS;
        if net.d_global < needed_global {
            return Err(WeightError::Invalid(format!(
                "network global width {} cannot carry the {N_COMPONENTS} weight fractions at \
                 entry {WEIGHT_FRACTION_INDEX} (needs at least {needed_global})",
                net.d_global
            )));
        }
        if net.d_model != d_model
            || net.num_heads != num_heads
            || net.attn_layers.len() != num_layers
        {
            return Err(WeightError::Invalid(format!(
                "network ({}x{} heads, {} layers) does not match tier {}",
                net.d_model,
                net.num_heads,
                net.attn_layers.len(),
                checkpoint.tier.code_str()
            )));
        }
        Ok(checkpoint)
    }

    /// Reject the checkpoint unless it was trained for `expected`.
    ///
    /// # Errors
    /// [`WeightError::ModeMismatch`].
    pub fn ensure_mode(&self, expected: TrainingMode) -> Result<(), WeightError> {
        if self.mode == expected {
            Ok(())
        } else {
            Err(WeightError::ModeMismatch {
                expected,
                got: self.mode,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{OBS_LAYOUT_VERSION, TierFeature};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pleochroma-ai-{}-{name}", std::process::id()))
    }

    fn small_checkpoint() -> Checkpoint {
        let tier = ModelTier::Small;
        Checkpoint::new(
            TrainingMode::Optimizer,
            tier,
            OBS_LAYOUT_VERSION,
            24,
            17,
            99,
            tier.init_network(5),
        )
    }

    #[test]
    fn round_trip_preserves_everything() {
        let path = temp_path("roundtrip.json");
        let original = small_checkpoint();
        original.save(&path).unwrap();
        let loaded = Checkpoint::load(&path, OBS_LAYOUT_VERSION).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn a_schema_six_file_without_probe_metadata_loads_with_the_default_probe_rule() {
        let path = temp_path("legacy-probe.json");
        let mut value = serde_json::to_value(small_checkpoint()).unwrap();
        value["schema_version"] = serde_json::Value::from(6);
        value.as_object_mut().unwrap().remove("probe_scale_deg");
        std::fs::write(&path, value.to_string()).unwrap();
        let migrated = Checkpoint::load(&path, OBS_LAYOUT_VERSION).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(migrated.schema_version, CHECKPOINT_SCHEMA_VERSION);
        assert_eq!(migrated.probe_scale_deg, None);
        assert_eq!(migrated.network, small_checkpoint().network);
        let config = crate::inference::InferenceConfig {
            probe_scale_deg: Some(0.0625),
            ..crate::inference::InferenceConfig::optimizer()
        };
        let legacy = crate::inference::RlOptimizerModel::from_checkpoint(migrated, config).unwrap();
        assert_eq!(legacy.config().probe_scale_deg, Some(0.0625));
    }

    #[test]
    fn checkpoint_rejects_overflow_and_underflow_in_the_probe_token() {
        let mut checkpoint = small_checkpoint();
        for invalid in [f64::MAX, f64::MIN_POSITIVE, f64::from(f32::from_bits(45))] {
            checkpoint.probe_scale_deg = Some(invalid);
            assert!(checkpoint.validate_probe_scale().is_err());
        }
        checkpoint.probe_scale_deg = Some(f64::from(f32::from_bits(46)));
        assert!(checkpoint.validate_probe_scale().is_ok());
    }

    #[test]
    fn saving_over_an_existing_checkpoint_replaces_the_previous_file() {
        let path = temp_path("replace.json");
        let first = small_checkpoint();
        first.save(&path).unwrap();
        let mut second = small_checkpoint();
        second.iteration += 1;
        second.seed += 1;
        second.save(&path).unwrap();
        let loaded = Checkpoint::load(&path, OBS_LAYOUT_VERSION).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded.iteration, second.iteration);
        assert_eq!(loaded.seed, second.seed);
    }

    #[test]
    fn layout_and_mode_mismatches_are_rejected() {
        let path = temp_path("mismatch.json");
        small_checkpoint().save(&path).unwrap();
        let wrong_layout = OBS_LAYOUT_VERSION + 1;
        let layout = Checkpoint::load(&path, wrong_layout);
        let loaded = Checkpoint::load(&path, OBS_LAYOUT_VERSION).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            layout,
            Err(WeightError::ObsLayoutMismatch { expected, got })
                if expected == wrong_layout && got == OBS_LAYOUT_VERSION
        ));
        assert!(matches!(
            loaded.ensure_mode(TrainingMode::Retarget),
            Err(WeightError::ModeMismatch { .. })
        ));
        assert!(loaded.ensure_mode(TrainingMode::Optimizer).is_ok());
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let path = temp_path("schema.json");
        let mut checkpoint = small_checkpoint();
        checkpoint.schema_version = CHECKPOINT_SCHEMA_VERSION + 1;
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(result, Err(WeightError::UnsupportedSchema { .. })));
    }

    #[test]
    fn schema_seven_stores_probe_context_alongside_the_richer_inputs() {
        assert_eq!(CHECKPOINT_SCHEMA_VERSION, 7);
        assert_eq!(small_checkpoint().schema_version, 7);
        let path = temp_path("schema7.json");
        small_checkpoint()
            .with_probe_scale_deg(0.125)
            .save(&path)
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 7);
        assert_eq!(value["probe_scale_deg"], 0.125);
        let network = value["network"].as_object().unwrap();
        assert!(network.contains_key("sens_head") && network.contains_key("gain_head"));
        assert!(network.contains_key("action_head"));
        assert!(!network.contains_key("actor_mean_head"));
    }

    #[test]
    fn a_version_three_file_is_refused_by_its_schema_version() {
        let path = temp_path("v3.json");
        // Relabel a fresh file as version 3 so the schema check, not the field parse, refuses it.
        let mut value = serde_json::to_value(small_checkpoint()).unwrap();
        value["schema_version"] = serde_json::Value::from(3u32);
        std::fs::write(&path, value.to_string()).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        assert!(
            matches!(
                result,
                Err(WeightError::UnsupportedSchema {
                    expected: CHECKPOINT_SCHEMA_VERSION,
                    got: 3
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_version_four_file_is_refused_before_parsing_the_missing_action_head() {
        let path = temp_path("v4.json");
        let mut value = serde_json::to_value(small_checkpoint()).unwrap();
        value["schema_version"] = serde_json::Value::from(4u32);
        value["network"]
            .as_object_mut()
            .unwrap()
            .remove("action_head");
        std::fs::write(&path, value.to_string()).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            result,
            Err(WeightError::UnsupportedSchema {
                expected: CHECKPOINT_SCHEMA_VERSION,
                got: 4
            })
        ));
    }

    #[test]
    fn version_five_cannot_reinterpret_the_enlarged_input_embeddings() {
        let path = temp_path("v5.json");
        let mut value = serde_json::to_value(small_checkpoint()).unwrap();
        value["schema_version"] = serde_json::Value::from(5u32);
        value["obs_layout_version"] = serde_json::Value::from(6u32);
        std::fs::write(&path, value.to_string()).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            result,
            Err(WeightError::UnsupportedSchema { got: 5, .. })
        ));
    }

    #[test]
    fn a_network_that_cannot_carry_the_weight_fractions_is_refused() {
        let path = temp_path("narrow.json");
        let (d_model, num_heads, num_layers) = ModelTier::Small.config();
        // Shaped like the Small tier, but a global token that stops before the fractions.
        let narrow = GemstoneSetAttentionNetwork::new(
            TierFeature::FEATURE_DIM,
            WEIGHT_FRACTION_INDEX,
            d_model,
            num_heads,
            num_layers,
            4,
        )
        .unwrap();
        Checkpoint::new(
            TrainingMode::Retarget,
            ModelTier::Small,
            OBS_LAYOUT_VERSION,
            24,
            0,
            1,
            narrow,
        )
        .save(&path)
        .unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        let err = result.unwrap_err();
        assert!(matches!(err, WeightError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("weight fractions"), "{err}");
    }

    /// Save a small checkpoint after `damage` and return the error `load` reports for it.
    fn load_error_after(name: &str, damage: impl FnOnce(&mut Checkpoint)) -> WeightError {
        let path = temp_path(name);
        let mut checkpoint = small_checkpoint();
        damage(&mut checkpoint);
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        result.unwrap_err()
    }

    #[test]
    fn a_malformed_sensitivity_or_gain_head_is_refused_on_load() {
        let err = load_error_after("bad-sens.json", |c| {
            c.network.sens_head.biases.pop();
        });
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("sens_head.biases"), "{err}");

        let err = load_error_after("bad-gain.json", |c| {
            c.network.gain_head.weights.pop();
        });
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("gain_head.weights"), "{err}");

        let err = load_error_after("bad-action.json", |c| {
            c.network.action_head.weights.pop();
        });
        assert!(err.to_string().contains("action_head.weights"), "{err}");
    }

    #[test]
    fn truncated_weights_are_rejected_on_load() {
        let path = temp_path("truncated.json");
        let mut checkpoint = small_checkpoint();
        checkpoint.network.tier_proj.weights.pop();
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, OBS_LAYOUT_VERSION);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(result, Err(WeightError::DimensionMismatch { .. })));
    }
}
