//! "Add to Queue" in the tilt video section: the same request the direct "Start Video
//! Export" builds, frozen as a render job instead of started.
//!
//! `gui::tilt::video_export::prepare_queued_video` resolves the export folder and the
//! finished stone and builds the request without creating or starting anything; this
//! module reserves the frame folder against the disk and the queue, freezes the request
//! and stores it. The folder itself is created by the executor when the job first runs.

use super::{controller::unix_now, wiring::Deps};
use crate::{
    MainWindow, TiltVideoExportModel,
    gui::{
        render::render_export::wiring::design_naming, tilt::video_export, tutorial_events::raise,
    },
};
use pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG;
use pleochroma_editor::guide::viewing_events::RENDER_JOB_ADDED;
use pleochroma_render_jobs::{codec, job::DesignInfo, paths::reserve_unique_folder};
use pleochroma_vault::model::render_job::NewRenderJob;
use slint::ComponentHandle;
use std::path::PathBuf;

/// What a tilt video job's detail line says about the sweep and the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFacts {
    pub start_deg: f64,
    pub end_deg: f64,
    pub step_deg: f64,
    pub frames: usize,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// The material's name.
    pub material: String,
}

/// The name a tilt video job has in the list: `Design · tilt video · axis 45°`.
pub fn video_label(design: &str, axes_deg: &[f32]) -> String {
    let angles: Vec<String> = axes_deg
        .iter()
        .map(|deg| format!("{deg:.0}\u{b0}"))
        .collect();
    let word = if angles.len() > 1 { "axes" } else { "axis" };
    format!(
        "{design} \u{b7} tilt video \u{b7} {word} {}",
        angles.join(" + ")
    )
}

/// An angle in the shortest form: `-90`, `0.5`.
fn degrees(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The detail line of a tilt video job:
/// `Sapphire · -90° to 90° in 1° steps · 181 frames · 30 fps · 1280 × 720 · 256 samples`.
pub fn video_summary(facts: &VideoFacts) -> String {
    format!(
        "{} \u{b7} {}\u{b0} to {}\u{b0} in {}\u{b0} steps \u{b7} {} frames \u{b7} {} fps \u{b7} {} \u{d7} {} \u{b7} {} samples",
        facts.material,
        degrees(facts.start_deg),
        degrees(facts.end_deg),
        degrees(facts.step_deg),
        facts.frames,
        facts.fps,
        facts.width,
        facts.height,
        facts.samples,
    )
}

fn refuse(ui: &MainWindow, message: &str) {
    let model = ui.global::<TiltVideoExportModel>();
    model.set_has_error(true);
    model.set_status_message(message.into());
}

/// "Add to Queue" for the tilt video of `axis_index`.
pub(super) fn add_tilt_video_job(ui: &MainWindow, deps: &Deps, axis_index: i32) {
    let render_ctx = deps.render_ctx.clone();
    let settings_store = deps.settings_store.clone();
    let deps = deps.clone();
    video_export::prepare_queued_video(
        ui,
        &render_ctx,
        &settings_store,
        axis_index,
        move |ui, prepared| match prepared {
            // The person closed the folder picker.
            Ok(None) => {
                let model = ui.global::<TiltVideoExportModel>();
                model.set_has_error(false);
                model.set_status_message("Nothing was added.".into());
            }
            Ok(Some(request)) => queue_request(ui, &deps, request),
            Err(message) => refuse(
                ui,
                &message.replacen("Nothing was exported", "Nothing was added", 1),
            ),
        },
    );
}

/// Freezes `request` and stores it as a job.
fn queue_request(ui: &MainWindow, deps: &Deps, mut request: video_export::run::VideoExportRequest) {
    let taken = {
        let db = deps
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.render_job_output_paths()
    };
    let taken: Vec<PathBuf> = match taken {
        Ok(paths) => paths.into_iter().map(PathBuf::from).collect(),
        Err(error) => {
            refuse(
                ui,
                &format!("The render job list could not be read: {error}"),
            );
            return;
        }
    };
    // Not created here: the executor creates the frame folder when the job first runs.
    request.out_dir = reserve_unique_folder(&request.out_dir, &taken, &|path| path.exists());

    let (design, designer, shape, ri) = design_naming(ui, &deps.render_ctx);
    let axes_deg: Vec<f32> = request
        .axes
        .iter()
        .map(|&index| PROFILE_AZIMUTHS_DEG.get(index).copied().unwrap_or(0.0))
        .collect();
    let label = video_label(&design, &axes_deg);
    let summary = job_summary(&request);
    let now = unix_now();
    let design_info = DesignInfo {
        title: design,
        designer,
        shape,
        ri,
        material: request.scene.material.name.clone(),
    };
    let video_name = request.out_name.clone();
    let built =
        super::convert::build_video_job(&request, label.clone(), design_info, video_name, now)
            .and_then(|job| {
                codec::to_text(&job)
                    .map(|text| (job.format, text))
                    .map_err(|e| e.to_string())
            });
    let (format, text) = match built {
        Ok(built) => built,
        Err(message) => {
            refuse(ui, &message);
            return;
        }
    };

    let output = request.out_dir.to_string_lossy().into_owned();
    let stored = {
        let db = deps
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.add_render_job(
            &NewRenderJob {
                kind: "tilt_video",
                label: &label,
                summary: &summary,
                frames_total: u32::try_from(request.total_frames).unwrap_or(u32::MAX),
                output_path: &output,
                snapshot_version: format,
                snapshot: &text,
            },
            now,
        )
    };
    // A zoned stone's zones are part of the job text (format 5), so the one write queues it whole.
    match stored {
        Ok(_) => {
            super::controller::with_controller(super::controller::JobController::refresh);
            announce_added(ui, &request);
        }
        Err(error) => refuse(ui, &format!("The job could not be saved: {error}")),
    }
}

/// The queue row's summary line for `request`: the video's facts, then its adaptive, path-cache
/// and beam details.
fn job_summary(request: &video_export::run::VideoExportRequest) -> String {
    video_summary(&VideoFacts {
        start_deg: request.start_deg,
        end_deg: request.end_deg,
        step_deg: request.step_deg,
        frames: request.total_frames,
        fps: request.fps,
        width: request.width,
        height: request.height,
        samples: request.samples_per_pixel,
        material: request.scene.material.name.clone(),
    }) + &super::convert::adaptive_detail(request.adaptive.as_ref())
        + &super::convert::path_cache_detail(request.path_cache.as_ref())
        + &super::convert::beam_detail(request.beam.as_ref())
}

/// Tells the person the job of `request` was queued (and what tilt videos leave out: the stone
/// shadow and the tray reflection), through the dialog's status line and a toast.
fn announce_added(ui: &MainWindow, request: &video_export::run::VideoExportRequest) {
    let mut message =
        "Added 1 render job to the queue. Start it from File > Render Jobs...".to_owned();
    if let Some(note) = video_export::run::tilt_views_note(&request.scene) {
        message.push(' ');
        message.push_str(&note);
    }
    let message = message.as_str();
    let model = ui.global::<TiltVideoExportModel>();
    model.set_has_error(false);
    model.set_status_message(message.into());
    crate::gui::show_toast(ui, message, "info");
    raise(ui, RENDER_JOB_ADDED);
}
