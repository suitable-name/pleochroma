//! Canonically ordered teacher candidates separated by nonblocking optical waits.

use super::{EnvCore, TeacherLabel, TierProbe, Winner, finish_label, offer};
use crate::env::{stepping::CandidateWork, teacher::action_from_direction};
use rayon::prelude::*;

/// A teacher finalizer tied to the unchanged state that produced its probes.
///
/// The coordinator polls readiness; workers advance geometry and score assembly.
/// No environment state changes while this work is in flight.
pub struct TeacherLabelWork {
    label: TeacherLabel,
    best: Option<Winner>,
    gradient: Vec<Winner>,
    actions: Vec<Winner>,
    probes: Vec<TierProbe>,
    h_deg: f64,
    index: usize,
    pending: Vec<CandidateWork>,
    rescue: bool,
    search_enabled: bool,
    done: bool,
}

impl TeacherLabelWork {
    /// Whether a worker can advance without waiting for optical results.
    pub fn poll(&mut self) -> bool {
        self.pending
            .iter_mut()
            .fold(true, |ready, work| work.poll() & ready)
    }

    pub(in crate::env) fn complete(self) -> TeacherLabel {
        assert!(self.done, "teacher work must finish before completion");
        if self.search_enabled {
            finish_label(self.label, self.best)
        } else {
            self.label
        }
    }
}

impl EnvCore {
    pub(in crate::env) fn prepare_teacher_label(
        &self,
        h_deg: f64,
        probes: &[TierProbe],
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> TeacherLabelWork {
        let (label, best, gradient, search_enabled) =
            self.teacher_search_seed(h_deg, probes, scales);
        let mut work = TeacherLabelWork {
            label,
            best,
            actions: gradient.clone(),
            gradient,
            probes: probes.to_vec(),
            h_deg,
            index: 0,
            pending: Vec::new(),
            rescue: false,
            search_enabled,
            done: !search_enabled,
        };
        self.prepare_next_teacher_candidate(&mut work, parallel);
        work
    }

    pub(in crate::env) fn advance_teacher_label(
        &self,
        work: &mut TeacherLabelWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        if work.done {
            return true;
        }
        if !work.poll() {
            return false;
        }
        for pending in std::mem::take(&mut work.pending) {
            let outcome = self.complete_candidate(pending);
            work.label.evaluations += usize::from(outcome.evaluated);
            if outcome.is_accepted() {
                let action = &work.actions[work.index];
                offer(
                    &mut work.best,
                    Winner {
                        raw: action.raw.clone(),
                        gain: self.current_eval.score
                            - outcome.score().expect("accepted candidate has a score"),
                        scale: action.scale,
                    },
                );
            }
            work.index += 1;
        }
        self.prepare_next_teacher_candidate(work, parallel);
        work.done
    }

    fn prepare_next_teacher_candidate(
        &self,
        work: &mut TeacherLabelWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) {
        if work.done {
            return;
        }
        if work.index == work.actions.len() {
            if !work.rescue && work.best.is_none() {
                work.actions = self.rescue_candidates(work.h_deg, &work.probes, &work.gradient);
                work.index = 0;
                work.rescue = true;
            } else {
                work.done = true;
                return;
            }
        }
        let remaining = &work.actions[work.index..];
        let prepare = |action: &Winner| {
            self.prepare_candidate(
                &action_from_direction(&action.raw, self.settings.max_step_deg),
                1.0,
            )
        };
        if parallel() && remaining.len() > 1 {
            work.pending = remaining.par_iter().map(prepare).collect();
        } else if let Some(action) = remaining.first() {
            work.pending = vec![prepare(action)];
        } else {
            work.done = true;
        }
    }
}
