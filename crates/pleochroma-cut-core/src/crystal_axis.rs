//! The Crystal Axis: the direction of a stone's principal crystal axis (the c axis of a
//! uniaxial stone, gamma of a biaxial one) and the optional Second Axis turn that fixes the
//! other two principal directions of a biaxial stone.
//!
//! # Convention
//!
//! Model space is the render's: `+Y` is the table normal, the stone sits on `-Y`.
//!
//! - `tilt` is the angle from `+Y`, `0..=90` degrees.
//! - `azimuth` runs from `+X` toward `+Z` (clockwise seen from above the table), `0..360`
//!   degrees. It is the same azimuth a facet index has: index `i` of a gear with `n` teeth and
//!   reference angle `r` points at `2 pi (i + r) / n` (`StandardGemCuts::index_to_azimuth`),
//!   so azimuth `0` is the direction of index `-r` (index `0` when the reference angle is
//!   `0`).
//! - `direction = (sin tilt cos azimuth, cos tilt, sin tilt sin azimuth)`.
//!
//! A principal axis is a *line*: `d` and `-d` are the same axis. The canonical form keeps the
//! upper hemisphere (`y > 0`), and on the equator (`y == 0`, tilt exactly 90) the azimuth in
//! `[0, 180)`; at tilt 0 the azimuth is meaningless and is stored as `0`.
//!
//! # Second Axis
//!
//! `second_turn_deg` rotates the alpha direction of a biaxial stone right-handed about the
//! Crystal Axis, starting from the material's own completion
//! (`alpha_axis_from_turn(gamma, 0)`). Alpha is a line too, so the turn lives in `[0, 180)`.
//! `None` leaves the material's own completion untouched, which is what every design that
//! never set a Second Axis renders with.

use glam::{DMat3, DVec3, Vec3};
use pleochroma::optics::birefringence::alpha_axis_from_turn;

use crate::design::StoneRenderEffects;

mod plan_record;
mod presets;
mod rules;
mod sentence;

pub use plan_record::{CrystalAxisPlan, PlanRuleOrigin};
pub use presets::{
    AxisPreset, ColorPreset, ColorPresetId, PrincipalDirection, color_preset, color_presets,
    orientation_through_table, principal_angle_to_table_deg, width_azimuth_deg,
};
pub use rules::{
    DEFAULT_TOLERANCE_DEG, DEFAULT_WEIGHT_PER_DEG, OrientationRule, OrientationRuleKind,
    RuleStrength, default_rule,
};
pub use sentence::{orientation_sentence, orientation_sentence_with_index};

/// Components smaller than this count as exactly zero when a direction is turned into angles
/// (the `y` of a tilt-90 axis comes out of `cos(90 deg)` as `6e-17`, not `0`).
const SNAP: f64 = 1e-12;

/// A crystal axis as a canonical line, stored as angles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrystalAxis {
    tilt_deg: f64,
    azimuth_deg: f64,
}

impl CrystalAxis {
    /// The axis at `tilt_deg` from the table normal and `azimuth_deg` around it, folded into
    /// the canonical form (see the module documentation). Any real angle is accepted (an
    /// angle that is not a number counts as `0`); the result is the same line the angles
    /// describe.
    #[must_use]
    pub fn from_angles(tilt_deg: f64, azimuth_deg: f64) -> Self {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        let mut tilt = finite(tilt_deg).rem_euclid(360.0);
        let mut azimuth = finite(azimuth_deg);
        if tilt > 180.0 {
            tilt = 360.0 - tilt;
            azimuth += 180.0;
        }
        if tilt > 90.0 {
            tilt = 180.0 - tilt;
            azimuth += 180.0;
        }
        let mut azimuth = azimuth.rem_euclid(360.0);
        if azimuth >= 360.0 {
            azimuth = 0.0;
        }
        if tilt == 0.0 {
            azimuth = 0.0;
        } else if tilt == 90.0 && azimuth >= 180.0 {
            azimuth -= 180.0;
        }
        Self {
            tilt_deg: tilt,
            azimuth_deg: azimuth,
        }
    }

