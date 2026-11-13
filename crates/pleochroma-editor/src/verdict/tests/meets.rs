//! Tests for the meet-point and girdle-level reasons ([`ReasonKind::NoMeetPoint`],
//! [`ReasonKind::GirdleNotLevel`]) and the snap fix ([`FixAction::SnapToMeets`]): the sentences
//! for constructed inputs, what [`gather()`] reads off two real designs, the fix as ONE undo
//! step with its question naming what it cuts away, and the shared safety rule
//! ([`judge_snap`]).
//!
//! The designs are the girdle-level fixtures of `pleochroma-cut-core`: Pentalume, whose P2, P3,
//! G2, C3 and T have no meet point and whose P3 and C3 cut into the girdle; and Cam's
//! Portuguese (PC 45.140, public domain), a meet-point design with a level girdle.

use super::{closed, out_of_order, windowing_entry};
use crate::{
    loading::design_from_asc_text,
    session::EditorSession,
    verdict::{
        CutAway, FixAction, FloatingTier, GirdleLevelFact, Level, MeetFacts, Reason, ReasonKind,
        SnapOutcome, VerdictInputs, evaluate, gather, judge_snap, plan_fix,
    },
};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{Design, Edit, History};

/// Pentalume, every mast as published, its tiers named `P1` ... `C3`, `T`.
const PENTALUME: &str =
    include_str!("../../../../pleochroma-cut-core/src/design/girdle_level/pentalume.asc");

/// Cam's Portuguese, verbatim.
const CAMS_PORTUGUESE: &str =
    include_str!("../../../../pleochroma-cut-core/src/design/girdle_level/cams_portuguese.asc");

/// Pentalume's refractive index (its `I` line).
const PENTALUME_N_D: f64 = 1.768;

/// The design `text` describes, opened the way the editor opens a bare `.asc`.
fn opened(text: &str) -> Design {
    design_from_asc_text("fixture.asc", text, None)
        .expect("the fixture parses")
        .design
}

/// Pentalume at 10 mm across.
fn pentalume() -> Design {
    let mut design = opened(PENTALUME);
    design.girdle_diameter_mm = Some(10.0);
    design
}

/// Cam's Portuguese with the table raised from 0.68812 to 0.75, so the table misses the
/// vertices the crown leaves; the nearest one (0.29 mm away at 10 mm) is beyond the snap's
/// reach (0.2 mm).
fn cams_raised_table() -> Design {
    let text = CAMS_PORTUGUESE.replace("a 0.00000 0.68812 96 n t", "a 0.00000 0.75000 96 n t");
    assert_ne!(text, CAMS_PORTUGUESE, "the table line is in the fixture");
    opened(&text)
}

/// The position of the tier called `name`.
fn position(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier is called {name}"))
}

/// The verdict's inputs for `design`, solved.
fn gathered(design: &Design, n_d: f64) -> VerdictInputs {
    let solved = design.solve().expect("every tier is pinned");
    gather(design, Some(&solved), n_d, None)
}

/// The one reason of `kind` among `reasons`, if any.
fn reason_of(reasons: &[Reason], kind: ReasonKind) -> Option<&Reason> {
    let mut found = reasons.iter().filter(|reason| reason.kind == kind);
    let first = found.next();
    assert!(found.next().is_none(), "one {kind:?} reason at most");
    first
}

fn floating(tier: usize, name: &str, miss: f64, snappable: bool) -> FloatingTier {
    FloatingTier {
        tier,
        name: name.to_string(),
        miss,
        snappable,
    }
}

