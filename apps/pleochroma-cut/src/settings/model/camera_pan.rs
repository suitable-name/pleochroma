//! `AppSettings::camera_pan`: where the stone sits in the picture, and the one rule every
//! loaded or saved pan passes through.

use pleochroma::optics::raytracer::MAX_CAMERA_PAN;

/// `pan` as every renderer takes it: each component inside `-MAX_CAMERA_PAN..=MAX_CAMERA_PAN`,
/// and a component that is not a number centred (`0.0`). A pan already inside comes back bit
/// for bit.
#[must_use]
pub const fn clamp_camera_pan(pan: [f32; 2]) -> [f32; 2] {
    [clamp_component(pan[0]), clamp_component(pan[1])]
}

/// One component of [`clamp_camera_pan`].
const fn clamp_component(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-MAX_CAMERA_PAN, MAX_CAMERA_PAN)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pan inside the limits is kept exactly, one beyond them is limited, and damage centres
    /// only the damaged component.
    #[test]
    fn the_pan_is_limited_per_component_and_damage_centres_it() {
        assert_eq!(clamp_camera_pan([0.0, 0.0]), [0.0, 0.0]);
        let inside = [0.3_f32, -0.75];
        assert_eq!(
            clamp_camera_pan(inside).map(f32::to_bits),
            inside.map(f32::to_bits)
        );
        assert_eq!(
            clamp_camera_pan([9.0, -9.0]),
            [MAX_CAMERA_PAN, -MAX_CAMERA_PAN]
        );
        assert_eq!(clamp_camera_pan([f32::NAN, 0.5]), [0.0, 0.5]);
        assert_eq!(clamp_camera_pan([0.25, f32::INFINITY]), [0.25, 0.0]);
    }
}
