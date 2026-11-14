//! The replan-on-edit request path of the shared viewport.
//!
//! [`submit_preview_replan`] and its
//! variants hand an edit to the solid preview's worker thread, and [`push_solved_preview`]
//! pushes the worker's frame back into the tier table, the banner and the yield figures.

use super::planned_frame_size;
use crate::{
    EditorModel, MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            manipulate::{frame_updates_mast_cache, note_committed_replan_submitted},
            state::{
                EditorState, apply_multi_selection, manufacturability_warnings_tagged_with,
                push_multi_selected_count, push_tiers, status_text_and_is_problem_from_solved,
                tier_items_from_solved_with_warnings, yield_report_texts_from_solved,
            },
        },
        render::camera_lighting::view_mode_index,
        solid_preview::{
            cut_slider,
            preview_state::{CameraPose, ReplanRequest, SolidLastSolved, SolidPreviewState},
        },
    },
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{Design, ManufacturabilityWarning};
use slint::{ComponentHandle, SharedString};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
};

/// Submits a replan-on-edit request to the Solid preview's worker thread. The solve
/// (`resolve_dirty` or `Design::solve`) never runs on the UI thread.
///
/// `dirty` names the tiers the edit touched; leave it empty for an edit that moves no
/// mast (material, preform, girdle diameter). Pass `force_full_solve: true` for an edit
/// whose blast radius is not tracked (undo/redo, a gear remap, a symmetry change). A full
/// `solve()` is always correct, while a subgraph `resolve_dirty` is only correct when
/// `dirty` is exact.
///
/// The request carries `state.generation`. A matching `Design` and `multi_selected`
/// snapshot is stashed by `auto_solve::stash_current_design`, and taken back by
/// `auto_solve::take_matching_design` when the frame reaches
/// `gui::solid_sink::SlintSolidSink::apply`.
pub(in crate::gui::editor) fn submit_preview_replan(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    dirty: BTreeSet<usize>,
    force_full_solve: bool,
) {
    submit_preview_replan_for(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        ReplanSource {
            design: &state.design,
            generation: state.generation.load(AtomicOrdering::Relaxed),
            multi_selected: &state.multi_selected,
        },
        dirty,
        force_full_solve,
    );
}

/// The `design`, `generation` and `multi_selected` inputs of a replan, bundled so that
/// [`submit_preview_replan_for`] stays under clippy's argument-count limit.
#[derive(Clone, Copy)]
pub(in crate::gui::editor) struct ReplanSource<'a> {
    pub(in crate::gui::editor) design: &'a Design,
    pub(in crate::gui::editor) generation: u64,
    pub(in crate::gui::editor) multi_selected: &'a BTreeSet<usize>,
}

/// The body of [`submit_preview_replan`], taking a [`ReplanSource`] snapshot instead of a
/// live `&EditorState`. `auto_solve::schedule_idle_replan_if_stale` uses it to resubmit a
/// follow-up replan once the worker is idle, without holding the live state.
pub(in crate::gui::editor) fn submit_preview_replan_for(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    source: ReplanSource<'_>,
    dirty: BTreeSet<usize>,
    force_full_solve: bool,
) {
    let generation = source.generation;
    let last_solved = if force_full_solve {
        None
    } else {
        solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|(cached_generation, masts)| {
                // Chain cached masts only if they match this generation or the one before.
                // Older masts are likely misaligned with `dirty`'s tier indices.
                // `live_update::plan_preview` falls back to a full solve on a length mismatch.
                (*cached_generation == generation || *cached_generation + 1 == generation)
                    .then(|| masts.clone())
            })
    };
    submit_preview_replan_chained(ui, render_ctx, preview_state, source, dirty, last_solved);
}

