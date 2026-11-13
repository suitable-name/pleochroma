//! The `EditorModel.tiers`/`multi_selected_count` push helpers and the thin
//! adapters from `pleochroma_editor::view_model`'s plain rows/chips to the Slint
//! `EditorTierItem`/`IndexChipItem`. The per-tier formatting itself lives in
//! `pleochroma_editor::view_model::row_format` (shared with the web app).

use crate::{EditorTierItem, IndexChipItem, MainWindow};
use pleochroma_editor::view_model::{IndexChip, TierRow, TierRowKind};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

/// The longest line of a hand-wrapped tooltip, in characters. The built-in Slint
/// `Tooltip` sizes itself from the unwrapped text but honours `\n`.
pub(in crate::gui::editor) const TOOLTIP_LINE_CHARS: usize = 70;

/// `text` with a `\n` put at the last space before a line would pass `width` characters,
/// for the built-in Slint `Tooltip` (it cannot wrap by itself). Existing `\n` stay; a word
/// longer than `width` keeps its own line. Wrapping already wrapped text changes nothing.
/// Only for strings that feed a tooltip: a status line or an accessible label must not get
/// the breaks.
#[must_use]
pub(in crate::gui::editor) fn wrap_tooltip_text(text: &str, width: usize) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut line_chars = 0;
        for word in paragraph.split_whitespace() {
            let word_chars = word.chars().count();
            if line_chars > 0 {
                if line_chars + 1 + word_chars > width {
                    out.push('\n');
                    line_chars = 0;
                } else {
                    out.push(' ');
                    line_chars += 1;
                }
            }
            out.push_str(word);
            line_chars += word_chars;
        }
    }
    out
}

/// One plain [`TierRow`] as the Slint `EditorTierItem` the tier table renders --
/// a field-for-field copy.
pub(in crate::gui::editor) fn tier_item_from_row(row: TierRow) -> EditorTierItem {
    EditorTierItem {
        index: row.index,
        angle_deg: row.angle_deg.into(),
        angle_full: row.angle_full.into(),
        name: row.name.into(),
        indices: row.indices.into(),
        indices_full: row.indices_full.into(),
        constraint_kind: row.constraint_kind,
        constraint_text: row.constraint_text.into(),
        mast: row.mast.into(),
        mast_mm: row.mast_mm.into(),
        mast_full: row.mast_full.into(),
        strategy: row.strategy.into(),
        strategy_is_uncertain: row.strategy_is_uncertain,
        strategy_detail: row.strategy_detail.into(),
        needs_anchor: row.needs_anchor,
        imported_meet_text: row.imported_meet_text.into(),
        orbit_status: row.orbit_status.into(),
        orbit_incomplete: row.orbit_incomplete,
        is_detached: row.is_detached,
        block: row.block.into(),
        code: row.code.into(),
        margin_text: row.margin_text.into(),
        risk_level: row.risk_level,
        meet_partners_text: row.meet_partners_text.into(),
        warning_text: row.warning_text.into(),
        multi_selected: row.multi_selected,
        proposed_angle: row.proposed_angle.into(),
        kind: i32::from(row.kind == TierRowKind::Concave),
        tool_line: row.tool_line.into(),
        relation_text: row.relation_text.into(),
        frosting_text: row.frosting_text.into(),
    }
}

/// Maps a whole plain row list with [`tier_item_from_row`].
///
/// A concave row's `index` is its position in `design.concave_tiers`, which would
/// collide with a flat tier of the same number in every `item.index ==
/// selected_tier_index` comparison and in the keyboard cursor. The table is flat rows
/// first, then concave ones, so the Slint item carries the row's POSITION instead
/// (flat count plus the concave index) and Rust maps it back with
/// [`concave_tier_index`]. Flat rows are untouched.
pub(in crate::gui::editor) fn tier_items_from_rows(rows: Vec<TierRow>) -> Vec<EditorTierItem> {
    let flat_count = rows
        .iter()
        .filter(|row| row.kind == TierRowKind::Flat)
        .count();
    rows.into_iter()
        .map(|row| {
            let concave_index = row.index;
            let mut item = tier_item_from_row(row);
            if item.kind == 1 {
                item.index = i32::try_from(flat_count)
                    .unwrap_or(i32::MAX)
                    .saturating_add(concave_index);
            }
            item
        })
        .collect()
}

