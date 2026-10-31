//! Orientation rules: how a material is best oriented, and how far a stone is from that.
//!
//! A rule is a default the app PROPOSES (the Verdict quotes it, the Rough Planner offers it);
//! nothing applies it silently. The per-material table is the plan's decision D3 and lives here
//! so every consumer reads one copy.
//!
//! Orientation words, with the table facing up and the Crystal Axis measured from the table
//! normal (tilt):
//!
//! - [`OrientationRuleKind::TableAcross`]: the table lies across the Crystal Axis, so the axis
//!   runs through the table (tilt 0). Looking down the table you look along it.
//! - [`OrientationRuleKind::InTablePlane`]: the Crystal Axis lies in the plane of the table
//!   (tilt 90). Looking down the table you look across it.

use pleochroma::optics::materials::{GemMaterial, OpticalCharacter};

use super::{
    CrystalAxis, CrystalOrientation,
    presets::{ColorPresetId, color_preset, principal_angle_to_table_deg},
};

/// How strongly a rule is held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleStrength {
    /// A preference: the planner charges `weight_per_deg` percent of the score for every degree
    /// beyond the tolerance (plan D4: 0.5 percent per degree).
    Preferred {
        /// Percent of the score per degree beyond the tolerance.
        weight_per_deg: f64,
    },
    /// A requirement: a pose beyond the tolerance is not used.
    Required,
}

/// What the rule asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrientationRuleKind {
    /// The material has no rule.
    None,
    /// The Crystal Axis through the table (tilt 0).
    TableAcross,
    /// The Crystal Axis in the table plane (tilt 90).
    InTablePlane,
    /// A chosen tilt, and optionally an azimuth.
    CustomAngle {
        /// Tilt from the table normal, degrees `[0, 90]`.
        tilt_deg: f64,
        /// Azimuth around the table normal, degrees; `None` leaves it free.
        azimuth_deg: Option<f64>,
    },
    /// A color-named preset (`FaceUp(BestBlueFaceUp)` ...).
    FaceUp(ColorPresetId),
}

/// A rule: what it asks for, how far a stone may be from it, how strongly it is held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientationRule {
    /// What the rule asks for.
    pub kind: OrientationRuleKind,
    /// The angle, in degrees, within which a stone counts as following the rule.
    pub tolerance_deg: f64,
    /// How strongly it is held.
    pub strength: RuleStrength,
}

/// The percent of the score per degree beyond the tolerance for a preferred rule (plan D4).
pub const DEFAULT_WEIGHT_PER_DEG: f64 = 0.5;

/// The tolerance most rules use (plan D8).
pub const DEFAULT_TOLERANCE_DEG: f64 = 15.0;

const fn preferred(kind: OrientationRuleKind, tolerance_deg: f64) -> OrientationRule {
    OrientationRule {
        kind,
        tolerance_deg,
        strength: RuleStrength::Preferred {
            weight_per_deg: DEFAULT_WEIGHT_PER_DEG,
        },
    }
}

/// The default rule of `material`, or `None` when it has none (the plan's D3 table).
///
/// | Material | Rule |
/// |---|---|
/// | Sapphire, Ruby | Table Across, 15 degrees |
/// | Emerald | Table Across, 20 degrees |
/// | Aquamarine, Benitoite | In Table Plane, 15 degrees |
/// | Tourmaline | In Table Plane, 10 degrees (the open or closed c-axis note is the cutter's) |
/// | Zircon, Synthetic Moissanite, Peridot | Table Across, 15 degrees (doubling) |
/// | Rutile | Table Across, 10 degrees |
/// | Tanzanite | Best Blue Face-Up, 15 degrees |
/// | Alexandrite | Strongest Color Change, 15 degrees |
/// | Andalusite, the Quartz family, Topaz, Chrysoberyl (Yellow) | none |
/// | an isotropic material | none |
/// | a custom pleochroic uniaxial material | Table Across, 15 degrees |
/// | a custom pleochroic biaxial material | none |
#[must_use]
pub fn default_rule(material: &GemMaterial) -> Option<OrientationRule> {
    use OrientationRuleKind::{FaceUp, InTablePlane, TableAcross};
    if material.optical_character == OpticalCharacter::Isotropic {
        return None;
    }
    let rule = match material.name.as_str() {
        "Sapphire" | "Ruby" | "Zircon" | "Synthetic Moissanite" | "Peridot" => {
            preferred(TableAcross, DEFAULT_TOLERANCE_DEG)
        }
        "Emerald" => preferred(TableAcross, 20.0),
        "Rutile" => preferred(TableAcross, 10.0),
        "Aquamarine" | "Benitoite" => preferred(InTablePlane, DEFAULT_TOLERANCE_DEG),
        "Tourmaline" => preferred(InTablePlane, 10.0),
        "Tanzanite" => preferred(FaceUp(ColorPresetId::BestBlueFaceUp), DEFAULT_TOLERANCE_DEG),
        "Alexandrite" => preferred(
            FaceUp(ColorPresetId::StrongestColorChange),
            DEFAULT_TOLERANCE_DEG,
        ),
        // No rule: cut to show the play (Andalusite), or no cutting orientation matters.
        "Andalusite"
        | "Quartz"
        | "Amethyst"
        | "Citrine"
        | "Topaz"
        | "Chrysoberyl (Yellow)"
        | "Morganite" => return None,
        // A custom material (not a built-in): the doubling rule when its absorption is
        // pleochroic and its recipe uniaxial, none otherwise. A biaxial recipe has no single
        // axis to put through the table (gamma through the table is only one of its three
        // face-up colors), so it proposes nothing.
        // (`by_name` would also match a built-in name inside a longer custom one, so compare
        // the names exactly.)
        name if !GemMaterial::all_materials()
            .iter()
            .any(|built_in| built_in.name.eq_ignore_ascii_case(name))
            && material.absorption.is_pleochroic
            && material.biaxial_delta_beta_alpha.is_none() =>
        {
            preferred(TableAcross, DEFAULT_TOLERANCE_DEG)
        }
        _ => return None,
    };
    Some(rule)
}

