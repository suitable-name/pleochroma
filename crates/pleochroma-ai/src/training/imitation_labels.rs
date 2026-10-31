//! Pack only verified policy actions; component derivatives remain independent targets.

use super::*;

/// Host-side label rows padded to the prepared window's real maximum tier count.
#[derive(Debug, Default)]
pub(super) struct LabelRows {
    /// Flattened `[sample][tier][component]` targets and masks.
    pub(super) sens_target: Vec<f32>,
    pub(super) sens_mask: Vec<f32>,
    /// Flattened `[sample][tier]` squashed, verified action fractions and masks.
    pub(super) action_target: Vec<f32>,
    pub(super) action_mask: Vec<f32>,
    /// One entry per sample.
    pub(super) gain_target: Vec<f32>,
    pub(super) gain_mask: Vec<f32>,
    pub(super) sample_has_labels: Vec<bool>,
}

/// Pack component slopes independently of verified policy actions.
///
/// A missing action row never falls back to score slopes: the local gradient can point into an
/// infeasible region or cancel on a symmetric improving valley. Verified zero coordinates are
/// supervised along with the moving coordinates. Cosine's zero-norm guard lives in the loss.
pub(super) fn label_rows(batch: &ImitationBatch<'_>, idx: &[usize], max_tiers: usize) -> LabelRows {
    let mut rows = LabelRows::default();
    for &i in idx {
        let mut has_labels = false;
        for k in 0..max_tiers {
            let usable = is_actionable_tier(batch, i, k);
            for label in component_labels(&batch.sensitivities[i], k, usable) {
                rows.sens_target.push(label.unwrap_or(0.0));
                rows.sens_mask.push(flag(label.is_some()));
                has_labels |= label.is_some();
            }
            let action = batch.action_targets[i]
                .get(k)
                .copied()
                .filter(|v| usable && v.is_finite());
            rows.action_target.push(action.map_or(0.0, f32::tanh));
            rows.action_mask.push(flag(action.is_some()));
            has_labels |= action.is_some();
        }
        let gain = Some(batch.oracle_gain[i]).filter(|g| g.is_finite());
        rows.gain_target.push(gain.unwrap_or(0.0));
        rows.gain_mask.push(flag(gain.is_some()));
        rows.sample_has_labels.push(has_labels || gain.is_some());
    }
    rows
}
