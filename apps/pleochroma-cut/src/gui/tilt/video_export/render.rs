//! Per-frame rendering for the tilt performance video.
//!
//! Each frame goes through `bridge::export_thread::render_image_rgba`, the per-image entry
//! point the still-image export uses too, so both share one render core (local CPU/GPU/hybrid
//! and remote) and cannot drift apart.
//!
//! [`VideoComputeConfig`] holds the compute settings a still-image export reads: the
//! local CPU/GPU/hybrid setting (`RenderContext::local_compute_target`) and the remote
//! endpoint (`AppSettings::remote`), plus the section's "Transfer" choice. The section's
//! "Compute" pill sets `compute_target`. Under `Both`, the remote-unavailable fallback in
//! `render_image_rgba` decides frame by frame whether the remote is reachable. Under
//! `RemoteOnly` there is no fallback, so an unreachable remote fails the frame.
//!
//! # Final picture only
//!
//! With "Transfer: Final picture only" each frame is one `FinalImageRequest`: the remote
//! renders and tone-maps the frame, and the viewer decodes the PNG to RGBA8 and overlays
//! it like a local frame. A remote that answers `UNSUPPORTED_REQUEST` is remembered for
//! the rest of the sweep (through the shared `AccumulationCarry`), and later frames use
//! full data.

use crate::{
    bridge::export_thread::{
        AccumulationCarry, ExportParams, ExportProgress, RemoteSelection, RenderedImage,
        SceneSnapshot, render_image_rgba,
    },
    settings::LocalComputeTarget,
};
use pleochroma::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use std::sync::atomic::AtomicBool;

/// The compute configuration of one whole video, read once when it starts and reused
/// for every frame of the sweep.
pub(super) struct VideoComputeConfig {
    /// The "Compute" choice, the remote endpoint (snapshotted once per video) and the
    /// "Transfer" choice.
    pub(super) remote: RemoteSelection,
    /// The persisted CPU/CPU+GPU/GPU setting, as it was when the video started.
    pub(super) local_compute: LocalComputeTarget,
}

/// One frame's outcome from [`render_frame_rgba`].
///
/// `Rendered` is tone-mapped RGBA8 whichever side did the tone-mapping: a video frame has
/// no PNG or ICC step, and the overlay draws straight onto this buffer in `run.rs`.
pub(super) enum FrameOutcome {
    /// Tone-mapped RGBA8 bytes, `width * height * 4` long.
    Rendered(Vec<u8>),
    /// `cancel` was observed before this frame finished.
    Cancelled,
    /// The render failed outright. Only reachable under `ComputeTarget::RemoteOnly`,
    /// which has no local fallback (see `AccumulationOutcome::Failed`).
    Failed(String),
}

