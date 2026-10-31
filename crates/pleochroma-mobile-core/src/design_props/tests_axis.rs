//! Tests of the Stone tab's Crystal Axis beyond the sliders: presets, "As Material", the Second
//! Axis, the material's default orientation and the orientation line.

use std::sync::Arc;

use pleochroma::optics::materials::{GemMaterial, OpticalCharacter};
use pleochroma_cut_core::templates::TEMPLATES;
use pleochroma_editor::EditorSession;
use pleochroma_formats::native::CustomMaterialSnapshot;

use super::*;
use crate::{app_state::AppState, platform::Platform, platform_mock::MockPlatform};

/// A state over the standard round brilliant template, with `material` as the design's material.
fn state_with(material: &str) -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.session.design.material.name = Some(material.to_owned());
    state
}

fn labels(state: &AppState) -> Vec<String> {
    axis_preset_rows(state)
        .into_iter()
        .map(|row| row.label)
        .collect()
}

#[test]
fn the_presets_follow_the_materials_optics() {
    // An isotropic material has no axis: nothing to offer, nothing available.
    let diamond = state_with("Diamond");
    assert_eq!(
        axis_preset_rows(&diamond),
        [] as [crate::design_props::axis::AxisPresetRow; 0]
    );
    assert!(!stone_view(&diamond).axis_available);

    // A uniaxial material gets the three geometry presets and no color-named ones.
    assert_eq!(
        labels(&state_with("Sapphire")),
        ["Through the Table", "In the Table Plane", "Along the Width"]
    );

    // Tanzanite (trichroic) adds its two color-named presets.
    let tanzanite = labels(&state_with("Tanzanite"));
    assert_eq!(tanzanite.len(), 5);
    assert_eq!(tanzanite[3], "Best Blue Face-Up");
    assert_eq!(tanzanite[4], "Violet Face-Up");
}

#[test]
fn a_geometry_preset_turns_the_override_on_and_is_one_undo_step() {
    let mut state = state_with("Sapphire");
    assert_eq!(apply_axis_preset(&mut state, 0), Ok(true));
    let effects = state.session.design.render_effects;
    assert!(effects.c_axis_override_enabled);
    assert_eq!(effects.c_axis_tilt_deg, 0.0);

    assert_eq!(apply_axis_preset(&mut state, 1), Ok(true));
    let effects = state.session.design.render_effects;
    assert_eq!(
        (effects.c_axis_tilt_deg, effects.c_axis_azimuth_deg),
        (90.0, 0.0)
    );
    assert_eq!(apply_axis_preset(&mut state, 1), Ok(false), "already so");

    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(state.session.design.render_effects.c_axis_tilt_deg, 0.0);
}

#[test]
fn along_the_width_needs_a_solved_design() {
    let mut state = state_with("Sapphire");
    let error = apply_axis_preset(&mut state, 2).expect_err("the width is not known yet");
    assert!(error.contains("Solve the design"), "{error}");
    assert!(state.session.design.render_effects.is_default());
}

#[test]
fn a_color_named_preset_sets_both_axes() {
    let mut state = state_with("Tanzanite");
    assert_eq!(apply_axis_preset(&mut state, 3), Ok(true));
    let effects = state.session.design.render_effects;
    assert!(effects.c_axis_override_enabled);
    assert!(
        effects.c_axis_second_azimuth_deg.is_some(),
        "the Second Axis comes with the color"
    );
    assert!(stone_view(&state).axis_second_on);
}

#[test]
fn a_preset_that_does_not_exist_or_an_isotropic_material_is_refused() {
    let mut tanzanite = state_with("Tanzanite");
    assert!(apply_axis_preset(&mut tanzanite, 9).is_err());
    let mut diamond = state_with("Diamond");
    assert!(apply_axis_preset(&mut diamond, 0).is_err());
    assert!(diamond.session.design.render_effects.is_default());
}

