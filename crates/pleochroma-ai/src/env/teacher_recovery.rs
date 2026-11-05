//! Portable checkpoints of completed teacher work, without device handles or pending kernels.

use std::sync::Mutex;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::{EnvCore, TeacherLabel, TierProbe, Winner, finish_label, offer, valid_coordinate};
use crate::env::{CUDA_ENVIRONMENT_MATH, OptimizerEnv, RetargetEnv};

/// Identity of the canonical probe, gradient and bounded-rescue search saved by recovery.
pub const TEACHER_SEARCH_RECOVERY_REVISION: &str = "canonical-teacher-search-v1";

const SNAPSHOT_VERSION: u32 = 1;
type Checkpoint<'a> = dyn Fn(&TeacherSearchSnapshot) -> Result<(), String> + Sync + 'a;

/// Completed work from one teacher search against one unchanged environment state.
///
/// Save using a serializer that preserves floating-point NaNs (for example MessagePack):
/// absent optical components legitimately have NaN sensitivities. A missing probe is distinct
/// from a completed probe whose candidates were both refused. Completed candidate outcomes
/// retain their evaluation counts and winners; reduction always follows canonical list order.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherSearchSnapshot {
    version: u32,
    identity: String,
    h_bits: u64,
    scale_bits: Vec<u64>,
    free_tiers: Vec<usize>,
    probes: Vec<Option<TierProbe>>,
    phase: Phase,
    gradient: Vec<Option<CandidateResult>>,
    rescue: Vec<Option<CandidateResult>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Probes,
    Gradient,
    Rescue,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateResult {
    winner: Option<Winner>,
    evaluations: usize,
}

impl TeacherSearchSnapshot {
    /// Number of completed probes and search candidates, including physical refusals.
    #[must_use]
    pub fn completed_units(&self) -> usize {
        self.probes.iter().flatten().count()
            + self.gradient.iter().flatten().count()
            + self.rescue.iter().flatten().count()
    }

    fn new(core: &EnvCore, h_deg: f64, scales: &[f64]) -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            identity: identity(core),
            h_bits: h_deg.to_bits(),
            scale_bits: scales.iter().map(|scale| scale.to_bits()).collect(),
            free_tiers: core.space.free().to_vec(),
            probes: vec![None; core.current_design.tiers.len()],
            phase: Phase::Probes,
            gradient: Vec::new(),
            rescue: Vec::new(),
        }
    }

    fn probes(&self) -> Vec<TierProbe> {
        self.probes
            .iter()
            .map(|probe| probe.unwrap_or_default())
            .collect()
    }

    fn results(&self, phase: Phase) -> &[Option<CandidateResult>] {
        match phase {
            Phase::Gradient => &self.gradient,
            Phase::Rescue => &self.rescue,
            _ => unreachable!("only candidate phases have results"),
        }
    }

    fn results_mut(&mut self, phase: Phase) -> &mut [Option<CandidateResult>] {
        match phase {
            Phase::Gradient => &mut self.gradient,
            Phase::Rescue => &mut self.rescue,
            _ => unreachable!("only candidate phases have results"),
        }
    }
}

// These are the inputs consumed by candidate geometry, scoring and acceptance. Full values,
// rather than a hash alone, reject a changed design/material/context on restore. Debug floats
// round-trip their finite value; absent-component NaNs are not candidate inputs. Deliberately
// omit live accelerator counters, caches, pointers and worker scheduling.
fn identity(core: &EnvCore) -> String {
    let math = if core.execution.is_some() {
        CUDA_ENVIRONMENT_MATH
    } else {
        "cpu-env-v1"
    };
    format!(
        "{TEACHER_SEARCH_RECOVERY_REVISION}\n{math}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{}",
        core.base_design,
        core.current_design,
        core.settings,
        core.gem,
        core.reward,
        core.base_eval,
        core.current_eval,
        core.preform_volume,
        core.step_count,
    )
}

