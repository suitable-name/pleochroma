//! States of the click-through passes for the Render Physics controls: the Stone tab's Inclusions,
//! Play-of-Color and Abraded Edges groups, the Tier tab's Surface Finish section, the Show Surface
//! Finish overlay's toolbar pill, the Preferences dialog's Faceting Machine section and the Gem
//! Material Editor's Per-Axis Curves, Inclusion Families and Play-of-Color sections.
//!
//! Proves: in each state every enabled button, tab, pill, check box, combo box and slider of the
//! scope is pressed once and every field touched (the engine of `overlays.rs`), after the state's
//! `open` function reached the page the way a user does: a material pick, a preset, a grade.
//!
//! What kept the controls unreached: the Inclusions controls exist only while a preset is chosen,
//! the Play-of-Color controls only for a precious opal or a labradorite with the group on, the
//! Abraded Edges wear only with a width, the Custom Surface Finish values only with the Custom
//! grade, the Mast Position and the Sweep radii only with a mast to the side and a custom sweep,
//! and the material editor's curve fields, family rows and structure controls only after their
//! switch is on. The Stone tab and the dialogs are `ScrollView`s whose scrolled-out controls are not
//! enumerated ([`scroll_scope`]).
//!
//! Not here: the tier table's right-click Surface Finish menu (native menu entries are not elements
//! of the window); its callbacks have direct-invoke rows in `rows_physics.rs`.

