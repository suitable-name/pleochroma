//! `Edit::SetTierMeetPick`: apply, undo and redo; what removing a tier does to the picks;
//! the gear remap; `ReplaceSchedule`; and the re-solve that follows.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::{Design, PickedFacet, TierMeetPick},
    edit::{Edit, History, RemapRounding, ScheduleState},
    resolve::resolve_after_edit,
};
use pleochroma::geometry::meet_solver::MeetConstraint;

/// P1, P2 and G1 (anchored, so the design solves) and P3, which meets P1 and P2. The ids are
/// allocated, so a pick can name them.
fn picked_design() -> Design {
    let mut design = fresh_design();
    for (name, angle, constraint) in [
        ("P1", 30.0, MeetConstraint::ScaleReference(0.5)),
        ("P2", 40.0, MeetConstraint::ScaleReference(0.45)),
        ("G1", 50.0, MeetConstraint::ScaleReference(0.6)),
        (
            "P3",
            45.0,
            MeetConstraint::MeetNamed(vec!["P1".to_owned(), "P2".to_owned()]),
        ),
    ] {
        design
            .tiers
            .push(tier(name, angle, constraint, &[0.0, 24.0, 48.0, 72.0]));
    }
    design.ensure_tier_ids();
    design
}

/// A pick for the tier at `3` through facets of the tiers at positions 0, 1 and 2.
fn pick_for(design: &Design, at: [f64; 3]) -> TierMeetPick {
    let id = |position: usize| design.tier_id_at(position).expect("tier id");
    TierMeetPick {
        facets: [
            PickedFacet {
                tier: id(0),
                index: at[0],
            },
            PickedFacet {
                tier: id(1),
                index: at[1],
            },
            PickedFacet {
                tier: id(2),
                index: at[2],
            },
        ],
    }
}

fn set_pick(index: usize, pick: Option<TierMeetPick>) -> Edit {
    Edit::SetTierMeetPick { index, pick }
}

// --- SetTierMeetPick ---

#[test]
fn setting_a_pick_then_undo_and_redo_is_exact() {
    let mut design = picked_design();
    let before = design.clone();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    let mut history = History::new();

    history
        .apply(&mut design, set_pick(3, Some(pick)))
        .expect("the pick applies");
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
    assert_ne!(design, before, "a pick is part of the design");

    assert!(history.undo(&mut design).unwrap());
    assert!(design.tier_meet_picks.is_empty());
    assert_eq!(design, before);

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
}

#[test]
fn replacing_a_pick_inverts_to_the_previous_pick_and_clearing_inverts_to_the_pick() {
    let mut design = picked_design();
    let first = pick_for(&design, [24.0, 48.0, 0.0]);
    let second = pick_for(&design, [0.0, 72.0, 24.0]);
    let id = design.tier_id_at(3).unwrap();

    let undo_first = design.apply_edit(set_pick(3, Some(first))).unwrap();
    assert_eq!(undo_first, set_pick(3, None));
    let undo_second = design.apply_edit(set_pick(3, Some(second))).unwrap();
    assert_eq!(undo_second, set_pick(3, Some(first)));
    let undo_clear = design.apply_edit(set_pick(3, None)).unwrap();
    assert_eq!(undo_clear, set_pick(3, Some(second)));
    assert!(!design.tier_meet_picks.contains_key(&id));
    design.apply_edit(undo_clear).unwrap();
    assert_eq!(design.tier_meet_picks.get(&id), Some(&second));
}

#[test]
fn a_pick_for_a_tier_past_the_end_is_refused_and_changes_nothing() {
    let mut design = picked_design();
    let before = design.clone();
    let pick = pick_for(&design, [0.0, 0.0, 0.0]);

    let error = design
        .apply_edit(set_pick(9, Some(pick)))
        .expect_err("tier 9 does not exist");
    assert_eq!(error.index, 9);
    assert_eq!(error.tier_count, 4);
    assert_eq!(design, before);
}

#[test]
fn a_pick_follows_its_tier_through_a_move() {
    let mut design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(pick))).unwrap();

    design
        .apply_edit(Edit::MoveTier { from: 3, to: 0 })
        .unwrap();
    assert_eq!(design.index_of_tier_id(id), Some(0));
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
}

#[test]
fn the_pick_sentence_names_the_tier_and_the_corner() {
    let design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    assert_eq!(
        set_pick(3, Some(pick)).describe(&design),
        "Pick the meet point of P3 (P1, P2, G1)"
    );
    assert_eq!(
        set_pick(3, None).describe(&design),
        "Clear the picked meet point of P3"
    );
}

// --- RemoveTier ---

/// Removing the picking tier takes its pick along, and undo puts both back.
#[test]
fn removing_the_picking_tier_and_undoing_restores_the_pick() {
    let mut design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(pick))).unwrap();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 3 })
        .expect("the tier is removed");
    assert!(design.tier_meet_picks.is_empty(), "the pick left with it");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(design.tier_ids_eq(&before));
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
}

/// Removing a tier that another tier's pick names leaves that pick stored: it reads as lost, and
/// undo makes it whole again with no extra step.
#[test]
fn removing_a_picked_facets_tier_keeps_the_pick_and_undo_brings_the_facet_back() {
    let mut design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(pick))).unwrap();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("the tier is removed");
    assert_eq!(
        design.tier_meet_picks.get(&id),
        Some(&pick),
        "the pick of the other tier is untouched"
    );
    assert!(
        design.index_of_tier_id(pick.facets[1].tier).is_none(),
        "its second facet's tier is gone"
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(
        design.index_of_tier_id(pick.facets[1].tier),
        Some(1),
        "the removed tier came back under its own id"
    );
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
}

