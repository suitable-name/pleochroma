//! The fan-out export queue and its job stepping.
//!
//! [`ExportQueue`] and [`ExportJob`] hold the queue, the pure scene-transform helpers build each
//! job's [`bridge::export_thread::SceneSnapshot`], and [`start_next_export_job`]/
//! [`finish_export_queue`] step through the queue one job at a time. The `mod.rs` doc comment
//! explains why the jobs run one after another and why the queue sits behind `Arc<Mutex<_>>`.

use crate::{
    ExportModel, MainWindow,
    bridge::{
        export_thread::{self, ComputeTarget, SceneSnapshot, TemplateContext},
        render_thread::{RenderContext, load_env_map},
    },
    gui::{
        progress_eta::{EtaEstimator, format_eta},
        show_toast,
    },
    settings::{LightingPreset as SavedLightingPreset, LocalComputeTarget, model::PITCH_RANGE_RAD},
};
use pleochroma::{
    color::ColorSpace,
    renderer::{
        accel::PathCacheSettings, accel_beam::BeamSettings, adaptive::AdaptiveSettings,
        env_map::HdrOrientation,
    },
};
use slint::ComponentHandle;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

/// Maps the export dialog's "Compute" pill index (0, 1 or 2) to a [`ComputeTarget`], following
/// the convention of `gui::color_space_from_index`.
pub(in crate::gui) const fn compute_target_from_index(index: i32) -> ComputeTarget {
    match index {
        0 => ComputeTarget::LocalOnly,
        1 => ComputeTarget::RemoteOnly,
        _ => ComputeTarget::Both,
    }
}

/// Overrides a captured [`SceneSnapshot`]'s bounce cap with the export dialog's own choice,
/// instead of the live viewport's `max_bounces` that `SceneSnapshot::capture` read.
///
/// A separate pure step, so `capture` keeps its lock short. The local and the remote renders
/// both read this one field. Overriding it once, before the fan-out clones the base capture,
/// makes the base render and every preset render trace at the same cap.
#[must_use]
pub(in crate::gui) const fn apply_export_bounce_cap(
    mut scene: SceneSnapshot,
    max_bounces: u32,
) -> SceneSnapshot {
    scene.max_bounces = max_bounces;
    scene
}

