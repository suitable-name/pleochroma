//! The Rust half of the Stone tab's Crystal Axis section and of the axis readouts
//! (`CrystalAxisModel`, `ui/models/crystal_axis.slint`).
//!
//! The values themselves (the "set for this stone" switch, tilt, azimuth, the Second Axis) are the
//! design's render effects and ride the same live path as the other Stone-tab controls
//! (`render::stone_effects`, `render::material`): a change shows at once in the render context
//! and a debounced commit writes ONE undoable `Edit::SetRenderEffects`. This module adds
//!
//! - the presets (Through the Table, In the Table Plane, Along the Width, and the ones named by
//!   the colors they give), each one a single set of the controls and so one undo step;
//! - the Second Axis switch and turn;
//! - the section's derived state: whether the material is biaxial, its color-named presets, which
//!   preset the axis equals, the "second axis not set" note;
//! - the readouts of the metrics strip (the axis angle to the table normal and Doubling);
//! - the "Crystal Axis" toolbar toggle's persistence and its session-only switch-on
//!   ([`show_axis_for_session`], which the planner's "Open in Editor" calls for a stone with an
//!   axis).
//!
//! [`effective_axes`] is the one place the overlay (`editor::manipulate::crystal_axis_overlay`)
//! asks what axis the stone shows.

use crate::{
    CrystalAxisModel, MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{RenderContext, resolve_material_with_override},
    gui::{
        is_c_axis_override_available,
        optics::c_axis::{angle_to_table_normal_deg, angles_to_c_axis},
        render::stone_effects,
        tutorial_events::{raise, raise_weak},
    },
};
use glam::{DVec3, Vec3};
use pleochroma::{
    geometry::{
        plane::GpuFacetPlane,
        stone_metrics::{caliper_frame, measure_solid_with_vertices},
    },
    optics::{birefringence::alpha_axis_from_turn, materials::GemMaterial},
};
use pleochroma_cut_core::{
    AxisPreset, ColorPreset, CrystalAxis, StoneRenderEffects, color_presets, width_azimuth_deg,
};
use pleochroma_editor::guide::viewing_events as events;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

/// Shown under the controls while a biaxial stone with a pleochroic color has no Second Axis.
const SECOND_AXIS_NOTE: &str =
    "Second axis not set; the color across the first axis is approximate.";

/// How close, in degrees, an axis must be to a preset's for the preset to light up.
const PRESET_TOLERANCE_DEG: f64 = 0.5;

/// The materials whose color across the first axis depends on the Second Axis, by name; a
/// custom material counts too when its absorption is pleochroic.
const SECOND_AXIS_NOTE_MATERIALS: [&str; 3] = ["Tanzanite", "Alexandrite", "Andalusite"];

/// The axes the stone currently shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::gui) struct EffectiveAxes {
    /// The Crystal Axis (a line; the unit vector, any sign).
    pub(in crate::gui) gamma: Vec3,
    /// The Second Axis (alpha) of a biaxial stone: perpendicular to `gamma`.
    pub(in crate::gui) alpha: Option<Vec3>,
}

/// The material the render context traces, as the Stone tab sees it.
pub(in crate::gui) fn resolved_material(ctx: &RenderContext) -> Option<GemMaterial> {
    resolve_material_with_override(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        ctx.material_override.as_ref(),
        &ctx.material_name,
    )
}

/// Whether the second direction of `material` matters to its color across the first axis, so
/// the "second axis not set" note applies.
fn needs_second_axis_note(material: &GemMaterial) -> bool {
    if material.biaxial_delta_beta_alpha.is_none() || !material.absorption.is_pleochroic {
        return false;
    }
    // A custom material is one whose name is not exactly a built-in's: `by_name` also matches a
    // built-in name inside a longer one ("My Tanzanite"), which would hide the note.
    SECOND_AXIS_NOTE_MATERIALS.contains(&material.name.as_str())
        || !GemMaterial::all_materials()
            .iter()
            .any(|built_in| built_in.name.eq_ignore_ascii_case(&material.name))
}

