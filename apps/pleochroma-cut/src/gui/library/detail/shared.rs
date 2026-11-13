//! Helpers shared by the local and remote detail-load paths.
//!
//! They cover angle-side classification, the filtered row count, display formatting, the
//! material guess and clearing the detail pane.

use crate::{
    AngleItem, DiagramDetailData, LibraryModel, MainWindow,
    gui::editor::material_lookup::material_for_refractive_index,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The angle (degrees) that marks a girdle row.
///
/// A row counts as the girdle when its angle is within [`GIRDLE_ANGLE_TOLERANCE_DEG`] of this.
/// This is an equality test, not a "steep enough" threshold: girdle rows sit at exactly 90
/// degrees, while the other near-90 facets stay below 88.73 degrees.
const GIRDLE_ANGLE_DEG: f64 = 90.0;

/// The tight tolerance that [`GIRDLE_ANGLE_DEG`] matches within.
const GIRDLE_ANGLE_TOLERANCE_DEG: f64 = 0.01;

/// Parses an angle-settings `angle` text, stripping a trailing degree sign first.
///
/// The vault crate's equivalent is private, so the logic is repeated here. U+FFFD is also
/// stripped, since some stored rows use it in place of the degree sign. `None` if the text
/// still does not parse.
pub(super) fn parse_catalogue_angle_deg(angle: &str) -> Option<f64> {
    angle
        .trim()
        .trim_end_matches(['\u{b0}', '\u{fffd}'])
        .trim()
        .parse()
        .ok()
}

/// Case-insensitive, trim-insensitive equality against a lowercase `target`.
fn eq_ignore_case_trim(text: &str, target: &str) -> bool {
    text.trim().eq_ignore_ascii_case(target)
}

/// Matches a table facet label: `T` or `Table` in any case, with an optional trailing `.`.
fn is_table_facet(facet: &str) -> bool {
    let f = facet.trim();
    let core = f.strip_suffix('.').unwrap_or(f);
    core.eq_ignore_ascii_case("t") || core.eq_ignore_ascii_case("table")
}

/// A row is the table row when its index column or its facet label says so. Catalogue rows
/// use either convention.
fn is_table_row(facet: &str, index_val: &str) -> bool {
    eq_ignore_case_trim(index_val, "table") || is_table_facet(facet)
}

/// Matches a pavilion-prefixed facet label: `P`/`p`, optionally `F`/`f`, then a digit
/// (`P1`, `PF1`, `pf3`, `P2(G)` -- only the prefix has to match).
fn is_pavilion_prefixed(facet: &str) -> bool {
    let mut chars = facet.trim().chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.eq_ignore_ascii_case(&'p') {
        return false;
    }
    let mut next = chars.next();
    if next.is_some_and(|c| c.eq_ignore_ascii_case(&'f')) {
        next = chars.next();
    }
    next.is_some_and(|c| c.is_ascii_digit())
}

/// Matches a crown-prefixed facet label: `C`/`c` then a digit (`C1`, `C1A`).
fn is_crown_prefixed(facet: &str) -> bool {
    let mut chars = facet.trim().chars();
    chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&'c'))
        && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// Matches digits with a single trailing `G`/`g` (`1G`, `2G`), which tag a girdle-adjacent facet.
