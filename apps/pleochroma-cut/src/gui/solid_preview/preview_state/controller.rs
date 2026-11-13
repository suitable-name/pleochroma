//! The editor-side controller struct [`SolidPreviewState`] and its UI-thread-facing entry points.
//!
//! The entry points are construction, the cheap camera-follow
//! reproject/facet-overlay submissions, the replan submission, and the tier-
//! cutoff cache. See the parent module's doc comment for the two-worker split
//! these methods feed into.

use super::{
    RedrawGate,
    request::{PlanJob, RedrawRequest, ReplanRequest},
    sink::PreviewSink,
    types::{CameraPose, FacetOverlay},
};
use glam::Vec3;
use pleochroma_cut_core::{Design, ManufacturabilityWarning};
use pleochroma_solid::preview::{Outlines, SharedOutlines, StoneGeometryBuf};
use std::sync::{
    Arc, Mutex, PoisonError, Weak,
    atomic::{AtomicU16, AtomicU64, Ordering},
    mpsc::Sender,
};

/// The editor-side controller described in the parent module's doc comment.
pub struct SolidPreviewState {
    pub(super) sink: Arc<dyn PreviewSink>,
    /// Consumed only by the RENDER worker (`super::render`'s `spawn_worker`) --
    /// the two-worker split, see the module doc comment.
    pub(super) gate: Arc<RedrawGate<RedrawRequest>>,
    /// Wake channel for the RENDER worker, created lazily on the first
    /// `request_*`/finished-plan call. `None` until then, so a `SolidPreviewState`
    /// never asked to redraw never spawns a thread.
    pub(super) wake: Mutex<Option<Sender<()>>>,
    /// Consumed only by the PLAN worker (`super::plan_worker`'s `spawn_plan_worker`). A queue
    /// of its own, so a slow solve never blocks `Reproject` requests.
    pub(super) plan_gate: Arc<RedrawGate<PlanJob>>,
    /// [`Self::wake`]'s counterpart for the PLAN worker, created on the first
    /// [`Self::request_replan`] call.
    pub(super) plan_wake: Mutex<Option<Sender<()>>>,
    /// A weak handle to this same value, set right after construction. The PLAN worker
    /// uses it to hand a finished frame to the RENDER worker through [`Self::submit`]. It is
    /// `Weak` so the worker never keeps the state alive.
    pub(super) self_weak: Mutex<Weak<Self>>,
    /// "Show through tier N" slider: `Some(n)` truncates the plane arrangement
    /// to `design.tiers[..=n]`. Cached here for UI access via [`Self::set_tier_cutoff`].
    pub(super) tier_cutoff: Mutex<Option<usize>>,
    /// The Cut slider: `Some(k)` draws the stone after the first `k` cutting steps
    /// (`Design::preview_steps`), `Some(0)` being the rough, `None` the finished stone.
    /// Wins over [`Self::tier_cutoff`] in a replan; set by [`Self::set_cut_steps`].
    pub(super) cut_steps: Mutex<Option<usize>>,
    /// Raised by [`Self::bump_generation_floor`] when the design is replaced wholesale. The
    /// PLAN worker drops any `PlanJob` whose `generation` is below it, since the plan gate
    /// only coalesces to the latest job, not to the latest job for the current design.
    pub(super) generation_floor: Arc<AtomicU64>,
    /// The Slice tool's provisional-tier outline and the drag-follower outline, shared
    /// with the RENDER worker. Written by [`Self::set_outlines`] and read when the worker
    /// draws, so no later request can drop or resurrect them.
    pub(super) outlines: SharedOutlines,
    /// When `Some`, replaces the `planes` of every [`Self::request_redraw_with_gear`]
    /// call: the Slice tool sets it to the provisional design's planes so a camera
    /// orbit, a view-mode switch or a background-solve redraw (all of which reproject
    /// the COMMITTED `RenderContext::active_planes`) keeps showing the provisional
    /// facet instead of snapping back to the committed stone.
    pub(super) planes_override: Mutex<Option<Vec<(Vec3, f32)>>>,
    /// The manufacturability findings of the latest planned designs. The PLAN worker writes
    /// an entry after it hands its frame on, and the RENDER worker copies the findings onto
    /// the frames it draws. The UI thread only displays them: the check (the stone's solid
    /// and the tools' polytopes) is too slow for it. See
    /// `pleochroma_cut_core::manufacturability::concave_tool_warnings`.
    pub(super) warnings: SharedWarnings,
    /// The Edit tab's view mode as last reported ([`Self::note_view_mode`]), or
    /// [`VIEW_MODE_UNNOTED`]. The RENDER worker draws every `Planned` frame at this mode, not
    /// the mode of its job: a plan can finish after the user switched views, and the frame
    /// would otherwise show a view the window cannot display.
    pub(super) view_mode_note: Arc<AtomicU16>,
}

