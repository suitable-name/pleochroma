//! The eight small-tilt poses (the four profile axes, tilted both ways) and their mean.
//!
//! The same poses are entries `90 - tilt` and `90 + tilt` of the full-axis tilt profile
//! ([`super::TILT_ANGLES_DEG`]) on each axis of [`super::PROFILE_AZIMUTHS_DEG`], so a reader
//! that has the whole profile can take the eight values from it and a reader that has only
//! the stone can evaluate exactly these poses. The RI range and the windowing Check use this
//! module and nothing else for a small tilt.

use super::PROFILE_AZIMUTHS_DEG;

/// How many poses [`small_tilt_poses`] returns: two directions on each profile axis.
pub const SMALL_TILT_POSE_COUNT: usize = 8;

/// The eight small-tilt poses for a tilt of `tilt_deg` degrees, as `(positive azimuth, signed
/// tilt)` in the convention of [`super::TILT_ANGLES_DEG`].
///
/// For each azimuth of [`PROFILE_AZIMUTHS_DEG`] in order the pose at `-tilt_deg` comes first,
/// then the one at `+tilt_deg`. A pose is the same as the entry `90 + tilt` of the full-axis
/// profile on that axis, where `tilt` is the signed value. Use [`small_tilt_camera`] for the
/// camera that evaluates a pose.
#[must_use]
pub fn small_tilt_poses(tilt_deg: f32) -> [(f32, f32); SMALL_TILT_POSE_COUNT] {
    let mut poses = [(0.0f32, 0.0f32); SMALL_TILT_POSE_COUNT];
    for (axis, &azimuth) in PROFILE_AZIMUTHS_DEG.iter().enumerate() {
        poses[2 * axis] = (azimuth, -tilt_deg);
        poses[2 * axis + 1] = (azimuth, tilt_deg);
    }
    poses
}

/// The mean of the eight values of a small-tilt measurement, in the order of
/// [`small_tilt_poses`].
#[must_use]
pub fn small_tilt_mean(values: &[f32; SMALL_TILT_POSE_COUNT]) -> f32 {
    values.iter().sum::<f32>() / 8.0
}

/// The camera `(yaw, pitch)` in radians for one pose of [`small_tilt_poses`].
///
/// It is the tilt-to-pose formula of the full-axis profile: the pitch is `90 - |tilt|` degrees,
/// and the yaw is the positive azimuth for a tilt of zero or more and the azimuth plus 180
/// degrees for a negative one. The result is bit-identical to the pose the profile sweep
/// evaluates for the same tilt.
#[must_use]
pub fn small_tilt_camera(positive_azimuth_deg: f32, tilt_deg: f32) -> (f32, f32) {
    let yaw_deg = if tilt_deg < 0.0 {
        positive_azimuth_deg + 180.0
    } else {
        positive_azimuth_deg
    };
    (yaw_deg.to_radians(), (90.0 - tilt_deg.abs()).to_radians())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::metrics::{
            TILT_ANGLES_DEG, evaluate_full_axis_profile_at_azimuth,
            evaluate_gem_optical_metrics_geom_core,
        },
        geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    /// Ten degrees is entry 80 (negative) and 100 (positive) of the 1 degree profile, per axis,
    /// in the order the axes are listed.
    #[test]
    fn ten_degrees_are_entries_80_and_100_of_the_profile() {
        let poses = small_tilt_poses(10.0);
        for (axis, &azimuth) in PROFILE_AZIMUTHS_DEG.iter().enumerate() {
            assert_eq!(poses[2 * axis], (azimuth, TILT_ANGLES_DEG[80]));
            assert_eq!(poses[2 * axis + 1], (azimuth, TILT_ANGLES_DEG[100]));
        }
    }

    /// The camera of each pose evaluates to the very figures the full-axis sweep stores at
    /// entries 80 and 100 of that axis.
    #[test]
    fn the_poses_measure_what_the_full_profile_holds() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let poses = small_tilt_poses(10.0);
        for axis in [0usize, 3] {
            let azimuth = PROFILE_AZIMUTHS_DEG[axis];
            let (brilliance, extinction, windowing) =
                evaluate_full_axis_profile_at_azimuth(&planes, &material, azimuth, environment);
            for (slot, entry) in [(0usize, 80usize), (1, 100)] {
                let (pose_azimuth, tilt) = poses[2 * axis + slot];
                let (yaw, pitch) = small_tilt_camera(pose_azimuth, tilt);
                let metrics = evaluate_gem_optical_metrics_geom_core(
                    StoneGeometry::planes_only(&planes),
                    &material,
                    yaw,
                    pitch,
                    environment,
                );
                assert_eq!(
                    metrics.brilliance_pct.to_bits(),
                    brilliance[entry].to_bits()
                );
                assert_eq!(
                    metrics.extinction_pct.to_bits(),
                    extinction[entry].to_bits()
                );
                assert_eq!(metrics.windowing_pct.to_bits(), windowing[entry].to_bits());
            }
        }
    }

    /// The mean is the plain mean of the eight values.
    #[test]
    fn the_mean_is_the_plain_mean() {
        let values = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        assert!((small_tilt_mean(&values) - 4.5).abs() < 1e-6);
        assert!(small_tilt_mean(&[7.0; 8]).to_bits() == 7.0f32.to_bits());
    }
}
