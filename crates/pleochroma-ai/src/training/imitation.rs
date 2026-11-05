//! Bounded supervised imitation of the seven-component finite-difference teacher.
//!
//! Component sensitivities use a masked Huber regression. Raw masked MSE and per-component support
//! remain telemetry. Verified actions use balanced Huber and bounded vector losses; oracle gain
//! uses a separate Huber term. Immobile and padded tiers cannot receive action or label weight.
//! Selected rows are packed once per bounded window and gathered into device mini-batches under a
//! 32 MiB prepared-input payload bound. Unavailable metrics are masked out.

use burn::{optim::GradientsParams, prelude::*, tensor::TensorData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

use super::{
    LrSchedule, TrainError,
    burn_model::{BurnForward, COUNT_CLAMP_MAX, host_vec},
    burn_ppo::{BurnTrainer, clip_global_grad_norm},
};
use crate::{
    env::{GLOBAL_DIM, MOBILITY_INDEX, N_COMPONENTS, TIER_DIM},
    model::Prng,
};

/// Added to the product of the two norms in the cosine denominator.
const COSINE_EPS: f32 = 1.0e-8;
/// Added under the square root of the predicted direction's squared norm. The gradient of a norm
/// is infinite at zero; with this term a sample without direction labels (prediction masked to
/// zero) yields a gradient of exactly zero instead of NaN.
const NORM_SQ_EPS: f32 = 1.0e-12;
/// Knee of the Huber loss on the gain, in points.
const HUBER_DELTA: f32 = 1.0;
/// Maximum observation and label tensor payload for one prepared window; requested updates are
/// streamed through additional windows rather than truncated to fit.
const PREPARED_ROUND_BUDGET_BYTES: usize = 32 * 1024 * 1024;
/// Bounds the schedule metadata when a tiny replay subset is sampled repeatedly.
const PREPARED_BATCHES_PER_WINDOW: usize = 4096;

/// Use a reusable upload shape only when all padding fits the prepared-input budget.
/// An oversized configured mini-batch keeps its original exact row count.
fn prepared_row_count(real: usize, needs_dummy: bool, capacity: usize) -> usize {
    let bucket = (real + usize::from(needs_dummy)).next_power_of_two();
    if bucket <= capacity { bucket } else { real }
}

/// Default of [`ImitationConfig::updates_per_round`]: 64 mini-batches of 256 samples per round.
const DEFAULT_UPDATES_PER_ROUND: usize = 64;

/// Training objective revision: robust component regression, balanced verified actions, and
/// bounded direction discrepancy. Recovery may retain Adam while explicitly migrating here.
pub const IMITATION_OBJECTIVE_REVISION: u32 = 2;

/// Hyperparameters of the imitation trainer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImitationConfig {
    /// Adam learning rate (peak value when a decaying schedule is used).
    pub learning_rate: f32,
    /// Learning-rate schedule over optimizer updates (the trainer's own update counter).
    pub lr_schedule: LrSchedule,
    /// Samples per mini-batch (one optimizer update each); capped at the batch size.
    pub mini_batch_size: usize,
    /// Passes over the batch per call, reshuffled every pass. Used only when
    /// [`Self::updates_per_round`] is `0`.
    pub epochs_per_round: usize,
    /// Optimizer updates per call. `0` restores the full-epoch behaviour (`epochs_per_round`
    /// passes over the whole batch, a cost that grows with the batch). `n > 0` takes exactly `n`
    /// mini-batches of `mini_batch_size` samples, each drawn uniformly without replacement from
    /// the batch (mini-batches are independent of each other), so the cost of a call does not
    /// depend on the batch size. A batch smaller than one mini-batch is used whole every time.
    pub updates_per_round: usize,
    /// Maximum **global** gradient L2 norm; a larger gradient is rescaled to it before the step.
    pub max_grad_norm: f32,
    /// Weight of the bounded direction discrepancy. The field name is retained for saved configs;
    /// cosine itself is reported as a diagnostic independently of the optimized loss.
    pub cosine_coeff: f32,
    /// Weight of the per-sample Huber regression on verified, squashed action fractions, balanced
    /// between moving and stationary coordinates.
    pub action_coeff: f32,
    /// Weight of the gain-regression term.
    pub gain_coeff: f32,
}

impl Default for ImitationConfig {
    fn default() -> Self {
        Self {
            learning_rate: 3e-4,
            lr_schedule: LrSchedule::Constant,
            mini_batch_size: 256,
            epochs_per_round: 2,
            updates_per_round: DEFAULT_UPDATES_PER_ROUND,
            max_grad_norm: 1.0,
            cosine_coeff: 0.5,
            action_coeff: 1.0,
            gain_coeff: 0.1,
        }
    }
}

