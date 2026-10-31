use super::*;

#[test]
fn a_cached_analysis_is_reused_and_returned() {
    let design = brilliant();
    let target = selection("Sapphire").resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let inputs = CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    };
    let (first, _) = run_check(&inputs, None, &|| false).unwrap();
    let (second, _) = run_check(&inputs, Some(Arc::clone(&first)), &|| false).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn the_ladder_is_skipped_when_even_the_largest_thickening_cannot_cure_the_overall_girdle() {
    let allowance = GirdleAllowance::standard();
    let thin = |was_percent: f64, now_percent: f64| InvalidReason::GirdleTooThin {
        was_percent,
        now_percent,
    };
    // Lost over 60 % of a 4 % band: 1.5 + 0.4 stays under half of 4.0.
    assert!(!thickening_can_cure(&thin(4.0, 1.5), allowance));
    // Close enough (1.7 + 0.4 = 2.1 >= 2.0): the +10 % rung can lift it over.
    assert!(thickening_can_cure(&thin(4.0, 1.7), allowance));
    // The corner and gone cases always keep the ladder.
    assert!(thickening_can_cure(
        &InvalidReason::GirdleThinAtCorners {
            was_percent: 4.0,
            now_percent: 0.0,
        },
        allowance
    ));
    assert!(thickening_can_cure(&InvalidReason::GirdleGone, allowance));
    // A reason a thicker girdle has nothing to do with.
    assert!(!thickening_can_cure(&InvalidReason::NotClosed, allowance));
}
