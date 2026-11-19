//! `Database::update_render_job`: the guarded rewrite of an edited render job.
//!
//! Covers the guard on state and `frames_done`, the columns a save writes and keeps, the
//! state and progress changes, and the refusals.

use super::super::{
    Database,
    render_jobs::{ExpectedRow, UpdatedJob},
};
use crate::model::render_job::{NewRenderJob, RenderJobMeta};

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

fn add(db: &Database, kind: &str) -> i64 {
    db.add_render_job(
        &NewRenderJob {
            kind,
            label: "Alpha",
            summary: "1920 x 1080 · 64 samples",
            frames_total: 4,
            output_path: "C:\\out\\a.png",
            snapshot_version: 8,
            snapshot: "{\"old\":true}",
        },
        1_000,
    )
    .unwrap()
}

fn meta(db: &Database, id: i64) -> RenderJobMeta {
    db.get_render_job(id).unwrap().expect("job exists").meta
}

fn expected(state: &str, frames_done: i64) -> ExpectedRow {
    ExpectedRow {
        state: state.to_string(),
        frames_done,
    }
}

fn edit() -> UpdatedJob {
    UpdatedJob {
        label: "Beta · ü".to_string(),
        summary: "3840 x 2160 · 128 samples".to_string(),
        output_path: "C:\\out\\b.png".to_string(),
        frames_total: 9,
        snapshot_version: 9,
        snapshot: "{\"new\":\"ü·×\"}".to_string(),
        new_state: None,
        reset_progress: false,
    }
}

#[test]
fn update_writes_the_edit_and_keeps_state_kind_and_progress() {
    let db = in_memory_db();
    let id = add(&db, "tilt_video");
    db.set_render_job_state(id, "paused", Some("note"), 2_000)
        .unwrap();
    db.set_render_job_progress(id, 3, 2_100).unwrap();
    db.set_render_job_result(id, Some("C:\\out\\x"), 2_200)
        .unwrap();

    assert!(
        db.update_render_job(id, &expected("paused", 3), &edit())
            .unwrap()
    );

    let loaded = db.get_render_job(id).unwrap().unwrap();
    assert_eq!(loaded.snapshot_version, 9);
    assert_eq!(loaded.snapshot, "{\"new\":\"ü·×\"}");
    let m = loaded.meta;
    assert_eq!(m.label, "Beta · ü");
    assert_eq!(m.summary, "3840 x 2160 · 128 samples");
    assert_eq!(m.output_path, "C:\\out\\b.png");
    assert_eq!(m.frames_total, 9);
    assert_eq!(m.kind, "tilt_video", "the kind never changes");
    assert_eq!(m.state, "paused");
    assert_eq!(m.frames_done, 3);
    assert_eq!(m.error_text.as_deref(), Some("note"));
    assert_eq!(m.result_path.as_deref(), Some("C:\\out\\x"));
    assert_eq!((m.created_at, m.position), (1_000, 1));
    assert!(m.updated_at > 2_200, "updated_at moves to the current time");
}

#[test]
fn update_returns_false_and_writes_nothing_when_the_state_changed() {
    let db = in_memory_db();
    let id = add(&db, "still");
    let before = db.get_render_job(id).unwrap().unwrap();

    assert!(
        !db.update_render_job(id, &expected("paused", 0), &edit())
            .unwrap()
    );
    assert_eq!(db.get_render_job(id).unwrap().unwrap(), before);
}

#[test]
fn update_returns_false_when_frames_done_changed() {
    let db = in_memory_db();
    let id = add(&db, "tilt_video");
    db.set_render_job_progress(id, 2, 1_500).unwrap();
    let before = db.get_render_job(id).unwrap().unwrap();

    assert!(
        !db.update_render_job(id, &expected("queued", 1), &edit())
            .unwrap()
    );
    assert_eq!(db.get_render_job(id).unwrap().unwrap(), before);
    assert!(
        db.update_render_job(id, &expected("queued", 2), &edit())
            .unwrap()
    );
}

