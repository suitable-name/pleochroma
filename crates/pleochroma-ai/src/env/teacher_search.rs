//! Verified behavior targets alongside independent finite-difference sensitivities.

use super::{
    CoordinateProbe, EnvCore, MEAN_BOUND, TeacherLabel, TierProbe, action_from_direction,
    assemble_label, direction_from_slopes, on_pool_worker, score_slopes, usable_scales,
};
use rayon::prelude::*;

#[path = "teacher_continuation.rs"]
mod continuation;
pub use continuation::TeacherLabelWork;
#[path = "teacher_recovery.rs"]
mod recovery;
pub use recovery::{TEACHER_SEARCH_RECOVERY_REVISION, TeacherSearchSnapshot};

/// Additional candidate attempts per label, including refused projections.
const RESCUE_ATTEMPTS: usize = 8;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Winner {
    raw: Vec<f32>,
    gain: f32,
    scale: f32,
}

/// Canonical finite action input.
///
/// Clamping is intentional: these labels train executable
/// actions rather than asking tanh to reach an infinite pre-squash endpoint.
pub(super) fn raw_for_fraction(fraction: f64) -> f32 {
    if !fraction.is_finite() {
        return 0.0;
    }
    let limit = MEAN_BOUND.tanh();
    (fraction as f32)
        .clamp(-limit, limit)
        .atanh()
        .clamp(-MEAN_BOUND, MEAN_BOUND)
}

fn offer(best: &mut Option<Winner>, candidate: Winner) {
    if candidate.gain.is_finite()
        && candidate.gain > 0.0
        && best.as_ref().is_none_or(|old| candidate.gain > old.gain)
    {
        *best = Some(candidate);
    }
}

fn valid_coordinate(probe: CoordinateProbe, tiers: usize) -> bool {
    probe.tier < tiers
        && probe.raw.is_finite()
        && probe.raw != 0.0
        && probe.raw.abs() <= MEAN_BOUND
        && probe.gain.is_finite()
        && probe.gain > 0.0
}

fn coordinate_winner(probe: CoordinateProbe, tiers: usize) -> Option<Winner> {
    if !valid_coordinate(probe, tiers) {
        return None;
    }
    let mut raw = vec![0.0; tiers];
    raw[probe.tier] = probe.raw;
    Some(Winner {
        raw,
        gain: probe.gain,
        scale: 1.0,
    })
}

