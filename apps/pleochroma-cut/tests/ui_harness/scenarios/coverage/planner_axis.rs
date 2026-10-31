//! The Rough Planner states of the Crystal Axis for `windows_reach.rs`: a sapphire rough with its
//! crystal axis set (the axis panel's fields and the Orientation Rule editor appear only then),
//! and a planned layout whose first stone is locked (the stone rows, their menu and the note
//! under them appear only for a planned layout of a rough with an axis).
//!
//! Each function takes the planner's model right after the window opened and the materials were
//! listed; the pass over the window (the overlay engine with `OVERLAY_DENY`) follows it.

use crate::support::app::{pump, wait_until};
use pleochroma_cut::RoughPlanModel;
use slint::Model;
use std::time::Duration;

/// How long a plan may take here, in seconds.
const PLAN_WAIT_SECS: u64 = 60;
/// The stone action of "Lock" (and of "Unlock"): `RoughPlanModel.stone_action`'s action 3.
const LOCK_ACTION: i32 = 3;

/// Picks the sapphire (else any corundum) in the planner's material list; `false` when there is
/// none.
fn choose_sapphire(model: &RoughPlanModel<'_>) -> bool {
    let names: Vec<String> = model.get_materials().iter().map(String::from).collect();
    let wanted = |text: &str| {
        names
            .iter()
            .position(|name| name.to_lowercase().contains(text))
    };
    let Some(index) = wanted("sapphire").or_else(|| wanted("corundum")) else {
        return false;
    };
    model.set_material_index(i32::try_from(index).unwrap_or(0));
    true
}

/// A 40 x 30 x 25 mm sapphire block for the whole library, at most two designs per layout, with
/// its crystal axis set (by Fit to Crystal Length, then typed values), so a plan started
/// afterwards is oriented by it. `false` when the sapphire is missing, offers no axis, or the
/// axis does not take.
fn block_with_axis(model: &RoughPlanModel<'_>) -> bool {
    if !choose_sapphire(model) {
        return false;
    }
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.set_max_designs(2);
    model.set_use_filter(false);
    model.invoke_base_field_committed();
    pump(250);
    if !wait_until(Duration::from_secs(10), || model.get_axis_available()) {
        return false;
    }
    model.invoke_axis_fit_length();
    if !wait_until(Duration::from_secs(10), || model.get_axis_set()) {
        return false;
    }
    model.invoke_axis_edit(0, "25".into());
    pump(200);
    model.invoke_axis_edit(1, "30".into());
    pump(300);
    true
}

/// The rough's crystal axis set, with its sentence, note and the Orientation Rule editor showing.
pub(super) fn axis_set(model: &RoughPlanModel<'_>) {
    let _ = block_with_axis(model);
}

/// The block planned with the axis set and the best layout selected; its first stone locked, so
/// the note under the stone rows shows and the layout is cut again around the locked stone.
pub(super) fn stone_locked(model: &RoughPlanModel<'_>) {
    if !block_with_axis(model) {
        return;
    }
    model.invoke_plan();
    let _ = wait_until(Duration::from_secs(PLAN_WAIT_SECS), || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    if model.get_results().row_count() == 0 {
        return;
    }
    model.invoke_select_result(0);
    if !wait_until(Duration::from_secs(10), || {
        model.get_stone_rows().row_count() > 0
    }) {
        return;
    }
    model.invoke_stone_action(0, LOCK_ACTION, "".into());
    let _ = wait_until(Duration::from_secs(PLAN_WAIT_SECS), || !model.get_running());
    pump(300);
}
