//! Tests of Update Job from This View: the owner guard, the running guard, the renderer switches, the video re-capture and the guarded write.

use super::update::{
    RUNNING_REASON, UpdateOutcome, availability, hdr_is_missing, owner_check, path_default_samples,
    prepare_snapshot, rebuild_job, saved_text, write_update,
};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::render_jobs::convert::{self, StillJobInputs},
};
use pleochroma::{
    color::ColorSpace,
    renderer::{
        accel::RenderAccel,
        engine::{EngineSpec, RayOptions, RenderEngine},
    },
};
use pleochroma_render_jobs::{
    ComputeChoice, JobColorSpace, JobKind, RenderJobFile, codec,
    job::{DesignInfo, HdrSource, MetricCurvesData, OverlaySelection, TiltVideoJob, format_for},
};
use pleochroma_vault::{
    db::sqlite::{Database, ExpectedRow},
    model::render_job::{NewRenderJob, RenderJobMeta},
};
use std::{path::Path, sync::Mutex};

/// The scene of the default context, as the live view captures it.
fn live() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("the scene captures")
}

fn ray(taps: u8) -> EngineSpec {
    EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: taps,
            fire: true,
        },
    }
}

fn still_job(output: &str) -> RenderJobFile {
    let remote = RemoteSelection::local_only();
    convert::build_still_job(
        &StillJobInputs {
            scene: &live(),
            params: ExportParams {
                engine: EngineSpec::default(),
                width: 16,
                height: 16,
                samples_per_pixel: 2,
                max_bounces: 4,
            },
            color_space: ColorSpace::Srgb,
            remote: &remote,
            output: Path::new(output),
            label: "Test".to_string(),
            design: DesignInfo {
                title: "Pentalume".to_string(),
                ..DesignInfo::default()
            },
            preset_label: String::new(),
            adaptive: None,
            path_cache: None,
            beam: None,
        },
        0,
    )
    .expect("the still job builds")
}

fn curves() -> MetricCurvesData {
    MetricCurvesData {
        brilliance: vec![0.5; 181],
        windowing: vec![0.5; 181],
        extinction: vec![0.5; 181],
    }
}

/// A video of two axes with an overlay and its frozen curves.
fn video_job(output: &str) -> RenderJobFile {
    let mut job = still_job(output);
    job.kind = JobKind::TiltVideo(TiltVideoJob {
        axis_index: 0,
        more_axes: vec![2],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 5,
        fps: 30,
        samples_per_pixel: 3,
        color_space: JobColorSpace::Srgb,
        overlay: OverlaySelection {
            brilliance: true,
            ..OverlaySelection::default()
        },
        curves: Some(curves()),
        more_curves: vec![curves()],
        keep_frames: true,
        video_name: "clip".to_string(),
        accel: RenderAccel::default(),
        keep_path_cache: false,
    });
    job.format = format_for(&job);
    job
}

/// The job after an update that captured the default scene for `engine`.
fn updated(job: &RenderJobFile, engine: EngineSpec, path_default: u32) -> RenderJobFile {
    let scene = prepare_snapshot(job, live(), engine).expect("the scene is ready");
    rebuild_job(job, &scene, engine, path_default).expect("the job rebuilds")
}

fn samples(job: &RenderJobFile) -> u32 {
    match &job.kind {
        JobKind::Still(still) => still.samples_per_pixel,
        JobKind::TiltVideo(video) => video.samples_per_pixel,
    }
}

fn db() -> Database {
    Database::new(Some(":memory:")).expect("the vault opens")
}

fn store(db: &Database, job: &RenderJobFile) -> i64 {
    let text = codec::to_text(job).expect("the job encodes");
    db.add_render_job(
        &NewRenderJob {
            kind: job.kind_word(),
            label: &job.label,
            summary: "test",
            frames_total: job.frames_total(),
            output_path: &job.output,
            snapshot_version: job.format,
            snapshot: &text,
        },
        0,
    )
    .expect("the row is added")
}

fn reload(db: &Database, id: i64) -> (RenderJobMeta, RenderJobFile) {
    let row = db.get_render_job(id).expect("reads").expect("exists");
    let job = codec::from_text(&row.snapshot).expect("decodes");
    (row.meta, job)
}

fn expected(meta: &RenderJobMeta) -> ExpectedRow {
    ExpectedRow {
        state: meta.state.clone(),
        frames_done: i64::from(meta.frames_done),
    }
}

/// Puts the row in `state` with `frames` finished.
fn put(db: &Database, id: i64, state: &str, frames: u32) {
    db.set_render_job_state(id, state, None, 1).expect("state");
    db.set_render_job_progress(id, frames, 1).expect("progress");
}

/// The job with another label, so that the update has something to write.
fn relabelled(job: &RenderJobFile) -> RenderJobFile {
    let mut new = job.clone();
    new.label = "Updated".to_string();
    new
}

