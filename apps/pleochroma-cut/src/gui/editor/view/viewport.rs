//! Feeds the shared viewport (GPU path-traced and solid preview).
//!
//! It holds the [`refresh_all_now`]/[`refresh_all`] entry points, the trace-staleness
//! marker, and the replan-on-edit path ([`submit_preview_replan`], [`submit_preview_replan_for`],
//! [`push_solved_preview`]) used by the solid-preview worker thread and its completion
//! callback.

mod replan;

use super::{
    SolidLastSolved, panel::refresh_editor_panel_from_solve, panel_stale::push_stale_content,
    state::EditorState,
};
use crate::{
    CuttingModeModel, EditorModel, MainWindow, SolidPreviewModel, TiltModel,
    bridge::{
        render_effects::surface_table,
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::{
        editor::auto_solve,
        render::{
            camera_lighting::{contained_request_size, device_pixels, view_mode_index},
            fit_view::{self, auto_frame_distance},
        },
        solid_preview::{
            cut_slider,
            preview_state::{CameraPose, SolidPreviewState},
        },
        tutorial_events::raise,
    },
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    guide::viewing_events as events,
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
};
use pleochroma_solid::preview::StoneGeometryBuf;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
    time::{Duration, Instant},
};

// The replan-on-edit request path lives in `replan`; these are its entry points.
pub(in crate::gui::editor) use replan::{
    ReplanSource, push_late_findings, push_solved_preview, submit_preview_replan,
    submit_preview_replan_chained, submit_preview_replan_for,
};

/// The Solid/Diagram viewport's size in PHYSICAL pixels, for a raster or pick request.
///
/// `SolidPreviewModel.viewport_width`/`viewport_height` are LOGICAL (DIP) sizes. A request
/// built from them rasterizes at one device pixel per logical pixel, which is soft on
/// `HiDPI` displays, so the window's scale factor is applied here. The pointer position in
/// `callbacks::tier_actions`' hover and click handling is multiplied by the same factor to
/// match this pick buffer. `solid_preview::diagram_wiring` does the same for the diagram.
fn scaled_viewport_size(ui: &MainWindow) -> (u32, u32) {
    let scale = ui.window().scale_factor();
    (
        device_pixels(ui.global::<SolidPreviewModel>().get_viewport_width() * scale),
        device_pixels(ui.global::<SolidPreviewModel>().get_viewport_height() * scale),
    )
}

/// The Live Render tab's viewport size in PHYSICAL pixels, from
/// `SolidPreviewModel.live_viewport_width`/`live_viewport_height` (pushed by
/// `gem_viewport.slint`), scaled as in [`scaled_viewport_size`].
fn scaled_live_viewport_size(ui: &MainWindow) -> (u32, u32) {
    let scale = ui.window().scale_factor();
    (
        device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_width() * scale),
        device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_height() * scale),
    )
}

/// The size, in physical pixels, a planned or redrawn solid frame is requested at.
///
/// The frame lands in the ONE shared `SolidPreviewModel.image`, which both the Edit
/// viewport and the Live Render tab show (`image-fit: contain`). While the Live tab is up
/// (`render_view_tab == 0`) the Edit viewport is not mounted, so its size is stale. A frame
/// sized from it would be scaled up into the Live tab, and its straight edges would cut the
/// gem. The Live tab therefore requests its own size, un-letterboxed: it always shows the
/// flat Solid raster, never the traced image. Cutting mode is the exception, since its
/// full-window screen sizes the render through the Edit properties whatever tab is below.
///
/// Same rule as the Live branch of `camera_lighting::resubmit_at_current_pose`. Edit-tab
/// callers get [`contained_request_size`] over the Edit viewport.
pub(in crate::gui::editor) fn planned_frame_size(
    ui: &MainWindow,
    view_mode: u8,
    render_size: (u32, u32),
) -> (u32, u32) {
    if ui.get_render_view_tab() == 0 && !ui.global::<CuttingModeModel>().get_open() {
        return scaled_live_viewport_size(ui);
    }
    contained_request_size(view_mode, scaled_viewport_size(ui), render_size)
}

/// Tells the viewport whether the path-traced image still describes the design on the bench.
///
/// Compares the generation stamped on the planes the render thread holds against `state`'s
/// live generation. Pushed from both the stale path (every edit, which makes it true) and
/// the refresh path (a real solve re-pushes the planes, which makes it false), so the marker
/// never outlives the condition it reports.
///
/// `pub(super)`: [`super::panel_stale::push_stale_content`] shares this push.
pub(super) fn push_trace_staleness(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let generation = state.generation.load(std::sync::atomic::Ordering::Relaxed);
    let planes_stale = {
        let ctx = RenderContext::lock(render_ctx);
        ctx.traced_planes_are_stale(generation)
    };
    ui.global::<crate::ViewportModel>()
        .set_trace_stale(planes_stale);
}

