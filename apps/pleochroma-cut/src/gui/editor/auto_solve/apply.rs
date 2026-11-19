//! Applying a completed background solve's result back onto the UI.
//!
//! This covers the two staleness checks, the panel/proportions/schedule pushes, the plane-cap
//! toast, and re-dispatching whatever request queued up behind the just-finished solve.

use super::{
    dispatch::is_current,
    runtime::{PendingDispatch, RUNTIME},
};
use crate::{
    AngleItem, EditorModel, EditorTierItem, MainWindow, SolidPreviewModel, TiltModel,
    bridge::{
        render_effects::surface_table,
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::{
        editor::{
            state::{
                cutting_instructions_rows, girdle_and_ratio_texts, preform_mm_texts,
                preform_y_offset_mm_text, proportions_texts, push_multi_selected_count, push_tiers,
            },
            view::{
                ReplanSource, facet_count_from_solved, girdle_and_ratio_texts_from_solved,
                planned_frame_size, preform_mm_texts_from_solved, proportions_texts_from_solved,
                push_proportion_verdicts_from_solved, submit_preview_replan_for,
            },
        },
        render::{camera_lighting::view_mode_index, fit_view},
        show_toast,
        solid_preview::{cut_slider, preview_state::CameraPose},
    },
};
use pleochroma::geometry::{GpuFacetPlane, ToolPrimitive, meet_solver::SolvedTier};
use pleochroma_cut_core::Design;
use pleochroma_solid::preview::StoneGeometryBuf;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// A completed background solve's results, bundled into one struct to keep
/// [`apply_background_solve_result`]'s argument count within clippy's limit.
pub(super) struct BackgroundSolveResult {
    pub(super) elapsed: Duration,
    pub(super) tiers: Vec<EditorTierItem>,
    pub(super) status_text: String,
    pub(super) status_is_problem: bool,
    /// `(tier index, warning text)` pairs, so the warnings can be tied to their rows.
    pub(super) warnings: Vec<(usize, String)>,
    pub(super) yield_texts: (String, String, String, String),
    pub(super) planes: Vec<GpuFacetPlane>,
    /// The frosted planes among `planes`; empty when the design has no frosting.
    pub(super) frosted: Vec<u32>,
    /// The concave tools cut out of `planes`, and each tool's `(concave tier,
    /// placement)`; both empty for a planar design.
    pub(super) tools: Vec<ToolPrimitive>,
    pub(super) placements: Vec<(usize, usize)>,
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// `true` iff `status_text` is the plane-cap sentence from
    /// `super::dispatch::too_many_planes_message`. Drives the once-per-design warning
    /// toast; see `super::runtime::Runtime::too_many_planes_toasted`.
    pub(super) too_many_planes: bool,
    /// Index-wheel tooth count and reference angle, for the diagram view's wheel.
    pub(super) gear: (u32, f32),
    /// `multi_selected.len()` at dispatch time, for the tier table's "N selected"
    /// header indicator.
    pub(super) multi_selected_count: usize,
    /// The design's effective refractive index, the same value used for the tier
    /// table's critical-angle margin column. Also used for the proportion-verdict chips.
    pub(super) n_d: f64,
    /// `true` iff this is the minimal placeholder from [`Self::panicked`]. All other
    /// fields are empty, so the panel must keep what it already shows.
    pub(super) panicked: bool,
    /// The `Design` snapshot that was solved. Used to build the panel fields that
    /// depend on the design itself.
    pub(super) design: Design,
}

impl BackgroundSolveResult {
    /// Minimal placeholder for a solve cancelled before `dispatch::panel_inputs` ran.
    /// `dispatch::cancel_in_flight_solve` bumps `Runtime::current_seq`, so
    /// `apply_background_solve_result` discards this result before any empty field is
    /// read. The empty values only let the struct be built on that path.
    pub(super) const fn cancelled(
        design: Design,
        elapsed: Duration,
        multi_selected_count: usize,
    ) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "gear reference angle (degrees) is stored as f32 in the panel model"
        )]
        let gear_angle = design.meta.gear_reference_angle as f32;
        Self {
            elapsed,
            tiers: Vec::new(),
            status_text: String::new(),
            status_is_problem: false,
            warnings: Vec::new(),
            yield_texts: (String::new(), String::new(), String::new(), String::new()),
            planes: Vec::new(),
            frosted: Vec::new(),
            tools: Vec::new(),
            placements: Vec::new(),
            solved: None,
            too_many_planes: false,
            gear: (design.meta.gear_teeth_abs(), gear_angle),
            multi_selected_count,
            n_d: 0.0,
            panicked: false,
            design,
        }
    }

    /// Placeholder for a worker that panicked in `dispatch::solve_and_build_result`.
    /// Unlike [`Self::cancelled`] it may still be current, so it is reported, not
    /// discarded. Only the status text is set; the panel keeps its previous data.
    pub(super) fn panicked(design: Design, elapsed: Duration, multi_selected_count: usize) -> Self {
        Self {
            status_text: "Solve failed unexpectedly (internal error) -- try again or simplify \
                          the design."
                .to_string(),
            status_is_problem: true,
            panicked: true,
            ..Self::cancelled(design, elapsed, multi_selected_count)
        }
    }
}

