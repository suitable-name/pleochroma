//! Deep Solve availability gates, the status text and the per-tier row text.

use super::*;
use crate::EditorSession;
use std::time::Duration;

const FLAT_HINT: &str = "Slow: runs locally off the UI thread and can be cancelled. \
                         Verification never edits constraints.";

fn printed() -> ExternalProportions {
    ExternalProportions {
        vol_w3: Some(1.2),
        lw: Some(1.0),
        ..ExternalProportions::default()
    }
}

/// The concave fixture with its first pavilion tier following a meet, so Deep Solve has
/// something to repair. A design built from an `.asc` schedule has every tier pinned.
fn concave_with_a_meet_tier() -> Design {
    let mut design = Design::concave_fixture();
    design.tiers[1].constraint = MeetConstraint::MeetExisting;
    design
}

fn flat_with_a_meet_tier() -> Design {
    let mut design = concave_with_a_meet_tier();
    design.concave_tiers.clear();
    design
}

fn report(accepted: bool) -> VerifiedSolveReport {
    VerifiedSolveReport {
        initial_score: 0.2,
        score_after_calibration: 0.1,
        final_score: 0.01,
        accepted,
        overrides_applied: 1,
        anchor_moves_applied: 0,
        pipeline_runs: 7,
    }
}

// --- availability ---

#[test]
fn without_printed_proportions_deep_solve_is_unavailable() {
    let (available, hint) = availability(&flat_with_a_meet_tier(), None);
    assert!(!available);
    assert!(hint.contains("printed proportions"), "{hint}");
}

#[test]
fn proportions_that_are_all_absent_or_not_positive_are_refused() {
    let design = flat_with_a_meet_tier();
    for targets in [
        ExternalProportions::default(),
        ExternalProportions {
            vol_w3: Some(0.0),
            ..ExternalProportions::default()
        },
        ExternalProportions {
            lw: Some(-1.0),
            ..printed()
        },
        ExternalProportions {
            hw: Some(f64::NAN),
            ..printed()
        },
        ExternalProportions {
            cw: Some(f64::INFINITY),
            ..printed()
        },
    ] {
        let (available, hint) = availability(&design, Some(&targets));
        assert!(!available);
        assert!(hint.contains("finite positive"), "{hint}");
    }
}

#[test]
fn a_design_without_an_index_gear_or_tiers_is_refused() {
    let mut no_gear = flat_with_a_meet_tier();
    no_gear.meta.gear_teeth = 0;
    let (available, hint) = availability(&no_gear, Some(&printed()));
    assert!(!available);
    assert!(hint.contains("index gear"), "{hint}");

    let mut no_tiers = flat_with_a_meet_tier();
    no_tiers.tiers.clear();
    let (available, hint) = availability(&no_tiers, Some(&printed()));
    assert!(!available);
    assert!(hint.contains("index gear"), "{hint}");
}

#[test]
fn a_design_with_every_tier_pinned_has_nothing_to_repair() {
    let mut design = flat_with_a_meet_tier();
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::ScaleReference(0.5);
    }
    let (available, hint) = availability(&design, Some(&printed()));
    assert!(!available);
    assert!(hint.contains("pinned") && hint.contains("Adopt"), "{hint}");
}

#[test]
fn a_flat_design_with_a_meet_tier_is_available_with_the_unchanged_hint() {
    let (available, hint) = availability(&flat_with_a_meet_tier(), Some(&printed()));
    assert!(available);
    assert_eq!(hint, FLAT_HINT);
}

#[test]
fn a_concave_design_is_available_and_says_it_verifies_the_flat_stone() {
    let design = concave_with_a_meet_tier();
    assert!(!design.concave_tiers.is_empty());
    let (available, hint) = availability(&design, Some(&printed()));
    assert!(available, "{hint}");
    assert!(hint.starts_with(FLAT_HINT), "{hint}");
    assert!(hint.contains("flat stone"), "{hint}");
    assert!(hint.contains("not part of printed proportions"), "{hint}");
}

