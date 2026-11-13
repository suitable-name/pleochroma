//! The one executor that renders a render job, without a window.
//!
//! The desktop queue and `pleochroma-cli render` / `tilt-video` both call [`execute_job`],
//! so a queued job, a job run from a script and a picture exported directly all go through
//! the same code:
//!
//! - a **still** is `bridge::export_thread::run_export`, the function the immediate export
//!   runs;
//! - a **tilt video** is `video_export::frames::render_frames`, the direct export's frame
//!   loop, run with `skip_existing` so a paused or interrupted video continues at its
//!   first missing frame.
//!
//! A video's frame folder carries a marker file naming the job (`FRAME_MARKER_FILE`), so
//! a resume only trusts frames that this job wrote, and never mixes in another export's.
//!
//! # The estimator
//!
//! A run renders with the process default estimator (`default_estimator()`, set at start-up by
//! the app or per run by the command line). [`job_for_estimator`] lays the job out for it before
//! anything renders:
//!
//! - when the estimator differs from the one the job recorded (`scene.estimator`), the job's token
//!   becomes `changed_token(token, "estimator=<word>")`, so a tilt video never mixes frames made
//!   with two estimators, and the scene records the new one (a remote worker renders with the
//!   estimator its scene carries);
//! - under exit collection the job's own path cache and beams are dropped
//!   ([`accel_for_estimator`]), each with a note, because both replay coin-flip paths.

use super::{convert, headless::changed_token};
use crate::{
    bridge::export_thread::{
        ExportOutcome, ExportParams, ExportProgress, RemoteSelection, RenderTaps, SceneSnapshot,
        run_export_tapped,
    },
    gui::{
        progress_eta::EtaEstimator,
        tilt::video_export::{
            encode::{self, EncodeOutcome},
            frames::{self, FrameReporter, FramesOutcome},
            params,
        },
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use pleochroma::{
    optics::raytracer::{Estimator, default_estimator},
    renderer::{
        accel::{PathCacheSettings, RenderAccel},
        accel_beam::BeamSettings,
        adaptive::AdaptiveSettings,
    },
};
use pleochroma_render_jobs::{
    ComputeChoice, FailureKind, JobKind, JobOutcome, JobProgress, JobSink, RenderJobFile, StillJob,
    TiltVideoJob, TransferChoice,
    paths::{FRAME_MARKER_FILE, marker_text, marker_token, reserve_unique_file},
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};

/// Where and how one job runs on this machine: everything that is machine configuration
/// rather than render content, plus the command line's overrides.
pub struct ExecContext {
    /// The folder relative paths in the job resolve against.
    pub base_dir: PathBuf,
    /// The local engines (processor, graphics card or both).
    pub local_compute: LocalComputeTarget,
    /// The remote worker to use, read from the settings (app) or the flags (command line).
    pub worker: Option<WorkerSettings>,
    /// Replaces the job's compute target.
    pub compute_override: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer_override: Option<TransferChoice>,
    /// Replaces the job's "this computer renders a share too" choice.
    pub contribute_local_override: Option<bool>,
    /// A still: the PNG path. A video: the frame folder. Replaces the job's own.
    pub output_override: Option<PathBuf>,
    /// A video: delete this job's frames first and render from frame one.
    pub restart_frames: bool,
}

/// [`execute_job_tapped`] without taps, with a panic turned into a failed outcome.
///
/// The outcome is always delivered, which the queue relies on to release the live viewport.
pub fn execute_job_catching(
    job: &RenderJobFile,
    ctx: &ExecContext,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    execute_job_catching_tapped(job, ctx, &RenderTaps::default(), cancel, sink)
}

/// [`execute_job_catching`] with the optional [`RenderTaps`] (sample offset, linear-image sink).
pub fn execute_job_catching_tapped(
    job: &RenderJobFile,
    ctx: &ExecContext,
    taps: &RenderTaps,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_job_tapped(job, ctx, taps, cancel, sink)
    }))
    .unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "the render worker panicked".to_string());
        failed(
            FailureKind::Render,
            format!("The render stopped unexpectedly: {message}"),
            0,
        )
    })
}

