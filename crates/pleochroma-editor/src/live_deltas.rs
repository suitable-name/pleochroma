//! Live Deltas: every figure of the current design next to the Baseline, with its signed
//! change and whether the change is better, worse or lost in the noise.
//!
//! [`figure_deltas`] takes two [`RowMetrics`] (the scored figures of the Baseline and of the
//! design now; the score itself is the sweep's) and returns one [`FigureDelta`] per figure,
//! ready for the strip that reads "Brilliance 21.0 % +1.8".
//!
//! The judging is [`crate::metric_deltas::judge`], so the strip, the compare window and
//! the trials table agree on what "about the same" means:
//!
//! - the five table-up figures use their own noise thresholds, fire its relative one;
//! - the yield uses [`YIELD_NOISE_POINTS`] (higher is better);
//! - the manufacturability count has no threshold: a facet that appears or vanishes is
//!   always reported (fewer is better);
//! - the tilt averages use the tilt threshold, and only when both sides carry them.

use crate::{
    metric_deltas::{DeltaMetric, OpticalFigures, Tone, YIELD_NOISE_POINTS, judge},
    sweep::RowMetrics,
};

/// The tooltip of a figure that moved less than its threshold.
const SAME_TOOLTIP: &str = "Within the noise threshold, about the same.";

/// The tooltip of a warning count that did not change.
const SAME_WARNINGS_TOOLTIP: &str = "The same number of warnings.";

/// One figure of a [`RowMetrics`] that a delta is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaFigure {
    /// Table-up brilliance.
    Brilliance,
    /// Table-up windowing.
    Windowing,
    /// Table-up extinction.
    Extinction,
    /// The fire index.
    Fire,
    /// Scintillation.
    Scintillation,
    /// The yield from the rough.
    Yield,
    /// The manufacturability warning count.
    Warnings,
    /// Tilt-averaged brilliance.
    TiltBrilliance,
    /// Tilt-averaged windowing.
    TiltWindowing,
    /// Tilt-averaged extinction.
    TiltExtinction,
}

impl DeltaFigure {
    /// The name of the figure.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brilliance => "Brilliance",
            Self::Windowing => "Windowing",
            Self::Extinction => "Extinction",
            Self::Fire => "Fire",
            Self::Scintillation => "Scintillation",
            Self::Yield => "Yield",
            Self::Warnings => "Warnings",
            Self::TiltBrilliance => "Tilt Brilliance",
            Self::TiltWindowing => "Tilt Windowing",
            Self::TiltExtinction => "Tilt Extinction",
        }
    }

    /// A short name for a narrow strip.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Brilliance => "Bril.",
            Self::Windowing => "Wind.",
            Self::Extinction => "Ext.",
            Self::Fire => "Fire",
            Self::Scintillation => "Scint.",
            Self::Yield => "Yield",
            Self::Warnings => "Warn.",
            Self::TiltBrilliance => "Tilt Bril.",
            Self::TiltWindowing => "Tilt Wind.",
            Self::TiltExtinction => "Tilt Ext.",
        }
    }

    /// Whether the figure is a percentage (the fire index and the warning count are not).
    #[must_use]
    pub const fn is_percent(self) -> bool {
        !matches!(self, Self::Fire | Self::Warnings)
    }

    /// Whether the figure is one of the tilt averages.
    #[must_use]
    pub const fn is_tilt(self) -> bool {
        matches!(
            self,
            Self::TiltBrilliance | Self::TiltWindowing | Self::TiltExtinction
        )
    }

    /// Every figure, in the order of the strip and the trials table.
    pub const ALL: [Self; 10] = [
        Self::Brilliance,
        Self::Windowing,
        Self::Extinction,
        Self::Fire,
        Self::Scintillation,
        Self::Yield,
        Self::Warnings,
        Self::TiltBrilliance,
        Self::TiltWindowing,
        Self::TiltExtinction,
    ];

    /// The position of the figure in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Brilliance => 0,
            Self::Windowing => 1,
            Self::Extinction => 2,
            Self::Fire => 3,
            Self::Scintillation => 4,
            Self::Yield => 5,
            Self::Warnings => 6,
            Self::TiltBrilliance => 7,
            Self::TiltWindowing => 8,
            Self::TiltExtinction => 9,
        }
    }

    /// Whether a larger figure is the better one.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        matches!(
            self,
            Self::Brilliance
                | Self::Fire
                | Self::Scintillation
                | Self::Yield
                | Self::TiltBrilliance
        )
    }

    /// The figure of `metrics`; `None` for a yield or a tilt average it does not carry.
    #[must_use]
    pub fn value(self, metrics: &RowMetrics) -> Option<f64> {
        Some(match self {
            Self::Brilliance => f64::from(metrics.brilliance_pct),
            Self::Windowing => f64::from(metrics.windowing_pct),
            Self::Extinction => f64::from(metrics.extinction_pct),
            Self::Fire => f64::from(metrics.fire_index),
            Self::Scintillation => f64::from(metrics.scintillation_pct),
            Self::Yield => return metrics.yield_pct,
            Self::Warnings => f64::from(u32::try_from(metrics.warning_count).unwrap_or(u32::MAX)),
            Self::TiltBrilliance => f64::from(metrics.tilt?.brilliance_pct),
            Self::TiltWindowing => f64::from(metrics.tilt?.windowing_pct),
            Self::TiltExtinction => f64::from(metrics.tilt?.extinction_pct),
        })
    }

    /// `value` as a table cell, without the unit: one decimal, the count whole.
    #[must_use]
    pub fn cell_text(self, value: f64) -> String {
        let decimals = self.decimals();
        format!("{value:.decimals$}")
    }

    /// The decimals the figure and its change are shown with.
    const fn decimals(self) -> usize {
        match self {
            Self::Warnings => 0,
            _ => 1,
        }
    }

    /// `value` as the figure is shown: `21.0 %`, `4.1` for fire, `2` for the count.
    fn value_text(self, value: f64) -> String {
        let decimals = self.decimals();
        if self.is_percent() {
            format!("{value:.decimals$} %")
        } else {
            format!("{value:.decimals$}")
        }
    }
}

