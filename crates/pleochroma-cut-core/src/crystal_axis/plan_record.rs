//! The orientation rule a stone was planned under, as its design file keeps it.
//!
//! The Rough Planner's hand-off writes it ([`CrystalAxisPlan`], the file's optional
//! `[crystal_axis_plan]` table): the rule the stone was cut under, where that rule came from and
//! the Reason of an override. The cutting sheet reads it back, so the sheet says "Table Across
//! (plan, within 15 deg)" and prints the Reason instead of quoting the material's default. A
//! design without the record keeps today's behavior, and its file stays byte for byte as it was.

use pleochroma_formats::native::design::CrystalAxisPlanTable;

use super::{
    ColorPresetId, DEFAULT_TOLERANCE_DEG, DEFAULT_WEIGHT_PER_DEG, OrientationRule,
    OrientationRuleKind, RuleStrength,
};

/// Where the rule a stone was planned under came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanRuleOrigin {
    /// The plan's own rule (the material's default or the cutter's setting for the whole plan).
    Plan,
    /// A rule the cutter set for this design.
    Design,
    /// The material's default rule, untouched.
    MaterialDefault,
}

impl PlanRuleOrigin {
    /// The word the file stores.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Design => "design",
            Self::MaterialDefault => "material_default",
        }
    }

    /// The origin a stored word names; an unknown word reads as the material's default.
    #[must_use]
    pub fn from_key(key: &str) -> Self {
        match key {
            "plan" => Self::Plan,
            "design" => Self::Design,
            _ => Self::MaterialDefault,
        }
    }
}

/// The rule a stone was planned under and the Reason of an override.
#[derive(Debug, Clone, PartialEq)]
pub struct CrystalAxisPlan {
    /// The rule; `None` when the cutter chose to ignore the axis.
    pub rule: Option<OrientationRule>,
    /// Where the rule came from.
    pub origin: PlanRuleOrigin,
    /// Why the cutter overrode the rule (a plan, design or stone Reason); empty when none.
    pub reason: String,
}

impl CrystalAxisPlan {
    /// Whether the record says nothing (no rule, nothing to explain): such a record is not kept.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.rule.is_none() && self.reason.trim().is_empty()
    }

    /// How the sheet names where the rule came from: `Sapphire default`, `plan` or `design`.
    #[must_use]
    pub fn source_text(&self, material_name: &str) -> String {
        match self.origin {
            PlanRuleOrigin::Plan => "plan".to_owned(),
            PlanRuleOrigin::Design => "design".to_owned(),
            PlanRuleOrigin::MaterialDefault => format!("{material_name} default"),
        }
    }

    /// The Reason, trimmed; `None` when there is none.
    #[must_use]
    pub fn reason_text(&self) -> Option<&str> {
        Some(self.reason.trim()).filter(|text| !text.is_empty())
    }

    /// The file table for this record.
    #[must_use]
    pub fn to_table(&self) -> CrystalAxisPlanTable {
        let (rule, tilt_deg, azimuth_deg, preset) = match self.rule.map(|rule| rule.kind) {
            None | Some(OrientationRuleKind::None) => ("none", None, None, None),
            Some(OrientationRuleKind::TableAcross) => ("table_across", None, None, None),
            Some(OrientationRuleKind::InTablePlane) => ("in_table_plane", None, None, None),
            Some(OrientationRuleKind::CustomAngle {
                tilt_deg,
                azimuth_deg,
            }) => ("custom_angle", Some(tilt_deg), azimuth_deg, None),
            Some(OrientationRuleKind::FaceUp(id)) => {
                ("face_up", None, None, Some(preset_key(id).to_owned()))
            }
        };
        let (strength, weight_per_deg) = match self.rule.map(|rule| rule.strength) {
            Some(RuleStrength::Preferred { weight_per_deg }) => ("preferred", Some(weight_per_deg)),
            Some(RuleStrength::Required) => ("required", None),
            None => ("", None),
        };
        CrystalAxisPlanTable {
            rule: rule.to_owned(),
            tilt_deg,
            azimuth_deg,
            preset,
            tolerance_deg: self.rule.map_or(0.0, |rule| rule.tolerance_deg),
            strength: strength.to_owned(),
            weight_per_deg,
            source: self.origin.key().to_owned(),
            reason: self.reason.trim().to_owned(),
            unknown: toml::Table::new(),
        }
    }

    /// The record a file table holds. A rule word this build does not know reads as "no rule" (the
    /// Reason and origin are still kept).
    #[must_use]
    pub fn from_table(table: &CrystalAxisPlanTable) -> Self {
        let kind = match table.rule.as_str() {
            "table_across" => Some(OrientationRuleKind::TableAcross),
            "in_table_plane" => Some(OrientationRuleKind::InTablePlane),
            "custom_angle" => Some(OrientationRuleKind::CustomAngle {
                tilt_deg: table.tilt_deg.unwrap_or(0.0).clamp(0.0, 90.0),
                azimuth_deg: table.azimuth_deg,
            }),
            "face_up" => table
                .preset
                .as_deref()
                .and_then(preset_from_key)
                .map(OrientationRuleKind::FaceUp),
            _ => None,
        };
        let rule = kind.map(|kind| OrientationRule {
            kind,
            tolerance_deg: if table.tolerance_deg.is_finite() && table.tolerance_deg > 0.0 {
                table.tolerance_deg.min(90.0)
            } else {
                DEFAULT_TOLERANCE_DEG
            },
            strength: if table.strength == "required" {
                RuleStrength::Required
            } else {
                RuleStrength::Preferred {
                    weight_per_deg: table.weight_per_deg.unwrap_or(DEFAULT_WEIGHT_PER_DEG),
                }
            },
        });
        Self {
            rule,
            origin: PlanRuleOrigin::from_key(&table.source),
            reason: table.reason.clone(),
        }
    }
}

