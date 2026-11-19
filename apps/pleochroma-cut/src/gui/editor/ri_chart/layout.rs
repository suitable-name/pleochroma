//! The chart's geometry: where an index and a figure fall in the plot, the chart lines, the
//! built-in materials and how their labels and chips are laid out. Slint-free, so it is tested
//! directly.

use pleochroma_cut_core::{MaterialCatalogue, MaterialKind};
use pleochroma_editor::ri_range::RiGridRow;
use std::fmt::Write as _;

/// The lowest index on the axis (the grid starts here).
pub(super) const RI_MIN: f64 = 1.40;

/// The highest index on the axis (the grid ends here).
pub(super) const RI_MAX: f64 = 2.70;

/// The label rows under the plot; a tick's name sits in one of them.
pub(super) const LABEL_ROWS: usize = 4;

/// Two labels in one row start at least this far apart, in pixels. A name closer than that to
/// the one before it in every row is left to the chips.
pub(super) const MIN_LABEL_START_GAP_PX: f64 = 24.0;

/// The free space between two labels of one row, in pixels.
const LABEL_GAP_PX: f64 = 6.0;

/// The room a label needs right of its tick before its text starts, in pixels.
const LABEL_INDENT_PX: f64 = 3.0;

/// The width of one character of a tick label (9 px text), in pixels.
const LABEL_CHAR_PX: f64 = 5.0;

/// The width of one character of a chip (11 px bold text), in pixels.
const CHIP_CHAR_PX: f64 = 6.6;

/// The room a chip needs besides its text, in pixels.
const CHIP_PAD_PX: f64 = 20.0;

/// The gap between two chips of a row, in pixels.
pub(super) const CHIP_SPACING_PX: f64 = 6.0;

/// A built-in material on the axis.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BuiltIn {
    /// The material's name.
    pub(super) name: String,
    /// Its refractive index at the sodium D line.
    pub(super) n: f64,
}

/// The built-in materials whose index is on the axis, by rising index. A custom material is not
/// among them: the chart draws no tick for it.
pub(super) fn built_in_materials() -> Vec<BuiltIn> {
    let mut materials: Vec<BuiltIn> = MaterialCatalogue::build(&[])
        .entries()
        .iter()
        .filter(|entry| entry.kind == MaterialKind::BuiltIn)
        .filter(|entry| (RI_MIN..=RI_MAX).contains(&entry.ri_d))
        .map(|entry| BuiltIn {
            name: entry.name.clone(),
            n: entry.ri_d,
        })
        .collect();
    materials.sort_by(|a, b| a.n.total_cmp(&b.n));
    materials
}

/// An index as a percentage of the axis (0 at [`RI_MIN`], 100 at [`RI_MAX`]).
pub(super) const fn x_percent(n: f64) -> f64 {
    ((n - RI_MIN) / (RI_MAX - RI_MIN) * 100.0).clamp(0.0, 100.0)
}

/// A figure in percent as a height from the top of the plot (0 at the top, 100 at the bottom).
pub(super) const fn y_percent(figure_pct: f64) -> f64 {
    100.0 - figure_pct.clamp(0.0, 100.0)
}

