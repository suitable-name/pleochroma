//! The shots of the Render Physics controls: the Stone tab's Inclusions, Abraded Edges and
//! Play-of-Color / Schiller groups, the Tier tab's Surface Finish section, the Show Surface Finish
//! overlay, the Preferences dialog's Faceting Machine section and the Gem Material Editor's
//! Per-Axis Curves, Inclusion Families and Play-of-Color sections. Each drives the real app through
//! the model calls its controls make.
//!
//! Not here: the tier table's right-click Surface Finish menu. The harness has no pointer API for a
//! right click, and the menu entries are native menu items, not elements of the window; the menu's
//! callbacks are driven by `coverage/rows_physics.rs` in the UI harness.

use crate::{
    axis_shots::section_into_view,
    dialog_shots::{card_shot, open_material_editor, scroll_to_label, wait_component},
    kit::{Rect, Sc, click_first, find_inherits, rect_of},
    nav::{CROWN_TIER, FVS, STUDIO},
    support::app::pump,
};
use pleochroma_cut::{
    EditorModel, FacetingMachineModel, ParametersModel, PhysicscolorModel, PreferencesModel,
    RenderEffectsModel, SurfaceModel,
};
use slint::{ComponentHandle, Model};
use uiprobe::screenshot::Shot;

/// How far one wheel step scrolls a dialog, in logical pixels.
const WHEEL_STEP: f32 = 300.0;
/// The edge band these shots set, in model units, and its wear.
const SHOWN_EDGE: (f32, f32) = (0.05, 0.4);
/// The grade the surface shots give tier 0: an index of the grade names (2 is Fine).
const SHOWN_GRADE: i32 = 2;

// ---------------------------------------------------------------------------------------------
// Stone tab
// ---------------------------------------------------------------------------------------------

/// Chooses the first inclusion preset of the material on the viewport.
fn choose_first_preset(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let effects = ui.global::<RenderEffectsModel>();
    effects.invoke_refresh();
    if !Sc::wait(15, || effects.get_inclusion_preset_names().row_count() > 0) {
        return Err("the material offers no inclusion preset".to_owned());
    }
    let name = effects
        .get_inclusion_preset_names()
        .row_data(0)
        .unwrap_or_default();
    effects.invoke_inclusion_preset_chosen(name);
    pump(800);
    Ok(())
}

/// Widens the edge band so the wear slider shows.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a `prepare` step of `stone_group`, which all return a result"
)]
fn widen_edges(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let effects = ui.global::<RenderEffectsModel>();
    effects.set_edge_width(SHOWN_EDGE.0);
    effects.set_edge_wear(SHOWN_EDGE.1);
    effects.invoke_edges_edited();
    pump(800);
    Ok(())
}

/// Turns the Play-of-Color / Schiller group on.
fn structure_on(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let effects = ui.global::<RenderEffectsModel>();
    effects.invoke_refresh();
    if !Sc::wait(15, || effects.get_structure_available()) {
        return Err("the material offers no Play-of-Color group".to_owned());
    }
    effects.set_structure_on(true);
    effects.invoke_structure_edited();
    pump(800);
    Ok(())
}

/// The Stone tab of a library design in `material`, prepared by `prepare`, scrolled to the section
/// `component`, cropped to the Parameters panel.
fn stone_group(
    sc: &Sc<'_, '_>,
    material: &str,
    component: &str,
    prepare: fn(&Sc<'_, '_>) -> Result<(), String>,
) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.choose_material(material)?;
    sc.show_live_view();
    sc.ui().global::<ParametersModel>().invoke_show_tab(0);
    pump(1000);
    prepare(sc)?;
    pump(1000);
    let panel = section_into_view(sc, component)?;
    let shot = sc.snap_main()?;
    let view = sc.view();
    let area = rect_of(&view, "ParametersDock").unwrap_or(panel);
    Ok(area.crop(&shot))
}

/// `physics_stone_inclusions.png`: the Stone tab of a sapphire with its first inclusion preset on.
///
/// # Errors
///
/// The design or the material is missing, or the section is not on screen.
pub fn stone_inclusions(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_group(sc, "sapphire", "InclusionsSection", choose_first_preset)
}

