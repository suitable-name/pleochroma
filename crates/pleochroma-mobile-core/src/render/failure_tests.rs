//! A trace that died while running must never keep the renderer dead: the next epoch starts
//! afresh, and the sentence the user reads is a plain one.

use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use pleochroma::diag::{self, Engine, PrepareProgress, PrepareStep};

use super::{
    PauseReason, RenderState, RenderStatus,
    pool::{RUN_FAILED, Shared},
};
use crate::settings::RenderBackend;

/// The longest any wait of these tests takes.
const SOON: Duration = Duration::from_secs(10);

/// A snapshot with `step` in flight, begun just now.
fn in_flight(step: PrepareStep) -> PrepareProgress {
    PrepareProgress {
        engine: Engine::PathTraced,
        step,
        done: 3,
        total: 18,
        start_ms: diag::process_ms(),
    }
}

#[test]
fn the_next_epoch_replaces_a_failed_one() {
    let shared = Shared::new(1);
    shared.begin_epoch(1, 8, false);
    shared.fail(1, RUN_FAILED);
    assert_eq!(shared.status(), RenderStatus::Failed(RUN_FAILED.to_owned()));

    shared.begin_epoch(2, 8, false);
    assert_eq!(
        shared.status(),
        RenderStatus::Tracing {
            samples: 0,
            target: 8
        }
    );
}

#[test]
fn a_failure_of_a_superseded_epoch_is_ignored() {
    let shared = Shared::new(1);
    shared.begin_epoch(1, 8, false);
    shared.begin_epoch(2, 8, false);
    shared.fail(1, RUN_FAILED);
    assert!(matches!(shared.status(), RenderStatus::Tracing { .. }));
}

#[test]
fn a_fresh_render_state_has_not_failed_while_running() {
    assert!(!RenderState::default().run_failed());
}

#[test]
fn the_failure_sentence_is_plain() {
    let text = RUN_FAILED.to_ascii_lowercase();
    assert!(
        !text.contains("mutex") && !text.contains("poison"),
        "{text}"
    );
    assert!(text.ends_with('.'));
}

#[test]
fn preparing_is_reported_only_under_the_three_conditions() {
    let shared = Shared::new(1);
    let building = in_flight(PrepareStep::BuildingShaders);
    // Nothing runs: a step in flight is not this render's.
    assert_eq!(shared.status_with(&building, false), RenderStatus::Idle);

    shared.begin_epoch(1, 8, false);
    // Local, nothing merged, a GPU step in flight: Preparing.
    assert!(matches!(
        shared.status_with(&building, false),
        RenderStatus::Preparing {
            step: PrepareStep::BuildingShaders,
            done: 3,
            total: 18,
            elapsed_s: 0,
        }
    ));
    let tracing = |samples| RenderStatus::Tracing { samples, target: 8 };
    // No step in flight: the status stays what it was.
    assert_eq!(
        shared.status_with(&in_flight(PrepareStep::Idle), false),
        tracing(0)
    );
    // A remote epoch keeps its own "Waiting for..." sentences.
    shared.backend.select(RenderBackend::Remote);
    assert_eq!(shared.status_with(&building, false), tracing(0));
    shared.backend.select(RenderBackend::Automatic);
    // Something is merged: it traces.
    shared.debug_set_samples(2);
    assert_eq!(shared.status_with(&building, false), tracing(2));
    // A paused render is paused, not preparing.
    shared.debug_set_samples(0);
    shared.update_control(|control| control.user_paused = true);
    assert_eq!(
        shared.status_with(&building, false),
        RenderStatus::Paused(PauseReason::User)
    );
    // The preview's frozen preparing state shows whatever the pool does.
    assert!(matches!(
        Shared::new(1).status_with(&building, true),
        RenderStatus::Preparing { .. }
    ));
}

#[test]
fn session_cpu_only_is_per_engine_and_asks_for_a_restart_once() {
    let mut state = RenderState::default();
    assert!(!state.session_cpu_only(Engine::PathTraced));
    assert!(!state.session_cpu_only(Engine::RayTraced));
    assert!(!state.take_restart_wanted());

    state.use_cpu_for_session(Engine::RayTraced);
    assert!(state.session_cpu_only(Engine::RayTraced));
    assert!(!state.session_cpu_only(Engine::PathTraced));
    assert!(state.take_restart_wanted());
    assert!(
        !state.take_restart_wanted(),
        "the restart is asked for once"
    );
    assert_eq!(state.status(), RenderStatus::Idle);
}

#[test]
fn a_hung_scene_preparation_is_abandoned_without_waiting_for_it() {
    let mut state = RenderState::default();
    let policy = state.compute_policy();
    policy.set_capacity(1);
    let shared = Arc::clone(&state.shared);

    // The fake hung step: a job on `render-aux` that holds a permit inside a scene
    // preparation and only ends when the test lets it.
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    state.aux().submit(move || {
        let _permit = shared.policy.acquire(&|| false).expect("a permit");
        let token = shared.begin_aux_prepare();
        held_tx.send(()).expect("signal");
        let _ = release_rx.recv();
        shared.end_aux_prepare(token);
    });
    held_rx.recv_timeout(SOON).expect("the step started");
    assert!(
        policy.acquire(&|| true).is_none(),
        "the hung step fills the budget"
    );
    // A job queued now would wait behind the hung one for good.
    assert!(state.shared.aux_preparing());

    assert!(state.abandon_preparation(Engine::PathTraced));
    assert!(!state.shared.aux_preparing());

    // The replaced queue serves new work at once, and the permit is back.
    let (done_tx, done_rx) = mpsc::channel();
    state
        .aux()
        .submit(move || done_tx.send(()).expect("signal"));
    done_rx
        .recv_timeout(SOON)
        .expect("the new queue runs a job");
    assert_eq!(policy.active(), 0);
    drop(policy.acquire(&|| false).expect("the returned permit"));

    // The abandoned step finally ends: its late end changes nothing.
    release_tx.send(()).expect("release");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(policy.active(), 0);
    assert!(!state.shared.aux_preparing());
    // With nothing hung there is nothing to abandon.
    assert!(!state.abandon_preparation(Engine::PathTraced));
}

#[test]
fn a_hung_render_thread_is_replaced_and_returns_its_permit() {
    let mut state = RenderState::default();
    state.ensure_workers().expect("the render threads start");
    let workers = state.threads.len();
    let policy = state.compute_policy();
    policy.set_capacity(1);

    // The fake hung compile: a thread that holds a permit until the test lets it go.
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let hung = thread::spawn({
        let policy = Arc::clone(&policy);
        move || {
            let _permit = policy.acquire(&|| false).expect("a permit");
            held_tx.send(()).expect("signal");
            let _ = release_rx.recv();
        }
    });
    held_rx.recv_timeout(SOON).expect("the compile started");
    state.shared.set_worker_gpu(0, 0, true);
    assert_eq!(state.shared.gpu_workers(), [0]);
    assert!(
        policy.acquire(&|| true).is_none(),
        "the hung step fills the budget"
    );

    assert!(state.abandon_preparation(Engine::PathTraced));
    assert_eq!(
        state.shared.worker_generation(0),
        1,
        "a new thread owns index 0"
    );
    assert!(state.shared.gpu_workers().is_empty());
    assert_eq!(state.threads.len(), workers, "the pool keeps its size");
    drop(policy.acquire(&|| false).expect("the returned permit"));

    release_tx.send(()).expect("release");
    hung.join().expect("the hung thread ends");
    assert_eq!(policy.active(), 0, "the late drop frees nothing twice");
}