/// Renders `job` and reports how it ended. Cancelling through `cancel` ends the run as
/// [`JobOutcome::Stopped`] at the next sample batch (still) or the next frame (video).
///
/// The tests' entry point; the app and the CLI go through [`execute_job_catching`] and
/// [`execute_job_catching_tapped`].
#[cfg(test)]
pub fn execute_job(
    job: &RenderJobFile,
    ctx: &ExecContext,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    execute_job_tapped(job, ctx, &RenderTaps::default(), cancel, sink)
}

/// Renders `job` with the optional [`RenderTaps`] and reports how it ended.
///
/// Every rendered picture starts at `taps.sample_offset` and publishes its linear image to
/// `taps.sink`. With [`RenderTaps::default`] the render is the untapped one, byte for byte.
/// Cancelling through `cancel` ends the run as [`JobOutcome::Stopped`] at the next sample batch
/// (still) or the next frame (video).
pub fn execute_job_tapped(
    job: &RenderJobFile,
    ctx: &ExecContext,
    taps: &RenderTaps,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    if let Err(e) = job.validate() {
        return failed(FailureKind::Input, e.to_string(), 0);
    }
    // The estimator rule comes first, so the scene, the token and the accelerations of the whole
    // run (the local and the remote parts alike) are the ones of the estimator it renders with.
    let EstimatorJob { job, change, notes } = job_for_estimator(job, default_estimator());
    let job = &job;
    let env_map = match convert::load_hdr(job, &ctx.base_dir) {
        Ok(env_map) => env_map,
        Err((kind, message)) => return failed(kind, message, 0),
    };
    // The job's color zones ride beside its scene; this puts them back on the material.
    let mut scene = convert::snapshot_from_job(job, env_map);
    scene.engine = job.engine;
    scene.optical_diagnostics = job
        .optical_diagnostics
        .map(convert::diagnostic_presentation);
    // Before anything renders: a job file can carry a combination the live dialogs refuse.
    if let Some(reason) = crate::bridge::optical_overlay::export::refusal(&scene) {
        return failed(FailureKind::Input, reason.to_string(), 0);
    }
    let remote = remote_selection_for(job, ctx);
    let taps = &taps_with_job_adaptive(job, taps);
    announce_notes(job, &notes, sink);
    match &job.kind {
        JobKind::Still(still) => {
            execute_still(job, still, ctx, &scene, &remote, taps, cancel, sink)
        }
        JobKind::TiltVideo(video) => execute_video(
            job,
            video,
            ctx,
            scene,
            remote,
            taps,
            change.as_ref(),
            cancel,
            sink,
        ),
    }
}

/// The note shown when exit collection drops the job's own path cache.
pub(super) const PATH_CACHE_OFF_NOTE: &str =
    "Path cache off: exit collection, the default estimator, does not use it.";

/// The note shown when exit collection drops the job's own beam tracing.
#[cfg(feature = "beam")]
pub(super) const BEAM_OFF_NOTE: &str =
    "Beam tracing off: exit collection, the default estimator, does not use it.";

/// The accelerations a job renders with under `estimator`, and a note for each one dropped.
///
/// Exit collection does not use the path cache or beam tracing, which replay coin-flip paths:
/// under it the path cache is dropped, and so are the beams in a build with the `beam` feature.
/// In a build without that feature `beam` stays as it is, so the render's own note "Beam tracing
/// off: this build has no beam tracer" still says why there are no beams (one beam note, never
/// two). Under [`Estimator::Branching`] nothing changes. Adaptive sampling is never touched.
pub(super) fn accel_for_estimator(
    accel: &RenderAccel,
    estimator: Estimator,
) -> (RenderAccel, Vec<&'static str>) {
    let mut accel = accel.clone();
    let mut notes = Vec::new();
    if estimator == Estimator::ExitCollection {
        if accel.path_cache.take().is_some() {
            notes.push(PATH_CACHE_OFF_NOTE);
        }
        #[cfg(feature = "beam")]
        if accel.beam.take().is_some() {
            notes.push(BEAM_OFF_NOTE);
        }
    }
    (accel, notes)
}

/// A job laid out for one estimator: see [`job_for_estimator`].
pub(super) struct EstimatorJob {
    /// The job to render: its token, scene estimator and accelerations follow the estimator.
    pub(super) job: RenderJobFile,
    /// What the estimator rule did to the token, when the job file recorded another estimator.
    pub(super) change: Option<TokenChange>,
    /// The notes of the accelerations that were dropped ([`accel_for_estimator`]).
    pub(super) notes: Vec<&'static str>,
}

