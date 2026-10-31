//! The direct-invoke rows of the Render Physics controls: the tier Surface Finish edits
//! (`SurfaceModel`), the Faceting Machine preference (`FacetingMachineModel`), the Stone tab's
//! Inclusions, Play-of-Color and Abraded Edges groups (`RenderEffectsModel`) and the Gem Material
//! Editor's Inclusion Families and Play-of-Color sections (`InclusionFamiliesModel`,
//! `StructureParamsModel`). `AxisCurvesModel` has no callback.
//!
//! A few rows are CHECKED (`rows_checked.rs` style): a material that offers the control is given to
//! the design first, so the handler reaches its success path and the row asserts the control it
//! sets. The others are plain rows with the empty value of each parameter type: without a dialog
//! open or a stone that offers the control, the handlers answer "nothing here" and must neither
//! panic nor change the design; the passes of `states_physics.rs` drive the controls themselves.

use super::table::{Invoke, Row, Setup};
use crate::support::app::{pump, wait_until};
use pleochroma_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Gives the editor's design the material containing `wanted` (lower case) and waits for the solve.
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
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Editor group, checked rows
// ---------------------------------------------------------------------------------------------

/// The first inclusion preset of a sapphire: the design takes it.
fn inclusion_preset(ui: &MainWindow) -> Result<(), String> {
    use_material(ui, "sapphire")?;
    let effects = ui.global::<RenderEffectsModel>();
    effects.invoke_refresh();
    if !wait_until(Duration::from_secs(10), || {
        effects.get_inclusion_preset_names().row_count() > 0
    }) {
        return Err("a sapphire offers no inclusion preset".to_owned());
    }
    let name = effects
        .get_inclusion_preset_names()
        .row_data(0)
        .unwrap_or_default();
    effects.invoke_inclusion_preset_chosen(name.clone());
    pump(300);
    if effects.get_inclusion_preset() != name {
        return Err(format!(
            "the preset {name} was not kept (the group shows {:?})",
            effects.get_inclusion_preset()
        ));
    }
    Ok(())
}

/// Reseed on a precious opal with Play-of-Color on: the seed changes.
fn structure_reseed(ui: &MainWindow) -> Result<(), String> {
    use_material(ui, "precious opal")?;
    let effects = ui.global::<RenderEffectsModel>();
    effects.invoke_refresh();
    if !wait_until(Duration::from_secs(10), || {
        effects.get_structure_available()
    }) {
        return Err("a precious opal offers no Play-of-Color group".to_owned());
    }
    effects.set_structure_on(true);
    effects.invoke_structure_edited();
    pump(300);
    let before = effects.get_structure_seed();
    effects.invoke_structure_reseed();
    pump(300);
    if effects.get_structure_seed() == before {
        return Err(format!("Reseed left the seed at {before}"));
    }
    Ok(())
}

/// "Wide" in the Faceting Machine preference: the sweep name follows.
fn sweep_preset(ui: &MainWindow) -> Result<(), String> {
    let machine = ui.global::<FacetingMachineModel>();
    machine.invoke_sweep_preset_chosen(2);
    pump(300);
    let preset = machine.get_sweep_preset();
    if preset != 2 {
        return Err(format!("the sweep name is {preset}, not 2 (Wide)"));
    }
    Ok(())
}

/// "Reset to the Plain Machine" after a mast to the side: the arm points at the lap centre again.
fn machine_reset(ui: &MainWindow) -> Result<(), String> {
    let machine = ui.global::<FacetingMachineModel>();
    machine.set_mast_distance_mm(30.0);
    machine.invoke_value_changed();
    pump(200);
    machine.invoke_reset_requested();
    pump(300);
    let mast = machine.get_mast_distance_mm();
    if mast.abs() > 0.01 {
        return Err(format!("the mast is still {mast} mm from the lap centre"));
    }
    Ok(())
}

const PHYSICS_CHECKED: &[Row] = checked_rows!(Editor, true;
    RenderEffectsModel.inclusion_preset_chosen => inclusion_preset;
    RenderEffectsModel.structure_reseed => structure_reseed;
    FacetingMachineModel.sweep_preset_chosen => sweep_preset;
    FacetingMachineModel.reset_requested => machine_reset;
);

// ---------------------------------------------------------------------------------------------
// Editor group, plain rows
// ---------------------------------------------------------------------------------------------

/// Editor setup, Main window, shared state.
const PHYSICS_MAIN_SHARED: &[Row] = rows!(MainWindow, Main, Editor, false;
    SurfaceModel.invoke_grade_chosen(Default::default(), Default::default());
    SurfaceModel.invoke_controls_changed(Default::default());
    SurfaceModel.invoke_no_sweep_set(Default::default(), Default::default());
    SurfaceModel.invoke_facet_picked(Default::default(), Default::default());
    SurfaceModel.invoke_facet_option_chosen(Default::default(), Default::default(), Default::default());
    SurfaceModel.invoke_overlay_changed();
    FacetingMachineModel.invoke_value_changed();
    RenderEffectsModel.invoke_inclusions_edited();
    RenderEffectsModel.invoke_structure_edited();
    RenderEffectsModel.invoke_edges_edited();
    RenderEffectsModel.invoke_silk_toggled(Default::default());
    RenderEffectsModel.invoke_refresh();
    InclusionFamiliesModel.invoke_add_family();
    InclusionFamiliesModel.invoke_remove_family(Default::default());
    InclusionFamiliesModel.invoke_edit_family(Default::default(), Default::default(), Default::default());
    InclusionFamiliesModel.invoke_notes(slint::ModelRc::default(), Default::default());
    InclusionFamiliesModel.invoke_signature(slint::ModelRc::default());
    StructureParamsModel.invoke_load_defaults(Default::default());
    StructureParamsModel.invoke_reseed();
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    [PHYSICS_MAIN_SHARED, PHYSICS_CHECKED]
        .into_iter()
        .flatten()
        .collect()
}
