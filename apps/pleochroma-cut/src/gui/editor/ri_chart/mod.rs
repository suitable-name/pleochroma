//! The RI chart: how windowing, brilliance and extinction hold up across refractive indices,
//! with the traced range shaded and the built-in materials as ticks
//! (`ui/components/ri_chart/`, state in `ui/models/ri_chart.slint`).
//!
//! The numbers are the traced range of the open design (`pleochroma_editor::ri_range`, computed
//! by the range worker of `gui::editor::ri_range`); this group only draws them:
//!
//! - [`layout`] -- the axis, the chart lines, the built-in materials, the label rows and the chip
//!   rows. Slint-free.
//! - [`text`] -- every sentence of the dialog. Slint-free.
//! - [`view`] -- builds everything the dialog shows from a range, in both forms of the high end
//!   (`HIGH_BOUND_CALIBRATED` false: a dashed "uncalibrated" end and the low bound alone judging
//!   the materials; true: a solid end and both bounds). Slint-free.
//! - [`dash`] -- cuts a line into dashes (`Path` has no dash attribute).
//! - [`present`] -- the pushes into `RiChartModel`.
//! - this file -- [`setup`], the callbacks and the small state the dialog keeps.
//!
//! "Queue Material Check Set" (the dialog's footer button, the material card's menu and the
//! command palette) is answered by `gui::render_jobs::check_set`; this group only sets
//! `RiChartModel.can_queue_check_set`.

mod dash;
mod layout;
mod present;
mod text;
mod view;

use self::view::Inputs;
use super::ri_range::when_ready;
use crate::{
    EditorModel, MainWindow, RiChartModel, ViewportModel, bridge::render_thread::RenderContext,
    gui::show_toast, settings::SettingsPersister,
};
use pleochroma_editor::ri_range::{HIGH_BOUND_CALIBRATED, RiRange};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::Duration,
};

/// How often the chart looks at whether a design is open (for `can_queue_check_set`).
const WATCH_INTERVAL: Duration = Duration::from_millis(400);

/// What the open chart shows: the range and the design's material.
struct Shown {
    range: Arc<RiRange>,
    material: String,
}

thread_local! {
    /// The range on show, kept so a resize of the plot can redraw the dashed lines.
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    /// Watches the design and the dialog; lives as long as the UI thread.
    static WATCH: Timer = Timer::default();
}

/// Registers `RiChartModel`'s own callbacks (`open_chart`, `plot_resized`) and starts the watch
/// that keeps `can_queue_check_set` true while an editable design is open.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    _db: &Arc<Mutex<Database>>,
    _render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    let model = ui.global::<RiChartModel>();
    let weak = ui.as_weak();
    model.on_open_chart(move || {
        if let Some(ui) = weak.upgrade() {
            open_chart(&ui);
        }
    });
    let weak = ui.as_weak();
    model.on_plot_resized(move || {
        if let Some(ui) = weak.upgrade() {
            redraw(&ui);
        }
    });
    refresh_can_queue(ui);
    let weak = ui.as_weak();
    WATCH.with(|timer| {
        timer.start(TimerMode::Repeated, WATCH_INTERVAL, move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            refresh_can_queue(&ui);
            if !ui.global::<RiChartModel>().get_open() {
                SHOWN.with(|shown| *shown.borrow_mut() = None);
            }
        });
    });
}

/// Whether an editable design is open on the viewport.
fn design_is_open(ui: &MainWindow) -> bool {
    ui.global::<EditorModel>().get_has_design()
        && ui.global::<ViewportModel>().get_shows_editor_design()
}

/// Sets `RiChartModel.can_queue_check_set`: the check set renders the open editable design.
pub(in crate::gui) fn refresh_can_queue(ui: &MainWindow) {
    let model = ui.global::<RiChartModel>();
    let can = design_is_open(ui);
    if model.get_can_queue_check_set() != can {
        model.set_can_queue_check_set(can);
    }
}

/// "RI Chart...": opens the dialog and fills it from the range of the design, waiting for the
/// range worker when it has no result yet.
///
/// The chart is about the editor's design. While a library stone or a job view holds the
/// viewport, the viewport is first given back to that design (the same hand-back as entering
/// the Edit view), so the chart and the picture behind it are of the same stone. The
/// hand-back may finish a moment later (a background solve), so the dialog opens on
/// "a design is open" alone.
fn open_chart(ui: &MainWindow) {
    if !ui.global::<EditorModel>().get_has_design() {
        refresh_can_queue(ui);
        show_toast(ui, "Open a design to see its RI chart.", "info");
        return;
    }
    if !ui.global::<ViewportModel>().get_shows_editor_design() {
        ui.invoke_edit_view_entered();
    }
    refresh_can_queue(ui);
    present::push_computing(ui);
    ui.global::<RiChartModel>().set_open(true);
    when_ready(ui, |ui, range| {
        let material = ui
            .global::<ViewportModel>()
            .get_design_material_name()
            .to_string();
        SHOWN.with(|shown| *shown.borrow_mut() = Some(Shown { range, material }));
        redraw(ui);
    });
}

/// Draws the range on show with the plot's current size (the dashes and the label rows depend on
/// it). Does nothing when no range is on show.
fn redraw(ui: &MainWindow) {
    let plot_px = present::plot_px(ui);
    let built = SHOWN.with(|shown| {
        shown.borrow().as_ref().map(|shown| {
            view::build(&Inputs {
                range: &shown.range,
                material: &shown.material,
                calibrated: HIGH_BOUND_CALIBRATED,
                plot_px,
            })
        })
    });
    match built {
        None => {}
        Some(Some(view)) => present::push_view(ui, &view),
        Some(None) => present::push_unavailable(
            ui,
            "No RI range can be traced for this design: it has no facets to trace.",
        ),
    }
}