/// Frees the in-flight slot and takes the [`PendingDispatch`] queued behind this
/// completion. Runs before any staleness check, so a queued request is still
/// dispatched even when this completion is stale.
fn free_in_flight_slot() -> Option<PendingDispatch> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.solve_in_flight = false;
        // The worker has finished, so its cancel flag has nothing left to stop.
        rt.current_cancel = None;
        // Removes the activity row on every completion path. A cancel from the
        // activity list has already reset `EditorModel`; this only removes the row.
        // See `Runtime::activity_id`.
        if let (Some(activity), Some(id)) = (rt.activity.clone(), rt.activity_id.take()) {
            activity.finish(id);
        }
        rt.pending_dispatch.take()
    })
}

/// Shows one warning toast for a design stuck over `MAX_PLANES`. The status strip
/// already repeats the sentence on every refresh, so the toast is not repeated.
/// See `super::runtime::Runtime::too_many_planes_toasted`.
fn maybe_toast_too_many_planes(ui: &MainWindow, result: &BackgroundSolveResult) {
    let already_toasted = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let already = rt.too_many_planes_toasted;
        rt.too_many_planes_toasted = result.too_many_planes;
        already
    });
    if result.too_many_planes && !already_toasted {
        show_toast(ui, &result.status_text, "warning");
    }
}

/// Applies a completed background solve's result, after the two staleness checks in the
/// parent module doc. The plane sign convention matches `view::refresh_viewport`, with the
/// planes computed on the worker thread.
pub(super) fn apply_background_solve_result(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    seq: u64,
    started_generation: u64,
    generation: &Arc<AtomicU64>,
    result: BackgroundSolveResult,
) {
    let pending = free_in_flight_slot();

    if !is_current(seq) {
        // A newer dispatch has already taken over the banner and will apply its own
        // result in turn -- nothing here is still current enough to show.
        // `record_solve_duration` must not run above this check -- a
        // superseded result's elapsed time is not a measurement of the design that
        // is actually current, and must never set `should_schedule_auto_solve`'s
        // budget baseline.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }
    super::scheduling::record_solve_duration(result.elapsed);
    ui.global::<EditorModel>().set_solve_running(false);
    if generation.load(Ordering::Relaxed) != started_generation {
        // The design changed after this solve was dispatched, without a newer
        // dispatch superseding it (auto-solve disabled, or this design's own budget
        // was already exceeded) -- the edit that changed it already repainted the
        // banner correctly via `refresh_editor_panel_stale`. Only the running flag
        // above needed clearing.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    if result.panicked {
        // `result`'s own tiers/planes/etc. are empty placeholders, not a
        // real solve -- pushing them would blank out whatever the panel
        // already showed. Report the failure and stop; `solve_running` above
        // is already cleared, which is all "Abandon Solve"'s own reset does
        // for this case too.
        ui.global::<EditorModel>()
            .set_status_text(result.status_text.clone().into());
        ui.global::<EditorModel>().set_status_is_problem(true);
        ui.global::<EditorModel>().set_solve_state("failed".into());
        show_toast(ui, &result.status_text, "error");
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    maybe_toast_too_many_planes(ui, &result);
    // Feeds the Baseline strip with the masts just solved (it scores them, never solves).
    if let Some(solved) = result.solved.as_deref() {
        crate::gui::editor::baseline::on_solved(&result.design, solved, started_generation);
    }

    // Runs before any field of `result` is moved out below, since it borrows `result`.
    push_solve_dependent_background_fields(ui, &result);
    push_tiers(ui, result.tiers);
    push_multi_selected_count(ui, result.multi_selected_count);
    ui.global::<EditorModel>()
        .set_status_text(result.status_text.into());
    ui.global::<EditorModel>()
        .set_status_is_problem(result.status_is_problem);
    // Below the staleness early returns above, same as the line it follows: a
    // superseded run must not relabel the strip for a design it does not describe.
    ui.global::<EditorModel>().set_solve_state(
        if result.status_is_problem {
            "failed"
        } else {
            "solved"
        }
        .into(),
    );
    ui.global::<EditorModel>()
        .set_last_solve_duration_ms(i32::try_from(result.elapsed.as_millis()).unwrap_or(i32::MAX));
    // The worked-example guide's "Solve and check" step may complete here. The
    // generation check above confirms `result.design` is still on screen.
    crate::gui::editor::guide::check_design_progress(ui, &result.design);
    // `result.warnings` is `(tier index, text)` pairs -- see
    // `PanelInputs::warnings`'s own doc comment.
    let warning_tiers: Vec<i32> = result
        .warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(
            result
                .warnings
                .into_iter()
                .map(|(_, text)| SharedString::from(text))
                .collect::<Vec<_>>(),
        )));
    ui.global::<EditorModel>()
        .set_manufacturability_warning_tiers(ModelRc::new(VecModel::from(warning_tiers)));
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = result.yield_texts;
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());
    // The Deep Solve/Optimize availability flags depend only on state a solve cannot
    // change, and the generation check above confirms no edit touched them.

    if !push_viewport_after_background_solve(
        ui,
        render_ctx,
        started_generation,
        StoneGeometryBuf {
            planes: result.planes,
            tools: result.tools,
            placements: result.placements,
            frosted: result.frosted,
        },
        result.gear,
        result.solved,
        &result.design,
    ) {
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    dispatch_pending(ui, render_ctx, pending);
}