/// Pentalume's tier names, by position.
fn pentalume_names() -> Vec<String> {
    ["P1", "P2", "P3", "G1", "G2", "C1", "C2", "C3", "T"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------
// The sentences, for constructed inputs
// ---------------------------------------------------------------------------------------

#[test]
fn tiers_with_no_meet_point_are_one_check_in_millimetres() {
    let mut inputs = closed();
    inputs.tier_names = pentalume_names();
    inputs.mm_per_unit = Some(5.0);
    inputs.meets = Some(MeetFacts {
        floating: vec![
            floating(1, "P2", 0.024, true),
            floating(2, "P3", 0.052, false),
            floating(8, "T", 0.166, false),
        ],
        size: 2.24,
    });
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Check);
    assert_eq!(verdict.headline, "Check 1 thing.");
    let reason = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert_eq!(reason.level, Level::Check);
    assert_eq!(
        reason.text,
        "3 tiers have no meet point: P2 (0.12 mm off), P3 (0.26 mm off) and T (0.83 mm off). \
         A cutter has to set their depth by measurement."
    );
    // "Show" selects the first of them.
    assert_eq!(reason.tier, Some(1));
    // One of them is close enough to move.
    assert_eq!(reason.fix, Some(FixAction::SnapToMeets));
    assert_eq!(
        reason.confirm.as_deref(),
        Some(
            "Move the depth of 1 tier to the nearest meet point? This changes the design; \
             check the optics afterwards."
        )
    );
}

#[test]
fn without_millimetres_a_miss_is_a_share_of_the_stones_size() {
    let mut inputs = closed();
    inputs.tier_names = pentalume_names();
    inputs.meets = Some(MeetFacts {
        floating: vec![
            floating(1, "P2", 0.024, true),
            floating(7, "C3", 0.01, true),
        ],
        size: 2.0,
    });
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert_eq!(
        reason.text,
        "2 tiers have no meet point: P2 (1.2 % of the stone's size off) and C3 (0.5 % of the \
         stone's size off). A cutter has to set their depth by measurement."
    );
    assert!(
        reason
            .confirm
            .as_deref()
            .is_some_and(|question| question.starts_with("Move the depth of 2 tiers")),
        "{:?}",
        reason.confirm
    );

    // One tier reads singular.
    inputs.meets = Some(MeetFacts {
        floating: vec![floating(8, "T", 0.02, false)],
        size: 2.0,
    });
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert_eq!(
        reason.text,
        "1 tier has no meet point: T (1.0 % of the stone's size off). A cutter has to set its \
         depth by measurement."
    );
}

#[test]
fn the_snap_is_offered_only_when_a_tier_is_close_enough_to_move() {
    let mut inputs = closed();
    inputs.meets = Some(MeetFacts {
        floating: vec![
            floating(2, "P3", 0.052, false),
            floating(8, "T", 0.166, false),
        ],
        size: 2.24,
    });
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert_eq!(reason.level, Level::Check);
    assert_eq!(reason.fix, None);
    assert_eq!(reason.confirm, None);
    // "Show" still has a tier to select.
    assert_eq!(reason.tier, Some(2));
}

#[test]
fn every_tier_meeting_or_no_closed_stone_gives_no_meet_reason() {
    let mut inputs = closed();
    inputs.meets = Some(MeetFacts {
        floating: Vec::new(),
        size: 2.0,
    });
    assert!(
        reason_of(&evaluate(&inputs).reasons, ReasonKind::NoMeetPoint).is_none(),
        "every tier meets"
    );
    inputs.meets = None;
    let verdict = evaluate(&inputs);
    assert!(reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).is_none());
    assert_eq!(verdict.level, Level::Good);
}

#[test]
fn an_unlevel_girdle_names_its_thickness_and_the_tiers_that_cut_into_it() {
    let mut inputs = closed();
    inputs.tier_names = pentalume_names();
    // Pentalume at 10 mm across, its caliper width 2.1029 in model units.
    inputs.mm_per_unit = Some(10.0 / 2.1029);
    inputs.girdle_level = Some(GirdleLevelFact {
        thickness: (0.054, 0.085),
        width: 2.13,
        cut_in: vec![(2, "P3".to_string()), (7, "C3".to_string())],
    });
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Check);
    let reason = reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).expect("a girdle reason");
    assert_eq!(reason.level, Level::Check);
    assert_eq!(
        reason.text,
        "The girdle is not level: it is 0.26 to 0.40 mm thick. P3 and C3 cut into it."
    );
    assert_eq!(reason.tier, Some(2));
    assert_eq!(reason.fix, None, "there is no fix for the girdle");
    assert_eq!(reason.confirm, None);

    // Without millimetres, a share of the width; one tier reads singular.
    inputs.mm_per_unit = None;
    inputs.girdle_level = Some(GirdleLevelFact {
        thickness: (0.054, 0.085),
        width: 2.13,
        cut_in: vec![(7, "C3".to_string())],
    });
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).expect("a girdle reason");
    assert_eq!(
        reason.text,
        "The girdle is not level: it is 2.5 to 4.0 % of the stone's width thick. C3 cuts into it."
    );

    // A tilted girdle's thinner end can come out below zero; it reads as nothing.
    inputs.girdle_level = Some(GirdleLevelFact {
        thickness: (-0.01, 0.085),
        width: 2.13,
        cut_in: vec![(7, "C3".to_string())],
    });
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).expect("a girdle reason");
    assert!(
        reason.text.contains("it is 0.0 to 4.0 %"),
        "{}",
        reason.text
    );
}

