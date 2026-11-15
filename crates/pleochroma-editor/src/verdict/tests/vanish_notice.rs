//! Tests for the vanished-facet notice: the snapshot read off verdict inputs, the before/after
//! comparison by tier id, and every wording of the notice.

use super::{design_with, tier, vanishing};
use crate::verdict::{
    VanishEntry, VanishNotice, VanishSnapshot, VerdictInputs, vanish_notice, vanish_snapshot,
};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::TierId;

fn entry(id: u64, index: usize, label: &str, vanished: usize, total: usize) -> VanishEntry {
    VanishEntry {
        id: TierId(id),
        index,
        label: label.to_string(),
        vanished,
        total,
    }
}

fn snapshot(entries: Vec<VanishEntry>) -> VanishSnapshot {
    VanishSnapshot { tiers: entries }
}

fn notice(before: &[VanishEntry], after: &[VanishEntry]) -> Option<VanishNotice> {
    vanish_notice(&snapshot(before.to_vec()), &snapshot(after.to_vec()))
}

#[test]
fn nothing_vanished_gives_no_notice() {
    assert_eq!(notice(&[], &[]), None);
    let same = [entry(1, 0, "P2", 8, 8)];
    assert_eq!(notice(&same, &same), None);
}

#[test]
fn a_newly_vanished_tier_gives_the_short_sentence_for_one_facet() {
    let found = notice(&[], &[entry(7, 3, "P2", 1, 1)]).expect("a notice");
    assert_eq!(found.text, "This cut away P2.");
    assert_eq!(found.select_tier, 3);
    assert_eq!(found.select_label, "P2");
}

#[test]
fn a_newly_vanished_tier_with_many_facets_says_all_of_them() {
    let found = notice(&[], &[entry(7, 3, "P2", 8, 8)]).expect("a notice");
    assert_eq!(found.text, "This cut away P2: all 8 of its facets.");
}

#[test]
fn a_partial_vanish_counts_the_facets_lost() {
    let found = notice(&[], &[entry(7, 3, "P2", 2, 8)]).expect("a notice");
    assert_eq!(found.text, "This cut away 2 of the 8 facets of P2.");
}

#[test]
fn a_partial_vanish_that_grows_counts_only_the_growth() {
    let found = notice(&[entry(7, 3, "P2", 2, 8)], &[entry(7, 3, "P2", 5, 8)]).expect("a notice");
    assert_eq!(found.text, "This cut away 3 of the 8 facets of P2.");
}

#[test]
fn a_partial_vanish_that_becomes_total_says_all() {
    let found = notice(&[entry(7, 3, "P2", 2, 8)], &[entry(7, 3, "P2", 8, 8)]).expect("a notice");
    assert_eq!(found.text, "This cut away P2: all 8 of its facets.");
}

#[test]
fn a_shrinking_vanish_gives_no_notice() {
    assert_eq!(
        notice(&[entry(7, 3, "P2", 6, 8)], &[entry(7, 3, "P2", 2, 8)]),
        None
    );
}

#[test]
fn a_removed_tier_gives_no_notice() {
    assert_eq!(notice(&[entry(7, 3, "P2", 8, 8)], &[]), None);
}

#[test]
fn a_removed_tier_does_not_hide_a_new_vanish() {
    let found = notice(&[entry(7, 3, "P2", 8, 8)], &[entry(9, 4, "P3", 8, 8)]).expect("a notice");
    assert_eq!(found.text, "This cut away P3: all 8 of its facets.");
    assert_eq!(found.select_tier, 4);
}

#[test]
fn tiers_are_matched_by_id_not_by_position() {
    // The tier moved from position 3 to position 5 and kept its vanished facets.
    assert_eq!(
        notice(&[entry(7, 3, "P2", 8, 8)], &[entry(7, 5, "P2", 8, 8)]),
        None
    );
    // A different tier now at position 3 is new.
    let found = notice(&[entry(7, 3, "P2", 8, 8)], &[entry(8, 3, "P3", 8, 8)]).expect("a notice");
    assert_eq!(found.select_label, "P3");
}

#[test]
fn an_unchanged_tier_next_to_a_new_one_is_not_named() {
    let found = notice(
        &[entry(1, 1, "G1", 8, 8)],
        &[entry(1, 1, "G1", 8, 8), entry(2, 4, "P2", 1, 1)],
    )
    .expect("a notice");
    assert_eq!(found.text, "This cut away P2.");
    assert_eq!(found.select_tier, 4);
}

