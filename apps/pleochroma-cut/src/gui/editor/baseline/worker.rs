//! The scoring thread of the Baseline strip, and the handle the UI thread polls.
//!
//! The same pattern as `gui::editor::sweep::run`: a job runs on a thread of its own and a timer
//! on the UI thread polls the [`RunHandle`] (the UI thread owns `Rc`s that a worker cannot reach,
//! so the worker never calls back). A job scores two designs at most: the design now, from the
//! masts the solve just produced (it never solves), and the Baseline when its figures are
//! missing (from its own masts, or with a solve of its own when it has none).
//!
//! Everything is rated under the Grading tray at the canonical light pose, like Optimize,
//! Retarget and Angle Sweep, so a delta, a trial and a sweep row compare directly whatever
//! lighting the viewport shows.

use crate::gui::editor::material_lookup::{EditorMaterialLookup, resolved_gem_material_for_design};
use pleochroma::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use pleochroma_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, MaterialSelection,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use pleochroma_editor::sweep::{Cancelled, RowMetrics, SweepOptions, SweepScene, score_design};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Everything a scoring run reads, owned so it can move to the worker thread.
pub(super) struct Job {
    /// The design now with the masts of its solve, when it has to be scored.
    pub design: Option<(Design, Vec<SolvedTier>)>,
    /// The Baseline's design and masts, when its figures are missing. Without masts the
    /// worker solves the Baseline itself, once.
    pub baseline: Option<(Design, Option<Vec<SolvedTier>>)>,
    /// The design's material (with its refractive index filled in when it names none): both
    /// designs are scored under it.
    pub material: MaterialSelection,
    /// The custom catalogue materials the selection may name.
    pub custom_materials: Vec<GemMaterial>,
    /// Also average the tilt performance (a few seconds), for "Measure Tilt".
    pub tilt: bool,
}

/// What a run made of one design.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Part {
    /// Not part of the job.
    Skipped,
    /// Scored; `None` is a design that gives no stone.
    Scored(Option<RowMetrics>),
    /// Stopped before it was scored.
    Cancelled,
}

/// What a finished run hands back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Scored {
    /// The Baseline's figures.
    pub baseline: Part,
    /// The design now's figures.
    pub design: Part,
}

/// What a look at a running job finds.
#[derive(Debug)]
pub(super) enum Poll {
    /// Still going.
    Running,
    /// Over, finished or stopped.
    Finished(Scored),
    /// The worker thread ended without a result (it panicked).
    Died,
}

/// A running job, seen from the UI thread. Dropping it stops the job.
pub(super) struct RunHandle {
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Scored>>>,
    worker: Option<JoinHandle<()>>,
}

impl RunHandle {
    /// Asks the job to stop; it ends with [`Part::Cancelled`] for what it had not scored.
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Looks at the job: still running, over, or dead.
    pub(super) fn poll(&mut self) -> Poll {
        // Whether the thread has ended is read before the result: the thread stores the
        // result before it ends, so an ended thread with no result really died.
        let ended = self.worker.as_ref().is_some_and(JoinHandle::is_finished);
        let result = self
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match (result, ended) {
            (Some(scored), _) => {
                self.reap();
                Poll::Finished(scored)
            }
            (None, true) => {
                self.reap();
                Poll::Died
            }
            (None, false) => Poll::Running,
        }
    }

    /// Joins the ended worker thread.
    fn reap(&mut self) {
        if let Some(worker) = self.worker.take() {
            // reason: a panic on the worker already shows as `Poll::Died`.
            let _ = worker.join();
        }
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts `job` on a worker thread.
///
/// # Errors
///
/// A sentence for the log when the operating system refuses the thread.
pub(super) fn start(job: Job) -> Result<RunHandle, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let worker = {
        let cancel = Arc::clone(&cancel);
        let result = Arc::clone(&result);
        thread::Builder::new()
            .name("baseline-deltas".to_owned())
            .spawn(move || {
                let scored = run(&job, &cancel);
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(scored);
            })
            .map_err(|error| format!("The figures could not be scored: {error}."))?
    };
    Ok(RunHandle {
        cancel,
        result,
        worker: Some(worker),
    })
}

/// Scores what `job` asks for under the Grading tray.
fn run(job: &Job, cancel: &AtomicBool) -> Scored {
    let lookup = EditorMaterialLookup::new(&job.custom_materials);
    let options = SweepOptions {
        tilt_average: job.tilt,
        workers: 1,
    };
    // Each design is scored in the orientation it gives its own stone, so a Baseline cut with
    // another Crystal Axis is compared as it was.
    let score = |design: &Design, solved: Option<&[SolvedTier]>| {
        let material =
            resolved_gem_material_for_design(&job.material, &lookup, &design.render_effects);
        let scene = SweepScene {
            material: &material,
            environment: CANONICAL_LIGHTING_PRESET.studio(
                1.0,
                CANONICAL_LIGHT_YAW,
                CANONICAL_LIGHT_PITCH,
            ),
        };
        match score_design(design, solved, &scene, &options, cancel) {
            Ok(metrics) => Part::Scored(metrics),
            Err(Cancelled) => Part::Cancelled,
        }
    };
    // The Baseline first: a delta is only shown once both sides are scored.
    let baseline = job
        .baseline
        .as_ref()
        .map_or(Part::Skipped, |(design, solved)| {
            score(design, solved.as_deref())
        });
    let design = if baseline == Part::Cancelled {
        Part::Cancelled
    } else {
        job.design
            .as_ref()
            .map_or(Part::Skipped, |(design, solved)| {
                score(design, Some(solved.as_slice()))
            })
    };
    Scored { baseline, design }
}