///
/// Checked before [`is_digits_with_optional_trailing_letter`], so `1G` reads as girdle.
fn is_digit_with_trailing_girdle_letter(facet: &str) -> bool {
    let f = facet.trim();
    match f.chars().next_back() {
        Some(last) if last.eq_ignore_ascii_case(&'g') => {
            let digits = &f[..f.len() - last.len_utf8()];
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Matches a facet label starting with `G`/`g` (`G`, `G1`, `G2`, `GIRDLE`).
fn starts_with_girdle_letter(facet: &str) -> bool {
    facet
        .trim()
        .chars()
        .next()
        .is_some_and(|c| c.eq_ignore_ascii_case(&'g'))
}

/// Matches pure digits with an optional single trailing letter (`1`, `21`, `1A`,
/// `6B`) -- the catalogue's numbered-pavilion-facet convention.
fn is_digits_with_optional_trailing_letter(facet: &str) -> bool {
    let f = facet.trim();
    let digits = match f.chars().next_back() {
        Some(c) if c.is_ascii_alphabetic() => &f[..f.len() - c.len_utf8()],
        _ => f,
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Matches a single lettered facet label other than `G`/`T` (`A`, `b`, `F`) -- the
/// catalogue's lettered-crown-facet convention.
fn is_single_crown_letter(facet: &str) -> bool {
    let f = facet.trim();
    let mut chars = f.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && c.is_ascii_alphabetic()
        && !c.eq_ignore_ascii_case(&'g')
        && !c.eq_ignore_ascii_case(&'t')
}

/// Classifies one row from its own label, with no knowledge of other rows.
///
/// `None` means the label alone is not enough, and [`sides_from_rows`] falls back to
/// [`positional_fallback`]. The order of the checks matters: girdle by angle or index text,
/// then table, pavilion prefix, crown prefix, digit with trailing `G`, girdle prefix, plain
/// digits (pavilion), and finally a lone crown letter.
fn label_side(facet: &str, angle_deg: Option<f64>, index_val: &str) -> Option<i32> {
    if angle_deg.is_some_and(|d| (d - GIRDLE_ANGLE_DEG).abs() < GIRDLE_ANGLE_TOLERANCE_DEG)
        || eq_ignore_case_trim(index_val, "girdle")
    {
        return Some(0);
    }
    if is_table_row(facet, index_val) {
        return Some(1);
    }
    if is_pavilion_prefixed(facet) {
        return Some(-1);
    }
    if is_crown_prefixed(facet) {
        return Some(1);
    }
    if is_digit_with_trailing_girdle_letter(facet) {
        return Some(0);
    }
    if starts_with_girdle_letter(facet) {
        return Some(0);
    }
    if is_digits_with_optional_trailing_letter(facet) {
        return Some(-1);
    }
    if is_single_crown_letter(facet) {
        return Some(1);
    }
    None
}

/// The positional fallback for a row [`label_side`] could not classify. It anchors on the
/// girdle rows (`girdle_positions`) and on the first table row (`table_position`).
///
/// A row inside the girdle block reports `0`. Outside it, the table row fixes the side:
/// rows on the same side of the girdle as the table are crown (`1`), the others pavilion
/// (`-1`). Many designs list the girdle first, so a plain before/after-girdle rule would
/// misread real crown facets. With no table row, before the girdle is crown and after it
/// is pavilion, but only when the girdle is neither the first nor the last row. Otherwise
/// the row reports `0`.
const fn positional_fallback(
    index: usize,
    girdle_positions: &[usize],
    table_position: Option<usize>,
    row_count: usize,
) -> i32 {
    let (Some(&first), Some(&last)) = (girdle_positions.first(), girdle_positions.last()) else {
        return 0;
    };
    if index >= first && index <= last {
        return 0;
    }
    let row_after_girdle = index > last;
    match table_position {
        Some(table_idx) if table_idx < first || table_idx > last => {
            let table_after_girdle = table_idx > last;
            if row_after_girdle == table_after_girdle {
                1
            } else {
                -1
            }
        }
        Some(_) => 0,
        None => {
            let girdle_is_first_or_last = first == 0 || last == row_count - 1;
            if girdle_is_first_or_last {
                0
            } else if row_after_girdle {
                -1
            } else {
                1
            }
        }
    }
}

/// Derives every row's [`AngleItem::side`]: `-1` pavilion, `1` crown, `0` girdle or unclassified.
///
/// Each row's own label decides first. Schedule position is used only when the label does not
/// say (see [`label_side`] and [`positional_fallback`]).
///
/// A purely positional rule does not work here, because the girdle row is often first.
/// Pure digits with an optional trailing letter, or a `P`/`PF` prefix, mean pavilion. A
/// single letter, a `C` prefix or the table row mean crown. A `G` prefix or an angle of
/// [`GIRDLE_ANGLE_DEG`] mean the girdle. A row that cannot be placed reports `0`: it shows
/// under All and is excluded from the Pavilion and Crown filters.
pub(super) fn sides_from_rows<'a>(
    rows: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) -> Vec<i32> {
    let rows: Vec<(&str, &str, &str)> = rows.collect();
    let provisional: Vec<Option<i32>> = rows
        .iter()
        .map(|&(facet, angle, index_val)| {
            label_side(facet, parse_catalogue_angle_deg(angle), index_val)
        })
        .collect();

    let girdle_positions: Vec<usize> = provisional
        .iter()
        .enumerate()
        .filter(|&(_, side)| *side == Some(0))
        .map(|(i, _)| i)
        .collect();
    let table_position =
        rows.iter()
            .zip(provisional.iter())
            .position(|(&(facet, _, index_val), side)| {
                *side == Some(1) && is_table_row(facet, index_val)
            });

    let row_count = rows.len();
    provisional
        .iter()
        .enumerate()
        .map(|(i, side)| {
            side.unwrap_or_else(|| {
                positional_fallback(i, &girdle_positions, table_position, row_count)
            })
        })
        .collect()
}

/// Which block a catalogue row numbers itself in, for [`standard_codes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeBlock {
    Pavilion,
    Crown,
    Girdle,
}

impl CodeBlock {
    /// The letter of the standard code (`P1`, `C1`, `G1`).
    const fn letter(self) -> char {
        match self {
            Self::Pavilion => 'P',
            Self::Crown => 'C',
            Self::Girdle => 'G',
        }
    }
}

/// The block of a row: its [`AngleItem::side`] says pavilion or crown; a row on neither side
/// is the girdle only when its own label or index column says so (an old-style label, a `G`
/// label, an index of "girdle"), so an unplaceable row is never numbered as a girdle facet.
fn code_block(label: &str, index_val: &str, side: i32) -> Option<CodeBlock> {
    match side.cmp(&0) {
        std::cmp::Ordering::Less => Some(CodeBlock::Pavilion),
        std::cmp::Ordering::Greater => Some(CodeBlock::Crown),
        std::cmp::Ordering::Equal => (pleochroma_cut_core::is_legacy_123_abc(label)
            || starts_with_girdle_letter(label)
            || eq_ignore_case_trim(index_val, "girdle"))
        .then_some(CodeBlock::Girdle),
    }
}

/// The label a person reads for each catalogue row.
///
/// Rows with the old 123/ABC labels get the standard code (`P1`, `G1`, `C1`, `T`). Other rows
/// keep the catalogue's own label.
///
/// `rows` is each row's `(facet label, index column)` in schedule order and `sides` the
/// matching [`sides_from_rows`] result. An old-style label (`1`, `3A`, `B`; see
/// `pleochroma_cut_core::is_legacy_123_abc`) is numbered the way `compute_tier_labels`
/// numbers a design's tiers: per block, in the listed order, counting every row of the block
/// (a row already labelled `P1` takes its place in the count). So `1`, `2` (a girdle row), `3`,
/// `A`, `B` become `P1`, `G1`, `P2`, `C1`, `C2`. The table is `T`. A row that already follows
/// the standard (`P1`, `C1`, `G1`, `PF1`, `Table`) keeps its label exactly, and so does a row
/// no block can be told for. A row with no label at all, on a known side, gets its code too.
pub(super) fn standard_codes(rows: &[(&str, &str)], sides: &[i32]) -> Vec<String> {
    let (mut pavilion, mut crown, mut girdle) = (0_usize, 0_usize, 0_usize);
    rows.iter()
        .zip(sides)
        .map(|(&(facet, index_val), &side)| {
            let label = facet.trim();
            if is_table_row(facet, index_val) {
                // The table is not numbered: `T` stays `T`, `Table` stays `Table`.
                return if label.eq_ignore_ascii_case("t") {
                    "T".to_string()
                } else {
                    facet.to_string()
                };
            }
            let Some(block) = code_block(label, index_val, side) else {
                return facet.to_string();
            };
            // A preform (`PF`) facet has its own numbering and its own label.
            if block == CodeBlock::Pavilion && label.to_ascii_uppercase().starts_with("PF") {
                return facet.to_string();
            }
            let counter = match block {
                CodeBlock::Pavilion => &mut pavilion,
                CodeBlock::Crown => &mut crown,
                CodeBlock::Girdle => &mut girdle,
            };
            *counter += 1;
            if label.is_empty() || pleochroma_cut_core::is_legacy_123_abc(label) {
                format!("{}{}", block.letter(), *counter)
            } else {
                facet.to_string()
            }
        })
        .collect()
}

/// Whether a row with this [`AngleItem::side`] is shown under `mode` (0 = All, 1 = Pavilion,
/// 2 = Crown).
///
/// This matches `cutting_table.slint`'s `row_shown` predicate. The row counts and the keyboard
/// cursor share it, so they cannot disagree.
#[must_use]
pub(super) const fn side_shown_in_mode(side: i32, mode: i32) -> bool {
    match mode {
        1 => side < 0,
        2 => side > 0,
        _ => true,
    }
}

/// Counts `angles` under `mode` (0 = All, 1 = Pavilion, 2 = Crown) by [`AngleItem::side`].
///
/// It uses [`side_shown_in_mode`], which keeps [`setup_filtered_row_count_callback`] thin.
#[must_use]
pub(super) fn count_angles_for_mode(angles: &ModelRc<AngleItem>, mode: i32) -> i32 {
    let count = angles
        .iter()
        .filter(|a| side_shown_in_mode(a.side, mode))
        .count();
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Registers `LibraryModel::filtered_row_count`, the per-mode count behind the cutting table's
/// "All Steps (N)", Pavilion and Crown pills.
///
/// Slint cannot compute it, since it has no general loop or array filter. The callback reads
/// `current_angles` on each call, so the count stays current.
pub fn setup_filtered_row_count_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_filtered_row_count(move |mode: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return 0;
            };
            let angles = ui.global::<LibraryModel>().get_current_angles();
            count_angles_for_mode(&angles, mode)
        });
    // The keyboard cursor is wired here too: it reads the same `side` filter.
    super::row_cursor::setup_row_cursor_callback(ui);
}

/// Clears the detail pane and drops the row selection.
///
/// Called when the selected design is deleted, and when a search refresh leaves it off the
/// visible list, so the selection never points at a row that is not shown. The 3D viewport is
/// left alone, because the design itself has not changed.
pub fn clear_current_detail_display(ui: &MainWindow) {
    ui.global::<LibraryModel>()
        .set_current_detail(DiagramDetailData {
            id: -1,
            title: SharedString::default(),
            url: SharedString::default(),
            designer: SharedString::default(),
            shape: SharedString::default(),
            gear: SharedString::default(),
            facets: SharedString::default(),
            lw_ratio: SharedString::default(),
            ri: SharedString::default(),
            material_guess: SharedString::default(),
            volume: SharedString::default(),
            competition: SharedString::default(),
            image_name: SharedString::default(),
            has_image: false,
            is_local: false,
            hw_ratio: SharedString::default(),
            cw_ratio: SharedString::default(),
            pw_ratio: SharedString::default(),
            symmetry_order: SharedString::default(),
            mirror_symmetry: false,
        });
    ui.global::<LibraryModel>().set_selected_entry_id(-1);
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(Vec::new())));
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(Vec::new())));
}

