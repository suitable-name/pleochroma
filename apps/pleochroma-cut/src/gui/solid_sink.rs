//! The real [`PreviewSink`] that hops a finished solid-preview frame back onto the
//! UI thread, plus the trace/solid plane-arrangement agreement check it feeds.

mod late_rows;

use crate::{
    MainWindow, SolidPreviewModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor,
        solid_preview::{
            cut_slider,
            diagram_wiring::{DiagramFacetOwners, DiagramHoverText, DiagramPick},
            facet_selection,
            preview_state::{
                FacetOwners, FrameGeometry, LateFindings, PickBuffer, PreviewFrame, PreviewSink,
                SolidLastSolved, SolidPreviewState,
            },
        },
        view_modes::EditViewMode,
    },
};
use late_rows::LateRows;
use pleochroma::geometry::{ToolPrimitive, meet_solver::SolvedTier, plane::GpuFacetPlane};
use pleochroma_cut_core::ManufacturabilityWarning;
use slint::{ComponentHandle, Weak};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::Duration,
};

thread_local! {
    /// Which committed frames have pushed their rows, and findings that arrived before their
    /// frame -- see [`LateRows`]. Only ever touched inside [`PreviewSink`]'s
    /// `upgrade_in_event_loop` closures, so it lives on the UI thread.
    static LATE_ROWS: RefCell<LateRows> = RefCell::default();
}

/// The real [`PreviewSink`]. Hops a finished solid-preview frame to the UI thread, where it
/// is pushed into `SolidPreviewModel`.
///
/// `pick`, `last_solved`, the Diagram tables and the hover tables are plain `Arc<Mutex<..>>`
/// state, not Slint properties, so the editor's and the Diagram's hover and click callbacks
/// and the next replan read them without a window handle. They are stored on the UI thread
/// inside [`PreviewSink::apply`]'s `upgrade_in_event_loop` closure, together with the image
/// swap: a click on the image still on screen must resolve against that image's own pick
/// buffer. See `preview_state`'s "Where `last_solved` lives" for why they are not on
/// `gui::editor::state::EditorState`.
#[derive(Clone)]
pub(super) struct SlintSolidSink {
    pub(super) ui: Weak<MainWindow>,
    pub(super) pick: Arc<Mutex<Option<PickBuffer>>>,
    pub(super) last_solved: SolidLastSolved,
    // Diagram mode's (view_mode 3) pick and hover tables. The diagram has its own pixel
    // layout, so they stay apart from `pick` (see `solid_preview::diagram_wiring`).
    pub(super) diagram_pick: DiagramPick,
    pub(super) diagram_hover_text: DiagramHoverText,
    pub(super) diagram_facet_owners: DiagramFacetOwners,
    /// The index wheel's per-pixel tooth-picking buffer (`PreviewFrame::diagram_tooth_pick`).
    /// Separate from `diagram_pick` because the diagram's pixel layout differs.
    pub(super) diagram_tooth_pick: DiagramPick,
    /// The diagram's per-pixel panel buffer (`PreviewFrame::diagram_panel_pick`),
    /// which the double-click that enlarges a panel reads. Same shape and lifetime
    /// as `diagram_tooth_pick`.
    pub(super) diagram_panel_pick: DiagramPick,
    /// The Solid view's facet id -> hover tooltip text, from every frame's
    /// [`PreviewFrame::hover_text`]. The solid picking callbacks read it through
    /// `SolidPickState`, so they need no `FacetMap` per mouse move.
    pub(super) hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's own facet id -> owning flat and concave tier tables, from every
    /// frame's [`PreviewFrame::facet_owners`] -- see `hover_text`'s doc comment.
    pub(super) facet_owners: Arc<Mutex<FacetOwners>>,
    /// The shared render context.
    ///
    /// A finished frame's [`PreviewFrame::planes`] are published into
    /// `RenderContext::active_planes` when they differ by more than
    /// `PLANE_REPUBLISH_EPSILON`, so a camera orbit after a live edit re-issues the
    /// edited geometry instead of the older one.
    ///
    /// Also read, by `active_planes` pointer identity, to decide whether this frame still
    /// matches the path tracer's last push (see `solid_active_planes`/`trace_active_planes`).
    pub(super) render_ctx: Arc<Mutex<RenderContext>>,
    /// The plane arrangement (`RenderContext::active_planes`, by `Arc`
    /// pointer identity) this sink last published a solid-preview frame with.
    /// Compared against `trace_active_planes` (written by the path tracer's own
    /// frame-push closure in [`super::main_window::build_main_window`]) to keep
    /// `SolidPreviewModel.trace_matches_solid` current -- see
    /// [`planes_generations_match`].
    pub(super) solid_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The path tracer's own last-pushed plane arrangement -- see
    /// `solid_active_planes`'s doc comment.
    pub(super) trace_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The loaded solid's bounding radius (`PreviewFrame::mesh_bounding_radius`). The orbit
    /// zoom clamp and the "Fit" pose read it through `render::camera_lighting`; only this
    /// sink writes it.
    pub(super) mesh_bounding_radius: Arc<Mutex<f64>>,
    /// The mesh geometry, camera pose and raster size of the frame on screen
    /// (`PreviewFrame::geometry`), stored inside the same UI-thread closure that
    /// swaps `pick` -- so the manipulation handles are always projected with the
    /// pose and size of the pick buffer a click resolves against. Shared with
    /// `SolidPickState::geometry`.
    pub(super) geometry: Arc<Mutex<Option<FrameGeometry>>>,
    /// The controller this sink belongs to, filled in right after the controller is built
    /// (it takes the sink in its constructor, so the sink cannot hold it any earlier). Read
    /// only by [`heal_missing_diagram`], which asks it for a redraw at the current pose.
    pub(super) preview_state: Arc<Mutex<std::sync::Weak<SolidPreviewState>>>,
}

