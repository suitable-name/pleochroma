//! The Tune view's mini sweep: a one-tier coarse sweep of the picked tier's angle, plus minus two
//! degrees in quarter-degree steps, run in the background by the same engine as the Angle Sweep
//! dialog (`plan_sweep` and `sweep_tier_angle`) under the Grading tray, so a row here and a row
//! of the dialog (or a figure of the strip) show the same numbers for the same design.
//!
//! - [`Basis`] says what a drawn sweep is a sweep *of*: the design (every tier's angle but the
//!   picked tier's own, the material), so a nudge of the picked tier itself keeps the chart valid
//!   and an edit of another tier makes it stale.
//! - [`Cache`] keeps a few drawn sweeps by their basis.
//! - [`start`] runs one on a worker thread and [`RunHandle`] is what the UI thread polls.
//! - [`best_row`], [`plateau_rows`] and [`jump_target`] read a drawn sweep.

use crate::gui::editor::material_lookup::{EditorMaterialLookup, resolved_gem_material_for_design};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, MaterialSelection,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use pleochroma_editor::{
    metric_deltas::{DeltaMetric, Tone, judge},
    sweep::{
        MAX_SWEEP_ANGLE_DEG, MIN_SWEEP_ANGLE_DEG, SweepError, SweepMetric, SweepOptions,
        SweepOutcome, SweepPlan, SweepRange, SweepRow, SweepScene, plan_sweep, sweep_tier_angle,
    },
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

/// How far either side of the current angle the mini sweep reaches, degrees.
pub(super) const HALF_RANGE_DEG: f64 = 2.0;

/// The distance between the angles of the mini sweep, degrees.
pub(super) const STEP_DEG: f64 = 0.25;

/// How many drawn sweeps are kept.
const CACHE_SIZE: usize = 16;

/// The figures the chart can show, in the figure combo's order. The same five the strip's
/// deltas judge (`DeltaMetric::ALL`).
pub(super) const FIGURES: [(SweepMetric, DeltaMetric); 5] = [
    (SweepMetric::Brilliance, DeltaMetric::Brilliance),
    (SweepMetric::Windowing, DeltaMetric::Windowing),
    (SweepMetric::Extinction, DeltaMetric::Extinction),
    (SweepMetric::Fire, DeltaMetric::Fire),
    (SweepMetric::Scintillation, DeltaMetric::Scintillation),
];

/// What a drawn sweep is a sweep of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Basis {
    /// The design epoch (another one is another design).
    pub epoch: u64,
    /// The tier swept.
    pub tier: usize,
    /// The angle of every tier but the swept one, as bit patterns, and the tier count.
    pub others: Vec<u64>,
    /// The design's material, written out.
    pub material: String,
}

impl Basis {
    /// The basis of a sweep of `tier` of `design`.
    pub(super) fn of(design: &Design, epoch: u64, tier: usize) -> Self {
        Self {
            epoch,
            tier,
            others: design
                .tiers
                .iter()
                .enumerate()
                .map(|(index, other)| {
                    if index == tier {
                        u64::MAX
                    } else {
                        other.angle_deg.to_bits()
                    }
                })
                .collect(),
            material: format!("{:?}", design.material),
        }
    }
}

/// The plan of the mini sweep of `tier`: plus or minus [`HALF_RANGE_DEG`] around its angle in
/// [`STEP_DEG`] steps, kept inside what a sweep takes; the current angle is always one of the
/// angles.
///
/// # Errors
///
/// Why the tier cannot be swept (it follows a relation, it is flat or vertical, ...).
pub(super) fn plan(design: &Design, tier: usize) -> Result<SweepPlan, SweepError> {
    let magnitude = design
        .tiers
        .get(tier)
        .map_or(0.0, |found| found.angle_deg.abs());
    plan_sweep(
        design,
        tier,
        SweepRange {
            from_deg: (magnitude - HALF_RANGE_DEG).max(MIN_SWEEP_ANGLE_DEG),
            to_deg: (magnitude + HALF_RANGE_DEG).min(MAX_SWEEP_ANGLE_DEG - 0.4),
            step_deg: STEP_DEG,
        },
    )
}

/// A few drawn sweeps, the newest last.
#[derive(Debug, Default)]
pub(super) struct Cache {
    entries: Vec<(Basis, SweepOutcome)>,
}

impl Cache {
    /// The sweep drawn for `basis`, if one is kept.
    pub(super) fn get(&self, basis: &Basis) -> Option<&SweepOutcome> {
        self.entries
            .iter()
            .find(|(kept, _)| kept == basis)
            .map(|(_, outcome)| outcome)
    }

    /// Keeps `outcome` for `basis`, replacing an older sweep of the same basis and dropping the
    /// oldest one when the cache is full.
    pub(super) fn put(&mut self, basis: Basis, outcome: SweepOutcome) {
        self.entries.retain(|(kept, _)| *kept != basis);
        if self.entries.len() >= CACHE_SIZE {
            self.entries.remove(0);
        }
        self.entries.push((basis, outcome));
    }