/// The estimator rule changed a job's token: the estimator and the token the job file's own
/// estimator gives its frames.
pub(super) struct TokenChange {
    /// The estimator the job file recorded.
    pub(super) recorded: Estimator,
    /// The job's token before the rule changed it.
    pub(super) old_token: String,
}

/// `job` laid out for a run that renders with `estimator`.
///
/// When `estimator` differs from the one the job recorded in `scene.estimator`, the token becomes
/// `changed_token(token, "estimator=<word>")` (the word is [`Estimator::cli_word`]) and the scene
/// records `estimator`. A job recorded with the same estimator keeps its token. The job's
/// accelerations follow [`accel_for_estimator`]; a tilt video also stops keeping its path cache
/// when the estimator does not use one.
///
/// The Ray-Traced engine has no estimator and caches no light paths: its job comes back as it was,
/// with no token change and no notes.
pub(super) fn job_for_estimator(job: &RenderJobFile, estimator: Estimator) -> EstimatorJob {
    let mut job = job.clone();
    if job.engine.is_ray() {
        return EstimatorJob {
            job,
            change: None,
            notes: Vec::new(),
        };
    }
    let change = (job.scene.estimator != estimator).then(|| TokenChange {
        recorded: job.scene.estimator,
        old_token: job.token.clone(),
    });
    if change.is_some() {
        job.token = changed_token(&job.token, &format!("estimator={}", estimator.cli_word()));
        job.scene.estimator = estimator;
    }
    let notes = match &mut job.kind {
        JobKind::Still(still) => {
            let (accel, notes) = accel_for_estimator(&still.accel, estimator);
            still.accel = accel;
            notes
        }
        JobKind::TiltVideo(video) => {
            let (accel, notes) = accel_for_estimator(&video.accel, estimator);
            video.accel = accel;
            if estimator == Estimator::ExitCollection {
                video.keep_path_cache = false;
            }
            notes
        }
    };
    EstimatorJob { job, change, notes }
}

/// What a refused frame folder is told when the estimator rule changed the job's token: the folder
/// may hold frames of the estimator the job file recorded.
pub(super) fn estimator_change_hint(recorded: Estimator, now: Estimator) -> String {
    let mut hint = format!(
        " This run renders with the {} estimator but the job was made with the {} estimator, \
         which gives the frames a token of their own.",
        now.cli_word(),
        recorded.cli_word()
    );
    if recorded == Estimator::ExitCollection {
        hint.push_str(
            " To continue them, run without --path-cache, --persist-cache and the --beam flags;",
        );
    }
    hint.push_str(" --restart renders them again (in the app, use Restart).");
    hint
}