thread_local! {
    /// How many frames in a row [`heal_missing_diagram`] has asked to have redrawn; reset by
    /// the first frame that shows the Diagram (or by leaving the Diagram view). UI-thread-only.
    static DIAGRAM_HEALS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// How many redraws in a row [`heal_missing_diagram`] may ask for before it gives up and
/// leaves the picture as it is: the next ordinary redraw (a camera move, an edit, the next
/// view-mode click) draws the Diagram again, but a defect that made every redraw come back
/// without one must not turn into an endless redraw loop.
const MAX_DIAGRAM_HEALS: u32 = 3;

/// Asks the render worker for one more redraw at the current pose when a frame lands on the
/// Diagram view without a Diagram while its stone closed.
///
/// Such a frame was drawn at another view mode (a request made before the mode changed).
/// The Diagram view shows only its "Solve to preview" placeholder without a Diagram frame,
/// and nothing else asks again. A frame with a status banner is not this case: its missing
/// Diagram has a reason (`frame_closed` requires a solid and no status).
///
/// The redraw is a camera-follow `Reproject` at the window's view mode, which also restores
/// the worker's remembered mode to the Diagram. Bounded by [`MAX_DIAGRAM_HEALS`].
fn heal_missing_diagram(
    ui: &MainWindow,
    sink: &SlintSolidSink,
    frame_has_diagram: bool,
    frame_closed: bool,
) {
    let wants_diagram = ui.get_render_view_tab() != 0
        && EditViewMode::from_raw(ui.global::<SolidPreviewModel>().get_view_mode())
            == EditViewMode::Diagram;
    if !wants_diagram || frame_has_diagram {
        DIAGRAM_HEALS.set(0);
        return;
    }
    if !frame_closed || DIAGRAM_HEALS.get() >= MAX_DIAGRAM_HEALS {
        return;
    }
    let Some(preview_state) = sink
        .preview_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upgrade()
    else {
        return;
    };
    DIAGRAM_HEALS.set(DIAGRAM_HEALS.get() + 1);
    let ctx = RenderContext::lock(&sink.render_ctx);
    crate::gui::render::camera_lighting::resubmit_at_current_pose(ui, &ctx, &preview_state);
}

/// Whether the last path-traced frame and the last solid-preview frame were produced from
/// the same plane arrangement, compared by `Arc` pointer identity of
/// `RenderContext::active_planes` at push time. Not by value: two different designs can
/// solve to identical planes, and a pointer compare is all either frame-push closure can
/// afford on the UI thread.
///
/// `true` while either side has no frame yet, as `SolidPreviewModel.trace_matches_solid`
/// defaults.
pub(super) fn planes_generations_match(
    trace: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    solid: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
) -> bool {
    let trace = trace
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let solid = solid
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match (trace, solid) {
        (Some(trace), Some(solid)) => Arc::ptr_eq(&trace, &solid),
        _ => true,
    }
}

/// Largest per-component difference two plane arrangements may show and still count as the
/// same geometry for `RenderContext::active_planes`. `GpuFacetPlane::new` normalises in `f32`
/// and is not idempotent (a round trip can move a normal by one ULP), so an exact compare
/// would republish on nearly every camera-follow frame. Anything a solver can distinguish
/// is far above this.
const PLANE_REPUBLISH_EPSILON: f32 = 1e-5;

/// Whether `a` and `b` describe the same plane arrangement within `epsilon` per
/// component (same length, same order; `normal[0..3]` and `d`).
#[must_use]
fn planes_equal_within(a: &[GpuFacetPlane], b: &[GpuFacetPlane], epsilon: f32) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(left, right)| {
            (left.d - right.d).abs() <= epsilon
                && left
                    .normal
                    .iter()
                    .zip(&right.normal)
                    .all(|(l, r)| (l - r).abs() <= epsilon)
        })
}

