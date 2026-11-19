//! Update Job from This View: the job takes the look the live view shows now.
//!
//! The button of the banner and the palette command call [`update_job_from_view`]. It works on the
//! job whose view is shown (`PlanesOwner::Job` with that id), captures the live view for the
//! current engine with the job's own size and bounce cap, and writes the job back through the
//! guarded row update of the vault. The token, the design record and the output settings are kept.
//! A tilt video is captured the way a queued video is, except that nothing here opens a folder
//! picker or reads the export dialogs: the sweep, axes, fps and overlay stay, and the curves are
//! measured again off the UI thread when an overlay is on.
//!
//! Every pure step ([`availability`], [`owner_check`], [`prepare_snapshot`], [`rebuild_job`],
//! [`write_update`]) is separate, so the tests need no window.

use super::session::active_job;
use crate::{
    GuideModel, MainWindow, RenderJobsModel,
    bridge::{
        export_thread::SceneSnapshot,
        render_thread::{PlanesOwner, RenderContext, remote_allowed},
    },
    gui::{
        editor::native_io::ask_write_confirm,
        render_jobs::{
            capture_video::{VideoFacts, video_summary},
            controller::{is_running, job_changed},
            convert,
            edit::{
                load::load_job_for_edit,
                naming::automatic_label,
                save::{EditDeps, deps as edit_deps},
                video_curves::spawn_measure,
            },
        },
        tilt::video_export::frames::{TiltTrayEffects, apply_tray_effects},
    },
    settings::SettingsPersister,
};
use pleochroma::{
    render_setup::StonePose,
    renderer::{accel::RenderAccel, engine::EngineSpec},
};
use pleochroma_net::messages::{content_hash, hash_hex};
use pleochroma_render_jobs::{
    ComputeChoice, JobKind, RenderJobFile, codec,
    edit::{EditMode, edit_rule, label_after_edit},
    job::{BACKDROP_MODEL_TRAY, MAX_SPP, MetricCurvesData, format_for},
    state::JobState,
};
use pleochroma_vault::{
    db::sqlite::{Database, ExpectedRow, UpdatedJob},
    model::render_job::RenderJobMeta,
};
use slint::{ComponentHandle as _, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

/// The reason line while the job renders.
pub(super) const RUNNING_REASON: &str = "This job is rendering. Pause it first.";

/// The reason line while the tilt curves of the job's stone are measured.
const MEASURING_REASON: &str = "Measuring the tilt curves of this job's stone\u{2026}";

/// The refusal when the view on screen is not the view of the job.
const NOT_THIS_VIEW: &str =
    "The view on screen is not the view of this job. Load the job view again first.";

/// The refusal when no job view shows.
const NO_VIEW: &str = "No job view is showing.";

/// The refusal before the job list exists.
const NO_LIST: &str = "The job list is not available.";

/// The refusal when the row changed while the update was prepared.
const ROW_CHANGED: &str = "This job changed in the meantime; nothing was changed.";

/// How often the availability of the update is read again (a job can start or stop at any time).
const REFRESH_EVERY: Duration = Duration::from_millis(400);

thread_local! {
    /// The job whose tilt curves are being measured for an update (UI thread only).
    static MEASURING: Cell<Option<i64>> = const { Cell::new(None) };
    /// The timer that keeps `job_view_can_update` current (UI thread only).
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Forgets what a previous main window left behind. See `gui::window_release`.
pub fn release_window_state() {
    let timer = REFRESH_TIMER.with(|cell| cell.borrow_mut().take());
    drop(timer);
    MEASURING.with(|cell| cell.set(None));
}

/// Names this module's holder in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub fn note_holders(out: &mut Vec<&'static str>) {
    if REFRESH_TIMER.with(|cell| cell.borrow().is_some()) {
        out.push("render_jobs::view::update REFRESH_TIMER");
    }
}

/// Registers `RenderJobsModel.update_job_from_view` and starts the timer that keeps the
/// availability properties current.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    _db: &Arc<Mutex<Database>>,
    _render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    let ui_weak = ui.as_weak();
    ui.global::<RenderJobsModel>()
        .on_update_job_from_view(move || {
            if let Some(ui) = ui_weak.upgrade() {
                update_job_from_view(&ui);
            }
        });
    let timer = Timer::default();
    let ui_weak = ui.as_weak();
    timer.start(TimerMode::Repeated, REFRESH_EVERY, move || {
        if let Some(ui) = ui_weak.upgrade() {
            refresh_can_update(&ui);
        }
    });
    REFRESH_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// Whether the update is on, and the reason line while it is off.
///
/// `shown` is the job whose view shows, `running` whether that job renders and `measuring`
/// whether its tilt curves are being measured for an update.
#[must_use]
pub(super) const fn availability(
    shown: Option<i64>,
    running: bool,
    measuring: bool,
) -> (bool, &'static str) {
    if shown.is_none() {
        (false, "")
    } else if running {
        (false, RUNNING_REASON)
    } else if measuring {
        (false, MEASURING_REASON)
    } else {
        (true, "")
    }
}

/// Fills `job_view_can_update` and `job_view_update_reason` from the session and the queue.
///
/// Called by the timer of [`setup`]; a lane that changes the session may call it as well.
pub(in crate::gui) fn refresh_can_update(ui: &MainWindow) {
    let shown = active_job();
    let running = shown.is_some_and(is_running);
    let measuring = MEASURING.with(Cell::get).is_some();
    let (can, reason) = availability(shown, running, measuring);
    let model = ui.global::<RenderJobsModel>();
    if model.get_job_view_can_update() != can {
        model.set_job_view_can_update(can);
    }
    if model.get_job_view_update_reason().as_str() != reason {
        model.set_job_view_update_reason(reason.into());
    }
}

/// Refuses unless the live stone is the stone of job `id`.
///
/// # Errors
///
/// The sentence to show.
pub(super) const fn owner_check(owner: PlanesOwner, id: i64) -> Result<(), &'static str> {
    match owner {
        PlanesOwner::Job { job_id } if job_id == id => Ok(()),
        _ => Err(NOT_THIS_VIEW),
    }
}

