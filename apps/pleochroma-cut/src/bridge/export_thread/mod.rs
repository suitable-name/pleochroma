//! High-resolution still export, run off the interactive render thread.
//!
//! The live viewport owns a buffer sized to its widget, so an export must not touch or block
//! it. The export takes its own read-only [`SceneSnapshot`] under a short lock, accumulates
//! into its own buffer sized to the export, runs on its own worker thread (parallel inside via
//! `thread::scope`) and reports progress and cancellation through an [`ExportHandle`].
//!
//! # Wide-gamut export
//!
//! Tone-mapping branches on the `ColorSpace`. `Srgb` uses the `xyz_to_srgb_gamma` path. Other
//! spaces use `ColorSpace::encode` with `ToneMap::AcesFilmic { exposure: 1.0 }`, which matches
//! `xyz_to_srgb_gamma`'s brightness, so only the primaries and transfer curve change. The PNG
//! embeds an ICC profile for every non-`Srgb` space so pixels are never read as sRGB.
//!
//! `ColorSpace::AcesCg` is not offered by `export_dialog.slint`: it is scene-linear, and 8-bit
//! quantization of that data would band.
//!
//! Submodules: [`params`] (validation, default output path), [`scene_snapshot`]
//! ([`SceneSnapshot`] capture), [`batch`] (hybrid calibration and dispatch, CPU scanline
//! tracer), [`tonemap_png`] (tone-map, PNG and ICC output), [`worker`] (`run_export` and the
//! concurrent local and remote dispatch and merge). This file holds the entry point
//! (`spawn_export`) and its handle and outcome types.

// `pub` because `bridge::preview_render` reuses `batch::render_batch` for its CPU fallback;
// the other items in `batch` stay `pub(super)`.
pub mod batch;
mod cache_run;
pub mod filename_template;
// Re-export of `pleochroma::render_setup::icc_profile::build` at this crate's old path, for
// `render_setup_pins`. Test-only: `save_png` calls `encode_png_with_icc` directly.
#[cfg(test)]
mod icc_profile;
mod params;
pub mod path_cache_memory;
mod preview;
pub mod remote;
mod sample_cursor;
mod scene_snapshot;
mod tonemap_png;
mod worker;

#[cfg(test)]
mod cache_run_tests;
#[cfg(test)]
mod hybrid_export_tests;
#[cfg(test)]
mod render_setup_pins;
#[cfg(test)]
mod scene_snapshot_inclusions_tests;
#[cfg(test)]
mod tests;

pub use filename_template::{DEFAULT_TEMPLATE, TemplateContext, resolve_export_path};
pub use params::{ComputeTarget, ExportParams, RemoteSelection, validate_export_params};
pub use remote::{forget_final_picture_refusals, probe_remote};
pub use scene_snapshot::SceneSnapshot;
// Per-image render entry points, also used by `gui::tilt::video_export` for each frame.
pub use worker::{
    AccumulationCarry, LinearImage, LinearImageKind, RawTapSink, RenderTaps, RenderedImage,
    render_image_rgba,
};

use crate::settings::LocalComputeTarget;
#[cfg(feature = "beam")]
use pleochroma::renderer::accel_beam::BeamPhase;
use pleochroma::{
    color::ColorSpace,
    renderer::{accel::PathCacheSettings, accel_beam::BeamSettings, adaptive::AdaptiveSettings},
};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, Weak};
#[cfg(feature = "beam")]
use std::time::Duration;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
// `run_export` (no taps) is called only by the tests; the app's exports pass their taps.
#[cfg(test)]
pub use worker::run_export;
pub use worker::run_export_tapped;

/// The serializable scene a render job stores: the remote wire's `SceneState` for `snapshot`
/// at `width x height`, at the snapshot's own camera.
///
/// The remote dispatch uses the same conversion, so a job holds what a worker would receive.
/// The HDR map is named by hash only; the caller stores its path (`gui::render_jobs::convert`).
#[must_use]
pub fn scene_state_for_job(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
) -> pleochroma_net::SceneState {
    scene_state_for_job_with(
        snapshot,
        width,
        height,
        pleochroma::optics::raytracer::default_estimator(),
    )
}

/// [`scene_state_for_job`] for the scene's `estimator` instead of the process default.
#[must_use]
pub fn scene_state_for_job_with(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
    estimator: pleochroma::optics::raytracer::Estimator,
) -> pleochroma_net::SceneState {
    remote::scene_state_from_snapshot_with(
        snapshot,
        width,
        height,
        snapshot.yaw,
        snapshot.pitch,
        estimator,
    )
}