/// Shows each of `notes` (the accelerations the estimator dropped) to `sink` before the render
/// starts.
fn announce_notes(job: &RenderJobFile, notes: &[&'static str], sink: &mut dyn JobSink) {
    let frames_total = match &job.kind {
        JobKind::Still(_) => 1,
        JobKind::TiltVideo(video) => video.total_frames,
    };
    for note in notes {
        sink.progress(&JobProgress {
            frames_done: 0,
            frames_total,
            fraction: 0.0,
            eta_secs: None,
            note: Some((*note).to_string()),
            beam_phase: None,
        });
    }
}

/// The adaptive-sampling settings a job asks for, capped at the job's own samples per pixel.
#[must_use]
pub fn job_adaptive_settings(job: &RenderJobFile) -> Option<AdaptiveSettings> {
    let (accel, samples) = match &job.kind {
        JobKind::Still(still) => (&still.accel, still.samples_per_pixel),
        JobKind::TiltVideo(video) => (&video.accel, video.samples_per_pixel),
    };
    accel.adaptive.map(|settings| AdaptiveSettings {
        max_spp: samples,
        ..settings
    })
}

/// The path-cache settings a job asks for. A still keeps its `accel.path_cache` as it is; a tilt
/// video's frame caches reach the disk only when the job says `keep_path_cache`.
#[must_use]
pub fn job_path_cache_settings(job: &RenderJobFile) -> Option<PathCacheSettings> {
    match &job.kind {
        JobKind::Still(still) => still.accel.path_cache,
        JobKind::TiltVideo(video) => video.accel.path_cache.map(|cache| PathCacheSettings {
            persist: video.keep_path_cache,
            ..cache
        }),
    }
}

/// The beam-tracing settings a job asks for (`accel.beam` of a still or a tilt video).
#[must_use]
pub const fn job_beam_settings(job: &RenderJobFile) -> Option<BeamSettings> {
    match &job.kind {
        JobKind::Still(still) => still.accel.beam,
        JobKind::TiltVideo(video) => video.accel.beam,
    }
}

/// `taps`, with the job's own adaptive, path-cache and beam settings when the caller (the command
/// line) set none: flags override the job. A job with none returns `taps` unchanged.
fn taps_with_job_adaptive(job: &RenderJobFile, taps: &RenderTaps) -> RenderTaps {
    let mut effective = taps.clone();
    if effective.adaptive.is_none() {
        effective.adaptive = job_adaptive_settings(job);
    }
    if effective.path_cache.is_none() {
        effective.path_cache = job_path_cache_settings(job);
    }
    if effective.beam.is_none() {
        effective.beam = job_beam_settings(job);
    }
    effective
}

/// A frame count at the `u32` the job records use; saturates instead of wrapping.
fn count_u32(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

const fn failed(kind: FailureKind, message: String, frames_done: u32) -> JobOutcome {
    JobOutcome::Failed {
        kind,
        message,
        frames_done,
    }
}

/// The job's compute choices with the context's overrides applied, and the worker the
/// context names.
pub(super) fn remote_selection_for(job: &RenderJobFile, ctx: &ExecContext) -> RemoteSelection {
    let mut compute = job.compute;
    if let Some(target) = ctx.compute_override {
        compute.target = target;
    }
    if let Some(transfer) = ctx.transfer_override {
        compute.transfer = transfer;
    }
    if let Some(contribute) = ctx.contribute_local_override {
        compute.contribute_local = contribute;
    }
    if !crate::bridge::render_thread::remote_allowed(job.engine) {
        compute.target = ComputeChoice::Local;
    }
    convert::remote_selection(compute, ctx.worker.clone())
}

fn output_of(job: &RenderJobFile, ctx: &ExecContext) -> PathBuf {
    ctx.output_override
        .clone()
        .unwrap_or_else(|| job.output_path(&ctx.base_dir))
}

// ---- Still ---------------------------------------------------------------------------

#[expect(
    clippy::too_many_arguments,
    reason = "the job, its context and scene, the remote selection, the taps, cancellation and the sink"
)]
fn execute_still(
    job: &RenderJobFile,
    still: &StillJob,
    ctx: &ExecContext,
    scene: &SceneSnapshot,
    remote: &RemoteSelection,
    taps: &RenderTaps,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let params = ExportParams {
        engine: job.engine,
        width: job.scene.width,
        height: job.scene.height,
        samples_per_pixel: still.samples_per_pixel,
        max_bounces: job.scene.max_bounces,
    };
    let mut out = output_of(job, ctx);
    if out.exists() {
        out = reserve_unique_file(&out, &[], &|path| path.exists());
    }
    let mut eta = EtaEstimator::default();
    let outcome = run_export_tapped(
        scene,
        params,
        convert::color_space_of(still.color_space),
        &out,
        remote,
        ctx.local_compute,
        taps,
        cancel,
        |progress: ExportProgress| {
            let now = Instant::now();
            #[cfg(feature = "beam")]
            let eta_secs = {
                // A render that gave up its beams starts over as an ordinary render: its estimate
                // must not be fitted through the probe's reports too.
                eta.observe_stretch(now, f64::from(progress.fraction), progress.beam.is_some());
                // A beam report carries the renderer's own estimate (per-batch rate plus the batch
                // in progress). The fraction regression is for the plain render.
                progress
                    .beam
                    .map_or_else(|| eta.eta(now), |beam| beam.eta)
                    .map(|left| left.as_secs_f64())
            };
            #[cfg(not(feature = "beam"))]
            let eta_secs = {
                eta.observe(now, f64::from(progress.fraction));
                eta.eta(now).map(|left| left.as_secs_f64())
            };
            sink.progress(&JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction: progress.fraction,
                eta_secs,
                note: progress.note,
                #[cfg(feature = "beam")]
                beam_phase: progress.beam.map(|beam| beam.phase),
                #[cfg(not(feature = "beam"))]
                beam_phase: None,
            });
        },
    );
    match outcome {
        ExportOutcome::Completed(result) => JobOutcome::Done { result, note: None },
        ExportOutcome::Cancelled => JobOutcome::Stopped { frames_done: 0 },
        ExportOutcome::Failed(message) => {
            let kind = if message.starts_with("Could not create output directory")
                || message.starts_with("Failed to write PNG")
            {
                FailureKind::Output
            } else {
                FailureKind::Render
            };
            failed(kind, message, 0)
        }
    }
}

