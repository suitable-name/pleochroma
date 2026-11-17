//! The background thread that renders every frame and writes the PNG sequence.
//!
//! It overlays the selected metrics and hands off to `encode` once the sweep finishes or is
//! cancelled. Kept separate from `mod.rs`'s UI-thread wiring so neither file grows past
//! clippy's function-length lint.
//!
//! # Pausing the live viewport for the whole video
//!
//! A local video renders on the GPU adapter and CPU that the live viewport also uses
//! (`render::render_frame_rgba`), so it follows the same rule as a still-image export:
//! the app runs one GPU program at a time. The caller increments
//! `RenderContext::export_active_count` before [`spawn`] (`mod.rs::start_video_run`), and
//! [`finish_video_export`] is the one place that decrements it. It runs on every terminal
//! path ([`report_cancelled`], [`report_failure`], [`report_done`]) and after a caught
//! panic in [`spawn`], so the viewport cannot stay paused once the video ends.
//!
//! A `RemoteOnly` video claims no pause ([`pauses_viewport`]). It traces nothing locally,
//! so the viewport stays usable for the whole video.

use super::{
    encode,
    frames::{self, FrameReporter, FramesOutcome},
    metrics, params,
};
use crate::{
    ActivityModel, MainWindow, TiltModel, TiltVideoExportModel,
    bridge::{
        export_thread::{RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::progress_eta::{EtaEstimator, format_eta},
    settings::LocalComputeTarget,
};
use pleochroma::{
    color::ColorSpace,
    renderer::{accel::PathCacheSettings, accel_beam::BeamSettings, adaptive::AdaptiveSettings},
};
use slint::ComponentHandle;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Instant,
};

/// Everything one video export run needs, built once on the UI thread.
///
/// Every field is `Send`, the same bar as `bridge::export_thread::spawn_export`'s request
/// state. Render jobs build the same request (`gui::render_jobs::convert`) and run it through
/// [`frames::render_frames`].
pub struct VideoExportRequest {
    pub scene: SceneSnapshot,
    /// The sweep axes in play order (indices into `PROFILE_AZIMUTHS_DEG`), never empty.
    /// Each axis is one `start -> end` pass; several run back to back (`params::frame_poses`).
    pub axes: Vec<usize>,
    pub start_deg: f64,
    pub end_deg: f64,
    pub step_deg: f64,
    pub total_frames: usize,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub samples_per_pixel: u32,
    pub color_space: ColorSpace,
    pub out_dir: PathBuf,
    pub out_name: String,
    pub selection: metrics::MetricSelection,
    /// One set of curves per entry of `axes`, in the same order.
    pub curves: Vec<metrics::MetricCurves>,
    pub keep_frames: bool,
    /// The "Compute" choice (`RemoteOnly` traces nothing locally, see the module doc), the
    /// remote endpoint (`AppSettings::remote`) and the "Transfer" choice.
    pub remote: RemoteSelection,
    /// The persisted CPU/CPU+GPU/GPU setting (`RenderContext::local_compute_target`).
    pub local_compute: LocalComputeTarget,
    /// "Adaptive Sampling": each frame stops its tiles once they are clean enough, with
    /// `samples_per_pixel` as the cap. `None` is the plain render.
    pub adaptive: Option<AdaptiveSettings>,
    /// "Path Cache": each frame shades its stone from cached light paths. `persist` is
    /// "Keep Path Cache for This Video". `None` is the plain render.
    pub path_cache: Option<PathCacheSettings>,
    /// "Beam Tracing (Faceted Stones)": each frame beam-traces its eligible stone. `None`
    /// is the plain render.
    pub beam: Option<BeamSettings>,
}

impl VideoExportRequest {
    /// Every frame of the video, in order.
    #[must_use]
    pub fn poses(&self) -> Vec<params::FramePose> {
        params::frame_poses(&self.axes, self.start_deg, self.end_deg, self.step_deg)
    }
}

/// Spawns the background render loop for `request`, without blocking the UI thread.
///
/// `cancel` is checked between frames, not mid-frame: a frame already runs in parallel
/// across the local lanes and the remote worker, so stopping partway would waste it.
///
/// `activity_id` is the `crate::ActivityModel` id registered for this run. Every exit
/// path finishes it, and each frame reports its progress to it. `render_ctx` is used only
/// to resume the live viewport when the run ends (see the module doc). The caller has
/// already incremented `export_active_count`.
pub(super) fn spawn(
    ui_weak: slint::Weak<MainWindow>,
    render_ctx: Arc<Mutex<RenderContext>>,
    request: VideoExportRequest,
    cancel: Arc<AtomicBool>,
    activity_id: i32,
) {
    let pausing = pauses_viewport(&request);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&ui_weak, &render_ctx, &request, &cancel, activity_id);
        }));
        if let Err(payload) = result {
            // `run`'s exit paths decrement `export_active_count` themselves. After a
            // caught panic none of them ran, so `report_failure` does it here.
            let message = panic_message(&*payload);
            report_failure(
                &ui_weak,
                &render_ctx,
                pausing,
                &format!("Video export failed unexpectedly: {message}"),
                activity_id,
            );
        }
    });
}