#[test]
fn two_tiers_join_with_and() {
    let found = notice(&[], &[entry(1, 2, "P1", 4, 4), entry(2, 5, "P2", 4, 4)]).expect("a notice");
    assert_eq!(found.text, "This cut away P1 and P2.");
    assert_eq!(found.select_tier, 2);
    assert_eq!(found.select_label, "P1");
}

#[test]
fn three_tiers_join_with_commas_and_a_final_and() {
    let found = notice(
        &[],
        &[
            entry(1, 2, "P1", 4, 4),
            entry(2, 5, "P2", 4, 4),
            entry(3, 6, "P3", 4, 4),
        ],
    )
    .expect("a notice");
    assert_eq!(found.text, "This cut away P1, P2 and P3.");
}

#[test]
fn five_tiers_name_two_and_count_the_rest() {
    let after: Vec<VanishEntry> = (0..5)
        .map(|n| entry(n as u64, n + 1, &format!("T{n}"), 2, 2))
        .collect();
    let found = notice(&[], &after).expect("a notice");
    assert_eq!(found.text, "This cut away T0, T1 and 3 more tiers.");
    assert_eq!(found.select_tier, 1);
    assert_eq!(found.select_label, "T0");
}

#[test]
fn four_tiers_name_two_and_count_two() {
    let after: Vec<VanishEntry> = (0..4)
        .map(|n| entry(n as u64, n, &format!("T{n}"), 2, 2))
        .collect();
    let found = notice(&[], &after).expect("a notice");
    assert_eq!(found.text, "This cut away T0, T1 and 2 more tiers.");
}

#[test]
fn snapshot_reads_the_vanishing_findings_in_schedule_order() {
    let design = design_with(vec![
        tier("A", 30.0, MeetConstraint::MeetExisting, &[0.0]),
        tier("", 40.0, MeetConstraint::MeetExisting, &[0.0]),
        tier("C", 50.0, MeetConstraint::MeetExisting, &[0.0]),
    ]);
    let inputs = VerdictInputs {
        warnings: vec![vanishing(2, "C", 1, 4), vanishing(1, "", 3, 3)],
        ..VerdictInputs::default()
    };
    let snap = vanish_snapshot(&design, &inputs);
    assert_eq!(snap.tiers.len(), 2);
    assert_eq!(snap.tiers[0].index, 1);
    assert_eq!(snap.tiers[0].label, "tier 2");
    assert_eq!(snap.tiers[0].id, design.tier_id_at(1).expect("id"));
    assert_eq!((snap.tiers[0].vanished, snap.tiers[0].total), (3, 3));
    assert_eq!(snap.tiers[1].label, "C");
    assert_eq!((snap.tiers[1].vanished, snap.tiers[1].total), (1, 4));
}

#[test]
fn snapshot_prefers_the_verdicts_display_names() {
    let design = design_with(vec![
        tier("3", 30.0, MeetConstraint::MeetExisting, &[0.0]),
        tier("A", 40.0, MeetConstraint::MeetExisting, &[0.0]),
    ]);
    let inputs = VerdictInputs {
        tier_names: vec!["P1".to_string(), "P2".to_string()],
        warnings: vec![vanishing(0, "3", 1, 1)],
        ..VerdictInputs::default()
    };
    let snap = vanish_snapshot(&design, &inputs);
    assert_eq!(snap.tiers[0].label, "P1");
}

#[test]
fn snapshot_of_inputs_without_findings_is_empty() {
    let design = design_with(vec![tier("A", 30.0, MeetConstraint::MeetExisting, &[0.0])]);
    let snap = vanish_snapshot(&design, &VerdictInputs::default());
    assert_eq!(snap, VanishSnapshot::default());
}

#[test]
fn snapshots_around_an_edit_give_the_notice() {
    let design = design_with(vec![
        tier("P1", 40.0, MeetConstraint::MeetExisting, &[0.0]),
        tier("P2", 45.0, MeetConstraint::MeetExisting, &[0.0]),
    ]);
    let before = vanish_snapshot(&design, &VerdictInputs::default());
    let inputs = VerdictInputs {
        warnings: vec![vanishing(1, "P2", 4, 4)],
        ..VerdictInputs::default()
    };
    let after = vanish_snapshot(&design, &inputs);
    let found = vanish_notice(&before, &after).expect("a notice");
    assert_eq!(found.text, "This cut away P2: all 4 of its facets.");
    assert_eq!(found.select_tier, 1);
}