/// The stone a solid-preview frame was rendered from, as
/// [`sync_planes_and_check_trace_match`] publishes it: the `(normal, offset)` planes, the
/// concave tools cut out of them and each tool's `(tier, placement)`.
#[derive(Clone, Copy)]
struct StonePublish<'a> {
    planes: &'a [(glam::Vec3, f32)],
    tools: &'a [ToolPrimitive],
    placements: &'a [(usize, usize)],
}

impl<'a> StonePublish<'a> {
    const fn new(
        planes: &'a [(glam::Vec3, f32)],
        tools: &'a [ToolPrimitive],
        placements: &'a [(usize, usize)],
    ) -> Self {
        Self {
            planes,
            tools,
            placements,
        }
    }
}

/// Publishes `stone`'s planes and concave tools into `render_ctx` when they differ from the
/// current `active_planes` (planes within [`PLANE_REPUBLISH_EPSILON`]; tools exactly, as
/// they are copied and never re-normalised). Empty planes are skipped, so the default
/// placeholder cut is never clobbered. Then the resulting arrangement is stored in
/// `solid_active_planes`, and the return says whether it agrees with `trace_active_planes`.
///
/// The write is tagged `PlanesOwner::Editor { generation }`, the same arbitration
/// `editor::auto_solve::apply_background_solve_result` uses, so a frame behind the live
/// design cannot overwrite a newer claim. A `generation` of `0` (no replan has landed for
/// the live design, see [`PreviewFrame::generation`]) is never published.
/// Split out of [`SlintSolidSink::apply`] to keep it under the function-length lint.
fn sync_planes_and_check_trace_match(
    stone: &StonePublish<'_>,
    render_ctx: &Mutex<RenderContext>,
    solid_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    trace_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    generation: u64,
) -> bool {
    let StonePublish {
        planes,
        tools,
        placements,
    } = *stone;
    if !planes.is_empty() {
        let converted: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|&(normal, offset)| GpuFacetPlane::new(normal, -offset))
            .collect();
        let mut ctx = RenderContext::lock(render_ctx);
        // A Library selection owns the slot until the editor claims it again with
        // a load or edit; a trailing solid frame must not flip the viewport back.
        if generation != 0
            && !matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. })
            && (!planes_equal_within(
                ctx.active_planes.as_slice(),
                &converted,
                PLANE_REPUBLISH_EPSILON,
            ) || ctx.active_tools.as_slice() != tools)
        {
            let design_gear = ctx.design_gear;
            // The frosting names planes by index. A frame that changes no plane count only
            // moved planes (an angle or index edit), so it keeps the frosting until the
            // background solve restates it; a frame with another plane count cannot say which
            // planes are the frosted ones, so it starts polished.
            let kept_frosted = (ctx.active_planes.len() == converted.len())
                .then(|| Arc::clone(&ctx.active_frosted));
            if ctx.claim_active_geometry(
                Arc::new(converted),
                Arc::new(tools.to_vec()),
                placements.to_vec(),
                design_gear,
                PlanesOwner::Editor { generation },
            ) {
                if let Some(kept) = kept_frosted {
                    ctx.active_frosted = kept;
                }
                ctx.dirty = true;
            }
        }
        *solid_active_planes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::clone(&ctx.active_planes));
    }
    planes_generations_match(trace_active_planes, solid_active_planes)
}

