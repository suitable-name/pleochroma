//! Tier notes, and the history trail ([`crate::native::DesignExtras::history_entries`]),
//! through a save and load of the design file.

use super::fixtures::{reopen, simple_design};
use crate::{
    design::ConstraintTier,
    edit::{Edit, History},
    native::{DesignExtras, design_from_str, design_to_file, design_to_string},
};
use pleochroma::geometry::meet_solver::MeetConstraint;

/// A note survives a full `Edit::History` add, remove and move sequence, then a save and load
/// round trip. The index maintenance of `crate::edit` is checked here through the native file.
///
/// Built on [`super::fixtures::unsolved_design`], which never solves. A solve result after a
/// reorder then cannot change what the test checks.
#[test]
fn tier_notes_survive_add_remove_and_reorder_then_a_save_load_round_trip() {
    let mut design = super::fixtures::unsolved_design();
    design.tiers.push(tier_named("B"));
    design.tiers.push(tier_named("C"));
    let mut history = History::new();

    // Tier 1 ("B") gets a note.
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: Some("grind slowly".to_string()),
            },
        )
        .expect("set tier note must apply");

    // Insert a new tier before it: the note must follow to index 2.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier_named("Z"),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(2), Some("grind slowly"));

    // Remove tier 0 (the one just inserted): the note must shift back to 1.
    history
        .apply(&mut design, Edit::RemoveTier { index: 0 })
        .expect("remove must apply");
    assert_eq!(design.tier_note(1), Some("grind slowly"));

    // Move the noted tier (1) to the end: the note must follow it.
    let last = design.tiers.len() - 1;
    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: last })
        .expect("move must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(last), Some("grind slowly"));

    // This design has no scale-reference anchor anywhere, so it never solves; the
    // design file still holds every tier in full, notes included.
    assert!(design.solve().is_err(), "this design can never solve");
    let loaded = reopen(&design);
    assert_eq!(loaded.design.tier_note(last), Some("grind slowly"));
}

/// A minimal tier with no indices, used as filler by the test above.
fn tier_named(name: &str) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 0.0,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// [`crate::native::DesignExtras::history_entries`] must reach the written file's
/// `[history]` table and come back out of `design_from_str` unchanged.
#[test]
fn history_entries_round_trip_through_save_and_load() {
    let design = simple_design();
    let entries = vec![
        "Set material to Diamond".to_string(),
        "Remove tier C1".to_string(),
    ];
    let extras = DesignExtras {
        history_entries: &entries,
        ..DesignExtras::default()
    };
    let text = design_to_string(&design, None, &extras).expect("must serialize");
    assert!(text.contains("[history]"));

    let loaded = design_from_str(&text).expect("must load back");
    assert_eq!(loaded.history_entries, entries);
}

/// An empty history (a brand-new design's first save) must write no `[history]`
/// table at all, and load back as an empty trail.
#[test]
fn an_empty_history_writes_no_history_table_at_all() {
    let design = simple_design();
    let file = design_to_file(&design, None, &DesignExtras::default());
    assert!(file.history.is_none());

    let loaded = reopen(&design);
    assert_eq!(loaded.history_entries, Vec::<String>::new());
}

/// [`crate::edit::History::description_log`] accumulates one entry per applied edit, in
/// order. A save's `history_entries` are drawn from it.
#[test]
fn history_description_log_accumulates_applied_edits_in_order() {
    let mut design = simple_design();
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(7.0),
            },
        )
        .expect("must apply");
    history
        .apply(&mut design, Edit::SetPreformYOffset { y_offset: 0.3 })
        .expect("must apply");
    assert_eq!(history.description_log().len(), 2);
    assert_eq!(
        history.description_log()[0],
        "Set girdle diameter to 7.00 mm"
    );
    assert_eq!(history.description_log()[1], "Set preform offset to 0.30");
}
