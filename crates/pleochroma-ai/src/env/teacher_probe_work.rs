//! Two-sided teacher probes with owned optical continuations.
use super::{ContinuousTierAction, EnvCore, TierProbe, on_pool_worker};
use crate::env::{OptimizerEnv, RetargetEnv, common::DesignEval, stepping::CandidateWork};

/// Prepared positive and negative probes of one unchanged environment state.
///
/// CPU preparation submits each side immediately. Retain at most the scheduler's fixed
/// work limit, poll outside its CPU pool, then complete against the original environment.
/// Dropping this value cancels both outstanding optical requests.
pub struct TierProbeWork {
    tier: usize,
    positive: Option<ProbeSide>,
    negative: Option<ProbeSide>,
}

struct ProbeSide {
    raw: f32,
    delta: f64,
    work: CandidateWork,
}

impl TierProbeWork {
    /// Retry queue admission and inspect both replies without blocking a CPU worker.
    pub fn poll(&mut self) -> bool {
        let positive = self.positive.as_mut().is_none_or(|side| side.work.poll());
        let negative = self.negative.as_mut().is_none_or(|side| side.work.poll());
        positive && negative
    }
}

impl EnvCore {
    /// Both sides are always prepared, so a pool worker shares them whatever `parallel`
    /// says ([`on_pool_worker`]); off a pool only `parallel` shares them.
    fn prepare_tier_probe_with(&self, tier: usize, h_deg: f64, parallel: bool) -> TierProbeWork {
        let mut work = TierProbeWork {
            tier,
            positive: None,
            negative: None,
        };
        if !(h_deg.is_finite() && h_deg > 0.0) || !self.policy_coordinate(tier).mobile {
            return work;
        }
        if parallel || on_pool_worker() {
            (work.positive, work.negative) = rayon::join(
                || self.prepare_probe_side(tier, h_deg),
                || self.prepare_probe_side(tier, -h_deg),
            );
        } else {
            work.positive = self.prepare_probe_side(tier, h_deg);
            work.negative = self.prepare_probe_side(tier, -h_deg);
        }
        work
    }

    fn prepare_probe_side(&self, tier: usize, requested: f64) -> Option<ProbeSide> {
        let (raw, delta) = self.canonical_probe(tier, requested);
        if delta == 0.0 {
            return None;
        }
        let mut deltas = vec![0.0; self.current_design.tiers.len()];
        deltas[tier] = delta;
        Some(ProbeSide {
            raw,
            delta,
            work: self.prepare_candidate(&ContinuousTierAction::new(deltas), 1.0),
        })
    }

    fn complete_probe_side(
        &self,
        side: Option<ProbeSide>,
    ) -> (f32, Option<(DesignEval, f64)>, usize) {
        let Some(side) = side else {
            return (0.0, None, 0);
        };
        let outcome = self.complete_candidate(side.work);
        let usable = outcome.girdle_gate && outcome.manufacturing_gate;
        let sample = outcome
            .scored
            .and_then(|(_, eval)| usable.then_some((eval, side.delta.abs())));
        (side.raw, sample, usize::from(outcome.evaluated))
    }

    fn complete_tier_probe(&self, work: TierProbeWork) -> TierProbe {
        let (positive_raw, positive, positive_count) = self.complete_probe_side(work.positive);
        let (negative_raw, negative, negative_count) = self.complete_probe_side(work.negative);
        let (sensitivity, best_action) =
            self.reduce_tier_samples(work.tier, positive_raw, negative_raw, positive, negative);
        TierProbe {
            sensitivity,
            best_action,
            evaluations: positive_count + negative_count,
        }
    }
}

impl OptimizerEnv {
    /// Prepare both probe sides without waiting for optical queue space or completion.
    #[must_use]
    pub fn prepare_tier_probe(&self, tier: usize, h_deg: f64) -> TierProbeWork {
        self.prepare_tier_probe_with(tier, h_deg, false)
    }

    /// Prepare both sides in the installed Rayon pool: always on a pool worker, otherwise
    /// when requested. Results do not depend on where the sides ran.
    #[must_use]
    pub fn prepare_tier_probe_with(
        &self,
        tier: usize,
        h_deg: f64,
        parallel: bool,
    ) -> TierProbeWork {
        self.core.prepare_tier_probe_with(tier, h_deg, parallel)
    }

    /// Assemble ready sides in positive/negative order against this unchanged state.
    #[must_use]
    pub fn complete_tier_probe(&self, work: TierProbeWork) -> TierProbe {
        self.core.complete_tier_probe(work)
    }
}

impl RetargetEnv {
    /// Prepare both probe sides without waiting for optical queue space or completion.
    #[must_use]
    pub fn prepare_tier_probe(&self, tier: usize, h_deg: f64) -> TierProbeWork {
        self.prepare_tier_probe_with(tier, h_deg, false)
    }

    /// Prepare both sides in the installed Rayon pool: always on a pool worker, otherwise
    /// when requested. Results do not depend on where the sides ran.
    #[must_use]
    pub fn prepare_tier_probe_with(
        &self,
        tier: usize,
        h_deg: f64,
        parallel: bool,
    ) -> TierProbeWork {
        self.core.prepare_tier_probe_with(tier, h_deg, parallel)
    }

    /// Assemble ready sides in positive/negative order against this unchanged state.
    #[must_use]
    pub fn complete_tier_probe(&self, work: TierProbeWork) -> TierProbe {
        self.core.complete_tier_probe(work)
    }
}

#[cfg(test)]
#[path = "teacher_probe_work_tests.rs"]
mod tests;