    /// Forgets everything (another design opened).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Everything a mini sweep reads, owned so it can move to the worker thread.
pub(super) struct Job {
    /// The design as it is now; every angle is solved on a clone of it.
    pub design: Design,
    pub plan: SweepPlan,
    /// The design's material (with its refractive index filled in when it names none).
    pub material: MaterialSelection,
    /// The custom catalogue materials the selection may name.
    pub custom_materials: Vec<GemMaterial>,
    /// How many threads the sweep may use.
    pub workers: usize,
}

/// What a look at a running mini sweep finds.
#[derive(Debug)]
pub(super) enum Poll {
    /// Still going: how many angles are done.
    Running { done: usize },
    /// Over, finished or stopped.
    Finished(Box<SweepOutcome>),
    /// The worker thread ended without an outcome (it panicked).
    Died,
}

/// A running mini sweep, seen from the UI thread. Dropping it stops the sweep.
pub(super) struct RunHandle {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<SweepOutcome>>>,
    worker: Option<JoinHandle<()>>,
}

impl RunHandle {
    /// How many angles the sweep tries.
    pub(super) const fn total(&self) -> usize {
        self.total
    }

    /// Looks at the sweep: still running, over, or dead.
    pub(super) fn poll(&mut self) -> Poll {
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
                Poll::Finished(Box::new(outcome))
            }
            (None, true) => {
                self.reap();
                Poll::Died
            }
            (None, false) => Poll::Running {
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

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts `job` on a worker thread.
///
/// # Errors
///
/// A sentence when the operating system refuses the thread.
pub(super) fn start(job: Job) -> Result<RunHandle, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let total = job.plan.angles.len();
    let worker = {
        let cancel = Arc::clone(&cancel);
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        thread::Builder::new()
            .name("tune-mini-sweep".to_owned())
            .spawn(move || {
                let lookup = EditorMaterialLookup::new(&job.custom_materials);
                // The stone is scored in the orientation the design gives it.
                let material = resolved_gem_material_for_design(
                    &job.material,
                    &lookup,
                    &job.design.render_effects,
                );
                // Rated under the Grading tray, whatever lighting the viewport shows.
                let lighting = CANONICAL_LIGHTING_PRESET;
                let scene = SweepScene {
                    material: &material,
                    environment: lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH),
                };
                let options = SweepOptions {
                    tilt_average: false,
                    workers: job.workers,
                };
                let progress = |finished: usize, _total: usize| {
                    done.store(finished, Ordering::Relaxed);
                };
                let outcome =
                    sweep_tier_angle(&job.design, &job.plan, &scene, &options, &cancel, &progress);
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .map_err(|error| format!("The sweep could not start: {error}."))?
    };
    Ok(RunHandle {
        cancel,
        done,
        total,
        result,
        worker: Some(worker),
    })
}

/// The row with the best `figure` (the highest or the lowest, whichever is better), the first of
/// equals; `None` when no row has the figure.
pub(super) fn best_row(rows: &[SweepRow], figure: usize) -> Option<usize> {
    let (metric, _) = FIGURES.get(figure)?;
    let mut best: Option<(usize, f64)> = None;
    for (index, row) in rows.iter().enumerate() {
        let Some(value) = metric.value(row) else {
            continue;
        };
        let better = best.is_none_or(|(_, held)| {
            if metric.higher_is_better() {
                value > held
            } else {
                value < held
            }
        });
        if better {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

/// The rows whose `figure` is as good as the best row's within the figure's noise threshold
/// (`metric_deltas::judge` calls the difference "same"), the best row included, in order.
pub(super) fn plateau_rows(rows: &[SweepRow], figure: usize) -> Vec<usize> {
    let (Some(best), Some(&(metric, delta))) = (best_row(rows, figure), FIGURES.get(figure)) else {
        return Vec::new();
    };
    let Some(best_value) = metric.value(&rows[best]) else {
        return Vec::new();
    };
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            metric.value(row).is_some_and(|value| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the judged figures are f32 percentages that were widened to f64"
                )]
                let tone =
                    judge(delta, best_value as f32, value as f32, false).map(|change| change.tone);
                tone == Some(Tone::Same)
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// The angle Jump to Best sets: the best row's (signed, as the tier stores it), unless that is
/// the angle the tier has now, `current_deg` (`None`).
pub(super) fn jump_target(rows: &[SweepRow], figure: usize, current_deg: f64) -> Option<f64> {
    let best = best_row(rows, figure)?;
    let row = rows.get(best)?;
    ((row.angle_deg - current_deg).abs() > ANGLE_EPSILON_DEG).then_some(row.angle_deg)
}

/// Two angles closer than this are the same angle, degrees.
pub(super) const ANGLE_EPSILON_DEG: f64 = 1e-6;