/// Whether a change helps, hurts or is lost in the noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaTone {
    /// A change for the better.
    Better,
    /// A change for the worse.
    Worse,
    /// Within the noise threshold: about the same.
    Same,
}

impl DeltaTone {
    /// The number the Slint side uses for the color: 0 same, 1 better, 2 worse (the code
    /// of [`Tone`]).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Same => 0,
            Self::Better => 1,
            Self::Worse => 2,
        }
    }
}

impl From<Tone> for DeltaTone {
    fn from(tone: Tone) -> Self {
        match tone {
            Tone::Same => Self::Same,
            Tone::Better => Self::Better,
            Tone::Worse => Self::Worse,
        }
    }
}

/// One figure against the Baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureDelta {
    /// The figure.
    pub figure: DeltaFigure,
    /// The Baseline's value.
    pub before: f64,
    /// The current value.
    pub after: f64,
    /// Current minus Baseline, in the figure's own unit (points, index points or facets).
    pub change: f64,
    /// Better, worse or about the same.
    pub tone: DeltaTone,
    /// The strip text: the current value and the signed change, e.g. `21.0 % +1.8`.
    pub text: String,
    /// The tooltip: the Baseline and current values, or the "about the same" sentence.
    pub tooltip: String,
}

/// The change of every figure from `baseline` to `current`.
///
/// One row per figure, in the order of [`DeltaFigure`]: the five table-up figures, the
/// yield (when both sides have one), the warning count (always), then the three tilt
/// averages (when both sides have them). A figure that is not a finite number is left out.
#[must_use]
pub fn figure_deltas(baseline: &RowMetrics, current: &RowMetrics) -> Vec<FigureDelta> {
    let (before, after) = (optical(baseline), optical(current));
    let mut rows = Vec::with_capacity(10);
    for metric in DeltaMetric::ALL {
        let figure = match metric {
            DeltaMetric::Brilliance => DeltaFigure::Brilliance,
            DeltaMetric::Windowing => DeltaFigure::Windowing,
            DeltaMetric::Extinction => DeltaFigure::Extinction,
            DeltaMetric::Fire => DeltaFigure::Fire,
            DeltaMetric::Scintillation => DeltaFigure::Scintillation,
        };
        rows.extend(judged_delta(
            figure,
            metric,
            metric.value(&before),
            metric.value(&after),
        ));
    }
    if let (Some(was), Some(now)) = (baseline.yield_pct, current.yield_pct)
        && was.is_finite()
        && now.is_finite()
    {
        let tone = if (now - was).abs() <= YIELD_NOISE_POINTS {
            DeltaTone::Same
        } else if now > was {
            DeltaTone::Better
        } else {
            DeltaTone::Worse
        };
        rows.push(delta(DeltaFigure::Yield, was, now, tone));
    }
    rows.push(warnings_delta(
        baseline.warning_count,
        current.warning_count,
    ));
    if let (Some(was), Some(now)) = (baseline.tilt, current.tilt) {
        let pairs = [
            (
                DeltaFigure::TiltBrilliance,
                DeltaMetric::Brilliance,
                was.brilliance_pct,
                now.brilliance_pct,
            ),
            (
                DeltaFigure::TiltWindowing,
                DeltaMetric::Windowing,
                was.windowing_pct,
                now.windowing_pct,
            ),
            (
                DeltaFigure::TiltExtinction,
                DeltaMetric::Extinction,
                was.extinction_pct,
                now.extinction_pct,
            ),
        ];
        for (figure, metric, was, now) in pairs {
            rows.extend(judged_delta_tilt(figure, metric, was, now));
        }
    }
    rows
}