/// The viewport, solid-preview redraw and tilt-staleness tail of
/// [`apply_background_solve_result`]. Returns `false` when `started_generation` no
/// longer owns `render_ctx`'s shared plane slot, in which case the caller skips the
/// rest of the pipeline.
///
/// `stone` is the finished design's geometry. While the Cut slider has the design cut
/// back, the stone at that cut is pushed instead ([`cut_slider::stone_at_cut`]), to the
/// path tracer and the solid raster alike.
fn push_viewport_after_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    started_generation: u64,
    stone: StoneGeometryBuf,
    gear: (u32, f32),
    solved: Option<Vec<SolvedTier>>,
    design: &Design,
) -> bool {
    let stone = cut_slider::stone_at_cut(
        stone,
        design,
        solved.as_deref(),
        cut_slider::sync_model(ui, design),
    );
    let planes = Arc::new(stone.planes);
    // The surface finish of the planes about to be claimed (empty for a design without one).
    let surface_words = surface_table(design, solved.as_deref());
    // The first result after a New/Load/Open frames the stone unless the cutter has
    // zoomed, as `view::refresh_viewport` does. Measured before the lock: it builds the
    // stone's solid.
    let framing = if super::scheduling::frame_pending() {
        fit_view::auto_frame_distance(ui, render_ctx, &planes)
    } else {
        None
    };
    let mut ctx = RenderContext::lock(render_ctx);
    // A Library pick or a job view made while this solve ran keeps the viewport. The
    // editor's next load or edit claims it back, so this claim must not override it.
    if ctx.library_took_over_since(started_generation) {
        return false;
    }
    // `started_generation`, not the live counter: a result for an older design must not
    // outrank a newer edit's claim (`may_claim_active_planes` compares them).
    if !ctx.claim_active_geometry(
        Arc::clone(&planes),
        Arc::new(stone.tools.clone()),
        stone.placements.clone(),
        Some(gear),
        PlanesOwner::Editor {
            generation: started_generation,
        },
    ) {
        return false;
    }
    ctx.claim_active_frosted(stone.frosted.clone());
    ctx.claim_active_surfaces(surface_words);
    // Re-checked under the lock: the cutter may have zoomed while the stone was measured.
    if let Some(distance) = framing
        && fit_view::is_unzoomed(ctx.distance)
    {
        ctx.distance = distance;
        fit_view::note_auto_framed(distance);
    }
    super::scheduling::clear_frame_pending();
    ctx.dirty = true;
    let design_gear = ctx.design_gear;
    // Live Render shows a "Cut: ..." pill only while the picture is the editor's design.
    crate::gui::editor::push_viewport_owner(ui, ctx.planes_owner);
    let camera = CameraPose {
        roll: ctx.camera_roll,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
        pan: ctx.camera_pan,
    };
    let render_size = (ctx.width, ctx.height);
    drop(ctx);
    let redraw_stone = StoneGeometryBuf {
        planes: planes.as_ref().clone(),
        tools: stone.tools,
        placements: stone.placements,
        frosted: stone.frosted,
    };
    let view_mode = view_mode_index(ui.global::<SolidPreviewModel>().get_view_mode());
    // The redraw must use the letterboxed size for the Path-traced/Both views, as
    // `camera_lighting::contained_request_size` describes. A raw viewport size here
    // would misregister picking after every background solve.
    let size = planned_frame_size(ui, view_mode, render_size);
    let (preview_state, solid_last_solved) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        (rt.preview_state.clone(), rt.solid_last_solved.clone())
    });
    if let Some(preview_state) = preview_state {
        preview_state.request_redraw_geometry(redraw_stone, camera, size, view_mode, design_gear);
    }
    let solved_fresh = solved.is_some();
    if let (Some(solved), Some(cache)) = (solved, solid_last_solved.clone()) {
        // Stamped with `started_generation`: the check above confirmed it names the design
        // this solve ran against.
        *cache.lock().unwrap_or_else(PoisonError::into_inner) = Some((started_generation, solved));
    }
    // The redraw above is a `Reproject`, which carries no `Design` and clears the
    // hover/label tables. Plan this design too, after the cache write so the fresh masts
    // chain; a full solve runs only when this solve left none.
    if let (Some(preview_state), Some(cache)) = (
        RUNTIME.with(|cell| cell.borrow().preview_state.clone()),
        solid_last_solved,
    ) {
        let multi_selected = RUNTIME
            .with(|cell| cell.borrow().editor_state.clone())
            .and_then(|state| state.try_borrow().ok().map(|st| st.multi_selected.clone()))
            .unwrap_or_default();
        submit_preview_replan_for(
            ui,
            render_ctx,
            &preview_state,
            &cache,
            ReplanSource {
                design,
                generation: started_generation,
                multi_selected: &multi_selected,
            },
            BTreeSet::new(),
            !solved_fresh,
        );
    }

    // The plane slot now holds this design's geometry, so the material name cached for
    // the tilt dialog does not apply. See `view::refresh_viewport`.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // If the tilt dialog is open, request a fresh sweep. `AxesCacheKey` hashes the
    // planes, so this does nothing when the planes did not change.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
    true
}