/// Redraws the viewport from `state`, claiming its planes and concave tools.
///
/// `solved` is the solve [`super::panel::refresh_editor_panel`] already computed, so this
/// never runs a second `Design::solve()`. It is `None` only for a design that does not
/// solve at all (a `MissingAnchor`, most often). The stone is then drawn without masts,
/// apart from the rough under the Cut slider, and the failed solve is not retried here.
fn refresh_viewport(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    solved: Option<&[SolvedTier]>,
    wholesale: bool,
) {
    // The real design is about to be drawn: a ghost still being solved must not land
    // over it.
    super::optimize_apply::cancel_ghost_preview();
    // The stone the Cut slider shows, which is not always the finished one. The planes and
    // concave tools of one solve are claimed together (the tools are empty for a planar
    // design), so the tracer never sees one without the other. Built before the render
    // context is locked. It never solves: without `solved` the design did not solve, and a
    // retry would fail again on the UI thread.
    let cut_steps = cut_slider::sync_model(ui, &state.design);
    let StoneGeometryBuf {
        planes: planes_for_claim,
        tools: tools_for_claim,
        placements: placements_for_claim,
        frosted: frosted_for_claim,
    } = cut_slider::cut_geometry_no_solve(&state.design, solved, cut_steps);
    // The surface finish of the facets just built (empty for every design without one).
    let surfaces_for_claim = surface_table(&state.design, solved);
    // A newly opened, loaded or created design is framed unless the cutter has zoomed. It
    // is measured before the render context is locked, since that builds the stone's
    // solid. An ordinary Solve keeps the camera where it is.
    let framing = if wholesale {
        auto_frame_distance(ui, render_ctx, &planes_for_claim)
    } else {
        None
    };
    let mut ctx = RenderContext::lock(render_ctx);
    // The editor always wins a claim, so this cannot fail. It still uses the single write
    // path shared by every writer, which keeps the ownership stamp true: a later catalogue
    // click must not silently replace the design under the cutter's hands.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the GPU/storage precision of the gear reference angle (degrees)"
    )]
    let gear_reference_angle = state.design.meta.gear_reference_angle as f32;
    let claimed = ctx.claim_active_geometry(
        std::sync::Arc::new(planes_for_claim),
        std::sync::Arc::new(tools_for_claim),
        placements_for_claim,
        Some((state.design.meta.gear_teeth_abs(), gear_reference_angle)),
        PlanesOwner::Editor {
            generation: state.generation.load(std::sync::atomic::Ordering::Relaxed),
        },
    );
    if claimed {
        // The frosted planes of the stone just claimed (empty for every unfrosted design).
        ctx.claim_active_frosted(frosted_for_claim);
        ctx.claim_active_surfaces(surfaces_for_claim);
        // Re-checked under the lock: the cutter may have zoomed while the stone was measured.
        if let Some(distance) = framing
            && fit_view::is_unzoomed(ctx.distance)
        {
            ctx.distance = distance;
            fit_view::note_auto_framed(distance);
        }
        if wholesale {
            // Framed here: the background solve's first result must not frame again.
            auto_solve::clear_frame_pending();
        }
    }
    ctx.dirty = true;
    let design_gear = ctx.design_gear;
    // Live Render shows a "Cut: ..." pill only while the picture is the editor's design.
    crate::gui::editor::push_viewport_owner(ui, ctx.planes_owner);
    let stone = StoneGeometryBuf {
        planes: ctx.active_planes.as_ref().clone(),
        tools: ctx.active_tools.as_ref().clone(),
        placements: ctx.active_placements.clone(),
        frosted: ctx.active_frosted.as_ref().clone(),
    };
    let view_mode = view_mode_index(ui.global::<SolidPreviewModel>().get_view_mode());
    // Letterboxed to the traced image's rectangle in Path-traced/Both, as
    // `camera_lighting::resubmit_at_current_pose` does for camera drags and view-mode
    // switches (see `contained_request_size`).
    let size = planned_frame_size(ui, view_mode, (ctx.width, ctx.height));
    preview_state.request_redraw_geometry(
        stone,
        CameraPose {
            roll: ctx.camera_roll,
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
        },
        size,
        view_mode,
        design_gear,
    );
    drop(ctx);

    // Cache this solve's masts, so the next edit's `submit_preview_replan` can chain a
    // subgraph `resolve_dirty` solve instead of a full one. Stored before the tilt sweep
    // below, which reads the cache for the finished stone (`gui::editor::finished_stone`)
    // and would otherwise find the previous generation's masts.
    if let Some(solved) = solved {
        *solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((
            state.generation.load(AtomicOrdering::Relaxed),
            solved.to_vec(),
        ));
    }
    // The redraw above is a `Reproject`, which carries no `Design` and clears the hover and
    // label tables. Only a `Planned` frame refills them, so plan this design here (after the
    // cache write, so the fresh masts chain; a full solve only if this solve left none).
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        BTreeSet::new(),
        solved.is_none(),
    );

    // The plane slot belongs to `state.design`. A material name cached by its previous
    // occupant (a catalogue preview load) does not apply to this design, so it is cleared.
    // That keeps "no cached curve for this design" representable.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // Geometry changed under an open tilt dialog, so request a fresh sweep to keep its curves
    // from describing the old stone. `AxesCacheKey` (tilt_profile.rs) hashes the planes, so
    // this is a no-op when they did not move.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
    // The tracer's planes carry this generation, so the trace-staleness marker clears.
    push_trace_staleness(ui, render_ctx, state);
}

