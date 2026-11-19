//! Copies a built [`View`] into `RiChartModel`.

use super::{
    dash::DEFAULT_PLOT_PX,
    view::{Chip, Mark, Tick, View},
};
use crate::{MainWindow, RiChartChip, RiChartChipRow, RiChartLine, RiChartModel, RiChartTick};
use slint::{ComponentHandle, ModelRc, VecModel};

/// A position that is not on the chart (the model's "not shown").
const NOT_SHOWN: f32 = -1.0;

fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// A mark's position, or [`NOT_SHOWN`].
fn at_of(mark: Option<&Mark>) -> f32 {
    mark.map_or(NOT_SHOWN, |mark| mark.at)
}

/// A mark's words, or nothing.
fn text_of(mark: Option<&Mark>) -> slint::SharedString {
    mark.map(|mark| mark.text.as_str())
        .unwrap_or_default()
        .into()
}

/// The slint tick of a built one; a folded name has row -1.
fn tick_item(tick: &Tick) -> RiChartTick {
    RiChartTick {
        x: tick.x,
        label: tick.label.as_str().into(),
        row: tick
            .row
            .and_then(|row| i32::try_from(row).ok())
            .unwrap_or(-1),
        state: tick.state.number(),
    }
}

/// The slint row of chips.
fn chip_row(chips: &[Chip]) -> RiChartChipRow {
    RiChartChipRow {
        chips: model_of(
            chips
                .iter()
                .map(|chip| RiChartChip {
                    text: chip.text.as_str().into(),
                    current: chip.current,
                })
                .collect(),
        ),
    }
}

/// The plot's size in pixels as the chart last reported it, or the default before it has.
pub(super) fn plot_px(ui: &MainWindow) -> (f64, f64) {
    let model = ui.global::<RiChartModel>();
    let (width, height) = (model.get_plot_width(), model.get_plot_height());
    if width > 0.0 && height > 0.0 {
        (f64::from(width), f64::from(height))
    } else {
        DEFAULT_PLOT_PX
    }
}

/// Shows the range being computed (the dialog says so and draws nothing else).
pub(super) fn push_computing(ui: &MainWindow) {
    let model = ui.global::<RiChartModel>();
    model.set_ready(false);
    model.set_computing(true);
    model.set_message("".into());
    model.set_status_text("".into());
}

/// Shows that there is no range to draw, and why.
pub(super) fn push_unavailable(ui: &MainWindow, message: &str) {
    let model = ui.global::<RiChartModel>();
    model.set_ready(false);
    model.set_computing(false);
    model.set_message(message.into());
    model.set_status_text("".into());
}

/// Copies the whole view: the lines, the range marks, the ticks, the chips and the sentences.
pub(super) fn push_view(ui: &MainWindow, view: &View) {
    let model = ui.global::<RiChartModel>();
    model.set_intro_text(view.intro.as_str().into());
    model.set_lines(model_of(
        view.lines
            .iter()
            .map(|(series, commands)| RiChartLine {
                commands: commands.as_str().into(),
                series: *series,
            })
            .collect(),
    ));
    model.set_band_low_x(view.band.0);
    model.set_band_high_x(view.band.1);
    model.set_low_text(view.low.text.as_str().into());
    model.set_high_x(at_of(view.high.as_ref()));
    model.set_high_text(text_of(view.high.as_ref()));
    model.set_current_x(at_of(view.current.as_ref()));
    model.set_current_text(text_of(view.current.as_ref()));
    model.set_limit_y(view.limit.at);
    model.set_limit_text(view.limit.text.as_str().into());
    model.set_axis_low(view.axis_low.as_str().into());
    model.set_axis_high(view.axis_high.as_str().into());
    model.set_ticks(model_of(view.ticks.iter().map(tick_item).collect()));
    model.set_summary_text(view.summary.as_str().into());
    model.set_works_heading(view.works_heading.as_str().into());
    model.set_chip_rows(model_of(
        view.chip_rows.iter().map(|row| chip_row(row)).collect(),
    ));
    model.set_more_text(view.more_text.as_str().into());
    model.set_more_hint(view.more_hint.as_str().into());
    model.set_outside_text(view.outside.as_str().into());
    model.set_status_text(view.status.as_str().into());
    model.set_message("".into());
    model.set_computing(false);
    model.set_ready(true);
}
