//! Tests for [`super`]: the corners a tier can be cut to, the edit that cuts it to one, the
//! picks the solver receives, and how a stored pick stands after a solve.
//!
//! The design is the standard round brilliant: the girdle (tier 4), the pavilion mains (tier 5)
//! and the lower girdle (tier 6) are cut in that order, so the lower girdle is cut on a stone of
//! girdle and main facets, and every corner it can meet there is formed by those two tiers.
//!
//! The last test picks on the Standard Round Brilliant template instead, the design the New
//! Design dialog makes and the UI harness arms pick mode on.

use super::{MeetPickBlock, MeetPickLoss, MeetPickState, PickCorner, PickedFacet, TierMeetPick};
use crate::{
    design::{ConstraintTier, CutMeet, Design, ScheduleMeta, TierId, TierTarget},
    edit::Edit,
    preform::PreformSpec,
    templates::TEMPLATES,
};
use pleochroma::geometry::meet_solver::{MeetConstraint, SolvedTier, tier_instance_normals};

const GIRDLE: usize = 4;
const PAVILION_MAIN: usize = 5;
const LOWER_GIRDLE: usize = 6;
const CULET: usize = 7;

/// The standard round brilliant, every tier pinned.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// A corner of the girdle and the pavilion mains, as the lower girdle can be cut to it.
fn girdle_main_corner(design: &Design, solved: &[SolvedTier]) -> PickCorner {
    design
        .pick_corners(solved, LOWER_GIRDLE)
        .expect("the stone closes")
        .into_iter()
        .find(|corner| corner.tiers == [GIRDLE, PAVILION_MAIN])
        .expect("the girdle and the pavilion mains form a corner")
}

/// The brilliant with its lower girdle cut to a corner of the girdle and the mains, solved,
/// and that corner.
fn picked() -> (Design, Vec<SolvedTier>, PickCorner) {
    let mut design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let corner = girdle_main_corner(&design, &solved);
    let edit = design
        .meet_pick_edit(LOWER_GIRDLE, &corner)
        .expect("every tier at the corner is named");
    design.apply_edit(edit).expect("the batch applies");
    let solved = design.solve().expect("the picked design solves");
    (design, solved, corner)
}

/// How the lower girdle's pick stands.
fn state_of(design: &Design, solved: &[SolvedTier]) -> Option<MeetPickState> {
    design
        .meet_pick_states(solved)
        .into_iter()
        .find(|&(tier, _)| tier == LOWER_GIRDLE)
        .map(|(_, state)| state)
}

/// Replaces the lower girdle's pick with `pick`.
fn set_pick(design: &mut Design, pick: TierMeetPick) {
    let id = design.tier_id_at(LOWER_GIRDLE).expect("the tier has an id");
    design.tier_meet_picks.insert(id, pick);
}

/// A design without picks hands the solver none and has no pick states.
#[test]
fn a_design_without_picks_hands_the_solver_none() {
    let design = brilliant();
    assert!(design.meet_tier_inputs().iter().all(|t| t.pick.is_none()));
    let solved = design.solve().expect("every tier is pinned");
    assert!(design.meet_pick_states(&solved).is_empty());
    assert!(design.tier_meet_pick(LOWER_GIRDLE).is_none());
}

/// The solver receives the picked tier's corner, by tier position and index-wheel position,
/// and no other tier gets one.
#[test]
fn a_picked_tier_hands_its_corner_to_the_solver() {
    let (design, _, corner) = picked();
    let inputs = design.meet_tier_inputs();
    for (index, input) in inputs.iter().enumerate() {
        assert_eq!(input.pick.is_some(), index == LOWER_GIRDLE, "tier {index}");
    }
    let pick = inputs[LOWER_GIRDLE]
        .pick
        .expect("the lower girdle is picked");
    for (plane, facet) in pick.planes.iter().zip(&corner.pick.facets) {
        assert_eq!(Some(plane.tier), design.index_of_tier_id(facet.tier));
        assert_eq!(plane.index, facet.index);
        assert!(plane.tier == GIRDLE || plane.tier == PAVILION_MAIN);
    }
}

/// The pick names tiers by id, so moving a tier changes its positions in the solver's input
/// and nothing else.
#[test]
fn a_pick_follows_its_tiers_through_a_move() {
    let (mut design, _, _) = picked();
    let picking = design.tier_id_at(LOWER_GIRDLE).expect("the tier has an id");
    let stored = *design
        .tier_meet_pick(LOWER_GIRDLE)
        .expect("the tier is picked");
    design
        .apply_edit(Edit::MoveTier { from: 0, to: 7 })
        .expect("the move applies");
    let now = design
        .index_of_tier_id(picking)
        .expect("the tier is still there");
    assert_ne!(now, LOWER_GIRDLE, "the table moved past it");
    let inputs = design.meet_tier_inputs();
    let pick = inputs[now].pick.expect("the pick is still handed over");
    for (plane, facet) in pick.planes.iter().zip(&stored.facets) {
        assert_eq!(Some(plane.tier), design.index_of_tier_id(facet.tier));
        assert_eq!(plane.index, facet.index);
    }
}

