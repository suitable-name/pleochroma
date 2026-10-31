//! The camera pills: Reset is the viewport's own pose restore.

use pleochroma_solid::preview::camera;

use super::{FIT_VIEW, RESET_POSE, RESET_VIEW, ViewportState};

/// Reset is the viewport's own pose restore: the shared `camera::standard_view` reads the same
/// index as Turn 90, so forwarding it would turn the image instead of resetting it.
#[test]
fn reset_restores_the_pose_and_is_not_the_shared_turn_view() {
    let mut vp = ViewportState::default();
    vp.set_view(320.0, 200.0, 1.0, false);
    vp.set_standard_view(1);
    let top = vp.camera;
    assert_ne!(top, RESET_POSE);
    assert_ne!(
        camera::standard_view(RESET_VIEW, top, 1.5),
        RESET_POSE,
        "the shared view 6 turns the stone about its vertical axis"
    );
    assert_eq!(FIT_VIEW, 5);
    vp.set_standard_view(RESET_VIEW);
    assert_eq!(vp.camera, RESET_POSE);
}