/// Shows a refusal.
fn refuse(ui: &MainWindow, sentence: &str) {
    crate::gui::show_toast(ui, sentence, "warning");
}

/// Why the first half of an update stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    /// A refusal, with its sentence.
    Refuse(String),
    /// The job's HDR map is missing and the view shows another one: ask first.
    AskHdr,
}

/// Puts the live view into the job whose view shows. UI thread only.
pub(in crate::gui) fn update_job_from_view(ui: &MainWindow) {
    run_update(ui, false);
}

/// The whole update; `hdr_confirmed` is set when the HDR question was answered yes.
fn run_update(ui: &MainWindow, hdr_confirmed: bool) {
    // The same lock the command's palette entry follows.
    if !ui.global::<GuideModel>().invoke_allows_view_tabs() {
        return;
    }
    if MEASURING.with(Cell::get).is_some() {
        return;
    }
    let Some(deps) = edit_deps() else {
        refuse(ui, NO_LIST);
        return;
    };
    let Some(id) = active_job() else {
        refuse(ui, NO_VIEW);
        return;
    };
    let owner = RenderContext::lock(&deps.render_ctx).planes_owner;
    if let Err(sentence) = owner_check(owner, id) {
        refuse(ui, sentence);
        return;
    }
    if is_running(id) {
        refuse(ui, RUNNING_REASON);
        refresh_can_update(ui);
        return;
    }
    // The row is read here and written below in the same call (or, for a video, written when
    // the curves are back, against the state read now).
    let (meta, job) = match load_job_for_edit(ui, id) {
        Ok(loaded) => loaded,
        Err(sentence) => {
            refuse(ui, &sentence);
            return;
        }
    };
    let new = match plan_update(&deps, &job, hdr_confirmed) {
        Ok(new) => new,
        Err(Stop::Refuse(sentence)) => {
            refuse(ui, &sentence);
            return;
        }
        Err(Stop::AskHdr) => {
            ask_hdr(ui);
            return;
        }
    };
    let seeded = ExpectedRow {
        state: meta.state.clone(),
        frames_done: i64::from(meta.frames_done),
    };
    if wants_curves(&new) {
        start_measure(ui, id, seeded, job, new);
        return;
    }
    let outcome = write_update(&deps.db(), &meta, &seeded, &job, &new, is_running(id));
    report(ui, id, &outcome, &job, &new);
}