/// The axes the stone shows now, or `None` for an isotropic or unresolved material: the controls'
/// own setting while "Set for This Stone" is on, else the material's axis. `alpha` only for a
/// biaxial material.
pub(in crate::gui) fn effective_axes(
    ui: &MainWindow,
    material: &GemMaterial,
) -> Option<EffectiveAxes> {
    if !is_c_axis_override_available(material) {
        return None;
    }
    let settings = ui.global::<SettingsModel>();
    let biaxial = material.biaxial_delta_beta_alpha.is_some();
    if settings.get_c_axis_override_enabled() {
        let gamma = angles_to_c_axis(
            settings.get_c_axis_tilt_deg().clamp(0.0, 90.0),
            settings.get_c_axis_azimuth_deg().clamp(0.0, 360.0),
        );
        // A turn set for this stone, else the material's own completion (turn zero).
        let axis = ui.global::<CrystalAxisModel>();
        let turn = if axis.get_second_set() {
            axis.get_second_deg()
        } else {
            0.0
        };
        // About the canonical axis line, as the render and the sheet turn it (alpha is a line,
        // so the sign of the slider vector must not change what a turn means).
        let alpha = biaxial.then(|| {
            alpha_axis_from_turn(
                CrystalAxis::from_angles(
                    f64::from(settings.get_c_axis_tilt_deg().clamp(0.0, 90.0)),
                    f64::from(settings.get_c_axis_azimuth_deg().clamp(0.0, 360.0)),
                )
                .direction()
                .as_vec3(),
                turn,
            )
        });
        return Some(EffectiveAxes { gamma, alpha });
    }
    let gamma = material.c_axis;
    let alpha = biaxial.then(|| {
        material
            .alpha_axis
            .unwrap_or_else(|| alpha_axis_from_turn(gamma, 0.0))
    });
    Some(EffectiveAxes { gamma, alpha })
}

// --- The design's width direction ---------------------------------------------------------

/// What [`design_width_azimuth_deg`] last measured, keyed on the planes it measured.
type WidthCache = Option<((usize, usize, u32), Option<f64>)>;

thread_local! {
    static WIDTH_CACHE: RefCell<WidthCache> = const { RefCell::new(None) };
}

/// The azimuth, degrees `[0, 180)`, of the width direction of the stone `planes` bound (the
/// rotating-caliper minimum width of its outline, `CaliperFrame::width_dir`), or `None` without
/// a closed stone. Remembered for the planes it measured: the Stone tab asks on every refresh.
pub(in crate::gui) fn design_width_azimuth_deg(planes: &Arc<Vec<GpuFacetPlane>>) -> Option<f64> {
    let key = (
        Arc::as_ptr(planes) as usize,
        planes.len(),
        planes.iter().fold(0_u32, |sum, plane| {
            sum.wrapping_add(plane.d.to_bits())
                .wrapping_add(plane.normal[1].to_bits())
        }),
    );
    if let Some(known) = WIDTH_CACHE.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|(cached, _)| *cached == key)
            .map(|(_, azimuth)| *azimuth)
    }) {
        return known;
    }
    let azimuth = measure_width_azimuth_deg(planes);
    WIDTH_CACHE.with(|cell| *cell.borrow_mut() = Some((key, azimuth)));
    azimuth
}

/// [`design_width_azimuth_deg`] without the cache.
fn measure_width_azimuth_deg(planes: &[GpuFacetPlane]) -> Option<f64> {
    let converted: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|plane| {
            (
                DVec3::new(
                    f64::from(plane.normal[0]),
                    f64::from(plane.normal[1]),
                    f64::from(plane.normal[2]),
                ),
                -f64::from(plane.d),
            )
        })
        .collect();
    let (_, vertices) = measure_solid_with_vertices(&converted)?;
    let outline: Vec<(f64, f64)> = vertices.iter().map(|v| (v.x, v.z)).collect();
    let frame = caliper_frame(&outline)?;
    width_azimuth_deg(frame.width_dir)
}

// --- Which preset the axis equals ---------------------------------------------------------

/// Whether two axis lines (tilt and azimuth in degrees) are the same line to within
/// [`PRESET_TOLERANCE_DEG`].
fn same_line(a: (f64, f64), b: (f64, f64)) -> bool {
    let a = CrystalAxis::from_angles(a.0, a.1).direction();
    let b = CrystalAxis::from_angles(b.0, b.1).direction();
    a.dot(b).abs() >= PRESET_TOLERANCE_DEG.to_radians().cos()
}

