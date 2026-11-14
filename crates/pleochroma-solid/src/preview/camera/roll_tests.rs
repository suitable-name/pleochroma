use super::{
    CameraPose, FRAC_PI_2, MAX_CAMERA_PAN, orbit_step, pan_pose, standard_view, unroll_drag_delta,
};
use pleochroma::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use std::f32::consts::{PI, TAU};

#[test]
fn turn_ninety_turns_the_yaw_keeps_tilt_and_zoom_and_four_turns_restore_the_original() {
    let oblique = CameraPose {
        yaw: 0.63,
        pitch: -0.37,
        roll: 0.0,
        distance: 3.7,
        pan: [0.0, 0.0],
    };
    for view in 0..=5 {
        let original = standard_view(view, oblique, 1.5);
        let mut turned = standard_view(6, original, 1.5);
        let expected_yaw = (original.yaw + FRAC_PI_2).rem_euclid(TAU);
        assert!((turned.yaw - expected_yaw).abs() < 1e-6, "view {view}");
        assert_eq!(
            (turned.pitch, turned.distance),
            (original.pitch, original.distance)
        );
        assert_eq!(turned.roll, 0.0, "Turn 90 never rolls the image");
        for _ in 0..3 {
            turned = standard_view(6, turned, 1.5);
        }
        let back = (turned.yaw - original.yaw.rem_euclid(TAU)).abs();
        assert!(
            back < 1e-5 || (back - TAU).abs() < 1e-5,
            "view {view}: {back}"
        );
    }
}

#[test]
fn every_view_but_fit_clears_a_roll_left_by_an_older_version() {
    let rolled = CameraPose {
        yaw: 0.63,
        pitch: 0.37,
        roll: FRAC_PI_2,
        distance: 3.7,
        pan: [0.0, 0.0],
    };
    for view in [0, 1, 2, 3, 4, 6] {
        assert_eq!(standard_view(view, rolled, 1.5).roll, 0.0, "view {view}");
    }
    assert_eq!(
        standard_view(5, rolled, 1.5).roll,
        FRAC_PI_2,
        "Fit changes only the distance"
    );
}

#[test]
fn drag_axes_follow_the_image_after_each_quarter_turn() {
    let pose = CameraPose {
        yaw: 0.6,
        pitch: 0.45,
        roll: 0.0,
        distance: 3.7,
        pan: [0.0, 0.0],
    };
    // Expected (yaw, pitch) changes for rightward and downward drags.
    for (roll, right, down) in [
        (0.0, (-0.08, 0.0), (0.0, 0.08)),
        (FRAC_PI_2, (0.0, -0.08), (-0.08, 0.0)),
        (PI, (0.08, 0.0), (0.0, -0.08)),
        (3.0 * FRAC_PI_2, (0.0, 0.08), (0.08, 0.0)),
    ] {
        let turned = CameraPose { roll, ..pose };
        for (dx, dy, expected) in [(10.0, 0.0, right), (0.0, 10.0, down)] {
            let moved = orbit_step(turned, dx, dy);
            assert!((moved.yaw - pose.yaw - expected.0).abs() < 1e-6);
            assert!((moved.pitch - pose.pitch - expected.1).abs() < 1e-6);
            assert_eq!(moved.roll, roll);
            assert_eq!(moved.distance, pose.distance);
        }
    }
}

fn panned_pose() -> CameraPose {
    CameraPose {
        yaw: 0.6,
        pitch: 0.45,
        roll: 0.3,
        distance: 3.7,
        pan: [0.2, -0.1],
    }
}

#[test]
fn pan_drag_moves_the_stone_with_the_pointer_and_clamps_to_the_picture() {
    let pose = panned_pose();
    // Half the height to the right: +1 in x; half the height down: -1 in y.
    let moved = pan_pose(pose, 150.0, 0.0, 600.0, 300.0);
    assert!((moved.pan[0] - 1.2).abs() < 1e-6);
    assert_eq!(moved.pan[1], pose.pan[1]);
    let moved = pan_pose(pose, 0.0, 75.0, 600.0, 300.0);
    assert!((moved.pan[1] - -0.6).abs() < 1e-6);
    assert_eq!(moved.pan[0], pose.pan[0]);
    assert_eq!(
        (moved.yaw, moved.pitch, moved.roll, moved.distance),
        (pose.yaw, pose.pitch, pose.roll, pose.distance)
    );
    let far = pan_pose(pose, 9000.0, -9000.0, 600.0, 300.0);
    assert_eq!(far.pan, [2.0, 1.0]);
    let far = pan_pose(pose, -9000.0, 9000.0, 600.0, 300.0);
    assert_eq!(far.pan, [-2.0, -1.0]);
}

/// In a viewport wider than 4:1 the stone stops at [`MAX_CAMERA_PAN`], the most a worker
/// accepts, before it reaches the side edge.
#[test]
fn a_very_wide_viewport_stops_the_pan_at_the_limit_a_worker_accepts() {
    let pose = panned_pose();
    let far = pan_pose(pose, 90_000.0, 0.0, 6000.0, 500.0);
    assert_eq!(far.pan, [MAX_CAMERA_PAN, pose.pan[1]]);
    let far = pan_pose(pose, -90_000.0, 0.0, 6000.0, 500.0);
    assert_eq!(far.pan, [-MAX_CAMERA_PAN, pose.pan[1]]);
}

#[test]
fn pan_drag_with_a_non_positive_size_changes_nothing() {
    let pose = panned_pose();
    for (width, height) in [(0.0, 300.0), (600.0, 0.0), (-1.0, 300.0), (600.0, f32::NAN)] {
        assert_eq!(pan_pose(pose, 10.0, 10.0, width, height), pose);
    }
}

#[test]
fn orbit_and_zoom_keep_the_pan_and_presets_clear_it() {
    let pose = panned_pose();
    assert_eq!(orbit_step(pose, 12.0, -7.0).pan, pose.pan);
    assert_eq!(standard_view(5, pose, 1.5).pan, pose.pan, "Fit keeps it");
    assert_eq!(
        standard_view(6, pose, 1.5).pan,
        pose.pan,
        "Turn 90 keeps it"
    );
    for view in [0, 1, 2, 3, 4] {
        assert_eq!(
            standard_view(view, pose, 1.5).pan,
            [0.0, 0.0],
            "view {view}"
        );
    }
}

#[test]
fn drag_mapping_preserves_the_displayed_camera_direction() {
    for (yaw, pitch) in [(0.0, 0.0), (0.0, FRAC_PI_2), (0.0, -FRAC_PI_2), (0.6, 0.45)] {
        let base = Camera::new(yaw, pitch, 3.7, DEFAULT_FOV_DEG);
        for roll in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2, 0.37] {
            let rolled = Camera::new(yaw, pitch, 3.7, DEFAULT_FOV_DEG).with_roll(roll);
            for (dx, dy) in [(10.0, 0.0), (0.0, 10.0), (7.0, -5.0)] {
                let (unrolled_x, unrolled_y) = unroll_drag_delta(roll, dx, dy);
                let displayed = rolled.right * dx - rolled.up * dy;
                let mapped = base.right * unrolled_x - base.up * unrolled_y;
                assert!(
                    (displayed - mapped).length() < 1e-5,
                    "drag ({dx}, {dy}) at roll {roll}, view ({yaw}, {pitch})"
                );
            }
        }
    }
}
