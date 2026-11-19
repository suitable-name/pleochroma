//! Tests of the edit dialog's saves: the Save table, the optimistic guard, the token, the new-job path and the footer.

use super::{
    footer::{FooterFacts, footer_text},
    save::{Decision, Requeue, SaveOutcome, SavePlan, decide, insert_after, write_edit},
};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::render_jobs::convert::{self, StillJobInputs},
};
use pleochroma::{
    color::ColorSpace,
    renderer::{accel::RenderAccel, engine::EngineSpec},
};
use pleochroma_render_jobs::{
    ComputeChoice, JobColorSpace, JobKind, RenderJobFile, codec,
    edit::{EditMode, JobEdit},
    job::{DesignInfo, OverlaySelection, TiltVideoJob, format_for},
    state::JobState,
};
use pleochroma_vault::{
    db::sqlite::{Database, ExpectedRow},
    model::render_job::{NewRenderJob, RenderJobMeta},
};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

fn still_job(output: &str) -> RenderJobFile {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("the scene captures");
    let remote = RemoteSelection::local_only();
    convert::build_still_job(
        &StillJobInputs {
            scene: &scene,
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
            design: DesignInfo::default(),
            preset_label: String::new(),
            adaptive: None,
            path_cache: None,
            beam: None,
        },
        0,
    )
    .expect("the still job builds")
}