impl ImitationConfig {
    /// Reject out-of-range hyperparameters instead of silently clamping them.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] naming the first offending field: a zero mini-batch size or
    /// epoch count, a learning rate or gradient-norm cap that is not finite and positive, a
    /// coefficient that is not finite and non-negative, or a linear schedule over zero updates.
    pub fn validate(&self) -> Result<(), TrainError> {
        if self.mini_batch_size == 0 {
            return Err(TrainError::InvalidConfig(
                "mini_batch_size must be >= 1".into(),
            ));
        }
        if self.epochs_per_round == 0 {
            return Err(TrainError::InvalidConfig(
                "epochs_per_round must be >= 1".into(),
            ));
        }
        for (name, value) in [
            ("learning_rate", self.learning_rate),
            ("max_grad_norm", self.max_grad_norm),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(TrainError::InvalidConfig(format!(
                    "{name} must be finite and > 0 (got {value})"
                )));
            }
        }
        for (name, value) in [
            ("cosine_coeff", self.cosine_coeff),
            ("action_coeff", self.action_coeff),
            ("gain_coeff", self.gain_coeff),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(TrainError::InvalidConfig(format!(
                    "{name} must be finite and >= 0 (got {value})"
                )));
            }
        }
        if let LrSchedule::Linear { total_updates } = self.lr_schedule
            && total_updates == 0
        {
            return Err(TrainError::InvalidConfig(
                "lr_schedule total_updates must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// Borrowed labelled states for one imitation call; all slices are parallel (one entry per sample).
#[derive(Debug, Clone, Copy)]
pub struct ImitationBatch<'a> {
    /// Flat observations, `[global | tier tokens]`, either compact at `n_tiers[i]` or padded to
    /// the model's maximum tier count. Device windows supply their own bounded padding.
    pub states: &'a [Vec<f32>],
    /// Number of real (unpadded) tiers of each sample, at least 1 and at most `max_tiers`.
    pub n_tiers: &'a [usize],
    /// Free-tier mask of each sample, one flag per real tier. Labels of a tier that is not free
    /// are ignored.
    pub free_tiers: &'a [Vec<bool>],
    /// Per sample, per real tier: the seven component slopes (points per degree), NaN = no label
    /// for that entry; a tier without a label is all NaN. Length `n_tiers[i]`.
    pub sensitivities: &'a [Vec<[f32; N_COMPONENTS]>],
    /// Per sample, per real tier: the score slope (points per degree, NaN = none), retained
    /// for diagnostics. Unverified slopes never become policy-action labels.
    pub score_slopes: &'a [Vec<f32>],
    /// Verified pre-squash actions, one complete row of `n_tiers[i]` finite values per sample.
    /// An entirely NaN row means no action label. Inactive tiers are masked, including padding;
    /// verified zeros on mobile tiers are supervised so unrelated coordinates do not drift.
    pub action_targets: &'a [Vec<f32>],
    /// Per sample: the score drop of the oracle step (points), NaN = none.
    pub oracle_gain: &'a [f32],
}

impl ImitationBatch<'_> {
    /// Number of samples.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.states.len()
    }

    /// Whether the batch holds no samples.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Check that the slices agree and that every sample has the layout the trainer expects.
    ///
    /// Missing derivative labels are NaN. An action row is either wholly finite or wholly NaN;
    /// mixing partial actions could supervise a different move from the one the teacher verified.
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] naming the sample index and the field when the slices differ
    /// in length, a state is neither compact nor padded to `max_tiers`, `n_tiers[i]` is
    /// outside `1..=max_tiers`, or a per-tier slice does not have `n_tiers[i]` entries;
    /// [`TrainError::NonFinite`] for a non-finite state or an action row that is neither entirely
    /// finite nor entirely NaN.
    pub fn validate(
        &self,
        d_global: usize,
        d_tier: usize,
        max_tiers: usize,
    ) -> Result<(), TrainError> {
        if d_global != GLOBAL_DIM || d_tier != TIER_DIM {
            return Err(TrainError::ShapeMismatch(format!(
                "imitation data uses observation layout {}x{}, trainer model is {}x{}",
                GLOBAL_DIM, TIER_DIM, d_global, d_tier
            )));
        }
        self.validate_lengths()?;
        let row_len = d_global + max_tiers * d_tier;
        for i in 0..self.len() {
            self.validate_sample(i, row_len, max_tiers)?;
        }
        Ok(())
    }

    /// Every parallel slice holds one entry per state.
    fn validate_lengths(&self) -> Result<(), TrainError> {
        let n = self.states.len();
        let fields = [
            ("n_tiers", self.n_tiers.len()),
            ("free_tiers", self.free_tiers.len()),
            ("sensitivities", self.sensitivities.len()),
            ("score_slopes", self.score_slopes.len()),
            ("action_targets", self.action_targets.len()),
            ("oracle_gain", self.oracle_gain.len()),
        ];
        fields
            .iter()
            .find(|&&(_, len)| len != n)
            .map_or(Ok(()), |&(name, len)| {
                Err(TrainError::ShapeMismatch(format!(
                    "batch slices disagree: {n} states but {len} entries of `{name}`"
                )))
            })
    }

    /// Layout and finiteness of sample `i`.
    fn validate_sample(
        &self,
        i: usize,
        row_len: usize,
        max_tiers: usize,
    ) -> Result<(), TrainError> {
        let mismatch = |detail: String| TrainError::ShapeMismatch(format!("sample {i}: {detail}"));
        let state = &self.states[i];
        let n = self.n_tiers[i];
        if n == 0 || n > max_tiers {
            return Err(mismatch(format!(
                "`n_tiers` is {n}, expected 1..={max_tiers}"
            )));
        }
        let compact_len = GLOBAL_DIM + n * TIER_DIM;
        if state.len() != row_len && state.len() != compact_len {
            return Err(mismatch(format!(
                "`states` row has {} values, expected compact {compact_len} or padded {row_len}",
                state.len()
            )));
        }
        for (field, len) in [
            ("free_tiers", self.free_tiers[i].len()),
            ("sensitivities", self.sensitivities[i].len()),
            ("score_slopes", self.score_slopes[i].len()),
            ("action_targets", self.action_targets[i].len()),
        ] {
            if len != n {
                return Err(mismatch(format!(
                    "`{field}` has {len} entries, expected {n} (one per real tier)"
                )));
            }
        }
        if state.iter().any(|v| !v.is_finite()) {
            return Err(TrainError::NonFinite(format!(
                "sample {i}: the state holds a NaN or infinite value"
            )));
        }
        let actions = &self.action_targets[i];
        if !(actions.iter().all(|v| v.is_nan()) || actions.iter().all(|v| v.is_finite())) {
            return Err(TrainError::NonFinite(format!(
                "sample {i}: `action_targets` must be entirely finite or entirely NaN"
            )));
        }
        Ok(())
    }
}