// ---- Tilt video ----------------------------------------------------------------------

#[expect(
    clippy::too_many_arguments,
    reason = "the job, its context and scene, remote, taps, token change, cancellation, sink"
)]
fn execute_video(
    job: &RenderJobFile,
    video: &TiltVideoJob,
    ctx: &ExecContext,
    scene: SceneSnapshot,
    remote: RemoteSelection,
    taps: &RenderTaps,
    change: Option<&TokenChange>,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let dir = output_of(job, ctx);
    let total = video.total_frames as usize;
    let axes: Vec<usize> = video.axes().into_iter().map(|axis| axis as usize).collect();
    if params::pose_count(&axes, video.start_deg, video.end_deg, video.step_deg) != total {
        return failed(
            FailureKind::Input,
            "The tilt range and step do not give the number of frames the job names.".to_string(),
            0,
        );
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return failed(
            FailureKind::Output,
            format!("Could not create the folder {}: {e}", dir.display()),
            0,
        );
    }
    let folder_rule = FolderRule {
        token: &job.token,
        restart: ctx.restart_frames,
        change,
        estimator: job.scene.estimator,
    };
    if let Err(outcome) = prepare_frame_folder(&dir, &folder_rule) {
        return outcome;
    }

    let request =
        convert::video_request_from_job(job, video, scene, dir.clone(), remote, ctx.local_compute);
    let frames_on_disk = frames::existing_frames(&dir, total)
        .into_iter()
        .filter(|exists| *exists)
        .count();
    let mut reporter = SinkReporter {
        sink,
        eta: EtaEstimator::default(),
        frames_done: frames_on_disk,
        work: None,
    };
    // A resumed video shows where it continues from before its first frame is done.
    reporter.report(
        frames_on_disk,
        total,
        frames_on_disk as f32 / total.max(1) as f32,
        None,
    );

    match frames::render_frames_tapped(&request, true, taps, cancel, &mut reporter) {
        FramesOutcome::Done(paths) => finish_video(video, &dir, &paths, total),
        FramesOutcome::Cancelled { frames_done } => JobOutcome::Stopped {
            frames_done: count_u32(frames_done),
        },
        FramesOutcome::Failed {
            message,
            frames_done,
        } => failed(FailureKind::Render, message, count_u32(frames_done)),
    }
}

/// What decides whether a tilt video may use its frame folder.
pub(super) struct FolderRule<'a> {
    /// The token of this run (the estimator rule already applied).
    pub(super) token: &'a str,
    /// A restart was asked for.
    pub(super) restart: bool,
    /// What the estimator rule did to the token, if it changed it.
    pub(super) change: Option<&'a TokenChange>,
    /// The estimator this run renders with.
    pub(super) estimator: Estimator,
}

impl FolderRule<'_> {
    /// Whether `owner`, the token a folder's marker names, is the job's own token before the
    /// estimator rule changed it: the folder holds frames of the estimator the job file recorded.
    fn holds_recorded_estimators_frames(&self, owner: Option<&str>) -> bool {
        self.change
            .is_some_and(|change| owner == Some(change.old_token.as_str()))
    }

    /// Whether the folder of the marker owner `owner` is refused: another job's folder is, except
    /// that a restart takes over the frames of the job's own file under its recorded estimator.
    pub(super) fn refuses(&self, owner: Option<&str>) -> bool {
        let takeover = self.restart && self.holds_recorded_estimators_frames(owner);
        owner != Some(self.token) && !takeover
    }

    /// The sentence a refusal adds when the folder holds frames of the recorded estimator.
    pub(super) fn hint(&self, owner: Option<&str>) -> String {
        self.change
            .filter(|_| self.holds_recorded_estimators_frames(owner))
            .map_or_else(String::new, |change| {
                estimator_change_hint(change.recorded, self.estimator)
            })
    }
}