#[test]
fn a_concave_design_still_meets_every_other_gate() {
    let design = concave_with_a_meet_tier();
    assert!(!availability(&design, None).0);

    let mut pinned = design.clone();
    for tier in &mut pinned.tiers {
        tier.constraint = MeetConstraint::ScaleReference(0.5);
    }
    let (available, hint) = availability(&pinned, Some(&printed()));
    assert!(!available);
    assert!(hint.contains("pinned"), "{hint}");

    let mut no_gear = design;
    no_gear.meta.gear_teeth = 0;
    assert!(!availability(&no_gear, Some(&printed())).0);
}

// --- status text ---

#[test]
fn the_report_text_without_concave_tiers_is_the_plain_text() {
    for accepted in [true, false] {
        for stale in [true, false] {
            let plain = format_deep_solve_report(&report(accepted), stale);
            assert_eq!(
                plain,
                format_deep_solve_report_for(&report(accepted), stale, 0)
            );
            assert!(!plain.contains("flat stone"), "{plain}");
        }
    }
}

#[test]
fn the_report_text_names_the_concave_tiers_that_are_not_verified() {
    let text = format_deep_solve_report_for(&report(true), false, 2);
    assert!(
        text.ends_with("(flat stone; 2 concave tier(s) are not part of the printed proportions)"),
        "{text}"
    );
    assert!(text.starts_with("Deep Solve: ACCEPTED"), "{text}");
}

#[test]
fn the_concave_note_comes_before_the_stale_note() {
    let text = format_deep_solve_report_for(&report(false), true, 1);
    let concave = text.find("(flat stone;").expect("the concave note");
    let stale = text
        .find("NOTE: the design changed")
        .expect("the stale note");
    assert!(concave < stale, "{text}");
}

// --- per-tier rows ---

fn delta(tier_index: usize, before_mast: f64, after_mast: f64) -> TierMastDelta {
    TierMastDelta {
        tier_index,
        before_mast,
        after_mast,
    }
}

#[test]
fn row_text_has_one_row_per_delta_with_the_shared_number_formats() {
    let design = flat_with_a_meet_tier();
    let rows = deep_solve_row_texts(&[delta(1, 1.0, 1.25), delta(2, 2.0, 1.9)], &design);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].tier_number, "#2");
    assert_eq!(rows[0].name, tier_name_for_row(&design, 1));
    assert_eq!(rows[0].before_mast, "1.0000");
    assert_eq!(rows[0].after_mast, "1.2500");
    assert_eq!(rows[0].delta, "+0.2500");
    assert_eq!(rows[1].tier_number, "#3");
    assert_eq!(rows[1].delta, "-0.1000");
}

#[test]
fn row_text_is_empty_for_no_deltas_and_blank_for_a_vanished_tier() {
    let design = flat_with_a_meet_tier();
    assert!(deep_solve_row_texts(&[], &design).is_empty());
    let rows = deep_solve_row_texts(&[delta(99, 1.0, 1.1)], &design);
    assert_eq!(rows[0].name, "");
    assert_eq!(rows[0].tier_number, "#100");
}

// --- Pin on a concave design ---

#[test]
fn pinning_a_flat_mast_of_a_concave_design_recomputes_its_tools() {
    let mut session = EditorSession::fresh();
    session.design = Design::concave_fixture();
    let tier = session
        .design
        .tiers
        .iter()
        .position(|tier| (tier.angle_deg + 42.0).abs() < 1e-9)
        .expect("the fixture has a -42 degree pavilion tier");
    let solved = session.design.solve().expect("the fixture solves");
    let (before, _) = session
        .design
        .concave_tools_from_solved(&solved)
        .expect("the tools resolve");
    assert!(!before.is_empty());

    let pinned = session
        .pin_tier_mast(tier, solved[tier].mast * 0.95, Duration::ZERO)
        .expect("the pin applies");
    assert!(pinned.is_some());

    let solved_after = session.design.solve().expect("the pinned design solves");
    let (after, _) = session
        .design
        .concave_tools_from_solved(&solved_after)
        .expect("the tools resolve");
    assert_eq!(after.len(), before.len());
    assert_ne!(after, before, "the tools follow the moved flat stone");
}
