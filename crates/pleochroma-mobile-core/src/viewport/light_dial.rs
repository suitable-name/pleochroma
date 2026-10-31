//! The light-position indicator of the Render view: a small compass with a dot for the
//! light's direction, and the numbers under it.
//!
//! A light drag cannot be shown by a picture while it runs (every light is a new scene and
//! the first pass of a new epoch takes longer than a drag event), so the dial is what
//! follows the finger. The azimuth runs around the dial clockwise from the top, the
//! elevation is the distance from the centre: a light straight overhead sits in the middle,
//! one at the horizon on the rim. Plain arithmetic, tested natively; the Slint side only
//! places the dot.

use crate::{app_state::AppState, design_props::lighting_view};

/// The light's elevation at the rim of the dial, in degrees.
const RIM_ELEVATION_DEG: f32 = 0.0;
/// The light's elevation at the centre of the dial, in degrees.
const CENTRE_ELEVATION_DEG: f32 = 90.0;

/// What the indicator shows.
#[derive(Debug, Clone, PartialEq)]
pub struct LightDial {
    /// The dot's horizontal place, `-1.0` (left rim) to `1.0` (right rim).
    pub x: f32,
    /// The dot's vertical place, `-1.0` (top rim) to `1.0` (bottom rim).
    pub y: f32,
    /// The numbers: "Azimuth 123° · Elevation 45°".
    pub readout: String,
}

/// The dot's place on the unit dial for a light at `azimuth_deg` and `elevation_deg`.
///
/// Azimuth `0` is the top, `90` the right. A value that is not a number puts the dot in the
/// centre; an elevation outside `0..=90` stays on the dial.
#[must_use]
pub fn dial_point(azimuth_deg: f32, elevation_deg: f32) -> (f32, f32) {
    if !azimuth_deg.is_finite() || !elevation_deg.is_finite() {
        return (0.0, 0.0);
    }
    let radius = ((CENTRE_ELEVATION_DEG - elevation_deg)
        / (CENTRE_ELEVATION_DEG - RIM_ELEVATION_DEG))
        .clamp(0.0, 1.0);
    let angle = azimuth_deg.to_radians();
    (radius * angle.sin(), -radius * angle.cos())
}

/// The numbers under the dial. The azimuth is shown wrapped into `0..360`, the elevation
/// rounded to whole degrees.
#[must_use]
pub fn readout(azimuth_deg: f32, elevation_deg: f32) -> String {
    let azimuth = if azimuth_deg.is_finite() {
        azimuth_deg.rem_euclid(360.0).round() % 360.0
    } else {
        0.0
    };
    let elevation = if elevation_deg.is_finite() {
        elevation_deg.round()
    } else {
        0.0
    };
    format!("Azimuth {azimuth:.0}\u{b0} \u{b7} Elevation {elevation:.0}\u{b0}")
}

/// The indicator for the light the settings hold now.
#[must_use]
pub fn light_dial(state: &AppState) -> LightDial {
    let values = lighting_view(state).values;
    let (x, y) = dial_point(values.azimuth_deg, values.elevation_deg);
    LightDial {
        x,
        y,
        readout: readout(values.azimuth_deg, values.elevation_deg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5
    }

    #[test]
    fn the_azimuth_runs_clockwise_from_the_top_at_the_horizon() {
        assert!(close(dial_point(0.0, 0.0), (0.0, -1.0)));
        assert!(close(dial_point(90.0, 0.0), (1.0, 0.0)));
        assert!(close(dial_point(180.0, 0.0), (0.0, 1.0)));
        assert!(close(dial_point(270.0, 0.0), (-1.0, 0.0)));
    }

    #[test]
    fn the_elevation_is_the_distance_from_the_centre() {
        assert!(close(dial_point(123.0, 90.0), (0.0, 0.0)));
        let (x, y) = dial_point(0.0, 45.0);
        assert!(x.abs() < 1e-6 && (y + 0.5).abs() < 1e-6, "{x} {y}");
        let outer = dial_point(48.0, 9.0);
        let inner = dial_point(48.0, 72.0);
        assert!(outer.0.hypot(outer.1) > inner.0.hypot(inner.1));
    }

    #[test]
    fn the_dot_never_leaves_the_dial() {
        for azimuth in [-720.0_f32, -1.0, 0.0, 359.9, 361.0, 1e6] {
            for elevation in [-30.0_f32, 0.0, 9.0, 88.0, 90.0, 400.0] {
                let (x, y) = dial_point(azimuth, elevation);
                assert!(x.hypot(y) <= 1.0 + 1e-5, "{azimuth} {elevation}");
            }
        }
    }

    #[test]
    fn a_light_that_is_not_a_number_sits_in_the_centre() {
        assert_eq!(dial_point(f32::NAN, 45.0), (0.0, 0.0));
        assert_eq!(dial_point(10.0, f32::INFINITY), (0.0, 0.0));
        assert_eq!(
            readout(f32::NAN, f32::NAN),
            "Azimuth 0\u{b0} \u{b7} Elevation 0\u{b0}"
        );
    }

    #[test]
    fn the_readout_names_both_angles_in_whole_degrees() {
        assert_eq!(
            readout(123.4, 44.6),
            "Azimuth 123\u{b0} \u{b7} Elevation 45\u{b0}"
        );
    }

    #[test]
    fn the_azimuth_wraps_in_the_readout() {
        assert_eq!(
            readout(359.6, 10.0),
            "Azimuth 0\u{b0} \u{b7} Elevation 10\u{b0}"
        );
        assert_eq!(
            readout(-90.0, 10.0),
            "Azimuth 270\u{b0} \u{b7} Elevation 10\u{b0}"
        );
    }

    #[test]
    fn the_indicator_follows_the_settings() {
        use crate::{
            design_props::{LightValues, set_light_values},
            platform_mock::MockPlatform,
        };
        let mut state = AppState::new(std::sync::Arc::new(MockPlatform::default()));
        let values = lighting_view(&state).values;
        set_light_values(
            &mut state,
            LightValues {
                azimuth_deg: 90.0,
                elevation_deg: 45.0,
                ..values
            },
        );
        let dial = light_dial(&state);
        assert!(
            (dial.x - 0.5).abs() < 1e-5 && dial.y.abs() < 1e-5,
            "{dial:?}"
        );
        assert_eq!(dial.readout, "Azimuth 90\u{b0} \u{b7} Elevation 45\u{b0}");
    }
}