fn invalid(detail: &str) -> String {
    format!("teacher search recovery: {detail}")
}

/// Completed work shared by parallel units, and the newest snapshot the checkpoint has seen.
///
/// A unit records its result under `state` only long enough to update the snapshot; the
/// checkpoint runs under `published`, outside that lock, on a copy taken while holding it, so a
/// slow durable write never stalls completions. A unit returns once a snapshot containing its
/// result was handed over. The writer always hands over the newest state, so an older
/// snapshot never follows a newer one, and results arriving during a write are coalesced into
/// the next one instead of costing one write each.
struct SharedSearch<'s> {
    state: Mutex<(&'s mut TeacherSearchSnapshot, u64)>,
    published: Mutex<u64>,
}

impl<'s> SharedSearch<'s> {
    const fn new(state: &'s mut TeacherSearchSnapshot) -> Self {
        Self {
            state: Mutex::new((state, 0)),
            published: Mutex::new(0),
        }
    }

    fn record(
        &self,
        apply: impl FnOnce(&mut TeacherSearchSnapshot),
        checkpoint: &Checkpoint<'_>,
    ) -> Result<(), String> {
        let sequence = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("checkpoint lock poisoned"))?;
            apply(&mut *state.0);
            state.1 += 1;
            state.1
        };
        let mut published = self
            .published
            .lock()
            .map_err(|_| invalid("checkpoint lock poisoned"))?;
        if *published >= sequence {
            return Ok(());
        }
        let (snapshot, newest) = {
            let state = self
                .state
                .lock()
                .map_err(|_| invalid("checkpoint lock poisoned"))?;
            (TeacherSearchSnapshot::clone(&*state.0), state.1)
        };
        // A failed write aborts this evaluation; the in-memory result stays recorded.
        checkpoint(&snapshot)?;
        *published = newest;
        Ok(())
    }
}

fn health(core: &EnvCore) -> Result<(), String> {
    if let Some(execution) = &core.execution {
        execution.check()?;
    }
    Ok(())
}

fn reduce_candidates(
    results: &[Option<CandidateResult>],
    label: &mut TeacherLabel,
    best: &mut Option<Winner>,
) {
    for result in results.iter().flatten() {
        label.evaluations += result.evaluations;
        if let Some(winner) = &result.winner {
            offer(best, winner.clone());
        }
    }
}

fn validate_candidates(results: &[Option<CandidateResult>], actions: &[Winner]) -> bool {
    results.len() == actions.len()
        && results.iter().zip(actions).all(|(result, action)| {
            result.as_ref().is_none_or(|result| {
                result.evaluations <= 1
                    && result.winner.as_ref().is_none_or(|winner| {
                        result.evaluations == 1
                            && winner.raw == action.raw
                            && winner.scale.to_bits() == action.scale.to_bits()
                            && winner.gain.is_finite()
                            && winner.gain > 0.0
                    })
            })
        })
}

