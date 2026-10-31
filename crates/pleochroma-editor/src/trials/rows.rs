//! The rows of the Trials table: figures, change against the Baseline, best marks.

use super::{TrialList, TrialSort};
use crate::{
    live_deltas::{DeltaFigure, DeltaTone, figure_deltas, signed},
    sweep::RowMetrics,
};

/// Two figures closer than this read the same at the table's one decimal, so both are the
/// best when they tie for it.
const TIE_TOLERANCE: f64 = 0.005;

/// In which figures one trial is the best of the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BestMarks([bool; 10]);

impl BestMarks {
    /// Whether the trial is the best in `figure`.
    #[must_use]
    pub const fn get(self, figure: DeltaFigure) -> bool {
        self.0[figure.index()]
    }
}

/// For every entry, in which figures it is the best of the list.
///
/// Only entries with the figure compete. An entry is best when it is within 0.005 of the
/// best value, and a figure that is the same in every entry marks none: nothing stands out
/// in it (the rule of the sweep table's best marks).
#[must_use]
pub fn best_marks(metrics: &[Option<RowMetrics>]) -> Vec<BestMarks> {
    let mut marks = vec![BestMarks::default(); metrics.len()];
    for figure in DeltaFigure::ALL {
        let values: Vec<Option<f64>> = metrics
            .iter()
            .map(|entry| entry.as_ref().and_then(|m| figure.value(m)))
            .collect();
        let (low, high) = values
            .iter()
            .flatten()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), &value| {
                (low.min(value), high.max(value))
            });
        if high - low <= TIE_TOLERANCE {
            continue;
        }
        let best = if figure.higher_is_better() { high } else { low };
        for (mark, value) in marks.iter_mut().zip(&values) {
            if value.is_some_and(|value| (value - best).abs() <= TIE_TOLERANCE) {
                mark.0[figure.index()] = true;
            }
        }
    }
    marks
}

/// One figure of one trial, as a table cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialCell {
    /// The figure.
    pub figure: DeltaFigure,
    /// The trial's value: one decimal, the warning count whole.
    pub text: String,
    /// The signed change against the Baseline (`+1.8`, `−3.0`); empty without a
    /// Baseline or where the Baseline lacks the figure.
    pub change_text: String,
    /// Better, worse or about the same as the Baseline; `Same` where there is nothing to
    /// compare.
    pub tone: DeltaTone,
    /// Whether the trial is the best of the list in this figure.
    pub best: bool,
    /// The tooltip of the change; empty where there is no change to explain.
    pub tooltip: String,
}

/// One row of the Trials table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialRowView {
    /// The trial's number.
    pub number: u32,
    /// The automatic label.
    pub label: String,
    /// The cutter's note.
    pub note: String,
    /// The origin's name.
    pub origin: &'static str,
    /// Whether the trial is starred.
    pub starred: bool,
    /// The material the figures were scored in.
    pub material_key: String,
    /// Whether the trial has scored figures at all.
    pub has_metrics: bool,
    /// The figure cells, in the order of [`DeltaFigure::ALL`]; the figures the trial lacks
    /// (a tilt average that was not measured) have no cell.
    pub cells: Vec<TrialCell>,
}

/// The table rows in trial-number order; see [`trial_rows_sorted`].
#[must_use]
pub fn trial_rows(list: &TrialList, baseline: Option<&RowMetrics>) -> Vec<TrialRowView> {
    trial_rows_sorted(list, baseline, TrialSort::Number)
}

/// The table rows in the order of `sort`.
///
/// Each figure is judged against `baseline` by [`figure_deltas`] (so the thresholds are the
/// Live Deltas'), with the best of every column marked over all the list's trials.
#[must_use]
pub fn trial_rows_sorted(
    list: &TrialList,
    baseline: Option<&RowMetrics>,
    sort: TrialSort,
) -> Vec<TrialRowView> {
    let metrics: Vec<Option<RowMetrics>> = list.iter().map(|trial| trial.metrics).collect();
    let marks: Vec<(u32, BestMarks)> = list
        .iter()
        .map(|trial| trial.number)
        .zip(best_marks(&metrics))
        .collect();
    list.sort_by(sort)
        .into_iter()
        .map(|trial| {
            let best = marks
                .iter()
                .find(|(number, _)| *number == trial.number)
                .map(|&(_, marks)| marks)
                .unwrap_or_default();
            let deltas = trial
                .metrics
                .as_ref()
                .zip(baseline)
                .map(|(now, was)| figure_deltas(was, now))
                .unwrap_or_default();
            let cells = trial.metrics.as_ref().map_or_else(Vec::new, |m| {
                DeltaFigure::ALL
                    .iter()
                    .filter_map(|&figure| {
                        let value = figure.value(m)?;
                        let delta = deltas.iter().find(|delta| delta.figure == figure);
                        Some(TrialCell {
                            figure,
                            text: figure.cell_text(value),
                            change_text: delta.map_or_else(String::new, |delta| {
                                signed(delta.change, usize::from(figure != DeltaFigure::Warnings))
                            }),
                            tone: delta.map_or(DeltaTone::Same, |delta| delta.tone),
                            best: best.get(figure),
                            tooltip: delta.map_or_else(String::new, |delta| delta.tooltip.clone()),
                        })
                    })
                    .collect()
            });
            TrialRowView {
                number: trial.number,
                label: trial.label.clone(),
                note: trial.note.clone(),
                origin: trial.origin.label(),
                starred: trial.starred,
                material_key: trial.material_key.clone(),
                has_metrics: trial.metrics.is_some(),
                cells,
            }
        })
        .collect()
}
