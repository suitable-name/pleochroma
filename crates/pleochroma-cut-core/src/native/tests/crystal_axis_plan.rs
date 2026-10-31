//! The optional `[crystal_axis_plan]` table of the design file: written only when the Rough
//! Planner's hand-off stored a rule, read back exactly, and undoable as one edit.

use super::fixtures::simple_design;
use crate::{
    crystal_axis::{
        CrystalAxisPlan, OrientationRule, OrientationRuleKind, PlanRuleOrigin, RuleStrength,
    },
    edit::Edit,
    native::{DesignExtras, design_from_str, design_to_string},
};

fn plan() -> CrystalAxisPlan {
    CrystalAxisPlan {
        rule: Some(OrientationRule {
            kind: OrientationRuleKind::CustomAngle {
                tilt_deg: 35.0,
                azimuth_deg: Some(120.0),
            },
            tolerance_deg: 12.0,
            strength: RuleStrength::Preferred {
                weight_per_deg: 0.5,
            },
        }),
        origin: PlanRuleOrigin::Design,
        reason: "keeps the window clean".to_owned(),
    }
}

fn text_of(design: &crate::design::Design) -> String {
    design_to_string(design, None, &DesignExtras::default()).expect("serializes")
}

#[test]
fn a_design_without_a_plan_record_writes_no_table() {
    let design = simple_design();
    let text = text_of(&design);
    assert!(!text.contains("crystal_axis_plan"), "{text}");
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.crystal_axis_plan, None);
}

#[test]
fn the_plan_record_round_trips_and_is_the_only_difference() {
    let plain = text_of(&simple_design());
    let mut design = simple_design();
    design.crystal_axis_plan = Some(plan());
    let text = text_of(&design);
    assert!(text.contains("[crystal_axis_plan]"), "{text}");
    assert!(text.contains("source = \"design\""), "{text}");

    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.crystal_axis_plan, Some(plan()));
    assert_eq!(loaded.design, design);
    // Stable under a reload.
    assert_eq!(text_of(&loaded.design), text);

    // Dropping the record writes exactly the plain file again.
    let mut cleared = loaded.design;
    cleared.crystal_axis_plan = None;
    assert_eq!(text_of(&cleared), plain);
}

#[test]
fn the_plan_record_is_one_undoable_edit() {
    let mut design = simple_design();
    let inverse = design
        .apply_edit(Edit::SetCrystalAxisPlan { plan: Some(plan()) })
        .expect("applies");
    assert_eq!(design.crystal_axis_plan, Some(plan()));
    design.apply_edit(inverse).expect("undoes");
    assert_eq!(design.crystal_axis_plan, None);
}

#[test]
fn a_blank_record_is_not_kept() {
    let mut design = simple_design();
    let blank = CrystalAxisPlan {
        rule: None,
        origin: PlanRuleOrigin::MaterialDefault,
        reason: String::new(),
    };
    design
        .apply_edit(Edit::SetCrystalAxisPlan { plan: Some(blank) })
        .expect("applies");
    assert_eq!(design.crystal_axis_plan, None);
}
