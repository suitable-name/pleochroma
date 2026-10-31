//! A rule resolved for the search: the target, the strength and the rough's crystal orientation,
//! reduced to plain `Copy` data a [`CandidateDesign`](crate::rough_plan::CandidateDesign) can
//! carry.
//!
//! # Where the angles are measured
//!
//! Everything is measured on the STONE: the rough's orientation is carried into the stone's
//! model frame by the rigid map of the pose (`placement::rough_to_stone_map`), and the deviation
//! is the angle between what the rule wants and what the stone has there (table normal = model
//! `+Y`). For a box stone the four box-symmetric poses (half turns about its own axes) keep the
//! table normal's line, so the deviation of a tilt rule is the same in all four; only a rule with
//! an azimuth target tells the poses apart, and [`ResolvedRule::assignment_poses`] names the
//! best one for each assignment (the planner records it as the stone's pose choice).
//!
//! # The factor
//!
//! A deviation `d` against the tolerance `t` gives the factor `f(d)`:
//!
//! - Preferred, `w` (a fraction) per degree: `max(0, 1 - w * max(0, d - t))` (no penalty inside
//!   the tolerance);
//! - Required: `1` inside the tolerance, `0` beyond (`0` means "not allowed": the planner's
//!   `stone_value` turns it into an infeasible stone).
//!
//! The angles use `acos`/`atan2` from `std`: results are identical run to run on one platform
//! (the planner's bit-exact portability claim covers the unruled path, which never calls them).

use glam::{DMat3, DVec3};
use pleochroma::optics::materials::GemMaterial;

use crate::{
    crystal_axis::{
        CrystalAxis, CrystalOrientation, OrientationRule, OrientationRuleKind, PrincipalDirection,
        RuleStrength, color_preset, principal_angle_to_table_deg,
    },
    rough_plan::{
        fit::StonePose,
        piece::ASSIGNMENTS,
        placement::{DesignPlacement, POSE_COUNT, pose_variant, rough_to_stone_map},
        tree::assignment_axes,
    },
};

/// Slack on the Required test: a stone exactly at the tolerance (to rounding) is inside it.
const REQUIRED_SLACK_DEG: f64 = 1e-9;

/// What a resolved rule asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleTarget {
    /// The Crystal Axis along the table normal.
    TableAcross,
    /// The Crystal Axis in the table plane.
    InPlane,
    /// The Crystal Axis at `tilt_deg` from the table normal and, with an azimuth, at
    /// `azimuth_deg` in the stone's frame.
    Custom {
        /// Tilt from the table normal, degrees.
        tilt_deg: f64,
        /// Azimuth target in the stone's model frame, degrees; `None` when only the tilt counts.
        azimuth_deg: Option<f64>,
    },
    /// The principal direction runs along the table normal (a color-named rule).
    Through(PrincipalDirection),
}

/// How hard the rule holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleHold {
    /// A penalty of `weight_per_deg` (a FRACTION: `0.005` is 0.5 percent) per degree beyond the
    /// tolerance.
    Preferred {
        /// Fraction of the stone's value lost per degree beyond the tolerance.
        weight_per_deg: f64,
    },
    /// Beyond the tolerance a stone is not allowed.
    Required,
}

/// A rule ready for the search.
///
/// Two designs with equal resolved rules are in the same rule class: the Pareto prune only lets
/// designs of one class dominate each other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedRule {
    /// What the rule asks for.
    pub target: RuleTarget,
    /// How far a stone may be off before the rule counts it as off, degrees.
    pub tolerance_deg: f64,
    /// How hard the rule holds.
    pub hold: RuleHold,
    /// The rough's crystal orientation, in the rough frame.
    pub orientation: CrystalOrientation,
    /// The design's placement, used only by a rule with an azimuth target (the identity for every
    /// other rule, so such designs share a class).
    pub placement: DesignPlacement,
}