/// Files a frame's solved `masts`: into the shared `solid_last_solved` cache under
/// `generation` for a committed frame. A provisional frame of the Slice tool (`committed`
/// is `false`) only goes to the provisional tier's own session, which needs them for its
/// outline and handles; nothing else may see them.
/// Split out of [`SlintSolidSink::apply`] to keep it under the function-length lint.
fn store_solved(
    committed: bool,
    generation: u64,
    masts: Vec<SolvedTier>,
    last_solved: &SolidLastSolved,
) {
    if committed {
        *last_solved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((generation, masts));
    } else {
        editor::note_provisional_frame_masts(masts);
    }
}

/// Overwrites `state` with `value` (poison-tolerant). Used per per-frame table by
/// [`SlintSolidSink::apply`], to keep it under the function-length lint.
fn store_value<T>(state: &Mutex<T>, value: T) {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// Stores `value` into `state` when `Some`; `None` leaves `state` untouched. The Diagram
/// tables (`PreviewFrame::diagram_pick`, `diagram_tooth_pick`, `diagram_hover_text`,
/// `diagram_facet_owners`) are `Some` only together, for a `view_mode`-3 request (see
/// `PreviewFrame::diagram_hover_text`).
fn store_if_some<T>(state: &Mutex<Option<T>>, value: Option<T>) {
    if let Some(value) = value {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
    }
}

/// Pushes a committed frame's tier rows, banner and yield figures from its own `masts`.
///
/// `warnings` is the manufacturability pass the plan worker ran for the frame. The pass
/// builds a solid (check 6), which the UI thread must not do, so the table only displays
/// the findings. A frame without them gets a second push of the rows once [`LateRows`]
/// says they are due. Split out of [`SlintSolidSink::apply`] to keep it under the
/// function-length lint.
///
/// [`LateRows`] hears about the frame only if it pushed rows (see
/// [`late_rows::land_committed_frame`]). A frame without its own findings gets the kept
/// copy for its generation, so a Cut slider drag builds each frame's rows once.
fn push_committed_rows(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    masts: &[SolvedTier],
    warnings: Option<&Arc<Vec<ManufacturabilityWarning>>>,
) {
    let due = LATE_ROWS.with(|rows| {
        late_rows::land_committed_frame(rows, generation, warnings, |shown| {
            editor::apply_matching_preview_frame(ui, render_ctx, generation, masts, shown)
        })
    });
    push_due_findings(ui, render_ctx, due, FINDINGS_RETRIES);
}

/// Records which flat tiers the committed frame just stored contains (the Cut slider's cut),
/// for the facet maps the tier-selection outlines are built from. A provisional frame
/// describes a design the editor does not have, so it records nothing.
fn note_drawn_cut(committed: bool, geometry: Option<&FrameGeometry>) {
    if committed {
        facet_selection::note_drawn_tiers(geometry.and_then(|shown| shown.visible_tiers.clone()));
    }
}

/// Whether a landed frame is an out-of-order PLANNED frame: its own `generation` is behind
/// the `solid_last_solved` watermark (`cached_generation`), so it must not replace anything
/// on screen.
///
/// A camera-follow frame (`planned == false`) is never superseded. Its `generation` is the
/// worker's last planned one carried forward, so it trails the watermark after
/// `editor::view::refresh_viewport` or `push_viewport_after_background_solve`, which write
/// the cache and send only a `Reproject` (see `PreviewFrame::planned`). It must still redraw.
pub(super) const fn frame_is_superseded(
    planned: bool,
    generation: u64,
    cached_generation: u64,
) -> bool {
    planned && generation < cached_generation
}

/// Whether a landed frame's masts may be filed in the shared `solid_last_solved` cache, and
/// pushed to the tier table, under the frame's `generation`: only a PLANNED frame that is not
/// behind the watermark (`superseded`).
///
/// A camera-follow frame (`Reproject`/`UpdateFacetOverlay`) carries the worker's last masts
/// under its last planned generation. After an `Unsolvable` plan that generation has no masts
/// of its own, so filing them would make `gui::editor::finished_stone` accept the new angles
/// on the old masts. A generation that solved has already filed its masts from its planned
/// frame, so a camera-follow frame only ever repeats them.
pub(in crate::gui) const fn frame_files_masts(planned: bool, superseded: bool) -> bool {
    planned && !superseded
}

/// How long findings that met a held editor state wait before the next try.
const FINDINGS_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// How many times findings that met a held editor state are tried again: three seconds in
/// all. A state that is still held after that belongs to a writer that keeps the event loop
/// busy for longer; the rows then get the findings with the next frame of their generation
/// ([`late_rows::land_committed_frame`] hands the kept copy out again).
const FINDINGS_RETRIES: u8 = 30;

/// Pushes the rows the findings of a late-arriving plan belong to, when they are due (see
/// [`LateRows`]). `due` is what the state machine returned for the frame or the findings
/// just handled. Split out of [`PreviewSink::apply`] and [`PreviewSink::apply_findings`] to
/// keep them short.
///
/// A push that finds the editor state held by a callback (one that pumps the event loop) did
/// not show anything: the findings go back to [`LateRows`] as still owed, and are tried again
/// on a timer, `retries_left` more times. They are never marked as shown on a refusal.
fn push_due_findings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    due: Option<Arc<LateFindings>>,
    retries_left: u8,
) {
    let Some(late) = due else {
        return;
    };
    let outcome = editor::apply_late_findings(
        ui,
        render_ctx,
        late.generation,
        &late.design,
        &late.masts,
        &late.warnings,
    );
    if outcome != editor::LatePush::Held {
        return;
    }
    LATE_ROWS.with(|rows| rows.borrow_mut().put_back(&late));
    let Some(next) = cut_slider::retries_after_busy(retries_left) else {
        return;
    };
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    slint::Timer::single_shot(FINDINGS_RETRY_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let due = LATE_ROWS.with(|rows| rows.borrow_mut().owed());
        push_due_findings(&ui, &render_ctx, due, next);
    });
}