/// The proportions, girdle/ratio, preform-mm, facet-count and cutting-instructions
/// half of [`apply_background_solve_result`]. Background solves are the normal path for
/// real designs, so these fields must be pushed here too. `result.solved` is the same
/// single solve the dispatch paid for, so each helper uses its `_from_solved` form.
fn push_solve_dependent_background_fields(ui: &MainWindow, result: &BackgroundSolveResult) {
    let solved_slice = result.solved.as_deref();
    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) = solved_slice
        .map_or_else(
            || proportions_texts(&result.design),
            |solved| proportions_texts_from_solved(&result.design, solved),
        );
    ui.global::<EditorModel>()
        .set_proportions_table_pct(table_pct.into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height(crown_height.into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth(pavilion_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth(total_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width(length_to_width.into());

    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) = solved_slice
        .map_or_else(
            || girdle_and_ratio_texts(&result.design),
            |solved| girdle_and_ratio_texts_from_solved(&result.design, solved),
        );
    ui.global::<EditorModel>()
        .set_girdle_thickness_text(girdle_thickness.into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text(crown_to_width.into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text(pavilion_to_width.into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text(girdle_to_width.into());

    let (preform_half_width_mm, preform_depth_mm) = solved_slice.map_or_else(
        || preform_mm_texts(&result.design),
        |solved| preform_mm_texts_from_solved(&result.design, solved),
    );
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text(preform_half_width_mm.into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text(preform_depth_mm.into());
    let mm_per_unit =
        solved_slice.and_then(|solved| result.design.yield_report(solved).mm_per_unit);
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(result.design.preform_y_offset, mm_per_unit).into(),
    );

    let facet_count = solved_slice.map_or(0, |solved| {
        i32::try_from(facet_count_from_solved(&result.design, solved)).unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);

    if let Some(solved) = solved_slice {
        let rows: Vec<AngleItem> = cutting_instructions_rows(&result.design, solved);
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(rows)));
    } else {
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    }

    // See `view::panel::push_proportion_verdicts_from_solved`.
    push_proportion_verdicts_from_solved(ui, &result.design, solved_slice, result.n_d);
}

/// Re-dispatches whatever `super::runtime::Runtime::pending_dispatch`
/// [`apply_background_solve_result`] took, if any -- the "dispatch once on
/// completion" half of the queue-instead-of- spawn-a-second-worker mechanism. A
/// no-op when nothing queued up while the just-finished solve was running.
fn dispatch_pending(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pending: Option<PendingDispatch>,
) {
    if let Some(pending) = pending {
        super::dispatch::dispatch_background_solve(
            ui,
            render_ctx,
            pending.design,
            &pending.generation,
            pending.multi_selected,
            pending.started_generation,
        );
    }
}
