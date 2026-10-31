//! The two-tier sweep's worker thread: starting it, and looking at it from the UI thread.
//!
//! The same shape as [`super::run`] for one tier: the sweep
//! ([`pleochroma_editor::sweep::sweep_two_tier_angles`]) runs on one thread of its own, which
//! fans out over scoped worker threads, and a timer on the UI thread polls the
//! [`RunHandle2d`] for the progress and, at the end, the outcome.

use crate::gui::editor::material_lookup::{EditorMaterialLookup, resolved_gem_material_for_design};
use pleochroma::optics::{LightingPreset, materials::GemMaterial};
use pleochroma_cut_core::{
    Design, MaterialSelection,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use pleochroma_editor::sweep::{
    Sweep2dOutcome, Sweep2dPlan, SweepOptions, SweepScene, sweep_two_tier_angles,
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Everything a two-tier sweep reads, owned so it can move to the worker thread.
pub(super) struct Job2d {
    /// The design as it is now; every angle pair is solved on a clone of it.
    pub design: Design,
    pub plan: Sweep2dPlan,
    /// The design's material (with its refractive index filled in when it names none).
    pub material: MaterialSelection,
    /// The custom catalogue materials the selection may name.
    pub custom_materials: Vec<GemMaterial>,
    /// The lighting the figures are scored under (the Grading tray).
    pub lighting: LightingPreset,
    pub options: SweepOptions,
}

/// What a look at a running two-tier sweep finds.
#[derive(Debug)]
pub(super) enum Poll2d {
    /// Still going: how many cells are done.
    Running { done: usize },
    /// Over, finished or stopped.
    Finished(Box<Sweep2dOutcome>),
    /// The worker thread ended without an outcome (it panicked).
    Died,
}

/// A running two-tier sweep, seen from the UI thread. Dropping it stops the sweep.
pub(super) struct RunHandle2d {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Sweep2dOutcome>>>,
    worker: Option<JoinHandle<()>>,
}

impl RunHandle2d {
    /// How many angle pairs the sweep tries.
    pub(super) const fn total(&self) -> usize {
        self.total
    }

    /// Asks the sweep to stop; the cells finished so far still come back from
    /// [`Self::poll`].
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Looks at the sweep: still running, over, or dead.
    pub(super) fn poll(&mut self) -> Poll2d {
        // Whether the thread has ended is read before the outcome: the thread stores the
        // outcome before it ends, so an ended thread with no outcome really died.
        let ended = self.worker.as_ref().is_some_and(JoinHandle::is_finished);
        let outcome = self
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match (outcome, ended) {
            (Some(outcome), _) => {
                self.reap();
                Poll2d::Finished(Box::new(outcome))
            }
            (None, true) => {
                self.reap();
                Poll2d::Died
            }
            (None, false) => Poll2d::Running {
                done: self.done.load(Ordering::Relaxed),
            },
        }
    }

    /// Joins the ended worker thread.
    fn reap(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RunHandle2d {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts `job` on a worker thread.
///
/// # Errors
///
/// A sentence for the dialog when the operating system refuses the thread.
pub(super) fn start_2d(job: Job2d) -> Result<RunHandle2d, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let total = job.plan.len();
    let worker = {
        let cancel = Arc::clone(&cancel);
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        thread::Builder::new()
            .name("angle-sweep-2d".to_owned())
            .spawn(move || {
                let lookup = EditorMaterialLookup::new(&job.custom_materials);
                // The stone is scored in the orientation the design gives it.
                let material = resolved_gem_material_for_design(
                    &job.material,
                    &lookup,
                    &job.design.render_effects,
                );
                let scene = SweepScene {
                    material: &material,
                    environment: job.lighting.studio(
                        1.0,
                        CANONICAL_LIGHT_YAW,
                        CANONICAL_LIGHT_PITCH,
                    ),
                };
                let progress = |finished: usize, _total: usize| {
                    done.store(finished, Ordering::Relaxed);
                };
                let outcome = sweep_two_tier_angles(
                    &job.design,
                    &job.plan,
                    &job.options,
                    &scene,
                    &cancel,
                    &progress,
                );
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .map_err(|error| format!("The sweep could not start: {error}."))?
    };
    Ok(RunHandle2d {
        cancel,
        done,
        total,
        result,
        worker: Some(worker),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use pleochroma_editor::sweep::{SweepRange, plan_sweep_2d};
    use std::time::{Duration, Instant};

    fn job(workers: usize) -> Job2d {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.ensure_tier_ids();
        let range = |from_deg: f64, to_deg: f64| SweepRange {
            from_deg,
            to_deg,
            step_deg: 1.0,
        };
        let plan = plan_sweep_2d(&design, 2, &range(34.0, 35.0), 5, &range(40.0, 41.0))
            .expect("a crown and pavilion grid");
        Job2d {
            design,
            plan,
            material: MaterialSelection::none(),
            custom_materials: Vec::new(),
            lighting: LightingPreset::default(),
            options: SweepOptions {
                tilt_average: false,
                workers,
            },
        }
    }

    fn wait_for(handle: &mut RunHandle2d) -> Poll2d {
        let give_up = Instant::now() + Duration::from_secs(120);
        loop {
            match handle.poll() {
                Poll2d::Running { .. } if Instant::now() < give_up => {
                    thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn a_started_sweep_is_polled_to_its_outcome() {
        let mut handle = start_2d(job(2)).expect("the thread starts");
        let total = handle.total();
        assert!(
            total >= 4,
            "two angles on each side, the current pair among them"
        );
        let Poll2d::Finished(outcome) = wait_for(&mut handle) else {
            panic!("the sweep should finish");
        };
        assert!(!outcome.cancelled);
        assert_eq!(outcome.cells.len(), total);
        assert_eq!(
            outcome.cells.iter().filter(|cell| cell.is_current).count(),
            1
        );
    }

    #[test]
    fn a_cancelled_sweep_still_hands_back_what_it_finished() {
        let mut handle = start_2d(job(1)).expect("the thread starts");
        let total = handle.total();
        handle.cancel();
        let Poll2d::Finished(outcome) = wait_for(&mut handle) else {
            panic!("a cancelled sweep still ends with an outcome");
        };
        assert!(outcome.cells.len() <= total);
        assert_eq!(outcome.cancelled, outcome.cells.len() < total);
    }
}