impl ResolvedRule {
    /// Resolves `rule` against the rough's `orientation`.
    ///
    /// A color-named rule needs `material` (its preset names the principal direction that runs
    /// through the table). `None` when the rule is [`OrientationRuleKind::None`], when a
    /// color-named rule is one the material does not have, or when the rule has no effect (a
    /// Preferred rule of weight zero).
    ///
    /// The rule's Preferred weight is in percent per degree (`0.5`); the resolved one is the
    /// fraction (`0.005`).
    #[must_use]
    pub fn from_rule(
        rule: &OrientationRule,
        orientation: CrystalOrientation,
        material: &GemMaterial,
    ) -> Option<Self> {
        let target = match rule.kind {
            OrientationRuleKind::None => return None,
            OrientationRuleKind::TableAcross => RuleTarget::TableAcross,
            OrientationRuleKind::InTablePlane => RuleTarget::InPlane,
            OrientationRuleKind::CustomAngle {
                tilt_deg,
                azimuth_deg,
            } => RuleTarget::Custom {
                tilt_deg: finite_or(tilt_deg, 0.0),
                azimuth_deg: azimuth_deg.filter(|a| a.is_finite()),
            },
            OrientationRuleKind::FaceUp(id) => {
                RuleTarget::Through(color_preset(material, id)?.through_table)
            }
        };
        let hold = match rule.strength {
            RuleStrength::Required => RuleHold::Required,
            RuleStrength::Preferred { weight_per_deg } => {
                let weight = finite_or(weight_per_deg, 0.0).max(0.0) / 100.0;
                if weight <= 0.0 {
                    return None;
                }
                RuleHold::Preferred {
                    weight_per_deg: weight,
                }
            }
        };
        Some(Self {
            target,
            tolerance_deg: finite_or(rule.tolerance_deg, 0.0).clamp(0.0, 90.0),
            hold,
            orientation,
            placement: DesignPlacement::IDENTITY,
        })
    }

    /// Whether the rule's target depends on the stone's azimuth (so on its pose and on the
    /// design's placement).
    #[must_use]
    pub const fn needs_azimuth(&self) -> bool {
        matches!(
            self.target,
            RuleTarget::Custom {
                azimuth_deg: Some(_),
                ..
            }
        )
    }

    /// The rule with `placement` set when its target needs one; any other rule is returned
    /// unchanged, so designs with different placements still share a class.
    #[must_use]
    pub const fn with_placement(self, placement: DesignPlacement) -> Self {
        if self.needs_azimuth() {
            Self { placement, ..self }
        } else {
            self
        }
    }

    /// The same rule held as Required at its tolerance: the cone a stone must stay in.
    #[must_use]
    pub const fn as_required(self) -> Self {
        Self {
            hold: RuleHold::Required,
            ..self
        }
    }

    /// Whether the rule is a Preferred one (a penalty beyond the tolerance), not a Required one.
    #[must_use]
    pub const fn is_preferred(&self) -> bool {
        matches!(self.hold, RuleHold::Preferred { .. })
    }

    /// The stone's orientation in its own model frame when it sits with the caliper-frame
    /// `axes` (a pose's axes in the rough frame, `axes[1]` the table normal); `None` when the
    /// axes are not a proper rotation.
    #[must_use]
    pub fn stone_orientation_for_axes(&self, axes: &[[f64; 3]; 3]) -> Option<CrystalOrientation> {
        let rotation = rotation_of(axes, &self.placement)?;
        Some(self.orientation.transformed(rotation))
    }

    /// The angle in degrees between the stone's crystal and what the rule asks for. A stone
    /// without a Second Axis has the material's own completion of the second direction.
    #[must_use]
    pub fn deviation_for_orientation(&self, stone: &CrystalOrientation) -> f64 {
        match self.target {
            RuleTarget::TableAcross | RuleTarget::Through(PrincipalDirection::Gamma) => {
                stone.axis.tilt_deg()
            }
            RuleTarget::InPlane => 90.0 - stone.axis.tilt_deg(),
            RuleTarget::Custom {
                tilt_deg,
                azimuth_deg: None,
            } => (stone.axis.tilt_deg() - folded_tilt(tilt_deg)).abs(),
            RuleTarget::Custom {
                tilt_deg,
                azimuth_deg: Some(azimuth),
            } => {
                let wanted = CrystalAxis::from_angles(tilt_deg, azimuth).direction();
                line_angle_deg(stone.axis.direction(), wanted)
            }
            RuleTarget::Through(direction) => principal_angle_to_table_deg(stone, direction),
        }
    }

