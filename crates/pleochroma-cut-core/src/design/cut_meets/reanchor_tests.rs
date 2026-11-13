//! Tests for [`super`]: recording the planes of each meet point, and putting the tiers back on
//! them after the angles turned.
//!
//! The expected masts come from an independent port of these rules, with the same `f32` facet
//! planes, preform, mesh tolerances, plane choice and signed miss.

use super::{
    super::tests::{CAMS_PORTUGUESE, design_from_asc},
    MeetTarget,
};
use crate::{
    design::{CutMeet, Design},
    templates::TEMPLATES,
};
use pleochroma::geometry::{meet_solver::MeetConstraint, plane::tier_is_crown_side};

/// The tiers of `design` whose cut-so-far meet is `Meets`, with the tiers they meet.
fn meetings(design: &Design) -> Vec<(usize, Vec<usize>)> {
    let solved = design.solve().expect("every tier is pinned");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    meets
        .tiers
        .iter()
        .enumerate()
        .filter_map(|(tier, meet)| match meet {
            CutMeet::Meets(tiers) => Some((tier, tiers.clone())),
            CutMeet::Anchor(_) | CutMeet::Floats { .. } => None,
        })
        .collect()
}

/// `design` with every crown and pavilion tier but the girdles, the table and the culet turned
/// one degree flatter, the depths kept.
fn turned_flatter(design: &Design) -> Design {
    let mut turned = design.clone();
    for tier in &mut turned.tiers {
        if tier.is_girdle() || tier.angle_deg == 0.0 {
            continue;
        }
        if tier_is_crown_side(tier.angle_deg) {
            tier.angle_deg -= 1.0;
        } else {
            tier.angle_deg += 1.0;
        }
    }
    turned
}

