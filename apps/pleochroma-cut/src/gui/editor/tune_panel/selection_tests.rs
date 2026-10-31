//! Selection uses design positions even when Tune reorders or omits rows.

use super::{listed_tier, requested_tier};
use crate::gui::editor::tune_panel::{
    order::{Side, TierInfo, next_untuned},
    present::{ListInput, tier_rows},
};
use std::collections::BTreeSet;

fn infos() -> Vec<TierInfo> {
    [
        (1, Side::Crown, false),
        (4, Side::Girdle, false),
        (5, Side::Pavilion, false),
        (7, Side::Crown, true),
    ]
    .into_iter()
    .map(|(index, side, driven)| TierInfo {
        index,
        code: index.to_string(),
        name: format!("Tier {index}"),
        angle_deg: if side == Side::Girdle { 90.0 } else { 40.0 },
        side,
        driven,
        relation: driven.then(|| "C1".into()),
    })
    .collect()
}

#[test]
fn a_facet_pick_highlights_its_design_tier_in_a_reordered_list() {
    let infos = infos();
    let selected = listed_tier(&infos, 1);
    let rows = tier_rows(&ListInput {
        infos: &infos,
        order: &[5, 7, 1, 4],
        baseline: &[],
        accepted: &BTreeSet::new(),
        selected,
        reorderable: true,
    });
    assert!(rows[2].selected);
    assert_eq!(rows[2].tier, 1);
    assert!(!rows[0].selected);
    assert_eq!(
        requested_tier(&infos, rows[0].tier, Some((3, 8)), (3, 8)),
        Some(5)
    );
}

#[test]
fn a_non_tunable_facet_selects_its_row_without_enabling_a_tune_request() {
    let infos = infos();
    for tier in [4, 7] {
        assert_eq!(listed_tier(&infos, tier), usize::try_from(tier).ok());
        assert_eq!(requested_tier(&infos, tier, Some((3, 8)), (3, 8)), None);
    }
    // Deselect, omitted table/culet, out-of-range and concave rows clear the Tune choice.
    for tier in [-1, 0, 9, i32::MAX] {
        assert_eq!(listed_tier(&infos, tier), None);
    }
}

#[test]
fn a_stale_row_cannot_select_the_tier_that_replaced_its_old_position() {
    let infos = infos();
    assert_eq!(requested_tier(&infos, 5, Some((3, 8)), (3, 9)), None);
    assert_eq!(requested_tier(&infos, 5, Some((3, 8)), (4, 8)), None);
    assert_eq!(requested_tier(&infos, 5, None, (3, 8)), None);
    assert_eq!(requested_tier(&infos, 5, Some((3, 8)), (3, 8)), Some(5));
}

#[test]
fn next_uses_the_next_tier_identity_rather_than_the_list_position() {
    let infos = infos();
    let next = next_untuned(
        &[5, 7, 1, 4],
        Some(5),
        &|tier| {
            infos
                .iter()
                .any(|info| info.index == tier && info.tunable())
        },
        &BTreeSet::new(),
    )
    .expect("another tunable tier");
    assert_eq!(next, 1);
    assert_eq!(
        listed_tier(&infos, i32::try_from(next).expect("tier fits")),
        Some(1)
    );
}
