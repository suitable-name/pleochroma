//! "Re-score Trials": scoring the trials whose figures were scored in another material, or not at
//! all, under the material of the design now.
//!
//! The same scene as the Baseline strip, the Angle Sweep and Optimize: the Grading tray at the
//! canonical light pose, never the viewport's lighting, so a re-scored trial compares directly
//! with the Baseline. One thread scores the trials one after the other and the UI thread's timer
//! files what has come in ([`Run::drain`]); the thread never calls back (the UI thread owns
//! `Rc`s it cannot reach).

use super::record::key_with_axis;
use crate::gui::editor::material_lookup::{EditorMaterialLookup, resolved_gem_material_for_design};
use pleochroma::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use pleochroma_cut_core::{
    CANONICAL_LIGHTING_PRESET, CrystalOrientation, Design, MaterialSelection,
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

/// One trial to score.
pub(super) struct Item {
    pub number: u32,
    pub design: Design,
    /// Its masts when it has them; without, the scoring solves the design itself.
    pub solved: Option<Vec<SolvedTier>>,
}

/// Everything a run reads, owned so it can move to the worker thread.
pub(super) struct Job {
    pub items: Vec<Item>,
    /// The material of the design now (its refractive index filled in when it names none).
    pub material: MaterialSelection,
    /// The custom catalogue materials the selection may name.
    pub custom_materials: Vec<GemMaterial>,
    /// The material's stored default Crystal Axis, for the key a trial's figures are filed under.
    pub material_default: Option<CrystalOrientation>,
}

/// The figures of one scored trial (`None` for a design that gives no stone) and the key they
/// are filed under: the material key with the trial's OWN crystal axis, the orientation they
/// were scored in.
type Done = (u32, Option<RowMetrics>, String);

/// A running job, seen from the UI thread. Dropping it stops the job.
pub(super) struct Run {
    cancel: Arc<AtomicBool>,
    results: Arc<Mutex<Vec<Done>>>,
    worker: Option<JoinHandle<()>>,
}

impl Run {
    /// Whether the thread has ended. Look at this before [`Self::drain`]: the thread stores
    /// every figure before it ends, so what is drained after an ended thread is everything.
    pub(super) fn is_over(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// The figures that came in since the last call.
    pub(super) fn drain(&self) -> Vec<Done> {
        std::mem::take(&mut *self.results.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            // reason: a panic on the worker only means some trials stay unscored.
            let _ = worker.join();
        }
    }
}

/// Starts `job` on a worker thread, filing each trial's figures under `key`'s material part
/// and that trial's own crystal axis.
///
/// # Errors
///
/// A sentence for the cutter when the operating system refuses the thread.
pub(super) fn start(job: Job, key: String) -> Result<Run, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let results = Arc::new(Mutex::new(Vec::new()));
    let worker = {
        let cancel = Arc::clone(&cancel);
        let results = Arc::clone(&results);
        thread::Builder::new()
            .name("trials-rescore".to_owned())
            .spawn(move || run(&job, &key, &cancel, &results))
            .map_err(|error| format!("The trials could not be scored: {error}."))?
    };
    Ok(Run {
        cancel,
        results,
        worker: Some(worker),
    })
}

/// Scores every item under the Grading tray, storing each figure as it is done.
pub(super) fn run(job: &Job, key: &str, cancel: &AtomicBool, results: &Mutex<Vec<Done>>) {
    let lookup = EditorMaterialLookup::new(&job.custom_materials);
    let options = SweepOptions {
        tilt_average: false,
        workers: 1,
    };
    for item in &job.items {
        // Each trial is scored in the orientation ITS design gives the stone (never the
        // design's now), and its figures are filed under the key naming that orientation
        // (`record::key_with_axis`), so a key never names an axis the figures were not scored in.
        let material =
            resolved_gem_material_for_design(&job.material, &lookup, &item.design.render_effects);
        let filed_under = key_with_axis(key, &item.design.render_effects, job.material_default);
        let scene = SweepScene {
            material: &material,
            environment: CANONICAL_LIGHTING_PRESET.studio(
                1.0,
                CANONICAL_LIGHT_YAW,
                CANONICAL_LIGHT_PITCH,
            ),
        };
        match score_design(
            &item.design,
            item.solved.as_deref(),
            &scene,
            &options,
            cancel,
        ) {
            Ok(metrics) => results
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((item.number, metrics, filed_under)),
            Err(Cancelled) => return,
        }
    }
}