#[test]
fn an_update_refuses_unless_the_live_stone_is_this_jobs() {
    assert_eq!(owner_check(PlanesOwner::Job { job_id: 7 }, 7), Ok(()));
    for owner in [
        PlanesOwner::Builtin,
        PlanesOwner::Editor { generation: 3 },
        PlanesOwner::Catalogue { entry_id: 7 },
        PlanesOwner::Job { job_id: 8 },
    ] {
        assert!(owner_check(owner, 7).is_err(), "{owner:?}");
    }
}

#[test]
fn the_update_is_off_while_the_job_runs_and_says_why() {
    assert_eq!(availability(Some(4), false, false), (true, ""));
    assert_eq!(availability(Some(4), true, false), (false, RUNNING_REASON));
    assert_eq!(RUNNING_REASON, "This job is rendering. Pause it first.");
    assert_eq!(availability(None, false, false), (false, ""));
    let (can, reason) = availability(Some(4), false, true);
    assert!(!can);
    assert!(reason.starts_with("Measuring the tilt curves of this job's stone"));
    // A running job wins over a measurement: its reason is the one to read.
    assert_eq!(availability(Some(4), true, true), (false, RUNNING_REASON));
}

#[test]
fn a_still_keeps_its_token_output_design_and_size() {
    let job = still_job("out/a.png");
    let new = updated(&job, EngineSpec::default(), 1024);
    assert_eq!(new.token, job.token);
    assert_eq!(new.output, job.output);
    assert_eq!(new.design.title, job.design.title);
    assert_eq!((new.scene.width, new.scene.height), (16, 16));
    assert_eq!(new.scene.max_bounces, 4, "the job's own bounce cap");
    assert_eq!(
        samples(&new),
        2,
        "Path-Traced to Path-Traced keeps the samples"
    );
    assert_eq!(new.format, format_for(&new));
    assert_eq!(new.validate(), Ok(()));
}

#[test]
fn path_to_ray_sets_the_taps_both_engine_copies_and_a_local_compute() {
    let mut job = still_job("out/a.png");
    job.compute.target = ComputeChoice::Both;
    job.format = format_for(&job);
    let new = updated(&job, ray(4), 1024);
    assert_eq!(new.engine, ray(4));
    assert_eq!(new.scene.engine, ray(4));
    assert_eq!(samples(&new), 4);
    assert_eq!(new.compute.target, ComputeChoice::Local);
    assert!(new.optical_diagnostics.is_none());
    assert!(new.zoning.is_none());
    assert_eq!(new.validate(), Ok(()));
}

#[test]
fn ray_to_path_takes_the_path_default_and_names_it() {
    let job = still_job("out/a.png");
    let as_ray = updated(&job, ray(8), 1024);
    assert_eq!(samples(&as_ray), 8);
    let back = updated(&as_ray, EngineSpec::default(), 512);
    assert!(!back.engine.is_ray());
    assert!(!back.scene.engine.is_ray());
    assert_eq!(samples(&back), 512);
    assert_eq!(back.validate(), Ok(()));
    let text = saved_text(3, &as_ray, &back, false);
    assert!(text.contains("512 samples per pixel"), "{text}");
    assert!(saved_text(3, &job, &as_ray, false).contains("Ray-Traced"));
    assert!(!saved_text(3, &job, &job, false).contains("Traced"));
    assert!(saved_text(3, &job, &job, true).ends_with("It is back in the queue."));
}

#[test]
fn the_path_default_follows_the_slider_exponent_and_stays_in_range() {
    assert_eq!(path_default_samples(10), 1024);
    assert_eq!(path_default_samples(15), 32_768);
    assert_eq!(path_default_samples(40), 32_768);
}

#[test]
fn an_automatic_label_follows_the_view_and_a_typed_one_stays() {
    let mut job = still_job("out/a.png");
    job.label = "Pentalume \u{b7} Daylight \u{b7} 16\u{d7}16".to_string();
    if let JobKind::Still(still) = &mut job.kind {
        still.preset_label = "Daylight".to_string();
    }
    let new = updated(&job, EngineSpec::default(), 1024);
    assert_eq!(new.label, "Pentalume \u{b7} Current View \u{b7} 16\u{d7}16");

    let mut typed = job;
    typed.label = "My favorite".to_string();
    assert_eq!(
        updated(&typed, EngineSpec::default(), 1024).label,
        "My favorite"
    );
}

