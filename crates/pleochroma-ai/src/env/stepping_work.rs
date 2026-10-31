//! Candidate preparation and completion around an owned optical submission.
use super::{CandidateOutcome, EnvCore, scaled};
use crate::env::{ContinuousTierAction, common::apply_action, evaluation::PreparedEvaluation};
use pleochroma_cut_core::Design;

pub(in crate::env) enum CandidateWork {
    Ready(Box<CandidateOutcome>),
    Pending(Box<(Design, PreparedEvaluation)>),
}

impl CandidateWork {
    fn ready(outcome: CandidateOutcome) -> Self {
        Self::Ready(Box::new(outcome))
    }

    /// Retry bounded submission and inspect completion without waiting.
    pub(in crate::env) fn poll(&mut self) -> bool {
        match self {
            Self::Ready(_) => true,
            Self::Pending(pending) => pending.1.poll(),
        }
    }
}

impl EnvCore {
    pub(in crate::env) fn prepare_candidate(
        &self,
        action: &ContinuousTierAction,
        scale: f64,
    ) -> CandidateWork {
        let design = match apply_action(
            &self.space,
            &self.current_design,
            &scaled(action, scale),
            self.settings.max_step_deg,
        ) {
            Ok(design) => design,
            Err(reason) => return CandidateWork::ready(CandidateOutcome::refused(reason, false)),
        };
        if design == self.current_design {
            return CandidateWork::ready(self.judge_candidate(
                design,
                self.current_eval.clone(),
                true,
            ));
        }
        match self.prepare_candidate_design(&design) {
            Ok(evaluation) => CandidateWork::Pending(Box::new((design, evaluation))),
            Err(error) => CandidateWork::ready(CandidateOutcome::refused(error.rejection(), true)),
        }
    }

    pub(in crate::env) fn complete_candidate(&self, work: CandidateWork) -> CandidateOutcome {
        match work {
            CandidateWork::Ready(outcome) => *outcome,
            CandidateWork::Pending(pending) => {
                let (design, evaluation) = *pending;
                match evaluation.complete() {
                    Ok(eval) => self.judge_candidate(design, eval, false),
                    Err(error) => CandidateOutcome::refused(error.rejection(), true),
                }
            }
        }
    }
}