/// Overlays a saved preset's light, camera and HDR-map fields onto a cloned base capture. The
/// rules are those of the live apply in `gui::render::lighting_presets`:
///
/// - The light, exposure, rig, glare, backdrop, head shadow, Stone Shadow and Tray Reflection
///   are always overlaid. Fields a preset lacks take their serde defaults (`LightingValues`).
/// - The camera pose is overlaid only when the preset carries one (`camera_yaw`/`camera_pitch`
///   both `Some`), with its roll and pan (an older pose is unrolled and centred). `None` leaves
///   the base capture's camera alone, pan included (see `settings::model::LightingPreset`).
/// - The HDR map is overlaid when the preset carries one and it loads, with the preset's own
///   rotation (yaw and "turns with the camera"). A map that cannot be loaded leaves the base
///   capture's environment in place, so a missing file does not fail the whole render. A preset
///   without a map clears the base capture's map, as the live apply does.
///
/// Material and geometry are never touched: a preset is a view, so every fanned-out render
/// shares the material and geometry of the base capture.
///
/// `mesh_bounding_radius` is the live figure the live apply and the orbit camera clamp `distance`
/// against. The caller reads it once before the fan-out starts.
#[must_use]
pub(super) fn apply_preset_to_scene(
    mut scene: SceneSnapshot,
    preset: &SavedLightingPreset,
    mesh_bounding_radius: f64,
) -> SceneSnapshot {
    // The scene fields as the one shared `LightingValues`, each limited the way the live
    // controls limit it, exactly as the live apply reads them.
    let values = preset.values().sanitised();
    scene.light_yaw = values.light_yaw_deg.to_radians();
    scene.light_pitch = values
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    scene.lighting_preset =
        crate::gui::optics::offered_lighting::offered_from_label(&values.lighting_rig);
    // A saved lighting is a complete look: its glare, backdrop, head shadow, Stone Shadow and
    // Tray Reflection come with it.
    scene.surface_glare = values.surface_glare;
    // Resolved at the saved lighting's own preset and pose (set just above), never the
    // base capture's: the tray's brightness follows the rig that lights it.
    scene.backdrop =
        values
            .backdrop
            .resolve(scene.lighting_preset, scene.light_yaw, scene.light_pitch);
    scene.head_shadow_deg = values.head_shadow_deg;
    scene.stone_shadow = values.stone_shadow;
    scene.tray_reflection = values.tray_reflection;
    // The base capture looked at the stone only if it asked for a shadow or a reflection;
    // a stone found not closed stays not closed.
    if scene.stone_closed {
        scene.stone_closed = SceneSnapshot::stone_closed_for(
            scene.stone_shadow,
            scene.tray_reflection,
            &scene.active_planes,
            &scene.tools,
        );
    }
    // The distance comes with a recorded camera pose only, matching `lighting_presets`'
    // own live-apply: a lighting-only preset keeps the base capture's zoom.
    if let Some(distance) = values.camera_distance {
        let (min_distance, max_distance) =
            crate::gui::render::camera_lighting::orbit_distance_bounds(mesh_bounding_radius);
        scene.distance = distance.clamp(min_distance, max_distance);
    }

    if let Some((yaw, pitch, roll)) = values.camera_pose() {
        scene.yaw = yaw;
        scene.pitch = pitch.clamp(-1.48, 1.48);
        scene.camera_roll = roll;
    }
    if let Some(pan) = values.pose_pan() {
        scene.camera_pan = pan;
    }

    if let Some(path) = &values.env_map_path {
        if let Ok(map) = load_env_map(path) {
            scene.env_map = Some(map);
            // The turn belongs to the map, as in the live apply (`keep_current_turn`): the
            // saved turn comes with the map it loads; a map that fails to load keeps the base
            // capture's map and its turn.
            scene.hdr_orientation = HdrOrientation {
                yaw: values.env_map_yaw_deg.to_radians(),
                follows_camera: values.env_map_follows_camera,
            };
        }
    } else {
        // No map saved means no map: the rig comes back, as when the lighting is applied live.
        scene.env_map = None;
        scene.hdr_orientation = HdrOrientation::default();
    }
    // Metered for the saved lighting's own rig and pose, as `RenderContext::effective_exposure`
    // meters the live view; an HDR map keeps the plain exposure.
    scene.exposure = if scene.env_map.is_some() {
        values.exposure
    } else {
        pleochroma::optics::raytracer::metered_exposure(
            values.exposure,
            scene.lighting_preset,
            scene.light_yaw,
            scene.light_pitch,
        )
    };

    scene
}

/// One render this export queue still needs to produce: the base current-view render
/// (`preset_label` empty) or one fanned-out preset render (`preset_label` the preset's
/// name, used both for the `{preset}` template variable and the progress label).
pub(in crate::gui) struct ExportJob {
    pub(in crate::gui) scene: SceneSnapshot,
    pub(in crate::gui) preset_label: String,
}

/// State shared by every job of one fan-out export.
///
/// It lives behind `Arc<Mutex<_>>` because the job callbacks run on other threads (see the
/// `mod.rs` doc comment).
///
/// # Progress: counting presets, not just samples within one image
///
/// The `total` jobs share the export dialog's one progress bar as a combined fraction,
/// `(current_index + this_job_fraction) / total`, so a five-preset export moves smoothly from 0%
/// to 100%. Per-job bars would snap back to 0% for each job, or reach 100% after the first one.
pub(super) struct ExportQueue {
    pub(super) jobs: VecDeque<ExportJob>,
    pub(super) total: usize,
    /// 0-based index of the job currently running (or about to run) -- also `{preset}`'s
    /// position for progress-label purposes; see [`ExportQueue`]'s own doc comment.
    pub(super) current_index: usize,
    pub(super) completed: Vec<PathBuf>,
    pub(super) failures: Vec<String>,
    /// Set by `on_cancel_export`; checked at the top of `start_next_export_job` so a
    /// cancel stops the WHOLE queue, not just the in-flight job -- see that
    /// function's own doc comment.
    pub(super) cancelled: bool,
    pub(super) current_handle: Option<export_thread::ExportHandle>,