/// Means over the optimizer updates of one [`BurnTrainer::train_imitation_epochs`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ImitationMetrics {
    /// Mean total loss (the loss each update saw before its step).
    pub loss: f32,
    /// Mean squared error of the predicted sensitivities over the labelled entries.
    pub sens_mse: f32,
    /// Mean cosine between the squashed policy mean and the verified teacher action, over
    /// nonzero action targets (`1` is perfect, no usable direction counts as `0`).
    pub cosine: f32,
    /// Mean per-sample Huber error of verified, squashed policy actions, balanced between moving
    /// and stationary coordinates.
    #[serde(default)]
    pub action_loss: f32,
    /// Selected unique samples with a verified action on at least one free, mobile tier.
    /// Counted once per call, including verified zero coordinates, independently of loss weight.
    #[serde(default)]
    pub action_samples: usize,
    /// Mean Huber loss of the predicted gain.
    pub gain_loss: f32,
    /// Mean pre-clip global gradient L2 norm.
    pub grad_norm: f32,
    /// Real, free tiers of the batch that carry at least one finite component label; counted once
    /// over the whole batch, not once per epoch.
    pub labelled_tiers: usize,
    /// Number of finite, mobile-tier labels for each objective component, in env component order.
    /// This reports target support independently of raw MSE, so an unavailable metric is visible.
    #[serde(default)]
    pub component_support: [usize; N_COMPONENTS],
    /// Optimizer steps taken (mini-batches without any label are skipped and not counted).
    pub updates: usize,
    /// Labelled mini-batches whose loss or gradient norm was not finite. Each was skipped
    /// without an optimizer step: weights, Adam moments and the update counter are untouched.
    #[serde(default)]
    pub skipped_minibatches: usize,
    /// Cumulative host elapsed milliseconds spent preparing and uploading bounded row windows.
    /// This is not device-kernel time; asynchronous device work may spill into later phases.
    #[serde(default)]
    pub packing_ms: f32,
    /// Cumulative host elapsed milliseconds spent gathering and updating mini-batches, including
    /// existing device readbacks for loss and gradient guards. Not GPU utilization.
    #[serde(default)]
    pub update_ms: f32,
    /// Host wall stage durations; lazy device execution/JIT may spill into readbacks.
    /// No additional synchronization is inserted to collect these figures.
    #[serde(default)]
    pub stages: ImitationStageDiagnostics,
    /// Loss definition of these figures: 0 for legacy/absent updates, 2 for balanced actions.
    #[serde(default)]
    pub objective_revision: u32,
}