/// Turns a `catch_unwind` payload into text, the same way
/// `gui::batch::preview::engine::panic_message` and `bridge::export_thread::spawn_export`
/// do.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Decrements `export_active_count`, resuming the live viewport. The only place that
/// decrements it; every terminal path calls it.
///
/// `pausing` is [`pauses_viewport`] for this run. A `RemoteOnly` video never incremented
/// the count, so it must not decrement it either.
fn finish_video_export(render_ctx: &Arc<Mutex<RenderContext>>, pausing: bool) {
    if pausing {
        RenderContext::lock(render_ctx).export_active_count -= 1;
    }
}

/// Whether `request`'s run holds the live-viewport pause: false only under `RemoteOnly`,
/// which traces nothing on this computer. `mod.rs::start_video_run` increments
/// `export_active_count` exactly when this is true, and [`finish_video_export`]
/// decrements it exactly when this is true.
pub(super) const fn pauses_viewport(request: &VideoExportRequest) -> bool {
    request.remote.compute_target.pauses_live_viewport()
}

fn run(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) {
    let digits = params::frame_number_digits(request.total_frames);
    let pausing = pauses_viewport(request);
    let frame_paths = match render_all_frames(ui_weak, request, cancel, activity_id) {
        FramesOutcome::Done(paths) => paths,
        FramesOutcome::Cancelled { .. } => {
            report_cancelled(ui_weak, render_ctx, pausing, activity_id);
            return;
        }
        FramesOutcome::Failed { message, .. } => {
            report_failure(ui_weak, render_ctx, pausing, &message, activity_id);
            return;
        }
    };

    let outcome = encode::encode(
        &request.out_dir,
        &frame_paths,
        digits,
        request.fps,
        &request.out_name,
    );
    if !request.keep_frames
        && matches!(
            outcome,
            encode::EncodeOutcome::Mp4(_) | encode::EncodeOutcome::Gif { .. }
        )
    {
        for path in &frame_paths {
            let _ = std::fs::remove_file(path);
        }
    }
    report_done(
        ui_weak,
        render_ctx,
        pausing,
        &request.out_dir,
        &outcome,
        activity_id,
    );
}

/// Renders every frame of `request`'s sweep through [`frames::render_frames`].
///
/// Progress goes to the dialog and the status strip through a [`SlintFrameReporter`].
/// Every frame is rendered: a direct export always starts a fresh folder. The compute
/// setup is described on `frames::render_frames`.
fn render_all_frames(
    ui_weak: &slint::Weak<MainWindow>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) -> FramesOutcome {
    // Time-remaining estimator, fed once per completed frame (`frames_done / frames_total`).
    // Fresh for each call: a new run's render rate says nothing about the last one. See
    // `gui::progress_eta` for why it is a rolling regression.
    let mut reporter = SlintFrameReporter {
        ui_weak: ui_weak.clone(),
        activity_id,
        eta: EtaEstimator::default(),
        work: None,
    };
    frames::render_frames(request, false, cancel, &mut reporter)
}