/// Whether two Second Axis turns (a line, so modulo a half turn) agree to within a degree.
fn same_turn(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d) < 1.0
}

/// The preset the axis equals, in the numbering of `CrystalAxisModel.active_preset`: 0 to 2 the
/// geometry presets, 3 and up the color presets in order, -1 none. A color preset wins over a
/// geometry one it coincides with (it is the more specific).
///
/// `axis` is `(tilt, azimuth)` of the Crystal Axis while "Set for This Stone" is on (`None`
/// while the material's own axis is used: no preset is lit then), `second` the Second Axis turn
/// when it is set.
pub(in crate::gui) fn active_preset(
    axis: Option<(f64, f64)>,
    second: Option<f64>,
    width_azimuth: Option<f64>,
    colors: &[ColorPreset],
) -> i32 {
    let Some(axis) = axis else {
        return -1;
    };
    for (index, preset) in colors.iter().enumerate() {
        let wanted = (
            preset.orientation.axis.tilt_deg(),
            preset.orientation.axis.azimuth_deg(),
        );
        let turn_agrees = match (second, preset.orientation.second_turn_deg) {
            (Some(have), Some(want)) => same_turn(have, want),
            _ => false,
        };
        if same_line(axis, wanted) && turn_agrees {
            return i32::try_from(3 + index).unwrap_or(-1);
        }
    }
    for (index, preset) in AxisPreset::ALL.into_iter().enumerate() {
        if let Some(wanted) = preset.angles(width_azimuth)
            && same_line(axis, wanted)
        {
            return i32::try_from(index).unwrap_or(-1);
        }
    }
    -1
}

// --- The section's state ------------------------------------------------------------------

/// Recomputes everything `CrystalAxisModel` derives: the biaxial flag, the color-named presets,
/// the lit preset, the Along-the-Width availability, the note and the axis readout. Called when
/// the section is shown and whenever an input changed (`CrystalAxisModel.refresh` /
/// `overlay_changed`).
pub(in crate::gui) fn refresh_section(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let (material, planes) = {
        let ctx = RenderContext::lock(render_ctx);
        (resolved_material(&ctx), Arc::clone(&ctx.active_planes))
    };
    let model = ui.global::<CrystalAxisModel>();
    let Some(material) = material.filter(is_c_axis_override_available) else {
        model.set_biaxial(false);
        set_color_presets(&model, &[]);
        model.set_width_preset_available(false);
        model.set_active_preset(-1);
        model.set_note(SharedString::new());
        model.set_axis_angle_text(SharedString::new());
        model.set_doubling_text(SharedString::new());
        return;
    };
    let settings = ui.global::<SettingsModel>();
    let biaxial = material.biaxial_delta_beta_alpha.is_some();
    let presets = color_presets(&material);
    let width_azimuth = design_width_azimuth_deg(&planes);
    let enabled = settings.get_c_axis_override_enabled();
    let second = {
        let axis = ui.global::<CrystalAxisModel>();
        axis.get_second_set()
            .then(|| f64::from(axis.get_second_deg()))
    };
    let axis = enabled.then(|| {
        (
            f64::from(settings.get_c_axis_tilt_deg()),
            f64::from(settings.get_c_axis_azimuth_deg()),
        )
    });
    model.set_biaxial(biaxial);
    set_color_presets(&model, &presets);
    model.set_width_preset_available(width_azimuth.is_some());
    model.set_active_preset(active_preset(axis, second, width_azimuth, &presets));
    let note = if enabled && !model.get_second_set() && needs_second_axis_note(&material) {
        SECOND_AXIS_NOTE
    } else {
        ""
    };
    model.set_note(note.into());
    let angle = effective_axes(ui, &material).map(|axes| angle_to_table_normal_deg(axes.gamma));
    model.set_axis_angle_text(
        angle.map_or_else(SharedString::new, |deg| format!("{deg:.0}\u{b0}").into()),
    );
}