impl EnvCore {
    fn validate_teacher_snapshot(
        &self,
        saved: &TeacherSearchSnapshot,
        h_deg: f64,
        scales: &[f64],
    ) -> Result<(), String> {
        let count = self.current_design.tiers.len();
        if saved.version != SNAPSHOT_VERSION
            || saved.identity != identity(self)
            || saved.h_bits != h_deg.to_bits()
            || saved.scale_bits
                != scales
                    .iter()
                    .map(|scale| scale.to_bits())
                    .collect::<Vec<_>>()
            || saved.free_tiers.as_slice() != self.space.free()
            || saved.probes.len() != count
        {
            return Err(invalid(
                "state, search settings, backend or version changed",
            ));
        }
        for (tier, probe) in saved.probes.iter().enumerate() {
            if let Some(probe) = probe {
                if !saved.free_tiers.contains(&tier)
                    || probe.evaluations > 2
                    || probe.sensitivity.is_some_and(|s| {
                        !s.score.is_finite() || s.components.iter().any(|value| value.is_infinite())
                    })
                    || probe.best_action.is_some_and(|action| {
                        action.tier != tier
                            || !valid_coordinate(action, count)
                            || !self.policy_coordinate(tier).mobile
                            || probe.evaluations == 0
                    })
                {
                    return Err(invalid("malformed completed tier probe"));
                }
            }
        }
        if saved.phase == Phase::Probes {
            return if saved.gradient.is_empty() && saved.rescue.is_empty() {
                Ok(())
            } else {
                Err(invalid("search results precede completed probes"))
            };
        }
        if saved
            .free_tiers
            .iter()
            .any(|&tier| saved.probes[tier].is_none())
        {
            return Err(invalid("search phase has unfinished probes"));
        }
        let probes = saved.probes();
        let (mut label, mut best, gradient, enabled) =
            self.teacher_search_seed(h_deg, &probes, scales);
        if !enabled {
            return if saved.phase == Phase::Complete
                && saved.gradient.is_empty()
                && saved.rescue.is_empty()
            {
                Ok(())
            } else {
                Err(invalid("disabled search has candidate work"))
            };
        }
        if !validate_candidates(&saved.gradient, &gradient) {
            return Err(invalid("malformed gradient results"));
        }
        if saved.phase == Phase::Gradient {
            return if saved.rescue.is_empty() {
                Ok(())
            } else {
                Err(invalid("premature rescue results"))
            };
        }
        if saved.gradient.iter().any(Option::is_none) {
            return Err(invalid("unfinished gradient before rescue/completion"));
        }
        reduce_candidates(&saved.gradient, &mut label, &mut best);
        if best.is_some() {
            if saved.phase != Phase::Complete || !saved.rescue.is_empty() {
                return Err(invalid("rescue after an improving gradient/coordinate"));
            }
        } else {
            let rescue = self.rescue_candidates(h_deg, &probes, &gradient);
            if !validate_candidates(&saved.rescue, &rescue)
                || (saved.phase == Phase::Complete && saved.rescue.iter().any(Option::is_none))
            {
                return Err(invalid("malformed or unfinished rescue results"));
            }
        }
        Ok(())
    }

    fn recoverable_teacher(
        &self,
        h_deg: f64,
        scales: &[f64],
        saved: Option<TeacherSearchSnapshot>,
        parallel: &(dyn Fn() -> bool + Sync),
        checkpoint: &Checkpoint<'_>,
    ) -> Result<TeacherLabel, String> {
        health(self)?;
        let mut state = match saved {
            Some(saved) => {
                self.validate_teacher_snapshot(&saved, h_deg, scales)?;
                saved
            }
            None => TeacherSearchSnapshot::new(self, h_deg, scales),
        };
        if state.phase == Phase::Probes {
            self.recoverable_probes(&mut state, h_deg, parallel, checkpoint)?;
        }
        let probes = state.probes();
        let (mut label, mut best, gradient, enabled) =
            self.teacher_search_seed(h_deg, &probes, scales);
        if !enabled {
            if state.phase != Phase::Complete {
                state.phase = Phase::Complete;
                checkpoint(&state)?;
            }
            return Ok(label);
        }
        if state.phase == Phase::Probes {
            state.phase = Phase::Gradient;
            state.gradient = vec![None; gradient.len()];
            checkpoint(&state)?;
        }
        if state.phase == Phase::Gradient {
            self.recoverable_candidates(
                &mut state,
                &gradient,
                Phase::Gradient,
                parallel,
                checkpoint,
            )?;
        }
        reduce_candidates(&state.gradient, &mut label, &mut best);
        if best.is_none() {
            let rescue = self.rescue_candidates(h_deg, &probes, &gradient);
            if state.phase == Phase::Gradient {
                state.phase = Phase::Rescue;
                state.rescue = vec![None; rescue.len()];
                checkpoint(&state)?;
            }
            if state.phase == Phase::Rescue {
                self.recoverable_candidates(
                    &mut state,
                    &rescue,
                    Phase::Rescue,
                    parallel,
                    checkpoint,
                )?;
            }
            reduce_candidates(&state.rescue, &mut label, &mut best);
        }
        health(self)?;
        if state.phase != Phase::Complete {
            state.phase = Phase::Complete;
            checkpoint(&state)?;
        }
        Ok(finish_label(label, best))
    }

