use super::*;

/// Masked mean squared error of the sensitivities,
/// `sum(mask * (hat - target)^2) / max(sum mask, 1)`, shape `[1]`; exactly `0` when the mask is
/// empty.
pub(super) fn masked_sens_mse(hat: Tensor<3>, target: Tensor<3>, mask: Tensor<3>) -> Tensor<1> {
    let diff = hat - target;
    let n_labels = mask.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    (diff.clone() * diff * mask).sum() / n_labels
}

/// Robust masked Huber regression for noisy finite-difference component labels.
/// The raw masked MSE remains a separately reported diagnostic.
pub(super) fn masked_sens_huber(hat: Tensor<3>, target: Tensor<3>, mask: Tensor<3>) -> Tensor<1> {
    let n_labels = mask.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    let abs_error = ((hat - target) * mask).abs();
    let quadratic = abs_error.clone().clamp(0.0, HUBER_DELTA);
    let linear = abs_error - quadratic.clone();
    ((quadratic.clone() * quadratic).mul_scalar(0.5) + linear.mul_scalar(HUBER_DELTA)).sum()
        / n_labels
}

/// Masked cosine between the predicted and the target direction, per sample, over the tiers of
/// `mask` (all `[batch, max_tiers]`). Returns `(cosine, cos_loss)`, both `[1]`: the mean cosine
/// over the samples that have at least one masked tier, and `1 - cosine` over the same samples.
/// A sample without any masked tier contributes `0` to both and is left out of the divisor, so a
/// batch without any direction label gives exactly `(0, 0)`.
pub(super) fn masked_cosine(
    hat: Tensor<2>,
    target: Tensor<2>,
    mask: Tensor<2>,
) -> (Tensor<1>, Tensor<1>) {
    let hat = hat * mask.clone();
    let target = target * mask.clone();
    let dot = (hat.clone() * target.clone()).sum_dim(1);
    let target_norm_sq = (target.clone() * target.clone()).sum_dim(1);
    let has_direction =
        mask.sum_dim(1).clamp(0.0, 1.0) * target_norm_sq.clone().greater_elem(NORM_SQ_EPS).float();
    let norm_hat = (hat.clone() * hat)
        .sum_dim(1)
        .add_scalar(NORM_SQ_EPS)
        .sqrt();
    let norm_target = target_norm_sq.sqrt();
    let cosine = dot / (norm_hat * norm_target).add_scalar(COSINE_EPS);
    let n_samples = has_direction.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    let cosine_sum = (cosine * has_direction.clone()).sum();
    let loss = (has_direction.sum() - cosine_sum.clone()) / n_samples.clone();
    (cosine_sum / n_samples, loss)
}

/// Masked Huber regression averaged over labelled entries.
///
/// Predictions have shape `[batch, values]`; the `[1]` loss is exactly zero for an empty mask.
pub(super) fn masked_huber(hat: Tensor<2>, target: Tensor<2>, mask: Tensor<2>) -> Tensor<1> {
    let n_labels = mask.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    let abs_error = ((hat - target) * mask).abs();
    // Quadratic up to the knee, linear beyond it: `0.5 q^2 + delta (a - q)`, `q = min(a, delta)`.
    let quadratic = abs_error.clone().clamp(0.0, HUBER_DELTA);
    let linear = abs_error - quadratic.clone();
    ((quadratic.clone() * quadratic).mul_scalar(0.5) + linear.mul_scalar(HUBER_DELTA)).sum()
        / n_labels
}

