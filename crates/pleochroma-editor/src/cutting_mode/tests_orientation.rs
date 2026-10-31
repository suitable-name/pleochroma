//! Tests for the orientation a cutting plan carries: the sentence, the rule and Reason of a stone
//! the Rough Planner handed over, and a custom material's own default axis.

use super::{CuttingPlan, build_plan, build_plan_with_default};
use pleochroma_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn plan_of(design: &Design) -> CuttingPlan {
    let solved = design.solve().expect("every flat tier solves");
    build_plan(design, &solved, &[]).expect("a design with tiers has a plan")
}

/// A sapphire round brilliant with its Crystal Axis set to `tilt` / `azimuth`.
fn oriented_sapphire(tilt: f64, azimuth: f64) -> Design {
    let mut design = round_brilliant();
    design.material.name = Some("Sapphire".to_owned());
    design.render_effects = pleochroma_cut_core::StoneRenderEffects {
        c_axis_override_enabled: true,
        c_axis_tilt_deg: tilt,
        c_axis_azimuth_deg: azimuth,
        ..pleochroma_cut_core::StoneRenderEffects::default()
    };
    design
}

#[test]
fn a_stone_without_an_axis_has_no_orientation_on_its_plan() {
    let plan = plan_of(&round_brilliant());
    assert!(plan.orientation.is_none());
    assert!(plan.orientation_note().is_none());
}

#[test]
fn the_plan_carries_the_orientation_with_the_rule_and_the_reason_of_a_handed_over_stone() {
    use pleochroma_cut_core::{
        CrystalAxisPlan, OrientationRule, OrientationRuleKind, PlanRuleOrigin, RuleStrength,
    };
    let mut design = oriented_sapphire(40.0, 90.0);
    let plain = plan_of(&design).orientation_note().expect("an axis is set");
    assert!(
        plain.contains("Orientation rule: Table Across (Sapphire default"),
        "{plain}"
    );
    assert!(!plain.contains("Reason:"), "{plain}");

    design.crystal_axis_plan = Some(CrystalAxisPlan {
        rule: Some(OrientationRule {
            kind: OrientationRuleKind::InTablePlane,
            tolerance_deg: 20.0,
            strength: RuleStrength::Required,
        }),
        origin: PlanRuleOrigin::Design,
        reason: "keeps the best color".to_owned(),
    });
    let note = plan_of(&design).orientation_note().expect("an axis is set");
    assert!(
        note.contains("Orientation rule: In the Table Plane (design, within 20"),
        "{note}"
    );
    assert!(note.ends_with("Reason: keeps the best color"), "{note}");
}

#[test]
fn a_custom_materials_default_axis_shows_when_the_catalogue_is_given() {
    use pleochroma::optics::materials::GemMaterial;
    let mut material = GemMaterial::sapphire();
    material.name = "Blue Fan".to_owned();
    // Its default axis is baked into the material, leaning 30 degrees.
    let tilted = pleochroma_cut_core::CrystalAxis::from_angles(30.0, 60.0).direction();
    material.c_axis = tilted.as_vec3();
    let mut design = round_brilliant();
    design.material.name = Some("Blue Fan".to_owned());
    let solved = design.solve().expect("every flat tier solves");

    let without = build_plan(&design, &solved, &[]).expect("a plan");
    assert!(without.orientation.is_none(), "the catalogue is unknown");
    let with = build_plan(&design, &solved, &[material]).expect("a plan");
    let block = with
        .orientation
        .expect("the custom material carries an axis");
    assert!((block.orientation.axis.tilt_deg() - 30.0).abs() < 0.01);
}

#[test]
fn a_stored_default_of_tilt_zero_shows_on_the_plan_and_none_changes_nothing() {
    use pleochroma::optics::materials::GemMaterial;
    use pleochroma_cut_core::CrystalOrientation;
    let mut material = GemMaterial::sapphire();
    material.name = "Blue Fan".to_owned();
    let mut design = round_brilliant();
    design.material.name = Some("Blue Fan".to_owned());
    let solved = design.solve().expect("every flat tier solves");
    let catalogue = [material];

    let plain = build_plan(&design, &solved, &catalogue).expect("a plan");
    assert!(
        plain.orientation.is_none(),
        "the baked axes show no default"
    );
    let none = build_plan_with_default(&design, &solved, &catalogue, None).expect("a plan");
    assert!(none.orientation.is_none());

    let stored = CrystalOrientation::from_angles(0.0, 0.0, None);
    let with = build_plan_with_default(&design, &solved, &catalogue, Some(stored)).expect("a plan");
    let block = with
        .orientation
        .expect("a stored default is an axis that is set");
    assert_eq!(block.orientation, stored);
}