/// Cam's Portuguese records a point for each of its six meeting tiers (3, 4, 5, b, c, t), each
/// formed by three planes of tiers cut before it: tier 3 sits on two girdle faces and a tier-2
/// facet, tier 4 on a tier-2 facet and two tier-3 facets, and so on, the table on a b facet and
/// two c facets. The facet passes within the meet tolerance of the point.
#[test]
fn every_meeting_tier_records_three_planes_of_earlier_tiers() {
    let design = design_from_asc(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every imported tier is pinned");
    let targets = design.meet_targets(&solved).expect("the stone closes");
    let tiers: Vec<usize> = targets.iter().map(|target| target.tier).collect();
    assert_eq!(tiers, [2, 3, 4, 6, 7, 8]);
    let formed_by: Vec<[usize; 3]> = targets
        .iter()
        .map(|target| target.planes.map(|(tier, _)| tier))
        .collect();
    assert_eq!(
        formed_by,
        [
            [0, 0, 1],
            [1, 2, 2],
            [2, 3, 3],
            [0, 0, 5],
            [5, 6, 6],
            [6, 7, 7]
        ]
    );
    let size = design.cut_meets(&solved).expect("the stone closes").size;
    for target in &targets {
        assert!(
            target.miss.abs() <= crate::design::MEET_TOLERANCE_FRACTION * size,
            "{target:?}"
        );
        assert_ne!(target.planes[0], target.planes[1], "{target:?}");
        assert_ne!(target.planes[1], target.planes[2], "{target:?}");
    }
}

/// Turned one degree flatter with its depths kept, Cam's Portuguese floats; put back on the
/// recorded points, every tier that met meets the same tiers again. The anchors (tiers 1, 2 and
/// a) keep their depth; the six meeting tiers move to the expected masts.
#[test]
fn reanchoring_puts_the_tiers_of_a_turned_design_back_on_their_points() {
    let design = design_from_asc(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every imported tier is pinned");
    let targets = design.meet_targets(&solved).expect("the stone closes");
    let before = meetings(&design);
    assert_eq!(before.len(), 6);

    let mut turned = turned_flatter(&design);
    assert!(
        meetings(&turned).len() < before.len(),
        "the turn leaves tiers without their points"
    );
    let turned_solved = turned.solve().expect("still pinned");
    let snaps = turned
        .reanchor_meets(&targets, &turned_solved)
        .expect("every point is still formed");
    let moved: Vec<(usize, f64)> = snaps
        .iter()
        .map(|snap| (snap.tier, snap.new_mast))
        .collect();
    let expected = [
        (2, 0.683_317_26),
        (3, 0.654_427_37),
        (4, 0.650_336_36),
        (6, 0.853_145_36),
        (7, 0.828_590_82),
        (8, 0.684_755_43),
    ];
    assert_eq!(moved.len(), expected.len(), "{moved:?}");
    for ((tier, mast), (want_tier, want_mast)) in moved.iter().zip(expected) {
        assert_eq!(*tier, want_tier);
        assert!((mast - want_mast).abs() < 1e-5, "tier {tier}: {mast}");
    }
    for tier in [0, 1, 5] {
        assert_eq!(turned.tiers[tier].constraint, design.tiers[tier].constraint);
    }
    assert_eq!(meetings(&turned), before);
}

/// A design whose angles did not change is already where its targets put it: nothing moves.
#[test]
fn an_unchanged_design_moves_nothing() {
    let mut design = design_from_asc(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every imported tier is pinned");
    let targets = design.meet_targets(&solved).expect("the stone closes");
    let tiers = design.tiers.clone();
    let moves = design
        .reanchor_meets(&targets, &solved)
        .expect("the points are formed");
    assert!(moves.is_empty(), "{moves:?}");
    assert_eq!(design.tiers, tiers);
}

/// The round brilliant template's culet is a pin-prick 5e-5 above the point the mains meet in.
/// Its target records that signed miss, and after a one-degree steeper pavilion the culet sits
/// 5e-5 above the mains' new point again: still a facet, still meeting.
#[test]
fn a_pin_prick_culet_keeps_its_hair_above_the_point() {
    let spec = &TEMPLATES[0];
    let design = Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers());
    let names: Vec<&str> = design.tiers.iter().map(|tier| tier.name.as_str()).collect();
    assert_eq!(names[0], "Pavilion Main");
    assert_eq!(names[2], "Lower Girdle");
    assert_eq!(names[3], "Culet");
    let solved = design.solve().expect("the template is pinned");
    let targets = design.meet_targets(&solved).expect("the stone closes");
    let culet: &MeetTarget = targets
        .iter()
        .find(|target| target.tier == 3)
        .expect("the culet meets the mains' point");
    assert_eq!(culet.planes.map(|(tier, _)| tier), [0, 0, 0]);
    assert!((culet.miss + 5e-5).abs() < 1e-6, "{culet:?}");
    let before = meetings(&design);

    let mut steeper = design.clone();
    for tier in [0, 2] {
        steeper.tiers[tier].angle_deg -= 1.0;
    }
    let steeper_solved = steeper.solve().expect("still pinned");
    let snaps = steeper
        .reanchor_meets(&targets, &steeper_solved)
        .expect("every point is still formed");
    let moved: Vec<usize> = snaps.iter().map(|snap| snap.tier).collect();
    assert_eq!(moved, [2, 3], "the lower girdles and the culet");

    // After the steeper pavilion, the mains' point is 0.67 / cos 42 deep; the culet is a hair
    // above it.
    let apex = 0.67 / 42.0_f64.to_radians().cos();
    let MeetConstraint::ScaleReference(culet_mast) = steeper.tiers[3].constraint else {
        panic!("the culet keeps an explicit depth");
    };
    assert!(
        (culet_mast - apex + 5e-5).abs() < 1e-6,
        "culet {culet_mast}, point {apex}"
    );
    assert_eq!(meetings(&steeper), before);
}

/// A triple that names one plane twice is degenerate, and a facet the design does not have
/// cannot form a point: either gives no answer, and the design is left as it was. So does a
/// mast list that does not belong to the design.
#[test]
fn a_degenerate_triple_gives_no_answer_and_changes_nothing() {
    let mut design = design_from_asc(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every imported tier is pinned");
    let mut targets = design.meet_targets(&solved).expect("the stone closes");
    let tiers = design.tiers.clone();
    let mut turned = turned_flatter(&design);
    let turned_solved = turned.solve().expect("still pinned");
    let turned_tiers = turned.tiers.clone();

    // A good target first, so a move would have happened before the bad one is reached.
    targets.push(MeetTarget {
        tier: 8,
        planes: [(6, 0), (6, 0), (7, 0)],
        miss: 0.0,
    });
    assert_eq!(turned.reanchor_meets(&targets, &turned_solved), None);
    assert_eq!(turned.tiers, turned_tiers);

    let missing = [MeetTarget {
        tier: 3,
        planes: [(1, 0), (2, 0), (2, 99)],
        miss: 0.0,
    }];
    assert_eq!(design.reanchor_meets(&missing, &solved), None);
    assert_eq!(design.reanchor_meets(&targets[..1], &solved[..3]), None);
    assert_eq!(design.tiers, tiers);
}
