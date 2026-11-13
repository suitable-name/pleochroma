//! The payloads passed between the UI thread and the plan and render workers.
//!
//! `RedrawRequest`, `PlanJob` and `PlannedFrame` come from `pleochroma_solid::preview`;
//! [`ReplanRequest`] is the UI-thread entry point for a replan.

use super::types::CameraPose;
use pleochroma::geometry::meet_solver::SolvedTier;
use std::{collections::BTreeSet, sync::Arc};

pub use pleochroma_solid::preview::{PlanJob, PlannedFrame, RedrawRequest};

/// [`super::SolidPreviewState::request_replan`]'s payload: bundles everything
/// `live_update::plan_preview` needs.
///
/// `design` is an `Arc` clone: cheap (refcount bump, not a deep clone) and lets
/// the worker thread own its handle without holding the UI thread's
/// `Rc<RefCell<EditorState>>` borrow open across the async round trip.
pub struct ReplanRequest {
    pub design: Arc<pleochroma_cut_core::Design>,
    /// The tier index/indices the triggering edit touched. An edit that cannot be
    /// described as "these tiers changed" (`Undo`/`Redo`, a gear remap, a
    /// symmetry/mirror change) should pass `last_solved: None` instead.
    pub dirty: BTreeSet<usize>,
    /// The previous call's resolved masts, or `None` to force a full
    /// `Design::solve()` rather than a subgraph `resolve_dirty` -- always safe (just
    /// slower), so prefer `None` over an under-approximated `dirty`.
    pub last_solved: Option<Vec<SolvedTier>>,
    pub camera: CameraPose,
    /// The Solid viewport's own LOGICAL size -- see
    /// [`super::SolidPreviewState::request_redraw`]'s doc comment.
    pub size: (u32, u32),
    /// The tier currently selected in the tier list (`facet_map::OverlayFlags::
    /// selected`), `None` when nothing is selected.
    pub selected_tier: Option<usize>,
    /// The design's effective refractive index the critical-angle overlay is
    /// computed against.
    pub n_d: f64,
    /// 0 = Solid, 1 = Path-traced, 2 = Both -- only `2` makes the worker also
    /// render the transparent-fill edges layer (see `super::sink::PreviewFrame::
    /// edges_image`).
    pub view_mode: u8,
    /// `gui::editor::state::EditorState::generation`'s value at the moment this
    /// request was submitted (`gui::editor::view::submit_preview_replan`) --
    /// Echoed onto the finished `super::sink::PreviewFrame` unchanged so the sink
    /// can tell a still-current result from one a later edit has superseded.
    pub generation: u64,
    /// `SolidPreviewModel.show_preform_planes`'s value at request time: the
    /// preform-visibility toggle, whether the rough-bounding preform facets
    /// (`facet_map::FacetMap::preform_plane_count`) should render at all
    /// rather than being hidden so only the schedule's own cut facets show.
    pub show_preform: bool,
    /// `SolidPreviewModel.diagram_enlarged_panel` at request time: `0`/`1`/`2` for
    /// Crown/Pavilion/Profile, anything else (such as the default `-1`) for no enlarged
    /// panel. Kept as a plain `i32`; the worker converts it with
    /// `pleochroma_solid::preview::panel_kind_from_index`.
    pub enlarged_panel: i32,
}