/// Shows `presets` as the section's color-named preset buttons, leaving the model alone when
/// they are already what it shows (so a refresh per slider tick does not rebuild the buttons).
fn set_color_presets(model: &CrystalAxisModel<'_>, presets: &[ColorPreset]) {
    let shown: Vec<String> = model
        .get_color_preset_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let wanted: Vec<&str> = presets.iter().map(|p| p.id.label()).collect();
    if shown.iter().map(String::as_str).eq(wanted.iter().copied()) {
        return;
    }
    model.set_color_preset_names(ModelRc::new(VecModel::from(
        wanted
            .iter()
            .map(|name| SharedString::from(*name))
            .collect::<Vec<_>>(),
    )));
    model.set_color_preset_swatches(ModelRc::new(VecModel::from(
        presets
            .iter()
            .map(|p| {
                slint::Color::from_rgb_u8(p.swatch_srgb[0], p.swatch_srgb[1], p.swatch_srgb[2])
            })
            .collect::<Vec<_>>(),
    )));
}

/// Shows the Doubling figure the metrics worker measured (arc-minutes), for an anisotropic
/// material; the readout stays empty for an isotropic one.
pub(in crate::gui) fn push_doubling(ui: &MainWindow, arcmin: f32) {
    let anisotropic = ui.global::<SettingsModel>().get_c_axis_override_available();
    let text = if anisotropic && arcmin.is_finite() {
        format!("{arcmin:.1} arcmin")
    } else {
        String::new()
    };
    ui.global::<CrystalAxisModel>()
        .set_doubling_text(text.into());
}

// --- Editing ------------------------------------------------------------------------------

/// Switches the toolbar's "Crystal Axis" toggle on for this session, without saving it: the
/// first time the axis is edited, or a planned stone with an axis is opened (the Rough
/// Planner's "Open in Editor" calls this). A toggle the user turned off stays off until they
/// turn it on again, because this is only called when something edits or opens an axis.
pub(in crate::gui) fn show_axis_for_session(ui: &MainWindow) {
    let model = ui.global::<CrystalAxisModel>();
    if !model.get_show_axis() {
        model.set_show_axis(true);
    }
}

/// A control changed the picture: hold the editor refresh off the controls until the design has
/// the new value, then ask for the debounced commit. Nothing to write for a library design,
/// whose effects are view-only.
fn raise_edited(ui: &MainWindow) {
    if !ui.global::<ViewportModel>().get_shows_editor_design() {
        return;
    }
    stone_effects::mark_live_edit_pending();
    ui.global::<SettingsModel>().invoke_stone_effects_edited();
}

/// Makes the render context trace the controls' current effects and queues their commit. The
/// one path every preset and the Second Axis take, so each is one set of the controls and one
/// undo step.
pub(in crate::gui) fn apply_controls(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    // The controls carry no render physics; the context keeps the design's own.
    let effects = stone_effects::effects_from_controls(ui, &StoneRenderEffects::default());
    {
        let mut ctx = RenderContext::lock(render_ctx);
        stone_effects::apply_control_values_to_context(&mut ctx, &effects);
    }
    // Color patterns that follow the Crystal Axis turn with it.
    crate::gui::color_pattern::follow::axis_applied(render_ctx);
    show_axis_for_session(ui);
    raise_edited(ui);
}

/// An angle in degrees at the `f32` precision of the Slint sliders.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the Slint sliders are f32; an angle in degrees is a few hundred at most"
)]
const fn narrow(degrees: f64) -> f32 {
    degrees as f32
}

/// Sets the Crystal Axis controls to `(tilt, azimuth)` and turns "Set for This Stone" on.
fn set_axis_controls(ui: &MainWindow, tilt_deg: f64, azimuth_deg: f64) {
    let settings = ui.global::<SettingsModel>();
    settings.set_c_axis_override_enabled(true);
    settings.set_c_axis_tilt_deg(narrow(tilt_deg).clamp(0.0, 90.0));
    settings.set_c_axis_azimuth_deg(narrow(azimuth_deg).clamp(0.0, 360.0));
}