    /// The canonical axis of the line through `direction` (it need not be a unit vector, and
    /// its sign does not matter). A zero or non-finite vector gives the table normal.
    #[must_use]
    pub fn from_direction(direction: DVec3) -> Self {
        let n = if direction.is_finite() && direction.length_squared() > 0.0 {
            direction.normalize()
        } else {
            DVec3::Y
        };
        let n = if n.y < 0.0 { -n } else { n };
        let y = if n.y.abs() < SNAP { 0.0 } else { n.y };
        let horizontal = n.x.hypot(n.z);
        if horizontal < SNAP {
            return Self::from_angles(0.0, 0.0);
        }
        let tilt = horizontal.atan2(y).to_degrees();
        let azimuth = n.z.atan2(n.x).to_degrees();
        Self::from_angles(tilt, azimuth)
    }

    /// Tilt from the table normal, `0..=90` degrees.
    #[must_use]
    pub const fn tilt_deg(&self) -> f64 {
        self.tilt_deg
    }

    /// Azimuth around the table normal, `0..360` degrees (`0` at tilt 0).
    #[must_use]
    pub const fn azimuth_deg(&self) -> f64 {
        self.azimuth_deg
    }

    /// The unit direction of the canonical line.
    #[must_use]
    pub fn direction(&self) -> DVec3 {
        let (sin_theta, cos_theta) = self.tilt_deg.to_radians().sin_cos();
        let (sin_phi, cos_phi) = self.azimuth_deg.to_radians().sin_cos();
        DVec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
    }

    /// The vector the angle sliders of the Parameters panel have always produced, in `f32`
    /// and *not* canonicalised: any tilt and azimuth are taken as they come, so a slider
    /// position keeps its exact vector (and the render its exact bits). It is the one
    /// formula behind the desktop's `angles_to_c_axis` and the web twin
    /// `c_axis_from_angles`; the line it points along is `CrystalAxis::from_angles(..)`'s.
    #[must_use]
    pub fn slider_direction(tilt_deg: f32, azimuth_deg: f32) -> Vec3 {
        let theta = tilt_deg.to_radians();
        let phi = azimuth_deg.to_radians();
        let (sin_theta, cos_theta) = theta.sin_cos();
        let (sin_phi, cos_phi) = phi.sin_cos();
        Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
    }
}

/// A stone's full crystal orientation: the Crystal Axis plus the optional Second Axis turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrystalOrientation {
    /// The principal axis (c of a uniaxial stone, gamma of a biaxial one).
    pub axis: CrystalAxis,
    /// Right-handed turn of the alpha direction about the axis, `[0, 180)` degrees. `None`
    /// is the material's own completion. Meaningless for uniaxial and isotropic stones.
    pub second_turn_deg: Option<f64>,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the renderer's material works in f32; a turn in degrees is far inside f32's range"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

impl CrystalOrientation {
    /// The orientation at the given angles; the turn (when finite) is wrapped into
    /// `[0, 180)` because alpha is a line.
    #[must_use]
    pub fn from_angles(tilt_deg: f64, azimuth_deg: f64, second_turn_deg: Option<f64>) -> Self {
        let second_turn_deg = second_turn_deg.filter(|t| t.is_finite()).map(|t| {
            let wrapped = t.rem_euclid(180.0);
            if wrapped >= 180.0 { 0.0 } else { wrapped }
        });
        Self {
            axis: CrystalAxis::from_angles(tilt_deg, azimuth_deg),
            second_turn_deg,
        }
    }

    /// The Crystal Axis as the renderer's `f32` unit vector (canonical sign).
    #[must_use]
    pub fn gamma(&self) -> Vec3 {
        self.axis.direction().as_vec3()
    }

    /// The Second Axis (alpha) as a unit vector perpendicular to [`Self::gamma`], or `None`
    /// when no turn is set.
    #[must_use]
    pub fn alpha(&self) -> Option<Vec3> {
        self.second_turn_deg
            .map(|turn| alpha_axis_from_turn(self.gamma(), narrow(turn)))
    }