/// Give each verified move equal weight, and split its weight equally between moving and
/// stationary coordinates when both exist.
///
/// Pooling all coordinates would dilute a sparse move
/// by the number of stationary tiers and give larger designs more weight. Tiny nonzero targets
/// retain their physical amplitude: neither this loss nor its gradient normalizes them to one.
pub(super) fn balanced_action_huber(
    hat: Tensor<2>,
    target: Tensor<2>,
    mask: Tensor<2>,
) -> Tensor<1> {
    let moving = target.clone().abs().greater_elem(0.0).float() * mask.clone();
    let stationary = mask.clone() - moving.clone();
    let abs_error = ((hat - target) * mask.clone()).abs();
    let quadratic = abs_error.clone().clamp(0.0, HUBER_DELTA);
    let linear = abs_error - quadratic.clone();
    let error = (quadratic.clone() * quadratic).mul_scalar(0.5) + linear.mul_scalar(HUBER_DELTA);
    let moving_count = moving.clone().sum_dim(1);
    let stationary_count = stationary.clone().sum_dim(1);
    let moving_loss =
        (error.clone() * moving).sum_dim(1) / moving_count.clone().clamp(1.0, COUNT_CLAMP_MAX);
    let stationary_loss =
        (error * stationary).sum_dim(1) / stationary_count.clone().clamp(1.0, COUNT_CLAMP_MAX);
    let groups = moving_count.clamp(0.0, 1.0) + stationary_count.clamp(0.0, 1.0);
    let per_sample = (moving_loss + stationary_loss) / groups.clamp(1.0, 2.0);
    let samples = mask.sum_dim(1).clamp(0.0, 1.0).sum();
    per_sample.sum() / samples.clamp(1.0, COUNT_CLAMP_MAX)
}

/// Bounded vector discrepancy for direction supervision, averaged over nonzero verified moves.
///
/// `||a-y||^2 / max(1, ||a||^2 + ||y||^2)` lies in `[0, 2]`, is zero at the verified move,
/// and has a finite gradient at a zero prediction. The denominator is differentiated as well;
/// for a positive rescaling `a = k*y`, the unique minimum remains `k = 1` (no scale ambiguity).
/// The unit floor uses action-fraction units;
/// a tiny teacher move produces a tiny squared error rather than a unit direction/large cosine
/// gradient. Raw cosine remains an independent diagnostic, not the optimized objective.
pub(super) fn bounded_direction_loss(
    hat: Tensor<2>,
    target: Tensor<2>,
    mask: Tensor<2>,
) -> Tensor<1> {
    let hat = hat * mask.clone();
    let target = target * mask;
    let target_norm_sq = (target.clone() * target.clone()).sum_dim(1);
    let labelled = target_norm_sq.clone().greater_elem(0.0).float();
    let norm_sum = (hat.clone() * hat.clone()).sum_dim(1) + target_norm_sq;
    let error = hat - target;
    let per_sample = (error.clone() * error).sum_dim(1) / norm_sum.clamp(1.0, COUNT_CLAMP_MAX);
    let samples = labelled.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    (per_sample * labelled).sum() / samples
}

/// The device-side loss terms of one mini-batch, each of shape `[1]`.
pub(super) struct LossTerms {
    pub(super) total: Tensor<1>,
    pub(super) sens_mse: Tensor<1>,
    pub(super) cosine: Tensor<1>,
    pub(super) action_loss: Tensor<1>,
    pub(super) gain_loss: Tensor<1>,
}

/// Combine component regression with verified-action, direction and gain supervision.
///
/// Policy targets live after `tanh`, where the environment interprets their relative magnitude.
/// This avoids fitting arbitrarily large raw means near an action bound.
pub(super) fn imitation_terms(
    out: BurnForward,
    labels: LabelTensors,
    config: &ImitationConfig,
) -> LossTerms {
    let sens_huber = masked_sens_huber(
        out.sensitivities.clone(),
        labels.sens_target.clone(),
        labels.sens_mask.clone(),
    );
    let sens_mse = masked_sens_mse(out.sensitivities, labels.sens_target, labels.sens_mask);
    let action = out.mean.tanh();
    let (cosine, _) = masked_cosine(
        action.clone(),
        labels.action_target.clone(),
        labels.action_mask.clone(),
    );
    let direction_loss = bounded_direction_loss(
        action.clone(),
        labels.action_target.clone(),
        labels.action_mask.clone(),
    );
    let action_loss = balanced_action_huber(action, labels.action_target, labels.action_mask);
    let gain_loss = masked_huber(out.gain, labels.gain_target, labels.gain_mask);
    let total = sens_huber
        + direction_loss.mul_scalar(config.cosine_coeff)
        + action_loss.clone().mul_scalar(config.action_coeff)
        + gain_loss.clone().mul_scalar(config.gain_coeff);
    LossTerms {
        total,
        sens_mse,
        cosine,
        action_loss,
        gain_loss,
    }
}