/// Asks whether the job may take the HDR map the view shows, then updates.
fn ask_hdr(ui: &MainWindow) {
    ask_write_confirm(
        ui,
        "HDR Map of This Job Is Missing",
        "The HDR map of this job is missing, and this view shows another one. Update Job uses \
         the map this view shows, and the job renders with it from now on."
            .to_string(),
        "Update Job",
        None,
        |ui| run_update(ui, true),
    );
}

/// The job as it is after the update, or why it cannot be.
fn plan_update(
    deps: &EditDeps,
    job: &RenderJobFile,
    hdr_confirmed: bool,
) -> Result<RenderJobFile, Stop> {
    // The renderer the view shows now: the job takes it, as far as the job rules allow.
    let live = RenderContext::lock(&deps.render_ctx).engine;
    let engine = EngineSpec {
        engine: live.engine,
        ray: live.ray.sanitized(),
    };
    // The stone is the context's own (the job's stone, while its view shows).
    let scene = SceneSnapshot::capture_finished_for_engine(&deps.render_ctx, None, engine)
        .map_err(Stop::Refuse)?;
    let base = base_dir(job);
    if !hdr_confirmed && scene.env_map.is_some() && hdr_is_missing(job, &base) {
        return Err(Stop::AskHdr);
    }
    let scene = prepare_snapshot(job, scene, engine).map_err(Stop::Refuse)?;
    let output = deps.settings_store.snapshot().settings.output.sanitised();
    let exponent = if matches!(job.kind, JobKind::TiltVideo(_)) {
        output.tilt_video.sample_exponent
    } else {
        output.still.sample_exponent
    };
    rebuild_job(job, &scene, engine, path_default_samples(exponent)).map_err(Stop::Refuse)
}