    /// This orientation seen through `rotation` (an orthonormal matrix): the Crystal Axis
    /// line is rotated, the Second Axis with it, and the result is canonical again, its turn
    /// recomputed against the new axis so `alpha()` is the rotated alpha (as a line).
    #[must_use]
    pub fn transformed(&self, rotation: DMat3) -> Self {
        let axis = CrystalAxis::from_direction(rotation * self.axis.direction());
        let second_turn_deg = self.alpha().map(|alpha| {
            let rotated = rotation * alpha.as_dvec3();
            let new_gamma = axis.direction();
            let reference = alpha_axis_from_turn(new_gamma.as_vec3(), 0.0).as_dvec3();
            let turn = new_gamma
                .dot(reference.cross(rotated))
                .atan2(reference.dot(rotated))
                .to_degrees()
                .rem_euclid(180.0);
            if turn >= 180.0 - 1e-9 { 0.0 } else { turn }
        });
        Self {
            axis,
            second_turn_deg,
        }
    }

    /// The pair the render hands to `material_for_stone_oriented`: the Crystal Axis and the
    /// Second Axis.
    #[must_use]
    pub fn to_override(&self) -> (Vec3, Option<Vec3>) {
        (self.gamma(), self.alpha())
    }
}

/// The orientation a stone renders with: the design's own setting when its override is on,
/// else the material's default orientation, else `None` (the material's built-in axis,
/// untouched).
#[must_use]
pub fn stone_orientation(
    effects: &StoneRenderEffects,
    material_default: Option<CrystalOrientation>,
) -> Option<CrystalOrientation> {
    if effects.c_axis_override_enabled {
        Some(CrystalOrientation::from_angles(
            effects.c_axis_tilt_deg,
            effects.c_axis_azimuth_deg,
            effects.c_axis_second_azimuth_deg,
        ))
    } else {
        material_default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-9
    }

    /// Whether two unit vectors span the same line (`f32` precision).
    fn same_line(a: Vec3, b: Vec3) -> bool {
        a.dot(b).abs() > 1.0 - 1e-5
    }

    #[test]
    fn the_convention_test_vectors() {
        // Tilt 0 is the table normal for any azimuth.
        assert_eq!(CrystalAxis::from_angles(0.0, 0.0).direction(), DVec3::Y);
        assert_eq!(CrystalAxis::from_angles(0.0, 123.0).direction(), DVec3::Y);
        // Tilt 90, azimuth 0 is +X; azimuth runs toward +Z.
        assert!(close(
            CrystalAxis::from_angles(90.0, 0.0).direction(),
            DVec3::X
        ));
        assert!(close(
            CrystalAxis::from_angles(90.0, 90.0).direction(),
            DVec3::Z
        ));
        // Azimuth 270 at tilt 90 is -Z, the same line as azimuth 90.
        let minus_z = CrystalAxis::from_angles(90.0, 270.0);
        assert_eq!(minus_z, CrystalAxis::from_angles(90.0, 90.0));
        assert!(close(minus_z.direction(), DVec3::Z));
        // A generic one.
        let (s, c) = 45f64.to_radians().sin_cos();
        let (sin_phi, cos_phi) = 30f64.to_radians().sin_cos();
        assert!(close(
            CrystalAxis::from_angles(45.0, 30.0).direction(),
            DVec3::new(s * cos_phi, c, s * sin_phi)
        ));
    }

    #[test]
    fn a_line_folds_into_the_upper_hemisphere() {
        // Tilt 135 at azimuth 0 is the line through (sin135, cos135, 0) = (0.707, -0.707, 0),
        // i.e. tilt 45 at azimuth 180.
        let folded = CrystalAxis::from_angles(135.0, 0.0);
        assert!((folded.tilt_deg() - 45.0).abs() < 1e-9);
        assert!((folded.azimuth_deg() - 180.0).abs() < 1e-9);
        // The pole has no azimuth.
        assert_eq!(
            CrystalAxis::from_angles(180.0, 77.0),
            CrystalAxis::from_angles(0.0, 0.0)
        );
        // Negative and out-of-range angles describe the line they say.
        let a = CrystalAxis::from_angles(-30.0, 10.0);
        let b = CrystalAxis::from_angles(30.0, 190.0);
        assert!((a.tilt_deg() - b.tilt_deg()).abs() < 1e-9);
        assert!((a.azimuth_deg() - b.azimuth_deg()).abs() < 1e-9);
        // Not a number is zero.
        assert_eq!(
            CrystalAxis::from_angles(f64::NAN, f64::NAN),
            CrystalAxis::from_angles(0.0, 0.0)
        );
        // The equator keeps azimuths in [0, 180).
        for azimuth in [0.0, 45.0, 179.0, 180.0, 225.0, 359.0] {
            let axis = CrystalAxis::from_angles(90.0, azimuth);
            assert!(
                (0.0..180.0).contains(&axis.azimuth_deg()),
                "azimuth {azimuth}"
            );
        }
    }

    #[test]
    fn from_direction_round_trips_canonical_axes() {
        for tilt in [0.0, 10.0, 45.0, 89.0, 90.0] {
            for azimuth in [0.0, 30.0, 90.0, 179.0] {
                let axis = CrystalAxis::from_angles(tilt, azimuth);
                let back = CrystalAxis::from_direction(axis.direction());
                assert!(
                    (back.tilt_deg() - axis.tilt_deg()).abs() < 1e-9
                        && (back.azimuth_deg() - axis.azimuth_deg()).abs() < 1e-9,
                    "tilt {tilt} azimuth {azimuth}: {axis:?} -> {back:?}"
                );
            }
        }
        // Either sign of a direction is the same axis; so is a non-unit length.
        let d = DVec3::new(0.3, 0.8, -0.5);
        let (a, b) = (
            CrystalAxis::from_direction(d),
            CrystalAxis::from_direction(-d * 3.0),
        );
        assert!((a.tilt_deg() - b.tilt_deg()).abs() < 1e-9);
        assert!((a.azimuth_deg() - b.azimuth_deg()).abs() < 1e-9);
        assert_eq!(CrystalAxis::from_direction(DVec3::ZERO).tilt_deg(), 0.0);
    }

    /// The slider vector is the formula the desktop and web UIs always used, bit for bit,
    /// and is not canonicalised.
    #[test]
    fn the_slider_direction_is_the_old_formula_bit_for_bit() {
        for tilt in [0.0f32, 12.5, 45.0, 90.0] {
            for azimuth in [0.0f32, 30.0, 180.0, 270.0, 359.0] {
                let (sin_theta, cos_theta) = tilt.to_radians().sin_cos();
                let (sin_phi, cos_phi) = azimuth.to_radians().sin_cos();
                let old = Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi);
                assert_eq!(
                    CrystalAxis::slider_direction(tilt, azimuth)
                        .to_array()
                        .map(f32::to_bits),
                    old.to_array().map(f32::to_bits),
                    "tilt {tilt} azimuth {azimuth}"
                );
            }
        }
        assert_eq!(CrystalAxis::slider_direction(0.0, 77.0), Vec3::Y);
        // The sliders' tilt-90 azimuth-270 vector is -Z (not folded to +Z).
        assert!(CrystalAxis::slider_direction(90.0, 270.0).z < -0.99);
    }

    #[test]
    fn the_turn_wraps_into_a_half_turn() {
        let turn = |t: f64| {
            CrystalOrientation::from_angles(20.0, 40.0, Some(t))
                .second_turn_deg
                .expect("set")
        };
        assert!((turn(200.0) - 20.0).abs() < 1e-9);
        assert!((turn(-30.0) - 150.0).abs() < 1e-9);
        assert_eq!(turn(180.0), 0.0);
        assert_eq!(
            CrystalOrientation::from_angles(20.0, 40.0, Some(f64::NAN)).second_turn_deg,
            None
        );
        assert_eq!(
            CrystalOrientation::from_angles(20.0, 40.0, None).alpha(),
            None
        );
    }

    #[test]
    fn the_axis_and_the_second_axis_are_perpendicular() {
        let orientation = CrystalOrientation::from_angles(30.0, 120.0, Some(55.0));
        let (gamma, alpha) = orientation.to_override();
        let alpha = alpha.expect("a turn is set");
        assert!(gamma.dot(alpha).abs() < 1e-5);
        assert!((alpha.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_quarter_turn_moves_the_axis_and_the_second_axis_together() {
        let rotation = DMat3::from_rotation_y(FRAC_PI_2);
        let orientation = CrystalOrientation::from_angles(30.0, 20.0, Some(40.0));
        let moved = orientation.transformed(rotation);

        let want_gamma = (rotation * orientation.axis.direction()).as_vec3();
        assert!(same_line(moved.gamma(), want_gamma));
        let want_alpha = (rotation * orientation.alpha().expect("set").as_dvec3()).as_vec3();
        assert!(same_line(moved.alpha().expect("kept"), want_alpha));
        let turn = moved.second_turn_deg.expect("kept");
        assert!((0.0..180.0).contains(&turn));

        // Turning back restores the original (as lines, hence the canonical angles).
        let back = moved.transformed(rotation.inverse());
        assert!((back.axis.tilt_deg() - 30.0).abs() < 1e-6);
        assert!((back.axis.azimuth_deg() - 20.0).abs() < 1e-6);
        let back_turn = back.second_turn_deg.expect("kept");
        assert!((back_turn - 40.0).abs() < 1e-3, "{back_turn}");
    }

    #[test]
    fn a_half_turn_over_the_equator_flips_the_axis_back_into_the_upper_hemisphere() {
        let rotation = DMat3::from_rotation_x(std::f64::consts::PI);
        let orientation = CrystalOrientation::from_angles(0.0, 0.0, Some(25.0));
        let moved = orientation.transformed(rotation);
        assert!(moved.gamma().y > 0.99, "canonical: the upper hemisphere");
        let want = (rotation * orientation.alpha().expect("set").as_dvec3()).as_vec3();
        assert!(same_line(moved.alpha().expect("kept"), want));
    }

    #[test]
    fn transforming_without_a_second_axis_keeps_it_unset() {
        let rotation = DMat3::from_rotation_z(0.7);
        let moved = CrystalOrientation::from_angles(10.0, 80.0, None).transformed(rotation);
        assert_eq!(moved.second_turn_deg, None);
        assert_eq!(moved.alpha(), None);
    }

    #[test]
    fn the_stone_orientation_precedence() {
        let material = CrystalOrientation::from_angles(90.0, 0.0, Some(10.0));
        let own = StoneRenderEffects {
            c_axis_override_enabled: true,
            c_axis_tilt_deg: 30.0,
            c_axis_azimuth_deg: 120.0,
            c_axis_second_azimuth_deg: Some(45.0),
            ..StoneRenderEffects::default()
        };
        // The design's own setting wins over the material's default.
        assert_eq!(
            stone_orientation(&own, Some(material)),
            Some(CrystalOrientation::from_angles(30.0, 120.0, Some(45.0)))
        );
        assert_eq!(
            stone_orientation(&own, None),
            Some(CrystalOrientation::from_angles(30.0, 120.0, Some(45.0)))
        );
        // Without the override the material's default stands.
        let off = StoneRenderEffects {
            c_axis_override_enabled: false,
            ..own
        };
        assert_eq!(stone_orientation(&off, Some(material)), Some(material));
        // Neither: the material's built-in axis, untouched.
        assert_eq!(
            stone_orientation(&StoneRenderEffects::default(), None),
            None
        );
        // An override without a second azimuth leaves the Second Axis unset.
        let no_second = StoneRenderEffects {
            c_axis_second_azimuth_deg: None,
            ..own
        };
        assert_eq!(
            stone_orientation(&no_second, Some(material))
                .expect("on")
                .second_turn_deg,
            None
        );
    }
}