/// Renders one frame at `(cam_yaw, cam_pitch)` through `render_image_rgba`.
///
/// This is the per-image entry point the still-image export uses. Only the camera pose
/// changes per frame: material, geometry, light, exposure and bounce cap come from `scene`.
///
/// `gpu` and `carry` belong to the whole video (owned by `frames::render_frames`), not to
/// one frame, so the accumulation carry is reused across frames. `config` is fixed for the
/// sweep.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one frame's own render request \
              (scene/pose, dimensions/samples, color space, the whole-video compute \
              config/GPU backend/calibration carry, cancellation, and progress \
              reporting) -- bundling them into a struct would just move the same \
              count into field access, not reduce it, matching \
              `bridge::export_thread::render_image_rgba`'s own identical shape"
)]
pub(super) fn render_frame_rgba(
    scene: &SceneSnapshot,
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    cam_yaw: f32,
    cam_pitch: f32,
    color_space: ColorSpace,
    config: &VideoComputeConfig,
    gpu: &GpuBackend,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> FrameOutcome {
    let params = ExportParams {
        engine: scene.engine,
        width,
        height,
        samples_per_pixel: samples_per_pixel.max(1),
        max_bounces: scene.max_bounces,
    };
    match render_image_rgba(
        scene,
        cam_yaw,
        cam_pitch,
        params,
        color_space,
        &config.remote,
        gpu,
        config.local_compute,
        carry,
        cancel,
        report_progress,
    ) {
        RenderedImage::Cancelled => FrameOutcome::Cancelled,
        RenderedImage::Failed(message) => FrameOutcome::Failed(message),
        RenderedImage::Rgba(rgba) => FrameOutcome::Rendered(rgba),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use std::sync::Mutex;

    /// CPU-only smoke test: one tiny frame must come back as a full, opaque RGBA buffer.
    ///
    /// The multi-frame pipeline is covered by the end-to-end test in `tests.rs`; this
    /// isolates the render and tone-map step.
    #[test]
    fn render_frame_rgba_produces_a_full_opaque_buffer() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render_frame_rgba(
            &scene,
            8,
            8,
            1,
            0.6,
            0.5,
            ColorSpace::Srgb,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert!(
            rgba.chunks(4).all(|px| px[3] == 255),
            "every pixel must be fully opaque"
        );
    }

    #[test]
    fn render_frame_rgba_accepts_wide_gamut_color_spaces_too() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render_frame_rgba(
            &scene,
            4,
            4,
            1,
            0.0,
            1.4,
            ColorSpace::DisplayP3,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }

    /// The `AccumulationCarry` is reused across frames, not reset.
    ///
    /// With `Both` and no worker configured, the remote probe fails at once, without a
    /// network call (see `probe_remote`'s `NoWorkerConfigured` case), and emits one note.
    /// That note must appear once for the whole sweep, which shows the probe was not
    /// repeated per frame.
    #[test]
    fn a_no_worker_configured_note_is_surfaced_once_not_once_per_frame() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection {
                compute_target: crate::bridge::export_thread::ComputeTarget::Both,
                ..RemoteSelection::local_only()
            },
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let mut notes = Vec::new();
        for (cam_yaw, cam_pitch) in [(0.1f32, 0.1f32), (0.2f32, 0.2f32), (0.3f32, 0.3f32)] {
            let notes = &mut notes;
            let outcome = render_frame_rgba(
                &scene,
                4,
                4,
                1,
                cam_yaw,
                cam_pitch,
                ColorSpace::Srgb,
                &config,
                &gpu,
                &mut carry,
                &cancel,
                |progress| {
                    if let Some(note) = progress.note {
                        notes.push(note);
                    }
                },
            );
            assert!(matches!(outcome, FrameOutcome::Rendered(_)));
        }
        assert_eq!(
            notes.len(),
            1,
            "the no-worker-configured note must surface once for the whole carry, not \
             once per frame: {notes:?}"
        );
    }

    /// A "Final picture only" remote that cannot deliver falls back to full data under `Both`.
    ///
    /// The remote here has no certificate bundle, so it fails before any network call. Every
    /// frame still renders, and the fallback is not retried: each note appears once.
    #[test]
    fn a_failing_final_picture_falls_back_to_full_data_once_per_sweep() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection {
                compute_target: crate::bridge::export_thread::ComputeTarget::Both,
                worker: Some(crate::settings::WorkerSettings {
                    address: "127.0.0.1:1".to_string(),
                    cert_dir: std::env::temp_dir()
                        .join("no-such-final-picture-bundle")
                        .display()
                        .to_string(),
                    ..crate::settings::WorkerSettings::default()
                }),
                transfer: crate::settings::ExportTransfer::FinalPicture,
                contribute_local: false,
            },
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let mut notes = Vec::new();
        for cam_yaw in [0.1_f32, 0.2, 0.3] {
            let notes = &mut notes;
            let outcome = render_frame_rgba(
                &scene,
                4,
                4,
                1,
                cam_yaw,
                0.2,
                ColorSpace::Srgb,
                &config,
                &gpu,
                &mut carry,
                &cancel,
                |progress| notes.extend(progress.note),
            );
            assert!(matches!(outcome, FrameOutcome::Rendered(ref rgba) if rgba.len() == 64));
        }
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("final-picture"), "{notes:?}");
    }
}