    pub(super) export_dir: PathBuf,
    pub(super) template: String,
    // Fields shared by every job's `TemplateContext` -- see `filename_template`'s own
    // doc comment for what each variable means.
    pub(super) design: String,
    pub(super) designer: String,
    pub(super) shape: String,
    pub(super) ri: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) spp: u32,
    pub(super) bounces: u32,
    pub(super) colorspace: String,

    pub(super) params: export_thread::ExportParams,
    /// The "Adaptive Sampling" choice (the cap is `params.samples_per_pixel`); `None` is off.
    pub(super) adaptive: Option<AdaptiveSettings>,
    /// The "Path Cache" choice (`persist` = "Keep for Re-Renders"); `None` is off.
    pub(super) path_cache: Option<PathCacheSettings>,
    /// The "Beam Tracing (Faceted Stones)" choice; `None` is off.
    pub(super) beam: Option<BeamSettings>,
    pub(super) color_space: ColorSpace,
    /// Compute target, remote endpoint and transfer, fixed for the whole queue.
    pub(super) remote: export_thread::RemoteSelection,
    pub(super) local_compute: LocalComputeTarget,

    /// The running job's time-remaining estimator. Reset when a job starts and fed on every
    /// progress tick (`handle_export_job_progress`). It is per job, not per fan-out: jobs can
    /// render at very different rates (lighting rig, HDR map, local or remote), so one fit
    /// across them would be wrong.
    pub(super) eta: EtaEstimator,
}

/// The display label of a `ColorSpace`, as `export_dialog.slint` shows it. Used only for the
/// `{colorspace}` template variable, so it stays out of the `ColorSpace` type.
pub(in crate::gui) const fn colorspace_label(cs: ColorSpace) -> &'static str {
    match cs {
        ColorSpace::Srgb => "sRGB",
        ColorSpace::DisplayP3 => "Display P3",
        ColorSpace::Rec2020 => "Rec.2020",
        // Not offered by the export dialog (a scene-linear space does not suit an 8-bit
        // picture, see `bridge::export_thread`). Listed so the match stays exhaustive.
        ColorSpace::AcesCg => "ACEScg",
    }
}

/// Builds this job's own `TemplateContext` from the queue's fixed, whole-export fields
/// plus this job's own resolved scene (material name, lighting rig actually in effect,
/// camera pose, exposure) and preset label.
fn template_context_for_job(queue: &ExportQueue, job: &ExportJob) -> TemplateContext {
    template_context(
        DesignNames {
            design: &queue.design,
            designer: &queue.designer,
            shape: &queue.shape,
            ri: &queue.ri,
        },
        ExportSettingsView {
            width: queue.width,
            height: queue.height,
            spp: queue.spp,
            bounces: queue.bounces,
            colorspace: &queue.colorspace,
        },
        job,
    )
}

/// The design fields of a filename template, shared by every picture of one export.
#[derive(Clone, Copy)]
pub(in crate::gui) struct DesignNames<'a> {
    pub(in crate::gui) design: &'a str,
    pub(in crate::gui) designer: &'a str,
    pub(in crate::gui) shape: &'a str,
    pub(in crate::gui) ri: &'a str,
}

/// The export settings of a filename template, shared by every picture of one export.
#[derive(Clone, Copy)]
pub(in crate::gui) struct ExportSettingsView<'a> {
    pub(in crate::gui) width: u32,
    pub(in crate::gui) height: u32,
    pub(in crate::gui) spp: u32,
    pub(in crate::gui) bounces: u32,
    pub(in crate::gui) colorspace: &'a str,
}

/// The `TemplateContext` of one picture: the whole export's design and settings, plus this
/// picture's own scene (material name, lighting rig actually in effect, camera pose,
/// exposure) and preset label. The immediate export and the render queue both name their
/// files through it, so the two cannot drift apart.
pub(in crate::gui) fn template_context(
    names: DesignNames<'_>,
    settings: ExportSettingsView<'_>,
    job: &ExportJob,
) -> TemplateContext {
    TemplateContext {
        design: names.design.to_string(),
        designer: names.designer.to_string(),
        shape: names.shape.to_string(),
        material: job.scene.material.name.clone(),
        ri: names.ri.to_string(),
        width: settings.width,
        height: settings.height,
        spp: settings.spp,
        bounces: settings.bounces,
        colorspace: settings.colorspace.to_string(),
        preset: job.preset_label.clone(),
        lighting: job.scene.lighting_preset.label().to_string(),
        yaw_deg: job.scene.yaw.to_degrees(),
        pitch_deg: job.scene.pitch.to_degrees(),
        distance: job.scene.distance,
        exposure: job.scene.exposure,
    }
}