impl OrientationRule {
    /// A short phrase for the rule, as a sentence part ("Table Across", "In the Table Plane",
    /// "Best Blue Face-Up").
    #[must_use]
    pub fn name(&self) -> String {
        match self.kind {
            OrientationRuleKind::None => "No Rule".to_string(),
            OrientationRuleKind::TableAcross => "Table Across".to_string(),
            OrientationRuleKind::InTablePlane => "In the Table Plane".to_string(),
            OrientationRuleKind::CustomAngle { tilt_deg, .. } => {
                format!("A Tilt of {tilt_deg:.0}\u{b0}")
            }
            OrientationRuleKind::FaceUp(id) => id.label().to_string(),
        }
    }

    /// How far, in degrees, a stone oriented as `orientation` is from this rule, or `None`
    /// when the rule says nothing about it (no rule, or a color-named preset the material does
    /// not have).
    ///
    /// Table Across and In the Table Plane measure the Crystal Axis; a custom angle the angle
    /// between the two lines (or the tilt difference when it names no azimuth); a color-named
    /// preset the angle between the principal direction it wants through the table and the
    /// table normal.
    #[must_use]
    pub fn deviation_deg(
        &self,
        material: &GemMaterial,
        orientation: &CrystalOrientation,
    ) -> Option<f64> {
        let tilt = orientation.axis.tilt_deg();
        match self.kind {
            OrientationRuleKind::None => None,
            OrientationRuleKind::TableAcross => Some(tilt),
            OrientationRuleKind::InTablePlane => Some(90.0 - tilt),
            OrientationRuleKind::CustomAngle {
                tilt_deg,
                azimuth_deg: None,
            } => Some((tilt - tilt_deg).abs()),
            OrientationRuleKind::CustomAngle {
                tilt_deg,
                azimuth_deg: Some(azimuth_deg),
            } => {
                let wanted = CrystalAxis::from_angles(tilt_deg, azimuth_deg).direction();
                let along = orientation
                    .axis
                    .direction()
                    .dot(wanted)
                    .abs()
                    .clamp(0.0, 1.0);
                Some(along.acos().to_degrees())
            }
            OrientationRuleKind::FaceUp(id) => {
                let preset = color_preset(material, id)?;
                Some(principal_angle_to_table_deg(
                    orientation,
                    preset.through_table,
                ))
            }
        }
    }

