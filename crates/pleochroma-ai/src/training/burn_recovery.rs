//! Complete device-independent trainer snapshots at completed update boundaries.
//!
//! The model's Burn record retains parameter IDs, so Adam's moment tensors still
//! belong to the same parameters on restore. The pure inference checkpoint alone
//! cannot provide that guarantee. A caller publishes the enclosing snapshot
//! directory only after every file has been written successfully.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

use burn::{
    module::{Module, ModuleVisitor, Param},
    nn::LayerNormConfig,
    optim::OptimizerRecord,
    prelude::Tensor,
    store::ModuleRecord,
};
use serde::{Deserialize, Serialize};

use super::{
    BurnTrainer, ModelShape, PpoConfig, TrainError, burn_recovery_optimizer::optimizer_for_restore,
};
use crate::model::Prng;

const VERSION: u32 = 1;
const META: &str = "trainer.json";
const MODEL: &str = "trainer-model.bpk";
const OPTIMIZER: &str = "trainer-optimizer.bpk";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrainerState {
    version: u32,
    shape: ModelShape,
    config: PpoConfig,
    updates: u64,
    shuffle: Prng,
    parameter_ids: Vec<u64>,
    optimizer_tensors: usize,
    pre_attn_eps: Vec<f32>,
    post_attn_eps: Vec<f32>,
    final_eps: f32,
}

#[derive(Default)]
struct ParameterIds(Vec<u64>);

impl ModuleVisitor for ParameterIds {
    fn visit_float<const D: usize>(&mut self, param: &Param<Tensor<D>>) {
        self.0.push(param.id.val());
    }
}

fn parameter_ids(model: &super::BurnSetAttention) -> Vec<u64> {
    let mut ids = ParameterIds::default();
    model.visit(&mut ids);
    ids.0
}

fn recovery_error(context: &str, error: impl std::fmt::Display) -> TrainError {
    TrainError::Recovery(format!("{context}: {error}"))
}

impl BurnTrainer {
    fn recovery_shape(&self) -> ModelShape {
        ModelShape {
            d_tier: self.model.d_tier,
            d_global: self.model.d_global,
            d_model: self.model.d_model,
            num_heads: self.model.num_heads,
            num_layers: self.model.num_layers,
            max_tiers: self.max_tiers,
        }
    }

    /// Write weights, Adam moments, shuffling state and schedule position.
    ///
    /// `directory` must be an unpublished snapshot directory. This method does
    /// not publish or replace a latest pointer; its caller must commit the whole
    /// run bundle only after all trainer and environment files are complete.
    ///
    /// # Errors
    ///
    /// [`TrainError::Recovery`] for an unreadable tensor or failed file write.
    pub fn save_training_state(&self, directory: &Path) -> Result<(), TrainError> {
        self.model
            .clone()
            .into_record()
            .save(directory.join(MODEL))
            .map_err(|error| recovery_error(MODEL, error))?;
        let optimizer = self.optimizer.to_record();
        let state = TrainerState {
            version: VERSION,
            shape: self.recovery_shape(),
            config: self.config.clone(),
            updates: self.updates,
            shuffle: self.shuffle.clone(),
            parameter_ids: parameter_ids(&self.model),
            optimizer_tensors: optimizer.len(),
            pre_attn_eps: self.model.pre_attn_eps.clone(),
            post_attn_eps: self.model.post_attn_eps.clone(),
            final_eps: self.model.final_eps,
        };
        optimizer
            .save(directory.join(OPTIMIZER))
            .map_err(|error| recovery_error(OPTIMIZER, error))?;
        let file =
            File::create(directory.join(META)).map_err(|error| recovery_error(META, error))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &state).map_err(|error| recovery_error(META, error))?;
        writer
            .flush()
            .map_err(|error| recovery_error(META, error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| recovery_error(META, error))
    }