/// How an export ended.
#[derive(Debug)]
pub enum ExportOutcome {
    Completed(PathBuf),
    Cancelled,
    Failed(String),
}

/// One progress update from an in-flight export, passed to `on_progress` after each sample batch.
///
/// `preview` is `Some` only when the `preview` submodule's rate limit allows a new thumbnail; on
/// `None` the caller keeps the displayed image.
pub struct ExportProgress {
    /// 0.0..=1.0, `samples_done as f32 / samples_total as f32`.
    pub fraction: f32,
    /// Samples completed so far.
    pub samples_done: u32,
    /// Samples the export will take in total.
    pub samples_total: u32,
    /// A small sRGB thumbnail of the accumulation so far, local plus remote (see
    /// `preview::downsample_preview`).
    pub preview: Option<SharedPixelBuffer<Rgba8Pixel>>,
    /// A one-off user-facing status line about the remote half (a fallback to local-only,
    /// or a recovered worker failure). `None` on other ticks; show it once.
    pub note: Option<String>,
    /// Phase and time left of a beam-traced render. `None` for other renders.
    #[cfg(feature = "beam")]
    pub beam: Option<BeamStatus>,
}

/// The phase and estimated time left of a beam-traced render.
///
/// The render-job rows read the phase from here rather than parsing the "Beams:" note.
#[cfg(feature = "beam")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeamStatus {
    /// Tracing the beam trees, or shading and running the residual ("finishing").
    pub phase: BeamPhase,
    /// The estimated time left, from the per-batch rate; `None` before the first batch.
    pub eta: Option<Duration>,
}

/// Handle returned by `spawn_export`. Cancellation is cooperative: the worker checks the
/// flag between sample batches.
pub struct ExportHandle {
    cancel: Arc<AtomicBool>,
}

impl ExportHandle {
    /// Requests cancellation; the running job stops at its next check.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Spawns the export worker thread.
///
/// `on_progress` runs on the UI event loop after each sample batch; `on_done` runs exactly
/// once, when the export finishes, is cancelled or fails. `color_space` selects the PNG's
/// gamut and transfer curve. `remote` selects the engines, endpoint and transfer.
/// `adaptive`, `path_cache` and `beam` are the dialog's sampling, path cache and beam choices;
/// `None` is the ordinary render. A stone that beams cannot render falls back to the ordinary
/// render with a note. Without the `beam` or `adaptive-sampling` feature those options are
/// always `None` here, and a job file that carries them renders without them, with a note.
///
/// # `on_done` fires on every exit path, including a panic
///
/// The worker body runs inside `std::panic::catch_unwind`. Otherwise a panic would skip
/// `on_done` and leave `RenderContext::export_active` set, freezing the live viewport.
/// A caught panic is reported as `ExportOutcome::Failed`.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one export request's own identity \
              (scene, output params/path/color-space, the remote selection, the local \
              compute choice, and the two UI callbacks) -- bundling them into a struct \
              would just move the same count into field access, not reduce it"
)]
pub fn spawn_export<T, P, D>(
    ui_weak: Weak<T>,
    scene: SceneSnapshot,
    params: ExportParams,
    adaptive: Option<AdaptiveSettings>,
    path_cache: Option<PathCacheSettings>,
    beam: Option<BeamSettings>,
    color_space: ColorSpace,
    output_path: PathBuf,
    remote: RemoteSelection,
    local_compute: LocalComputeTarget,
    on_progress: P,
    on_done: D,
) -> ExportHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, ExportProgress) + Send + 'static + Clone,
    D: Fn(&T, ExportOutcome) + Send + 'static + Clone,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let ui_weak_done = ui_weak.clone();

    thread::spawn(move || {
        let progress_ui_weak = ui_weak;
        let report_progress = move |progress: ExportProgress| {
            let on_progress = on_progress.clone();
            let _ = progress_ui_weak.upgrade_in_event_loop(move |ui| on_progress(&ui, progress));
        };
        let taps = RenderTaps {
            adaptive,
            path_cache,
            beam,
            ..RenderTaps::default()
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_export_tapped(
                &scene,
                params,
                color_space,
                &output_path,
                &remote,
                local_compute,
                &taps,
                &cancel_worker,
                report_progress,
            )
        }))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "the export worker panicked".to_string());
            ExportOutcome::Failed(format!("Export failed unexpectedly: {message}"))
        });
        let _ = ui_weak_done.upgrade_in_event_loop(move |ui| on_done(&ui, outcome));
    });

    ExportHandle { cancel }
}