/// What a tilt view (the hover preview) of `scene` leaves out.
///
/// One sentence per effect: the hover preview does not draw the stone shadow or the tray
/// reflection. `None` when nothing is left out. Tilt videos draw both and use no note.
pub(in crate::gui) fn tilt_views_note(scene: &SceneSnapshot) -> Option<String> {
    omitted_effects_note(
        scene.stone_shadow_enabled(),
        scene.tray_reflection_applies(),
    )
}

/// The sentences of [`tilt_views_note`] for the effects that are on, joined by a space.
fn omitted_effects_note(stone_shadow: bool, tray_reflection: bool) -> Option<String> {
    let mut sentences = Vec::new();
    if stone_shadow {
        sentences.push("Tilt views do not show the stone shadow yet.");
    }
    if tray_reflection {
        sentences.push("Tilt views don't show the tray reflection.");
    }
    (!sentences.is_empty()).then(|| sentences.join(" "))
}

/// Writes frame progress into `TiltVideoExportModel` and the status strip's `ActivityChip`.
struct SlintFrameReporter {
    ui_weak: slint::Weak<MainWindow>,
    activity_id: i32,
    eta: EtaEstimator,
    /// Rendering work done as of the next saved frame (`FrameReporter::work_done`). Copied
    /// frames cost nothing, so the estimate follows this, not the frame count.
    work: Option<f64>,
}

impl FrameReporter for SlintFrameReporter {
    fn frame_progress(&mut self, index: usize, total: usize, fraction: f32, note: Option<String>) {
        report_frame_progress(
            &self.ui_weak,
            index,
            total,
            fraction,
            note,
            self.activity_id,
        );
    }

    fn frame_saved(&mut self, _index: usize, frames_done: usize, total: usize, frame_secs: f32) {
        let now = Instant::now();
        let by_count = frames_done as f64 / total.max(1) as f64;
        let work = self.work.take().unwrap_or(by_count);
        self.eta.observe(now, work);
        report_progress(
            &self.ui_weak,
            frames_done,
            total,
            frame_secs,
            format_eta(self.eta.eta(now)),
            self.activity_id,
        );
    }

    fn work_done(&mut self, fraction: f64) {
        self.work = Some(fraction);
    }
}

/// Reports one completed frame: its counter, its time, and the status strip's fraction.
///
/// [`report_frame_progress`] adds sample progress within the frame, so the bar moves
/// continuously.
///
/// The caller formats `eta_text` (`gui::progress_eta::format_eta`), which keeps this a
/// plain UI-thread setter.
fn report_progress(
    ui_weak: &slint::Weak<MainWindow>,
    done: usize,
    total: usize,
    last_frame_secs: f32,
    eta_text: String,
    activity_id: i32,
) {
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_current_frame(i32::try_from(done).unwrap_or(i32::MAX));
        let fraction = params::export_progress_fraction(done, total);
        model.set_progress(fraction);
        model.set_last_frame_seconds(last_frame_secs);
        model.set_eta_text(eta_text.into());
        // Same fraction as the dialog bar above, for the status strip's `ActivityChip`.
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, fraction);
    });
}

/// Reports one sample-batch tick from `ExportProgress` inside a frame.
///
/// The fraction is blended with the `frame_index` frames already done, so the bar advances
/// through a frame instead of jumping at frame boundaries.
///
/// A remote-lane `note` (a worker declined, dropped a chunk, or was paused after repeated
/// failures) goes to the status message, as the still-image export does with
/// `ExportProgress::note`.
fn report_frame_progress(
    ui_weak: &slint::Weak<MainWindow>,
    frame_index: usize,
    total_frames: usize,
    frame_fraction: f32,
    note: Option<String>,
    activity_id: i32,
) {
    let overall_fraction = (frame_index as f32 + frame_fraction) / (total_frames.max(1) as f32);
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_progress(overall_fraction);
        if let Some(note) = note {
            model.set_status_message(note.into());
        }
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, overall_fraction);
    });
}