/// A geometry preset button: sets the tilt and azimuth (the Second Axis is left as it is).
fn choose_geometry_preset(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, index: i32) {
    let Some(preset) = usize::try_from(index)
        .ok()
        .and_then(|i| AxisPreset::ALL.get(i).copied())
    else {
        return;
    };
    let planes = Arc::clone(&RenderContext::lock(render_ctx).active_planes);
    let Some((tilt_deg, azimuth_deg)) = preset.angles(design_width_azimuth_deg(&planes)) else {
        return;
    };
    set_axis_controls(ui, tilt_deg, azimuth_deg);
    apply_controls(ui, render_ctx);
    refresh_section(ui, render_ctx);
    // A tutorial step may wait for a preset.
    raise(ui, events::CRYSTAL_AXIS_PRESET_CHOSEN);
}

/// A color-named preset button: sets the Crystal Axis and the Second Axis together.
fn choose_color_preset(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, index: i32) {
    let material = {
        let ctx = RenderContext::lock(render_ctx);
        resolved_material(&ctx)
    };
    let Some(preset) = material.as_ref().and_then(|material| {
        usize::try_from(index)
            .ok()
            .and_then(|i| color_presets(material).get(i).copied())
    }) else {
        return;
    };
    set_axis_controls(
        ui,
        preset.orientation.axis.tilt_deg(),
        preset.orientation.axis.azimuth_deg(),
    );
    let axis = ui.global::<CrystalAxisModel>();
    axis.set_second_set(true);
    axis.set_second_deg(narrow(preset.orientation.second_turn_deg.unwrap_or(0.0)));
    apply_controls(ui, render_ctx);
    refresh_section(ui, render_ctx);
    raise(ui, events::CRYSTAL_AXIS_COLOR_PRESET_CHOSEN);
}

/// The Second Axis switch or turn changed.
fn change_second_axis(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    set: bool,
    degrees: f32,
) {
    let axis = ui.global::<CrystalAxisModel>();
    axis.set_second_set(set);
    axis.set_second_deg(degrees.clamp(0.0, 180.0));
    apply_controls(ui, render_ctx);
    refresh_section(ui, render_ctx);
    raise(ui, events::CRYSTAL_SECOND_AXIS_CHANGED);
}