#[test]
fn as_material_switches_the_override_off() {
    let mut state = state_with("Sapphire");
    assert_eq!(
        use_material_axis(&mut state),
        Ok(false),
        "already as material"
    );
    apply_axis_preset(&mut state, 1).expect("applies");
    assert_eq!(use_material_axis(&mut state), Ok(true));
    assert!(!state.session.design.render_effects.c_axis_override_enabled);
    assert!(!stone_view(&state).axis_on);
}

#[test]
fn the_second_axis_sets_clamps_and_clears() {
    let mut state = state_with("Tanzanite");
    assert!(stone_view(&state).axis_biaxial);
    assert!(!stone_view(&state_with("Sapphire")).axis_biaxial);

    assert_eq!(set_second_axis(&mut state, true, 400.0), Ok(true));
    assert_eq!(
        state
            .session
            .design
            .render_effects
            .c_axis_second_azimuth_deg,
        Some(180.0)
    );
    assert_eq!(set_second_axis(&mut state, true, 37.0), Ok(true));
    let view = stone_view(&state);
    assert!(view.axis_second_on);
    assert_eq!(view.axis_second_deg, 37.0);

    assert_eq!(set_second_axis(&mut state, false, 37.0), Ok(true));
    assert_eq!(
        state
            .session
            .design
            .render_effects
            .c_axis_second_azimuth_deg,
        None
    );
    assert_eq!(set_second_axis(&mut state, false, 0.0), Ok(false));
}

#[test]
fn the_summary_and_the_orientation_line_say_how_the_axis_runs() {
    let mut state = state_with("Sapphire");
    // No axis set: the stone keeps its material's own, nothing to say.
    assert_eq!(stone_view(&state).axis_summary, "");
    assert_eq!(orientation_line(&state), None);

    apply_axis_preset(&mut state, 0).expect("applies");
    let line = orientation_line(&state).expect("an axis is set");
    assert_eq!(
        line,
        "Orient the rough so the crystal axis (c-axis) runs straight through the table."
    );
    assert_eq!(stone_view(&state).axis_summary, line);

    // An isotropic material has no axis to describe, even with an override stored.
    state.session.design.material.name = Some("Diamond".to_owned());
    assert_eq!(orientation_line(&state), None);
}

/// A custom material that stores a default orientation: it shows as the material's own axis, and
/// the design's setting still wins over it.
#[test]
fn a_custom_materials_default_orientation_is_the_materials_own() {
    let mut state = state_with("Zorbium");
    let mut material = GemMaterial::new_custom("Zorbium", 1.76, 0.02, 0.008, [0.0; 3]);
    material.optical_character = OpticalCharacter::UniaxialNegative;
    let snapshot = CustomMaterialSnapshot::new(1.76, 0.02, 0.008, None, "trigonal", "uniaxial")
        .with_default_orientation(Some(90.0), Some(33.0), None);
    state
        .props
        .register_custom(material, Some(snapshot))
        .expect("registers");

    let default = material_default_orientation(&state).expect("the default is stored");
    assert_eq!(default.axis.tilt_deg(), 90.0);
    assert_eq!(default.axis.azimuth_deg(), 33.0);
    let view = stone_view(&state);
    assert!(view.axis_available);
    assert!(view.axis_material_default);
    assert!(!view.axis_on, "the design itself sets nothing");
    assert!(
        view.axis_summary.contains("azimuth 33"),
        "{}",
        view.axis_summary
    );

    // The design's own axis beats the material's default.
    apply_axis_preset(&mut state, 0).expect("applies");
    assert_eq!(
        orientation_line(&state).as_deref(),
        Some("Orient the rough so the crystal axis (c-axis) runs straight through the table.")
    );
    // "As Material" gives the default back.
    use_material_axis(&mut state).expect("switches off");
    assert!(
        orientation_line(&state)
            .expect("the material default")
            .contains("azimuth 33")
    );
}

/// A built-in or a custom material without a stored default brings no orientation.
#[test]
fn a_material_without_a_stored_default_brings_none() {
    assert_eq!(material_default_orientation(&state_with("Sapphire")), None);
    assert_eq!(
        material_default_orientation(&state_with("Nothing Here")),
        None
    );
}
