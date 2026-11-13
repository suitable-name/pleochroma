//! Where a finished (or status-only) solid-preview frame goes once the worker
//! thread has it -- the [`PreviewSink`] trait and its [`PreviewFrame`] payload.

use super::types::{FacetOwners, FrameGeometry, PickBuffer};
use glam::Vec3;
use pleochroma::geometry::{ToolPrimitive, meet_solver::SolvedTier};
use pleochroma_cut_core::{Design, ManufacturabilityWarning};
use std::sync::Arc;

/// Where a finished (or status-only) solid-preview frame goes once the worker
/// thread has it.
///
/// Called from the WORKER thread, never the UI thread -- an
/// implementation that needs to touch a Slint window must hop over via
/// `slint::Weak::upgrade_in_event_loop` itself.
///
/// `image` is always a fully formed pixel buffer of the request's `size`, background-coloured
/// when `has_solid` is `false`. `status` is a short reason when `!has_solid`, and is always set
/// for an `Unsolvable` `Planned` frame, whose drawn solid is the held-over previous one.
/// `solved` is the plan's masts for a `Planned` frame, and `None` when no solve produced masts.
/// A `Reproject` frame repeats the last masts under its planned generation and is never a
/// solve of its own. `stale` is set only for `live_update::Freshness::Stale`. `edges_image` is
/// `Some` only for view mode 2 with a closed arrangement.
///
/// The image is a `SharedPixelBuffer`, not a `slint::Image`, which is not `Send`. The UI
/// thread converts it with `slint::Image::from_rgba8`.
pub trait PreviewSink: Send + Sync + 'static {
    fn apply(&self, frame: PreviewFrame);

    /// Called from the PLAN worker once a plan's manufacturability findings are ready, after
    /// its frame went out (see [`LateFindings`]). The default does nothing.
    fn apply_findings(&self, _findings: LateFindings) {}
}

/// The manufacturability findings of one plan, delivered after its frame. The frame goes
/// first so the picture never waits for the costly concave-tool check.
pub struct LateFindings {
    /// The generation of the plan the findings were computed for.
    pub generation: u64,
    /// The planned design (the allocation the plan job carried).
    pub design: Arc<Design>,
    /// The plan's own solved masts.
    pub masts: Vec<SolvedTier>,
    /// The full pass for `design` and `masts`.
    pub warnings: Arc<Vec<ManufacturabilityWarning>>,
}