#[test]
fn the_new_reasons_follow_the_cut_order_and_come_before_the_windowing() {
    let mut inputs = closed();
    inputs.warnings = vec![out_of_order(0, 2)];
    inputs.windowing = vec![windowing_entry(5, -1.0, false, true)];
    inputs.meets = Some(MeetFacts {
        floating: vec![floating(1, "P2", 0.024, true)],
        size: 2.0,
    });
    inputs.girdle_level = Some(GirdleLevelFact {
        thickness: (0.054, 0.085),
        width: 2.13,
        cut_in: vec![(2, "P3".to_string())],
    });
    let verdict = evaluate(&inputs);
    let kinds: Vec<ReasonKind> = verdict.reasons.iter().map(|reason| reason.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ReasonKind::OutOfOrder,
            ReasonKind::NoMeetPoint,
            ReasonKind::GirdleNotLevel,
            ReasonKind::Windowing,
        ]
    );
    assert_eq!(verdict.headline, "Check 4 things.");
}

#[test]
fn the_snap_has_its_own_words() {
    let snap = FixAction::SnapToMeets;
    assert_eq!(snap.label(), "Snap to Meets");
    assert!(snap.hint().contains("nearest point"), "{}", snap.hint());
    assert_eq!(snap.confirm_heading(), "Move Depths to Meet Points?");
    assert_eq!(snap.confirm_button(), "Move");
    // Its question names what it cuts away, so it comes with the plan.
    assert!(snap.asks_after_planning());
    let remove = FixAction::RemoveVanished { tier: 0 };
    assert_eq!(remove.confirm_heading(), "Remove What Is Cut Away?");
    assert_eq!(remove.confirm_button(), "Remove");
    assert!(!remove.asks_after_planning());
}

// ---------------------------------------------------------------------------------------
// gather(): what the verdict reads off the two designs
// ---------------------------------------------------------------------------------------

#[test]
fn pentalume_gets_both_checks_with_the_right_tiers() {
    let design = pentalume();
    let inputs = gathered(&design, PENTALUME_N_D);
    assert!(
        inputs.mm_per_unit.is_some(),
        "10 mm across gives millimetres"
    );

    // P1 (cut to the centerpoint), G1 (the stone size) and C1 (the girdle width) are anchors and
    // C2 meets; P2, P3, G2, C3 and the table reach no point on the stone cut so far.
    let meets = inputs.meets.as_ref().expect("the stone closes");
    let floating: Vec<usize> = meets.floating.iter().map(|tier| tier.tier).collect();
    let expected: Vec<usize> = ["P2", "P3", "G2", "C3", "T"]
        .iter()
        .map(|name| position(&design, name))
        .collect();
    assert_eq!(floating, expected, "{:?}", meets.floating);
    // The snap reaches 0.2 mm at 10 mm: P2 (0.12 mm off), G2 (0.17 mm) and C3 (0.05 mm) can move,
    // P3 (0.26 mm) and the table (0.83 mm) cannot.
    let snappable: Vec<bool> = meets.floating.iter().map(|tier| tier.snappable).collect();
    assert_eq!(
        snappable,
        vec![true, false, true, true, false],
        "{:?} (size {})",
        meets.floating,
        meets.size
    );

    let girdle = inputs.girdle_level.as_ref().expect("P3 and C3 cut into it");
    assert_eq!(
        girdle.cut_in,
        vec![
            (position(&design, "P3"), "P3".to_string()),
            (position(&design, "C3"), "C3".to_string())
        ]
    );

    let verdict = evaluate(&inputs);
    assert_ne!(verdict.level, Level::Good);
    let meet = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert_eq!(meet.level, Level::Check);
    assert!(
        meet.text.starts_with("5 tiers have no meet point: P2 ("),
        "{}",
        meet.text
    );
    for listed in ["), P3 (", "), G2 (", "), C3 (", ") and T ("] {
        assert!(meet.text.contains(listed), "{listed} in {}", meet.text);
    }
    assert!(meet.text.contains(" mm off)"), "{}", meet.text);
    assert!(
        meet.text
            .ends_with(". A cutter has to set their depth by measurement."),
        "{}",
        meet.text
    );
    assert_eq!(meet.tier, Some(position(&design, "P2")));
    assert_eq!(meet.fix, Some(FixAction::SnapToMeets));
    assert!(
        meet.confirm
            .as_deref()
            .is_some_and(|question| question.starts_with("Move the depth of 3 tiers")),
        "{:?}",
        meet.confirm
    );

    let level = reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).expect("a girdle reason");
    assert_eq!(level.level, Level::Check);
    // 0.054 to 0.085 units at 10 mm across the 2.13 units the proportions read as the width.
    assert_eq!(
        level.text,
        "The girdle is not level: it is 0.25 to 0.40 mm thick. P3 and C3 cut into it."
    );
    assert_eq!(level.fix, None);
}