/// The concave tier a table position names: `position` past the flat tiers, within the
/// concave ones. `None` for a flat position, a negative one, or one beyond the table.
/// The inverse of the position [`tier_items_from_rows`] gives a concave row.
#[must_use]
pub(in crate::gui::editor) fn concave_tier_index(
    flat_count: usize,
    concave_count: usize,
    position: i32,
) -> Option<usize> {
    usize::try_from(position)
        .ok()?
        .checked_sub(flat_count)
        .filter(|&index| index < concave_count)
}

/// [`pleochroma_editor::view_model::row_format::index_chip_items`], mapped to the
/// inspector's Slint chips -- see that function for the detached-flag rule.
#[must_use]
pub(in crate::gui::editor) fn index_chip_items(
    indices: &[f64],
    detached: &[f64],
) -> Vec<IndexChipItem> {
    pleochroma_editor::view_model::row_format::index_chip_items(indices, detached)
        .into_iter()
        .map(|chip: IndexChip| IndexChipItem {
            position: chip.position,
            label: chip.label.into(),
            detached: chip.detached,
            // Set by `view::selected_tier_chips`, which knows the design's frosting.
            frosted: false,
        })
        .collect()
}

/// Patches [`EditorTierItem::multi_selected`] onto every row in `rows` from
/// `multi_selected`. The tier-list builders ([`super::rows::tier_items`] and the
/// `tier_items_from_*` family) always set the flag to `false`, so a caller that replaces
/// the whole `editor_tiers` model applies this right after the rebuild. The selection
/// toggle patches the already-pushed model in place instead, so a selection change never
/// triggers a full rebuild.
pub(in crate::gui::editor) fn apply_multi_selection(
    rows: &mut [EditorTierItem],
    multi_selected: &std::collections::BTreeSet<usize>,
) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// Pushes `EditorModel.multi_selected_count`, the tier table's "N selected" header
/// indicator (`editor_tier_table.slint`). Unlike [`apply_multi_selection`] it touches a
/// Slint global, so it must run on the UI thread.
pub(in crate::gui::editor) fn push_multi_selected_count(ui: &MainWindow, count: usize) {
    ui.global::<crate::EditorModel>()
        .set_multi_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
}

/// Pushes `rows` into `EditorModel.tiers`. With an unchanged row count the existing model
/// is updated row by row ([`slint::Model::set_row_data`]). Slint rebuilds a `for` loop's
/// row components only when the model itself is replaced, so this keeps keyboard focus
/// in an open inline angle edit. A changed count replaces the model.
///
/// Invokes `EditorModel.recompute_dirty` itself: the `changed tiers` watcher in
/// `editor.slint` fires only when the property is reassigned, not for in-place row
/// updates.
pub(in crate::gui::editor) fn push_tiers(ui: &MainWindow, mut rows: Vec<EditorTierItem>) {
    // Tooltip-only text, and the built-in Slint Tooltip cannot wrap: break it here, after
    // the row builders, whose output stays plain.
    for row in &mut rows {
        if !row.strategy_detail.is_empty() {
            row.strategy_detail =
                wrap_tooltip_text(&row.strategy_detail, TOOLTIP_LINE_CHARS).into();
        }
    }
    push_rows(
        &ui.global::<crate::EditorModel>().get_tiers(),
        rows,
        |model| {
            ui.global::<crate::EditorModel>().set_tiers(model);
        },
    );
    ui.global::<crate::EditorModel>().invoke_recompute_dirty();
}

