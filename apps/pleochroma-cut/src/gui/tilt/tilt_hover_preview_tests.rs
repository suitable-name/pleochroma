use super::camera_pose_for_hover;
use pleochroma::{
    color::metrics::PROFILE_AZIMUTHS_DEG,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};

/// Positive tilt stays on the axis's own (positive) azimuth, pitch equal to
/// `90 - tilt` (tilt is measured away from table-up, not camera elevation).
#[test]
fn positive_tilt_uses_the_axis_own_azimuth() {
    let (yaw, pitch) = camera_pose_for_hover(1, 30.0);
    assert!((yaw - PROFILE_AZIMUTHS_DEG[1].to_radians()).abs() < 1e-6);
    assert!((pitch - (90.0 - 30.0f32).to_radians()).abs() < 1e-6);
}

/// Negative tilt stays on the same meridian and carries on over the pole: same azimuth,
/// pitch `90 - tilt` (past 90 degrees). The eye is where the measured curve's opposite
/// azimuth at `90 - |tilt|` puts it.
#[test]
fn negative_tilt_carries_on_over_the_pole() {
    let (yaw, pitch) = camera_pose_for_hover(2, -25.0);
    assert!((yaw - PROFILE_AZIMUTHS_DEG[2].to_radians()).abs() < 1e-6);
    assert!((pitch - (90.0 + 25.0f32).to_radians()).abs() < 1e-6);

    let over_the_pole = Camera::new(yaw, pitch, 3.0, DEFAULT_FOV_DEG);
    let opposite = Camera::new(
        (PROFILE_AZIMUTHS_DEG[2] + 180.0).to_radians(),
        (90.0 - 25.0f32).to_radians(),
        3.0,
        DEFAULT_FOV_DEG,
    );
    assert!((over_the_pole.origin - opposite.origin).length() < 1e-5);
    assert!((over_the_pole.forward - opposite.forward).length() < 1e-5);
}

/// The flip a tilt video showed half-way: across table-up the picture must not turn.
/// The image basis one small step either side of tilt 0 is nearly the same, on every
/// sweep axis (it turned 180 degrees with the old opposite-azimuth pose).
#[test]
fn crossing_table_up_does_not_turn_the_picture() {
    for axis in 0..PROFILE_AZIMUTHS_DEG.len() {
        let camera = |tilt: f32| {
            let (yaw, pitch) = camera_pose_for_hover(axis, tilt);
            Camera::new(yaw, pitch, 3.0, DEFAULT_FOV_DEG)
        };
        for (before, after) in [(-0.5_f32, 0.5_f32), (-0.5, 0.0), (0.0, 0.5)] {
            let (a, b) = (camera(before), camera(after));
            assert!(
                a.right.dot(b.right) > 0.99,
                "axis {axis}: right turned ({before} -> {after})"
            );
            assert!(
                a.up.dot(b.up) > 0.99,
                "axis {axis}: up turned ({before} -> {after})"
            );
        }
    }
}

/// The shared table-up point: `tilt_deg == 0` must resolve to pitch 90°
/// (table-up), on the axis's own positive azimuth.
#[test]
fn zero_tilt_is_table_up_on_the_positive_azimuth() {
    let (yaw, pitch) = camera_pose_for_hover(0, 0.0);
    assert!((yaw - PROFILE_AZIMUTHS_DEG[0].to_radians()).abs() < 1e-6);
    assert!((pitch - 90.0f32.to_radians()).abs() < 1e-6);
}

/// The two edge-on extremes: `tilt_deg == +90` is pitch 0 deg and `-90` is pitch 180 deg
/// (edge-on/profile from the far side, reached over the pole), both on the axis's azimuth.
#[test]
fn extreme_tilt_is_edge_on() {
    let (positive_yaw, positive_pitch) = camera_pose_for_hover(3, 90.0);
    assert!((positive_yaw - PROFILE_AZIMUTHS_DEG[3].to_radians()).abs() < 1e-6);
    assert!(positive_pitch.abs() < 1e-6);

    let (negative_yaw, negative_pitch) = camera_pose_for_hover(3, -90.0);
    assert!((negative_yaw - PROFILE_AZIMUTHS_DEG[3].to_radians()).abs() < 1e-6);
    assert!((negative_pitch - 180.0f32.to_radians()).abs() < 1e-6);
}

/// An out-of-range axis index falls back to azimuth 0 rather than panicking --
/// `PROFILE_AZIMUTHS_DEG.get(axis_index)` is `None` past index 3.
#[test]
fn out_of_range_axis_index_falls_back_to_azimuth_zero() {
    let (yaw, _pitch) = camera_pose_for_hover(99, 10.0);
    assert!((yaw - 0.0f32.to_radians()).abs() < 1e-6);
}