/// [`SolidPreviewState::view_mode_note`] before any mode was reported: planned frames keep
/// the mode of their own job.
pub(super) const VIEW_MODE_UNNOTED: u16 = u16::MAX;

/// One planned design's manufacturability findings, matched by the `Arc` of the plan job
/// (the planned frame carries the same allocation), not by generation.
///
/// A replan of the same design gets a fresh `Arc`, so it finds no entry here and its frame
/// goes out without findings unless its own pass finishes first. `gui::solid_sink`'s
/// `LateRows` restores them to the rows.
pub(super) struct PlanFindings {
    /// The design the findings were computed for. Holding it keeps the allocation from being
    /// reused while the entry can still be looked up.
    pub(super) design: Arc<Design>,
    pub(super) warnings: Arc<Vec<ManufacturabilityWarning>>,
}

/// The few most recent [`PlanFindings`], newest last -- see [`SolidPreviewState::warnings`].
pub(super) type SharedWarnings = Arc<Mutex<Vec<PlanFindings>>>;

impl SolidPreviewState {
    /// Builds a controller that hands every finished frame to `sink`. No thread is
    /// spawned yet -- see [`Self::request_redraw`].
    #[must_use]
    pub fn new(sink: Arc<dyn PreviewSink>) -> Arc<Self> {
        let state = Arc::new(Self {
            sink,
            gate: Arc::new(RedrawGate::new()),
            wake: Mutex::new(None),
            plan_gate: Arc::new(RedrawGate::new()),
            plan_wake: Mutex::new(None),
            self_weak: Mutex::new(Weak::new()),
            tier_cutoff: Mutex::new(None),
            cut_steps: Mutex::new(None),
            generation_floor: Arc::new(AtomicU64::new(0)),
            outlines: Arc::new(Mutex::new(Outlines::default())),
            planes_override: Mutex::new(None),
            warnings: Arc::new(Mutex::new(Vec::new())),
            view_mode_note: Arc::new(AtomicU16::new(VIEW_MODE_UNNOTED)),
        });
        *state
            .self_weak
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&state);
        state
    }

    /// Records the Edit tab's view mode (0 Solid ... 3 Diagram ... 5) the moment the window
    /// changes it, before the redraw at the new mode is requested. From then on a `Planned`
    /// frame is drawn at this mode whatever mode its job was planned at -- see
    /// [`Self::view_mode_note`]. Does NOT redraw.
    pub fn note_view_mode(&self, view_mode: u8) {
        self.view_mode_note
            .store(u16::from(view_mode), Ordering::Relaxed);
    }

    /// Submits a cheap camera-follow redraw and returns at once; the frame reaches
    /// [`PreviewSink::apply`] asynchronously. `size` is the viewport's logical size in
    /// points. `view_mode` is 0 Solid, 1 Path-traced, 2 Both or 3 Diagram. Only the last of a
    /// burst of calls is drawn (see `RedrawGate`).
    ///
    /// This is [`Self::request_redraw_with_gear`] with `gear: None`, which keeps the worker's
    /// last-known Diagram gear info. Prefer the `_with_gear` form when the loaded design's
    /// gear info is known.
    pub fn request_redraw(
        &self,
        planes: &[(Vec3, f32)],
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
    ) {
        self.request_redraw_with_gear(planes, camera, size, view_mode, None);
    }

    /// Same as [`Self::request_redraw`], with `gear`: the loaded design's gear tooth count and
    /// reference angle. A Diagram-mode frame draws that gear wheel. `None` keeps the worker's
    /// last-known gear info.
    pub fn request_redraw_with_gear(
        &self,
        planes: &[(Vec3, f32)],
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) {
        self.request_redraw_geometry(
            StoneGeometryBuf::from_halfspaces(planes),
            camera,
            size,
            view_mode,
            gear,
        );
    }

    /// [`Self::request_redraw_with_gear`] for a stone that may carry concave tools, with the
    /// whole `geometry`: planes, tools and each tool's `(tier, placement)`. Callers that know
    /// the design's tools use this form, so the notches are kept.
    ///
    /// [`Self::set_planes_override`] replaces the planes only; the tools stay.
    pub fn request_redraw_geometry(
        &self,
        mut geometry: StoneGeometryBuf,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) {
        let override_planes = self
            .planes_override
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(planes) = override_planes {
            geometry.planes = StoneGeometryBuf::from_halfspaces(&planes).planes;
        }
        self.submit(RedrawRequest::Reproject {
            geometry,
            camera,
            size,
            view_mode,
            gear,
        });
    }

    /// [`Self::request_redraw_geometry`] for a viewport that shows only the committed design.
    /// [`Self::set_planes_override`] is not applied, so a provisional slice never shows.
    pub fn request_redraw_committed_geometry(
        &self,
        geometry: StoneGeometryBuf,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) {
        self.submit(RedrawRequest::Reproject {
            geometry,
            camera,
            size,
            view_mode,
            gear,
        });
    }

    /// Submits a facet highlight update (hover, click, multi-select; see [`FacetOverlay`]).
    /// It re-renders with the camera, planes, size and view mode the worker last used, so the
    /// caller needs to resend none of them. A no-op before the worker has rendered anything.
    pub fn request_facet_overlay(&self, overlay: FacetOverlay) {
        self.submit(RedrawRequest::UpdateFacetOverlay(overlay));
    }

    /// Stores the provisional-tier and drag-follower outlines for the RENDER worker (see
    /// [`Self::outlines`]). Does NOT redraw: follow with [`Self::request_facet_overlay`].
    /// Only the 3D overlay path calls this; the Diagram view's overlays never clear outlines.
    pub fn set_outlines(&self, provisional: &[u32], moved: &[u32]) {
        let mut outlines = self.outlines.lock().unwrap_or_else(PoisonError::into_inner);
        outlines.provisional.clear();
        outlines.provisional.extend_from_slice(provisional);
        outlines.moved.clear();
        outlines.moved.extend_from_slice(moved);
    }

    /// Makes every later [`Self::request_redraw_with_gear`] draw `planes` instead of
    /// the planes its caller passes (`None` restores the caller's). Used by the Slice
    /// tool while a provisional facet is on screen -- see [`Self::planes_override`].
    pub fn set_planes_override(&self, planes: Option<Vec<(Vec3, f32)>>) {
        *self
            .planes_override
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = planes;
    }

    /// Submits a replan-on-edit request to the PLAN worker.
    /// `live_update::plan_preview` never runs on the UI thread.
    pub fn request_replan(&self, request: ReplanRequest) {
        let tier_cutoff = *self
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let cut_steps = *self
            .cut_steps
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.submit_plan(PlanJob {
            design: request.design,
            dirty: request.dirty,
            last_solved: request.last_solved,
            camera: request.camera,
            size: request.size,
            selected_tier: request.selected_tier,
            n_d: request.n_d,
            view_mode: request.view_mode,
            generation: request.generation,
            show_preform: request.show_preform,
            enlarged_panel: request.enlarged_panel,
            tier_cutoff,
            cut_steps,
        });
    }

    /// Sets the Cut slider for the NEXT [`Self::request_replan`]: `Some(k)` draws the
    /// stone after the first `k` cutting steps (`Design::preview_steps`; `0` is the
    /// rough, the preform alone), `None` the finished stone. Does NOT redraw; the
    /// caller submits a replan afterwards, as `gui::editor::view::submit_preview_replan`
    /// does after reading the slider (`gui::solid_preview::cut_slider`).
    pub fn set_cut_steps(&self, steps: Option<usize>) {
        *self
            .cut_steps
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = steps;
    }

    /// Sets the "show through tier N" cutoff: `Some(n)` truncates to `design.tiers[..=n]`.
    /// Does NOT redraw; the caller submits [`Self::request_replan`] afterwards.
    ///
    /// The Cut slider goes through [`Self::set_cut_steps`], which wins over this value.
    pub fn set_tier_cutoff(&self, cutoff: Option<usize>) {
        *self
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = cutoff;
    }

    /// Raises [`Self::generation_floor`] to at least `floor`, called on each wholesale design
    /// replacement. It never moves backwards.
    pub fn bump_generation_floor(&self, floor: u64) {
        self.generation_floor.fetch_max(floor, Ordering::Relaxed);
    }
}