/// Wires the `CrystalAxisModel` callbacks of the Stone tab and the toolbar toggle. Called once
/// from `render::material::setup_material_effect_override_callbacks`.
pub(in crate::gui) fn setup(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let model = ui.global::<CrystalAxisModel>();

    let ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    model.on_refresh(move || {
        if let Some(ui) = ui_weak.upgrade() {
            refresh_section(&ui, &ctx);
        }
    });

    let ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    model.on_geometry_preset_chosen(move |index: i32| {
        if let Some(ui) = ui_weak.upgrade() {
            choose_geometry_preset(&ui, &ctx, index);
        }
    });

    let ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    model.on_color_preset_chosen(move |index: i32| {
        if let Some(ui) = ui_weak.upgrade() {
            choose_color_preset(&ui, &ctx, index);
        }
    });

    let ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    model.on_second_axis_changed(move |set: bool, degrees: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            change_second_axis(&ui, &ctx, set, degrees);
        }
    });

    // The toolbar toggle: remembered per user. (The session-only switch-on never reaches this.)
    let ui_weak = ui.as_weak();
    model.on_show_axis_toggled(move |show: bool| {
        crate::gui::preferences::persist_show_crystal_axis(show);
        // A tutorial step may wait for the button, whichever way it turned.
        raise_weak(&ui_weak, events::CRYSTAL_AXIS_TOGGLED);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ColorPresetId, PrincipalDirection, orientation_through_table};

    fn tanzanite() -> GemMaterial {
        GemMaterial::by_name("Tanzanite").expect("a built-in material")
    }

    #[test]
    fn the_geometry_presets_light_up_for_the_axis_they_set() {
        let width = Some(33.0);
        assert_eq!(active_preset(Some((0.0, 0.0)), None, width, &[]), 0);
        // The azimuth means nothing at tilt 0.
        assert_eq!(active_preset(Some((0.0, 123.0)), None, width, &[]), 0);
        assert_eq!(active_preset(Some((90.0, 0.0)), None, width, &[]), 1);
        assert_eq!(active_preset(Some((90.0, 33.0)), None, width, &[]), 2);
        // The same line from the other side.
        assert_eq!(active_preset(Some((90.0, 213.0)), None, width, &[]), 2);
        assert_eq!(active_preset(Some((45.0, 10.0)), None, width, &[]), -1);
        // "As Material" lights nothing, and without a width the third preset cannot light.
        assert_eq!(active_preset(None, None, width, &[]), -1);
        assert_eq!(active_preset(Some((90.0, 33.0)), None, None, &[]), -1);
    }

    #[test]
    fn a_color_preset_lights_up_only_with_its_second_axis() {
        let presets = color_presets(&tanzanite());
        assert_eq!(presets.len(), 2, "Best Blue Face-Up and Violet Face-Up");
        let blue = presets[0];
        let axis = (
            blue.orientation.axis.tilt_deg(),
            blue.orientation.axis.azimuth_deg(),
        );
        let turn = blue.orientation.second_turn_deg.expect("a turn");
        assert_eq!(active_preset(Some(axis), Some(turn), None, &presets), 3);
        // The same axis with another turn is a different orientation: no color preset, and the
        // axis is in the table plane, so the geometry preset "In the Table Plane" lights.
        assert_eq!(
            active_preset(Some(axis), Some(turn + 40.0), Some(10.0), &presets),
            1
        );
        // Violet Face-Up (gamma through the table) wins over Through the Table.
        let violet = presets[1];
        let violet_axis = (
            violet.orientation.axis.tilt_deg(),
            violet.orientation.axis.azimuth_deg(),
        );
        let violet_turn = violet.orientation.second_turn_deg.expect("a turn");
        assert_eq!(
            active_preset(Some(violet_axis), Some(violet_turn), None, &presets),
            4
        );
        assert_eq!(active_preset(Some(violet_axis), None, None, &presets), 0);
    }

    #[test]
    fn the_color_preset_orientations_put_the_named_direction_through_the_table() {
        let presets = color_presets(&tanzanite());
        for preset in presets {
            let wanted = orientation_through_table(preset.through_table);
            assert_eq!(preset.orientation, wanted);
        }
        assert_eq!(
            color_presets(&tanzanite())[0].id,
            ColorPresetId::BestBlueFaceUp
        );
        assert_eq!(
            color_presets(&tanzanite())[0].through_table,
            PrincipalDirection::Alpha
        );
    }

    #[test]
    fn the_second_axis_note_is_for_the_trichroic_materials_and_pleochroic_custom_ones() {
        assert!(needs_second_axis_note(&tanzanite()));
        for name in ["Alexandrite", "Andalusite"] {
            let material = GemMaterial::by_name(name).expect("a built-in material");
            assert!(needs_second_axis_note(&material), "{name}");
        }
        // Peridot is biaxial and pleochroic but weakly so: no note. Uniaxial: nothing to set.
        for name in ["Peridot", "Sapphire", "Topaz", "Diamond"] {
            let material = GemMaterial::by_name(name).expect("a built-in material");
            assert!(!needs_second_axis_note(&material), "{name}");
        }
        // A custom biaxial material with a pleochroic tensor gets it.
        let mut custom = tanzanite();
        custom.name = "My Tanzanite".to_string();
        assert!(needs_second_axis_note(&custom));
        custom.absorption.is_pleochroic = false;
        assert!(!needs_second_axis_note(&custom));
    }

    #[test]
    fn the_width_azimuth_of_a_box_is_measured_from_its_planes() {
        // A box twice as long along X as along Z: its narrowest direction is along Z.
        let plane = |normal: [f32; 3], d: f32| GpuFacetPlane { normal, d };
        let planes = Arc::new(vec![
            plane([1.0, 0.0, 0.0], -2.0),
            plane([-1.0, 0.0, 0.0], -2.0),
            plane([0.0, 0.0, 1.0], -1.0),
            plane([0.0, 0.0, -1.0], -1.0),
            plane([0.0, 1.0, 0.0], -1.0),
            plane([0.0, -1.0, 0.0], -1.0),
        ]);
        let azimuth = design_width_azimuth_deg(&planes).expect("a closed box");
        assert!((azimuth - 90.0).abs() < 1e-6, "{azimuth}");
        // The second ask is the cached answer.
        assert_eq!(design_width_azimuth_deg(&planes), Some(azimuth));
        // Planes that bound nothing give no width.
        let open = Arc::new(vec![plane([0.0, 1.0, 0.0], -1.0)]);
        assert_eq!(design_width_azimuth_deg(&open), None);
    }
}