/// Observable imitation stages and largest tensor shapes used by one update round.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ImitationStageDiagnostics {
    /// Host configuration, shape and finite-input validation for the round.
    pub validation_ms: f32,
    /// Device index tensor creation and mini-batch gather submission.
    pub gather_ms: f32,
    /// Model forward and masked loss submission before the scalar readback.
    pub forward_loss_submit_ms: f32,
    /// Existing scalar loss readback, including pending device work it waits for.
    pub loss_readback_ms: f32,
    /// Includes the gradient-norm readback needed by the existing finite-gradient guard.
    pub backward_optimizer_ms: f32,
    /// Number of bounded windows uploaded, including windows whose targets are all absent.
    pub prepared_windows: usize,
    /// Largest device row count of a prepared upload, including masked padding rows.
    pub max_prepared_rows: usize,
    /// Largest gathered row count used by an optimizer step, including masked padding rows.
    pub max_minibatch_rows: usize,
    /// Largest tier width uploaded, including masked tier padding.
    pub max_padded_tiers: usize,
}

/// Loss/gradient aggregates and cumulative host timings for one imitation call.
#[derive(Debug, Clone, Copy, Default)]
struct UpdateStats {
    loss: f32,
    sens_mse: f32,
    cosine: f32,
    action_loss: f32,
    gain_loss: f32,
    grad_norm: f32,
    packing_ms: f32,
    update_ms: f32,
    stages: ImitationStageDiagnostics,
    /// Labelled mini-batches skipped for a non-finite loss or gradient norm (sums only).
    skipped_minibatches: usize,
}

/// What one mini-batch did to the trainer.
#[derive(Clone, Copy)]
enum MinibatchOutcome {
    /// One optimizer step was taken; its figures.
    Stepped(UpdateStats),
    /// No row carries a label: no forward pass ran.
    Unlabelled,
    /// The loss or the global gradient norm was not finite: no optimizer step was taken, so the
    /// weights, the Adam moments and the update counter are those before the mini-batch.
    NonFinite,
}

#[cfg(test)]
impl MinibatchOutcome {
    /// The figures of a mini-batch that took an optimizer step.
    const fn stepped(self) -> Option<UpdateStats> {
        match self {
            Self::Stepped(stats) => Some(stats),
            Self::Unlabelled | Self::NonFinite => None,
        }
    }
}

/// Add one update's figures into the running sums (the mean is taken by the caller).
fn add_stats(sum: &mut UpdateStats, m: &UpdateStats) {
    sum.loss += m.loss;
    sum.sens_mse += m.sens_mse;
    sum.cosine += m.cosine;
    sum.action_loss += m.action_loss;
    sum.gain_loss += m.gain_loss;
    sum.grad_norm += m.grad_norm;
    sum.packing_ms += m.packing_ms;
    sum.update_ms += m.update_ms;
    sum.stages.gather_ms += m.stages.gather_ms;
    sum.stages.forward_loss_submit_ms += m.stages.forward_loss_submit_ms;
    sum.stages.loss_readback_ms += m.stages.loss_readback_ms;
    sum.stages.backward_optimizer_ms += m.stages.backward_optimizer_ms;
    sum.stages.max_minibatch_rows = sum
        .stages
        .max_minibatch_rows
        .max(m.stages.max_minibatch_rows);
}

/// Move `count` uniformly drawn entries of `order` to its front (a partial Fisher-Yates step), so
/// that `order[..count]` is a uniform sample without replacement. `order` must stay a permutation
/// of the sample indices between calls; whatever permutation it holds, the draw is uniform.
fn draw_minibatch(rng: &mut Prng, order: &mut [usize], count: usize) {
    let total = order.len();
    for at in 0..count.min(total) {
        let pick = at + rng.next_index(total - at);
        order.swap(at, pick);
    }
}

/// `1.0` for `true`, `0.0` for `false`: the value of a mask entry.
const fn flag(on: bool) -> f32 {
    if on { 1.0 } else { 0.0 }
}

/// Whether tier `k` is real, free, and mobile in this observation.
fn is_actionable_tier(batch: &ImitationBatch<'_>, sample: usize, k: usize) -> bool {
    let n = batch.n_tiers[sample];
    if k >= n || !batch.free_tiers[sample].get(k).copied().unwrap_or(false) {
        return false;
    }
    let offset = GLOBAL_DIM + k * TIER_DIM + MOBILITY_INDEX;
    batch.states[sample]
        .get(offset)
        .is_some_and(|&mobile| mobile > 0.5)
}

/// The finite component labels of tier `k`; every entry is `None` unless the tier is `usable`
/// (real and free) and has a row.
fn component_labels(
    sens: &[[f32; N_COMPONENTS]],
    k: usize,
    usable: bool,
) -> [Option<f32>; N_COMPONENTS] {
    let row = sens.get(k).filter(|_| usable);
    std::array::from_fn(|c| row.map(|r| r[c]).filter(|v| v.is_finite()))
}

