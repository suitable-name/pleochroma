//! Tests of the rule settings, the stone choices and the rule class of the Pareto prune.

use std::collections::BTreeMap;

use pleochroma::optics::materials::GemMaterial;

use super::{
    PlanSettingsOverrides, ResolveContext, ResolvedRule, RuleChoice, RuleSetting, RuleSource,
    StoneChoice,
};
use crate::{
    crystal_axis::{CrystalOrientation, OrientationRule, OrientationRuleKind, RuleStrength},
    rough_plan::{CandidateDesign, pareto::pareto_front},
};

fn sapphire() -> GemMaterial {
    GemMaterial::by_name("Sapphire").expect("sapphire is a built-in material")
}

fn own_rule() -> OrientationRule {
    OrientationRule {
        kind: OrientationRuleKind::InTablePlane,
        tolerance_deg: 10.0,
        strength: RuleStrength::Required,
    }
}

fn design(entry_id: i64) -> CandidateDesign {
    CandidateDesign {
        entry_id,
        width: 1.0,
        length: 1.0,
        height: 1.0,
        volume: 1.0,
        orientation: None,
    }
}

fn up() -> CrystalOrientation {
    CrystalOrientation::from_angles(0.0, 0.0, None)
}

#[test]
fn no_rule_applies_while_the_orientation_is_unknown() {
    let settings = PlanSettingsOverrides::default();
    let material = sapphire();
    assert!(settings.plan_rule(None, &material).is_none());
    assert!(settings.rule_line(None, &material).is_none());
    // With an orientation the material's default switches on.
    let rule = settings
        .plan_rule(Some(&up()), &material)
        .expect("sapphire has a default rule");
    assert_eq!(rule.kind, OrientationRuleKind::TableAcross);
    // A material without a default has no rule even then.
    let quartz = GemMaterial::by_name("Quartz").expect("a built-in material");
    assert!(settings.plan_rule(Some(&up()), &quartz).is_none());
}

#[test]
fn the_rule_line_names_the_material_default_or_the_cutters_setting() {
    let material = sapphire();
    let mut settings = PlanSettingsOverrides::default();
    let line = settings
        .rule_line(Some(&up()), &material)
        .expect("a rule applies");
    assert_eq!(line.source, RuleSource::MaterialDefault);
    assert_eq!(
        line.text,
        "Stones are oriented to the crystal axis: Table Across (Sapphire default)"
    );

    settings.set_plan(RuleSetting {
        choice: RuleChoice::Set(own_rule()),
        reason: "cut for color".to_string(),
    });
    let line = settings
        .rule_line(Some(&up()), &material)
        .expect("a rule applies");
    assert_eq!(line.source, RuleSource::Own);
    assert!(line.text.ends_with("(your setting)"), "{}", line.text);
    assert_eq!(settings.plan_reason(), Some("cut for color"));

    settings.set_plan(RuleSetting {
        choice: RuleChoice::Ignore,
        reason: String::new(),
    });
    assert!(settings.rule_line(Some(&up()), &material).is_none());
    assert!(settings.plan_reason().is_none());
}

#[test]
fn a_design_rule_wins_over_the_plan_rule() {
    let material = sapphire();
    let mut settings = PlanSettingsOverrides::default();
    let plan = settings.plan_rule(Some(&up()), &material);
    assert_eq!(settings.design_rule(7, plan), plan);

    settings.set_design(
        7,
        RuleSetting {
            choice: RuleChoice::Set(own_rule()),
            reason: "this one lies flat".to_string(),
        },
    );
    settings.set_design(
        8,
        RuleSetting {
            choice: RuleChoice::Ignore,
            reason: String::new(),
        },
    );
    assert_eq!(settings.design_rule(7, plan), Some(own_rule()));
    assert_eq!(settings.design_rule(8, plan), None);
    assert_eq!(settings.design_rule(9, plan), plan);
    assert_eq!(settings.design_reason(7), Some("this one lies flat"));
    assert!(settings.design_reason(8).is_none());

    // The untouched default removes the entry again.
    settings.set_design(7, RuleSetting::default());
    assert!(!settings.designs.contains_key(&7));
}

#[test]
fn resolving_gives_each_design_its_rule_and_nothing_without_an_orientation() {
    let material = sapphire();
    let placements = BTreeMap::new();
    let mut settings = PlanSettingsOverrides::default();
    settings.set_design(
        2,
        RuleSetting {
            choice: RuleChoice::Ignore,
            reason: String::new(),
        },
    );
    let mut designs = vec![design(1), design(2)];

    let unknown = ResolveContext {
        rough: None,
        material: &material,
        placements: &placements,
    };
    settings.resolve_designs(&mut designs, &unknown);
    assert!(designs.iter().all(|d| d.orientation.is_none()));

    let known = ResolveContext {
        rough: Some(up()),
        ..unknown
    };
    settings.resolve_designs(&mut designs, &known);
    let first = designs[0].orientation.expect("design 1 follows the plan");
    assert!((first.tolerance_deg - 15.0).abs() < 1e-12);
    assert!(first.is_preferred());
    assert!(
        designs[1].orientation.is_none(),
        "design 2 ignores the axis"
    );
    assert_eq!(
        Some(first),
        ResolvedRule::from_rule(
            &settings
                .plan_rule(Some(&up()), &material)
                .expect("a rule applies"),
            up(),
            &material
        )
    );

    // Resolving again after the orientation went away clears the rules again.
    settings.resolve_designs(&mut designs, &unknown);
    assert!(designs.iter().all(|d| d.orientation.is_none()));
}

#[test]
fn stone_choices_that_change_nothing_are_not_kept() {
    let mut settings = PlanSettingsOverrides::default();
    assert!(settings.is_default());
    let locked = StoneChoice {
        locked: true,
        ..StoneChoice::default()
    };
    settings.set_stone_choice(0, 3, locked.clone());
    assert_eq!(settings.stone_choice(0, 3), Some(&locked));
    assert!(!settings.is_default());
    settings.set_stone_choice(0, 3, StoneChoice::default());
    assert!(settings.stone_choice(0, 3).is_none());
    assert!(settings.is_default());
}

#[test]
fn designs_of_other_rule_classes_are_never_dominated() {
    let rule = ResolvedRule::from_rule(&own_rule(), up(), &sapphire()).expect("a rule applies");
    let better = design(1);
    // Larger in every way and with another rule: it would be dominated without the class.
    let worse_other_class = CandidateDesign {
        width: 1.5,
        length: 1.5,
        height: 1.5,
        volume: 1.0,
        orientation: Some(rule),
        ..design(2)
    };
    let worse_same_class = CandidateDesign {
        orientation: None,
        ..worse_other_class
    };
    let front = pareto_front(&[better, worse_other_class]);
    assert_eq!(front.len(), 2, "a design of another rule class stays");
    let front = pareto_front(&[better, worse_same_class]);
    assert_eq!(front.len(), 1, "without rules the plain prune applies");
}