/// Formats a stored proportion to 3 decimal places for the detail header's chips.
///
/// This is display only: the database keeps the full text.
///
/// Only a value that parses as a finite `f64` is reformatted. Anything else passes through
/// unchanged, so it never shows as a misleading "0.000".
fn format_proportion(raw: &str) -> String {
    match raw.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => format!("{n:.3}"),
        _ => raw.to_string(),
    }
}

/// [`format_proportion`] over an `Option<&str>`, collapsing `None` to `""` -- the same
/// "empty string hides the chip" convention every other optional field on
/// `DiagramDetailData` already uses (see `detail_header.slint`'s `if root.detail.xxx
/// != "":` chips).
pub(super) fn format_optional_proportion(raw: Option<&str>) -> String {
    raw.map(format_proportion).unwrap_or_default()
}

/// Trims `text`, and maps a blank result to `None`. A cleared metadata field is stored as
/// `None`, not as an empty string.
pub(super) fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// The material guess for [`DiagramDetailData::material_guess`], from the refractive index alone.
///
/// A catalogue row stores no material. The text matches the editor's badge ("Sapphire? (from RI
/// 1.76)"). It is empty when no built-in is within [`MATERIAL_MATCH_TOLERANCE`], which hides
/// the chip.
pub(super) fn catalogue_material_guess(ri_text: Option<&str>) -> String {
    let Some(n_d) = ri_text.and_then(|s| s.trim().parse::<f64>().ok()) else {
        return String::new();
    };
    material_for_refractive_index(n_d).map_or_else(String::new, |(name, _)| {
        format!("{name}? (from RI {n_d:.2})")
    })
}