    /// Whether a stone oriented as `orientation` follows the rule (within its tolerance).
    /// `None` when the rule says nothing about it.
    #[must_use]
    pub fn is_followed(
        &self,
        material: &GemMaterial,
        orientation: &CrystalOrientation,
    ) -> Option<bool> {
        self.deviation_deg(material, orientation)
            .map(|deviation| deviation <= self.tolerance_deg + 1e-9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(name: &str) -> GemMaterial {
        GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} is a built-in material"))
    }

    #[test]
    fn the_d3_table_is_encoded() {
        let rule = |name: &str| default_rule(&material(name));
        let across =
            |tolerance_deg: f64| Some(preferred(OrientationRuleKind::TableAcross, tolerance_deg));
        assert_eq!(rule("Sapphire"), across(15.0));
        assert_eq!(rule("Ruby"), across(15.0));
        assert_eq!(rule("Emerald"), across(20.0));
        assert_eq!(rule("Rutile"), across(10.0));
        assert_eq!(rule("Zircon"), across(15.0));
        assert_eq!(rule("Synthetic Moissanite"), across(15.0));
        assert_eq!(rule("Peridot"), across(15.0));
        assert_eq!(
            rule("Aquamarine"),
            Some(preferred(OrientationRuleKind::InTablePlane, 15.0))
        );
        assert_eq!(
            rule("Benitoite"),
            Some(preferred(OrientationRuleKind::InTablePlane, 15.0))
        );
        assert_eq!(
            rule("Tourmaline"),
            Some(preferred(OrientationRuleKind::InTablePlane, 10.0))
        );
        assert_eq!(
            rule("Tanzanite").map(|r| r.kind),
            Some(OrientationRuleKind::FaceUp(ColorPresetId::BestBlueFaceUp))
        );
        assert_eq!(
            rule("Alexandrite").map(|r| r.kind),
            Some(OrientationRuleKind::FaceUp(
                ColorPresetId::StrongestColorChange
            ))
        );
        for none in [
            "Andalusite",
            "Quartz",
            "Topaz",
            "Chrysoberyl (Yellow)",
            "Diamond",
        ] {
            assert_eq!(rule(none), None, "{none}");
        }
    }

    #[test]
    fn a_custom_biaxial_pleochroic_material_proposes_no_rule() {
        let mut custom = material("Tanzanite");
        custom.name = "My Violet Crystal".to_string();
        assert_eq!(default_rule(&custom), None);
        let mut uniaxial = material("Sapphire");
        uniaxial.name = "My Blue Crystal".to_string();
        uniaxial.absorption.is_pleochroic = true;
        assert_eq!(
            default_rule(&uniaxial),
            Some(preferred(
                OrientationRuleKind::TableAcross,
                DEFAULT_TOLERANCE_DEG
            ))
        );
    }

    #[test]
    fn a_preferred_rule_charges_half_a_percent_per_degree() {
        let rule = default_rule(&material("Sapphire")).expect("a rule");
        assert_eq!(
            rule.strength,
            RuleStrength::Preferred {
                weight_per_deg: 0.5
            }
        );
    }

    #[test]
    fn deviations_follow_the_words() {
        let sapphire = material("Sapphire");
        let across = default_rule(&sapphire).expect("a rule");
        let upright = CrystalOrientation::from_angles(0.0, 0.0, None);
        let leaning = CrystalOrientation::from_angles(20.0, 70.0, None);
        let flat = CrystalOrientation::from_angles(90.0, 10.0, None);
        assert_eq!(across.deviation_deg(&sapphire, &upright), Some(0.0));
        assert_eq!(across.is_followed(&sapphire, &upright), Some(true));
        assert_eq!(across.deviation_deg(&sapphire, &leaning), Some(20.0));
        assert_eq!(across.is_followed(&sapphire, &leaning), Some(false));
        assert_eq!(across.is_followed(&sapphire, &flat), Some(false));

        let tourmaline = material("Tourmaline");
        let in_plane = default_rule(&tourmaline).expect("a rule");
        assert_eq!(in_plane.deviation_deg(&tourmaline, &flat), Some(0.0));
        assert_eq!(in_plane.deviation_deg(&tourmaline, &upright), Some(90.0));
    }

    #[test]
    fn a_custom_angle_measures_the_angle_between_the_lines() {
        let sapphire = material("Sapphire");
        let rule = OrientationRule {
            kind: OrientationRuleKind::CustomAngle {
                tilt_deg: 40.0,
                azimuth_deg: Some(30.0),
            },
            tolerance_deg: 5.0,
            strength: RuleStrength::Required,
        };
        let same = CrystalOrientation::from_angles(40.0, 30.0, None);
        assert!(rule.deviation_deg(&sapphire, &same).expect("a figure") < 1e-6);
        let tilted = CrystalOrientation::from_angles(0.0, 0.0, None);
        let off = rule.deviation_deg(&sapphire, &tilted).expect("a figure");
        assert!((off - 40.0).abs() < 1e-6, "{off}");
        let free = OrientationRule {
            kind: OrientationRuleKind::CustomAngle {
                tilt_deg: 40.0,
                azimuth_deg: None,
            },
            ..rule
        };
        assert_eq!(free.deviation_deg(&sapphire, &tilted), Some(40.0));
    }

    #[test]
    fn tanzanites_rule_measures_the_alpha_direction_against_the_table_normal() {
        let tanzanite = material("Tanzanite");
        let rule = default_rule(&tanzanite).expect("a rule");
        let blue = color_preset(&tanzanite, ColorPresetId::BestBlueFaceUp).expect("a preset");
        let deviation = rule
            .deviation_deg(&tanzanite, &blue.orientation)
            .expect("a figure");
        assert!(deviation < 0.1, "{deviation}");
        let violet = color_preset(&tanzanite, ColorPresetId::VioletFaceUp).expect("a preset");
        let far = rule
            .deviation_deg(&tanzanite, &violet.orientation)
            .expect("a figure");
        assert!(far > 80.0, "{far}");
        assert_eq!(
            rule.is_followed(&tanzanite, &violet.orientation),
            Some(false)
        );
    }

    #[test]
    fn a_rule_the_material_lacks_says_nothing() {
        let sapphire = material("Sapphire");
        let rule = OrientationRule {
            kind: OrientationRuleKind::FaceUp(ColorPresetId::BestBlueFaceUp),
            tolerance_deg: 15.0,
            strength: RuleStrength::Required,
        };
        let any = CrystalOrientation::from_angles(0.0, 0.0, None);
        assert_eq!(rule.deviation_deg(&sapphire, &any), None);
        assert_eq!(rule.is_followed(&sapphire, &any), None);
    }
}