/// Finalizes the queue once every job has run or the queue was cancelled: resumes the live
/// viewport, clears the preview, and reports one combined outcome for every file attempted, so
/// an earlier failure is not hidden by a later success.
pub(super) fn finish_export_queue(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &ExportQueue,
) {
    ui.global::<ExportModel>().set_is_exporting(false);
    // A count, not a flag: batch preview and tilt jobs can run alongside this queue, so it
    // releases only its own claim. See `RenderContext::export_active_count`.
    if queue.remote.compute_target.pauses_live_viewport() {
        RenderContext::lock(render_ctx).export_active_count -= 1;
    }
    ui.global::<ExportModel>()
        .set_preview_image(slint::Image::default());
    // No estimate makes sense once the whole queue has stopped, on every exit path
    // (finished, cancelled, or every job failed).
    ui.global::<ExportModel>()
        .set_eta_text(String::new().into());

    if queue.cancelled {
        ui.global::<ExportModel>().set_has_error(false);
        ui.global::<ExportModel>()
            .set_status_message("Export cancelled.".into());
        // Run in the background, nothing on screen would say so (the finished and failed
        // outcomes below always toast).
        if !ui.global::<ExportModel>().get_is_open() {
            show_toast(ui, "Export cancelled.", "info");
        }
        return;
    }

    if queue.failures.is_empty() {
        let msg = if queue.completed.len() == 1 {
            format!("Exported to {}", queue.completed[0].display())
        } else {
            format!(
                "Exported {} files to {}",
                queue.completed.len(),
                queue.export_dir.display()
            )
        };
        ui.global::<ExportModel>().set_has_error(false);
        ui.global::<ExportModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "success");
    } else {
        let msg = format!(
            "Exported {} of {} files -- {} failed: {}",
            queue.completed.len(),
            queue.total,
            queue.failures.len(),
            queue.failures.join("; ")
        );
        ui.global::<ExportModel>().set_has_error(true);
        ui.global::<ExportModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "error");
    }
}

/// Starts the next job of `queue`, or finalizes the queue when none remain or it was cancelled.
/// Called once to start the export and then from each job's `on_done`. It recurses through a
/// named function, not a self-capturing closure, so `spawn_export`'s `Fn` bound on `on_done`
/// holds: each call builds fresh closures over `Arc` clones.
pub(super) fn start_next_export_job(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &Arc<Mutex<ExportQueue>>,
) {
    let job = {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.cancelled {
            None
        } else {
            q.jobs.pop_front()
        }
    };

    let Some(job) = job else {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        finish_export_queue(ui, render_ctx, &q);
        return;
    };

    let (current_index, total, output_path) = {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = template_context_for_job(&q, &job);
        let output_path = export_thread::resolve_export_path(&q.export_dir, &q.template, &ctx);
        // A fresh job starts with a fresh rate estimate (see `ExportQueue::eta`).
        q.eta.reset();
        (q.current_index, q.total, output_path)
    };
    set_starting_job_ui_state(ui, current_index, total, &job);

    let (params, adaptive, path_cache, beam, color_space, remote, local_compute) = {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            q.params,
            q.adaptive,
            q.path_cache,
            q.beam,
            q.color_space,
            q.remote.clone(),
            q.local_compute,
        )
    };
    ui.global::<ExportModel>()
        .set_preview_samples_total(params.samples_per_pixel.cast_signed());

    let queue_progress = queue.clone();
    let render_ctx_done = render_ctx.clone();
    let queue_done = queue.clone();

    let handle = export_thread::spawn_export(
        ui.as_weak(),
        job.scene,
        params,
        adaptive,
        path_cache,
        beam,
        color_space,
        output_path,
        remote,
        local_compute,
        move |ui: &MainWindow, progress: export_thread::ExportProgress| {
            handle_export_job_progress(ui, &queue_progress, progress);
        },
        move |ui: &MainWindow, outcome: export_thread::ExportOutcome| {
            handle_export_job_done(ui, &render_ctx_done, &queue_done, outcome);
        },
    );

    {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.current_handle = Some(handle);
    }
}