/// [`submit_preview_replan_for`] with the `last_solved` masts passed in, for a design that is
/// not the committed one: the shared cache only holds the committed design's masts. The
/// Slice tool's provisional replans pass their previous frame's masts with
/// `dirty = {provisional tier}`, so `live_update::plan_preview` re-solves a subgraph.
/// `None` means a full solve.
pub(in crate::gui::editor) fn submit_preview_replan_chained(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    source: ReplanSource<'_>,
    dirty: BTreeSet<usize>,
    last_solved: Option<Vec<SolvedTier>>,
) {
    // A ghost still being solved would land over this replan's frame.
    super::super::optimize_apply::cancel_ghost_preview();
    let ReplanSource {
        design,
        generation,
        multi_selected,
    } = source;
    let (camera, render_size, n_d) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            CameraPose {
                roll: ctx.camera_roll,
                yaw: ctx.yaw,
                pitch: ctx.pitch,
                distance: ctx.distance,
                pan: ctx.camera_pan,
            },
            (ctx.width, ctx.height),
            // `_with` resolves a CUSTOM
            // catalogue material by name too, not only a built-in preset -- the
            // bare accessor silently falls back to 1.5442 (fused quartz) for any
            // design named after a custom material, which the tilt overlay this
            // `n_d` feeds (`tier_items`'s critical-angle margin column) would then
            // measure against the wrong critical angle with nothing on screen
            // saying so.
            design.effective_refractive_index_with(&ctx.custom_materials),
        )
    };
    // A selected concave row sits past the flat tiers (`design.tiers.len() + concave index`).
    // `FacetMap::overlay_flags` tints that tier's tool facets from it. Indices past the last
    // concave row are dropped.
    let selected_tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
        .ok()
        .filter(|&index| index < design.tiers.len() + design.concave_tiers.len());
    let view_mode = view_mode_index(ui.global::<SolidPreviewModel>().get_view_mode());
    // Sized like the frame in `refresh_viewport` (see `planned_frame_size`).
    let size = planned_frame_size(ui, view_mode, render_size);
    let show_preform = ui.global::<SolidPreviewModel>().get_show_preform_planes();
    let enlarged_panel = ui
        .global::<SolidPreviewModel>()
        .get_diagram_enlarged_panel();
    // One `Arc<Design>` snapshot per edit, shared by the stash below and the replan request.
    // `auto_solve::take_matching_design` can hand it back to
    // `editor::apply_matching_preview_frame` when a frame for this generation lands. That
    // path cannot read `state` directly: the sink runs on a worker thread and cannot reach
    // the `Rc<RefCell<EditorState>>`.
    //
    // `ReplanRequest::design` is an `Arc<Design>` too, so `Arc::clone` shares this
    // allocation instead of cloning the design again. A burst of coalesced requests (an
    // angle-nudge drag, most often) therefore clones the design at most once per generation.
    let design_snapshot = Arc::new(design.clone());
    // The Slice tool's provisional design (a reserved generation) is not the live design.
    // Stashing it would overwrite the committed snapshot and arm the idle replan on the
    // wrong design. `frame_updates_mast_cache` tells the two apart.
    if frame_updates_mast_cache(generation) {
        // A committed job takes the plan gate's single slot, so a queued provisional replan
        // is dropped and the Slice tool must not wait for its frame.
        note_committed_replan_submitted();
        auto_solve::stash_current_design(
            generation,
            Arc::clone(&design_snapshot),
            multi_selected.clone(),
        );
    }
    // Read here, not cached on `SolidPreviewState` alone, so the slider and the redraw agree
    // on the shown cut (`None` is finished, `Some(0)` the rough). Only a committed replan
    // refreshes the slider's fields from this design; a provisional Slice design has one
    // tier too many. The Diagram always shows the finished design.
    let position_steps = if frame_updates_mast_cache(generation) {
        cut_slider::sync_model(ui, design)
    } else {
        cut_slider::current_steps(ui, design)
    };
    preview_state.set_cut_steps(position_steps.filter(|_| {
        crate::gui::view_modes::EditViewMode::from_raw(i32::from(view_mode))
            != crate::gui::view_modes::EditViewMode::Diagram
    }));
    preview_state.request_replan(ReplanRequest {
        design: Arc::clone(&design_snapshot),
        dirty,
        last_solved,
        camera,
        size,
        selected_tier,
        n_d,
        view_mode,
        generation,
        show_preform,
        enlarged_panel,
    });
}

