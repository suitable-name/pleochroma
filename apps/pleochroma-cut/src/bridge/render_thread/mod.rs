//! The live-viewport render thread: `RenderContext` (the shared, live-mutated render
//! configuration the GUI writes into), the progressive-accumulation loop that reads a
//! per-frame snapshot from it, and everything that loop dispatches to -- the CPU
//! scanline tracer, the GPU backend wrapper, and the gemological metrics worker.
//!
//! Split into submodules: [`context`] (the `RenderContext`/`FrameInputs` state, plus
//! material resolution), [`metrics`] (gemological-metrics cache), [`metrics_worker`]
//! (the thread that evaluates the metrics off this loop), [`scanline`] (CPU
//! scanline tracer), [`gpu_backend`] (GPU backend wrapper and frame dispatch),
//! [`denoise`] (denoise + tone-map, pure functions), [`display_thread`] (the dedicated
//! thread that calls them, off the trace loop -- see its own doc comment for the
//! pipelining design), [`frame_helpers`] (small, pure, unit-tested per-frame decisions
//! `spawn_render_thread`'s loop calls into), and [`redraw_gate`] (coalesces a burst of
//! redraw-worthy events into at most one pending Slint UI-thread closure; also used by
//! `gui::remote::orchestrator`). The progressive loop lives in [`worker`], keeping scheduling separate from these
//! shared exports.
//!
//! # Local+Remote combined live rendering
//!
//! Once the camera settles, `LiveComputeTarget::Both` (the default whenever a worker is
//! configured) lets this thread's local tracing keep running alongside a dispatched
//! remote render, both contributing to the same displayed image, rather than local
//! being suspended for the image's whole lifetime the way `LiveComputeTarget::RemoteOnly`
//! still works.
//!
//! **The two engines never write one buffer.** `accum_buffer` stays a plain `Vec<Vec3>`
//! owned outright by this thread, touched under no lock. The remote side's radiance
//! lives in `RenderContext::live_epoch` (`bridge::sample_cursor::live::LiveEpoch`): one
//! merged sum of finished chunks plus the in-flight chunk's own `Accumulator`, which
//! the remote connection thread sums `FRAME` deltas into. This thread only reads them,
//! and only at the display cadence ([`DENOISE_MIN_INTERVAL`], not per traced sample):
//! a short lock, a full-buffer read, an elementwise add into a scratch buffer for that
//! cycle alone. Neither engine's buffer is ever discarded or overwritten by the other.
//!
//! **Disjoint sample ranges.** There is no fixed reservation any more: the epoch owns
//! one shared cursor over `[0, target_samples)`. The remote lane claims chunks sized to
//! a second or two of the worker's measured rate, and every frame this loop claims its
//! own `spp`-sample range from the same cursor ([`live_split::local_frame_claim`]) and
//! traces it at that absolute offset. The combined display divides the local sum
//! plus the remote sum plus the in-flight chunk's buffer by the matching total count,
//! and is denoised once. Tracing stops once that total reaches `target_samples`, the
//! single global target.
//!
//! **The old suspend-while-remote mechanism.** `remote_active`/`resolve_remote_ownership`
//! keep an unchanged pure contract, but what a resolved `remote_active == true` does now
//! depends on `live_compute_target`: for `RemoteOnly` it still suspends tracing outright
//! (local racing back in over a finished remote image is still a live risk there); for
//! `Both` that's structurally impossible (local only ever adds into a display-time
//! sum), so `remote_active` instead gates [`should_combine_remote`].
//! `resolve_remote_ownership`'s release condition is the one mechanism behind both
//! effects.

#[cfg(feature = "render-bench")]
pub mod benchmark;
mod context;
mod denoise;
mod engine;
pub use engine::remote_allowed;
mod display_thread;
mod frame_helpers;
mod gpu_backend;
mod interactive;
mod live_split;
mod local_preview;
mod metrics;
mod metrics_worker;
mod redraw_gate;
mod scanline;
mod whitted_scanline;

pub use context::{
    ColorTarget, MaterialOverrides, MaterialSources, PlanesOwner, RenderContext, ViewColor,
    apply_material_overrides, load_env_map, resolve_material, resolve_material_with_override,
};
pub use denoise::{
    DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame, tonemap_running_average,
};
pub use metrics::hash_planes;
pub use redraw_gate::RedrawGate;

mod pacing;
pub mod progress;
mod worker;
pub use worker::spawn_render_thread;
