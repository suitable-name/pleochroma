//! The editor-side controller for the solid preview.
//!
//! [`SolidPreviewState`] coalesces a burst of redraw requests (an orbit drag, a fast-typed
//! edit) down to the last one, under the same [`RedrawGate`] contract as the path-traced
//! view. Finished frames go to a [`PreviewSink`], which the Slint side implements.
//!
//! # Threads
//!
//! Requests are submitted from the UI thread and return at once. Two worker threads, spawned
//! lazily, do the work:
//!
//! - The **plan worker** ([`plan_worker`]) alone runs the expensive solve. It has its own
//!   [`RedrawGate`], so a burst of edits runs as the current solve, then the latest queued one.
//! - The **render worker** ([`render`]) owns the mesh cache and the rasterizer, neither of
//!   which is `Sync`. It handles `Reproject`, `UpdateFacetOverlay` and the plan worker's
//!   `Planned` frame. It never solves, so a camera drag is not blocked by a slow solve.
//!
//! The plan worker hands its frame over through [`SolidPreviewState::submit`], using the
//! `Weak` handle back to this state (`self_weak`).
//!
//! The "Unbounded" check is split: [`plan_worker::build_planned_frame`] plans, and the render
//! worker's `resolve_planned_state` in `pleochroma_solid` finishes it. That step reuses the
//! mesh the planner built, so the mesh cache hits.
//!
//! # Where `last_solved` lives
//!
//! `EditorState` is `Rc<RefCell<..>>`, which the worker thread cannot reach, and
//! [`PreviewSink`] must stay `Send + Sync`. So [`PreviewSink::apply`] receives the solved
//! tiers as plain `Send` data, and the wiring layer keeps them in a shared `Arc<Mutex<..>>`
//! for the editor to read when it builds the next [`request::ReplanRequest`].
//!
//! # `PreviewSink`
//!
//! [`SolidPreviewState`] is written against the [`PreviewSink`] trait, not a
//! `slint::Weak<MainWindow>`, so a synchronous fake (`tests::FakeSink`) can drive its tests.

//!
//! # The pipeline lives in `pleochroma_solid::preview`
//!
//! The planner, the request types, the render worker's state machine and its draw step
//! live in `pleochroma_solid::preview`, shared with the web app. This module keeps the
//! threads, the gates and the Slint pixel-buffer conversion, and re-exports the shared items.
//! `tests::pins` holds the byte-identity pins of the rendered output.

#[cfg(test)]
use super::facet_map::FacetMap;
use super::{
    live_update, mesh_cache::MeshCache, raster::SolidRasterizer, to_diagram_pixel_buffer,
    to_pixel_buffer,
};

use crate::bridge::render_thread::RedrawGate;

mod controller;
mod plan_worker;
mod render;
mod request;
mod sink;
mod state;
#[cfg(test)]
mod tests;
mod types;

pub use controller::SolidPreviewState;
pub use request::ReplanRequest;
pub use sink::{DEFAULT_MESH_BOUNDING_RADIUS, LateFindings, PreviewFrame, PreviewSink};
pub use types::{
    CameraPose, FacetOverlay, FacetOwners, FrameGeometry, PickBuffer, SolidLastSolved,
    SolidPickState,
};
