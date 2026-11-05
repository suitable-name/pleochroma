//! Ordered line-search continuation with no optical wait on a CPU worker.
//!
//! Scales are prepared one at a time: a smaller scale is projected and scored only after
//! the larger one was rejected. Speculating on later scales would spend projections on the
//! CPU that bounds training throughput, and the first accepted scale discards them.

use super::{CandidateOutcome, CandidateWork, EnvCore, Search, usable_scales};
use crate::env::{ContinuousTierAction, common::CoreStep};

/// An unfinished step tied to the unchanged environment that prepared it.
///
/// Poll on the coordinator, advance on a CPU worker, then commit exactly once.
/// Dropping it abandons the step without changing the environment.
pub struct StepWork {
    action: ContinuousTierAction,
    scales: Vec<f64>,
    next: usize,
    pending: Option<CandidateWork>,
    first: Option<CandidateOutcome>,
    search: Option<Search>,
    evaluations: usize,
    last_components: Option<pleochroma_cut_core::ObjectiveComponents>,
}

impl StepWork {
    /// Whether a worker can advance this step without waiting for optical results.
    pub fn poll(&mut self) -> bool {
        self.pending.as_mut().is_none_or(CandidateWork::poll)
    }
}

impl EnvCore {
    /// Prepare the first usable scale (the unit scale for an empty list).
    ///
    /// `_parallel` is kept for the staged API; the line search never speculates.
    pub(in crate::env) fn prepare_step_scaled(
        &self,
        action: &ContinuousTierAction,
        scales: &[f64],
        _parallel: &(dyn Fn() -> bool + Sync),
    ) -> StepWork {
        let mut scales: Vec<_> = usable_scales(scales).collect();
        if scales.is_empty() {
            scales.push(1.0);
        }
        let mut work = StepWork {
            action: action.clone(),
            scales,
            next: 0,
            pending: None,
            first: None,
            search: None,
            evaluations: 0,
            last_components: None,
        };
        self.prepare_step_candidate(&mut work);
        work
    }

    /// Complete the ready scale; prepare the next one only after a rejection.
    ///
    /// A next scale that is ready at once (refused before scoring, an identity, or answered
    /// while this worker prepared it) is completed in the same call.
    pub(in crate::env) fn advance_step(
        &self,
        work: &mut StepWork,
        _parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        while work.search.is_none() {
            if !work.poll() {
                return false;
            }
            let pending = work
                .pending
                .take()
                .expect("an unfinished step has a prepared scale");
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
            } else {
                if work.first.is_none() {
                    work.first = Some(outcome);
                }
                work.next += 1;
                if work.next < work.scales.len() {
                    self.prepare_step_candidate(work);
                } else {
                    work.search = Some(Search {
                        outcome: work.first.take().expect("nonempty scale list"),
                        accepted_scale: None,
                        evaluations: work.evaluations,
                        last_components: work.last_components,
                    });
                }
            }
        }
        true
    }

    fn prepare_step_candidate(&self, work: &mut StepWork) {
        work.pending = Some(self.prepare_candidate(&work.action, work.scales[work.next]));
    }

    pub(in crate::env) fn complete_step(&mut self, mut work: StepWork) -> CoreStep {
        self.finish_search(work.search.take().expect("step must finish before commit"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{DEFAULT_LINE_SEARCH, test_fixtures::fixture_env_with};

    #[test]
    fn staged_line_search_prepares_a_smaller_scale_only_after_a_rejection() {
        let never = || -> bool { panic!("the line search never consults spare capacity") };
        let mut staged = fixture_env_with(|config| config.max_step_deg = 4.0);
        let mut reference = fixture_env_with(|config| config.max_step_deg = 4.0);
        let tiers = staged.current_design().tiers.len();
        for magnitude in [0.0, 0.4, -0.4, 4.0, -4.0] {
            let action = ContinuousTierAction::new(vec![magnitude; tiers]);
            let mut work =
                staged.prepare_step_scaled_adaptive(&action, &DEFAULT_LINE_SEARCH, &never);
            assert_eq!(work.next, 0);
            assert!(work.pending.is_some());
            assert!(
                staged.advance_step_adaptive(&mut work, &never),
                "CPU fixture scales are ready at once"
            );
            assert!(
                work.pending.is_none(),
                "no scale after the decision is prepared"
            );
            match work
                .search
                .as_ref()
                .expect("finished search")
                .accepted_scale
            {
                Some(scale) => assert_eq!(scale, DEFAULT_LINE_SEARCH[work.next]),
                None => assert_eq!(work.next, DEFAULT_LINE_SEARCH.len()),
            }
            let got = staged.complete_step(work);
            let want = reference.step_scaled_adaptive(&action, &DEFAULT_LINE_SEARCH, &never);
            assert_eq!(got, want);
            assert_eq!(staged.current_design(), reference.current_design());
        }
    }
}
