//! Share spare validation capacity without changing the ordered teacher or step results.

use pleochroma_ai::env::{
    ContinuousTierAction, GemstoneObservation, Step, TeacherLabel, TeacherSearchSnapshot, TierProbe,
};
use rayon::prelude::*;

use super::{EnvKind, pool};

/// Probe in tier order until remaining cases leave spare workers, then share the suffix.
///
/// `tail` comes from `run_items_ranked`, which runs even a single case in the trainer's
/// persistent pool, so nested Rayon work never creates the global pool. Indexed collection
/// keeps the serial arithmetic order.
pub(super) fn oracle(env: &EnvKind, h_deg: f64, scales: &[f64], tail: &pool::Tail) -> TeacherLabel {
    if pool::pool_threads() == 1 {
        return env.teacher_label(h_deg, scales);
    }
    let free = env.free_tiers();
    let mut probes = vec![TierProbe::default(); env.current_design().tiers.len()];
    for (position, &tier) in free.iter().enumerate() {
        if tail.is_active() && free.len() - position > 1 {
            let suffix: Vec<_> = free[position..]
                .par_iter()
                .map(|&index| env.probe_tier(index, h_deg))
                .collect();
            for (&index, probe) in free[position..].iter().zip(suffix) {
                probes[index] = probe;
            }
            break;
        }
        probes[tier] = env.probe_tier(tier, h_deg);
    }
    match env {
        EnvKind::Optimizer(env) => {
            env.teacher_label_from_probes_adaptive(h_deg, &probes, scales, &|| tail.is_active())
        }
        EnvKind::Retarget(env) => {
            env.teacher_label_from_probes_adaptive(h_deg, &probes, scales, &|| tail.is_active())
        }
    }
}

/// Teacher label with checkpointing: units already done in `saved` are restored, and each new
/// snapshot is passed to `checkpoint` before the search advances.
pub(super) fn oracle_with_recovery(
    env: &EnvKind,
    h_deg: f64,
    scales: &[f64],
    tail: &pool::Tail,
    saved: Option<TeacherSearchSnapshot>,
    checkpoint: &(dyn Fn(&TeacherSearchSnapshot) -> Result<(), String> + Sync),
) -> Result<TeacherLabel, String> {
    let parallel = || pool::pool_threads() > 1 && tail.is_active();
    match env {
        EnvKind::Optimizer(env) => {
            env.teacher_label_with_recovery_adaptive(h_deg, scales, saved, &parallel, checkpoint)
        }
        EnvKind::Retarget(env) => {
            env.teacher_label_with_recovery_adaptive(h_deg, scales, saved, &parallel, checkpoint)
        }
    }
}

/// One validation step. Scales run in order (a smaller scale only after a rejection), and
/// `tail` is checked before each unstarted scale, so spare workers only shorten the search.
pub(super) fn step(
    env: &mut EnvKind,
    action: &ContinuousTierAction,
    scales: &[f64],
    tail: &pool::Tail,
) -> Step<GemstoneObservation> {
    if pool::pool_threads() == 1 {
        return env.step_scaled(action, scales);
    }
    match env {
        EnvKind::Optimizer(env) => env.step_scaled_adaptive(action, scales, &|| tail.is_active()),
        EnvKind::Retarget(env) => env.step_scaled_adaptive(action, scales, &|| tail.is_active()),
    }
}