/// A pick is dormant, and the solver gets none, while the tier's constraint is not a named meet
/// or its names do not cover the picked tiers; it is live again with the names.
#[test]
fn a_dormant_pick_is_not_handed_to_the_solver() {
    let (mut design, solved, corner) = picked();
    let names: Vec<String> = corner
        .tiers
        .iter()
        .map(|&tier| design.tiers[tier].name.clone())
        .collect();

    design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::ScaleReference(0.68);
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_none());
    assert_eq!(
        state_of(&design, &solved),
        Some(MeetPickState::Dormant),
        "a pinned tier"
    );

    design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetNamed(names[..1].to_vec());
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_none());
    assert_eq!(
        state_of(&design, &solved),
        Some(MeetPickState::Dormant),
        "names that leave a picked tier out"
    );

    design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetNamed(names);
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_some());
}

/// A pick that names a missing tier or facet, or three parallel planes, is not handed over.
#[test]
fn an_unusable_pick_is_not_handed_to_the_solver() {
    let (mut design, _, _) = picked();
    let stored = *design
        .tier_meet_pick(LOWER_GIRDLE)
        .expect("the tier is picked");

    let mut removed = stored;
    removed.facets[0].tier = TierId(9_999);
    set_pick(&mut design, removed);
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_none());

    let mut missing = stored;
    missing.facets[0].index = 1.5;
    set_pick(&mut design, missing);
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_none());

    set_pick(&mut design, stored);
    assert!(design.meet_tier_inputs()[LOWER_GIRDLE].pick.is_some());
}

/// The first tier cut has no corner; the lower girdle's corners are formed by the tiers cut
/// before it, each by three independent facets of tiers named in the corner, and the mast of
/// each is the depth at which the tier's deepest facet passes through the corner.
#[test]
fn pick_corners_are_the_corners_of_the_stone_cut_before() {
    let design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(design.pick_corners(&solved, GIRDLE), Some(Vec::new()));

    let corners = design
        .pick_corners(&solved, LOWER_GIRDLE)
        .expect("the stone closes");
    assert!(!corners.is_empty());
    let normals = tier_instance_normals(design.meta.gear_teeth_abs(), &design.meet_tier_inputs());
    for corner in &corners {
        assert!(!corner.tiers.is_empty());
        assert!(
            corner
                .tiers
                .iter()
                .all(|&tier| tier == GIRDLE || tier == PAVILION_MAIN),
            "{corner:?}"
        );
        assert!(corner.unnamed_tier.is_none());
        assert!(corner.mast.is_finite() && corner.mast > 0.0);
        let facet_normals: Vec<_> = corner
            .pick
            .facets
            .iter()
            .map(|facet| {
                let tier = design
                    .index_of_tier_id(facet.tier)
                    .expect("the tier exists");
                assert!(corner.tiers.contains(&tier));
                let slot = design.tiers[tier]
                    .indices
                    .iter()
                    .position(|&index| (index - facet.index).abs() < 1e-9)
                    .unwrap_or(0);
                normals[tier][slot]
            })
            .collect();
        let det = facet_normals[0].dot(facet_normals[1].cross(facet_normals[2]));
        assert!(det.abs() >= 5e-3, "{corner:?}");
        let deepest = normals[LOWER_GIRDLE]
            .iter()
            .map(|normal| normal.dot(corner.point))
            .fold(f64::MIN, f64::max);
        assert!((deepest - corner.mast).abs() < 1e-4, "{corner:?}");
    }
}

/// A tier that is not in the design has no corners.
#[test]
fn pick_corners_of_a_missing_tier_is_none() {
    let design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(design.pick_corners(&solved, 99), None);
    assert_eq!(design.pick_corners(&solved[..3], LOWER_GIRDLE), None);
}

/// A corner with an unnamed tier is offered but cannot be picked.
#[test]
fn a_corner_with_an_unnamed_tier_cannot_be_picked() {
    let mut design = brilliant();
    design.tiers[GIRDLE].name = String::new();
    let solved = design.solve().expect("every tier is pinned");
    let corners = design
        .pick_corners(&solved, LOWER_GIRDLE)
        .expect("the stone closes");
    let unnamed = corners
        .iter()
        .find(|corner| corner.unnamed_tier == Some(GIRDLE))
        .expect("a corner of the unnamed girdle");
    assert!(design.meet_pick_edit(LOWER_GIRDLE, unnamed).is_none());
}