/// The folder a relative path of the job resolves against (the app's rule).
fn base_dir(job: &RenderJobFile) -> PathBuf {
    Path::new(&job.output)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Whether the job names an HDR map that is gone or no longer the file the job was made with.
#[must_use]
pub(super) fn hdr_is_missing(job: &RenderJobFile, base: &Path) -> bool {
    let (Some(source), Some(path)) = (&job.hdr, job.hdr_path(base)) else {
        return false;
    };
    !std::fs::read(&path).is_ok_and(|bytes| hash_hex(&content_hash(&bytes)) == source.sha256_hex)
}

/// The samples a Path-Traced render starts with at slider exponent `exponent` (2 to the power).
#[must_use]
pub(super) fn path_default_samples(exponent: u32) -> u32 {
    1_u32.checked_shl(exponent).unwrap_or(MAX_SPP).min(MAX_SPP)
}

/// The capture made ready for the job: its bounce cap and engine, and for a video the way a
/// queued video prepares its scene.
///
/// The order is that of the tilt video section's request: the Stone Pose cleared, the tray
/// effects of the live capture (a video draws the effects the view shows), then the optical
/// diagnostics refusal. The job's own bounce cap and diagnostics are kept.
///
/// # Errors
///
/// The sentence of the diagnostics refusal.
pub(super) fn prepare_snapshot(
    job: &RenderJobFile,
    mut scene: SceneSnapshot,
    engine: EngineSpec,
) -> Result<SceneSnapshot, String> {
    scene.max_bounces = job.scene.max_bounces;
    scene.engine = engine;
    if matches!(job.kind, JobKind::TiltVideo(_)) {
        scene.clear_stone_pose();
        let effects = TiltTrayEffects {
            stone_shadow: scene.stone_shadow,
            tray_reflection: scene.tray_reflection > 0.0,
        };
        apply_tray_effects(&mut scene, effects);
    }
    scene.optical_diagnostics = if engine.is_ray() {
        None
    } else {
        job.optical_diagnostics
            .map(convert::diagnostic_presentation)
    };
    if let Some(reason) = crate::bridge::optical_overlay::export::refusal(&scene) {
        return Err(reason.to_string());
    }
    Ok(scene)
}

/// The samples per pixel of a job of the renderer `was` that now renders with `now`.
///
/// Ray-Traced samples are the tap count. From Ray-Traced back to Path-Traced the count is the
/// Path-Traced default the dialogs would start with now (`path_default`); Path-Traced to
/// Path-Traced keeps the job's number.
#[must_use]
pub(super) fn samples_after(was: EngineSpec, now: EngineSpec, old: u32, path_default: u32) -> u32 {
    if now.is_ray() {
        u32::from(now.ray.sanitized().aa_taps)
    } else if was.is_ray() {
        path_default
    } else {
        old
    }
}

/// The samples per pixel of a job.
const fn samples_of(job: &RenderJobFile) -> u32 {
    match &job.kind {
        JobKind::Still(still) => still.samples_per_pixel,
        JobKind::TiltVideo(video) => video.samples_per_pixel,
    }
}

/// Clears the accelerations a Ray-Traced job cannot use.
fn strip_accel(accel: &mut RenderAccel, engine: EngineSpec) {
    if engine.is_ray() {
        *accel = RenderAccel::default();
    }
}

/// The job with the capture `scene` in place of its scene.
///
/// Kept: the token, the output settings, the design record (its material name follows), the
/// size, the bounce cap, the transfer, and for a video the sweep, axes, fps, overlay, curves
/// and frame folder. Changed: the scene, the engine in both copies, the HDR map, the zones, the
/// samples when the renderer changes, and the label while it is still the automatic one.
///
/// # Errors
///
/// The sentence of the job rule that refuses the result (`RenderJobFile::validate`), or of an
/// HDR map that was not loaded from a file.
pub(super) fn rebuild_job(
    job: &RenderJobFile,
    scene: &SceneSnapshot,
    engine: EngineSpec,
    path_default: u32,
) -> Result<RenderJobFile, String> {
    let hdr = convert::hdr_source(scene)?;
    let mut new = job.clone();
    new.scene = convert::scene_from_snapshot(scene, job.scene.width, job.scene.height);
    // A remote worker reads `scene.engine`, the executor `engine`: both are always set together.
    new.engine = engine;
    new.scene.engine = engine;
    new.scene_backdrop_model = BACKDROP_MODEL_TRAY;
    new.hdr = hdr;
    new.zoning = None;
    new.optical_diagnostics = if engine.is_ray() {
        None
    } else {
        job.optical_diagnostics
    };
    if !remote_allowed(engine) {
        new.compute.target = ComputeChoice::Local;
    }
    let samples = samples_after(job.engine, engine, samples_of(job), path_default);
    match &mut new.kind {
        JobKind::Still(still) => {
            still.samples_per_pixel = samples;
            // The picture is the current view now, not the saved lighting the job was added with.
            still.preset_label.clear();
            strip_accel(&mut still.accel, engine);
        }
        JobKind::TiltVideo(video) => {
            video.samples_per_pixel = samples;
            strip_accel(&mut video.accel, engine);
            if engine.is_ray() {
                video.keep_path_cache = false;
            }
        }
    }
    // The zones leave the scene's material for the job's own field (a Ray-Traced job keeps none).
    new.move_scene_zoning_to_file();
    new.format = format_for(&new);
    new.design.material.clone_from(&new.scene.material.name);
    new.label = label_after_edit(&job.label, &automatic_label(job), &automatic_label(&new));
    if new.label.trim().is_empty() {
        new.label = automatic_label(&new);
    }
    new.validate().map_err(|error| error.to_string())?;
    Ok(new)
}

/// Whether the job is a video that draws an overlay, so its curves must be measured again.
fn wants_curves(job: &RenderJobFile) -> bool {
    matches!(&job.kind, JobKind::TiltVideo(video) if video.overlay.count() > 0)
}

/// Puts measured curves into a video: the first axis into `curves`, the rest into `more_curves`.
fn put_curves(job: &mut RenderJobFile, curves: Vec<MetricCurvesData>) {
    if let JobKind::TiltVideo(video) = &mut job.kind {
        let mut all = curves.into_iter();
        video.curves = all.next();
        video.more_curves = all.collect();
    }
}

/// Measures the curves of the new stone off the UI thread, then writes the job.
fn start_measure(
    ui: &MainWindow,
    id: i64,
    seeded: ExpectedRow,
    old: RenderJobFile,
    new: RenderJobFile,
) {
    let JobKind::TiltVideo(video) = &new.kind else {
        return;
    };
    let axes = video.axes();
    MEASURING.with(|cell| cell.set(Some(id)));
    refresh_can_update(ui);
    let measured = new.clone();
    spawn_measure(ui, measured, axes, move |ui, result| {
        finish_video(ui, id, &seeded, &old, new, result);
    });
}

/// The second half of a video update: the curves are back, so the row is written.
fn finish_video(
    ui: &MainWindow,
    id: i64,
    seeded: &ExpectedRow,
    old: &RenderJobFile,
    mut new: RenderJobFile,
    result: Result<Vec<MetricCurvesData>, String>,
) {
    MEASURING.with(|cell| cell.set(None));
    refresh_can_update(ui);
    let curves = match result {
        Ok(curves) => curves,
        Err(sentence) => {
            refuse(
                ui,
                &format!("The tilt curves of this job's stone could not be measured: {sentence}"),
            );
            return;
        }
    };
    put_curves(&mut new, curves);
    if let Err(error) = new.validate() {
        refuse(ui, &error.to_string());
        return;
    }
    let Some(deps) = edit_deps() else {
        refuse(ui, NO_LIST);
        return;
    };
    let meta = match load_job_for_edit(ui, id) {
        Ok((meta, _)) => meta,
        Err(sentence) => {
            refuse(ui, &sentence);
            return;
        }
    };
    let outcome = write_update(&deps.db(), &meta, seeded, old, &new, is_running(id));
    report(ui, id, &outcome, old, &new);
}

/// How an update ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdateOutcome {
    /// The row was rewritten; `requeued` when the job went back in the queue.
    Saved {
        /// Whether the job was put back in the queue.
        requeued: bool,
    },
    /// The view already matches the job; nothing was written.
    Unchanged,
    /// The row no longer has the state and `frames_done` read before.
    RowChanged,
    /// Nothing was written; the sentence says why.
    Refused(String),
}

