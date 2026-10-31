//! The direct-invoke rows of the Crystal Axis: the Stone tab's presets and Second Axis, the
//! overlay's pointer callbacks (editor group), and the Rough Planner's axis, rule and stone
//! callbacks (planner group).
//!
//! The presets and the Second Axis are CHECKED rows (`rows_checked.rs` style): a sapphire or a
//! tanzanite is given to the design first, so the handlers reach their success path and the row
//! asserts the controls they set. The overlay and planner callbacks are plain rows with the empty
//! value of each parameter type: without an overlay on screen or an axis in the rough, the
//! handlers answer "nothing here" and must neither panic nor change the design; the passes of
//! `states_axis.rs` and `planner_axis.rs` drive them with an axis on screen.

use super::table::{Invoke, Row, Setup};
use crate::support::app::{pump, wait_until};
use pleochroma_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Gives the editor's design the material containing `wanted` (lower case) and waits until the
/// Stone tab offers a crystal axis for it.
fn use_material(ui: &MainWindow, wanted: &str) -> Result<(), String> {
    let name = ui
        .global::<ViewportModel>()
        .get_material_options()
        .iter()
        .find(|name| name.to_lowercase().contains(wanted))
        .ok_or_else(|| format!("the app offers no {wanted} material"))?;
    let editor = ui.global::<EditorModel>();
    let code = editor.invoke_apply_named_material(name, 0.0);
    if code != 0 {
        return Err(format!("the design did not take {wanted} (answer {code})"));
    }
    let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
    pump(300);
    let settings = ui.global::<SettingsModel>();
    if wait_until(Duration::from_secs(10), || {
        settings.get_c_axis_override_available()
    }) {
        Ok(())
    } else {
        Err(format!("a {wanted} offers no crystal axis"))
    }
}

// ---------------------------------------------------------------------------------------------
// Editor group, checked rows
// ---------------------------------------------------------------------------------------------

/// "In the Table Plane" on a sapphire: "Set for This Stone" is on and the tilt is 90 degrees.
fn geometry_preset(ui: &MainWindow) -> Result<(), String> {
    use_material(ui, "sapphire")?;
    ui.global::<CrystalAxisModel>()
        .invoke_geometry_preset_chosen(1);
    pump(300);
    let settings = ui.global::<SettingsModel>();
    if !settings.get_c_axis_override_enabled() {
        return Err("In the Table Plane did not turn Set for This Stone on".to_owned());
    }
    let tilt = settings.get_c_axis_tilt_deg();
    if (tilt - 90.0).abs() > 0.5 {
        return Err(format!("In the Table Plane set the tilt to {tilt}, not 90"));
    }
    Ok(())
}

/// "Best Blue Face-Up" on a tanzanite: "Set for This Stone" is on and the Second Axis is set too.
fn color_preset(ui: &MainWindow) -> Result<(), String> {
    use_material(ui, "tanzanite")?;
    let axis = ui.global::<CrystalAxisModel>();
    axis.invoke_refresh();
    if !wait_until(Duration::from_secs(10), || {
        axis.get_color_preset_names().row_count() > 0
    }) {
        return Err("a tanzanite offers no color-named preset".to_owned());
    }
    axis.invoke_color_preset_chosen(0);
    pump(300);
    if !ui.global::<SettingsModel>().get_c_axis_override_enabled() {
        return Err("the preset did not turn Set for This Stone on".to_owned());
    }
    if !axis.get_second_set() {
        return Err("the preset did not set the Second Axis".to_owned());
    }
    Ok(())
}

/// A Second Axis turn of 40 degrees on a tanzanite: the switch is on and holds the turn.
fn second_axis(ui: &MainWindow) -> Result<(), String> {
    use_material(ui, "tanzanite")?;
    let axis = ui.global::<CrystalAxisModel>();
    axis.invoke_second_axis_changed(true, 40.0);
    pump(300);
    if !axis.get_second_set() {
        return Err("the Second Axis switch did not stay on".to_owned());
    }
    let turn = axis.get_second_deg();
    if (turn - 40.0).abs() > 0.5 {
        return Err(format!("the Second Axis turn is {turn}, not 40"));
    }
    Ok(())
}

const AXIS_CHECKED: &[Row] = checked_rows!(Editor, true;
    CrystalAxisModel.geometry_preset_chosen => geometry_preset;
    CrystalAxisModel.color_preset_chosen => color_preset;
    CrystalAxisModel.second_axis_changed => second_axis;
);

// ---------------------------------------------------------------------------------------------
// Editor group, plain rows
// ---------------------------------------------------------------------------------------------

/// Editor setup, Main window, shared state.
const AXIS_MAIN_SHARED: &[Row] = rows!(MainWindow, Main, Editor, false;
    CrystalAxisModel.invoke_drag_begin(Default::default(), Default::default(), Default::default());
    CrystalAxisModel.invoke_drag_end();
    CrystalAxisModel.invoke_drag_move(Default::default(), Default::default());
    CrystalAxisModel.invoke_hit_test(Default::default(), Default::default());
    CrystalAxisModel.invoke_hover(Default::default(), Default::default());
    CrystalAxisModel.invoke_live_overlay_changed();
    CrystalAxisModel.invoke_overlay_changed();
    CrystalAxisModel.invoke_refresh();
    CrystalAxisModel.invoke_show_axis_toggled(Default::default());
);

/// Editor setup, Main window, fresh state per row.
const AXIS_MAIN_FRESH: &[Row] = rows!(MainWindow, Main, Editor, true;
    CrystalAxisModel.invoke_drag_cancel();
);

// ---------------------------------------------------------------------------------------------
// Planner group
// ---------------------------------------------------------------------------------------------

/// Planner setup, Planner window, shared state.
const AXIS_PLANNER_SHARED: &[Row] = rows!(RoughPlannerWindow, Planner, Planner, false;
    RoughPlanModel.invoke_axis_edit(Default::default(), slint::SharedString::default());
    RoughPlanModel.invoke_axis_fit_length();
    RoughPlanModel.invoke_axis_from_zones();
    RoughPlanModel.invoke_axis_make_set();
    RoughPlanModel.invoke_axis_unknown();
    RoughPlanModel.invoke_rule_edit(Default::default(), Default::default(), slint::SharedString::default());
    RoughPlanModel.invoke_stone_action(Default::default(), Default::default(), slint::SharedString::default());
);

/// Planner setup, Planner window, fresh state per row: Align to Face leaves the view waiting for
/// a click, which later rows' view callbacks must not meet.
const AXIS_PLANNER_FRESH: &[Row] = rows!(RoughPlannerWindow, Planner, Planner, true;
    RoughPlanModel.invoke_axis_pick_start(1);
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    [
        AXIS_MAIN_SHARED,
        AXIS_MAIN_FRESH,
        AXIS_CHECKED,
        AXIS_PLANNER_SHARED,
        AXIS_PLANNER_FRESH,
    ]
    .into_iter()
    .flatten()
    .collect()
}