/// Real, free tiers of the whole batch with at least one finite component label.
fn count_labelled_tiers(batch: &ImitationBatch<'_>, indices: &[usize]) -> usize {
    indices
        .iter()
        .map(|&i| {
            let n = batch.n_tiers[i];
            (0..n)
                .filter(|&k| {
                    component_labels(&batch.sensitivities[i], k, is_actionable_tier(batch, i, k))
                        .iter()
                        .any(Option::is_some)
                })
                .count()
        })
        .sum()
}

/// Count finite labels by component, on real tiers the environment can move.
fn count_component_support(batch: &ImitationBatch<'_>, indices: &[usize]) -> [usize; N_COMPONENTS] {
    let mut support = [0; N_COMPONENTS];
    for &i in indices {
        let n = batch.n_tiers[i];
        for k in 0..n {
            if is_actionable_tier(batch, i, k) {
                for (component, label) in batch.sensitivities[i][k].iter().enumerate() {
                    support[component] += usize::from(label.is_finite());
                }
            }
        }
    }
    support
}

/// Count verified action support among the selected unique rows.
fn count_action_samples(batch: &ImitationBatch<'_>, indices: &[usize]) -> usize {
    indices
        .iter()
        .filter(|&&i| {
            batch.action_targets[i]
                .iter()
                .enumerate()
                .any(|(k, action)| is_actionable_tier(batch, i, k) && action.is_finite())
        })
        .count()
}

#[path = "imitation_labels.rs"]
mod labels;
use labels::*;

/// Label tensors of one mini-batch on the training device.
struct LabelTensors {
    /// `[batch, max_tiers, N_COMPONENTS]`
    sens_target: Tensor<3>,
    sens_mask: Tensor<3>,
    /// `[batch, max_tiers]`
    action_target: Tensor<2>,
    action_mask: Tensor<2>,
    /// `[batch, 1]`
    gain_target: Tensor<2>,
    gain_mask: Tensor<2>,
}

/// One mini-batch on the training device.
struct PackedImitation {
    /// `[batch, max_tiers, d_tier]`
    tiers: Tensor<3>,
    /// `[batch, d_global]`
    globals: Tensor<2>,
    /// Real-tier mask (attention keys, value pooling), `[batch, max_tiers]`.
    mask: Tensor<2>,
    labels: LabelTensors,
    /// Per-row label support, used to skip optimizer steps without any target.
    row_has_labels: Vec<bool>,
    /// A zero-feature row with zero tier/label masks, reserved for mini-batch padding.
    dummy_row: Option<usize>,
}

impl PackedImitation {
    /// Gather a deterministic mini-batch from the prepared round tensors on the training device.
    fn gather_rows(&self, rows: &[usize], device: &Device) -> Self {
        let mut index_values = rows.iter().map(|&row| row as i64).collect::<Vec<_>>();
        if let Some(dummy) = self.dummy_row {
            index_values.resize(rows.len().next_power_of_two(), dummy as i64);
        }
        let index_len = index_values.len();
        let indices =
            Tensor::<1, Int>::from_data(TensorData::new(index_values, [index_len]), device);
        let mut row_has_labels: Vec<_> = rows.iter().map(|&row| self.row_has_labels[row]).collect();
        row_has_labels.resize(index_len, false);
        Self {
            tiers: self.tiers.clone().select(0, indices.clone()),
            globals: self.globals.clone().select(0, indices.clone()),
            mask: self.mask.clone().select(0, indices.clone()),
            labels: LabelTensors {
                sens_target: self.labels.sens_target.clone().select(0, indices.clone()),
                sens_mask: self.labels.sens_mask.clone().select(0, indices.clone()),
                action_target: self.labels.action_target.clone().select(0, indices.clone()),
                action_mask: self.labels.action_mask.clone().select(0, indices.clone()),
                gain_target: self.labels.gain_target.clone().select(0, indices.clone()),
                gain_mask: self.labels.gain_mask.clone().select(0, indices),
            },
            row_has_labels,
            dummy_row: None,
        }
    }
}

#[path = "imitation_loss.rs"]
mod loss;
use loss::*;

impl BurnTrainer {
    /// Conservative prepared-input capacity using the trainer's largest supported tier width.
    fn imitation_row_capacity(&self) -> usize {
        let row_values = self.model.d_global
            + self.max_tiers * self.model.d_tier
            + self.max_tiers * (2 * N_COMPONENTS + 3)
            + 2;
        let bytes_per_row = row_values.saturating_mul(std::mem::size_of::<f32>());
        (PREPARED_ROUND_BUDGET_BYTES / bytes_per_row.max(1)).max(1)
    }

