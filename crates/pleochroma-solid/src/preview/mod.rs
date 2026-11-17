//! The Solid/Diagram frame pipeline, run by both the desktop app and the web app.
//!
//! A design is planned into a plane arrangement and a facet style, then rasterized
//! (solid, "Both" edges layer, or the 2D diagram) with hover and selection overlays.
//! The desktop runs these on worker threads and the web app on its main thread, through
//! the same functions:
//!
//! - [`build_planned_frame`] (the planner, around [`crate::live_update::plan_preview`],
//!   with an injected [`crate::live_update::Clock`]);
//! - [`render_request`] (resolve one [`RedrawRequest`] against [`WorkerMemory`],
//!   rasterize, build the diagram) returning a [`RenderedFrame`];
//! - [`PreviewPipeline`], the two run back to back on one thread;
//! - [`camera`]'s orbit/zoom/pose math and [`view`]'s hit testing, raster sizing,
//!   selection stepping and dirty-tier diff.
//!
//! The desktop keeps its threads, its `RedrawGate` coalescing and the Slint
//! pixel-buffer conversion. The tests check that the web and desktop entry points
//! produce identical frames, equal to the pinned bytes on Windows.

pub mod camera;
mod pipeline;
mod plan;
mod render;
mod request;
mod state;
#[cfg(test)]
mod tests;
mod types;
pub mod view;

pub use pipeline::PreviewPipeline;
pub use plan::{build_planned_frame, build_planned_frame_themed, restyle_for_theme};
pub use render::{DEFAULT_MESH_BOUNDING_RADIUS, RenderedFrame, render_request};
pub use request::{PlanJob, PlannedFrame, RedrawRequest, panel_kind_from_index};
pub use state::{
    DiagramMemory, Outlines, RequestState, SharedOutlines, WorkerMemory, dim_style,
    escaping_tier_label, resolve_request_state,
};
pub use types::{CameraPose, FacetOverlay, FrameGeometry, PickBuffer, StoneGeometryBuf};
