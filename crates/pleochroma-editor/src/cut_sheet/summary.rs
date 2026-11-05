//! [`StoneSummary`]: the "Stone summary" of the HTML sheet's additional information as plain
//! data -- the stone's size in millimetres, its proportions, its material and refractive index,
//! and the carat estimate with how it scales with the width.
//!
//! The figures are the ones [`super::SheetLayout`] already has (Size Data and the yield report's
//! carat estimate), plus the table width and girdle thickness of
//! [`Design::stone_proportions`], the editor's own proportions readout. Nothing here reads a clock
//! or a file, so the same design always gives the same summary.

use super::layout::{NO_VALUE, SizeData, material_label, trimmed};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::Design;
use std::fmt::Write as _;

/// The caveat every carat estimate carries, as the clause the text sheet prints after the figure
/// (`4.310 ct -- assumes ...`).
///
/// The millimetre scale is anchored to the finished solid's width, so a stone still inside its
/// preform's walls weighs more than the estimate.
pub const CARAT_CAVEAT: &str = "assumes the authored facets reach the preform's own walls";

/// [`CARAT_CAVEAT`] as the sentence the Stone summary prints under the carat figure.
pub const CARAT_NOTE: &str = "Assumes the authored facets reach the preform's own walls.";

/// The label of the size line: width, length and total depth in millimetres.
pub const SIZE_LABEL: &str = "Size (W \u{d7} L \u{d7} D)";

/// The label of the carat line.
pub const CARAT_LABEL: &str = "Carat weight (estimate)";

/// One line of the Stone summary: a label, its figure and an optional caveat under the figure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRow {
    /// What the line gives (`Size (W × L × D)`, `Table`).
    pub label: &'static str,
    /// The figure (`6.50 × 6.50 × 3.93 mm`, `53.1% of width`).
    pub value: String,
    /// A caveat printed under the figure, in smaller type.
    pub note: Option<&'static str>,
}

/// A summary line without a caveat.
const fn row(label: &'static str, value: String) -> SummaryRow {
    SummaryRow {
        label,
        value,
        note: None,
    }
}

/// The Stone summary of a design.
///
/// Every figure is `None` when the design does not give it, and a `None` figure prints no line:
/// nothing is invented to fill a gap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoneSummary {
    /// The stone's width in millimetres: the design's girdle diameter, which anchors the
    /// millimetre scale of the mast table and the carat estimate.
    pub width_mm: Option<f64>,
    /// Width, length and total depth in millimetres: the girdle diameter times `1`, `L/W` and
    /// `H/W` of Size Data. `None` without a girdle diameter or when the design does not measure.
    pub outline_mm: Option<[f64; 3]>,
    /// The table's width as a percentage of the stone width (`Design::stone_proportions`).
    pub table_percent: Option<f64>,
    /// Crown height as a percentage of the width: `C/W` of Size Data.
    pub crown_percent: Option<f64>,
    /// Pavilion depth as a percentage of the width: `P/W` of Size Data.
    pub pavilion_percent: Option<f64>,
    /// Girdle thickness as a percentage of the width (`Design::stone_proportions`).
    pub girdle_percent: Option<f64>,
    /// Total depth (table to culet) as a percentage of the width: `H/W` of Size Data.
    pub depth_percent: Option<f64>,
    /// The material as Design Data names it: the design's own, else a guess marked with a
    /// question mark, else `None`.
    pub material: Option<String>,
    /// The refractive index the sheet prints: the design's effective `n_D`.
    pub refractive_index: f64,
    /// The carat estimate at [`Self::width_mm`], when the design has a girdle diameter, measures
    /// and has a specific gravity.
    pub carat_weight: Option<f64>,
}

impl StoneSummary {
    /// The summary of `design` from the figures its layout already has.
    ///
    /// Those are `sizes` (Size Data), `n_d` (the printed refractive index) and `carat_weight`
    /// (the yield report's estimate). The table and girdle percentages come from [`Design::stone_proportions`], one flat mesh of
    /// `solved` -- the readout the editor's proportions panel computes on every solve. A design
    /// with no tiers has none: its preform block is not a stone.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Design::cutting_sheet`]: `solved` must have one entry per
    /// tier `design` currently has, in the same order.
    #[must_use]
    pub fn build(
        design: &Design,
        solved: &[SolvedTier],
        sizes: &SizeData,
        n_d: f64,
        carat_weight: Option<f64>,
    ) -> Self {
        let width_mm = design
            .girdle_diameter_mm
            .filter(|mm| mm.is_finite() && *mm > 0.0);
        let outline_mm = width_mm.and_then(|width| {
            Some([
                width,
                width * sizes.length_to_width?,
                width * sizes.height_to_width?,
            ])
        });
        let proportions = if design.tiers.is_empty() {
            None
        } else {
            design.stone_proportions(solved)
        };
        let percent = |ratio: Option<f64>| ratio.map(|ratio| 100.0 * ratio);
        Self {
            width_mm,
            outline_mm,
            table_percent: proportions.and_then(|figures| figures.table_percent),
            crown_percent: percent(sizes.crown_to_width),
            pavilion_percent: percent(sizes.pavilion_to_width),
            girdle_percent: proportions.and_then(|figures| figures.girdle_to_width_percent),
            depth_percent: percent(sizes.height_to_width),
            material: material_label(design, n_d),
            refractive_index: n_d,
            carat_weight,
        }
    }

