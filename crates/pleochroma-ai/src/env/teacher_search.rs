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
        let (mut label, mut best, actions, search_enabled) =
            self.teacher_search_seed(h_deg, probes, scales);
        if !search_enabled {
            return label;
        }
        label.evaluations += self.search_raw_actions(&actions, parallel, &mut best);
        if best.is_none() {
            label.evaluations += self.rescue_actions(h_deg, probes, &actions, parallel, &mut best);
        }
        finish_label(label, best)
    }

    fn teacher_search_seed(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        scales: &[f64],
    ) -> (TeacherLabel, Option<Winner>, Vec<Winner>, bool) {
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
                false,
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
            return (label, None, Vec::new(), false);
        }
        let mut best = None;
        for (tier, probe) in probes.iter().enumerate() {
            if self.policy_coordinate(tier).mobile
                && let Some(measured) = probe.best_action.filter(|action| action.tier == tier)
                && let Some(winner) = coordinate_winner(measured, count)
            {
                offer(&mut best, winner);
            }
        }
        let slopes = score_slopes(&label.tiers);
        let actions = self.gradient_actions(h_deg, &slopes, &scales);
        (label, best, actions, true)
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
                    let requested = f64::from(value.tanh()) * self.settings.max_step_deg * scale;
                    self.canonical_probe(tier, requested).0
                })
                .collect();
            if raw.iter().any(|&value| value != 0.0)
                && !self.is_original_probe(&raw, h_deg)
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

    /// A single-coordinate candidate already measured by the independent probes.
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

    fn evaluate_raw(&self, action: &Winner) -> (Option<Winner>, usize) {
        let outcome = self.try_candidate(
            &action_from_direction(&action.raw, self.settings.max_step_deg),
            1.0,
        );
        let winner = outcome.is_accepted().then(|| Winner {
            raw: action.raw.clone(),
            gain: self.current_eval.score
                - outcome.score().expect("accepted candidate has a score"),
            scale: action.scale,
        });
        (winner, usize::from(outcome.evaluated))
    }

    /// Evaluate every distinct canonical gradient scale.
    ///
    /// Every listed action is evaluated whatever the others return, so a pool worker always
    /// shares the list ([`on_pool_worker`]); off a pool the remaining suffix is shared once
    /// `parallel` reports spare capacity. Reduction stays in list order; unlike rollout
    /// acceptance the teacher keeps the best.
    fn search_raw_actions(
        &self,
        actions: &[Winner],
        parallel: &(dyn Fn() -> bool + Sync),
        best: &mut Option<Winner>,
    ) -> usize {
        let mut evaluations = 0;
        for (index, action) in actions.iter().enumerate() {
            if actions.len() - index > 1 && (on_pool_worker() || parallel()) {
                let outcomes: Vec<_> = actions[index..]
                    .par_iter()
                    .map(|candidate| self.evaluate_raw(candidate))
                    .collect();
                for (winner, spent) in outcomes {
                    evaluations += spent;
                    if let Some(winner) = winner {
                        offer(best, winner);
                    }
                }
                break;
            }
            let (winner, spent) = self.evaluate_raw(action);
            evaluations += spent;
            if let Some(winner) = winner {
                offer(best, winner);
            }
        }
        evaluations
    }

    /// Bounded escape search, not an exhaustive coordinate or joint optimizer.
    ///
    /// Prefer the largest measured slopes with stable tier ties. Up to six coordinate probes
    /// retain smaller/larger radii; two paired directions cover some coupled escapes.
    /// The total cap includes invalid projections and is not an exhaustive pair search.
    fn rescue_actions(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        measured: &[Winner],
        parallel: &(dyn Fn() -> bool + Sync),
        best: &mut Option<Winner>,
    ) -> usize {
        let candidates = self.rescue_candidates(h_deg, probes, measured);
        self.search_raw_actions(&candidates, parallel, best)
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
        // Every gradient action was evaluated in this unchanged environment. Exact raw
        // equality means evaluate_raw would execute the same action at unit scale again.
        // Filter after the original cap, without refilling: search coverage and tie order
        // stay the same, and only repeated failed projections/scorings disappear.
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
