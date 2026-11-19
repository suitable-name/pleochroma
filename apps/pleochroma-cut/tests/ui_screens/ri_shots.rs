//! The shots of the cutter-feedback screens: the geometric hint and the traced chip on the Stone
//! tab, the RI range field of Design Settings, the RI chart, the material reference's "Your
//! designs that work here", the Compute RI Ranges dialog, the tilt figures of Compare and
//! Baseline (at the default ±90° and at ±30°) and Preferences' "Figures" section.
//!
//! Each function takes `small`: the `_640` shots shrink the window before the state opens. The
//! ±30° shots set the preference through `PreferencesModel.tilt_range_changed` and set it back to
//! ±90° (index 5) on every path, so no later shot sees the narrower range.

use crate::{
    dialog_shots::{card_shot, popup_in, scroll_to_label, wait_component},
    kit::{Rect, Sc, click_first, find_inherits, rect_of, size_window, sized_shots},
    nav::{FVS, STUDIO},
    support::{WindowView, app::pump},
    window_shots::{open_compare, press_pill, settle_compare, snap_compare},
};
use pleochroma_cut::{
    BaselineModel, CompareModel, MaterialReferenceModel, ParametersModel, PreferencesModel,
    RiBatchModel, RiChartModel, RiFieldModel, RiRangeModel, harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::{Duration, Instant};
use uiprobe::screenshot::{self, Shot};

/// The path cache folder the Preferences shot shows: the app's Linux default, without a user name.
const NEUTRAL_RENDER_CACHE: &str = "~/.config/pleochroma-cut/render_cache";
/// How long the trace of a design's RI range may take, in seconds.
const RANGE_SECS: u64 = 240;
/// How long the tilt average may take, in seconds.
const TILT_SECS: u64 = 600;
/// The index of "±30°" among the Tilt Average Range choices.
const RANGE_30: i32 = 2;
/// The index of "±90° (all tilts)", the default.
const RANGE_90: i32 = 5;

/// The Parameters dock as a rectangle.
fn dock_rect(sc: &Sc<'_, '_>) -> Result<Rect, String> {
    let view = sc.view();
    rect_of(&view, "ParametersDock").or_else(|_| rect_of(&view, "ParametersPanel"))
}

/// The state of the Stone tab's RI chip: 0 hidden, 1 hint, 2 traced.
fn chip_state(sc: &Sc<'_, '_>) -> i32 {
    sc.ui().global::<RiRangeModel>().get_chip_state()
}

/// Waits until the chip shows the traced range; a timeout is a note.
fn wait_traced(sc: &mut Sc<'_, '_>) {
    if !Sc::wait(RANGE_SECS, || chip_state(sc) == 2) {
        sc.note(format!(
            "the RI range was not traced within {RANGE_SECS} s; photographed anyway"
        ));
    }
    pump(800);
}

/// `stone_ri_hint.png`: the Stone tab while the chip still shows the geometric hint.
///
/// The hint shows until the trace lands. The design is loaded with the tab already open, and the
/// window is captured the moment the chip leaves state 0; when the trace was faster than the
/// poll, the shot shows the traced chip and says so.
fn stone_ri_hint_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.select_design(FVS)?;
    sc.show_live_view();
    if small {
        sc.small();
    }
    let ui = sc.ui();
    ui.global::<ParametersModel>().invoke_show_tab(0);
    pump(600);
    ui.global::<pleochroma_cut::EditorModel>()
        .invoke_load_selected();
    let start = Instant::now();
    while chip_state(sc) == 0 {
        if start.elapsed() > Duration::from_secs(180) {
            return Err("the Stone tab showed no RI hint within 180 s".to_owned());
        }
        pump(15);
    }
    let state = chip_state(sc);
    let shot = screenshot::capture(sc.ctx.app.ui().window())?;
    sc.privacy(&sc.view())?;
    if state != 1 {
        sc.note("stone_ri_hint: the trace landed before the capture; the chip shows the range");
    }
    Ok(dock_rect(sc)?.crop(&shot))
}

sized_shots!(
    "The Stone tab with the geometric RI hint.",
    stone_ri_hint,
    stone_ri_hint_640,
    stone_ri_hint_at
);

/// `stone_ri_traced.png`: the Stone tab after the trace, with the chip "RI 1.63 and up".
fn stone_ri_traced_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.show_live_view();
    if small {
        sc.small();
    }
    sc.ui().global::<ParametersModel>().invoke_show_tab(0);
    wait_traced(sc);
    let shot = sc.snap_main()?;
    Ok(dock_rect(sc)?.crop(&shot))
}

sized_shots!(
    "The Stone tab with the traced RI chip.",
    stone_ri_traced,
    stone_ri_traced_640,
    stone_ri_traced_at
);