use super::{
    super::{Piece, existing_install},
    overlay_states::{Base, NONE, State, deny, edit_tab, inspector, parameters, press},
    state_pieces,
    states_axis::design_material,
    states_dialogs::{material_open, saved_material, scroll_until_visible},
    states_reach::scroll_scope,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    smoke::{DenyMatch, DenyRule},
};
use pleochroma_cut::{
    AxisCurvesModel, FacetingMachineModel, InclusionFamiliesModel, PreferencesModel,
    RenderEffectsModel, StructureParamsModel, SurfaceModel,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The pieces of this file: one `overlay:reach-physics:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    state_pieces("reach-physics", existing_install, physics_states)
}

/// Preferences: switching the update check on is a GitHub request.
const DENY_PREFERENCES: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "check for new versions",
    "network: switching it on starts an update check (UpdateModel.enabled_changed)",
)];

/// How far the Stone tab is scrolled to show its last sections (the clamp stops it at the end).
const STONE_END: f32 = -4000.0;

// ---------------------------------------------------------------------------------------------
// Stone tab
// ---------------------------------------------------------------------------------------------

/// Chooses the first inclusion preset of the material on the viewport and widens the edge band, so
/// the Inclusions controls and the wear slider exist.
fn preset_and_edges(a: &HarnessApp) {
    let effects = a.ui().global::<RenderEffectsModel>();
    effects.invoke_refresh();
    let _ = wait_until(Duration::from_secs(8), || {
        effects.get_inclusion_preset_names().row_count() > 0
    });
    if let Some(name) = effects.get_inclusion_preset_names().row_data(0) {
        effects.invoke_inclusion_preset_chosen(name);
    }
    effects.set_edge_width(0.05);
    effects.invoke_edges_edited();
    pump(300);
}

/// Turns the Play-of-Color / Schiller group on for the material on the viewport.
fn structure_on(a: &HarnessApp) {
    let effects = a.ui().global::<RenderEffectsModel>();
    effects.invoke_refresh();
    let _ = wait_until(Duration::from_secs(8), || effects.get_structure_available());
    effects.set_structure_on(true);
    effects.invoke_structure_edited();
    pump(300);
}

/// The Parameters panel on the Stone tab for a design in the material containing `material`,
/// prepared by `prepare` once the panel shows, scrolled by `dy`.
fn stone_tab(a: &HarnessApp, material: &str, prepare: fn(&HarnessApp), dy: f32) {
    edit_tab(a);
    let _ = design_material(a, material);
    parameters(a, 0);
    prepare(a);
    scroll_scope(a, "ParametersPanel", 0.0, dy);
}

// ---------------------------------------------------------------------------------------------
// Tier tab, overlay, Preferences
// ---------------------------------------------------------------------------------------------

/// The Tier tab with tier 0 on the Custom grade, a Custom Angle and a facet list: the Roughness,
/// Anisotropy and Angle sliders and the per-facet override exist.
fn custom_surface_finish(a: &HarnessApp) {
    inspector(a, 0);
    let surface = a.ui().global::<SurfaceModel>();
    let _ = wait_until(Duration::from_secs(8), || {
        surface.get_grade_names().row_count() > 0
    });
    let custom: i32 = surface
        .get_grade_names()
        .row_count()
        .saturating_sub(1)
        .try_into()
        .unwrap_or(0);
    surface.invoke_grade_chosen(0, custom);
    pump(300);
    surface.set_use_angle(true);
    surface.invoke_controls_changed(0);
    pump(300);
}

/// The Edit view with tier 0 on a Fine finish and Show Surface Finish on: the toolbar's pill
/// is pressed in its on state, and the overlay is asked for its marks.
fn surface_overlay_on(a: &HarnessApp) {
    edit_tab(a);
    let surface = a.ui().global::<SurfaceModel>();
    let _ = wait_until(Duration::from_secs(8), || {
        surface.get_grade_names().row_count() > 2
    });
    surface.invoke_grade_chosen(0, 2);
    pump(300);
    surface.set_show_overlay(true);
    surface.invoke_overlay_changed();
    pump(500);
}

/// The Preferences dialog with the mast to the side and a custom sweep, scrolled to the Faceting
/// Machine section's last row.
fn faceting_machine(a: &HarnessApp) {
    a.ui().global::<PreferencesModel>().set_dialog_open(true);
    pump(250);
    let machine = a.ui().global::<FacetingMachineModel>();
    machine.set_mast_distance_mm(30.0);
    machine.invoke_value_changed();
    machine.invoke_sweep_preset_chosen(3);
    pump(250);
    scroll_until_visible(a, "PreferencesDialog", "Reset to the Plain Machine");
}

// ---------------------------------------------------------------------------------------------
// Material editor
// ---------------------------------------------------------------------------------------------

/// The Gem Material Editor over a saved anisotropic material with the Curves switch on, two
/// inclusion families and the Play-of-Color section on, scrolled to the end.
fn editor_sections(a: &HarnessApp, name: &str, crystal: i32, character: i32, delta: f32) {
    saved_material(a, name, crystal, character, 0.01, delta, "");
    material_open(a);
    scroll_until_visible(a, "MaterialEditorDialog", "Curves");
    if !press(a, "Curves", None) {
        println!("editor_sections: the Curves switch was not pressed");
    }
    let curves = a.ui().global::<AxisCurvesModel>();
    if curves.get_mode() != 1 {
        curves.set_mode(1);
    }
    let families = a.ui().global::<InclusionFamiliesModel>();
    families.invoke_add_family();
    families.invoke_add_family();
    let structure = a.ui().global::<StructureParamsModel>();
    structure.set_enabled(true);
    structure.invoke_load_defaults(1);
    pump(300);
    scroll_scope(a, "MaterialEditorDialog", 0.0, STONE_END);
}

fn physics_states() -> Vec<State> {
    vec![
        State {
            name: "Parameters: Stone, inclusions and edges of a sapphire",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "sapphire", preset_and_edges, -2400.0),
        },
        State {
            name: "Parameters: Stone, inclusions and edges of a sapphire, end",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "sapphire", preset_and_edges, STONE_END),
        },
        State {
            name: "Parameters: Stone, cat's-eye of a chrysoberyl",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "chrysoberyl", preset_and_edges, STONE_END),
        },
        State {
            name: "Parameters: Stone, play-of-color of a precious opal",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "precious opal", structure_on, STONE_END),
        },
        State {
            name: "Parameters: Stone, schiller of a labradorite",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "labradorite", structure_on, STONE_END),
        },
        State {
            name: "Inspector: Tier, custom surface finish",
            base: Base::Design,
            scope: Some("EditorInspector"),
            deny: NONE,
            open: custom_surface_finish,
        },
        State {
            name: "Edit view, surface finish drawn",
            base: Base::Design,
            scope: Some("SolidViewportToolbar"),
            deny: NONE,
            open: surface_overlay_on,
        },
        State {
            name: "Preferences, faceting machine",
            base: Base::Startup,
            scope: Some("PreferencesDialog"),
            deny: DENY_PREFERENCES,
            open: faceting_machine,
        },
        State {
            name: "Material editor, curves, families and structure of a uniaxial material",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: |a| editor_sections(a, "Harness Curves Uniaxial", 3, 2, 0.0),
        },
        State {
            name: "Material editor, curves of a biaxial material",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: |a| editor_sections(a, "Harness Curves Biaxial", 4, 3, 0.004),
        },
    ]
}