#[test]
fn a_level_meet_point_design_gets_neither_check() {
    let design = opened(CAMS_PORTUGUESE);
    let inputs = gathered(&design, 2.16);
    let meets = inputs.meets.as_ref().expect("the stone closes");
    assert!(meets.floating.is_empty(), "{:?}", meets.floating);
    assert_eq!(inputs.girdle_level, None, "the girdle is level");
    let verdict = evaluate(&inputs);
    assert!(reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).is_none());
    assert!(reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).is_none());
}

#[test]
fn a_tier_too_far_from_any_point_gets_the_check_without_the_snap() {
    let design = cams_raised_table();
    let inputs = gathered(&design, 2.16);
    let meets = inputs.meets.as_ref().expect("the stone closes");
    let table = position(&design, "t");
    assert_eq!(
        meets.floating,
        vec![FloatingTier {
            tier: table,
            name: "t".to_string(),
            miss: meets.floating.first().map_or(f64::NAN, |tier| tier.miss),
            snappable: false,
        }],
        "only the raised table floats, beyond the snap's reach (size {})",
        meets.size
    );
    // No girdle diameter: the miss is a share of the stone's size. The name `t` reads as its code.
    let verdict = evaluate(&inputs);
    let reason = reason_of(&verdict.reasons, ReasonKind::NoMeetPoint).expect("a meet reason");
    assert!(
        reason
            .text
            .starts_with("1 tier has no meet point: T (2.9 % of the stone's size off)."),
        "{}",
        reason.text
    );
    assert_eq!(reason.fix, None);
    assert_eq!(reason.confirm, None);
    assert!(reason_of(&verdict.reasons, ReasonKind::GirdleNotLevel).is_none());
}

// ---------------------------------------------------------------------------------------
// The snap fix
// ---------------------------------------------------------------------------------------

#[test]
fn snapping_to_meets_moves_floating_depths_as_one_undo_step() {
    let design = pentalume();
    let before = gathered(&design, PENTALUME_N_D);
    let floating_before: Vec<usize> = before
        .meets
        .as_ref()
        .expect("the stone closes")
        .floating
        .iter()
        .map(|tier| tier.tier)
        .collect();
    let original = design.clone();

    let plan = plan_fix(&design, &FixAction::SnapToMeets, PENTALUME_N_D)
        .expect("P2 is 0.12 mm from the culet point");
    let steps = match &plan.edit {
        Edit::Batch(steps) => steps.clone(),
        single => vec![single.clone()],
    };
    // The anchors (P1 to the centerpoint, G1 the stone size, C1 the girdle width) never move. A
    // tier that met before may float once an earlier one has moved (C2 meets a corner G2 forms),
    // so the moved tiers need not all be among the first verdict's.
    let anchors = [
        position(&design, "P1"),
        position(&design, "G1"),
        position(&design, "C1"),
    ];
    let mut moved = Vec::new();
    for step in &steps {
        let Edit::SetConstraint {
            index,
            constraint: MeetConstraint::ScaleReference(mast),
        } = step
        else {
            panic!("a snap only sets explicit depths: {step:?}");
        };
        assert!(!anchors.contains(index), "anchor tier {index} moved");
        assert!(mast.is_finite() && *mast > 0.0, "tier {index}: mast {mast}");
        moved.push(*index);
    }
    assert!(moved.contains(&position(&design, "P2")), "{moved:?}");
    assert_eq!(plan.select, moved.first().copied());
    assert!(
        plan.message.starts_with("Moved the depth of P2"),
        "{}",
        plan.message
    );
    // P2 down to the culet point takes P3's place, and C3's goes with the corner G2 and C2 now
    // meet at: both tiers are cut away completely. The question says so before anything is
    // applied, and the sentence after.
    let cut_away = "P3 (all 10 facets) and C3 (all 10 facets)";
    assert_eq!(
        plan.confirm,
        Some(format!(
            "Move the depth of {} tier{} to the nearest meet point? This changes the design; \
             check the optics afterwards. It cuts away {cut_away}.",
            moved.len(),
            if moved.len() == 1 { "" } else { "s" }
        ))
    );
    assert!(
        plan.message
            .ends_with(&format!("The new depths cut away {cut_away}.")),
        "{}",
        plan.message
    );

    let mut session = EditorSession::with_history(design, History::new());
    session
        .try_apply(plan.edit)
        .map_err(|error| error.to_string())
        .expect("the planned edit applies");
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    for (index, (now, was)) in session.design.tiers.iter().zip(&original.tiers).enumerate() {
        if !moved.contains(&index) {
            assert_eq!(now, was, "tier {index} is not part of the snap");
        }
    }
    // The verdict, recomputed from the new solve, has fewer tiers without a meet point, and P2
    // is not one of them.
    let after = gathered(&session.design, PENTALUME_N_D);
    let floating_after: Vec<usize> = after
        .meets
        .as_ref()
        .expect("the snapped stone still closes")
        .floating
        .iter()
        .map(|tier| tier.tier)
        .collect();
    assert!(
        floating_after.len() < floating_before.len(),
        "{floating_before:?} -> {floating_after:?}"
    );
    assert!(!floating_after.contains(&position(&session.design, "P2")));

    session.undo().expect("undoes");
    assert_eq!(session.design.tiers, original.tiers);
}

