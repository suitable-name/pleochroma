//! The verdict's worker: the geometry checks and the optical measurement, off the UI thread.
//!
//! [`compute`] is the whole job and is pure (a design, its masts and a material in, the
//! verdict's inputs out), so it is what the tests call. [`spawn`] runs it on a thread and
//! hands the result back to [`super::finish`] on the UI thread.
//!
//! The job never solves: the masts come from the solve the caller already has. What it does
//! is geometry (the planes, the solid mesh, the manufacturability checks, a few milliseconds)
//! and, when the design names a material, one fast table-up optical measurement (about 2 ms,
//! the same one the Retarget dialog's table uses).
//!
//! The windowing Check also reads the 10 degree tilt: the RI range worker
//! (`gui::editor::ri_range`) traces it at the design's own index, and [`apply_small_tilt`] feeds
//! it into the verdict on screen when it lands, so the Check updates in place and nothing is
//! measured twice.

use super::{SLOT, finish, present, services};
use crate::{MainWindow, gui::editor::ri_range};
use pleochroma::{
    geometry::meet_solver::SolvedTier,
    optics::{LightingPreset, materials::GemMaterial, raytracer::FacetFinish},
};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material_for_design},
    retarget::metrics::measure_column_with_finishes,
    solve_policy::design_to_gpu_planes_from_solved,
    verdict::{
        SolveFacts, VanishSnapshot, VerdictInputs, axis_fact, evaluate, gather, vanish_snapshot,
    },
};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

thread_local! {
    /// The inputs of the latest verdict that landed, with the request's serial, so the RI range
    /// can be added to them without measuring again.
    static LAST_INPUTS: RefCell<Option<(u64, VerdictInputs)>> = const { RefCell::new(None) };
}

/// Everything a worker needs, bundled so the debounce timer can hold it until it fires.
pub(super) struct Job {
    /// A copy of the design the verdict is about.
    pub(super) design: Design,
    /// The masts of its solve, or `None` when it did not solve.
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// The design's effective refractive index (what the tier table's margins use).
    pub(super) n_d: f64,
    /// The lighting the viewport uses, for the optical measurement.
    pub(super) lighting: LightingPreset,
    /// The custom materials, to find the design's material; `None` skips the measurement.
    pub(super) custom: Option<Arc<Vec<GemMaterial>>>,
    /// The request this job answers; a newer request makes the result a stale one.
    pub(super) serial: u64,
    /// The window to report to.
    pub(super) ui_weak: slint::Weak<MainWindow>,
}

/// The design's material as the tracer sees it, when the design names one.
///
/// A preset or a refractive-index override counts. `None` for a design with no material set:
/// there is nothing to measure then, and the verdict is worked out without the optical figures.
fn design_gem(design: &Design, custom: &[GemMaterial]) -> Option<GemMaterial> {
    let material = &design.material;
    (material.name.is_some() || material.refractive_index_override.is_some()).then(|| {
        resolved_gem_material_for_design(
            material,
            &EditorMaterialLookup::new(custom),
            &design.render_effects,
        )
    })
}

/// The verdict's inputs for `design`: the geometry facts of [`gather`] and, when the stone
/// closes and the design names a material, the optical figures.
///
/// `solved` is the mast list of the design's solve (`None` when it did not solve), `n_d` its
/// effective refractive index and `custom` the custom materials to look the design's material
/// up in (`None` skips the measurement).
pub(super) fn compute(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
    lighting: LightingPreset,
    custom: Option<&[GemMaterial]>,
) -> VerdictInputs {
    let inputs = gather(design, solved, n_d, None);
    if inputs.solve != SolveFacts::Closed {
        return inputs;
    }
    let (Some(solved), Some(custom)) = (solved, custom) else {
        return inputs;
    };
    let Some(gem) = design_gem(design, custom) else {
        return inputs;
    };
    // The crystal-axis note (informational only: it never changes the verdict's level).
    let inputs = inputs.with_axis(axis_fact(design, &gem, None));
    // `Closed` means `gather` accepted the mast list as one entry per tier.
    let planes = design.planes_from_solved(solved);
    let finishes = design_finishes(design, solved);
    let column = measure_column_with_finishes(&planes, &[], &finishes, &gem, lighting);
    inputs.with_optics(Some(column))
}