/// Hands the optical overlay the facet names of a committed frame, if it made any.
fn publish_optical_labels(
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    labels: Option<Arc<Vec<String>>>,
) {
    if let Some(labels) = labels {
        crate::bridge::optical_overlay::facet_labels(
            &RenderContext::lock(render_ctx),
            generation,
            labels,
        );
    }
}

/// Shows the frame's optional edges image and Diagram-mode image. A frame without edges
/// hides the edges layer; a frame without a Diagram image keeps the previous one (its
/// `has_diagram` flag says whether it is shown).
fn show_edges_and_diagram(
    ui: &MainWindow,
    edges_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    diagram_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    has_diagram: bool,
) {
    let model = ui.global::<SolidPreviewModel>();
    if let Some(edges_image) = edges_image {
        model.set_edges_image(crate::bridge::pixel_buffer::rgba_image(edges_image));
        model.set_has_solid_edges(true);
    } else {
        model.set_has_solid_edges(false);
    }
    if let Some(diagram_image) = diagram_image {
        model.set_diagram_image(crate::bridge::pixel_buffer::rgba_image(diagram_image));
    }
    model.set_has_diagram(has_diagram);
}

impl PreviewSink for SlintSolidSink {
    /// # Atomicity guarantee
    ///
    /// The frame's pick, solved and Diagram tables and its displayed image are swapped
    /// together, in one `upgrade_in_event_loop` closure on the UI thread. A click on the
    /// image still on screen must resolve against that image's own pick buffer, never a
    /// newer one stored early on the worker thread.
    fn apply(&self, frame: PreviewFrame) {
        let PreviewFrame {
            image,
            has_solid,
            status,
            solved,
            stale,
            pick,
            edges_image,
            diagram_image,
            has_diagram,
            diagram_pick,
            diagram_tooth_pick,
            diagram_panel_pick,
            diagram_hover_text,
            diagram_facet_owners,
            planes,
            tools,
            placements,
            hover_text,
            facet_owners,
            generation,
            planned,
            mesh_bounding_radius,
            geometry,
            warnings,
        } = frame;

        // Cloned Arcs (cheap), not `self` -- the closure below outlives this call.
        let sink = self.clone();

        // `slint::Image::from_rgba8` runs HERE, not on the worker thread -- see
        // `gui::solid_preview::to_pixel_buffer`'s doc comment (`slint::Image` is not
        // `Send`).
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            // `EditorState::generation` is one counter for the whole session (see
            // `auto_solve::scheduling::reset_for_new_design`), so the cached generation is a
            // reliable "highest frame applied so far" watermark. A PLANNED frame older than
            // it is an out-of-order result (a slow solve for a design a newer frame has
            // already superseded) and must not replace this frame's content.
            let cached_generation = sink
                .last_solved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map_or(0, |(cached_generation, _)| *cached_generation);
            // A camera-follow frame behind the watermark still redraws (see
            // `frame_is_superseded`); it only must not push its older masts back
            // into the cache or the tier table below -- `frame_files_masts` gates those.
            let superseded = generation < cached_generation;
            // Harness-only: what this frame says about the label table (see
            // `harness_support::FrameDiagnostics`).
            #[cfg(feature = "ui-harness")]
            let frame_facts = crate::harness_support::FrameFacts {
                generation,
                planned,
                has_masts: solved.is_some(),
                hover_len: hover_text.len(),
                plane_count: planes.len(),
                has_diagram,
            };
            if frame_is_superseded(planned, generation, cached_generation) {
                #[cfg(feature = "ui-harness")]
                crate::harness_support::record_frame_event(
                    crate::harness_support::FrameEvent::PlannedSuperseded(frame_facts),
                );
                return;
            }
            #[cfg(feature = "ui-harness")]
            crate::harness_support::record_frame_event(
                crate::harness_support::FrameEvent::Applied(frame_facts),
            );
            // The Slice tool's PROVISIONAL frames (`gui::editor::manipulate::
            // PROVISIONAL_GENERATION`) show the committed design plus a tier that is not in
            // `EditorState`. What is on screen (pick buffer, tables, geometry, images)
            // follows them. The committed design does not: `solid_last_solved`, the tier
            // table and the path tracer's plane slot never see a provisional frame.
            let committed = editor::frame_updates_mast_cache(generation);
            store_value(&sink.pick, Some(pick));
            note_drawn_cut(committed, geometry.as_ref());
            // Swapped with the pick buffer, so the manipulation handles project with the
            // pose and size of the frame on screen (see `SlintSolidSink::geometry`).
            store_value(&sink.geometry, geometry);
            // Swapped with the image too, so the orbit clamp (`render::camera_lighting`)
            // never reads a radius from a different frame than the one on screen.
            store_value(&sink.mesh_bounding_radius, mesh_bounding_radius);
            // Every replan frame's solved masts land here, on every edit. The debounced
            // background solve (`editor::auto_solve::on_edit`) is a separate `Design::solve()`
            // of the same design and races this one. `editor::apply_matching_preview_frame`
            // resolves the race: for a `generation` that still names the live design (see
            // `editor::auto_solve::take_matching_design`) it pushes the tier table from
            // `solved` and cancels the pending auto-solve. A superseded frame is a no-op.
            // `solved.as_ref()` only borrows; `solved` moves into `sink.last_solved` below.
            //
            // The plan worker delivers the manufacturability findings after its frame
            // (`PreviewSink::apply_findings`). A frame without them is followed by a second
            // push of the rows, which `LateRows` schedules.
            let files_masts = frame_files_masts(planned, superseded);
            if committed
                && files_masts
                && let Some(masts) = solved.as_ref()
            {
                push_committed_rows(&ui, &sink.render_ctx, generation, masts, warnings.as_ref());
            }
            // Only masts this frame solved are filed, under its own `generation`. An
            // `Unsolvable` plan carries none, and a camera-follow frame only repeats the
            // worker's last masts (see `frame_files_masts`).
            if files_masts && let Some(masts) = solved {
                store_solved(committed, generation, masts, &sink.last_solved);
            }
            // A provisional frame's planes go to the Slice tool too, after its masts. Redraws
            // that reproject the committed planes would otherwise draw the committed stone
            // over the provisional facet.
            if !committed {
                editor::note_provisional_frame_planes(&planes);
            }
            store_if_some(&sink.diagram_pick, diagram_pick);
            store_if_some(&sink.diagram_tooth_pick, diagram_tooth_pick);
            store_if_some(&sink.diagram_panel_pick, diagram_panel_pick);
            store_if_some(&sink.diagram_hover_text, diagram_hover_text);
            store_if_some(&sink.diagram_facet_owners, diagram_facet_owners);
            // Unconditional, unlike the Diagram tables above (see `SlintSolidSink::hover_text`).
            // The optical overlay gets one shared copy, made only for a committed frame, so
            // there is one clone per committed frame.
            let optical_labels = committed.then(|| Arc::new(hover_text.clone()));
            store_value(&sink.hover_text, hover_text);
            store_value(&sink.facet_owners, facet_owners);

            // Passes this frame's `generation`, so a stale frame's planes are not published
            // (see `sync_planes_and_check_trace_match`).
            let trace_matches = sync_planes_and_check_trace_match(
                &StonePublish::new(&planes, &tools, &placements),
                &sink.render_ctx,
                &sink.solid_active_planes,
                &sink.trace_active_planes,
                // `0` means "never publish these planes" (see the function's doc
                // comment): a provisional frame's planes must not reach the tracer.
                if committed { generation } else { 0 },
            );
            publish_optical_labels(&sink.render_ctx, generation, optical_labels);
            let model = ui.global::<SolidPreviewModel>();
            model.set_trace_matches_solid(trace_matches);

            model.set_image(crate::bridge::pixel_buffer::rgba_image(image));
            model.set_has_solid(has_solid);
            let frame_closed = has_solid && status.is_empty();
            model.set_status(status.into());
            model.set_stale(stale);
            show_edges_and_diagram(&ui, edges_image, diagram_image, has_diagram);
            // LAST, so every store above is already visible: the manipulation
            // handles refresh from here. The frame's own generation tells the Slice
            // tool whether this frame replaced its provisional picture.
            editor::on_solid_frame_landed(&ui, generation);
            heal_missing_diagram(&ui, &sink, has_diagram, frame_closed);
        });
    }

    /// The plan worker's manufacturability findings for a plan whose frame it handed on
    /// first. Hops to the UI thread like [`Self::apply`]; [`LateRows`] decides there whether
    /// the rows are refreshed now (their frame landed without the findings), later (the
    /// frame is still on its way) or not at all (the frame carried them, or the editor has
    /// moved on). A copy is kept for any further frame of the same generation (a tier
    /// selection or a Cut slider move plans the same design again), whose rows would
    /// otherwise start without them.
    fn apply_findings(&self, findings: LateFindings) {
        let render_ctx = Arc::clone(&self.render_ctx);
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            let due = LATE_ROWS.with(|rows| rows.borrow_mut().findings_arrived(findings));
            push_due_findings(&ui, &render_ctx, due, FINDINGS_RETRIES);
        });
    }
}

#[cfg(test)]
#[path = "solid_sink_tests.rs"]
mod tests;
