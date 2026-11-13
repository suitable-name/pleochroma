//! [`GirdleReading`]: how thick the girdle is, the way a cutter reads it -- at the girdle LINE.
//!
//! The solid's own girdle figure (`StoneProportions::girdle_thickness`, `measure_solid`'s band)
//! is the full height of the girdle faces, from their highest vertex to their lowest. On a
//! correctly cut faceted brilliant that height includes the V-scallops the break facets cut
//! between the girdle corners: some 5.7 % of the width on their own, fixed by the break angles.
//! Judged against a girdle window, or printed as "the girdle", it calls every such stone's girdle
//! far too thick.
//!
//! The girdle line is where [`pleochroma_cut_core::Design::girdle_level`] judges the girdle: the
//! corners where girdle faces meet and the ends of level edges. A level girdle reads one
//! thickness there; an unlevel one a range. Every user-facing girdle percentage (the proportion
//! verdict, the readouts beside it, the cutting sheet's Stone summary) reads it from here, so they
//! agree. Where there is no girdle line to read (no girdle tier, or no crown or pavilion facet
//! reaching the girdle faces) the reading falls back to the band.
//!
//! The band stays what it is wherever it is not a girdle judgement: the total depth (crown height
//! plus pavilion depth plus the band is the stone's height), volume and weight, the Retarget gate's
//! before-and-after comparison, the optimizer's guards and the `metrics` figures.

use pleochroma::geometry::{meet_solver::SolvedTier, stone_metrics::StoneProportions};
use pleochroma_cut_core::{Design, GirdleLevel};

/// How thick the girdle reads (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GirdleReading {
    /// At the girdle line ([`GirdleLevel`]).
    Line {
        /// The thinnest line thickness ([`GirdleLevel::thickness_range`]), in design units,
        /// never below zero.
        thinnest: f64,
        /// The thickest line thickness, in design units.
        thickest: f64,
        /// The stone's width, in design units ([`GirdleLevel::width`]).
        width: f64,
        /// Whether the line is level ([`GirdleLevel::is_level`]).
        level: bool,
    },
    /// The band's full height: the design has no girdle line to read.
    Band {
        /// The band's height, in design units (`StoneProportions::girdle_thickness`).
        thickness: f64,
        /// The band's height as a percentage of the width
        /// (`StoneProportions::girdle_to_width_percent`).
        percent: f64,
    },
}

impl GirdleReading {
    /// The reading of `design` solved as `solved`, whose stone measures `proportions`: at the
    /// girdle line when [`Design::girdle_level`] has one, else the band. `None` when there is
    /// neither (no live girdle face at all).
    #[must_use]
    pub fn of(
        design: &Design,
        solved: &[SolvedTier],
        proportions: &StoneProportions,
    ) -> Option<Self> {
        Self::from_level(design.girdle_level(solved).as_ref(), proportions)
    }

    /// [`Self::of`] from a girdle level already worked out (`None` when the design has none).
    #[must_use]
    pub fn from_level(level: Option<&GirdleLevel>, proportions: &StoneProportions) -> Option<Self> {
        if let Some(level) = level.filter(|level| level.width.is_finite() && level.width > 0.0) {
            let (thinnest, thickest) = level.thickness_range();
            return Some(Self::Line {
                thinnest: thinnest.max(0.0),
                thickest: thickest.max(0.0),
                width: level.width,
                level: level.is_level(),
            });
        }
        let (thickness, percent) = (
            proportions.girdle_thickness?,
            proportions.girdle_to_width_percent?,
        );
        Some(Self::Band { thickness, percent })
    }

    /// The thinnest and the thickest reading as percentages of the width (the band's one figure
    /// twice).
    #[must_use]
    pub const fn percents(self) -> (f64, f64) {
        match self {
            Self::Line {
                thinnest,
                thickest,
                width,
                ..
            } => (100.0 * thinnest / width, 100.0 * thickest / width),
            Self::Band { percent, .. } => (percent, percent),
        }
    }

    /// The figure a proportion window judges, as a percentage of the width: the thinnest line
    /// thickness, since a thin girdle is the durability risk (an unlevel line is the verdict's
    /// "girdle is not level" check), or the band.
    #[must_use]
    pub const fn judged_percent(self) -> f64 {
        self.percents().0
    }

    /// Whether the reading is one figure: a level line, or the band.
    #[must_use]
    pub const fn is_level(self) -> bool {
        match self {
            Self::Line { level, .. } => level,
            Self::Band { .. } => true,
        }
    }

    /// The reading as a percentage of the width, for a compact readout: `2.8%`, or the range of
    /// an unlevel line, `2.5–3.9%`.
    #[must_use]
    pub fn percent_text(self) -> String {
        let (thinnest, thickest) = self.percents();
        if self.is_level() {
            format!("{thinnest:.1}%")
        } else {
            format!("{thinnest:.1}\u{2013}{thickest:.1}%")
        }
    }

    /// The reading as the cutting sheet prints it: `2.8% of width`, or `2.5–3.9% of width (not
    /// level)`.
    #[must_use]
    pub fn width_text(self) -> String {
        if self.is_level() {
            format!("{} of width", self.percent_text())
        } else {
            format!("{} of width (not level)", self.percent_text())
        }
    }

    /// The reading as a length: in millimetres with `mm_per_unit` (`0.275 mm`), else in the
    /// design's own units (`0.0550`); an unlevel line as a range, `0.254–0.399 mm (not level)`.
    #[must_use]
    pub fn thickness_text(self, mm_per_unit: Option<f64>) -> String {
        let (scale, unit) = mm_per_unit.map_or((1.0, ""), |scale| (scale, " mm"));
        match self {
            Self::Line {
                thinnest,
                thickest,
                level: false,
                ..
            } => format!(
                "{:.3}\u{2013}{:.3}{unit} (not level)",
                thinnest * scale,
                thickest * scale
            ),
            Self::Line { thinnest, .. } => format!("{:.3}{unit}", thinnest * scale),
            Self::Band { thickness, .. } => format!("{:.3}{unit}", thickness * scale),
        }
    }
}

#[cfg(test)]
mod tests;