/// Checks the frame folder belongs to this job (or is fresh), clears it when a restart was
/// asked for, removes the leftovers of an interrupted frame and writes the marker. A folder of
/// another job is refused; when it holds the frames of the estimator the job file recorded, the
/// message names the way out.
fn prepare_frame_folder(dir: &Path, rule: &FolderRule<'_>) -> Result<(), JobOutcome> {
    let (token, restart) = (rule.token, rule.restart);
    let marker = dir.join(FRAME_MARKER_FILE);
    if marker.exists() {
        let text = std::fs::read_to_string(&marker).ok();
        let owner = text.as_deref().and_then(marker_token);
        if rule.refuses(owner) {
            return Err(failed(
                FailureKind::Input,
                format!(
                    "The folder {} belongs to another render job.{}",
                    dir.display(),
                    rule.hint(owner)
                ),
                0,
            ));
        }
    } else if frames::holds_frames(dir) && !restart {
        return Err(failed(
            FailureKind::Input,
            format!(
                "The folder {} already holds frames from another export. Choose another \
                 folder or delete those frames.",
                dir.display()
            ),
            0,
        ));
    }
    if restart && let Err(e) = frames::clear_frames(dir) {
        return Err(failed(
            FailureKind::Output,
            format!("Could not clear the old frames in {}: {e}", dir.display()),
            0,
        ));
    }
    frames::remove_partials(dir);
    std::fs::write(&marker, marker_text(token)).map_err(|e| {
        failed(
            FailureKind::Output,
            format!("Could not write in the folder {}: {e}", dir.display()),
            0,
        )
    })
}

/// Encodes the finished frames. A video that was made (and not asked to keep its frames)
/// takes the frames and the marker with it.
fn finish_video(video: &TiltVideoJob, dir: &Path, paths: &[PathBuf], total: usize) -> JobOutcome {
    let outcome = encode::encode(
        dir,
        paths,
        params::frame_number_digits(total),
        video.fps,
        &video.video_name,
    );
    let (result, message) = frames::encode_outcome_message(&outcome, dir);
    if !video.keep_frames && matches!(outcome, EncodeOutcome::Mp4(_) | EncodeOutcome::Gif { .. }) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(dir.join(FRAME_MARKER_FILE));
    }
    let note = match outcome {
        EncodeOutcome::Mp4(_) => None,
        EncodeOutcome::Gif { .. } | EncodeOutcome::FramesOnly { .. } => Some(message),
    };
    JobOutcome::Done { result, note }
}

/// Forwards the frame loop's progress to a job sink.
struct SinkReporter<'a> {
    sink: &'a mut dyn JobSink,
    eta: EtaEstimator,
    /// Frames on disk, including the ones that were there before this run.
    frames_done: usize,
    /// This run's rendering work done as of the next saved frame
    /// (`frames::FrameReporter::work_done`): the time estimate follows it, since copied
    /// frames cost nothing.
    work: Option<f64>,
}

impl SinkReporter<'_> {
    fn report(&mut self, frames_done: usize, total: usize, fraction: f32, note: Option<String>) {
        let eta_secs = self.eta.eta(Instant::now()).map(|left| left.as_secs_f64());
        self.sink.progress(&JobProgress {
            frames_done: count_u32(frames_done),
            frames_total: count_u32(total),
            fraction,
            eta_secs,
            note,
            beam_phase: None,
        });
    }
}

impl FrameReporter for SinkReporter<'_> {
    fn frame_progress(&mut self, _index: usize, total: usize, fraction: f32, note: Option<String>) {
        let overall = (self.frames_done as f32 + fraction) / total.max(1) as f32;
        self.report(self.frames_done, total, overall, note);
    }

    fn frame_saved(&mut self, _index: usize, frames_done: usize, total: usize, _secs: f32) {
        self.frames_done = frames_done;
        let by_count = frames_done as f64 / total.max(1) as f64;
        let work = self.work.take().unwrap_or(by_count);
        self.eta.observe(Instant::now(), work);
        self.report(
            frames_done,
            total,
            frames_done as f32 / total.max(1) as f32,
            None,
        );
        self.sink
            .frame_finished(count_u32(frames_done), count_u32(total));
    }

    fn work_done(&mut self, fraction: f64) {
        self.work = Some(fraction);
    }
}