/// `design_settings_ri_range.png`: the Design Settings sheet with the RI range field on
/// "Pleochroma Estimate".
fn design_settings_ri_range_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    wait_traced(sc);
    if small {
        sc.small();
    }
    let before = sc.snap_main()?;
    if !click_first(&sc.view(), "Edit Design Settings") {
        return Err("the Edit view has no Edit Design Settings button".to_owned());
    }
    pump(800);
    let ui = sc.ui();
    let field = ui.global::<RiFieldModel>();
    field.set_source_index(2);
    field.invoke_source_chosen(2);
    pump(1000);
    let after = sc.snap_main()?;
    // The sheet is a popup the element queries do not reach: found by the pixels it changed,
    // else the whole window.
    Ok(popup_in(&before, &after).unwrap_or(after))
}

sized_shots!(
    "The Design Settings sheet with the RI range field.",
    design_settings_ri_range,
    design_settings_ri_range_640,
    design_settings_ri_range_at
);

/// `ri_chart.png`: the RI chart of the open design.
fn ri_chart_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    if small {
        sc.small();
    }
    let ui = sc.ui();
    let chart = ui.global::<RiChartModel>();
    chart.invoke_open_chart();
    if !Sc::wait(30, || chart.get_open()) {
        return Err("the RI chart did not open".to_owned());
    }
    if !Sc::wait(RANGE_SECS, || chart.get_ready()) {
        return Err(format!(
            "the RI chart has no range after {RANGE_SECS} s: {}",
            chart.get_message()
        ));
    }
    wait_component(sc, "RiChartDialog")?;
    pump(800);
    card_shot(sc, "RiChartDialog")
}

sized_shots!(
    "The RI chart of the open design.",
    ri_chart,
    ri_chart_640,
    ri_chart_at
);

/// `material_reference_designs.png`: the material reference's detail page scrolled to "Your
/// designs that work here". The design is opened in the editor first, so its traced range is in
/// the library.
fn material_reference_designs_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    wait_traced(sc);
    sc.show_live_view();
    sc.ui().global::<ParametersModel>().invoke_show_tab(0);
    pump(600);
    let opener = sc
        .view()
        .elements()
        .into_iter()
        .find(|element| {
            element
                .accessible_label()
                .is_some_and(|label| label.to_lowercase().contains("material details"))
        })
        .ok_or("the Stone tab has no Material Details button")?;
    opener.invoke_accessible_default_action();
    if !Sc::wait(30, || {
        harness_support::material_reference_window().is_some()
    }) {
        return Err("the Material Reference window did not open".to_owned());
    }
    let window = harness_support::material_reference_window().ok_or("no reference window")?;
    size_window(&window, if small { (640, 480) } else { (1180, 780) });
    let model = window.global::<MaterialReferenceModel>();
    let position = model
        .get_rows()
        .iter()
        .position(|row| row.name.as_str() == "Diamond");
    if let Some(row) = position.and_then(|row| i32::try_from(row).ok()) {
        model.invoke_row_opened(row);
    }
    pump(1000);
    // The card is the last of the detail page: scroll down to it.
    let view = WindowView::of("MaterialReferenceWindow", &window);
    if let Some(area) = view
        .root
        .query_descendants()
        .match_inherits("ScrollView")
        .find_first()
    {
        for _ in 0..8 {
            area.scroll(0.0, -400.0);
            pump(150);
        }
    }
    if model.get_design_rows().row_count() == 0 {
        sc.note("material_reference_designs: the card lists no design (no range was stored)");
    }
    pump(500);
    sc.snap_window("MaterialReferenceWindow", &window)
}

sized_shots!(
    "The material reference's list of the designs that work in the material.",
    material_reference_designs,
    material_reference_designs_640,
    material_reference_designs_at
);

/// `ri_range_batch.png`: the Compute RI Ranges dialog at its confirm step. Its Compute button is
/// never pressed: it would trace every design of the library.
fn ri_range_batch_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    if small {
        sc.small();
    }
    let ui = sc.ui();
    let batch = ui.global::<RiBatchModel>();
    batch.invoke_open_batch();
    if !Sc::wait(30, || batch.get_visible()) {
        return Err("the Compute RI Ranges dialog did not open".to_owned());
    }
    if !Sc::wait(60, || batch.get_design_count() >= 0) {
        sc.note("ri_range_batch: the designs were not counted within 60 s");
    }
    wait_component(sc, "RiRangeBatchDialog")?;
    pump(600);
    card_shot(sc, "RiRangeBatchDialog")
}

sized_shots!(
    "The Compute RI Ranges dialog.",
    ri_range_batch,
    ri_range_batch_640,
    ri_range_batch_at
);

/// Sets the Tilt Average Range choice (an index into its six options).
fn set_tilt_range(sc: &Sc<'_, '_>, index: i32) {
    sc.ui()
        .global::<PreferencesModel>()
        .invoke_tilt_range_changed(index);
    pump(800);
}

