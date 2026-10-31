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
    let (cosine, cos_loss) = masked_cosine(
        action.clone(),
        labels.action_target.clone(),
        labels.action_mask.clone(),
    );
    let action_loss = masked_huber(action, labels.action_target, labels.action_mask);
    let gain_loss = masked_huber(out.gain, labels.gain_target, labels.gain_mask);
    let total = sens_huber
        + cos_loss.mul_scalar(config.cosine_coeff)
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