    /// The table normal's angle to the rough's Crystal Axis line, degrees (the stone's tilt).
    fn table_tilt_deg(&self, table_normal: [f64; 3]) -> f64 {
        let dot = self
            .orientation
            .axis
            .direction()
            .dot(DVec3::from_array(table_normal))
            .abs()
            .min(1.0);
        dot.acos().to_degrees()
    }

    /// The deviation of a stone placed with caliper-frame `axes`: the tilt rules by a dot
    /// product, the others through the full orientation of the stone. `None` when the axes are
    /// not a proper rotation (only the rules that need the full orientation can tell).
    #[must_use]
    pub fn deviation_for_axes(&self, axes: &[[f64; 3]; 3]) -> Option<f64> {
        match self.target {
            RuleTarget::TableAcross | RuleTarget::Through(PrincipalDirection::Gamma) => {
                Some(self.table_tilt_deg(axes[1]))
            }
            RuleTarget::InPlane => Some(90.0 - self.table_tilt_deg(axes[1])),
            RuleTarget::Custom {
                tilt_deg,
                azimuth_deg: None,
            } => Some((self.table_tilt_deg(axes[1]) - folded_tilt(tilt_deg)).abs()),
            _ => self
                .stone_orientation_for_axes(axes)
                .map(|stone| self.deviation_for_orientation(&stone)),
        }
    }

    /// The deviation of a stone placed with `axes`, best over the four box-symmetric poses when
    /// the rule depends on the azimuth, with the index of that pose (`0` otherwise).
    #[must_use]
    pub fn deviation_with_pose(&self, axes: &[[f64; 3]; 3]) -> (Option<f64>, u8) {
        if !self.needs_azimuth() {
            return (self.deviation_for_axes(axes), 0);
        }
        let base = StonePose {
            center_mm: [0.0; 3],
            axes: *axes,
            mm_per_unit: 1.0,
        };
        let mut best: (Option<f64>, u8) = (None, 0);
        for index in 0..POSE_COUNT {
            let Some(variant) = pose_variant(&base, index) else {
                continue;
            };
            let deviation = self.deviation_for_axes(&variant.axes);
            let better = match (deviation, best.0) {
                (Some(new), Some(old)) => new < old,
                (Some(_), None) => true,
                _ => false,
            };
            if better {
                best = (deviation, index);
            }
        }
        best
    }

    /// Table normals, in the rough frame, that satisfy the rule's tilt exactly: the directions the
    /// fit adds to its orientation tables when the tables' own grid has (almost) none inside a
    /// tight cone.
    ///
    /// The table normal makes the rule's target tilt with the rough's Crystal Axis line `a`: `0`
    /// for Table Across, `90` for In Plane, the folded tilt for Custom Angle, and a few rings for a
    /// color-named rule (whose through-table direction depends on the spin). A target tilt of `0`
    /// is `a` itself; any other tilt is a ring of twelve normals around `a`.
    #[must_use]
    pub(crate) fn table_directions(&self) -> Vec<[f64; 3]> {
        const RING: u32 = 12;
        let tilts: &[f64] = match self.target {
            RuleTarget::TableAcross | RuleTarget::Through(PrincipalDirection::Gamma) => &[0.0],
            RuleTarget::InPlane => &[90.0],
            RuleTarget::Custom { tilt_deg, .. } => &[folded_tilt(tilt_deg)],
            RuleTarget::Through(_) => &[0.0, 30.0, 60.0, 90.0],
        };
        let axis = self.orientation.axis.direction();
        let helper = if axis.y.abs() < 0.9 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let e1 = (helper - axis * helper.dot(axis)).normalize();
        let e2 = axis.cross(e1);
        let mut out = Vec::new();
        for &tilt in tilts {
            let (sin_t, cos_t) = tilt.to_radians().sin_cos();
            if sin_t.abs() < 1e-12 {
                out.push(axis.to_array());
                continue;
            }
            for k in 0..RING {
                let (sin_p, cos_p) =
                    (std::f64::consts::TAU * f64::from(k) / f64::from(RING)).sin_cos();
                let normal = axis * cos_t + (e1 * cos_p + e2 * sin_p) * sin_t;
                out.push(normal.normalize().to_array());
            }
        }
        out
    }