/// The edit names every tier at the corner, stores the pick and clears a target first; solved,
/// the tier sits at the corner's mast, the pick is met, and the cut-so-far meet names the
/// corner's tiers.
#[test]
fn cutting_a_tier_to_a_corner_meets_it() {
    let mut design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let corner = girdle_main_corner(&design, &solved);

    let id = design.tier_id_at(LOWER_GIRDLE).expect("the tier has an id");
    design.tier_targets.insert(id, TierTarget::DepthMm(1.0));
    let Some(Edit::Batch(edits)) = design.meet_pick_edit(LOWER_GIRDLE, &corner) else {
        panic!("a batch is expected");
    };
    assert_eq!(
        edits.first(),
        Some(&Edit::SetTierTarget {
            index: LOWER_GIRDLE,
            target: None
        })
    );

    let (design, solved, corner) = picked();
    assert_eq!(
        design.tiers[LOWER_GIRDLE].constraint,
        MeetConstraint::MeetNamed(vec!["Girdle".to_owned(), "Pavilion Main".to_owned()])
    );
    assert_eq!(
        design.tier_meet_pick(LOWER_GIRDLE),
        Some(&corner.pick),
        "the pick is stored"
    );
    assert!(
        (solved[LOWER_GIRDLE].mast - corner.mast).abs() < 1e-5,
        "{} against {}",
        solved[LOWER_GIRDLE].mast,
        corner.mast
    );
    assert_eq!(
        design.meet_pick_states(&solved),
        [(LOWER_GIRDLE, MeetPickState::Met)]
    );
    let meets = design.cut_meets(&solved).expect("the stone closes");
    let CutMeet::Meets(tiers) = &meets.tiers[LOWER_GIRDLE] else {
        panic!("the lower girdle meets: {:?}", meets.tiers[LOWER_GIRDLE]);
    };
    assert!(corner.tiers.iter().all(|tier| tiers.contains(tier)));
}

/// The states of a pick: lost for a removed tier, a removed facet and parallel planes, off the
/// stone when the tier no longer passes through the corner, dormant when it is not in use.
#[test]
fn a_stored_pick_reports_how_it_stands() {
    let (mut design, solved, _) = picked();
    let stored = *design
        .tier_meet_pick(LOWER_GIRDLE)
        .expect("the tier is picked");
    assert_eq!(state_of(&design, &solved), Some(MeetPickState::Met));

    let mut removed = stored;
    removed.facets[0].tier = TierId(9_999);
    set_pick(&mut design, removed);
    assert_eq!(
        state_of(&design, &solved),
        Some(MeetPickState::Lost(MeetPickLoss::TierRemoved))
    );

    let mut facet_removed = stored;
    facet_removed.facets[0].index = 1.5;
    set_pick(&mut design, facet_removed);
    assert_eq!(
        state_of(&design, &solved),
        Some(MeetPickState::Lost(MeetPickLoss::FacetRemoved))
    );

    let girdle = design.tier_id_at(GIRDLE).expect("the tier has an id");
    let on_girdle = |index: f64| PickedFacet {
        tier: girdle,
        index,
    };
    set_pick(
        &mut design,
        TierMeetPick {
            facets: [on_girdle(0.0), on_girdle(6.0), on_girdle(12.0)],
        },
    );
    assert_eq!(
        state_of(&design, &solved),
        Some(MeetPickState::Lost(MeetPickLoss::PlanesParallel)),
        "girdle facets are all vertical"
    );

    set_pick(&mut design, stored);
    let mut deeper = solved;
    deeper[LOWER_GIRDLE].mast += 0.02;
    assert_eq!(
        state_of(&design, &deeper),
        Some(MeetPickState::OffStone),
        "the tier no longer passes through the corner"
    );
}

/// The first tier cut cannot take a pick, nor can a tier that alone anchors its block.
#[test]
fn some_tiers_cannot_take_a_pick() {
    let mut design = brilliant();
    assert_eq!(
        design.meet_pick_blocked(GIRDLE),
        Some(MeetPickBlock::NothingCutBefore)
    );
    assert_eq!(design.meet_pick_blocked(LOWER_GIRDLE), None);
    assert_eq!(design.meet_pick_blocked(PAVILION_MAIN), None);

    design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetExisting;
    design.tiers[CULET].constraint = MeetConstraint::MeetExisting;
    assert_eq!(
        design.meet_pick_blocked(PAVILION_MAIN),
        Some(MeetPickBlock::SoleBlockAnchor)
    );
}

/// Two designs that pick the same corner are equal; one that does not pick is different.
#[test]
fn designs_compare_by_their_picks() {
    let (design, _, _) = picked();
    assert_eq!(design, design.clone());
    let mut without = design.clone();
    without.tier_meet_picks.clear();
    assert_ne!(design, without);
}