/// Resets the export panel's progress and preview for the job about to start. Kept apart to
/// stay under clippy's function-length lint.
fn set_starting_job_ui_state(ui: &MainWindow, current_index: usize, total: usize, job: &ExportJob) {
    ui.global::<ExportModel>()
        .set_preset_index(i32::try_from(current_index).unwrap_or(i32::MAX));
    ui.global::<ExportModel>()
        .set_preset_total(i32::try_from(total).unwrap_or(i32::MAX));
    ui.global::<ExportModel>()
        .set_current_preset_label(if job.preset_label.is_empty() {
            "Current View".into()
        } else {
            job.preset_label.clone().into()
        });
    ui.global::<ExportModel>()
        .set_progress(current_index as f32 / total as f32);
    ui.global::<ExportModel>().set_preview_samples_done(0);
    ui.global::<ExportModel>()
        .set_preview_image(slint::Image::default());
    // The previous job's estimate must not linger; `handle_export_job_progress` sets a new one.
    ui.global::<ExportModel>()
        .set_eta_text(String::new().into());
    // Likewise the previous job's note (the path cache's size line or the reason it is off).
    ui.global::<ExportModel>()
        .set_progress_note(String::new().into());
}

/// The `on_progress` callback of `spawn_export` (see `start_next_export_job`). Kept apart for
/// clippy's function-length lint.
fn handle_export_job_progress(
    ui: &MainWindow,
    queue: &Arc<Mutex<ExportQueue>>,
    progress: export_thread::ExportProgress,
) {
    let (current_index, total, eta_text) = {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        #[cfg(feature = "beam")]
        q.eta
            .observe_stretch(now, f64::from(progress.fraction), progress.beam.is_some());
        #[cfg(not(feature = "beam"))]
        q.eta.observe(now, f64::from(progress.fraction));
        let eta_text = format_eta(q.eta.eta(now));
        (q.current_index, q.total, eta_text)
    };
    let combined = (current_index as f32 + progress.fraction) / total as f32;
    ui.global::<ExportModel>().set_progress(combined);
    ui.global::<ExportModel>().set_eta_text(eta_text.into());
    ui.global::<ExportModel>()
        .set_preview_samples_done(progress.samples_done.cast_signed());
    ui.global::<ExportModel>()
        .set_preview_samples_total(progress.samples_total.cast_signed());
    if let Some(buf) = progress.preview {
        ui.global::<ExportModel>()
            .set_preview_image(crate::bridge::pixel_buffer::rgba_image(buf));
    }
    if let Some(note) = progress.note {
        // A note comes once: the toast shows it for a moment, the dialog keeps it under the
        // progress bar for the rest of the job (the path cache's size line, or why it is off).
        ui.global::<ExportModel>()
            .set_progress_note(note.clone().into());
        show_toast(ui, &note, "info");
    }
}