/// The finish of each of `design`'s planes (`design.planes_from_solved(solved)`): the frosted
/// facets and the frosted girdle, as the render shows them. Empty when nothing is frosted.
fn design_finishes(design: &Design, solved: &[SolvedTier]) -> Vec<FacetFinish> {
    design.facet_finishes_from_solved(solved, &design_to_gpu_planes_from_solved(design, solved))
}

/// What a worker hands back: the verdict's inputs and the facets that vanish in the design.
pub(super) type Outcome = (VerdictInputs, VanishSnapshot);

/// Hands the design of `job` to the RI range worker: the hint shows at once and the trace starts
/// once changes pause. Called on the UI thread, wherever the verdict's worker starts.
fn hand_to_ri_range(job: &Job) {
    let Some(ui) = job.ui_weak.upgrade() else {
        return;
    };
    // The library row of the design; an editor that is borrowed right now has none to give.
    let entry_id = services().and_then(|services| {
        let editor = services.editor.try_borrow().ok()?;
        editor.source_entry_id
    });
    ri_range::note_solved(&job.design, job.solved.as_deref(), job.n_d, entry_id);
    ri_range::design_changed(&ui);
}

/// Runs `job` on a worker thread; the result comes back through [`finish`] on the UI thread.
pub(super) fn spawn(job: Job) {
    hand_to_ri_range(&job);
    std::thread::spawn(move || {
        let Job {
            design,
            solved,
            n_d,
            lighting,
            custom,
            serial,
            ui_weak,
        } = job;
        let had_solution = solved.is_some();
        // A panic inside the geometry must not leave the badge on "out of date" for good; the
        // UI thread keeps what it showed.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let inputs = compute(
                &design,
                solved.as_deref(),
                n_d,
                lighting,
                custom.as_deref().map(Vec::as_slice),
            );
            let vanished = vanish_snapshot(&design, &inputs);
            (inputs, vanished)
        }));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Ok((inputs, _)) = &outcome {
                remember_inputs(serial, inputs);
            }
            finish(&ui, serial, had_solution, outcome.ok());
            // The range of this design may have landed before its verdict did.
            apply_small_tilt(&ui);
        });
    });
}

/// Keeps the inputs of the verdict that answers request `serial`.
pub(super) fn remember_inputs(serial: u64, inputs: &VerdictInputs) {
    LAST_INPUTS.with(|cell| *cell.borrow_mut() = Some((serial, inputs.clone())));
}

/// The inputs of the verdict on screen, when they describe the design as it stands.
pub(super) fn current_inputs() -> Option<VerdictInputs> {
    let (serial, inputs) = LAST_INPUTS.with(|cell| cell.borrow().clone())?;
    SLOT.with(|cell| {
        let slot = cell.borrow();
        (slot.current && slot.serial == serial).then_some(inputs)
    })
}

/// Adds the RI range worker's 10 degree windowing mean to the verdict on screen and shows the
/// Check again when it changed (while `WINDOWING_CHECK_READS_TILT` is off it never does).
///
/// Called when the range lands and when a verdict lands; does nothing until both exist for the
/// same design.
pub(in crate::gui::editor) fn apply_small_tilt(ui: &MainWindow) {
    let Some(range) = ri_range::latest() else {
        return;
    };
    let Some(inputs) = current_inputs() else {
        return;
    };
    if inputs.solve != SolveFacts::Closed {
        return;
    }
    let inputs = inputs.with_small_tilt_windowing(Some(range.own_small_tilt_windowing_pct()));
    let verdict = evaluate(&inputs);
    let changed = SLOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.verdict.as_ref() == Some(&verdict) {
            return false;
        }
        slot.verdict = Some(verdict.clone());
        true
    });
    if changed {
        present::push_verdict(ui, &verdict);
    }
}