/// Refreshes like [`refresh_all`], but paints "Solving" before a synchronous solve.
///
/// Callers that replace or re-solve the design (in `callbacks::tier_actions`,
/// `callbacks::solve_actions` and `callbacks::retarget_actions`) use this. Other edit
/// callbacks call [`super::panel_stale::refresh_editor_panel_stale`] and leave the
/// viewport untouched.
///
/// `wholesale` is `true` only when `EditorState` was replaced wholesale (New, Load
/// Selected, Open); see [`auto_solve::reset_for_new_design`]. Other callers pass `false`,
/// so the last measured solve time may veto an inline solve.
///
/// A design that is cheap enough (see [`solves_inline`]) is solved here on the UI thread.
/// Any other design gets the stale content [`super::panel_stale::refresh_editor_panel_stale`]
/// pushes after an edit, plus a background solve dispatched at once by
/// [`auto_solve::dispatch_background_solve`].
///
/// `state` is the shared `Rc<RefCell<EditorState>>`, so the deferred call can reach it.
///
/// # Paint first
///
/// [`refresh_all`]'s inline branch runs the blocking solve without yielding to the event
/// loop, so the toolkit would only paint the final state. This function makes the same
/// [`solves_inline`] decision. When the inline branch will be taken, it sets the solving
/// flags now and defers the same [`refresh_all`] call by a zero-delay timer, so the frame
/// paints first. The background branch does not block and runs at once.
pub(in crate::gui::editor) fn refresh_all_now(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
    wholesale: bool,
) {
    // `refresh_all` makes the same decision and runs `auto_solve::reset_for_new_design`,
    // which must happen exactly once, so that call is left to the real refresh below.
    if wholesale {
        // The inline branch defers `refresh_all` by an event-loop turn: reset the Cut
        // slider now so the old design's cut is not on screen for that turn.
        cut_slider::reset_to_finished(ui, &state.borrow().design);
    }
    let inline = solves_inline(&state.borrow().design, wholesale);
    if !inline {
        let st = state.borrow();
        refresh_all(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            &st,
            wholesale,
        );
        return;
    }
    // Paints "Solving..." now; the solve itself runs on the next event-loop turn.
    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>().set_solve_state("solving".into());
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let state = Rc::clone(state);
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let st = state.borrow();
        refresh_all(
            &ui,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &st,
            wholesale,
        );
    });
}

/// Gives the shared viewport back to the editor's design when the Edit sub-tab is entered
/// while a Library selection owns it (see [`RenderContext::may_claim_active_planes`]).
/// Does nothing when the editor already owns the viewport or has no design. Otherwise it
/// runs the same refresh as a "Solve" click, which also restores the editor's material and
/// panel.
pub(in crate::gui::editor) fn reclaim_viewport_for_editor(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    let catalogue_owns = {
        let mut ctx = RenderContext::lock(render_ctx);
        let owns = matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. });
        if owns && state.has_design {
            // Restored at once, not when a (possibly background) solve claims the planes, so
            // a Library row without a material does not leave tracing suspended.
            ctx.restore_editor_material();
        }
        owns
    };
    if catalogue_owns && state.has_design {
        refresh_all(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            state,
            false,
        );
        let name = RenderContext::lock(render_ctx).material_name.clone();
        let options = ui.global::<crate::ViewportModel>().get_material_options();
        if let Some(index) = crate::gui::startup_settings::find_option_index(&options, &name) {
            ui.global::<crate::ViewportModel>()
                .set_selected_material_index(index);
        }
    }
}