/// The `on_done` callback of `spawn_export`: records the finished job's outcome, then starts
/// the next one (see `start_next_export_job`).
fn handle_export_job_done(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &Arc<Mutex<ExportQueue>>,
    outcome: export_thread::ExportOutcome,
) {
    {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.current_handle = None;
        match outcome {
            export_thread::ExportOutcome::Completed(path) => q.completed.push(path),
            export_thread::ExportOutcome::Cancelled => q.cancelled = true,
            export_thread::ExportOutcome::Failed(err) => q.failures.push(err),
        }
        q.current_index += 1;
    }
    start_next_export_job(ui, render_ctx, queue);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The export's bounce cap comes from the export dialog, not from the live viewport: a
    /// snapshot captured at one bounce count is overridden by `apply_export_bounce_cap`.
    #[test]
    fn apply_export_bounce_cap_overrides_the_snapshot_not_the_viewport_default() {
        let viewport_ctx = Mutex::new(RenderContext {
            max_bounces: 12, // the viewport's own setting, left untouched by this test
            ..Default::default()
        });
        let captured = SceneSnapshot::capture(&viewport_ctx).expect("Diamond resolves");
        assert_eq!(
            captured.max_bounces, 12,
            "capture must still read the viewport's own setting by itself"
        );

        let exported = apply_export_bounce_cap(captured, 64);
        assert_eq!(
            exported.max_bounces, 64,
            "the export dialog's own bounce cap must win over the captured viewport value"
        );
    }

    /// Pins the pill mapping. `gui::color_space_from_index` follows the same convention and has
    /// its own test in `gui::startup_settings`.
    #[test]
    fn compute_target_from_index_maps_all_three_pill_values() {
        assert_eq!(compute_target_from_index(0), ComputeTarget::LocalOnly);
        assert_eq!(compute_target_from_index(1), ComputeTarget::RemoteOnly);
        assert_eq!(compute_target_from_index(2), ComputeTarget::Both);
    }

    #[test]
    fn compute_target_from_index_falls_back_to_both_for_unknown_values() {
        assert_eq!(compute_target_from_index(-1), ComputeTarget::Both);
        assert_eq!(compute_target_from_index(99), ComputeTarget::Both);
    }

    #[test]
    fn colorspace_label_covers_every_variant() {
        assert_eq!(colorspace_label(ColorSpace::Srgb), "sRGB");
        assert_eq!(colorspace_label(ColorSpace::DisplayP3), "Display P3");
        assert_eq!(colorspace_label(ColorSpace::Rec2020), "Rec.2020");
        assert_eq!(colorspace_label(ColorSpace::AcesCg), "ACEScg");
    }

    /// A preset without a camera pose leaves the base capture's camera untouched, as the live
    /// apply does (see `settings::model::LightingPreset`).
    #[test]
    fn apply_preset_to_scene_leaves_camera_untouched_when_the_preset_carries_none() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            yaw: 1.23,
            pitch: 0.44,
            camera_pan: [0.2, 0.1],
            ..Default::default()
        }))
        .expect("Diamond resolves");
        let preset = SavedLightingPreset {
            name: "Mood Only".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: None,
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: false,
            tray_reflection: 0.0,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let overlaid = apply_preset_to_scene(base.clone(), &preset, 1.5);
        assert_eq!(overlaid.yaw, base.yaw);
        assert_eq!(overlaid.pitch, base.pitch);
        assert_eq!(overlaid.camera_pan, base.camera_pan);
        assert_eq!(
            overlaid.distance.to_bits(),
            base.distance.to_bits(),
            "a lighting-only preset keeps the zoom of the picture it is applied to"
        );
        assert_eq!(overlaid.light_yaw, preset.light_yaw_deg.to_radians());
    }

    /// A preset with a camera pose overlays it on the export path too, its pan included; an
    /// older pose without a pan centres the stone.
    #[test]
    fn apply_preset_to_scene_overlays_camera_when_the_preset_carries_one() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            yaw: 1.23,
            pitch: 0.44,
            camera_pan: [0.2, 0.1],
            ..Default::default()
        }))
        .expect("Diamond resolves");
        let mut preset = SavedLightingPreset {
            name: "Full Shot".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: Some(0.75),
            camera_pitch: Some(0.3),
            camera_roll: Some(std::f32::consts::FRAC_PI_2),
            camera_pan: Some([0.4, -0.3]),
            env_map_path: None,
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: false,
            tray_reflection: 0.0,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let overlaid = apply_preset_to_scene(base.clone(), &preset, 1.5);
        assert_eq!(overlaid.yaw, 0.75);
        assert_eq!(overlaid.pitch, 0.3);
        assert_eq!(overlaid.camera_roll, std::f32::consts::FRAC_PI_2);
        assert_eq!(overlaid.camera_pan, [0.4, -0.3]);
        preset.camera_pan = None;
        let older = apply_preset_to_scene(base, &preset, 1.5);
        assert_eq!(older.camera_pan, [0.0, 0.0], "an older view is centred");
    }

    /// A saved lighting is a complete look: its glare and head shadow reach the exported scene,
    /// and one without an HDR map clears the map the live view had loaded.
    #[test]
    fn apply_preset_to_scene_carries_the_whole_look() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            surface_glare: 1.0,
            head_shadow_deg: 16.0,
            stone_shadow: false,
            env_map: Some(Arc::new(
                pleochroma::renderer::env_map::EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]),
            )),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert!(base.env_map.is_some());
        let preset = SavedLightingPreset {
            name: "Look".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: None,
            export_usable: true,
            surface_glare: 0.25,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: 5.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let overlaid = apply_preset_to_scene(base, &preset, 1.5);
        assert_eq!(overlaid.surface_glare.to_bits(), 0.25_f32.to_bits());
        assert_eq!(overlaid.head_shadow_deg.to_bits(), 5.0_f32.to_bits());
        assert!(overlaid.env_map.is_none(), "no saved map means the rig");
    }

    /// A saved lighting carries its map rotation into the fanned-out scene: a preset with a map
    /// sets the preset's yaw and follow switch, and one without a map clears the base capture's
    /// turn along with its map.
    #[test]
    fn apply_preset_to_scene_sets_the_saved_maps_orientation() {
        let pixels: Vec<image::Rgb<f32>> = (0..64)
            .map(|i| image::Rgb([(i as f32).mul_add(0.01, 0.2), 0.5, 0.25]))
            .collect();
        let mut bytes = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&pixels, 16, 4)
            .expect("the map encodes");
        let path =
            std::env::temp_dir().join(format!("pleochroma_queue_turn_{}.hdr", std::process::id()));
        std::fs::write(&path, &bytes).expect("the map is written");
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let mut preset = SavedLightingPreset {
            name: "Turned".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: Some(path.to_string_lossy().into_owned()),
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: false,
            tray_reflection: 0.0,
            env_map_yaw_deg: 90.0,
            env_map_follows_camera: true,
        };
        let turned = apply_preset_to_scene(base, &preset, 1.5);
        assert!(turned.env_map.is_some(), "the saved map loads");
        assert_eq!(
            turned.hdr_orientation,
            HdrOrientation {
                yaw: 90.0_f32.to_radians(),
                follows_camera: true
            },
            "the saved lighting's turn reaches the exported scene"
        );
        preset.env_map_path = None;
        let studio = apply_preset_to_scene(turned, &preset, 1.5);
        assert!(studio.env_map.is_none());
        assert_eq!(studio.hdr_orientation, HdrOrientation::default());
        let _ = std::fs::remove_file(&path);
    }

    /// A saved lighting with the Stone Shadow on reaches the fanned-out scene, and one with it
    /// off clears what the base capture had.
    #[test]
    fn apply_preset_to_scene_carries_the_stone_shadow() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_shadow: false,
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert!(!base.stone_shadow);
        let mut preset = SavedLightingPreset {
            name: "Shadow".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: None,
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: true,
            tray_reflection: 0.0,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let on = apply_preset_to_scene(base, &preset, 1.5);
        assert!(
            on.stone_shadow,
            "the saved lighting's shadow reaches the scene"
        );
        preset.stone_shadow = false;
        let off = apply_preset_to_scene(on, &preset, 1.5);
        assert!(!off.stone_shadow, "a saved lighting without it clears it");
    }

    /// A saved lighting with a Tray Reflection reaches the fanned-out scene, and one without it
    /// clears what the base capture had.
    #[test]
    fn apply_preset_to_scene_carries_the_tray_reflection() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            tray_reflection: 0.5,
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(base.tray_reflection.to_bits(), 0.5f32.to_bits());
        let mut preset = SavedLightingPreset {
            name: "Glossy".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: None,
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: false,
            tray_reflection: 0.04,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let on = apply_preset_to_scene(base, &preset, 1.5);
        assert_eq!(on.tray_reflection.to_bits(), 0.04f32.to_bits());
        preset.tray_reflection = 0.0;
        let off = apply_preset_to_scene(on, &preset, 1.5);
        assert_eq!(off.tray_reflection.to_bits(), 0);
    }

    /// A base capture of a stone that is not closed asked for nothing, so it never looked; a
    /// preset that turns the shadow on must still leave the remote scene without it.
    #[test]
    fn apply_preset_to_scene_keeps_a_stone_that_is_not_closed_out_of_the_remote_scene() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_shadow: false,
            active_planes: Arc::new(Vec::new()),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert!(
            base.stone_closed,
            "nothing was asked for, so nothing looked"
        );
        let preset = SavedLightingPreset {
            name: "Shadow".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            env_map_path: None,
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
            stone_shadow: true,
            tray_reflection: 0.04,
            env_map_yaw_deg: 0.0,
            env_map_follows_camera: false,
        };
        let overlaid = apply_preset_to_scene(base, &preset, 1.5);
        assert!(!overlaid.stone_closed);
        assert!(!overlaid.remote_stone_shadow());
        assert_eq!(overlaid.remote_tray_reflection().to_bits(), 0);
    }

    /// A job scene of a stone that is not closed resolves to a snapshot that is not closed, and
    /// the scene built back from it carries no shadow.
    #[test]
    fn a_job_scene_of_a_stone_that_is_not_closed_carries_no_shadow() {
        use crate::gui::render_jobs::convert::{scene_from_snapshot, snapshot_from_scene};
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let mut scene = scene_from_snapshot(&base, 16, 16);
        scene.planes.clear();
        scene.stone_shadow = true;
        let snapshot = snapshot_from_scene(&scene, None);
        assert!(!snapshot.stone_closed);
        assert!(!scene_from_snapshot(&snapshot, 16, 16).stone_shadow);
    }
}