// --- RemapIndices ---

/// A 96 to 64 remap moves the picked facets' index-wheel positions like the tiers' own, and
/// undo restores them exactly.
#[test]
fn a_gear_remap_moves_the_picked_positions_and_undo_restores_them() {
    let mut design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 72.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(pick))).unwrap();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RemapIndices {
                from_gear: 96,
                to_gear: 64,
                rounding: RemapRounding::Nearest,
            },
        )
        .expect("the remap applies");
    let moved = design.tier_meet_picks.get(&id).expect("the pick stays");
    let positions: Vec<f64> = moved.facets.iter().map(|facet| facet.index).collect();
    assert_eq!(positions, [16.0, 32.0, 48.0]);
    let tiers: Vec<_> = moved.facets.iter().map(|facet| facet.tier).collect();
    assert_eq!(
        tiers,
        pick.facets.map(|facet| facet.tier),
        "only the positions move"
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.tier_meet_picks.get(&id), Some(&pick));
}

/// A remap of a design without picks keeps its plain `RestoreIndices` inverse.
#[test]
fn a_remap_without_picks_keeps_the_plain_inverse() {
    let mut design = picked_design();
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: 96,
            to_gear: 64,
            rounding: RemapRounding::Nearest,
        })
        .unwrap();
    assert!(matches!(inverse, Edit::RestoreIndices { .. }));
}

// --- ReplaceSchedule ---

#[test]
fn replace_schedule_carries_the_picks_both_ways() {
    let mut design = picked_design();
    let first = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(first))).unwrap();
    let before = design.clone();

    let mut state = ScheduleState::of(&design);
    assert_eq!(state.tier_meet_picks.get(&id), Some(&first));
    let second = pick_for(&design, [0.0, 72.0, 24.0]);
    state.tier_meet_picks.insert(id, second);
    let mut history = History::new();

    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state)))
        .expect("the state applies");
    assert_eq!(design.tier_meet_picks.get(&id), Some(&second));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.tier_meet_picks.get(&id), Some(&first));

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tier_meet_picks.get(&id), Some(&second));
}

#[test]
fn replace_schedule_can_drop_a_pick_and_undo_brings_it_back() {
    let mut design = picked_design();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let id = design.tier_id_at(3).unwrap();
    design.apply_edit(set_pick(3, Some(pick))).unwrap();
    let before = design.clone();

    let mut state = ScheduleState::of(&design);
    state.tier_meet_picks.clear();
    let inverse = design
        .apply_edit(Edit::ReplaceSchedule(Box::new(state)))
        .unwrap();
    assert!(!design.tier_meet_picks.contains_key(&id));
    design.apply_edit(inverse).unwrap();
    assert_eq!(design, before);
}

/// A pick keyed by an id the state's tiers do not hold is refused, like the other id-keyed
/// maps, and the design is left as it was.
#[test]
fn replace_schedule_refuses_a_pick_for_an_unknown_tier_id() {
    let mut design = picked_design();
    let before = design.clone();
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let mut state = ScheduleState::of(&design);
    state
        .tier_meet_picks
        .insert(crate::design::TierId(9_999), pick);

    assert!(
        design
            .apply_edit(Edit::ReplaceSchedule(Box::new(state)))
            .is_err()
    );
    assert_eq!(design, before);
}

// --- resolve_after_edit ---

/// A pick can move any non-anchor tier, so the re-solve after it is the full solve.
#[test]
fn resolve_after_a_pick_equals_a_full_solve() {
    let mut design = picked_design();
    let baseline = design.solve().expect("the design solves");
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let edit = set_pick(3, Some(pick));
    design.apply_edit(edit.clone()).unwrap();

    let resolved = resolve_after_edit(&design, &baseline, &edit).expect("resolves");
    let full = design.solve().expect("solves");
    assert_eq!(resolved.len(), full.len());
    for (resolved, full) in resolved.iter().zip(&full) {
        assert_eq!(resolved.mast.to_bits(), full.mast.to_bits());
    }
}

/// The same holds inside a batch, which is how the editor applies a pick (constraint, target and
/// pick as one step).
#[test]
fn resolve_after_a_batch_with_a_pick_equals_a_full_solve() {
    let mut design = picked_design();
    let baseline = design.solve().expect("the design solves");
    let pick = pick_for(&design, [24.0, 48.0, 0.0]);
    let edit = Edit::Batch(vec![
        Edit::SetConstraint {
            index: 3,
            constraint: MeetConstraint::MeetNamed(vec![
                "P1".to_owned(),
                "P2".to_owned(),
                "G1".to_owned(),
            ]),
        },
        set_pick(3, Some(pick)),
    ]);
    design.apply_edit(edit.clone()).unwrap();

    let resolved = resolve_after_edit(&design, &baseline, &edit).expect("resolves");
    let full = design.solve().expect("solves");
    assert_eq!(resolved.len(), full.len());
    for (resolved, full) in resolved.iter().zip(&full) {
        assert_eq!(resolved.mast.to_bits(), full.mast.to_bits());
    }
}