/// The refractive index as the chart prints it: three decimals with the trailing zeros dropped
/// (`1.654`, `1.79`, `2`).
pub(super) fn ri_text(n: f64) -> String {
    let text = format!("{n:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// One chart line as SVG path data in the 0..100 viewbox: the figure of each grid row over the
/// index. Empty for fewer than two rows.
pub(super) fn line_commands(rows: &[RiGridRow], figure: impl Fn(&RiGridRow) -> f32) -> String {
    if rows.len() < 2 {
        return String::new();
    }
    let mut out = String::new();
    for (index, row) in rows.iter().enumerate() {
        let command = if index == 0 { 'M' } else { 'L' };
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(
            out,
            "{command} {:.2} {:.2}",
            x_percent(row.n),
            y_percent(f64::from(figure(row)))
        );
    }
    out
}

/// The grid row nearest to `n`, for the figures printed under the chart.
pub(super) fn nearest_row(rows: &[RiGridRow], n: f64) -> Option<&RiGridRow> {
    rows.iter()
        .min_by(|a, b| (a.n - n).abs().total_cmp(&(b.n - n).abs()))
}

/// The label row of each tick, or `None` for a name folded into the chips.
///
/// `ticks` are `(x in pixels, name length in characters)` by rising x. A name goes to the first
/// row where it clears the previous name by [`LABEL_GAP_PX`] and starts at least
/// [`MIN_LABEL_START_GAP_PX`] after it; when no row has room it is folded.
pub(super) fn label_rows(ticks: &[(f64, usize)]) -> Vec<Option<usize>> {
    // Per row: where the last label starts and where it ends.
    let mut last: [Option<(f64, f64)>; LABEL_ROWS] = [None; LABEL_ROWS];
    ticks
        .iter()
        .map(|&(x, chars)| {
            let start = x + LABEL_INDENT_PX;
            let end = start + count_f64(chars) * LABEL_CHAR_PX;
            let row = last.iter().position(|slot| {
                slot.is_none_or(|(prev_start, prev_end)| {
                    start >= prev_end + LABEL_GAP_PX && start >= prev_start + MIN_LABEL_START_GAP_PX
                })
            })?;
            last[row] = Some((start, end));
            Some(row)
        })
        .collect()
}

/// The width of a chip with `text`, in pixels (an estimate: the layout wraps by it).
pub(super) fn chip_width_px(text: &str) -> f64 {
    count_f64(text.chars().count()) * CHIP_CHAR_PX + CHIP_PAD_PX
}

/// A small count as a float (character counts are far below 2^52).
#[expect(
    clippy::cast_precision_loss,
    reason = "a character count is far below the range where f64 loses integers"
)]
const fn count_f64(count: usize) -> f64 {
    count as f64
}

/// Wraps items of the given widths into rows of at most `available_px` (every row holds at
/// least one item). Returns each row's item indices.
pub(super) fn wrap_rows(widths: &[f64], available_px: f64) -> Vec<Vec<usize>> {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (index, &width) in widths.iter().enumerate() {
        let fits = used > 0.0 && used + CHIP_SPACING_PX + width <= available_px;
        if fits && let Some(row) = rows.last_mut() {
            row.push(index);
            used += CHIP_SPACING_PX + width;
        } else {
            rows.push(vec![index]);
            used = width;
        }
    }
    rows
}

