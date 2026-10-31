use super::{CameraPose, FRAC_PI_2, orbit_step, standard_view, unroll_drag_delta};
use pleochroma::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use std::f32::consts::{PI, TAU};

#[test]
fn turn_ninety_turns_the_yaw_keeps_tilt_and_zoom_and_four_turns_restore_the_original() {
    let oblique = CameraPose {
        yaw: 0.63,
        pitch: -0.37,
        roll: 0.0,
        distance: 3.7,
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