/// One finished worker-thread render -- everything `super::render::render_request`
/// produces. See [`PreviewSink`]'s doc comment for what each field means.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the finished-render message mirrors the render result; has_solid, stale, has_diagram and the rest are independent flags"
)]
pub struct PreviewFrame {
    pub image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    pub has_solid: bool,
    pub status: String,
    pub solved: Option<Vec<SolvedTier>>,
    pub stale: bool,
    pub pick: PickBuffer,
    pub edges_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    /// View mode 3's three-panel 2D faceting diagram (`diagram2d::render_diagram`),
    /// built only when the request's `view_mode` was `3` and the arrangement
    /// closed -- `None` otherwise (never built for view modes 0/1/2, matching
    /// `edges_image`'s own "only when asked for" contract).
    pub diagram_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    pub has_diagram: bool,
    /// The diagram's own per-pixel facet-picking buffer -- a SEPARATE buffer from
    /// `pick` (the solid rasterizer's), since the diagram's pixel layout is
    /// entirely different, but resolving to the SAME globally-unique `facet_id`
    /// space, so a click on any panel maps to the correct facet through the exact
    /// same `facet_map::FacetMap` lookups the solid view's hover/click already use.
    pub diagram_pick: Option<PickBuffer>,
    /// The index wheel's own per-pixel tooth-picking buffer. A SEPARATE buffer
    /// from `diagram_pick` (different hit regions), using the same `PickBuffer`
    /// encoding. `None` when no diagram or arrangement did not close.
    pub diagram_tooth_pick: Option<PickBuffer>,
    /// Which panel painted each pixel: the same `PickBuffer` encoding, but a lookup
    /// yields the panel index (`0` Crown, `1` Pavilion, `2` Profile) that
    /// `SolidPreviewModel.diagram_enlarged_panel` uses. Backs the double-click that
    /// enlarges a panel. `None` when no diagram or arrangement did not close.
    pub diagram_panel_pick: Option<PickBuffer>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`),
    /// built alongside the diagram image so the UI thread can resolve a diagram
    /// hover without needing `Design`/`FacetMap` access itself. `None` when no
    /// diagram was requested this frame; a `super::request::RedrawRequest::
    /// Reproject` request in view mode 3 carries forward the WORKER's last-known
    /// table rather than rebuilding it (there is no `Design` to rebuild it from on
    /// that path).
    pub diagram_hover_text: Option<Vec<String>>,
    /// Facet id -> owning flat tier (`facet_map::FacetMap::tier_of`) and owning concave
    /// tier, the other half of what a diagram click needs to select a tier -- see
    /// `diagram_hover_text`'s doc comment for why this travels with the frame
    /// instead of being recomputed on the UI thread.
    pub diagram_facet_owners: Option<FacetOwners>,
    /// The plane arrangement this frame was rendered from, in `(normal, offset)` form. Always
    /// present. The sink copies it into `RenderContext::active_planes` when it differs, so a
    /// later camera orbit does not restore the pre-edit planes.
    pub planes: Vec<(Vec3, f32)>,
    /// The concave tools subtracted from [`Self::planes`] (empty for a planar design). The sink
    /// copies them into `RenderContext::active_tools` for the same reason.
    pub tools: Vec<ToolPrimitive>,
    /// `(concave tier, placement)` of each of [`Self::tools`].
    pub placements: Vec<(usize, usize)>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`), for every
    /// view mode. It is built on the worker from the same design as `pick`, so its facet ids
    /// always match the displayed frame.
    pub hover_text: Vec<String>,
    /// Facet id -> owning flat tier (`facet_map::FacetMap::tier_of`) and owning concave
    /// tier: the Solid view's counterpart to `diagram_facet_owners`.
    pub facet_owners: FacetOwners,
    /// The design-state generation this frame reflects -- `super::request::
    /// ReplanRequest::generation` for a `Replan` frame, or `super::state::
    /// WorkerMemory::generation` (carried forward unchanged) for a `Reproject`/
    /// `UpdateFacetOverlay` one; `0` for a frame from a non-`editor` build, or
    /// before this worker thread has ever received a `Replan`. Lets a real
    /// [`PreviewSink`] recognize a frame whose already-solved [`Self::solved`]
    /// masts still describe the LIVE design, and rebuild the tier table's rows
    /// from them directly instead of leaving that to a second, separately
    /// dispatched `Design::solve()`.
    pub generation: u64,
    /// `true` for a frame answering a [`super::request::RedrawRequest::Planned`]
    /// request (a fresh replan of the design); `false` for a `Reproject`/
    /// `UpdateFacetOverlay` frame, whose [`Self::generation`] is only the worker's
    /// LAST planned generation carried forward. The sink's out-of-order guard applies to
    /// planned frames only, since a redraw-only frame may trail the `solid_last_solved` mark.
    pub planned: bool,
    /// The bounding radius of the mesh this frame drew, or [`DEFAULT_MESH_BOUNDING_RADIUS`]
    /// before anything has closed. The sink sizes the orbit camera's distance clamp from it.
    pub mesh_bounding_radius: f64,
    /// The mesh geometry and camera pose the rasterizer used, for the direct-manipulation
    /// handles. `None` only when no closed solid was ever built. The sink stores it with
    /// `pick`, so it describes the same pick buffer.
    pub geometry: Option<FrameGeometry>,
    /// The manufacturability findings of this frame's design, from the PLAN worker. `None` when
    /// they are not ready yet; they then come through [`PreviewSink::apply_findings`].
    pub warnings: Option<Arc<Vec<ManufacturabilityWarning>>>,
}

/// [`PreviewFrame::mesh_bounding_radius`]'s fallback before any arrangement has closed,
/// about the half-width of a round brilliant. Re-exported from `pleochroma_solid::preview`.
pub use pleochroma_solid::preview::DEFAULT_MESH_BOUNDING_RADIUS;
