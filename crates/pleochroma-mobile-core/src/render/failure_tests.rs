//! A trace that died while running must never keep the renderer dead: the next epoch starts
//! afresh, and the sentence the user reads is a plain one.

use super::{
    RenderState, RenderStatus,
    pool::{RUN_FAILED, Shared},
};

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