/// Runs `shoot` with the range at ±30° and puts it back to ±90° on every path.
fn at_range_30(
    sc: &Sc<'_, '_>,
    shoot: impl FnOnce() -> Result<Shot, String>,
) -> Result<Shot, String> {
    set_tilt_range(sc, RANGE_30);
    let result = shoot();
    set_tilt_range(sc, RANGE_90);
    result
}

/// Compare with the Tilt Average measured, the pane scrolled to its tilt rows.
fn compare_with_tilt(sc: &mut Sc<'_, '_>, small: bool) -> Result<(), String> {
    open_compare(sc)?;
    press_pill(sc, "Side by Side")?;
    press_pill(sc, "Path Traced")?;
    settle_compare(sc);
    let ui = sc.ui();
    let model = ui.global::<CompareModel>();
    if !model.get_tilt_available() {
        return Err("the comparison offers no Tilt Average for this pair".to_owned());
    }
    press_pill(sc, "Tilt Average")?;
    if !Sc::wait(TILT_SECS, || model.get_tilt_done()) {
        return Err(format!(
            "the Tilt Average did not finish within {TILT_SECS} s"
        ));
    }
    if small {
        sc.small();
    }
    pump(1000);
    // The tilt rows and their caption are the last of the table: at 640 x 480 they are below the
    // fold.
    if let Some(pane) = find_inherits(&sc.view(), "OptimizerComparePane") {
        pane.scroll(0.0, -3000.0);
        pump(400);
    }
    Ok(())
}

/// `compare_tilt.png`: Compare after the Tilt Average, at the default ±90°.
fn compare_tilt_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    compare_with_tilt(sc, small)?;
    snap_compare(sc)
}

sized_shots!(
    "Compare after the Tilt Average, at the default range.",
    compare_tilt,
    compare_tilt_640,
    compare_tilt_at
);

/// `compare_tilt_range_30.png`: the same Compare with the preference at ±30°.
fn compare_tilt_range_30_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    compare_with_tilt(sc, small)?;
    at_range_30(sc, || snap_compare(sc))
}

sized_shots!(
    "Compare after the Tilt Average, with the range at 30 degrees.",
    compare_tilt_range_30,
    compare_tilt_range_30_640,
    compare_tilt_range_30_at
);

/// The Edit view's Baseline figures with the tilt averages measured.
fn baseline_with_tilt(sc: &mut Sc<'_, '_>, small: bool) -> Result<(), String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    let baseline = ui.global::<BaselineModel>();
    let scored = Sc::wait(180, || {
        baseline.get_has_baseline() && !baseline.get_busy() && baseline.get_cells().row_count() > 0
    });
    if !scored {
        return Err("the Baseline holds no figures after 180 s".to_owned());
    }
    baseline.invoke_measure_tilt();
    let measured = Sc::wait(TILT_SECS, || {
        !baseline.get_measuring_tilt()
            && baseline
                .get_cells()
                .iter()
                .any(|cell| cell.name.to_lowercase().contains("tilt"))
    });
    if !measured {
        return Err(format!(
            "the Baseline's tilt averages were not measured within {TILT_SECS} s: {}",
            baseline.get_note()
        ));
    }
    if small {
        sc.small();
    }
    pump(1000);
    Ok(())
}

/// The Baseline figures card as a picture.
fn snap_baseline(sc: &Sc<'_, '_>) -> Result<Shot, String> {
    let shot = sc.snap_main()?;
    Ok(rect_of(&sc.view(), "EditFigures")?.crop(&shot))
}

/// `baseline_tilt.png`: the Baseline card after Measure Tilt, caption "Tilt average to ±90°".
fn baseline_tilt_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    baseline_with_tilt(sc, small)?;
    snap_baseline(sc)
}

sized_shots!(
    "The Baseline card after Measure Tilt, at the default range.",
    baseline_tilt,
    baseline_tilt_640,
    baseline_tilt_at
);

/// `baseline_tilt_range_30.png`: the same card with the preference at ±30°.
fn baseline_tilt_range_30_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    baseline_with_tilt(sc, small)?;
    at_range_30(sc, || snap_baseline(sc))
}

sized_shots!(
    "The Baseline card after Measure Tilt, with the range at 30 degrees.",
    baseline_tilt_range_30,
    baseline_tilt_range_30_640,
    baseline_tilt_range_30_at
);

/// `preferences_figures.png`: Preferences scrolled to its "Figures" section.
fn preferences_figures_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    if small {
        sc.small();
    }
    let ui = sc.ui();
    let prefs = ui.global::<PreferencesModel>();
    prefs.set_path_cache_dir(NEUTRAL_RENDER_CACHE.into());
    prefs.set_dialog_open(true);
    wait_component(sc, "PreferencesDialog")?;
    let _ = scroll_to_label(sc, "PreferencesDialog", "Tilt Average Range")?;
    pump(500);
    card_shot(sc, "PreferencesDialog")
}

sized_shots!(
    "Preferences at its Figures section.",
    preferences_figures,
    preferences_figures_640,
    preferences_figures_at
);