    /// Copy the samples `idx` of `batch` into device tensors (padding tiers with zeros), labels
    /// included. The batch must have passed [`ImitationBatch::validate`].
    #[cfg(test)]
    fn pack_imitation(&self, batch: &ImitationBatch<'_>, idx: &[usize]) -> PackedImitation {
        self.pack_imitation_padded(batch, idx, idx.len())
    }

    /// Upload a stable row bucket; every added row has zero features and zero target masks.
    fn pack_imitation_padded(
        &self,
        batch: &ImitationBatch<'_>,
        idx: &[usize],
        padded_rows: usize,
    ) -> PackedImitation {
        let (dt, dg) = (self.model.d_tier, self.model.d_global);
        let real_tiers = idx.iter().map(|&i| batch.n_tiers[i]).max().unwrap_or(1);
        let mt = super::burn_model_host::tier_bucket(real_tiers, self.max_tiers);
        let b = padded_rows;
        let mut globals = Vec::with_capacity(b * dg);
        let mut tiers = Vec::with_capacity(b * mt * dt);
        let mut mask = Vec::with_capacity(b * mt);
        for &i in idx {
            let state = &batch.states[i];
            globals.extend_from_slice(&state[..dg]);
            let n = batch.n_tiers[i];
            tiers.extend_from_slice(&state[dg..dg + n * dt]);
            tiers.resize(tiers.len() + (mt - n) * dt, 0.0);
            mask.extend((0..mt).map(|k| flag(k < n)));
        }
        globals.resize(b * dg, 0.0);
        tiers.resize(b * mt * dt, 0.0);
        mask.resize(b * mt, 0.0);
        let mut rows = label_rows(batch, idx, mt);
        rows.sens_target.resize(b * mt * N_COMPONENTS, 0.0);
        rows.sens_mask.resize(b * mt * N_COMPONENTS, 0.0);
        rows.action_target.resize(b * mt, 0.0);
        rows.action_mask.resize(b * mt, 0.0);
        rows.gain_target.resize(b, 0.0);
        rows.gain_mask.resize(b, 0.0);
        rows.sample_has_labels.resize(b, false);
        let dev = &self.device;
        PackedImitation {
            tiers: Tensor::<3>::from_data(TensorData::new(tiers, [b, mt, dt]), dev),
            globals: Tensor::<2>::from_data(TensorData::new(globals, [b, dg]), dev),
            mask: Tensor::<2>::from_data(TensorData::new(mask, [b, mt]), dev),
            labels: LabelTensors {
                sens_target: Tensor::<3>::from_data(
                    TensorData::new(rows.sens_target, [b, mt, N_COMPONENTS]),
                    dev,
                ),
                sens_mask: Tensor::<3>::from_data(
                    TensorData::new(rows.sens_mask, [b, mt, N_COMPONENTS]),
                    dev,
                ),
                action_target: Tensor::<2>::from_data(
                    TensorData::new(rows.action_target, [b, mt]),
                    dev,
                ),
                action_mask: Tensor::<2>::from_data(
                    TensorData::new(rows.action_mask, [b, mt]),
                    dev,
                ),
                gain_target: Tensor::<2>::from_data(TensorData::new(rows.gain_target, [b, 1]), dev),
                gain_mask: Tensor::<2>::from_data(TensorData::new(rows.gain_mask, [b, 1]), dev),
            },
            row_has_labels: rows.sample_has_labels,
            dummy_row: (b > idx.len()).then_some(idx.len()),
        }
    }

