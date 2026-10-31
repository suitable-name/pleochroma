//! Tests of the hand-off's real stone choice and of the rule record it stores on the design.

use super::{
    tests::{memory_db, session_of_two_results, target_with, turned_pose},
    *,
};
use pleochroma_cut_core::{
    OrientationRule, OrientationRuleKind, RuleStrength, default_rule,
    rough_plan::{DesignPlacement, orientation::RuleSetting},
};

/// A session whose result 0 has stone 0 turned by the cutter (box pose 1) and carrying a
/// Reason.
fn session_with_a_turned_stone() -> Session {
    let mut session = session_of_two_results();
    session.overrides.set_stone_choice(
        0,
        0,
        StoneChoice {
            pose: 1,
            reason: "  for the window  ".to_owned(),
            ..StoneChoice::default()
        },
    );
    session
}

#[test]
fn the_hand_off_reads_the_cutters_own_pose() {
    let session = session_with_a_turned_stone();
    let picked = pick_stone(&session, 0, 0).expect("picked");
    assert!(picked.session_posed);
    let planned = session.run.layouts[0].stones[0].pose;
    let expected = StoneChoice {
        pose: 1,
        ..StoneChoice::default()
    }
    .pose_for(&planned);
    assert_eq!(picked.layout.stones[0].pose, expected);
    assert_ne!(
        picked.layout.stones[0].pose, planned,
        "pose 1 turns the stone"
    );
    // The same pose reaches the target (it wins over the colour wizard's choice).
    let stone = &picked.layout.stones[0];
    assert_eq!(stone_pose_for(0, 0, stone, true), expected);
    // The Reason travels with the pick.
    assert_eq!(picked.stone_reason.trim(), "for the window");

    // A stone nobody turned keeps the planner's pose and is not session-posed.
    let other = pick_stone(&session, 0, 1).expect("picked");
    assert!(!other.session_posed);
    assert_eq!(other.layout.stones[1].pose, planned);
}

#[test]
fn a_free_pose_is_the_stones_pose() {
    let mut session = session_of_two_results();
    let free = turned_pose();
    session.overrides.set_stone_choice(
        0,
        0,
        StoneChoice {
            free_pose: Some(free),
            ..StoneChoice::default()
        },
    );
    let picked = pick_stone(&session, 0, 0).expect("picked");
    assert!(picked.session_posed);
    assert_eq!(picked.layout.stones[0].pose, free);
}

#[test]
fn a_lock_or_a_reason_alone_does_not_change_the_pose() {
    let mut session = session_of_two_results();
    session.overrides.set_stone_choice(
        0,
        0,
        StoneChoice {
            locked: true,
            reason: "kept".to_owned(),
            ..StoneChoice::default()
        },
    );
    let picked = pick_stone(&session, 0, 0).expect("picked");
    assert!(!picked.session_posed);
    assert_eq!(
        picked.layout.stones[0].pose,
        session.run.layouts[0].stones[0].pose
    );
    assert_eq!(picked.stone_reason, "kept");
}

fn rules_with(plan: RuleSetting, design: Option<(i64, RuleSetting)>) -> PlanSettingsOverrides {
    let mut rules = PlanSettingsOverrides::default();
    rules.set_plan(plan);
    if let Some((entry_id, setting)) = design {
        rules.set_design(entry_id, setting);
    }
    rules
}

fn record_for(rules: PlanSettingsOverrides, reason: &str) -> Option<CrystalAxisPlan> {
    let mut target = target_with(turned_pose(), DesignPlacement::IDENTITY);
    target.picked.rules = rules;
    target.picked.stone_reason = reason.to_owned();
    let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in");
    let rough = CrystalOrientation::from_angles(20.0, 10.0, None);
    plan_record(Some(&rough), &sapphire, &target)
}

fn set(kind: OrientationRuleKind) -> RuleChoice {
    RuleChoice::Set(OrientationRule {
        kind,
        tolerance_deg: 12.0,
        strength: RuleStrength::Required,
    })
}

#[test]
fn an_untouched_plan_records_the_material_default() {
    let record = record_for(PlanSettingsOverrides::default(), "").expect("a record");
    assert_eq!(record.origin, PlanRuleOrigin::MaterialDefault);
    let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in");
    assert_eq!(record.rule, default_rule(&sapphire));
    assert_eq!(record.reason.len(), 0);
}

#[test]
fn the_plans_own_rule_and_a_designs_rule_are_told_apart() {
    let plan_rule = RuleSetting {
        choice: set(OrientationRuleKind::InTablePlane),
        reason: "plan reason".to_owned(),
    };
    let plan_only = record_for(rules_with(plan_rule.clone(), None), "").expect("a record");
    assert_eq!(plan_only.origin, PlanRuleOrigin::Plan);
    assert_eq!(
        plan_only.rule.map(|rule| rule.kind),
        Some(OrientationRuleKind::InTablePlane)
    );
    assert_eq!(plan_only.reason, "plan reason");

    let design_rule = RuleSetting {
        choice: set(OrientationRuleKind::TableAcross),
        reason: "design reason".to_owned(),
    };
    let both = record_for(rules_with(plan_rule, Some((7, design_rule))), "").expect("a record");
    assert_eq!(both.origin, PlanRuleOrigin::Design);
    assert_eq!(
        both.rule.map(|rule| rule.kind),
        Some(OrientationRuleKind::TableAcross)
    );
    assert_eq!(
        both.reason, "design reason",
        "the design's Reason wins over the plan's"
    );
}

#[test]
fn a_stone_reason_wins_and_an_ignored_axis_still_keeps_it() {
    let ignore = RuleSetting {
        choice: RuleChoice::Ignore,
        reason: String::new(),
    };
    let record = record_for(rules_with(ignore, None), "  cut for yield ").expect("a record");
    assert_eq!(record.rule, None);
    assert_eq!(record.reason, "cut for yield");
    // Ignoring the axis with no Reason anywhere leaves nothing to record.
    let silent = RuleSetting {
        choice: RuleChoice::Ignore,
        reason: String::new(),
    };
    assert_eq!(record_for(rules_with(silent, None), ""), None);
}

#[test]
fn the_hand_off_request_carries_the_record_with_the_axis() {
    let target = target_with(turned_pose(), DesignPlacement::IDENTITY);
    let rough = CrystalOrientation::from_angles(35.0, 20.0, None);
    let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in");
    let request = request_from(memory_db(), Some(&rough), Some(&sapphire), &target, None);
    assert!(request.orientation.is_some());
    assert!(request.axis_plan.is_some());
    // Without the material the rule cannot be resolved: the axis still goes, the record not.
    let request = request_from(memory_db(), Some(&rough), None, &target, None);
    assert!(request.orientation.is_some());
    assert_eq!(request.axis_plan, None);
}
