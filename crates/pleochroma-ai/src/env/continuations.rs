//! Public staged work for dependency-aware training coordinators.

use super::{
    ContinuousTierAction, GemstoneObservation, OptimizerEnv, RetargetEnv, Step, StepWork,
    TeacherLabel, TeacherLabelWork, TierProbe, common::CoreStep,
};

fn with_observation(
    outcome: CoreStep,
    observation: GemstoneObservation,
) -> Step<GemstoneObservation> {
    Step {
        observation,
        reward: outcome.reward,
        terminated: outcome.terminated,
        truncated: outcome.truncated,
        info: outcome.info,
    }
}

macro_rules! continuations {
    ($kind:ty) => {
        impl $kind {
            /// Prepare the first teacher-search candidate without waiting for CUDA optics.
            ///
            /// Use the exact state that produced `probes`; keep it unchanged until complete.
            pub fn prepare_teacher_label(
                &self,
                h_deg: f64,
                probes: &[TierProbe],
                scales: &[f64],
            ) -> TeacherLabelWork {
                self.prepare_teacher_label_adaptive(h_deg, probes, scales, &|| false)
            }

            /// Prepare teacher candidates, sharing spare capacity in the installed Rayon pool.
            pub fn prepare_teacher_label_adaptive(
                &self,
                h_deg: f64,
                probes: &[TierProbe],
                scales: &[f64],
                parallel: &(dyn Fn() -> bool + Sync),
            ) -> TeacherLabelWork {
                self.core
                    .prepare_teacher_label(h_deg, probes, scales, parallel)
            }

            /// Assemble a ready result and prepare the next teacher candidate on a worker.
            ///
            /// Returns true only when the entire finalizer is complete. Otherwise poll the
            /// work again before dispatching the next advance. Keep this environment unchanged.
            pub fn advance_teacher_label(&self, work: &mut TeacherLabelWork) -> bool {
                self.advance_teacher_label_adaptive(work, &|| false)
            }

            /// Advance ready teacher results and optionally prepare the remaining search in parallel.
            pub fn advance_teacher_label_adaptive(
                &self,
                work: &mut TeacherLabelWork,
                parallel: &(dyn Fn() -> bool + Sync),
            ) -> bool {
                self.core.advance_teacher_label(work, parallel)
            }

            /// Consume a completed finalizer from this unchanged environment.
            ///
            /// # Panics
            /// Panics unless `advance_teacher_label` has returned true.
            pub fn complete_teacher_label(&self, work: TeacherLabelWork) -> TeacherLabel {
                work.complete()
            }

            /// Prepare the first line-search candidate without waiting for CUDA optics.
            ///
            /// Keep this environment unchanged until committing the completed work.
            pub fn prepare_step_scaled(
                &self,
                action: &ContinuousTierAction,
                scales: &[f64],
            ) -> StepWork {
                self.prepare_step_scaled_adaptive(action, scales, &|| false)
            }

            /// Prepare a step, optionally sharing spare CPU capacity for remaining scales.
            pub fn prepare_step_scaled_adaptive(
                &self,
                action: &ContinuousTierAction,
                scales: &[f64],
                parallel: &(dyn Fn() -> bool + Sync),
            ) -> StepWork {
                self.core.prepare_step_scaled(action, scales, parallel)
            }

            /// Assemble a ready result and prepare the next scale on a CPU worker.
            ///
            /// Returns true only after the first accepted scale or the last rejection.
            /// Otherwise poll the work before dispatching the next advance.
            pub fn advance_step(&self, work: &mut StepWork) -> bool {
                self.advance_step_adaptive(work, &|| false)
            }

            /// Advance a ready step and recheck spare capacity before unstarted scales.
            pub fn advance_step_adaptive(
                &self,
                work: &mut StepWork,
                parallel: &(dyn Fn() -> bool + Sync),
            ) -> bool {
                self.core.advance_step(work, parallel)
            }

            /// Commit the completed step and build its resulting observation exactly once.
            ///
            /// # Panics
            /// Panics unless `advance_step` has returned true. This must be the same
            /// unchanged environment that prepared the work.
            pub fn complete_step(&mut self, work: StepWork) -> Step<GemstoneObservation> {
                let outcome = self.core.complete_step(work);
                with_observation(outcome, self.build_observation())
            }
        }
    };
}

continuations!(OptimizerEnv);
continuations!(RetargetEnv);

#[cfg(test)]
#[path = "continuations_tests.rs"]
mod tests;