    /// One optimizer step over the samples `idx` of an already validated batch;
    /// [`MinibatchOutcome::Unlabelled`] when the mini-batch carries no label at all (no forward
    /// pass is run and the model is untouched).
    ///
    /// The gradient is clipped to the global `max_grad_norm` before the Adam step. A non-finite
    /// loss or gradient norm is [`MinibatchOutcome::NonFinite`]: the step is skipped before the
    /// optimizer runs, so neither the weights, the Adam moments nor the update counter move.
    fn imitation_update(
        &mut self,
        prepared: &PackedImitation,
        idx: &[usize],
        config: &ImitationConfig,
    ) -> Result<MinibatchOutcome, TrainError> {
        let gather = Instant::now();
        let packed = prepared.gather_rows(idx, &self.device);
        if !packed.row_has_labels.iter().any(|&has| has) {
            return Ok(MinibatchOutcome::Unlabelled);
        }
        let mut stages = ImitationStageDiagnostics {
            gather_ms: gather.elapsed().as_secs_f32() * 1000.0,
            max_minibatch_rows: packed.row_has_labels.len(),
            ..ImitationStageDiagnostics::default()
        };
        let forward = Instant::now();
        let out = self
            .model
            .forward(packed.tiers, packed.globals, packed.mask);
        let terms = imitation_terms(out, packed.labels, config);
        stages.forward_loss_submit_ms = forward.elapsed().as_secs_f32() * 1000.0;

        // One read-back for the five reported scalars.
        let readback = Instant::now();
        let figures = host_vec(Tensor::cat(
            vec![
                terms.total.clone(),
                terms.sens_mse,
                terms.cosine,
                terms.action_loss,
                terms.gain_loss,
            ],
            0,
        ))?;
        stages.loss_readback_ms = readback.elapsed().as_secs_f32() * 1000.0;
        let [loss, sens_mse, cosine, action_loss, gain_loss] = <[f32; 5]>::try_from(figures)
            .map_err(|v| {
                TrainError::Tensor(format!("expected 5 loss statistics, read {}", v.len()))
            })?;
        if ![loss, sens_mse, cosine, action_loss, gain_loss]
            .iter()
            .all(|v| v.is_finite())
        {
            return Ok(MinibatchOutcome::NonFinite);
        }

        let backward = Instant::now();
        let grads = GradientsParams::from_grads(terms.total.backward(), &self.model);
        let (grads, grad_norm) =
            match clip_global_grad_norm(grads, &self.model, config.max_grad_norm) {
                Ok(clipped) => clipped,
                Err(TrainError::NonFinite(_)) => return Ok(MinibatchOutcome::NonFinite),
                Err(error) => return Err(error),
            };
        let lr = config
            .lr_schedule
            .learning_rate(config.learning_rate, self.updates);
        self.model = self
            .optimizer
            .step(f64::from(lr), self.model.clone(), grads);
        self.updates += 1;
        stages.backward_optimizer_ms = backward.elapsed().as_secs_f32() * 1000.0;
        Ok(MinibatchOutcome::Stepped(UpdateStats {
            loss,
            sens_mse,
            cosine,
            action_loss,
            gain_loss,
            grad_norm,
            stages,
            ..UpdateStats::default()
        }))
    }

    /// Train every update in one bounded prepared window, accumulating the round metrics.
    fn train_prepared_window(
        &mut self,
        batch: &ImitationBatch<'_>,
        selected_rows: &[usize],
        local_batches: &[Vec<usize>],
        config: &ImitationConfig,
        sum: &mut UpdateStats,
        updates: &mut usize,
    ) -> Result<(), TrainError> {
        if selected_rows.is_empty() {
            return Ok(());
        }
        let packing_started = Instant::now();
        let needs_dummy = local_batches
            .iter()
            .any(|rows| !rows.len().is_power_of_two());
        let padded_rows = prepared_row_count(
            selected_rows.len(),
            needs_dummy,
            self.imitation_row_capacity(),
        );
        let prepared = self.pack_imitation_padded(batch, selected_rows, padded_rows);
        sum.packing_ms += packing_started.elapsed().as_secs_f64() as f32 * 1000.0;
        sum.stages.prepared_windows += 1;
        sum.stages.max_prepared_rows = sum.stages.max_prepared_rows.max(padded_rows);
        sum.stages.max_padded_tiers = sum.stages.max_padded_tiers.max(prepared.mask.dims()[1]);
        for rows in local_batches {
            let update_started = Instant::now();
            match self.imitation_update(&prepared, rows, config)? {
                MinibatchOutcome::Stepped(stats) => {
                    add_stats(sum, &stats);
                    *updates += 1;
                }
                MinibatchOutcome::Unlabelled => {}
                MinibatchOutcome::NonFinite => sum.skipped_minibatches += 1,
            }
            sum.update_ms += update_started.elapsed().as_secs_f64() as f32 * 1000.0;
        }
        Ok(())
    }

