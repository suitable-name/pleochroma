//! The tier list's rows: core rows -> Slint rows, kept in step with the state.
//!
//! The list is in true cutting order (the order of the printed cutting sheet and the tier
//! codes), so the model position is not the tier index: `ui::flat_tier` and `ui::flat_row`
//! map a position to the tier and back.

use pleochroma_cut_core::design::TierRef;
use pleochroma_mobile_core::{
    app_state::AppState,
    design_state::{Badge, BadgeLevel, MobileTierRow, TierBadges, tier_rows},
};

use super::ui::{self, RowsKey, sync_model};
use crate::TierRowData;

/// Writes the rows to the model unless they are what was pushed last.
pub(super) fn push(state: &AppState) {
    let key = RowsKey {
        generation: state.session.current_generation(),
        solve_revision: state.solve.revision(),
        selected: state.selected_tier,
        selected_concave: state.session.selected_concave,
        multi: state.session.multi_selected.clone(),
    };
    if ui::with(|ui| ui.rows_key.as_ref() == Some(&key)) {
        return;
    }
    let rows = in_cutting_order(state, tier_rows(state));
    let data: Vec<TierRowData> = rows.iter().map(row_data).collect();
    ui::with(|ui| {
        sync_model(&ui.rows_model, data);
        ui.rows = rows;
        ui.rows_key = Some(key);
    });
}

/// Forgets what was pushed, so the next [`push`] rebuilds the rows.
pub(super) fn invalidate() {
    ui::with(|ui| ui.rows_key = None);
}

/// `rows` (flat tiers as authored, then the concave ones) in the design's cutting order. A
/// list that does not match the design is returned as it is.
fn in_cutting_order(state: &AppState, rows: Vec<MobileTierRow>) -> Vec<MobileTierRow> {
    let design = &state.session.design;
    let flat = design.tiers.len();
    if rows.len() != flat + design.concave_tiers.len() {
        return rows;
    }
    let mut slots: Vec<Option<MobileTierRow>> = rows.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for tier in design.cutting_order() {
        let position = match tier {
            TierRef::Flat(index) => index,
            TierRef::Concave(index) => flat + index,
        };
        if let Some(row) = slots.get_mut(position).and_then(Option::take) {
            ordered.push(row);
        }
    }
    // Whatever the order did not name keeps its place at the end.
    ordered.extend(slots.into_iter().flatten());
    ordered
}

/// One core row as the Slint struct.
fn row_data(row: &MobileTierRow) -> TierRowData {
    let (margin_text, margin_level) = badge_parts(row.badges.margin.as_ref());
    let (check_text, check_level) = check_words(&row.badges).map_or_else(
        || (slint::SharedString::default(), 3),
        |(text, level)| (text.as_str().into(), level.index()),
    );
    TierRowData {
        index: i32::try_from(row.index).unwrap_or(0),
        code: row.code.as_str().into(),
        name: row.name.as_str().into(),
        angle: format!("{}\u{b0}", row.angle_text).into(),
        block: row.block.as_str().into(),
        indices: row.indices_text.as_str().into(),
        mast: row.mast_text.as_str().into(),
        summary: if row.is_concave {
            row.tool_line.as_str().into()
        } else {
            row.constraint_text.as_str().into()
        },
        margin: margin_text,
        margin_level,
        check: check_text,
        check_level,
        selected: row.is_selected,
        multi: row.is_multi_selected,
        stale: row.is_stale,
        concave: row.is_concave,
        detached: row.is_detached,
    }
}

/// How urgent a badge is.
const fn severity(level: BadgeLevel) -> u8 {
    match level {
        BadgeLevel::Problem => 3,
        BadgeLevel::Check => 2,
        BadgeLevel::Info => 1,
        BadgeLevel::Good => 0,
    }
}

/// What needs a look on a row, in words: the solve note and the manufacturability finding,
/// joined, with the more urgent of their two levels. `None` when the row has neither (the
/// light margin is the row's chip, not a check).
fn check_words(badges: &TierBadges) -> Option<(String, BadgeLevel)> {
    let found: Vec<&Badge> = [&badges.solve, &badges.warning]
        .into_iter()
        .flatten()
        .collect();
    let level = found
        .iter()
        .map(|badge| badge.level)
        .max_by_key(|level| severity(*level))?;
    let text = found
        .iter()
        .map(|badge| badge.text.as_str())
        .collect::<Vec<_>>()
        .join(". ");
    Some((text, level))
}

/// A badge's text and level for Slint; empty text means none.
fn badge_parts(badge: Option<&Badge>) -> (slint::SharedString, i32) {
    badge.map_or_else(
        || (slint::SharedString::default(), 3),
        |badge| (badge.text.as_str().into(), badge.level.index()),
    )
}

#[cfg(test)]
mod tests {
    use pleochroma_mobile_core::design_state::{Badge, BadgeLevel, TierBadges};

    use super::*;

    fn badge(text: &str, level: BadgeLevel) -> Badge {
        Badge {
            text: text.to_owned(),
            level,
        }
    }

    #[test]
    fn the_check_joins_the_solve_note_and_the_finding_at_the_worse_level() {
        let badges = TierBadges {
            solve: Some(badge("Estimate", BadgeLevel::Check)),
            margin: Some(badge("Safe +3.1\u{b0}", BadgeLevel::Good)),
            warning: Some(badge("Thin girdle", BadgeLevel::Problem)),
        };
        let (text, level) = check_words(&badges).expect("two findings");
        assert_eq!(text, "Estimate. Thin girdle");
        assert_eq!(level, BadgeLevel::Problem);
    }

    #[test]
    fn the_light_margin_alone_is_no_check() {
        let badges = TierBadges {
            solve: None,
            margin: Some(badge("Safe +3.1\u{b0}", BadgeLevel::Good)),
            warning: None,
        };
        assert!(check_words(&badges).is_none());
    }

    #[test]
    fn no_badges_give_no_check_and_no_margin() {
        let badges = TierBadges::default();
        assert!(check_words(&badges).is_none());
        let (text, level) = badge_parts(badges.margin.as_ref());
        assert!(text.is_empty());
        assert_eq!(level, 3);
        assert_eq!(severity(BadgeLevel::Info), 1);
    }
}