/// The generic form of [`push_tiers`]'s update: reuses `current` through
/// [`slint::Model::set_row_data`] when its row count matches `rows`, and calls `set` with
/// a new `ModelRc` only when the length changed. `set` is never called on the reuse path,
/// so the `changed tiers` watcher does not fire for a plain edit.
///
/// Callers decide what to push; this decides only how it reaches `EditorModel`. The
/// cutting rows, manufacturability warnings and selected tier chips in the view use it
/// too (`push_stale_content`, `push_manufacturability_and_preform_scratch`,
/// `push_selected_tier_chips`).
pub(in crate::gui::editor) fn push_rows<T: Clone + 'static>(
    current: &ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() == rows.len() {
        for (index, row) in rows.into_iter().enumerate() {
            current.set_row_data(index, row);
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

#[cfg(test)]
mod concave_row_tests {
    use super::*;

    fn row(kind: TierRowKind, index: i32, name: &str) -> TierRow {
        TierRow {
            index,
            name: name.to_owned(),
            kind,
            tool_line: if kind == TierRowKind::Concave {
                "CYL  +10.00\u{b0}".to_owned()
            } else {
                String::new()
            },
            ..TierRow::default()
        }
    }

    #[test]
    fn a_concave_row_carries_its_table_position_so_it_never_collides_with_a_flat_tier() {
        let rows = vec![
            row(TierRowKind::Flat, 0, "P1"),
            row(TierRowKind::Flat, 1, "C1"),
            row(TierRowKind::Concave, 0, "Groove"),
            row(TierRowKind::Concave, 1, "Dimple"),
        ];
        let items = tier_items_from_rows(rows);
        let positions: Vec<i32> = items.iter().map(|item| item.index).collect();
        assert_eq!(positions, [0, 1, 2, 3], "one position per table row");
        let kinds: Vec<i32> = items.iter().map(|item| item.kind).collect();
        assert_eq!(kinds, [0, 0, 1, 1]);
        assert_eq!(items[2].tool_line.as_str(), "CYL  +10.00\u{b0}");
        assert_eq!(items[0].tool_line.as_str(), "");
    }

    /// The table's code column reads `EditorTierItem::code`, for flat and concave rows alike.
    #[test]
    fn a_rows_code_reaches_the_table_item() {
        let mut flat = row(TierRowKind::Flat, 0, "Pavilion Main");
        flat.code = "P1".to_owned();
        let mut concave = row(TierRowKind::Concave, 0, "Groove");
        concave.code = "P3".to_owned();
        let items = tier_items_from_rows(vec![flat, concave]);
        assert_eq!(items[0].code.as_str(), "P1");
        assert_eq!(items[1].code.as_str(), "P3");
    }

    #[test]
    fn tooltip_text_breaks_at_spaces_and_wrapping_twice_changes_nothing() {
        let text = "Slow: runs locally off the UI thread and can be cancelled. \
                    Verification never edits constraints.";
        let wrapped = wrap_tooltip_text(text, 40);
        assert!(wrapped.contains('\n'));
        assert!(wrapped.lines().all(|line| line.chars().count() <= 40));
        assert_eq!(wrapped.replace('\n', " "), text);
        assert_eq!(wrap_tooltip_text(&wrapped, 40), wrapped);
        assert_eq!(wrap_tooltip_text("short", 40), "short");
        assert_eq!(wrap_tooltip_text("a\nb", 40), "a\nb");
        assert_eq!(wrap_tooltip_text("", 40), "");
        assert_eq!(
            wrap_tooltip_text("unbroken-word more", 5),
            "unbroken-word\nmore"
        );
    }

    #[test]
    fn a_planar_row_list_maps_exactly_as_before() {
        let rows = vec![
            row(TierRowKind::Flat, 0, "P1"),
            row(TierRowKind::Flat, 1, "C1"),
        ];
        let items = tier_items_from_rows(rows);
        assert_eq!(
            items.iter().map(|item| item.index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(items.iter().all(|item| item.kind == 0));
    }

    #[test]
    fn a_position_past_the_flat_tiers_names_the_concave_tier_and_nothing_else_does() {
        // Two flat tiers, two concave ones: positions 2 and 3 are the concave tiers.
        assert_eq!(concave_tier_index(2, 2, 2), Some(0));
        assert_eq!(concave_tier_index(2, 2, 3), Some(1));
        assert_eq!(concave_tier_index(2, 2, 1), None, "a flat position");
        assert_eq!(concave_tier_index(2, 2, 4), None, "beyond the table");
        assert_eq!(concave_tier_index(2, 2, -1), None, "no selection");
        assert_eq!(
            concave_tier_index(2, 0, 2),
            None,
            "a planar design has none"
        );
    }
}
