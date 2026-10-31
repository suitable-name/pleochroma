//! The Crystal Axis note of the verdict (plan decision D9): when a material has an orientation
//! rule (`pleochroma_cut_core::default_rule`), the verdict says how far the stone's Crystal
//! Axis is from it.
//!
//! The note is informational only. It is a reason at [`Level::Good`], so it never changes the
//! verdict's level or headline and never costs anything in a score; it is listed after every
//! reason that does. A material with no rule, or an isotropic one, gets no note.

use super::{Level, Reason, ReasonKind, reason, sentence};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    CrystalAxis, CrystalOrientation, Design, OrientationRule, default_rule, stone_orientation,
};

/// How a stone's Crystal Axis stands against its material's orientation rule.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisFact {
    /// The material's name.
    pub material: String,
    /// The rule's name ("Table Across", "Best Blue Face-Up").
    pub rule: String,
    /// The tolerance of the rule, degrees.
    pub tolerance_deg: f64,
    /// How far the stone is from the rule, degrees.
    pub deviation_deg: f64,
    /// The angle between the Crystal Axis and the table normal, degrees.
    pub tilt_deg: f64,
}

impl AxisFact {
    /// Whether the stone follows the rule.
    #[must_use]
    pub fn is_followed(&self) -> bool {
        self.deviation_deg <= self.tolerance_deg + 1e-9
    }
}

/// The orientation `design` renders with: its own setting, else `material_default`, else the
/// material's built-in axis.
fn orientation_of(
    design: &Design,
    material: &GemMaterial,
    material_default: Option<CrystalOrientation>,
) -> CrystalOrientation {
    stone_orientation(&design.render_effects, material_default).unwrap_or_else(|| {
        CrystalOrientation {
            axis: CrystalAxis::from_direction(material.c_axis.as_dvec3()),
            second_turn_deg: None,
        }
    })
}

/// The fact for `design` in `material`, or `None` when the material has no rule.
///
/// `None` also when the rule says nothing about this stone. `material_default` is the material's own default
/// orientation (a custom material's stored one), `None` for a built-in.
#[must_use]
pub fn axis_fact(
    design: &Design,
    material: &GemMaterial,
    material_default: Option<CrystalOrientation>,
) -> Option<AxisFact> {
    let rule: OrientationRule = default_rule(material)?;
    let orientation = orientation_of(design, material, material_default);
    let deviation_deg = rule.deviation_deg(material, &orientation)?;
    Some(AxisFact {
        material: material.name.clone(),
        rule: rule.name(),
        tolerance_deg: rule.tolerance_deg,
        deviation_deg,
        tilt_deg: orientation.axis.tilt_deg(),
    })
}

/// The reason a fact gives. Always [`Level::Good`]: informational.
pub(super) fn reason_of(fact: &AxisFact) -> Reason {
    let text = if fact.is_followed() {
        format!(
            "The crystal axis leans {:.0}\u{b0} from the table normal, which follows the usual orientation for {} ({}, within {:.0}\u{b0}).",
            fact.tilt_deg, fact.material, fact.rule, fact.tolerance_deg
        )
    } else {
        format!(
            "The crystal axis is {:.0}\u{b0} from the usual orientation for {} ({}, within {:.0}\u{b0}). This is a note only: it does not change the verdict.",
            fact.deviation_deg, fact.material, fact.rule, fact.tolerance_deg
        )
    };
    reason(ReasonKind::CrystalAxis, Level::Good, sentence(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{VerdictInputs, evaluate};
    use pleochroma_cut_core::{PreformSpec, StoneRenderEffects};

    fn plain_design() -> Design {
        Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62)
    }

    fn design_with_axis(tilt_deg: f64, azimuth_deg: f64) -> Design {
        let mut design = plain_design();
        design.render_effects = StoneRenderEffects {
            c_axis_override_enabled: true,
            c_axis_tilt_deg: tilt_deg,
            c_axis_azimuth_deg: azimuth_deg,
            ..StoneRenderEffects::default()
        };
        design
    }

    fn material(name: &str) -> GemMaterial {
        GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} is a built-in material"))
    }

    #[test]
    fn a_material_with_no_rule_gets_no_note() {
        let design = design_with_axis(30.0, 0.0);
        for name in ["Diamond", "Andalusite", "Quartz", "Topaz"] {
            assert_eq!(axis_fact(&design, &material(name), None), None, "{name}");
        }
    }

    #[test]
    fn sapphire_on_the_table_normal_follows_its_rule() {
        let design = design_with_axis(5.0, 0.0);
        let fact = axis_fact(&design, &material("Sapphire"), None).expect("a rule");
        assert!(fact.is_followed());
        assert!((fact.deviation_deg - 5.0).abs() < 1e-9);
        assert_eq!(fact.rule, "Table Across");
    }

    #[test]
    fn a_stone_without_its_own_axis_is_read_from_the_material() {
        // Tourmaline's built-in axis lies along +X: in the table plane, as its rule wants.
        let mut design = plain_design();
        design.render_effects = StoneRenderEffects::default();
        let fact = axis_fact(&design, &material("Tourmaline"), None).expect("a rule");
        assert!(fact.is_followed(), "{fact:?}");
        // Sapphire's is +Y: through the table.
        let fact = axis_fact(&design, &material("Sapphire"), None).expect("a rule");
        assert!(fact.is_followed(), "{fact:?}");
    }

    #[test]
    fn the_note_never_changes_the_level_or_the_headline() {
        let base = VerdictInputs {
            solve: crate::verdict::SolveFacts::Closed,
            ..VerdictInputs::default()
        };
        let plain = evaluate(&base);
        let design = design_with_axis(80.0, 0.0);
        let fact = axis_fact(&design, &material("Sapphire"), None).expect("a rule");
        assert!(!fact.is_followed());
        let with_note = evaluate(&base.with_axis(Some(fact)));
        assert_eq!(with_note.level, plain.level);
        assert_eq!(with_note.headline, plain.headline);
        assert_eq!(with_note.reasons.len(), plain.reasons.len() + 1);
        let note = with_note.reasons.last().expect("the note is listed last");
        assert_eq!(note.kind, ReasonKind::CrystalAxis);
        assert_eq!(note.level, Level::Good);
        assert!(note.fix.is_none());
    }

    #[test]
    fn the_note_does_not_count_as_a_thing_to_check() {
        let base = VerdictInputs {
            solve: crate::verdict::SolveFacts::Closed,
            missing_table: true,
            ..VerdictInputs::default()
        };
        let plain = evaluate(&base);
        let design = design_with_axis(80.0, 0.0);
        let fact = axis_fact(&design, &material("Sapphire"), None).expect("a rule");
        let with_note = evaluate(&base.with_axis(Some(fact)));
        assert_eq!(with_note.headline, plain.headline);
        assert_eq!(with_note.level, Level::Check);
    }
}