/// Pushes the tier table, the validation banner, the manufacturability warnings and the
/// yield figures from a solid-preview frame's already-solved `solved` masts. It is called
/// once `auto_solve::take_matching_design` confirms the frame's generation is still the
/// live one. `design` and `multi_selected` are the clones that call returned.
///
/// Narrower than [`super::super::panel::refresh_editor_panel`] and
/// [`super::super::panel_stale::push_stale_content`]. Proportions, the cutting schedule,
/// preform and material scratch fields, and Deep Solve/Optimize availability are either
/// solve-independent (pushed at edit time by
/// [`super::super::panel_stale::refresh_editor_panel_stale`]) or not pushed here.
/// Only the fields that the background solve's completion would otherwise be the sole
/// source of are updated.
///
/// Called from `gui::solid_sink::SlintSolidSink::apply` through
/// `editor::apply_matching_preview_frame`, which hops from the solid-preview worker thread onto the
/// UI thread.
///
/// `custom_materials` resolves the effective refractive index the same custom-catalogue-aware
/// way as [`super::super::inspector::refresh_design_settings`]. The bare accessor would fall
/// back to 1.5442 for a design named after a custom material.
///
/// `custom_sg` is the catalogue's custom-material specific-gravity table, for
/// [`yield_report_texts_from_solved`].
///
/// `precomputed_warnings`: the manufacturability pass the plan worker ran
/// (`PreviewFrame::warnings`). The rows and banner only display it, so the UI thread never
/// builds the stone's solid for the concave-tool check. `None` means no pass exists for this
/// frame's generation, and the row builders resolve it.
pub(in crate::gui::editor) fn push_solved_preview(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[pleochroma::optics::materials::GemMaterial],
    custom_sg: &[(String, f64)],
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    let n_d = design.effective_refractive_index_with(custom_materials);
    push_tier_rows(
        ui,
        design,
        solved,
        multi_selected,
        n_d,
        precomputed_warnings,
    );

    let (status_text, is_problem) = status_text_and_is_problem_from_solved(design, solved);
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);
    ui.global::<EditorModel>()
        .set_solve_state(if is_problem { "failed" } else { "solved" }.into());
    // Same solve as the verdict: a stale edit reset the count to 0.
    ui.global::<EditorModel>().set_facet_count(
        i32::try_from(crate::gui::editor::view::facet_count_from_solved(
            design, solved,
        ))
        .unwrap_or(i32::MAX),
    );
    // Lets the guide's "Solve and check" step see the verdict set above.
    crate::gui::editor::guide::check_design_progress(ui, design);

    push_warning_list(ui, design, solved, precomputed_warnings);

    let (vol_yield_text, carat_text, sg_used_text, fit_text) =
        yield_report_texts_from_solved(design, solved, custom_sg);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());

    // The verdict chips must describe this design too; see
    // `panel::push_proportion_verdicts_from_solved`.
    super::super::panel::push_proportion_verdicts_from_solved(ui, design, Some(solved), n_d);
}

/// Refreshes the tier rows and warning list with the full manufacturability `findings` of a
/// frame that has already landed. The plan worker submits the frame before its costly
/// concave-tool check, so the first push only had the checks that need no solid. Status,
/// yield figures and verdict chips do not depend on the findings and are left alone.
///
/// The caller has already checked that `design` is still the editor's design. With
/// Auto-Solve Off and a design the last edit left "Not solved" nothing is pushed.
pub(in crate::gui::editor) fn push_late_findings(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[pleochroma::optics::materials::GemMaterial],
    findings: &[ManufacturabilityWarning],
) {
    // Auto-Solve Off with a "Not solved" design: the frame never pushed its rows
    // (`editor::apply_matching_preview_frame`), so its findings must not either.
    if !auto_solve::preview_may_push_status(ui) {
        return;
    }
    let n_d = design.effective_refractive_index_with(custom_materials);
    push_tier_rows(ui, design, solved, multi_selected, n_d, Some(findings));
    push_warning_list(ui, design, solved, Some(findings));
}

/// The tier table's rows for `design` from its masts, with the multi-selection marked.
/// `precomputed_warnings` is the plan worker's manufacturability pass, when it has one.
fn push_tier_rows(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    n_d: f64,
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    let mut tiers = tier_items_from_solved_with_warnings(design, solved, n_d, precomputed_warnings);
    apply_multi_selection(&mut tiers, multi_selected);
    push_tiers(ui, tiers);
    push_multi_selected_count(ui, multi_selected.len());
}

/// The manufacturability warning list and the tier index each one is about.
fn push_warning_list(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    // Tagged pairs rather than plain text, so the tier index survives to
    // `manufacturability_warning_tiers`, which `editor_tier_table.slint` reads to flag the row.
    let tagged_warnings =
        manufacturability_warnings_tagged_with(design, solved, precomputed_warnings);
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    super::super::state::push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    super::super::state::push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );
}