/// `physics_stone_edges.png`: the Stone tab's Abraded Edges group with a band and its wear.
///
/// # Errors
///
/// As [`stone_inclusions`].
pub fn stone_edges(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_group(sc, "sapphire", "EdgesSection", widen_edges)
}

/// `physics_stone_play_of_color.png`: the Stone tab of a precious opal with Play-of-Color on.
///
/// # Errors
///
/// As [`stone_inclusions`], or the material has no Play-of-Color group.
pub fn stone_play_of_color(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_group(
        sc,
        "precious opal (white)",
        "StructureSection",
        structure_on,
    )
}

/// `physics_stone_schiller.png`: the Stone tab of a labradorite with Schiller on.
///
/// # Errors
///
/// As [`stone_play_of_color`].
pub fn stone_schiller(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_group(sc, "labradorite", "StructureSection", structure_on)
}

// ---------------------------------------------------------------------------------------------
// Tier tab and overlay
// ---------------------------------------------------------------------------------------------

/// Gives the selected tier the grade `SHOWN_GRADE` and waits for the section to show it.
fn grade_selected_tier(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let surface = ui.global::<SurfaceModel>();
    if !Sc::wait(15, || surface.get_grade_names().row_count() > 2) {
        return Err("the Surface Finish grades were not listed".to_owned());
    }
    let tier = ui.global::<EditorModel>().get_selected_tier_index();
    surface.invoke_grade_chosen(tier, SHOWN_GRADE);
    pump(1000);
    Ok(())
}

/// `physics_surface_inspector.png`: the Tier tab's Surface Finish section of a crown tier with the
/// Fine grade.
///
/// # Errors
///
/// The design is missing, or the section is not on screen.
pub fn surface_inspector(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.select_tier(CROWN_TIER)?;
    sc.set_inspector_tab(0);
    sc.expand_inspector();
    grade_selected_tier(sc)?;
    let mut section = None;
    for _ in 0..30 {
        section = find_inherits(&sc.view(), "SurfaceFinishSection");
        if section.is_some() {
            break;
        }
        let view = sc.view();
        if let Some(inspector) = find_inherits(&view, "EditorInspector") {
            let area = inspector
                .query_descendants()
                .match_inherits("ScrollView")
                .find_first();
            area.as_ref().unwrap_or(&inspector).scroll(0.0, -WHEEL_STEP);
        }
        pump(150);
    }
    let section =
        section.ok_or_else(|| "the Surface Finish section never came into view".to_owned())?;
    pump(300);
    let shot = sc.snap_main()?;
    Ok(Rect::of(&section).crop(&shot))
}

/// `physics_surface_overlay.png`: the Edit view in Solid shading with Show Surface Finish on.
///
/// # Errors
///
/// The design is missing, or the overlay is not drawn.
pub fn surface_overlay(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.select_tier(CROWN_TIER)?;
    grade_selected_tier(sc)?;
    sc.set_view_mode(0);
    let ui = sc.ui();
    let surface = ui.global::<SurfaceModel>();
    surface.set_show_overlay(true);
    surface.invoke_overlay_changed();
    if !Sc::wait(30, || surface.get_overlay_visible()) {
        return Err("the surface finish was not drawn over the stone".to_owned());
    }
    pump(1500);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let viewport =
        rect_of(&view, "SolidViewportView").or_else(|_| rect_of(&view, "GemViewportView"))?;
    Ok(viewport.crop(&shot))
}

// ---------------------------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------------------------

/// `physics_faceting_machine.png`: the Preferences dialog scrolled to the Faceting Machine section
/// with the mast to the side.
///
/// # Errors
///
/// The design is missing, or the section does not come into view.
pub fn faceting_machine(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    let ui = sc.ui();
    ui.global::<PreferencesModel>().set_dialog_open(true);
    wait_component(sc, "PreferencesDialog")?;
    let machine = ui.global::<FacetingMachineModel>();
    machine.set_mast_distance_mm(35.0);
    machine.invoke_value_changed();
    pump(500);
    scroll_to_label(sc, "PreferencesDialog", "Reset to the Plain Machine")?;
    pump(300);
    card_shot(sc, "PreferencesDialog")
}