    /// The printed lines, in order.
    ///
    /// They are the size in millimetres (or the girdle diameter alone when the design does not
    /// measure), the proportions as percentages of the width, the material, the refractive index
    /// and the carat line with its caveat.
    #[must_use]
    pub fn rows(&self) -> Vec<SummaryRow> {
        let mut rows = Vec::new();
        match (self.outline_mm, self.width_mm) {
            (Some([width, length, depth]), _) => rows.push(row(
                SIZE_LABEL,
                format!("{width:.2} \u{d7} {length:.2} \u{d7} {depth:.2} mm"),
            )),
            (None, Some(width)) => rows.push(row("Girdle diameter", format!("{width:.2} mm"))),
            (None, None) => {}
        }
        for (label, percent) in [
            ("Table", self.table_percent),
            ("Crown height", self.crown_percent),
            ("Pavilion depth", self.pavilion_percent),
            ("Girdle thickness", self.girdle_percent),
            ("Total depth", self.depth_percent),
        ] {
            if let Some(percent) = percent {
                rows.push(row(label, format!("{percent:.1}% of width")));
            }
        }
        rows.push(row(
            "Material",
            self.material.as_deref().unwrap_or(NO_VALUE).to_string(),
        ));
        rows.push(row(
            "Refractive index",
            format!("{:.3}", self.refractive_index),
        ));
        if let Some(text) = self.carat_text() {
            rows.push(SummaryRow {
                label: CARAT_LABEL,
                value: text,
                note: Some(CARAT_NOTE),
            });
        }
        rows
    }

    /// The carat line, `None` without an estimate.
    ///
    /// It gives the estimate at the stone's width, then the same design at two smaller round
    /// widths (`smaller_round_sizes`), since a weight scales with the cube of the width:
    /// `4.31 ct at 10.00 mm wide; scales with the cube of the width: 2.20 ct at 8 mm, 0.93 ct at
    /// 6 mm`.
    #[must_use]
    pub fn carat_text(&self) -> Option<String> {
        let (carat, width) = (self.carat_weight?, self.width_mm?);
        let mut text = format!("{} ct at {width:.2} mm wide", carat_number(carat));
        let scaled: Vec<String> = smaller_round_sizes(width)
            .into_iter()
            .map(|size| {
                format!(
                    "{} ct at {} mm",
                    carat_number(carat * (size / width).powi(3)),
                    trimmed(size)
                )
            })
            .collect();
        if !scaled.is_empty() {
            let _ = write!(
                text,
                "; scales with the cube of the width: {}",
                scaled.join(", ")
            );
        }
        Some(text)
    }
}

/// A carat figure: two decimals, three under a tenth of a carat (where two would print a small
/// stone as `0.02` or `0.01`).
fn carat_number(carat: f64) -> String {
    if carat < 0.1 {
        format!("{carat:.3}")
    } else {
        format!("{carat:.2}")
    }
}

/// Up to two round widths smaller than `width_mm` to scale a carat estimate to.
///
/// They are about four fifths and three fifths of the width, rounded to whole millimetres (half
/// millimetres under 4 mm), each strictly smaller than the one before. 10 mm gives 8 and 6 mm,
/// 6.5 mm gives 5 and 4 mm; a stone too small to have a smaller round size gives fewer.
#[must_use]
pub(super) fn smaller_round_sizes(width_mm: f64) -> Vec<f64> {
    let step = if width_mm < 4.0 { 0.5 } else { 1.0 };
    let mut sizes: Vec<f64> = Vec::with_capacity(2);
    for fraction in [0.8, 0.6] {
        let size = (width_mm * fraction / step).round() * step;
        let below = sizes.last().copied().unwrap_or(width_mm);
        if size >= step && size < below {
            sizes.push(size);
        }
    }
    sizes
}
