//! Meet picks in the design file (version 5, written only when a pick exists), and the
//! loader's rule for a pick it cannot keep.

use super::fixtures::{design_text, reopen, simple_design};
use crate::{
    design::{Design, PickedFacet, TierMeetPick},
    native::{DesignExtras, design_from_file, design_from_str, design_to_file},
};
use pleochroma_formats::native::{
    MeetPickFacetTable,
    design::{DESIGN_VERSION_MEET_PICKS, DesignFile},
};

/// The simple design with tier ids assigned and tier 2 picked at the corner of tiers 0, 1 and 2.
fn picked_design() -> (Design, TierMeetPick) {
    let mut design = simple_design();
    design.ensure_tier_ids();
    let ids = design.tier_ids.clone();
    let pick = TierMeetPick {
        facets: [
            PickedFacet {
                tier: ids[0],
                index: 1.0,
            },
            PickedFacet {
                tier: ids[1],
                index: 2.0,
            },
            PickedFacet {
                tier: ids[2],
                index: 3.0,
            },
        ],
    };
    design.tier_meet_picks.insert(ids[2], pick);
    (design, pick)
}

fn file_of(design: &Design) -> DesignFile {
    design_to_file(design, None, &DesignExtras::default())
}

/// C8: a design without a pick writes neither the key nor version 5.
#[test]
fn a_design_without_a_pick_writes_no_pick_and_no_version_5() {
    let mut design = simple_design();
    design.ensure_tier_ids();
    let file = file_of(&design);
    assert_ne!(file.version, DESIGN_VERSION_MEET_PICKS);
    assert!(file.tiers.iter().all(|t| t.meet_pick.is_empty()));
    assert!(!design_text(&design).contains("meet_pick"));
    let loaded = reopen(&design);
    assert!(loaded.design.tier_meet_picks.is_empty());
    assert!(loaded.load_notes.is_empty());
}

/// C8: a picked design is version 5 and equal after a reload.
#[test]
fn a_pick_round_trips_as_version_5() {
    let (design, pick) = picked_design();
    assert_eq!(file_of(&design).version, DESIGN_VERSION_MEET_PICKS);
    let text = design_text(&design);
    assert!(text.starts_with("format = \"pleochroma-design\"\nversion = 5\n"));
    let loaded = design_from_str(&text).expect("opens");
    assert!(loaded.load_notes.is_empty());
    assert_eq!(loaded.design.tier_meet_pick(2), Some(&pick));
    assert_eq!(loaded.design.tier_meet_pick(0), None);
    assert_eq!(loaded.design.tier_meet_picks, design.tier_meet_picks);
    assert_eq!(loaded.design, design);
}

/// C8: a pick that does not have exactly three entries loads as no pick, with a note.
#[test]
fn a_pick_with_two_entries_loads_as_no_pick_and_adds_a_note() {
    let (design, _) = picked_design();
    let mut file = file_of(&design);
    file.tiers[2].meet_pick.truncate(2);
    let loaded = design_from_file(file).expect("a bad pick is not an error");
    assert!(loaded.design.tier_meet_picks.is_empty());
    assert_eq!(loaded.load_notes.len(), 1);
    assert!(loaded.load_notes[0].contains("meet pick"));
}

/// C8: a pick that names a tier id the file does not hold loads as no pick, with a note.
#[test]
fn a_pick_with_an_unknown_tier_id_loads_as_no_pick_and_adds_a_note() {
    let (design, _) = picked_design();
    let mut file = file_of(&design);
    file.tiers[2].meet_pick[1] = MeetPickFacetTable {
        tier: 9_999,
        at: 2.0,
    };
    let loaded = design_from_file(file).expect("a bad pick is not an error");
    assert!(loaded.design.tier_meet_picks.is_empty());
    assert_eq!(loaded.load_notes.len(), 1);
}

/// A pick is kept while another tier's bad pick is dropped.
#[test]
fn only_the_bad_pick_is_dropped() {
    let (mut design, pick) = picked_design();
    let first = design.tier_ids[0];
    design.tier_meet_picks.insert(first, pick);
    let mut file = file_of(&design);
    file.tiers[0].meet_pick.pop();
    let loaded = design_from_file(file).expect("opens");
    assert_eq!(loaded.design.tier_meet_pick(0), None);
    assert_eq!(loaded.design.tier_meet_pick(2), Some(&pick));
    assert_eq!(loaded.load_notes.len(), 1);
}

/// Picks are written only when the design's tier ids are in step with its tiers.
#[test]
fn picks_are_not_written_when_the_ids_are_out_of_step() {
    let (mut design, _) = picked_design();
    design.tier_ids.pop();
    let file = file_of(&design);
    assert!(file.tiers.iter().all(|t| t.meet_pick.is_empty()));
    assert_ne!(file.version, DESIGN_VERSION_MEET_PICKS);
}