    /// Restore a complete snapshot on this trainer's explicitly selected device.
    ///
    /// Model shape must match. Unlike loading inference weights, this preserves
    /// Adam moments, parameter identities, shuffle RNG and the exact schedule
    /// position. Device changes may change floating-point results, not progress.
    /// Nothing is installed until all three files have loaded and validated.
    ///
    /// # Errors
    ///
    /// [`TrainError::Recovery`] for incompatible, incomplete or corrupt state.
    pub fn restore_training_state(&mut self, directory: &Path) -> Result<(), TrainError> {
        let file = File::open(directory.join(META)).map_err(|error| recovery_error(META, error))?;
        let state: TrainerState = serde_json::from_reader(BufReader::new(file))
            .map_err(|error| recovery_error(META, error))?;
        self.validate_recovery(&state)?;
        let record = ModuleRecord::load(directory.join(MODEL))
            .map_err(|error| recovery_error(MODEL, error))?;
        // The record contains tensors, not LayerNorm's private epsilon. Construct
        // the matching norm modules before installing saved parameters and IDs.
        let mut model = self.model.clone();
        let norm = |epsilon: f32| {
            LayerNormConfig::new(state.shape.d_model)
                .with_epsilon(f64::from(epsilon))
                .init(&self.device)
        };
        model.pre_attn_norms = state
            .pre_attn_eps
            .iter()
            .map(|&epsilon| norm(epsilon))
            .collect();
        model.post_attn_norms = state
            .post_attn_eps
            .iter()
            .map(|&epsilon| norm(epsilon))
            .collect();
        model.final_norm = norm(state.final_eps);
        let mut model = model
            .try_load_record(record)
            .map_err(|error| recovery_error(MODEL, error))?;
        if parameter_ids(&model) != state.parameter_ids {
            return Err(recovery_error(
                MODEL,
                "parameter identities do not match trainer metadata",
            ));
        }
        model.pre_attn_eps = state.pre_attn_eps;
        model.post_attn_eps = state.post_attn_eps;
        model.final_eps = state.final_eps;
        // Also check finite weights using the existing pure-network validation.
        model
            .export_to_set_attention()?
            .validate()
            .map_err(|error| recovery_error(MODEL, error))?;
        let optimizer = OptimizerRecord::load(directory.join(OPTIMIZER))
            .map_err(|error| recovery_error(OPTIMIZER, error))?;
        if optimizer.len() != state.optimizer_tensors {
            return Err(recovery_error(
                OPTIMIZER,
                "moment tensor count differs from metadata",
            ));
        }
        let optimizer = catch_unwind(AssertUnwindSafe(|| {
            optimizer_for_restore(&self.device).load_record(optimizer)
        }))
        .map_err(|_| {
            recovery_error(
                OPTIMIZER,
                "invalid Adam state or backend rejected restoration",
            )
        })?;
        if optimizer.to_record().len() != state.optimizer_tensors {
            return Err(recovery_error(
                OPTIMIZER,
                "incomplete Adam state was refused",
            ));
        }
        self.model = model;
        self.optimizer = optimizer;
        self.config = state.config;
        self.updates = state.updates;
        self.shuffle = state.shuffle;
        Ok(())
    }

    fn validate_recovery(&self, state: &TrainerState) -> Result<(), TrainError> {
        if state.version != VERSION || state.shape != self.recovery_shape() {
            return Err(recovery_error(
                META,
                "trainer schema or model shape mismatch",
            ));
        }
        state.config.validate()?;
        if state.pre_attn_eps.len() != state.shape.num_layers
            || state.post_attn_eps.len() != state.shape.num_layers
            || state
                .pre_attn_eps
                .iter()
                .chain(&state.post_attn_eps)
                .chain([&state.final_eps])
                .any(|epsilon| !epsilon.is_finite() || *epsilon <= 0.0)
            || (state.updates > 0 && state.optimizer_tensors == 0)
        {
            return Err(recovery_error(
                META,
                "missing optimizer state or invalid normalization epsilon",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "burn_recovery_tests.rs"]
mod tests;
