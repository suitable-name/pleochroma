//! Hand-written rows for the editor and viewport callbacks that the generated tables never had a
//! row for (lane H4 of the 2026-10-28 harness findings): the Baseline strip's three buttons, the
//! Frosted tick box and the facet chips' frost button, the Optical Diagnostics overlay's
//! `invalidate`, `refresh` and `hover`, Stop After This Job, the Thumbnail Backdrop pills, the Live
//! Render tab's resize and the Performance graph's `cancel_hover`.
//!
//! Every row is a CHECKED row (`rows_checked.rs` style): it calls the callback in a state where it
//! does something and asserts what the handler leaves behind. Where the state cannot make the
//! handler act (no render has produced an optical map, no queue is running) the asserted effect is
//! the absence of one, like the cancel rows of `rows_checked.rs`.

use super::{
    rows_checked::clear_toast,
    table::{Invoke, Row, Setup},
};
use crate::support::app::{pump, wait_until};
use pleochroma_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Editor group: the Baseline strip
// ---------------------------------------------------------------------------------------------

/// Set as Baseline: the handler is the command bar's Set Baseline (`EditorModel.snapshot_design`),
/// so the effect is the "Baseline set" toast it raises once the design's masts are known and the
/// strip holding a Baseline.
fn baseline_set(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    ui.global::<BaselineModel>().invoke_set_baseline();
    if !wait_until(Duration::from_secs(20), || {
        ui.get_toast_visible() && ui.get_toast_message().as_str().starts_with("Baseline set")
    }) {
        return Err(format!(
            "Set as Baseline raised no \"Baseline set\" toast within 20 s (toast: {:?})",
            ui.get_toast_message().as_str()
        ));
    }
    let strip = ui.global::<BaselineModel>();
    if wait_until(Duration::from_secs(5), || strip.get_has_baseline()) {
        Ok(())
    } else {
        Err("the Baseline strip does not hold the Baseline that was set".to_owned())
    }
}

/// Compare...: opens the tier-by-tier comparison of the Baseline with the design now (the
/// Retarget dialog's compare page, `RetargetModel.compare_open`). A design that is opened holds a
/// Baseline already, so the button has something to compare.
fn baseline_compare(ui: &MainWindow) -> Result<(), String> {
    let retarget = ui.global::<RetargetModel>();
    retarget.set_compare_open(false);
    clear_toast(ui);
    ui.global::<BaselineModel>().invoke_compare();
    let shown = wait_until(Duration::from_secs(20), || retarget.get_compare_open());
    retarget.set_compare_open(false);
    pump(100);
    if shown {
        Ok(())
    } else {
        Err(format!(
            "Compare... opened no comparison within 20 s (toast: {:?})",
            ui.get_toast_message().as_str()
        ))
    }
}

/// Measure Tilt: the first call asks for the measurement (the button turns into Stop, or the strip
/// says why it cannot start), the second call is the Stop press and ends it again. A tilt
/// measurement that was started is stopped at once, so no long job is left behind.
fn baseline_measure_tilt(ui: &MainWindow) -> Result<(), String> {
    let strip = ui.global::<BaselineModel>();
    strip.invoke_measure_tilt();
    let reacted = wait_until(Duration::from_secs(5), || {
        strip.get_measuring_tilt() || !strip.get_note().is_empty()
    });
    if strip.get_measuring_tilt() {
        strip.invoke_measure_tilt();
        if !wait_until(Duration::from_secs(15), || !strip.get_measuring_tilt()) {
            return Err("the Stop press did not end the tilt measurement within 15 s".to_owned());
        }
    }
    if reacted {
        Ok(())
    } else {
        Err("Measure Tilt changed neither the button nor the strip's note".to_owned())
    }
}

// ---------------------------------------------------------------------------------------------
// Editor group: frosting
// ---------------------------------------------------------------------------------------------

/// How the design frosts tier 0, as the tier table's row says it ("" polished).
fn tier0_frosting(ui: &MainWindow) -> String {
    ui.global::<EditorModel>()
        .get_tiers()
        .row_data(0)
        .map(|row| row.frosting_text.to_string())
        .unwrap_or_default()
}

/// The inspector's Frosted tick box: tier 0 frosted as a whole, then polished again.
fn tier_frosted(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    editor.invoke_set_tier_frosted(0, true);
    if !wait_until(Duration::from_secs(8), || tier0_frosting(ui) == "Frosted") {
        return Err(format!(
            "Frosted on tier 0 shows {:?} in the tier table, not \"Frosted\"",
            tier0_frosting(ui)
        ));
    }
    editor.invoke_set_tier_frosted(0, false);
    if wait_until(Duration::from_secs(8), || tier0_frosting(ui).is_empty()) {
        Ok(())
    } else {
        Err(format!(
            "unticking Frosted left tier 0 at {:?}",
            tier0_frosting(ui)
        ))
    }
}