#[test]
fn snapping_refuses_when_no_tier_is_close_enough_or_none_floats() {
    for design in [cams_raised_table(), opened(CAMS_PORTUGUESE)] {
        let refusal = plan_fix(&design, &FixAction::SnapToMeets, 2.16)
            .expect_err("nothing is within the snap's reach");
        assert!(refusal.contains("close enough"), "{refusal}");
    }
}

// ---------------------------------------------------------------------------------------
// The shared safety rule
// ---------------------------------------------------------------------------------------

#[test]
fn the_snap_rule_reports_the_moved_tiers_and_what_they_cut_away() {
    let design = pentalume();
    let plan = plan_fix(&design, &FixAction::SnapToMeets, PENTALUME_N_D).expect("plans");
    let mut snapped = design.clone();
    snapped
        .apply_edit(plan.edit.clone())
        .expect("the planned edit applies");
    let outcome = judge_snap(&design, &snapped).expect("the snapped stone is sound");
    let moved: Vec<usize> = match &plan.edit {
        Edit::Batch(steps) => steps
            .iter()
            .filter_map(|step| match step {
                Edit::SetConstraint { index, .. } => Some(*index),
                _ => None,
            })
            .collect(),
        Edit::SetConstraint { index, .. } => vec![*index],
        other => panic!("a snap only sets explicit depths: {other:?}"),
    };
    assert_eq!(outcome.moved, moved);
    assert_eq!(
        outcome.cut_away,
        vec![
            CutAway {
                tier: position(&design, "P3"),
                lost: 10,
                total: 10,
            },
            CutAway {
                tier: position(&design, "C3"),
                lost: 10,
                total: 10,
            },
        ]
    );
    assert_eq!(
        outcome.cut_away_list(&design).as_deref(),
        Some("P3 (all 10 facets) and C3 (all 10 facets)")
    );

    // A snap that moved nothing is sound and cuts nothing away.
    let unchanged = judge_snap(&design, &design).expect("the design itself is sound");
    assert_eq!(unchanged, SnapOutcome::default());
    assert_eq!(unchanged.cut_away_list(&design), None);
}

#[test]
fn the_snap_rule_refuses_a_stone_that_breaks_or_a_design_with_other_tiers() {
    let design = pentalume();
    // No tier keeps an explicit depth: the snapped design no longer solves.
    let mut broken = design.clone();
    for tier in &mut broken.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let refusal = judge_snap(&design, &broken).expect_err("the stone breaks");
    assert!(
        refusal.sentence.starts_with(
            "Moving the depths to the meet points would break the stone: The design does not solve"
        ),
        "{refusal}"
    );
    assert!(
        refusal.sentence.ends_with("Set the depths by hand."),
        "{refusal}"
    );

    let mut shorter = design.clone();
    shorter.tiers.pop();
    let refusal = judge_snap(&design, &shorter).expect_err("not the same tiers");
    assert!(refusal.to_string().contains("same tiers"), "{refusal}");
}

#[test]
fn a_cut_away_list_names_one_facet_some_facets_and_all_of_them() {
    let design = pentalume();
    let outcome = SnapOutcome {
        moved: vec![position(&design, "P2")],
        cut_away: vec![
            CutAway {
                tier: position(&design, "P3"),
                lost: 3,
                total: 10,
            },
            CutAway {
                tier: position(&design, "C3"),
                lost: 10,
                total: 10,
            },
            CutAway {
                tier: position(&design, "T"),
                lost: 1,
                total: 1,
            },
        ],
    };
    assert_eq!(
        outcome.cut_away_list(&design).as_deref(),
        Some("P3 (3 of 10 facets), C3 (all 10 facets) and T (its facet)")
    );
}
