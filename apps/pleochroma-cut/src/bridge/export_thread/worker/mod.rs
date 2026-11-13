//! [`render_accumulation`]: the shared local/remote render core.
//!
//! It calibrates and runs the local/remote concurrent phase and merges every engine's
//! contribution exactly once into a linear accumulation buffer, without writing anything to
//! disk. [`run_export_tapped`] is the still-image export's own thin wrapper around it (tone-map,
//! then PNG/ICC write); the tilt performance video (`gui::tilt::video_export`) calls
//! [`render_accumulation`] directly, once per swept frame, reusing one `GpuBackend`
//! and one [`AccumulationCarry`] across the whole sweep instead of the still export's
//! always-fresh-per-call state (see [`AccumulationCarry`]). The `export_thread` module doc
//! describes the wider layout.
//!
//! Split into [`types`] (the `Accumulation`/`AccumulationOutcome`/`AccumulationCarry`
//! types), [`core`] (`render_accumulation`; its `resolve_remote_capability` and
//! `resolve_remote_rate` helpers live in `core/lanes.rs`), [`export`] (`run_export_tapped`'s
//! tone-map-then-write wrapper), [`final_picture`] (`render_image_rgba`: one finished RGBA8
//! image via either transfer -- full data through `render_accumulation`, or the remote's own
//! final picture), and [`tests`].

mod core;
mod export;
pub(super) mod export_ray;
mod final_picture;
mod taps;
#[cfg(test)]
mod tests;
mod types;

// `run_export` is the tests' entry point; the app passes its taps to `run_export_tapped`.
#[cfg(test)]
pub use export::run_export;
pub use export::run_export_tapped;
pub use final_picture::{RenderedImage, render_image_rgba};
pub use taps::{LinearImage, LinearImageKind, RawTapSink, RenderTaps};
pub use types::AccumulationCarry;
// `render_local_share` is used outside `worker` only by `hybrid_export_tests` (a sibling of
// `worker` under `export_thread`), which needs this re-export to reach it. The other re-exports
// here are test-only too, for that module's cross-check against `render_accumulation`.
#[cfg(test)]
pub(super) use core::render_accumulation;
#[cfg(test)]
pub(in crate::bridge::export_thread) use core::render_local_share;
#[cfg(test)]
pub(super) use types::{Accumulation, AccumulationOutcome};
