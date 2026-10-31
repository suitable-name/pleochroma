//! The Rough Planner state for the elements run 8 left uncovered in the Crystal Axis panel (lane
//! H5 of the 2026-10-28 findings): "Second Axis (deg)" and the Orientation Rule editor's own
//! controls ("Kind of Orientation Rule", "Preferred", "Required", "Tolerance (deg)", "Weight (%
//! per deg)").
//!
//! What kept them unreached, by control:
//!
//! * "Second Axis (deg)" exists only for a BIAXIAL material (`axis_biaxial`); the earlier axis
//!   states (`planner_axis.rs`) pick a sapphire, which is uniaxial;
//! * the rule's kind combo, its two pills and its two fields exist only while the rule is the
//!   cutter's own (`plan_rule_choice == 2`, the rule editor's "Set" pill); every earlier state
//!   left it on "Material Default";
//! * "From Color Zones" is enabled only when the saved plan's stored rough color has a zone that
//!   runs along the crystal; no state can store one (see `coverage_allow.rs`).
//!
//! The state is a tanzanite block (else another biaxial material) with its crystal axis fitted and
//! the plan's rule set to "Set", then the pass over the planner window follows (`run_window`).

use super::{
    super::{Piece, existing_install},
    overlays,
    windows_reach::run_window,
};
use crate::support::{
    HarnessApp, Profile, SecondaryWindow, SettingsSeed,
    app::{pump, wait_until},
};
use pleochroma_cut::{RoughPlanModel, harness_support};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::backend::wait_until_labeled;

/// The state's name in the report.
const STATE: &str = "a biaxial crystal axis with its own rule";

/// Biaxial materials the planner's list may hold, in the order they are tried.
const BIAXIAL: [&str; 6] = [
    "tanzanite",
    "iolite",
    "topaz",
    "kyanite",
    "peridot",
    "alexandrite",
];

/// The pieces of this file: one `window:reach-planner:<state>` piece.
pub fn pieces() -> Vec<Piece> {
    vec![Piece::new(
        format!("window:reach-planner:{STATE}"),
        existing_install,
        |ctx| {
            let profile: &Profile = ctx.profile;
            let mut open = |app: &mut HarnessApp| prepare(app, profile);
            run_window(
                ctx,
                SecondaryWindow::RoughPlanner,
                STATE,
                false,
                &[],
                &mut open,
            );
        },
    )]
}

/// Picks the first biaxial material of [`BIAXIAL`] the planner lists; `false` when it lists none.
fn choose_biaxial(model: &RoughPlanModel<'_>) -> bool {
    let names: Vec<String> = model
        .get_materials()
        .iter()
        .map(|name| name.to_string().to_lowercase())
        .collect();
    let Some(index) = BIAXIAL
        .iter()
        .find_map(|wanted| names.iter().position(|name| name.contains(wanted)))
    else {
        return false;
    };
    model.set_material_index(i32::try_from(index).unwrap_or(0));
    true
}

/// A 40 x 30 x 25 mm block of a biaxial material with its crystal axis fitted to the block's
/// length (so the axis fields, the Second Axis among them, and the rule editor show). `false` when
/// the material is missing or the planner offers no axis for it.
fn biaxial_block_with_axis(model: &RoughPlanModel<'_>) -> bool {
    if !choose_biaxial(model) {
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
    wait_until(Duration::from_secs(10), || {
        model.get_axis_set() && model.get_axis_biaxial()
    })
}

/// The library, then the planner on the biaxial block with its axis and its own rule.
fn prepare(app: &mut HarnessApp, profile: &Profile) {
    let seed = SettingsSeed::ExistingInstall;
    if let Err(message) = overlays::launch_library(app, profile, &seed) {
        println!("ui-harness:   planner state: {message}");
        if !app.is_open() && app.relaunch(profile, &seed).is_err() {
            return;
        }
    }
    if !overlays::open_planner(app) {
        return;
    }
    let Some(window) = harness_support::rough_planner_window() else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    let _ = wait_until_labeled("planner-open", Duration::from_secs(10), || {
        model.get_materials().row_count() > 0
    });
    if !biaxial_block_with_axis(&model) {
        return;
    }
    // The rule editor's own "Set" pill: the plan's rule (entry -1), field 0 (the choice), value 2.
    // The axis panel has a "Set" pill too, so the callback is called as the pill calls it.
    model.invoke_rule_edit(-1, 0, "2".into());
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_plan_rule_choice() == 2
    });
    let _ = wait_until(Duration::from_secs(10), || {
        !model.get_running() && !model.get_mesh_busy()
    });
    pump(300);
}