#[test]
fn update_of_a_missing_job_is_false() {
    let db = in_memory_db();
    assert!(
        !db.update_render_job(99_999, &expected("queued", 0), &edit())
            .unwrap()
    );
}

#[test]
fn a_second_save_with_the_old_seed_is_refused_after_a_state_change() {
    let db = in_memory_db();
    let id = add(&db, "still");
    let seed = expected("queued", 0);
    assert!(db.update_render_job(id, &seed, &edit()).unwrap());
    db.set_render_job_state(id, "running", None, 5_000).unwrap();
    assert!(!db.update_render_job(id, &seed, &edit()).unwrap());
    assert_eq!(meta(&db, id).state, "running");
}

#[test]
fn a_new_state_clears_the_error_and_sets_the_timestamps() {
    let db = in_memory_db();
    let id = add(&db, "still");
    db.set_render_job_state(id, "failed", Some("boom"), 2_000)
        .unwrap();
    let job = UpdatedJob {
        new_state: Some("queued".to_string()),
        ..edit()
    };

    assert!(
        db.update_render_job(id, &expected("failed", 0), &job)
            .unwrap()
    );
    let m = meta(&db, id);
    assert_eq!(m.state, "queued");
    assert_eq!((m.error_text, m.finished_at), (None, None));

    let done = UpdatedJob {
        new_state: Some("done".to_string()),
        ..edit()
    };
    assert!(
        db.update_render_job(id, &expected("queued", 0), &done)
            .unwrap()
    );
    let m = meta(&db, id);
    assert_eq!(m.state, "done");
    assert!(m.finished_at.is_some());
}

#[test]
fn reset_progress_zeroes_the_frames_and_clears_result_and_start() {
    let db = in_memory_db();
    let id = add(&db, "tilt_video");
    db.set_render_job_state(id, "running", None, 2_000).unwrap();
    db.set_render_job_progress(id, 3, 2_100).unwrap();
    db.set_render_job_state(id, "paused", None, 2_200).unwrap();
    db.set_render_job_result(id, Some("C:\\out\\x"), 2_300)
        .unwrap();
    let job = UpdatedJob {
        reset_progress: true,
        ..edit()
    };

    assert!(
        db.update_render_job(id, &expected("paused", 3), &job)
            .unwrap()
    );
    let m = meta(&db, id);
    assert_eq!((m.state.as_str(), m.frames_done), ("paused", 0));
    assert_eq!((m.result_path, m.started_at), (None, None));
}

#[test]
fn a_done_job_restarts_through_new_state_and_reset_together() {
    let db = in_memory_db();
    let id = add(&db, "still");
    db.set_render_job_progress(id, 1, 2_000).unwrap();
    db.set_render_job_result(id, Some("C:\\out\\a.png"), 2_100)
        .unwrap();
    db.set_render_job_state(id, "done", None, 2_200).unwrap();
    let job = UpdatedJob {
        new_state: Some("queued".to_string()),
        reset_progress: true,
        ..edit()
    };

    assert!(
        db.update_render_job(id, &expected("done", 1), &job)
            .unwrap()
    );
    let m = meta(&db, id);
    assert_eq!((m.state.as_str(), m.frames_done), ("queued", 0));
    assert_eq!(
        (m.result_path, m.error_text, m.started_at, m.finished_at),
        (None, None, None, None)
    );
}

#[test]
fn an_empty_label_or_an_unknown_state_is_refused_before_the_update() {
    let db = in_memory_db();
    let id = add(&db, "still");
    let before = db.get_render_job(id).unwrap().unwrap();

    let no_label = UpdatedJob {
        label: String::new(),
        ..edit()
    };
    assert!(
        db.update_render_job(id, &expected("queued", 0), &no_label)
            .is_err()
    );
    let bad_state = UpdatedJob {
        new_state: Some("bogus".to_string()),
        ..edit()
    };
    assert!(
        db.update_render_job(id, &expected("queued", 0), &bad_state)
            .is_err()
    );
    assert_eq!(db.get_render_job(id).unwrap().unwrap(), before);
}
