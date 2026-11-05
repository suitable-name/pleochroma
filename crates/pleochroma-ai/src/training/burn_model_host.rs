//! The batched rollout forward of the Burn model, and the state packing the trainers share.
//!
//! [`BurnSetAttention::forward_host`] runs a batch of flat observations on the model's
//! device and reads policy means, log-stds and values back together. Outputs are trimmed to
//! each sample's real tiers. The caller samples on the host using its own random stream.
//!
//! # No gradient tracking
//!
//! Burn 0.22 has no switch on a forward call that turns tracking off; a computation records a
//! graph when one of its operands is tracked, and a trainer's parameters are autodiff leaves.
//! The forward therefore runs on `Module::valid()` of the model: the same weights re-wrapped
//! without their autodiff association (the data is shared, nothing is copied), with the input
//! tensors created on the device without autodiff as well. `Module::no_grad()` would only
//! clear the gradient requirement and keep the autodiff dispatch, and `Tensor::detach` works
//! per tensor; `valid()` is the supported inference transition and also what
//! `BurnTrainer::rollout_model` hands out. [`BurnSetAttention::forward_host`] applies it
//! itself, so it is gradient-free whichever model it is called on.
//!
//! # Determinism
//!
//! The numbers of this pass are the device's. GPU f32 kernels can differ from the pure-Rust
//! twin in their last bits; tests compare them within about `1e-4` per entry. Batch shape can
//! also change numerical results. Adaptive rollout grouping depends on completion timing, so
//! the same device and thread layout do not guarantee an identical rollout. CPU evaluation
//! and standalone inference use the pure-Rust twin (`GemstoneSetAttentionNetwork`); the
//! trainer's selected GPU validation backend uses this batched path. Checkpoints retain the
//! exported twin.
//!
//! # Packing
//!
//! `pack_states` cuts flat states (`[global | padded tier tokens]`, what
//! `GemstoneObservation::flatten(max_tiers)` produces) into the three model inputs. The PPO
//! trainer and the rollout forward pack through it, so a state is laid out the same way
//! wherever the model reads it. (The imitation trainer's `pack_imitation` repeats the same three
//! copies next to its labels and could call it.)

use burn::{prelude::*, tensor::TensorData};
use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::{
    TrainError,
    burn_model::{BurnSetAttention, host_vec},
};

/// The policy-relevant outputs of one batched rollout forward pass, as plain host vectors.
///
/// Row `i` of every field belongs to sample `i` of the batch handed to
/// [`BurnSetAttention::forward_host`], trimmed to that sample's `n_tiers[i]` real tiers
/// (padding positions are dropped, not zeroed).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostPolicyBatch {
    /// Policy means (sensitivity direction plus action residual) in pre-squash units; sample `i`
    /// has `n_tiers[i]` entries. Tiers the environment does not move are not distinguished
    /// here: the caller masks them (`GemstoneObservation::free_tier_mask`).
    pub means: Vec<Vec<f32>>,
    /// Policy log standard deviations (state-independent, selected per tier by its block), of
    /// the same shape as `means`.
    pub log_stds: Vec<Vec<f32>>,
    /// The critic's baseline `V(s)` of each sample.
    pub values: Vec<f32>,
}

/// Host wall timings for a forward, without additional device synchronization.
/// Lazy execution and first-use compilation can appear in readback rather than submission;
/// these are observable stage durations, not compiler or GPU-kernel timings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct HostForwardDiagnostics {
    /// Actual observations, in caller order.
    pub real_rows: usize,
    /// Device row count, including masked dummy rows.
    pub padded_rows: usize,
    /// Device tier width, including masked padding positions.
    pub padded_tiers: usize,
    /// Host input shape and finiteness validation.
    pub validation_ms: f64,
    /// Creation of the gradient-free model view and device handle.
    pub inference_view_ms: f64,
    /// Host packing and device tensor creation/submission.
    pub packing_ms: f64,
    /// Model forward submission, before the bundled output readback.
    pub forward_submit_ms: f64,
    /// Bundled output readback, including any pending device work it waits for.
    pub readback_ms: f64,
    /// Host output validation and trimming to the real rows and tier counts.
    pub unpacking_ms: f64,
}

/// Round tier widths to a small set of reusable shapes, bounded by the input layout.
/// This changes padding only: rows and sampling order are never regrouped.
pub(crate) fn tier_bucket(real: usize, limit: usize) -> usize {
    real.max(1).next_power_of_two().min(limit)
}

/// The widths a batch of flat states is cut by (see [`pack_states`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackDims {
    /// Width of the global token.
    pub(crate) d_global: usize,
    /// Width of one tier token.
    pub(crate) d_tier: usize,
    /// Padded tier count: how many tier tokens follow the global token in a state.
    pub(crate) max_tiers: usize,
}