fn report_cancelled(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pausing: bool,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_status_message("Video export cancelled.".into());
        model.set_eta_text(String::new().into());
        // Run in the background, nothing on screen would say so.
        if !ui.global::<TiltModel>().get_dialog_open() {
            crate::gui::show_toast(&ui, "Video export cancelled.", "info");
        }
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_failure(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pausing: bool,
    message: &str,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let message = message.to_string();
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(true);
        // A toast too: the dialog may be closed, and the error stays until dismissed.
        crate::gui::show_toast(&ui, &message, "error");
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_done(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pausing: bool,
    out_dir: &Path,
    outcome: &encode::EncodeOutcome,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let (_, message) = frames::encode_outcome_message(outcome, out_dir);
    let toast_kind = if matches!(outcome, encode::EncodeOutcome::Mp4(_)) {
        "success"
    } else {
        "info"
    };
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_progress(1.0);
        // A toast too: the popup may be closed (run in the background).
        crate::gui::show_toast(&ui, &message, toast_kind);
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

#[cfg(test)]
mod tests {
    use super::{super::render, *};
    use crate::bridge::export_thread::AccumulationCarry;
    use pleochroma::renderer::gpu_backend::GpuBackend;

    fn test_config() -> render::VideoComputeConfig {
        render::VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        }
    }

    /// What a tilt view leaves out is named once per effect, the shadow first, and nothing is
    /// said when nothing is left out.
    #[test]
    fn the_tilt_note_names_each_omitted_effect_and_is_silent_without_one() {
        assert_eq!(omitted_effects_note(false, false), None);
        assert_eq!(
            omitted_effects_note(true, false).as_deref(),
            Some("Tilt views do not show the stone shadow yet.")
        );
        assert_eq!(
            omitted_effects_note(false, true).as_deref(),
            Some("Tilt views don't show the tray reflection.")
        );
        assert_eq!(
            omitted_effects_note(true, true).as_deref(),
            Some(
                "Tilt views do not show the stone shadow yet. \
                 Tilt views don't show the tray reflection."
            )
        );
    }

    /// The note reads the scene's effective effects: a plain scene says nothing, a glossy tray
    /// and a shadow each add their sentence.
    #[test]
    fn the_tilt_note_follows_the_scenes_effective_effects() {
        let capture = |ctx: RenderContext| {
            SceneSnapshot::capture(&Mutex::new(ctx)).expect("Diamond resolves")
        };
        assert_eq!(tilt_views_note(&capture(RenderContext::default())), None);
        let both = capture(RenderContext {
            stone_shadow: true,
            tray_reflection: 0.04,
            ..RenderContext::default()
        });
        assert_eq!(
            tilt_views_note(&both).as_deref(),
            Some(
                "Tilt views do not show the stone shadow yet. \
                 Tilt views don't show the tray reflection."
            )
        );
    }

    /// A run stopped partway leaves only the frames already written, and no video.
    ///
    /// `run` calls `encode::encode` only with the `FramesOutcome::Done` of
    /// `render_all_frames`; a cancelled sweep returns before encoding. That function needs
    /// a live `MainWindow`, so this drives the per-frame render and save steps directly and
    /// checks that no `.mp4` or `.gif` exists beside the frames.
    #[test]
    fn a_run_stopped_partway_through_leaves_only_the_frames_already_written_and_no_video() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let total_frames = 3;
        let dir =
            std::env::temp_dir().join(format!("tilt_video_cancel_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Only frame 1 of 3 is saved, as after a cancel following the first frame.
        let path = dir.join(params::frame_file_name(1, total_frames));
        let config = test_config();
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render::render_frame_rgba(
            &scene,
            8,
            8,
            1,
            0.0,
            1.5,
            ColorSpace::Srgb,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let render::FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        frames::save_png_atomic(&path, 8, 8, &rgba).unwrap();

        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "only the one frame written before the simulated cancel should exist"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.path().extension().and_then(|ext| ext.to_str()) != Some("mp4")),
            "no .mp4 must exist for a run that stopped before encoding was ever reached"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.path().extension().and_then(|ext| ext.to_str()) != Some("gif")),
            "no .gif must exist for a run that stopped before encoding was ever reached"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_png_round_trips_a_frame_to_disk() {
        let dir =
            std::env::temp_dir().join(format!("tilt_video_save_png_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame_001.png");
        let rgba = vec![255u8; 4 * 4 * 4];
        frames::save_png_atomic(&path, 4, 4, &rgba).unwrap();
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