/// The word the file stores for a color-named preset.
const fn preset_key(id: ColorPresetId) -> &'static str {
    match id {
        ColorPresetId::BestBlueFaceUp => "best_blue_face_up",
        ColorPresetId::VioletFaceUp => "violet_face_up",
        ColorPresetId::StrongestColorChange => "strongest_color_change",
        ColorPresetId::BestGreenFaceUp => "best_green_face_up",
    }
}

/// The preset a stored word names.
fn preset_from_key(key: &str) -> Option<ColorPresetId> {
    Some(match key {
        "best_blue_face_up" => ColorPresetId::BestBlueFaceUp,
        "violet_face_up" => ColorPresetId::VioletFaceUp,
        "strongest_color_change" => ColorPresetId::StrongestColorChange,
        "best_green_face_up" => ColorPresetId::BestGreenFaceUp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: OrientationRuleKind, strength: RuleStrength, reason: &str) -> CrystalAxisPlan {
        CrystalAxisPlan {
            rule: Some(OrientationRule {
                kind,
                tolerance_deg: 12.5,
                strength,
            }),
            origin: PlanRuleOrigin::Design,
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn every_rule_kind_round_trips_through_the_table() {
        let preferred = RuleStrength::Preferred {
            weight_per_deg: 0.75,
        };
        let kinds = [
            OrientationRuleKind::TableAcross,
            OrientationRuleKind::InTablePlane,
            OrientationRuleKind::CustomAngle {
                tilt_deg: 40.0,
                azimuth_deg: None,
            },
            OrientationRuleKind::CustomAngle {
                tilt_deg: 40.0,
                azimuth_deg: Some(120.0),
            },
            OrientationRuleKind::FaceUp(ColorPresetId::BestBlueFaceUp),
            OrientationRuleKind::FaceUp(ColorPresetId::StrongestColorChange),
        ];
        for kind in kinds {
            for strength in [preferred, RuleStrength::Required] {
                let record = plan(kind, strength, "cut for the window");
                assert_eq!(CrystalAxisPlan::from_table(&record.to_table()), record);
            }
        }
    }

    #[test]
    fn an_ignored_axis_keeps_its_reason() {
        let record = CrystalAxisPlan {
            rule: None,
            origin: PlanRuleOrigin::Plan,
            reason: "  cut for yield  ".to_owned(),
        };
        let back = CrystalAxisPlan::from_table(&record.to_table());
        assert_eq!(back.rule, None);
        assert_eq!(back.origin, PlanRuleOrigin::Plan);
        assert_eq!(back.reason_text(), Some("cut for yield"));
        assert!(!record.is_blank());
    }

    #[test]
    fn a_record_without_rule_or_reason_is_blank() {
        let record = CrystalAxisPlan {
            rule: None,
            origin: PlanRuleOrigin::MaterialDefault,
            reason: "   ".to_owned(),
        };
        assert!(record.is_blank());
    }

    #[test]
    fn the_sheet_names_the_source() {
        let mut record = plan(OrientationRuleKind::TableAcross, RuleStrength::Required, "");
        assert_eq!(record.source_text("Sapphire"), "design");
        record.origin = PlanRuleOrigin::Plan;
        assert_eq!(record.source_text("Sapphire"), "plan");
        record.origin = PlanRuleOrigin::MaterialDefault;
        assert_eq!(record.source_text("Sapphire"), "Sapphire default");
        assert_eq!(record.reason_text(), None);
    }

    #[test]
    fn unknown_words_read_as_no_rule_and_the_default_origin() {
        let table = CrystalAxisPlanTable {
            rule: "something_new".to_owned(),
            source: "elsewhere".to_owned(),
            reason: "kept".to_owned(),
            ..plan(OrientationRuleKind::TableAcross, RuleStrength::Required, "").to_table()
        };
        let record = CrystalAxisPlan::from_table(&table);
        assert_eq!(record.rule, None);
        assert_eq!(record.origin, PlanRuleOrigin::MaterialDefault);
        assert_eq!(record.reason, "kept");
    }
}
