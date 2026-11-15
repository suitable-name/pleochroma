//! The Live Render viewport's Solid view and crystal-axis marker with a Stone Pose.
//!
//! The traced picture shows the stone turned by the pose `R` in front of a fixed camera. The Solid
//! raster has no tray and always draws the stone as designed, so it shows the same picture by
//! drawing the unrotated stone with the camera turned by `R^-1` about the stone's origin: the
//! eye, the right axis and the up axis of the camera all turn, and the view of the stone is the
//! same. Any orientation that looks at the origin is a yaw, a pitch and a roll, so no new camera
//! type is needed; the pan is in picture axes and carries over unchanged. Only the Live Render
//! tab uses this; the Edit tab's Solid camera never receives it.

use crate::gui::solid_preview::preview_state::CameraPose;
use pleochroma::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
    render_setup::StonePose,
};

/// Below this horizontal length of the unit eye direction the eye is at a pole of the orbit and
/// its yaw is arbitrary; the camera's own yaw is kept there and the roll takes up the rest.
const POLE_HORIZONTAL: f32 = 1e-4;

/// The camera that draws the stone as designed the way `camera` sees it posed by `pose`: the
/// eye, the right axis and the up axis of `camera` (orbit, roll and distance) turned by the
/// inverse of the pose, as a yaw, a pitch within `-90..=90` degrees and a roll. The distance and
/// the pan are kept. The identity pose returns `camera` untouched.
///
/// A camera looking straight down on the table of a stone that faces it (the Face the Camera
/// pose) lands on the pole, where the yaw is `camera.yaw` and the roll carries the turn.
#[must_use]
pub fn inverse_pose_camera(pose: StonePose, camera: CameraPose) -> CameraPose {
    if pose.is_identity() {
        return camera;
    }
    // The rotation is orthonormal, so its transpose is its inverse.
    let to_model = pose.rotation().transpose();
    // The camera's own frame at unit distance, roll included, turned into the stone's frame.
    let seen = Camera::new(camera.yaw, camera.pitch, 1.0, DEFAULT_FOV_DEG).with_roll(camera.roll);
    let eye = to_model * seen.origin;
    let right = to_model * seen.right;
    let horizontal = eye.x.hypot(eye.z);
    let pitch = eye.y.atan2(horizontal);
    let yaw = if horizontal < POLE_HORIZONTAL {
        camera.yaw
    } else {
        eye.x.atan2(eye.z)
    };
    // The unrolled camera of that eye; the roll is the angle that turns its right and up axes
    // onto the turned right axis (`Camera::with_roll` turns them clockwise by the roll).
    let level = Camera::new(yaw, pitch, 1.0, DEFAULT_FOV_DEG);
    let roll = right.dot(level.up).atan2(right.dot(level.right));
    CameraPose {
        yaw,
        pitch,
        roll,
        distance: camera.distance,
        pan: camera.pan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use std::f32::consts::FRAC_PI_2;

    const POSES: [StonePose; 5] = [
        StonePose {
            tilt_deg: 90.0,
            turn_deg: 0.0,
            spin_deg: 0.0,
        },
        StonePose {
            tilt_deg: 75.0,
            turn_deg: 40.0,
            spin_deg: 0.0,
        },
        StonePose {
            tilt_deg: 30.0,
            turn_deg: 200.0,
            spin_deg: 65.0,
        },
        StonePose {
            tilt_deg: 150.0,
            turn_deg: 310.0,
            spin_deg: 12.0,
        },
        StonePose {
            tilt_deg: 0.0,
            turn_deg: 80.0,
            spin_deg: 0.0,
        },
    ];

    fn cameras() -> Vec<CameraPose> {
        let mut out = Vec::new();
        for (yaw, pitch, roll) in [
            (0.0, 0.0, 0.0),
            (0.6, 0.35, 0.0),
            (-2.1, 0.9, 0.4),
            (3.0, -0.6, -1.2),
            (0.8, 1.9, 0.0),
            (1.0, 0.0, FRAC_PI_2),
        ] {
            out.push(CameraPose {
                yaw,
                pitch,
                roll,
                distance: 2.4,
                pan: [0.2, -0.15],
            });
        }
        out
    }

    fn camera_of(pose: CameraPose) -> Camera {
        Camera::new(pose.yaw, pose.pitch, pose.distance, DEFAULT_FOV_DEG)
            .with_roll(pose.roll)
            .with_pan(pose.pan)
    }

    /// The coordinates of `point` in the camera's own frame: what decides where it is drawn.
    fn view_of(camera: &Camera, point: Vec3) -> Vec3 {
        let d = point - camera.origin;
        Vec3::new(d.dot(camera.right), d.dot(camera.up), d.dot(camera.forward))
    }

    #[test]
    fn the_identity_pose_leaves_the_camera_alone() {
        for camera in cameras() {
            let same = inverse_pose_camera(StonePose::IDENTITY, camera);
            assert_eq!(
                (same.yaw, same.pitch, same.roll, same.distance, same.pan),
                (
                    camera.yaw,
                    camera.pitch,
                    camera.roll,
                    camera.distance,
                    camera.pan
                )
            );
        }
    }

    /// Drawing the unrotated stone with the returned camera shows every point exactly where the
    /// posed stone shows it from the original camera.
    #[test]
    fn the_inverse_camera_sees_the_unrotated_stone_as_the_camera_sees_the_posed_one() {
        let points = [
            Vec3::new(0.3, 0.9, -0.2),
            Vec3::new(-0.6, -0.4, 0.5),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.2, 0.8),
        ];
        for pose in POSES {
            let r = pose.rotation();
            for camera in cameras() {
                let inverse = inverse_pose_camera(pose, camera);
                assert!(
                    inverse.pitch.abs() <= FRAC_PI_2 + 1e-6,
                    "the pitch stays within the poles"
                );
                assert_eq!(inverse.distance.to_bits(), camera.distance.to_bits());
                assert_eq!(inverse.pan, camera.pan, "the pan is in picture axes");
                let posed_view = camera_of(camera);
                let model_view = camera_of(inverse);
                for p in points {
                    let posed = view_of(&posed_view, r * p);
                    let model = view_of(&model_view, p);
                    assert!(
                        (posed - model).length() < 5e-4,
                        "pose {pose:?} camera {camera:?} point {p}: {posed} vs {model}"
                    );
                }
            }
        }
    }

    /// The camera frame itself is the old frame turned by the inverse rotation.
    #[test]
    fn the_inverse_camera_is_the_old_frame_turned_by_the_inverse_pose() {
        let pose = POSES[1];
        let to_model = pose.rotation().transpose();
        for camera in cameras() {
            let old = camera_of(camera);
            let new = camera_of(inverse_pose_camera(pose, camera));
            for (turned, got) in [
                (to_model * old.origin, new.origin),
                (to_model * old.forward, new.forward),
                (to_model * old.right, new.right),
                (to_model * old.up, new.up),
            ] {
                assert!((turned - got).length() < 5e-4, "{turned} vs {got}");
            }
        }
    }

    /// A stone that faces the camera (tilt 90, turn 0 for a camera at yaw 0, pitch 0) is seen from
    /// straight above in its own frame: the camera lands on the pole.
    #[test]
    fn a_stone_facing_the_camera_puts_the_camera_over_its_table() {
        let camera = CameraPose {
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            distance: 2.4,
            pan: [0.0, 0.0],
        };
        let inverse = inverse_pose_camera(POSES[0], camera);
        assert!(
            (inverse.pitch - FRAC_PI_2).abs() < 1e-3,
            "{}",
            inverse.pitch
        );
        let eye = camera_of(inverse).origin;
        assert!((eye - Vec3::new(0.0, 2.4, 0.0)).length() < 5e-3, "{eye}");
    }
}