    /// Mini-batch imitation training over a labelled batch.
    ///
    /// Packs selected rows into bounded device windows, then gathers each mini-batch on-device.
    /// All requested updates and epochs are processed. Steps without labels are skipped. The same
    /// selected row may be packed again if a long schedule requires another bounded window.
    ///
    /// A labelled mini-batch whose loss or global gradient norm is not finite is skipped: no
    /// optimizer step, no Adam state change and no update-counter (learning-rate schedule)
    /// advance for it. It is counted in [`ImitationMetrics::skipped_minibatches`] and the
    /// remaining mini-batches train normally, so one bad row cannot abort a whole round.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] for an invalid `config`, [`TrainError::ShapeMismatch`] /
    /// [`TrainError::NonFinite`] for a malformed batch (see [`ImitationBatch::validate`];
    /// nothing is padded or guessed; nothing is trained then), [`TrainError::NonFinite`] when
    /// every labelled mini-batch was skipped as non-finite (no step was taken at all). A device
    /// or readback error ([`TrainError::Tensor`]) can surface after earlier mini-batches were
    /// applied; those steps stay applied on this trainer.
    pub fn train_imitation_epochs(
        &mut self,
        batch: &ImitationBatch<'_>,
        config: &ImitationConfig,
    ) -> Result<ImitationMetrics, TrainError> {
        let validation = Instant::now();
        config.validate()?;
        if batch.is_empty() {
            return Ok(ImitationMetrics::default());
        }
        batch.validate(self.model.d_global, self.model.d_tier, self.max_tiers)?;
        let total = batch.len();
        let mb_size = config.mini_batch_size.min(total);
        // Preserve the configured batch size even when a single batch itself exceeds the nominal
        // bound; otherwise memory windows are capped by available rows under the bound.
        let capacity = self.imitation_row_capacity();
        // A power-of-two upload bucket must fit the payload bound, including a sentinel when
        // a tail mini-batch needs padding. An oversized configured batch remains one batch.
        let bucket_capacity = if capacity.is_power_of_two() {
            capacity
        } else {
            capacity.next_power_of_two() / 2
        };
        let max_rows = bucket_capacity.saturating_sub(1).max(mb_size).max(1);
        let mut sum = UpdateStats::default();
        sum.stages.validation_ms = validation.elapsed().as_secs_f32() * 1000.0;
        let mut updates = 0usize;
        let mut selected_rows = Vec::<usize>::new();
        let mut selected_any = vec![false; total];
        let mut row_positions = HashMap::<usize, usize>::new();
        let mut local_batches = Vec::<Vec<usize>>::new();
        let mut add_batch = |this: &mut Self, rows: &[usize]| -> Result<(), TrainError> {
            for &row in rows {
                selected_any[row] = true;
            }
            let additional = rows
                .iter()
                .filter(|&&row| !row_positions.contains_key(&row))
                .count();
            if !local_batches.is_empty()
                && (selected_rows.len() + additional > max_rows
                    || local_batches.len() >= PREPARED_BATCHES_PER_WINDOW)
            {
                this.train_prepared_window(
                    batch,
                    &selected_rows,
                    &local_batches,
                    config,
                    &mut sum,
                    &mut updates,
                )?;
                selected_rows.clear();
                row_positions.clear();
                local_batches.clear();
            }
            let mut local = Vec::with_capacity(rows.len());
            for &row in rows {
                let position = *row_positions.entry(row).or_insert_with(|| {
                    let position = selected_rows.len();
                    selected_rows.push(row);
                    position
                });
                local.push(position);
            }
            local_batches.push(local);
            Ok(())
        };
        let mut order: Vec<usize> = (0..total).collect();
        if config.updates_per_round == 0 {
            for _ in 0..config.epochs_per_round {
                self.shuffle.shuffle(&mut order);
                for chunk in order.chunks(mb_size) {
                    add_batch(self, chunk)?;
                }
            }
        } else {
            for _ in 0..config.updates_per_round {
                draw_minibatch(&mut self.shuffle, &mut order, mb_size);
                add_batch(self, &order[..mb_size])?;
            }
        }
        drop(add_batch);
        self.train_prepared_window(
            batch,
            &selected_rows,
            &local_batches,
            config,
            &mut sum,
            &mut updates,
        )?;

        if updates == 0 && sum.skipped_minibatches > 0 {
            return Err(TrainError::NonFinite(format!(
                "all {} labelled imitation mini-batches had a non-finite loss or gradient norm; \
                 no optimizer step was taken",
                sum.skipped_minibatches
            )));
        }
        let selected_indices: Vec<usize> = selected_any
            .iter()
            .enumerate()
            .filter_map(|(index, &selected)| selected.then_some(index))
            .collect();
        let denom = updates.max(1) as f32;
        Ok(ImitationMetrics {
            loss: sum.loss / denom,
            sens_mse: sum.sens_mse / denom,
            cosine: sum.cosine / denom,
            action_loss: sum.action_loss / denom,
            action_samples: count_action_samples(batch, &selected_indices),
            gain_loss: sum.gain_loss / denom,
            grad_norm: sum.grad_norm / denom,
            labelled_tiers: count_labelled_tiers(batch, &selected_indices),
            component_support: count_component_support(batch, &selected_indices),
            updates,
            skipped_minibatches: sum.skipped_minibatches,
            packing_ms: sum.packing_ms,
            update_ms: sum.update_ms,
            stages: sum.stages,
            objective_revision: if updates > 0 {
                IMITATION_OBJECTIVE_REVISION
            } else {
                0
            },
        })
    }
}

#[cfg(test)]
#[path = "imitation_rework_tests.rs"]
mod rework_tests;
#[cfg(test)]
#[path = "imitation_tests.rs"]
mod tests;
