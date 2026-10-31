//! Ordered line-search continuation with no optical wait on a CPU worker.

use super::{CandidateOutcome, CandidateWork, EnvCore, Search, usable_scales};
use crate::env::{ContinuousTierAction, common::CoreStep};
use rayon::prelude::*;

/// An unfinished step tied to the unchanged environment that prepared it.
///
/// Poll on the coordinator, advance on a CPU worker, then commit exactly once.
/// Dropping it abandons the step without changing the environment.
pub struct StepWork {
    action: ContinuousTierAction,
    scales: Vec<f64>,
    next: usize,
    pending: Vec<CandidateWork>,
    first: Option<CandidateOutcome>,
    search: Option<Search>,
    evaluations: usize,
    last_components: Option<pleochroma_cut_core::ObjectiveComponents>,
}

impl StepWork {
    /// Whether a worker can advance this step without waiting for optical results.
    pub fn poll(&mut self) -> bool {
        self.pending
            .iter_mut()
            .fold(true, |ready, work| work.poll() & ready)
    }
}

impl EnvCore {
    pub(in crate::env) fn prepare_step_scaled(
        &self,
        action: &ContinuousTierAction,
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> StepWork {
        let mut scales: Vec<_> = usable_scales(scales).collect();
        if scales.is_empty() {
            scales.push(1.0);
        }
        let mut work = StepWork {
            action: action.clone(),
            scales,
            next: 0,
            pending: Vec::new(),
            first: None,
            search: None,
            evaluations: 0,
            last_components: None,
        };
        self.prepare_step_candidates(&mut work, parallel);
        work
    }

    pub(in crate::env) fn advance_step(
        &self,
        work: &mut StepWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        if work.search.is_some() {
            return true;
        }
        if !work.poll() {
            return false;
        }
        for pending in std::mem::take(&mut work.pending) {
            let outcome = self.complete_candidate(pending);
            work.evaluations += usize::from(outcome.evaluated);
            work.last_components = outcome.components().or(work.last_components);
            if outcome.is_accepted() {
                work.search = Some(Search {
                    outcome,
                    accepted_scale: Some(work.scales[work.next]),
                    evaluations: work.evaluations,
                    last_components: work.last_components,
                });
                return true;
            }
            if work.first.is_none() {
                work.first = Some(outcome);
            }
            work.next += 1;
        }
        if work.next < work.scales.len() {
            self.prepare_step_candidates(work, parallel);
            false
        } else {
            work.search = Some(Search {
                outcome: work.first.take().expect("nonempty scale list"),
                accepted_scale: None,
                evaluations: work.evaluations,
                last_components: work.last_components,
            });
            true
        }
    }

    fn prepare_step_candidates(&self, work: &mut StepWork, parallel: &(dyn Fn() -> bool + Sync)) {
        let rest = &work.scales[work.next..];
        work.pending = if parallel() && rest.len() > 1 {
            rest.par_iter()
                .map(|&scale| self.prepare_candidate(&work.action, scale))
                .collect()
        } else {
            vec![self.prepare_candidate(&work.action, rest[0])]
        };
    }

    pub(in crate::env) fn complete_step(&mut self, mut work: StepWork) -> CoreStep {
        self.finish_search(work.search.take().expect("step must finish before commit"))
    }
}