/// The detail line of a job in the list.
#[must_use]
fn summary_of(job: &RenderJobFile) -> String {
    let material = &job.scene.material.name;
    match &job.kind {
        JobKind::Still(still) => {
            format!(
                "{material} \u{b7} {} \u{d7} {} \u{b7} {} samples \u{b7} {} bounces \u{b7} {} \u{b7} {}",
                job.scene.width,
                job.scene.height,
                still.samples_per_pixel,
                job.scene.max_bounces,
                still.color_space.label(),
                job.scene.lighting_preset.label(),
            ) + &convert::adaptive_detail(still.accel.adaptive.as_ref())
                + &convert::path_cache_detail(still.accel.path_cache.as_ref())
                + &convert::beam_detail(still.accel.beam.as_ref())
                + &convert::stone_pose_detail(StonePose::from_array(job.scene.stone_pose))
        }
        JobKind::TiltVideo(video) => {
            video_summary(&VideoFacts {
                start_deg: video.start_deg,
                end_deg: video.end_deg,
                step_deg: video.step_deg,
                frames: video.total_frames as usize,
                fps: video.fps,
                width: job.scene.width,
                height: job.scene.height,
                samples: video.samples_per_pixel,
                material: material.clone(),
            }) + &convert::adaptive_detail(video.accel.adaptive.as_ref())
                + &convert::path_cache_detail(video.accel.path_cache.as_ref())
                + &convert::beam_detail(video.accel.beam.as_ref())
        }
    }
}