impl EnvCore {
    /// Complete a label from probes of this exact, unchanged environment state.
    ///
    /// Coordinate improvements survive central-slope cancellation and a rejected joint
    /// gradient direction. Every behavior target was evaluated as the same finite raw action
    /// the learner can execute. Sensitivity labels retain their measured secants.
    pub(crate) fn teacher_label_from_probes_adaptive(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> TeacherLabel {
        let (mut label, mut best, actions, execution_scale) =
            self.teacher_search_seed(h_deg, probes, scales);
        let Some(execution_scale) = execution_scale else {
            return label;
        };
        label.evaluations +=
            self.search_raw_actions(&actions, execution_scale, parallel, &mut best);
        if best.is_none() {
            label.evaluations += self.rescue_actions(
                h_deg,
                probes,
                &actions,
                execution_scale,
                parallel,
                &mut best,
            );
        }
        finish_label(label, best)
    }

    fn teacher_search_seed(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        scales: &[f64],
    ) -> (TeacherLabel, Option<Winner>, Vec<Winner>, Option<f64>) {
        let count = self.current_design.tiers.len();
        if probes.len() != count
            || !h_deg.is_finite()
            || h_deg <= 0.0
            || probes.iter().enumerate().any(|(tier, probe)| {
                probe.sensitivity.is_some_and(|value| {
                    !value.score.is_finite()
                        || value.components.iter().any(|value| value.is_infinite())
                }) || probe.best_action.is_some_and(|value| {
                    value.tier != tier
                        || !valid_coordinate(value, count)
                        || !self.policy_coordinate(tier).mobile
                })
            })
        {
            return (
                assemble_label(vec![None; count], 0, super::OracleOutcome::default()),
                None,
                Vec::new(),
                None,
            );
        }
        let tiers = probes.iter().map(|probe| probe.sensitivity).collect();
        let label = assemble_label(
            tiers,
            probes.iter().map(|probe| probe.evaluations).sum(),
            super::OracleOutcome::default(),
        );
        let scales: Vec<_> = usable_scales(scales).collect();
        if scales.is_empty()
            || !self
                .space
                .free()
                .iter()
                .any(|&tier| self.policy_coordinate(tier).mobile)
        {
            return (label, None, Vec::new(), None);
        }
        let execution_scale = scales[0];
        let mut best = None;
        let mut actions = Vec::new();
        for (tier, probe) in probes.iter().enumerate() {
            if self.policy_coordinate(tier).mobile
                && let Some(measured) = probe.best_action.filter(|action| action.tier == tier)
                && let Some(mut winner) = coordinate_winner(measured, count)
            {
                if execution_scale == 1.0 {
                    offer(&mut best, winner);
                } else {
                    // A probe measures a unit-scale action. Invert the rollout's first scale,
                    // clip to a representable command and verify the resulting physical move.
                    let requested = f64::from(measured.raw.tanh()) * self.settings.max_step_deg;
                    winner.raw[tier] = self.canonical_probe(tier, requested / execution_scale).0;
                    if winner.raw[tier] != 0.0 {
                        winner.gain = 0.0;
                        actions.push(winner);
                    }
                }
            }
        }
        let slopes = score_slopes(&label.tiers);
        for gradient in self.gradient_actions(h_deg, &slopes, &scales) {
            if !actions.iter().any(|old| old.raw == gradient.raw) {
                actions.push(gradient);
            }
        }
        (label, best, actions, Some(execution_scale))
    }

    fn gradient_actions(&self, h_deg: f64, slopes: &[Option<f32>], scales: &[f64]) -> Vec<Winner> {
        let slopes: Vec<_> = slopes
            .iter()
            .enumerate()
            .map(|(tier, &slope)| {
                self.policy_coordinate(tier)
                    .mobile
                    .then_some(slope)
                    .flatten()
            })
            .collect();
        let direction = direction_from_slopes(&slopes);
        let mut actions: Vec<Winner> = Vec::new();
        for &scale in scales {
            let raw: Vec<_> = direction
                .iter()
                .enumerate()
                .map(|(tier, &value)| {
                    if !self.policy_coordinate(tier).mobile {
                        return 0.0;
                    }
                    let requested =
                        f64::from(value.tanh()) * self.settings.max_step_deg * (scale / scales[0]);
                    self.canonical_probe(tier, requested).0
                })
                .collect();
            if raw.iter().any(|&value| value != 0.0)
                && (scales[0] != 1.0 || !self.is_original_probe(&raw, h_deg))
                && !actions.iter().any(|old| old.raw == raw)
            {
                actions.push(Winner {
                    raw,
                    gain: 0.0,
                    scale: scale.min(f64::from(f32::MAX)) as f32,
                });
            }
        }
        actions
    }

    /// Whether `raw` is a single-coordinate action at a probe's own `±h_deg` step.
    fn is_original_probe(&self, raw: &[f32], h_deg: f64) -> bool {
        let mut nonzero = raw.iter().enumerate().filter(|(_, value)| **value != 0.0);
        let Some((tier, &value)) = nonzero.next() else {
            return false;
        };
        nonzero.next().is_none()
            && [h_deg, -h_deg]
                .into_iter()
                .any(|h| value == self.canonical_probe(tier, h).0)
    }

    fn evaluate_raw(&self, action: &Winner, execution_scale: f64) -> (Option<Winner>, usize) {
        let outcome = self.try_candidate(
            &action_from_direction(&action.raw, self.settings.max_step_deg),
            execution_scale,
        );
        let winner = outcome.is_accepted().then(|| Winner {
            raw: action.raw.clone(),
            gain: self.current_eval.score
                - outcome.score().expect("accepted candidate has a score"),
            scale: action.scale,
        });
        (winner, usize::from(outcome.evaluated))
    }

    /// Evaluate the actions in list order and keep the best winner.
    ///
    /// Every listed action is evaluated whatever the others return, so a pool worker shares
    /// the whole list ([`on_pool_worker`]). Off a pool, the remaining suffix is shared once
    /// `parallel` reports spare capacity. Reduction stays in list order; unlike a rollout step,
    /// which takes the first accepted scale, the teacher keeps the largest gain.
    fn search_raw_actions(
        &self,
        actions: &[Winner],
        execution_scale: f64,
        parallel: &(dyn Fn() -> bool + Sync),
        best: &mut Option<Winner>,
    ) -> usize {
        let mut evaluations = 0;
        for (index, action) in actions.iter().enumerate() {
            if actions.len() - index > 1 && (on_pool_worker() || parallel()) {
                let outcomes: Vec<_> = actions[index..]
                    .par_iter()
                    .map(|candidate| self.evaluate_raw(candidate, execution_scale))
                    .collect();
                for (winner, spent) in outcomes {
                    evaluations += spent;
                    if let Some(winner) = winner {
                        offer(best, winner);
                    }
                }
                break;
            }
            let (winner, spent) = self.evaluate_raw(action, execution_scale);
            evaluations += spent;
            if let Some(winner) = winner {
                offer(best, winner);
            }
        }
        evaluations
    }

    /// Bounded escape search, not an exhaustive coordinate or joint optimizer.
    ///
    /// The two tiers with the largest measured slopes (ties by index) get up to six coordinate
    /// probes at two radii. Two paired directions cover some coupled escapes. Actions already
    /// measured are dropped after the cap, without refilling it.
    fn rescue_actions(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        measured: &[Winner],
        execution_scale: f64,
        parallel: &(dyn Fn() -> bool + Sync),
        best: &mut Option<Winner>,
    ) -> usize {
        let candidates = self.rescue_candidates(h_deg, probes, measured);
        self.search_raw_actions(&candidates, execution_scale, parallel, best)
    }

    fn rescue_candidates(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        measured: &[Winner],
    ) -> Vec<Winner> {
        let mut tiers: Vec<_> = self
            .space
            .free()
            .iter()
            .copied()
            .filter(|&tier| self.policy_coordinate(tier).mobile)
            .collect();
        let strength = |tier: usize| {
            probes[tier]
                .sensitivity
                .map_or(0.0, |sensitivity| sensitivity.score.abs())
        };
        tiers.sort_by(|&a, &b| strength(b).total_cmp(&strength(a)).then_with(|| a.cmp(&b)));
        let mut candidates = Vec::new();
        let maximum = f64::from(MEAN_BOUND.tanh()) * self.settings.max_step_deg;
        let nominal = h_deg.min(maximum);
        let second = if nominal < maximum {
            maximum
        } else {
            nominal * 0.25
        };
        for &tier in tiers.iter().take(2) {
            for radius in [nominal * 0.5, second] {
                for sign in [1.0, -1.0] {
                    let (raw, _) = self.canonical_probe(tier, radius * sign);
                    if raw == 0.0
                        || raw == self.canonical_probe(tier, h_deg).0
                        || raw == self.canonical_probe(tier, -h_deg).0
                    {
                        continue;
                    }
                    let mut action = vec![0.0; probes.len()];
                    action[tier] = raw;
                    if !candidates.iter().any(|old: &Winner| old.raw == action) {
                        candidates.push(Winner {
                            raw: action,
                            gain: 0.0,
                            scale: 1.0,
                        });
                    }
                }
            }
        }
        candidates.truncate(RESCUE_ATTEMPTS - 2);
        if tiers.len() >= 2 {
            let preferred = |tier: usize| {
                let sign = if probes[tier].sensitivity.is_some_and(|s| s.score > 0.0) {
                    -1.0
                } else {
                    1.0
                };
                if self.canonical_probe(tier, nominal * sign).0 == 0.0 {
                    -sign
                } else {
                    sign
                }
            };
            // Probe same/preferred and opposed directions. The two other quadrants and
            // other tier pairs remain outside this explicitly bounded search.
            for orientation in [1.0, -1.0] {
                let mut action = vec![0.0; probes.len()];
                action[tiers[0]] = self
                    .canonical_probe(tiers[0], nominal * preferred(tiers[0]))
                    .0;
                action[tiers[1]] = self
                    .canonical_probe(tiers[1], nominal * preferred(tiers[1]) * orientation)
                    .0;
                if action.iter().filter(|&&raw| raw != 0.0).count() == 2
                    && !candidates.iter().any(|old| old.raw == action)
                {
                    candidates.push(Winner {
                        raw: action,
                        gain: 0.0,
                        scale: 1.0,
                    });
                }
            }
        }
        // Gradient actions were already evaluated in this unchanged environment, and an equal
        // raw action would be executed again at the same scale. Filter after the cap, without
        // refilling, so search coverage and tie order stay the same.
        candidates.retain(|candidate| !measured.iter().any(|old| old.raw == candidate.raw));
        candidates
    }
}

fn finish_label(mut label: TeacherLabel, best: Option<Winner>) -> TeacherLabel {
    label.oracle_gain = Some(best.as_ref().map_or(0.0, |winner| winner.gain));
    if let Some(winner) = best {
        label.oracle_scale = Some(winner.scale);
        label.action_target = Some(winner.raw);
    }
    label
}

#[cfg(test)]
#[path = "teacher_search_tests.rs"]
mod tests;
