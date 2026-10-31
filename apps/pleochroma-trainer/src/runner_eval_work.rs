//! Share spare validation capacity without changing the ordered teacher or step results.

use pleochroma_ai::env::{
    ContinuousTierAction, GemstoneObservation, Step, TeacherLabel, TierProbe,
};
use rayon::prelude::*;

use super::{EnvKind, pool};

/// Probe in tier order until remaining cases leave spare workers, then share the suffix.
///
/// The caller obtains `tail` inside `run_items_ranked`, which installs the trainer's
/// persistent pool even for a single case. Nested Rayon work therefore never creates a
/// global pool. Indexed collection and finalization preserve the serial arithmetic order.
pub(super) fn oracle(
    env: &EnvKind,
    h_deg: f64,
    scales: &[f64],
    tail: &pool::Tail,
) -> TeacherLabel {
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
        EnvKind::Optimizer(env) => env.teacher_label_from_probes_adaptive(
            h_deg,
            &probes,
            scales,
            &|| tail.is_active(),
        ),
        EnvKind::Retarget(env) => env.teacher_label_from_probes_adaptive(
            h_deg,
            &probes,
            scales,
            &|| tail.is_active(),
        ),
    }
}

/// Share a line-search suffix only while validation leaves spare pool capacity.
///
/// The environment retains its original first-accepted ordering and sequential evaluation
/// count; speculative candidates cannot change the selected state or reported metrics.
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
        EnvKind::Optimizer(env) => {
            env.step_scaled_adaptive(action, scales, &|| tail.is_active())
        }
        EnvKind::Retarget(env) => {
            env.step_scaled_adaptive(action, scales, &|| tail.is_active())
        }
    }
}