/// Writes `new` over row `meta.job_id`, if the row still has the state and frames read before.
///
/// The picture changed, so the Save rules of the edit dialog for a pixel change apply: a waiting
/// job is saved in place, a paused job keeps its place and starts from frame 0, and a failed,
/// done or cancelled job goes back in the queue from frame 0. The token never changes.
pub(super) fn write_update(
    db: &Database,
    meta: &RenderJobMeta,
    seeded: &ExpectedRow,
    old: &RenderJobFile,
    new: &RenderJobFile,
    running: bool,
) -> UpdateOutcome {
    if meta.state != seeded.state || i64::from(meta.frames_done) != seeded.frames_done {
        return UpdateOutcome::RowChanged;
    }
    if running {
        return UpdateOutcome::Refused(RUNNING_REASON.to_string());
    }
    let Some(state) = JobState::parse(&meta.state) else {
        return UpdateOutcome::Refused("This job has a state this version does not know.".into());
    };
    let mode = match edit_rule(state) {
        Ok(mode) => mode,
        Err(sentence) => return UpdateOutcome::Refused(sentence.to_string()),
    };
    if new == old {
        return UpdateOutcome::Unchanged;
    }
    let requeued = !matches!(mode, EditMode::InPlace);
    let text = match codec::to_text(new) {
        Ok(text) => text,
        Err(error) => return UpdateOutcome::Refused(error.to_string()),
    };
    let updated = UpdatedJob {
        label: new.label.clone(),
        summary: summary_of(new),
        output_path: new.output.clone(),
        frames_total: i64::from(new.frames_total()),
        snapshot_version: i64::from(new.format),
        snapshot: text,
        new_state: requeued.then(|| JobState::Queued.as_str().to_string()),
        reset_progress: true,
    };
    match db.update_render_job(meta.job_id, seeded, &updated) {
        Ok(true) => {
            // The zones now live in the job file itself.
            if let Err(error) = crate::gui::rough_colour::store::delete_job_zoning(db, meta.job_id)
            {
                tracing::warn!(
                    "The color zones of render job {} were not removed: {error:#}",
                    meta.job_id
                );
            }
            UpdateOutcome::Saved { requeued }
        }
        Ok(false) => UpdateOutcome::RowChanged,
        Err(error) => UpdateOutcome::Refused(format!("The job could not be saved: {error}")),
    }
}

/// The sentence a saved update shows; it names the new sample count when the renderer changes
/// from Ray-Traced to Path-Traced.
#[must_use]
pub(super) fn saved_text(
    id: i64,
    old: &RenderJobFile,
    new: &RenderJobFile,
    requeued: bool,
) -> String {
    let mut text = format!("Job {id} now shows this view.");
    match (old.engine.is_ray(), new.engine.is_ray()) {
        (false, true) => text.push_str(" It is now Ray-Traced and renders on this computer."),
        (true, false) => {
            text.push_str(" It is now Path-Traced with ");
            text.push_str(&samples_of(new).to_string());
            text.push_str(" samples per pixel; Edit Render Job changes that.");
        }
        _ => {}
    }
    if requeued {
        text.push_str(" It is back in the queue.");
    }
    text
}

/// Shows how the update ended and refreshes the rows.
fn report(
    ui: &MainWindow,
    id: i64,
    outcome: &UpdateOutcome,
    old: &RenderJobFile,
    new: &RenderJobFile,
) {
    match outcome {
        UpdateOutcome::Saved { requeued } => {
            job_changed(ui, id);
            crate::gui::show_toast(ui, &saved_text(id, old, new, *requeued), "info");
        }
        UpdateOutcome::Unchanged => crate::gui::show_toast(
            ui,
            "This view already matches the job; nothing was changed.",
            "info",
        ),
        UpdateOutcome::RowChanged => refuse(ui, ROW_CHANGED),
        UpdateOutcome::Refused(sentence) => refuse(ui, sentence),
    }
}