    fn recoverable_probes(
        &self,
        state: &mut TeacherSearchSnapshot,
        h_deg: f64,
        parallel: &(dyn Fn() -> bool + Sync),
        checkpoint: &Checkpoint<'_>,
    ) -> Result<(), String> {
        let pending: Vec<_> = state
            .free_tiers
            .iter()
            .copied()
            .filter(|&tier| state.probes[tier].is_none())
            .collect();
        let shared = SharedSearch::new(state);
        run_pending(&pending, parallel, |tier| {
            health(self)?;
            let probe = self.probe_tier(tier, h_deg);
            health(self)?;
            shared.record(|state| state.probes[tier] = Some(probe), checkpoint)
        })
    }

    fn recoverable_candidates(
        &self,
        state: &mut TeacherSearchSnapshot,
        actions: &[Winner],
        phase: Phase,
        parallel: &(dyn Fn() -> bool + Sync),
        checkpoint: &Checkpoint<'_>,
    ) -> Result<(), String> {
        let pending: Vec<_> = state
            .results(phase)
            .iter()
            .enumerate()
            .filter_map(|(index, result)| result.is_none().then_some(index))
            .collect();
        let shared = SharedSearch::new(state);
        run_pending(&pending, parallel, |index| {
            health(self)?;
            let (winner, evaluations) = self.evaluate_raw(&actions[index]);
            health(self)?;
            shared.record(
                |state| {
                    state.results_mut(phase)[index] = Some(CandidateResult {
                        winner,
                        evaluations,
                    });
                },
                checkpoint,
            )
        })
    }
}

fn run_pending(
    pending: &[usize],
    parallel: &(dyn Fn() -> bool + Sync),
    run: impl Fn(usize) -> Result<(), String> + Sync,
) -> Result<(), String> {
    for (position, &index) in pending.iter().enumerate() {
        if parallel() && pending.len() - position > 1 {
            return pending[position..]
                .par_iter()
                .try_for_each(|&index| run(index));
        }
        run(index)?;
    }
    Ok(())
}

macro_rules! recoverable_teacher {
    ($kind:ty) => {
        impl $kind {
            /// Continue a teacher search, handing each completed probe/candidate to `checkpoint`.
            ///
            /// The environment must remain unchanged. The callback must commit atomically and
            /// must not reenter this search. It runs serially, outside the lock that guards the
            /// search state, and always receives the newest snapshot; results completed while
            /// it runs are coalesced into its next call. Every snapshot holds all completed
            /// work, so the callback may skip a durable write at the price of recomputation.
            /// Ready work may finish out of order, but winners and counts are reduced in the
            /// original canonical order. In-flight work is recomputed after interruption.
            ///
            /// # Errors
            /// Returns an error for a changed/malformed snapshot, checkpoint I/O or accelerator failure.
            pub fn teacher_label_with_recovery_adaptive(
                &self,
                h_deg: f64,
                scales: &[f64],
                saved: Option<TeacherSearchSnapshot>,
                parallel: &(dyn Fn() -> bool + Sync),
                checkpoint: &(dyn Fn(&TeacherSearchSnapshot) -> Result<(), String> + Sync),
            ) -> Result<TeacherLabel, String> {
                self.core
                    .recoverable_teacher(h_deg, scales, saved, parallel, checkpoint)
            }
        }
    };
}

recoverable_teacher!(OptimizerEnv);
recoverable_teacher!(RetargetEnv);

#[cfg(test)]
#[path = "teacher_recovery_tests.rs"]
mod tests;
