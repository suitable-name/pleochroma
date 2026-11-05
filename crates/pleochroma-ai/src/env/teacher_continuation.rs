//! Canonically ordered teacher candidates separated by nonblocking optical waits.

use super::{EnvCore, TeacherLabel, TierProbe, Winner, finish_label, offer, on_pool_worker};
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
        self.prepare_teacher_phase(&mut work, parallel);
        work
    }

    /// Complete every ready candidate in list order and prepare the next phase.
    ///
    /// A phase whose candidates are all ready at once (refused before scoring, or answered
    /// while this worker prepared them) is completed in the same call.
    pub(in crate::env) fn advance_teacher_label(
        &self,
        work: &mut TeacherLabelWork,
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> bool {
        while !work.done {
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
            self.prepare_teacher_phase(work, parallel);
        }
        true
    }

    /// Prepare every remaining candidate of the current phase at once.
    ///
    /// Every gradient action is always evaluated, and every rescue action once rescue
    /// starts (no winner after the gradient phase), so a phase holds no speculative work.
    /// Its projections fan out on the pool ([`on_pool_worker`], or `parallel` off a pool)
    /// and its optical requests wait together; completion keeps list order.
    fn prepare_teacher_phase(
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
        if remaining.is_empty() {
            work.done = true;
            return;
        }
        let prepare = |action: &Winner| {
            self.prepare_candidate(
                &action_from_direction(&action.raw, self.settings.max_step_deg),
                1.0,
            )
        };
        work.pending = if remaining.len() > 1 && (on_pool_worker() || parallel()) {
            remaining.par_iter().map(prepare).collect()
        } else {
            remaining.iter().map(prepare).collect()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{DEFAULT_LINE_SEARCH, test_fixtures::fixture_env};

    #[test]
    fn pool_workers_prepare_each_phase_at_once_and_keep_the_serial_label() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("test pool");
        let env = fixture_env();
        let count = env.current_design().tiers.len();
        let probes: Vec<_> = (0..count).map(|tier| env.probe_tier(tier, 0.25)).collect();
        for scales in [&DEFAULT_LINE_SEARCH[..], &[][..]] {
            let expected = env.teacher_label_from_probes(0.25, &probes, scales);
            // Spare capacity is never reported; pool workers fan out regardless.
            let actual = pool.install(|| {
                let mut work = env
                    .core
                    .prepare_teacher_label(0.25, &probes, scales, &|| false);
                assert_eq!(work.pending.len(), work.actions.len() - work.index);
                while !env.core.advance_teacher_label(&mut work, &|| false) {
                    assert!(
                        work.poll(),
                        "CPU fixture has no asynchronous optical request"
                    );
                }
                work.complete()
            });
            assert_eq!(
                serde_json::to_value(actual).expect("staged label"),
                serde_json::to_value(&expected).expect("serial label")
            );
        }
    }
}