/// The template's pavilion tiers. A template stores its tiers in the order they are cut.
const TEMPLATE_PAVILION_MAIN: usize = 0;
const TEMPLATE_GIRDLE: usize = 1;
const TEMPLATE_LOWER_GIRDLE: usize = 2;

/// The Standard Round Brilliant template on its own rough, with its lower girdle meeting the
/// named facets Pavilion Main and Girdle and no pick yet: the design the UI harness's
/// `meet_pick_mode` scenario arms pick mode on.
fn template_meeting_by_name() -> Design {
    let spec = &TEMPLATES[0];
    assert_eq!(spec.name, "Standard Round Brilliant", "the first template");
    let mut design = Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers());
    assert_eq!(design.tiers[TEMPLATE_PAVILION_MAIN].name, "Pavilion Main");
    assert_eq!(design.tiers[TEMPLATE_GIRDLE].name, "Girdle");
    assert_eq!(design.tiers[TEMPLATE_LOWER_GIRDLE].name, "Lower Girdle");
    design
        .apply_edit(Edit::SetConstraint {
            index: TEMPLATE_LOWER_GIRDLE,
            constraint: MeetConstraint::MeetNamed(vec![
                "Pavilion Main".to_owned(),
                "Girdle".to_owned(),
            ]),
        })
        .expect("the Meets setting applies");
    design
}

/// Whichever corner pick mode offers for the template's lower girdle, the tier reaches it once
/// the pick is solved: the mast is the corner's, and the pick is met, not off the stone.
///
/// The stone cut before the lower girdle is eight mains and sixteen girdle faces, so it has 25
/// corners a tier can be cut to:
///
/// - the 16 corners where a main crosses the edge between two girdle faces (the template's own
///   meet, mast 0.69655),
/// - the 8 points where the ridge between two mains ends on a girdle face (mast 0.70214, the
///   shallowest point of the two names), and
/// - the point where the eight mains meet (mast 0.65453).
///
/// The harness picks "the first pickable ring", which depends on the camera, so every corner is
/// picked here. Until the solve of the pick lands, the masts on hand are those of the solve
/// before it: there the tier does not pass through a corner at another depth, and the pick
/// reads off the stone. That is the "corner cut away" a row shows when it is worked out too
/// early; it is not how the pick stands.
#[test]
fn every_corner_the_template_offers_is_met_once_its_pick_is_solved() {
    let named = template_meeting_by_name();
    let before = named.solve().expect("the lower girdle meets its names");
    let corners = named
        .pick_corners(&before, TEMPLATE_LOWER_GIRDLE)
        .expect("the stone closes");
    let formed_by = |tiers: &[usize]| corners.iter().filter(|c| c.tiers == tiers).count();
    assert_eq!(
        (
            formed_by(&[TEMPLATE_PAVILION_MAIN, TEMPLATE_GIRDLE]),
            formed_by(&[TEMPLATE_PAVILION_MAIN]),
            corners.len()
        ),
        (24, 1, 25),
        "{corners:?}"
    );

    let mast_before = before[TEMPLATE_LOWER_GIRDLE].mast;
    let mut off_the_stone_before_the_solve = 0;
    for corner in &corners {
        assert_eq!(corner.unnamed_tier, None, "{corner:?}");
        let mut design = named.clone();
        let edit = design
            .meet_pick_edit(TEMPLATE_LOWER_GIRDLE, corner)
            .expect("every tier at the corner is named");
        design.apply_edit(edit).expect("the batch applies");
        assert_eq!(
            design.tier_meet_pick(TEMPLATE_LOWER_GIRDLE),
            Some(&corner.pick),
            "the pick is stored"
        );

        // The masts of the solve before the pick, for a corner at another depth.
        if (corner.mast - mast_before).abs() > 1e-3 {
            assert_eq!(
                design.meet_pick_states(&before),
                [(TEMPLATE_LOWER_GIRDLE, MeetPickState::OffStone)],
                "on the masts of the solve before the pick: {corner:?}"
            );
            off_the_stone_before_the_solve += 1;
        }

        let solved = design.solve().expect("the picked design solves");
        assert!(
            (solved[TEMPLATE_LOWER_GIRDLE].mast - corner.mast).abs() < 1e-5,
            "{} against {}: {corner:?}",
            solved[TEMPLATE_LOWER_GIRDLE].mast,
            corner.mast
        );
        assert_eq!(
            design.meet_pick_states(&solved),
            [(TEMPLATE_LOWER_GIRDLE, MeetPickState::Met)],
            "{corner:?}"
        );
    }
    // The corners lie at three depths, so at least two kinds differ from the depth before.
    assert!(off_the_stone_before_the_solve >= 9);
}