/// Scrolls the scroll area of the dialog `component` on by `dy` logical pixels.
fn scroll_dialog(sc: &Sc<'_, '_>, component: &str, dy: f32) {
    let view = sc.view();
    if let Some(root) = find_inherits(&view, component) {
        let area = root
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        area.as_ref().unwrap_or(&root).scroll(0.0, -dy);
    }
    pump(300);
}

/// The Gem Material Editor on a sapphire, scrolled to the control labelled `anchor`, and on by
/// `extra` logical pixels so the section's rows show; `then` runs once the anchor is on screen.
fn material_section(
    sc: &Sc<'_, '_>,
    anchor: &str,
    extra: f32,
    then: impl FnOnce(&Sc<'_, '_>) -> Result<(), String>,
) -> Result<Shot, String> {
    open_material_editor(sc, "sapphire")?;
    scroll_to_label(sc, "MaterialEditorDialog", anchor)?;
    then(sc)?;
    scroll_dialog(sc, "MaterialEditorDialog", extra);
    card_shot(sc, "MaterialEditorDialog")
}

/// Presses the control labelled `label` and lets the dialog follow.
fn press(sc: &Sc<'_, '_>, label: &str) -> Result<(), String> {
    if !click_first(&sc.view(), label) {
        return Err(format!("there is no {label:?} control to press"));
    }
    pump(500);
    Ok(())
}

/// What a host choice proposes to the open Gem Material Editor for a sapphire, as the physics state
/// pushes it (`PhysicscolorModel.prefill_*`): the editor starts on the Cubic / Isotropic defaults
/// for a built-in material, and its Per-Axis Curves section belongs to a doubly refracting one.
fn propose_sapphire_optics(sc: &Sc<'_, '_>) {
    let ui = sc.ui();
    let physics = ui.global::<PhysicscolorModel>();
    physics.set_prefill_ri(1.77);
    physics.set_prefill_dispersion(0.018);
    physics.set_prefill_birefringence(-0.008);
    physics.set_prefill_sg(4.0);
    // The crystal system list is Cubic, Tetragonal, Hexagonal, Trigonal ...; the optical character
    // list is Isotropic, Uniaxial (+), Uniaxial (-), Biaxial (+), Biaxial (-).
    physics.set_prefill_crystal_system_idx(3);
    physics.set_prefill_optical_character_idx(2);
    physics.set_prefill_biaxial_delta(0.0);
    physics.set_prefill_serial(physics.get_prefill_serial() + 1);
    pump(500);
}

/// `physics_material_curves.png`: the Gem Material Editor's Per-Axis Curves section on a sapphire
/// with the Curves switch on.
///
/// # Errors
///
/// The design or the material is missing, or the section does not come into view.
pub fn material_curves(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    open_material_editor(sc, "sapphire")?;
    propose_sapphire_optics(sc);
    scroll_to_label(sc, "MaterialEditorDialog", "Per-Axis Curves:")?;
    press(sc, "Curves")?;
    scroll_dialog(sc, "MaterialEditorDialog", WHEEL_STEP);
    card_shot(sc, "MaterialEditorDialog")
}

/// `physics_material_families.png`: the Inclusion Families section with two families added.
///
/// # Errors
///
/// As [`material_curves`].
pub fn material_families(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    material_section(sc, "Inclusion Families:", WHEEL_STEP, |sc| {
        press(sc, "Add Family")?;
        press(sc, "Add Family")
    })
}

/// `physics_material_structure.png`: the Play-of-Color section with the switch on.
///
/// # Errors
///
/// As [`material_curves`].
pub fn material_structure(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    material_section(sc, "Play-of-Color", WHEEL_STEP * 2.0, |sc| {
        press(sc, "Play-of-Color")
    })
}