/// The three model inputs of a batch of states, on one device.
pub(crate) struct StateTensors {
    /// Tier tokens, `[batch, max_tiers, d_tier]`.
    pub(crate) tiers: Tensor<3>,
    /// Global tokens, `[batch, d_global]`.
    pub(crate) globals: Tensor<2>,
    /// Real-tier mask, `[batch, max_tiers]`: `1.0` for the first `n` tiers of a sample, `0.0`
    /// for its padding.
    pub(crate) mask: Tensor<2>,
}

/// Copies flat states into the model's three inputs on `device`.
///
/// `rows` yields, per sample, its flat state (`[global | max_tiers tier tokens]`) and its real
/// tier count; the output keeps the iteration order. Padding is zeroed before projection so
/// even large finite padding values cannot overflow and contaminate masked computations.
/// Callers validate shapes first (`TrainBatch`'s validation, the checks of
/// [`BurnSetAttention::forward_host`]).
///
/// # Panics
///
/// When a state is shorter than `d_global + n_tiers * d_tier` values.
pub(crate) fn pack_states<'a, I>(rows: I, dims: PackDims, device: &Device) -> StateTensors
where
    I: ExactSizeIterator<Item = (&'a [f32], usize)>,
{
    pack_states_padded(rows, dims, device, 0)
}

/// Append masked dummy rows for inference without changing the real samples.
fn pack_states_padded<'a, I>(
    rows: I,
    dims: PackDims,
    device: &Device,
    dummy_rows: usize,
) -> StateTensors
where
    I: ExactSizeIterator<Item = (&'a [f32], usize)>,
{
    let PackDims {
        d_global: dg,
        d_tier: dt,
        max_tiers: mt,
    } = dims;
    let b = rows.len() + dummy_rows;
    let mut globals = Vec::with_capacity(b * dg);
    let mut tiers = Vec::with_capacity(b * mt * dt);
    let mut mask = Vec::with_capacity(b * mt);
    for (state, n) in rows {
        globals.extend_from_slice(&state[..dg]);
        tiers.extend_from_slice(&state[dg..dg + n * dt]);
        tiers.resize(tiers.len() + (mt - n) * dt, 0.0);
        mask.extend((0..mt).map(|k| if k < n { 1.0f32 } else { 0.0 }));
    }
    globals.resize(b * dg, 0.0);
    tiers.resize(b * mt * dt, 0.0);
    mask.resize(b * mt, 0.0);
    StateTensors {
        tiers: Tensor::<3>::from_data(TensorData::new(tiers, [b, mt, dt]), device),
        globals: Tensor::<2>::from_data(TensorData::new(globals, [b, dg]), device),
        mask: Tensor::<2>::from_data(TensorData::new(mask, [b, mt]), device),
    }
}

impl BurnSetAttention {
    /// One forward pass over a whole batch of flat observations, read back to the host.
    ///
    /// `states[i]` is the flat observation of sample `i` (`[global token | padded tier tokens]`,
    /// what `GemstoneObservation::flatten(max_tiers)` produces) and `n_tiers[i]` its real tier
    /// count. Every state has the same length `d_global + max_tiers * d_tier` with
    /// `max_tiers >= 1`; the padded tier count is derived from it. The batch is packed on the
    /// model's device like a training mini-batch, run through [`Self::forward`] on a
    /// gradient-free view of the model (see the module docs), and the means, log-stds and values
    /// are read back together and cut to the samples' real tiers. The forward width is rounded up
    /// to a reusable tier bucket, bounded by the input layout; the row count is rounded to the
    /// next power of two with masked dummy rows. An empty batch gives an empty result and does not
    /// touch the device.
    ///
    /// The samples are independent rows: a sample's numbers do not depend on the other samples
    /// or on how far the batch is padded, up to the last bits of the device's kernels. The
    /// numbers are the device's, not the pure-Rust twin's (about `1e-4` apart per entry).
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] when `states` and `n_tiers` differ in length, the first
    /// state is not `d_global + k * d_tier` long for a `k >= 1`, a later state has another
    /// length, or an `n_tiers[i]` is 0 or exceeds the padded tier count;
    /// [`TrainError::NonFinite`] when a state holds a NaN or an infinity (the outputs would be
    /// meaningless); [`TrainError::Tensor`] when an output cannot be read back.
    pub fn forward_host(
        &self,
        states: &[Vec<f32>],
        n_tiers: &[usize],
    ) -> Result<HostPolicyBatch, TrainError> {
        self.forward_host_profiled(states, n_tiers)
            .map(|(batch, _)| batch)
    }

    /// The same forward and output order as [`Self::forward_host`], with host stage timings.
    /// Includes the existing bundled readback, and introduces no device synchronization.
    ///
    /// # Errors
    ///
    /// Returns the same shape, finiteness, and readback errors as [`Self::forward_host`].
    pub fn forward_host_profiled(
        &self,
        states: &[Vec<f32>],
        n_tiers: &[usize],
    ) -> Result<(HostPolicyBatch, HostForwardDiagnostics), TrainError> {
        let validation = Instant::now();
        let Some(input_tiers) = self.check_host_batch(states, n_tiers)? else {
            return Ok((
                HostPolicyBatch::default(),
                HostForwardDiagnostics::default(),
            ));
        };
        let mut diagnostics = HostForwardDiagnostics {
            real_rows: states.len(),
            validation_ms: validation.elapsed().as_secs_f64() * 1000.0,
            ..HostForwardDiagnostics::default()
        };
        let max_tiers = tier_bucket(n_tiers.iter().copied().max().unwrap_or(1), input_tiers);
        let view = Instant::now();
        let inference = self.valid();
        let device = inference.tier_proj.weight.val().device().without_autodiff();
        diagnostics.inference_view_ms = view.elapsed().as_secs_f64() * 1000.0;
        let packing = Instant::now();
        let dims = PackDims {
            d_global: self.d_global,
            d_tier: self.d_tier,
            max_tiers,
        };
        let rows = states
            .iter()
            .zip(n_tiers)
            .map(|(state, &n)| (state.as_slice(), n));
        // Round only the row count; dummy rows have zero features and no real tiers.
        // The context token remains a valid attention key, so softmax stays finite.
        let padded_batch = states.len().next_power_of_two();
        diagnostics.padded_rows = padded_batch;
        diagnostics.padded_tiers = max_tiers;
        let input = pack_states_padded(rows, dims, &device, padded_batch - states.len());
        diagnostics.packing_ms = packing.elapsed().as_secs_f64() * 1000.0;
        let forward = Instant::now();
        let out = inference.forward(input.tiers, input.globals, input.mask);
        diagnostics.forward_submit_ms = forward.elapsed().as_secs_f64() * 1000.0;

        // Bundle all rollout outputs into one read-back. Copying one tensor three times adds
        // synchronization points on accelerators, even though the sampling loop needs all fields.
        let readback = Instant::now();
        let packed = host_vec(Tensor::cat(
            vec![out.mean, out.log_std, out.value.reshape([padded_batch, 1])],
            1,
        ))?;
        diagnostics.readback_ms = readback.elapsed().as_secs_f64() * 1000.0;
        let unpacking = Instant::now();
        let row_width = 2 * max_tiers + 1;
        if packed.len() != padded_batch * row_width {
            return Err(TrainError::Tensor(format!(
                "read back {} rollout values for {padded_batch} padded samples of {row_width}",
                packed.len()
            )));
        }
        let mut means = Vec::with_capacity(states.len());
        let mut log_stds = Vec::with_capacity(states.len());
        let mut values = Vec::with_capacity(states.len());
        for (row, &n) in packed.chunks_exact(row_width).zip(n_tiers) {
            means.push(row[..n].to_vec());
            log_stds.push(row[max_tiers..max_tiers + n].to_vec());
            values.push(row[2 * max_tiers]);
        }
        let batch = HostPolicyBatch {
            means,
            log_stds,
            values,
        };
        diagnostics.unpacking_ms = unpacking.elapsed().as_secs_f64() * 1000.0;
        Ok((batch, diagnostics))
    }

    /// Checks a rollout batch against the model and returns its padded tier count, `None` for
    /// an empty batch (see [`Self::forward_host`] for the errors).
    fn check_host_batch(
        &self,
        states: &[Vec<f32>],
        n_tiers: &[usize],
    ) -> Result<Option<usize>, TrainError> {
        if states.len() != n_tiers.len() {
            return Err(TrainError::ShapeMismatch(format!(
                "{} states but {} tier counts",
                states.len(),
                n_tiers.len()
            )));
        }
        let Some(first) = states.first() else {
            return Ok(None);
        };
        let row_len = first.len();
        let Some(max_tiers) = self.tiers_in_state(row_len) else {
            return Err(TrainError::ShapeMismatch(format!(
                "state 0 has {row_len} values, expected {} global plus a whole number (at least \
                 one) of {}-value tiers",
                self.d_global, self.d_tier
            )));
        };
        for (i, (state, &n)) in states.iter().zip(n_tiers).enumerate() {
            if state.len() != row_len {
                return Err(TrainError::ShapeMismatch(format!(
                    "state {i} has {} values, state 0 has {row_len}",
                    state.len()
                )));
            }
            if n == 0 || n > max_tiers {
                return Err(TrainError::ShapeMismatch(format!(
                    "sample {i} has {n} tiers, expected 1..={max_tiers}"
                )));
            }
            if state.iter().any(|value| !value.is_finite()) {
                return Err(TrainError::NonFinite(format!(
                    "state {i} holds a NaN or infinite value"
                )));
            }
        }
        Ok(Some(max_tiers))
    }

    /// The tier slots of a flat state of `len` values: `(len - d_global) / d_tier`, `None`
    /// unless that is a whole number of at least one.
    fn tiers_in_state(&self, len: usize) -> Option<usize> {
        let tier_values = len.checked_sub(self.d_global)?;
        (tier_values.checked_rem(self.d_tier)? == 0)
            .then(|| tier_values / self.d_tier)
            .filter(|&tiers| tiers > 0)
    }
}

#[cfg(test)]
#[path = "burn_model_host_tests.rs"]
mod tests;