#[test]
fn a_video_keeps_its_sweep_axes_fps_overlay_and_frame_folder() {
    let job = video_job("out/clip_frames");
    let new = updated(&job, EngineSpec::default(), 1024);
    let (JobKind::TiltVideo(before), JobKind::TiltVideo(after)) = (&job.kind, &new.kind) else {
        panic!("both are videos");
    };
    assert_eq!(after.axes(), before.axes());
    assert_eq!(
        (
            after.start_deg,
            after.end_deg,
            after.step_deg,
            after.total_frames
        ),
        (
            before.start_deg,
            before.end_deg,
            before.step_deg,
            before.total_frames
        )
    );
    assert_eq!(
        (after.fps, after.keep_frames),
        (before.fps, before.keep_frames)
    );
    assert_eq!(after.overlay, before.overlay);
    assert_eq!(after.video_name, before.video_name);
    assert_eq!(after.samples_per_pixel, 3);
    assert_eq!(new.output, job.output);
    assert_eq!(new.token, job.token);
    assert_eq!(
        new.scene.stone_pose, [0.0; 3],
        "a video shows the stone as designed"
    );
}

#[test]
fn a_video_may_switch_to_ray_traced_as_far_as_the_job_rules_allow() {
    let job = video_job("out/clip_frames");
    let new = updated(&job, ray(16), 1024);
    assert_eq!(new.engine, ray(16));
    assert_eq!(new.scene.engine, ray(16));
    assert_eq!(samples(&new), 16);
    assert_eq!(new.validate(), Ok(()));
    let back = updated(&new, EngineSpec::default(), 2048);
    assert_eq!(samples(&back), 2048);
    assert_eq!(back.validate(), Ok(()));
}

#[test]
fn a_missing_hdr_map_is_found_by_its_path() {
    let mut job = still_job("out/a.png");
    assert!(
        !hdr_is_missing(&job, Path::new("out")),
        "a studio job has none"
    );
    job.hdr = Some(HdrSource {
        path: "no_such_folder/no_such_map.hdr".to_string(),
        sha256_hex: "00".repeat(32),
    });
    assert!(hdr_is_missing(&job, Path::new("out")));
}

#[test]
fn a_done_job_goes_back_in_the_queue_from_frame_zero_with_its_token() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    put(&db, id, "done", 1);
    let (meta, _) = reload(&db, id);
    let outcome = write_update(&db, &meta, &expected(&meta), &old, &relabelled(&old), false);
    assert_eq!(outcome, UpdateOutcome::Saved { requeued: true });
    let (after, job) = reload(&db, id);
    assert_eq!((after.state.as_str(), after.frames_done), ("queued", 0));
    assert_eq!(job.label, "Updated");
    assert_eq!(after.label, "Updated");
    assert_eq!(job.token, old.token);
    assert_eq!(job.output, old.output);
}

#[test]
fn a_paused_job_keeps_its_state_and_starts_from_frame_zero() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    put(&db, id, "paused", 1);
    let (meta, _) = reload(&db, id);
    let outcome = write_update(&db, &meta, &expected(&meta), &old, &relabelled(&old), false);
    assert_eq!(outcome, UpdateOutcome::Saved { requeued: false });
    let (after, _) = reload(&db, id);
    assert_eq!((after.state.as_str(), after.frames_done), ("paused", 0));
}

#[test]
fn a_failed_job_goes_back_in_the_queue_from_frame_zero() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    put(&db, id, "failed", 0);
    let (meta, _) = reload(&db, id);
    let outcome = write_update(&db, &meta, &expected(&meta), &old, &relabelled(&old), false);
    assert_eq!(outcome, UpdateOutcome::Saved { requeued: true });
    assert_eq!(reload(&db, id).0.state, "queued");
}

#[test]
fn a_running_job_is_refused_and_nothing_is_written() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    let (meta, _) = reload(&db, id);
    let outcome = write_update(&db, &meta, &expected(&meta), &old, &relabelled(&old), true);
    assert_eq!(outcome, UpdateOutcome::Refused(RUNNING_REASON.to_string()));
    assert_eq!(reload(&db, id).1.label, "Test");
}

#[test]
fn a_row_that_changed_in_the_meantime_is_not_written() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    let seeded = expected(&reload(&db, id).0);
    put(&db, id, "paused", 0);
    let (fresh, _) = reload(&db, id);
    let outcome = write_update(&db, &fresh, &seeded, &old, &relabelled(&old), false);
    assert_eq!(outcome, UpdateOutcome::RowChanged);
    assert_eq!(reload(&db, id).1.label, "Test");
}

#[test]
fn a_view_that_matches_the_job_writes_nothing() {
    let db = db();
    let old = still_job("out/a.png");
    let id = store(&db, &old);
    put(&db, id, "done", 1);
    let (meta, _) = reload(&db, id);
    let outcome = write_update(&db, &meta, &expected(&meta), &old, &old.clone(), false);
    assert_eq!(outcome, UpdateOutcome::Unchanged);
    let (after, _) = reload(&db, id);
    assert_eq!((after.state.as_str(), after.frames_done), ("done", 1));
}