/// The five table-up figures of a row.
const fn optical(metrics: &RowMetrics) -> OpticalFigures {
    OpticalFigures {
        brilliance_pct: metrics.brilliance_pct,
        windowing_pct: metrics.windowing_pct,
        extinction_pct: metrics.extinction_pct,
        fire_index: metrics.fire_index,
        scintillation_pct: metrics.scintillation_pct,
    }
}

/// A table-up figure judged by [`judge`]; `None` for a figure that is not finite.
fn judged_delta(
    figure: DeltaFigure,
    metric: DeltaMetric,
    before: f32,
    after: f32,
) -> Option<FigureDelta> {
    let judged = judge(metric, before, after, false)?;
    Some(delta(
        figure,
        f64::from(before),
        f64::from(after),
        judged.tone.into(),
    ))
}

/// A tilt average judged by [`judge`] with the tilt threshold.
fn judged_delta_tilt(
    figure: DeltaFigure,
    metric: DeltaMetric,
    before: f32,
    after: f32,
) -> Option<FigureDelta> {
    let judged = judge(metric, before, after, true)?;
    Some(delta(
        figure,
        f64::from(before),
        f64::from(after),
        judged.tone.into(),
    ))
}

/// The warning count: any change is a change, and fewer is better.
fn warnings_delta(before: usize, after: usize) -> FigureDelta {
    let count = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
    let tone = match after.cmp(&before) {
        std::cmp::Ordering::Equal => DeltaTone::Same,
        std::cmp::Ordering::Less => DeltaTone::Better,
        std::cmp::Ordering::Greater => DeltaTone::Worse,
    };
    let mut row = delta(DeltaFigure::Warnings, count(before), count(after), tone);
    if tone == DeltaTone::Same {
        SAME_WARNINGS_TOOLTIP.clone_into(&mut row.tooltip);
    }
    row
}

/// A [`FigureDelta`] with its texts.
fn delta(figure: DeltaFigure, before: f64, after: f64, tone: DeltaTone) -> FigureDelta {
    let change = after - before;
    let text = format!(
        "{} {}",
        figure.value_text(after),
        signed(change, figure.decimals())
    );
    let tooltip = if tone == DeltaTone::Same {
        SAME_TOOLTIP.to_owned()
    } else {
        format!(
            "{}: Baseline {}, now {}.",
            figure.label(),
            figure.value_text(before),
            figure.value_text(after)
        )
    };
    FigureDelta {
        figure,
        before,
        after,
        change,
        tone,
        text,
        tooltip,
    }
}

/// `value` with an explicit sign and `decimals` decimals, a true minus sign (U+2212) for
/// a negative one; a value that rounds to zero carries no sign.
#[must_use]
pub fn signed(value: f64, decimals: usize) -> String {
    let magnitude = format!("{:.decimals$}", value.abs());
    if magnitude.chars().all(|c| c == '0' || c == '.') {
        magnitude
    } else if value < 0.0 {
        format!("\u{2212}{magnitude}")
    } else {
        format!("+{magnitude}")
    }
}

#[cfg(test)]
mod tests;
