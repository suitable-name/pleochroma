//! The live-viewport render thread.
//!
//! It consists of `RenderContext` (the render configuration the GUI
//! writes into while the viewport is live), the progressive-accumulation loop that reads a
//! per-frame snapshot of it, and what that loop dispatches to: the CPU scanline tracer, the
//! GPU backend wrapper and the gemological metrics worker.
//!
//! Submodules: [`context`] (`RenderContext`, `FrameInputs`, material resolution),
//! [`metrics`] (gemological-metrics cache), [`metrics_worker`] (evaluates the metrics off
//! this loop), [`scanline`] (CPU tracer), [`gpu_backend`] (GPU backend wrapper and frame
//! dispatch), [`denoise`] (denoise and tone-map, pure functions), [`display_thread`] (runs
//! them on a dedicated thread, off the trace loop), `adaptive_view` (opt-in adaptive sampling
//! of a still view, `adaptive-sampling` feature), `still_beams` and `ray_beams` (opt-in
//! beam-traced still pictures, `beam` feature, off in shipped builds), [`frame_helpers`]
//! (small pure, unit-tested per-frame decisions) and [`redraw_gate`] (coalesces redraw
//! requests into at most one pending Slint UI closure; also used by
//! `gui::remote::orchestrator`). The progressive loop lives in [`worker`].
//!
//! # Local+Remote combined live rendering
//!
//! Once the camera settles, `LiveComputeTarget::Both` (the default) keeps this thread's
//! local tracing running alongside a dispatched remote render; both contribute to the same
//! displayed image. `LiveComputeTarget::RemoteOnly` instead suspends local tracing for the
//! image's whole lifetime.
//!
//! **The two engines never write one buffer.** `accum_buffer` stays a plain `Vec<Vec3>`
//! owned outright by this thread, touched under no lock. The remote side's radiance
//! lives in `RenderContext::live_epoch` (`bridge::sample_cursor::live::LiveEpoch`): one
//! merged sum of finished chunks plus the in-flight chunk's own `Accumulator`, which
//! the remote connection thread sums `FRAME` deltas into. This thread only reads them,
//! and only at the display cadence (`pacing::DENOISE_MIN_INTERVAL`, not per traced sample):
//! a short lock, a full-buffer read, an elementwise add into a scratch buffer for that
//! cycle alone. Neither engine's buffer is ever discarded or overwritten by the other.
//!
//! **Disjoint sample ranges.** The epoch owns one shared cursor over `[0, target_samples)`.
//! The remote lane claims chunks sized to a second or two of the worker's measured rate,
//! and every frame this loop claims its own `spp`-sample range from the same cursor
//! ([`live_split::local_frame_claim`]) and traces it at that absolute offset. The combined
//! display divides the local sum plus the remote sum plus the in-flight chunk's buffer by
//! the matching total count, and is denoised once. Tracing stops once that total reaches
//! `target_samples`, the single global target.
//!
//! **Suspension while remote owns the image.** `remote_active` and
//! `resolve_remote_ownership` decide it. For `RemoteOnly`, a resolved `remote_active ==
//! true` suspends tracing outright, because local racing back in over a finished remote
//! image is a live risk there. For `Both` that cannot happen (local only adds into a
//! display-time sum), so `remote_active` gates `should_combine_remote` instead. The release
//! condition of `resolve_remote_ownership` drives both effects.

#[cfg(feature = "adaptive-sampling")]
mod adaptive_view;
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
#[cfg(feature = "beam")]
mod ray_beams;
mod redraw_gate;
mod remote_lane;
mod reshade_view;
mod scanline;
#[cfg(feature = "beam")]
mod still_beams;
mod tray_shadow_worker;
mod viewport_cache;
mod whitted_scanline;

pub use context::{
    ColorTarget, MaterialOverrides, MaterialSources, PlanesOwner, RenderContext, ViewColor,
    apply_material_overrides, load_env_map, resolve_material, resolve_material_with_override,
};
pub use denoise::{
    DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame, tonemap_running_average,
};
pub use gpu_backend::PREPARING_STATUS as GPU_PREPARING_STATUS;
pub use metrics::hash_planes;
pub use redraw_gate::RedrawGate;

mod pacing;
pub mod progress;
mod worker;
pub use worker::spawn_render_thread;