/// Mirrors [`EditorState::has_design`] into `EditorModel.has_design`, the only setter for
/// that flag. Called by [`refresh_all`] and directly by the install paths
/// (`do_new_design_create`, `apply_loaded_design`, `native_io::finish_state_replace`),
/// since [`refresh_all_now`] may defer the refresh by one event-loop turn.
pub(in crate::gui::editor) fn push_has_design(ui: &MainWindow, state: &EditorState) {
    ui.global::<EditorModel>().set_has_design(state.has_design);
}

/// Whether `design` may be solved inline on the UI thread, by
/// [`should_solve_synchronously_for`] over its solve-free cost estimate.
///
/// A `wholesale` refresh passes no measurement, since the previous design's solve time
/// says nothing about this one. Otherwise the last measured solve time of this design may
/// veto an inline solve.
///
/// Designs with concave tools never solve inline. The cost estimate counts flat planes
/// only, but the manufacturability pass carves every tool out of the solid, which is too
/// slow for the UI thread. Those designs take the background solve.
fn solves_inline(design: &Design, wholesale: bool) -> bool {
    if !design.concave_tiers.is_empty() {
        return false;
    }
    let last_solve = if wholesale {
        None
    } else {
        auto_solve::last_solve()
    };
    should_solve_synchronously_for(SolveCostEstimate::of(design), last_solve)
}

/// Refreshes the panel and the viewport from `state` -- solving synchronously when
/// that is cheap, otherwise pushing stale content and dispatching a background solve
/// -- then lets the worked-example guide check whether its current step's goal was
/// just reached (`guide::check_progress`).
pub(in crate::gui::editor) fn refresh_all(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    wholesale: bool,
) {
    if wholesale {
        // A wholesale replacement is solved against a different design than the one
        // last measured, so the stored measurement (`auto_solve::last_solve`) is cleared
        // unconditionally. The inline branch overwrites it; the background branch leaves
        // it `None` until that solve completes, which `auto_solve::should_schedule_auto_solve`
        // reads as "try auto-solve".
        auto_solve::reset_for_new_design(state.generation.load(AtomicOrdering::Relaxed));
        // A new, opened or loaded design starts uncut: the Cut slider of the design it
        // replaced must not keep its label (or truncate the view) for this one.
        cut_slider::reset_to_finished(ui, &state.design);
        // The lighting this design remembered (or the normal lighting, if it has none)
        // replaces the one the design before it left showing.
        crate::gui::render::design_lighting::design_opened(
            ui,
            state.design_uuid(),
            state.has_design,
        );
    }
    if solves_inline(&state.design, wholesale) {
        // Sets the same `solve_running`/`solve_state` signal the background solve uses.
        // Nothing repaints during this call, so the visible wait comes from the deferred
        // path in [`refresh_all_now`].
        ui.global::<EditorModel>().set_solve_running(true);
        ui.global::<EditorModel>().set_solve_state("solving".into());
        // Timed around `Design::solve()` alone, so it is comparable with the background
        // timer in `auto_solve::dispatch_background_solve` that `should_schedule_auto_solve`
        // compares against. Panel UI pushes are excluded for the same reason.
        //
        // This is the only solve on this path: `refresh_editor_panel_from_solve` takes the
        // result instead of solving again.
        let start = Instant::now();
        let solved_result = state.design.solve();
        let elapsed = start.elapsed();
        auto_solve::record_solve_duration(elapsed);
        // The figure shown is the one the auto-solve budget decides on.
        ui.global::<EditorModel>()
            .set_last_solve_duration_ms(i32::try_from(elapsed.as_millis()).unwrap_or(i32::MAX));
        // Handed to `refresh_viewport`, which therefore does not solve again.
        let solved = refresh_editor_panel_from_solve(ui, render_ctx, state, solved_result);
        refresh_viewport(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            state,
            solved.as_deref(),
            wholesale,
        );
        // `refresh_editor_panel_from_solve` has already set `solve_state` to
        // "solved"/"failed" -- only the running flag still needs clearing.
        ui.global::<EditorModel>().set_solve_running(false);
    } else {
        // Too expensive to solve inline. The cached `solid_last_solved` may describe the
        // design that was just replaced, or may sit behind a partial subgraph resolve, so
        // every tier is marked dirty rather than trusting a length match.
        let all_dirty: BTreeSet<usize> = (0..state.design.tiers.len()).collect();
        push_stale_content(ui, render_ctx, state, &all_dirty);
        auto_solve::dispatch_background_solve(
            ui,
            render_ctx,
            state.design.clone(),
            &state.generation,
            state.multi_selected.clone(),
            state.generation.load(AtomicOrdering::Relaxed),
        );
    }
    push_has_design(ui, state);
    if wholesale {
        // New Design, Load Selected and Open end here: a tutorial step may wait for a
        // different design to replace the open one.
        raise(ui, events::DESIGN_REPLACED);
    }
    crate::gui::editor::guide::check_progress(ui, state);
}