    /// The factor a deviation of `deviation` degrees gives (`1` when it could not be measured).
    #[must_use]
    pub fn factor_of(&self, deviation: Option<f64>) -> f64 {
        let Some(deviation) = deviation else {
            return 1.0;
        };
        let excess = (deviation - self.tolerance_deg).max(0.0);
        match self.hold {
            RuleHold::Required => {
                if excess > REQUIRED_SLACK_DEG {
                    0.0
                } else {
                    1.0
                }
            }
            RuleHold::Preferred { weight_per_deg } => {
                if excess <= 0.0 {
                    1.0
                } else {
                    weight_per_deg.mul_add(-excess, 1.0).max(0.0)
                }
            }
        }
    }

    /// The factor of a stone placed with `axes`.
    #[must_use]
    pub fn factor_for_axes(&self, axes: &[[f64; 3]; 3]) -> f64 {
        self.factor_of(self.deviation_with_pose(axes).0)
    }

    /// The factor of each of the six assignments of a design to a piece (the table faces rough
    /// axis `ASSIGNMENTS[o][2]`); `0` marks an assignment a Required rule does not allow.
    #[must_use]
    pub fn assignment_factors(&self) -> [f64; 6] {
        let mut out = [1.0; 6];
        for (slot, assignment) in out.iter_mut().zip(&ASSIGNMENTS) {
            *slot = self.factor_for_axes(&assignment_axes(assignment));
        }
        out
    }

    /// The deviation of each assignment's stone, degrees (`None` for an assignment the rule
    /// cannot measure).
    #[must_use]
    pub fn assignment_deviations(&self) -> [Option<f64>; 6] {
        let mut out = [None; 6];
        for (slot, assignment) in out.iter_mut().zip(&ASSIGNMENTS) {
            *slot = self.deviation_with_pose(&assignment_axes(assignment)).0;
        }
        out
    }

    /// The pose index (`0..POSE_COUNT`) that puts each assignment's stone nearest the rule's
    /// azimuth target; all `0` for a rule that does not depend on the azimuth.
    #[must_use]
    pub fn assignment_poses(&self) -> [u8; 6] {
        let mut out = [0; 6];
        for (slot, assignment) in out.iter_mut().zip(&ASSIGNMENTS) {
            *slot = self.deviation_with_pose(&assignment_axes(assignment)).1;
        }
        out
    }
}

/// `value`, or `fallback` when it is not finite.
const fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// A target tilt folded into `0..=90` (a tilt past 90 is the same line from the other side).
fn folded_tilt(tilt_deg: f64) -> f64 {
    CrystalAxis::from_angles(tilt_deg, 0.0).tilt_deg()
}

/// The angle between two lines, degrees.
fn line_angle_deg(a: DVec3, b: DVec3) -> f64 {
    a.dot(b).abs().clamp(0.0, 1.0).acos().to_degrees()
}

/// The rotation part of the rough-to-stone map of a stone with caliper-frame `axes` and the
/// design's `placement`.
pub(crate) fn rotation_of(axes: &[[f64; 3]; 3], placement: &DesignPlacement) -> Option<DMat3> {
    let pose = StonePose {
        center_mm: [0.0; 3],
        axes: *axes,
        mm_per_unit: 1.0,
    };
    rough_to_stone_map(&pose, placement).map(|map| map.rotation)
}