/// "A", "A and B", "A, B and C"; with more than `cap` names the first `cap - 1` and "and N more".
pub(super) fn join_names(names: &[String], cap: usize) -> String {
    let cap = cap.max(2);
    if names.len() > cap {
        let shown = &names[..cap - 1];
        return format!(
            "{} and {} more",
            shown.join(", "),
            names.len() - shown.len()
        );
    }
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: f64, windowing_tilt: f32) -> RiGridRow {
        RiGridRow {
            n,
            windowing_table_up: 1.0,
            windowing_tilt,
            brilliance_tilt: 50.0,
            extinction_tilt: 10.0,
        }
    }

    #[test]
    fn the_axis_runs_from_1_40_to_2_70() {
        assert!(x_percent(RI_MIN).abs() < 1e-9);
        assert!((x_percent(RI_MAX) - 100.0).abs() < 1e-9);
        assert!((x_percent(2.05) - 50.0).abs() < 1e-9);
        // Off the axis clamps.
        assert!(x_percent(1.0).abs() < 1e-9 && (x_percent(3.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_figure_is_a_height_from_the_top() {
        assert!((y_percent(0.0) - 100.0).abs() < 1e-9);
        assert!((y_percent(15.0) - 85.0).abs() < 1e-9);
        assert!(y_percent(140.0).abs() < 1e-9);
    }

    #[test]
    fn an_index_prints_without_trailing_zeros() {
        assert_eq!(ri_text(1.654), "1.654");
        assert_eq!(ri_text(1.79), "1.79");
        assert_eq!(ri_text(2.0), "2");
        assert_eq!(ri_text(1.768), "1.768");
    }

    #[test]
    fn a_line_has_a_point_per_row_on_one_percent_scale() {
        let rows = [row(1.40, 60.0), row(2.05, 10.0), row(2.70, 0.0)];
        let commands = line_commands(&rows, |row| row.windowing_tilt);
        assert_eq!(commands, "M 0.00 40.00 L 50.00 90.00 L 100.00 100.00");
        assert_eq!(line_commands(&rows[..1], |row| row.windowing_tilt), "");
    }

    #[test]
    fn the_nearest_row_is_found() {
        let rows = [row(1.40, 0.0), row(1.63, 0.0), row(1.70, 0.0)];
        assert!((nearest_row(&rows, 1.64).expect("a row").n - 1.63).abs() < 1e-9);
        assert!(nearest_row(&[], 1.5).is_none());
    }

    #[test]
    fn the_built_ins_are_sorted_named_and_inside_the_axis() {
        let materials = built_in_materials();
        assert!(materials.len() > 20, "the table has dozens of species");
        assert!(materials.windows(2).all(|pair| pair[0].n <= pair[1].n));
        assert!(
            materials
                .iter()
                .all(|material| (RI_MIN..=RI_MAX).contains(&material.n))
        );
        let sapphire = materials
            .iter()
            .find(|material| material.name == "Sapphire")
            .expect("sapphire");
        assert!((sapphire.n - 1.768).abs() < 0.01);
    }

    #[test]
    fn labels_use_the_first_free_row_and_fold_when_every_row_is_taken() {
        // Five names 10 px apart: four rows, the fifth is folded.
        let crowded: Vec<(f64, usize)> = (0..5).map(|i| (f64::from(i) * 10.0, 6)).collect();
        assert_eq!(
            label_rows(&crowded),
            vec![Some(0), Some(1), Some(2), Some(3), None]
        );
        // Far apart, they all share the first row.
        let spread: Vec<(f64, usize)> = (0..4).map(|i| (f64::from(i) * 200.0, 6)).collect();
        assert_eq!(label_rows(&spread), vec![Some(0); 4]);
    }

    #[test]
    fn a_short_name_still_starts_24_px_after_the_one_before_it() {
        // "Jet" is 15 px wide: at 21 px its label would clear the name before it by 6 px, but
        // the 24 px start gap sends it to the second row.
        let ticks = [(0.0, 3), (21.0, 3)];
        assert_eq!(label_rows(&ticks), vec![Some(0), Some(1)]);
        let ticks = [(0.0, 3), (30.0, 3)];
        assert_eq!(label_rows(&ticks), vec![Some(0), Some(0)]);
    }

    #[test]
    fn chips_wrap_to_the_available_width() {
        let widths = [100.0, 100.0, 100.0, 100.0];
        // Two per row: 100 + 6 + 100 = 206 <= 210.
        assert_eq!(wrap_rows(&widths, 210.0), vec![vec![0, 1], vec![2, 3]]);
        // A chip wider than the row still gets a row.
        assert_eq!(wrap_rows(&[300.0], 100.0), vec![vec![0]]);
        assert!(wrap_rows(&[], 100.0).is_empty());
    }

    #[test]
    fn names_join_in_plain_words() {
        let names = |list: &[&str]| list.iter().map(|&name| name.to_owned()).collect::<Vec<_>>();
        assert_eq!(join_names(&[], 6), "");
        assert_eq!(join_names(&names(&["Quartz"]), 6), "Quartz");
        assert_eq!(
            join_names(&names(&["Quartz", "Topaz"]), 6),
            "Quartz and Topaz"
        );
        assert_eq!(
            join_names(&names(&["Quartz", "Aquamarine", "Tourmaline"]), 6),
            "Quartz, Aquamarine and Tourmaline"
        );
        let seven = names(&["A", "B", "C", "D", "E", "F", "G"]);
        assert_eq!(join_names(&seven, 6), "A, B, C, D, E and 2 more");
    }
}
