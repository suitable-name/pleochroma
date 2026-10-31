//! Curve visibility and the multi-metric legend. This module only reads completed rows;
//! changing visibility never starts, cancels or invalidates a sweep.

use super::mini_sweep::FIGURES;
use crate::{MainWindow, TuneMetricRow, TuneModel};
use pleochroma_editor::sweep::{SweepMetric, SweepRow, chart_path, series_range_text};
use slint::{ComponentHandle as _, Model as _, ModelRc, VecModel};

/// Independent from the objective used for best-angle/plateau calculations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Visibility([bool; FIGURES.len()]);

impl Default for Visibility {
    fn default() -> Self {
        Self([true; FIGURES.len()])
    }
}

impl Visibility {
    pub(super) fn shown(self, index: usize) -> bool {
        self.0.get(index).copied().unwrap_or(false)
    }

    /// Ignore callbacks naming an unavailable metric.
    pub(super) fn toggle(&mut self, index: i32) -> bool {
        let Some(shown) = usize::try_from(index)
            .ok()
            .and_then(|index| self.0.get_mut(index))
        else {
            return false;
        };
        *shown = !*shown;
        true
    }

    pub(super) fn selected(self) -> Vec<SweepMetric> {
        FIGURES
            .iter()
            .enumerate()
            .filter_map(|(index, &(metric, _))| self.shown(index).then_some(metric))
            .collect()
    }
}

/// Real ranges/units accompany every independently scaled curve. A constant metric is
/// drawn at mid-height, as in the Angle Sweep dialog; invalid rows break the path.
pub(super) fn rows(rows: &[SweepRow], visible: Visibility) -> Vec<TuneMetricRow> {
    FIGURES
        .iter()
        .enumerate()
        .map(|(index, &(metric, _))| TuneMetricRow {
            index: i32::try_from(index).unwrap_or(-1),
            label: metric.label().into(),
            range_text: series_range_text(rows, metric).into(),
            commands: chart_path(rows, metric).into(),
            shown: visible.shown(index),
        })
        .collect()
}

pub(super) fn push(ui: &MainWindow, measured: &[SweepRow], visible: Visibility) {
    let model = ui.global::<TuneModel>();
    let updated = rows(measured, visible);
    let current = model.get_metrics();
    // Keep repeated legend controls alive so Space/Enter toggles retain keyboard focus.
    if let Some(current) = current.as_any().downcast_ref::<VecModel<TuneMetricRow>>() {
        if current.row_count() == updated.len() {
            for (index, row) in updated.into_iter().enumerate() {
                if current.row_data(index).is_none_or(|old| old != row) {
                    current.set_row_data(index, row);
                }
            }
        } else {
            current.set_vec(updated);
        }
    } else {
        model.set_metrics(ModelRc::new(VecModel::from(updated)));
    }
    model.set_no_metrics_shown(visible.selected().is_empty());
    model
        .set_objective_shown(visible.shown(usize::try_from(model.get_figure_index()).unwrap_or(0)));
}