/// A facet chip's frost button: the first facet of tier 0 frosted (the row says so), then polished.
fn facet_frosted(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    let first = editor
        .get_tiers()
        .row_data(0)
        .ok_or("the editor lists no tier")?;
    let position: f32 = first
        .indices_full
        .replace(',', " ")
        .split_whitespace()
        .next()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("tier 0 lists no facet position ({:?})", first.indices_full))?;
    editor.invoke_toggle_facet_frosted(0, position);
    if !wait_until(Duration::from_secs(8), || !tier0_frosting(ui).is_empty()) {
        return Err(format!(
            "frosting the facet at {position} left tier 0 polished in the tier table"
        ));
    }
    editor.invoke_toggle_facet_frosted(0, position);
    if wait_until(Duration::from_secs(8), || tier0_frosting(ui).is_empty()) {
        Ok(())
    } else {
        Err(format!(
            "toggling the facet at {position} again left tier 0 at {:?}",
            tier0_frosting(ui)
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// Editor group: the Optical Diagnostics overlay and the Performance graph
// ---------------------------------------------------------------------------------------------

/// Puts a map on show (`ready` and a readout), as a finished diagnostic pass leaves the model.
fn optical_with_map(ui: &MainWindow) -> OpticalModel<'_> {
    let optical = ui.global::<OpticalModel>();
    optical.set_ready(true);
    optical.set_hover_text("harness readout".into());
    optical
}

/// The pointer-down and resize handlers' `invalidate`: the old map is hidden at once.
fn optical_invalidate(ui: &MainWindow) -> Result<(), String> {
    let optical = optical_with_map(ui);
    optical.invoke_invalidate();
    pump(60);
    if optical.get_ready() || !optical.get_hover_text().is_empty() {
        return Err("invalidate left the old diagnostic map and its readout showing".to_owned());
    }
    Ok(())
}

/// Refresh Diagnostic Map: the controller drops what it traced and the map is hidden until the
/// next pass.
fn optical_refresh(ui: &MainWindow) -> Result<(), String> {
    let optical = optical_with_map(ui);
    optical.invoke_refresh();
    pump(60);
    if optical.get_ready() || !optical.get_hover_text().is_empty() {
        return Err("refresh left the old diagnostic map and its readout showing".to_owned());
    }
    Ok(())
}

/// The pointer over the picture: with no traced map in the controller (no render has run here) the
/// readout stays empty; it is the absence of a map that is asserted.
fn optical_hover(ui: &MainWindow) -> Result<(), String> {
    let optical = ui.global::<OpticalModel>();
    optical.set_ready(true);
    optical.set_hover_text("".into());
    optical.invoke_hover(0.5, 0.5);
    pump(60);
    let text = optical.get_hover_text().to_string();
    optical.set_ready(false);
    if text.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "hover wrote the readout {text:?} without a traced map"
        ))
    }
}

/// The Performance graph's pointer leaving the curve: the hover picture is dropped.
fn tilt_cancel_hover(ui: &MainWindow) -> Result<(), String> {
    let tilt = ui.global::<TiltModel>();
    tilt.set_hover_ready(true);
    tilt.invoke_cancel_hover();
    pump(60);
    if tilt.get_hover_ready() {
        Err("cancel_hover left the hover picture marked ready".to_owned())
    } else {
        Ok(())
    }
}

const GAPS_EDITOR_CHECKED: &[Row] = checked_rows!(Editor, false;
    BaselineModel.set_baseline => baseline_set;
    BaselineModel.compare => baseline_compare;
    BaselineModel.measure_tilt => baseline_measure_tilt;
    EditorModel.set_tier_frosted => tier_frosted;
    EditorModel.toggle_facet_frosted => facet_frosted;
    OpticalModel.invalidate => optical_invalidate;
    OpticalModel.refresh => optical_refresh;
    OpticalModel.hover => optical_hover;
    TiltModel.cancel_hover => tilt_cancel_hover;
);

/// The Live Render tab's size changed: the 50 ms debounce re-plans the frame (the row's own pump
/// outlasts it). Nothing observable beyond "no panic" in the harness, so a plain row.
const GAPS_EDITOR_PLAIN: &[Row] = rows!(MainWindow, Main, Editor, false;
    SolidPreviewModel.invoke_live_viewport_resized();
);

// ---------------------------------------------------------------------------------------------
// Startup group
// ---------------------------------------------------------------------------------------------

/// Stop After This Job with no queue running: there is no job to stop after, so the flag stays down
/// and no toast says otherwise.
fn stop_after_current(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    let jobs = ui.global::<RenderJobsModel>();
    jobs.invoke_toggle_stop_after_current();
    pump(100);
    if jobs.get_stop_after_current() {
        return Err("Stop After This Job was set with no queue running".to_owned());
    }
    if ui.get_toast_visible() {
        return Err(format!(
            "Stop After This Job raised a toast with no queue running: {:?}",
            ui.get_toast_message().as_str()
        ));
    }
    Ok(())
}

/// A Thumbnail Backdrop pick (3 is Black): the model follows, then the default (1, Grey) is put
/// back.
fn thumbnail_backdrop(ui: &MainWindow) -> Result<(), String> {
    let settings = ui.global::<SettingsModel>();
    settings.invoke_thumbnail_backdrop_selected(3);
    pump(100);
    let picked = settings.get_thumbnail_backdrop_index();
    settings.invoke_thumbnail_backdrop_selected(1);
    pump(100);
    if picked == 3 {
        Ok(())
    } else {
        Err(format!("picking Black left the backdrop index at {picked}"))
    }
}

const GAPS_STARTUP_CHECKED: &[Row] = checked_rows!(Startup, false;
    RenderJobsModel.toggle_stop_after_current => stop_after_current;
    SettingsModel.thumbnail_backdrop_selected => thumbnail_backdrop;
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    [GAPS_EDITOR_CHECKED, GAPS_EDITOR_PLAIN, GAPS_STARTUP_CHECKED]
        .into_iter()
        .flatten()
        .collect()
}
