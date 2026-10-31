//! What the results say about a stone's orientation, and what the design gets.
//!
//! # The TRUE orientation
//!
//! A stone's crystal orientation in its own (model) frame is the rough's orientation carried
//! through the rigid map of the stone's ACTUAL pose ([`stone_orientation_in_stone_frame`]): the
//! pose the cutter chose (a box-symmetric pose, or a free pose), at the scale and placement the
//! stone really has, never the rule's target. Open in Editor and Use Color write exactly this
//! to the design, so the design shows the crystal as it sits in the stone.
//!
//! # The rows
//!
//! [`stone_orientation_row`] is the data of a stone's row: the angle between the Crystal Axis
//! and the table normal, whether the stone follows the rule (a check mark), is off it ("15 deg
//! off"), or was overridden by the cutter (with the Reason), and for a biaxial material the angle
//! of the color direction to the table.

use pleochroma::optics::materials::GemMaterial;

use super::StoneChoice;
use crate::{
    crystal_axis::{
        CrystalOrientation, OrientationRule, OrientationRuleKind, PrincipalDirection,
        principal_angle_to_table_deg,
    },
    rough_plan::{
        fit::StonePose,
        placement::{DesignPlacement, rough_to_stone_map},
    },
};

/// The stone's crystal orientation in its own model frame.
///
/// It comes from the rough's `rough` orientation and the ACTUAL pose of the stone (`pose`: the
/// chosen pose, at the stone's real scale) for a design placed by `placement`. `None` when the pose or the placement is not usable.
#[must_use]
pub fn stone_orientation_in_stone_frame(
    rough: &CrystalOrientation,
    pose: &StonePose,
    placement: &DesignPlacement,
) -> Option<CrystalOrientation> {
    let map = rough_to_stone_map(pose, placement)?;
    Some(rough.transformed(map.rotation))
}

/// How a stone stands against the plan's rule.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleStatus {
    /// No rule applies to the stone's design.
    NoRule,
    /// The stone follows the rule (within its tolerance).
    Followed,
    /// The stone is off the rule by this many degrees.
    Off {
        /// How far off, degrees.
        degrees: f64,
    },
    /// The cutter overrode the stone's pose; the Reason may be empty.
    Overridden {
        /// Why, as the cutter wrote it.
        reason: String,
    },
}

impl RuleStatus {
    /// The text of the rule column: a check mark, "15 degrees off" (with the degree sign),
    /// "overridden (reason)", or empty without a rule.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::NoRule => String::new(),
            Self::Followed => "\u{2713}".to_string(),
            Self::Off { degrees } => format!("{degrees:.0}\u{b0} off"),
            Self::Overridden { reason } if reason.is_empty() => "overridden".to_string(),
            Self::Overridden { reason } => format!("overridden ({reason})"),
        }
    }
}

/// The data of one stone's row.
#[derive(Debug, Clone, PartialEq)]
pub struct OrientationRow {
    /// The stone's true orientation in its own frame.
    pub orientation: CrystalOrientation,
    /// The angle between the Crystal Axis and the table normal, degrees `[0, 90]`.
    pub axis_to_table_deg: f64,
    /// How the stone stands against the rule.
    pub status: RuleStatus,
    /// For a biaxial material: the angle between the color direction (the principal direction
    /// the rule's color-named preset puts through the table, else alpha) and the table normal,
    /// degrees `[0, 90]`.
    pub color_direction_deg: Option<f64>,
}

/// The row of a stone.
///
/// `rough` is the rough's orientation, `pose` the stone's ACTUAL pose (see
/// [`StoneChoice::pose_for`]), `rule` the rule of its design (if any), `choice` the cutter's
/// choice for it. `None` when the pose or the placement is not usable.
#[must_use]
pub fn stone_orientation_row(
    rough: &CrystalOrientation,
    pose: &StonePose,
    placement: &DesignPlacement,
    material: &GemMaterial,
    rule: Option<&OrientationRule>,
    choice: Option<&StoneChoice>,
) -> Option<OrientationRow> {
    let orientation = stone_orientation_in_stone_frame(rough, pose, placement)?;
    let status = match (choice, rule) {
        (Some(choice), _) if choice.is_overridden() => RuleStatus::Overridden {
            reason: choice.reason.clone(),
        },
        (_, None) => RuleStatus::NoRule,
        (_, Some(rule)) => match rule.is_followed(material, &orientation) {
            None => RuleStatus::NoRule,
            Some(true) => RuleStatus::Followed,
            Some(false) => RuleStatus::Off {
                degrees: rule.deviation_deg(material, &orientation).unwrap_or(0.0),
            },
        },
    };
    let color_direction_deg = material.biaxial_delta_beta_alpha.is_some().then(|| {
        let direction = match rule.map(|rule| rule.kind) {
            Some(OrientationRuleKind::FaceUp(id)) => {
                crate::crystal_axis::color_preset(material, id)
                    .map_or(PrincipalDirection::Alpha, |preset| preset.through_table)
            }
            _ => PrincipalDirection::Alpha,
        };
        principal_angle_to_table_deg(&orientation, direction)
    });
    Some(OrientationRow {
        axis_to_table_deg: orientation.axis.tilt_deg(),
        orientation,
        status,
        color_direction_deg,
    })
}