fn video_job(output: &str) -> RenderJobFile {
    let mut job = still_job(output);
    job.kind = JobKind::TiltVideo(TiltVideoJob {
        axis_index: 0,
        more_axes: Vec::new(),
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        samples_per_pixel: 1,
        color_space: JobColorSpace::Srgb,
        overlay: OverlaySelection::default(),
        curves: None,
        more_curves: Vec::new(),
        keep_frames: false,
        video_name: "clip".to_string(),
        accel: RenderAccel::default(),
        keep_path_cache: false,
    });
    job.format = format_for(&job);
    job
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

fn save(db: &Database, id: i64, edit: &JobEdit, requeue: Requeue) -> SaveOutcome {
    let (meta, job) = reload(db, id);
    write_edit(db, &meta, &job, &expected(&meta), edit, requeue, false)
}

/// Puts the row in `state` with `frames` finished.
fn put(db: &Database, id: i64, state: &str, frames: u32) {
    db.set_render_job_state(id, state, None, 1).expect("state");
    db.set_render_job_progress(id, frames, 1).expect("progress");
}

fn label_edit() -> JobEdit {
    JobEdit {
        label: Some("New label".to_string()),
        ..JobEdit::default()
    }
}

fn compute_edit() -> JobEdit {
    JobEdit {
        target: Some(ComputeChoice::Both),
        ..JobEdit::default()
    }
}

#[test]
fn the_save_table_decides_every_row() {
    use EditMode::{InPlace, KeepFramesRequeue, RequeueWithConfirm};
    let write = |new_state, reset_progress| {
        Decision::Write(SavePlan {
            new_state,
            reset_progress,
        })
    };
    let queued = Some(JobState::Queued);
    assert_eq!(
        decide(InPlace, false, false, Requeue::NotAsked),
        write(None, false)
    );
    assert_eq!(
        decide(InPlace, false, true, Requeue::NotAsked),
        write(None, true)
    );
    assert_eq!(
        decide(KeepFramesRequeue, true, false, Requeue::NotAsked),
        write(None, false)
    );
    assert_eq!(
        decide(KeepFramesRequeue, false, false, Requeue::NotAsked),
        write(queued, false)
    );
    assert_eq!(
        decide(KeepFramesRequeue, false, true, Requeue::NotAsked),
        write(queued, true)
    );
    assert_eq!(
        decide(RequeueWithConfirm, true, false, Requeue::NotAsked),
        write(None, false)
    );
    assert_eq!(
        decide(RequeueWithConfirm, false, false, Requeue::NotAsked),
        Decision::Ask
    );
    assert_eq!(
        decide(RequeueWithConfirm, false, false, Requeue::SaveOnly),
        write(None, false)
    );
    assert_eq!(
        decide(RequeueWithConfirm, false, false, Requeue::RenderAgain),
        write(queued, true)
    );
    assert_eq!(
        decide(RequeueWithConfirm, false, true, Requeue::NotAsked),
        write(queued, true)
    );
}

#[test]
fn a_failed_job_with_a_compute_change_keeps_its_frames_and_goes_back_in_the_queue() {
    let db = db();
    let id = store(&db, &still_job("out/a.png"));
    put(&db, id, "failed", 1);
    assert_eq!(
        save(&db, id, &compute_edit(), Requeue::NotAsked),
        SaveOutcome::Saved
    );
    let (meta, job) = reload(&db, id);
    assert_eq!((meta.state.as_str(), meta.frames_done), ("queued", 1));
    assert_eq!(job.compute.target, ComputeChoice::Both);
}

#[test]
fn a_failed_job_with_a_label_only_edit_stays_failed() {
    let db = db();
    let id = store(&db, &still_job("out/a.png"));
    put(&db, id, "failed", 0);
    assert_eq!(
        save(&db, id, &label_edit(), Requeue::NotAsked),
        SaveOutcome::Saved
    );
    let (meta, job) = reload(&db, id);
    assert_eq!(meta.state, "failed");
    assert_eq!(job.label, "New label");
    assert_eq!(meta.label, "New label");
}

#[test]
fn a_done_job_keeps_its_state_for_a_label_and_asks_for_a_compute_change() {
    let db = db();
    let id = store(&db, &still_job("out/a.png"));
    put(&db, id, "done", 1);
    assert_eq!(
        save(&db, id, &label_edit(), Requeue::NotAsked),
        SaveOutcome::Saved
    );
    assert_eq!(reload(&db, id).0.state, "done");

    assert_eq!(
        save(&db, id, &compute_edit(), Requeue::NotAsked),
        SaveOutcome::AskRequeue
    );
    assert_eq!(
        reload(&db, id).1.compute.target,
        ComputeChoice::Local,
        "nothing written yet"
    );

    assert_eq!(
        save(&db, id, &compute_edit(), Requeue::SaveOnly),
        SaveOutcome::Saved
    );
    let (meta, job) = reload(&db, id);
    assert_eq!((meta.state.as_str(), meta.frames_done), ("done", 1));
    assert_eq!(job.compute.target, ComputeChoice::Both);
}

#[test]
fn render_again_resets_the_frames() {
    let db = db();
    let job = still_job("out/a.png");
    let id = store(&db, &job);
    put(&db, id, "cancelled", 1);
    let edit = JobEdit {
        target: Some(ComputeChoice::Both),
        ..JobEdit::default()
    };
    assert_eq!(
        save(&db, id, &edit, Requeue::RenderAgain),
        SaveOutcome::Saved
    );
    let meta = reload(&db, id).0;
    assert_eq!((meta.state.as_str(), meta.frames_done), ("queued", 0));
}

#[test]
fn a_pixel_change_resets_the_frames_and_never_changes_the_token() {
    let db = db();
    let job = still_job("out/a.png");
    let id = store(&db, &job);
    put(&db, id, "paused", 1);
    let edit = JobEdit {
        size: Some((32, 32)),
        ..JobEdit::default()
    };
    assert_eq!(save(&db, id, &edit, Requeue::NotAsked), SaveOutcome::Saved);
    let (meta, saved) = reload(&db, id);
    assert_eq!((meta.state.as_str(), meta.frames_done), ("paused", 0));
    assert_eq!((saved.scene.width, saved.scene.height), (32, 32));
    assert_eq!(saved.token, job.token);
    assert!(
        meta.summary.contains("32"),
        "the detail line follows the size"
    );
}

#[test]
fn a_row_that_changed_under_the_dialog_is_not_written() {
    let db = db();
    let id = store(&db, &still_job("out/a.png"));
    let seeded = expected(&reload(&db, id).0);
    put(&db, id, "paused", 0);
    let (fresh, job) = reload(&db, id);
    let outcome = write_edit(
        &db,
        &fresh,
        &job,
        &seeded,
        &label_edit(),
        Requeue::NotAsked,
        false,
    );
    assert_eq!(outcome, SaveOutcome::RowChanged);
    assert_eq!(reload(&db, id).1.label, "Test");
}

#[test]
fn a_running_job_is_refused() {
    let db = db();
    let id = store(&db, &still_job("out/a.png"));
    let (meta, job) = reload(&db, id);
    let outcome = write_edit(
        &db,
        &meta,
        &job,
        &expected(&meta),
        &label_edit(),
        Requeue::NotAsked,
        true,
    );
    assert!(matches!(outcome, SaveOutcome::Refused(_)));
}

#[test]
fn a_new_job_goes_right_after_its_source_with_its_own_output_and_token() {
    let db = db();
    let job = still_job("out/a.png");
    let first = store(&db, &job);
    let second = store(&db, &still_job("out/b.png"));
    let new_id = insert_after(&db, first, job.clone()).expect("inserts");
    let order: Vec<i64> = db
        .list_render_jobs()
        .expect("lists")
        .iter()
        .map(|meta| meta.job_id)
        .collect();
    assert_eq!(order, vec![first, new_id, second]);
    let (meta, copy) = reload(&db, new_id);
    assert_eq!(meta.state, "queued");
    assert_ne!(copy.token, job.token);
    let outputs: Vec<PathBuf> = db
        .render_job_output_paths()
        .expect("paths")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(outputs.len(), 3);
    assert!(outputs[1] != outputs[0] && outputs[1] != outputs[2]);
}

#[test]
fn a_video_whose_frame_folder_changed_renders_from_the_start() {
    let db = db();
    let id = store(&db, &video_job("frames/clip"));
    put(&db, id, "paused", 2);
    let edit = JobEdit {
        output: Some(PathBuf::from("other/clip")),
        ..JobEdit::default()
    };
    assert_eq!(save(&db, id, &edit, Requeue::NotAsked), SaveOutcome::Saved);
    let (meta, job) = reload(&db, id);
    assert_eq!((meta.state.as_str(), meta.frames_done), ("paused", 0));
    assert!(job.output.contains("other"));

    let renamed = JobEdit {
        video_name: Some("clip two".to_string()),
        ..JobEdit::default()
    };
    put(&db, id, "paused", 2);
    assert_eq!(
        save(&db, id, &renamed, Requeue::NotAsked),
        SaveOutcome::Saved
    );
    assert_eq!(
        reload(&db, id).0.frames_done,
        0,
        "renaming a video resets it too"
    );
}

fn facts(state: JobState) -> FooterFacts {
    FooterFacts {
        state,
        is_video: false,
        frames_done: 0,
        edited: false,
        only_label: false,
        resets: false,
        field: None,
        samples_moved: false,
        sweep_or_axes: false,
        folder_changed: false,
        measuring: false,
        total_frames: 1,
    }
}

#[test]
fn the_footer_says_what_save_will_do_with_real_counts() {
    assert_eq!(
        footer_text(&facts(JobState::Queued)),
        "Waiting job: Save changes it in place."
    );
    let video = FooterFacts {
        is_video: true,
        frames_done: 36,
        ..facts(JobState::Queued)
    };
    assert_eq!(
        footer_text(&video),
        "Waiting video: Save keeps its 36 finished frames unless the picture changes."
    );
    let sweep = FooterFacts {
        edited: true,
        resets: true,
        sweep_or_axes: true,
        total_frames: 72,
        ..video
    };
    assert_eq!(
        footer_text(&sweep),
        "The sweep changed, so the video renders from the start (72 frames)."
    );
    let failed = FooterFacts {
        edited: true,
        resets: true,
        field: Some("size"),
        ..facts(JobState::Failed)
    };
    assert_eq!(
        footer_text(&failed),
        "Failed job: Save puts it back in the queue. The size changed, so it renders from the start."
    );
    let label_only = FooterFacts {
        edited: true,
        only_label: true,
        ..facts(JobState::Done)
    };
    assert_eq!(
        footer_text(&label_only),
        "Done job: a new label or name keeps it done."
    );
    let measuring = FooterFacts {
        measuring: true,
        ..facts(JobState::Paused)
    };
    assert!(footer_text(&measuring).starts_with("Measuring the tilt curves"));
}
