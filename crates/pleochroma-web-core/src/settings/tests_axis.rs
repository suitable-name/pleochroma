//! The Crystal Axis and Second Axis of a web render: the linked design's own effects win over the
//! per-user setting, the per-user setting applies to an unlinked render, and a render with no
//! Second Axis is exactly what it was before the Second Axis existed.

use super::*;
use glam::Vec3;
use pleochroma_cut_core::CrystalOrientation;

/// The Second Axis of `(tilt, azimuth, turn)` as the render sends it, built from the canonical
/// axis line.
fn alpha_of(tilt: f64, azimuth: f64, turn: f64) -> Option<[f32; 3]> {
    CrystalOrientation::from_angles(tilt, azimuth, Some(turn))
        .alpha()
        .map(|alpha| alpha.to_array())
}

/// Whether two optional directions are the same LINE (alpha has no sign).
fn same_line(a: Option<[f32; 3]>, b: Option<[f32; 3]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Vec3::from_array(a).dot(Vec3::from_array(b)).abs() > 1.0 - 1e-5,
        (a, b) => a == b,
    }
}

fn design_with(effects: StoneRenderEffects) -> Design {
    let mut design = pleochroma_editor::EditorSession::fresh().design;
    design.material.name = Some("Tanzanite".to_string());
    design.render_effects = effects;
    design
}

fn axis_effects(tilt: f64, azimuth: f64, second: Option<f64>) -> StoneRenderEffects {
    StoneRenderEffects {
        c_axis_override_enabled: true,
        c_axis_tilt_deg: tilt,
        c_axis_azimuth_deg: azimuth,
        c_axis_second_azimuth_deg: second,
        ..StoneRenderEffects::default()
    }
}

/// A per-user axis that no design below shares, so a mix-up shows.
fn user_axis() -> RenderSettings {
    RenderSettings {
        c_axis_override: true,
        c_axis_tilt_deg: 10.0,
        c_axis_azimuth_deg: 20.0,
        c_axis_second_on: true,
        c_axis_second_deg: 99.0,
        ..RenderSettings::default()
    }
}

#[test]
fn a_linked_designs_axis_wins_over_the_per_user_setting() {
    let design = design_with(axis_effects(40.0, 100.0, Some(37.0)));
    let material = render_material(&user_axis(), Some(&design), &[]).expect("resolves");
    let overrides = user_axis().material_overrides_for(&material);
    let gamma = c_axis_from_angles(40.0, 100.0);
    assert_eq!(overrides.c_axis_override, Some(gamma.to_array()));
    assert!(same_line(
        overrides.c_axis_second,
        alpha_of(40.0, 100.0, 37.0)
    ));
}

#[test]
fn the_second_axis_is_the_same_line_for_either_end_of_the_axis() {
    // At tilt 90 an azimuth of 200 is the axis line of azimuth 20: the turn means the same
    // direction on both.
    let (_, high) = axis::axis_pair(true, 90.0, 200.0, Some(30.0));
    let (_, low) = axis::axis_pair(true, 90.0, 20.0, Some(30.0));
    assert!(same_line(high, low), "{high:?} vs {low:?}");
}

#[test]
fn a_linked_design_without_an_axis_traces_the_material_as_cut() {
    // The design's override is off: the per-user axis is not applied while linked.
    let design = design_with(StoneRenderEffects::default());
    let material = render_material(&user_axis(), Some(&design), &[]).expect("resolves");
    let overrides = user_axis().material_overrides_for(&material);
    assert_eq!(overrides.c_axis_override, None);
    assert_eq!(overrides.c_axis_second, None);
}

#[test]
fn a_design_axis_without_a_turn_sends_no_second_axis() {
    let design = design_with(axis_effects(90.0, 0.0, None));
    let material = render_material(&user_axis(), Some(&design), &[]).expect("resolves");
    let overrides = user_axis().material_overrides_for(&material);
    assert_eq!(
        overrides.c_axis_override,
        Some(c_axis_from_angles(90.0, 0.0).to_array())
    );
    assert_eq!(overrides.c_axis_second, None);
}

#[test]
fn an_unlinked_render_uses_the_per_user_axis_and_second_axis() {
    let settings = RenderSettings {
        link_material: false,
        ..user_axis()
    };
    let design = design_with(axis_effects(40.0, 100.0, Some(37.0)));
    let material = render_material(&settings, Some(&design), &[]).expect("resolves");
    assert_eq!(material.linked_effects, None);
    let overrides = settings.material_overrides_for(&material);
    let gamma = c_axis_from_angles(10.0, 20.0);
    assert_eq!(overrides.c_axis_override, Some(gamma.to_array()));
    assert!(same_line(
        overrides.c_axis_second,
        alpha_of(10.0, 20.0, 99.0)
    ));
}

#[test]
fn the_per_user_second_axis_needs_its_switch_and_an_axis() {
    // Switch off: no Second Axis, byte-identical to a build before it existed.
    let off = RenderSettings {
        c_axis_second_on: false,
        ..user_axis()
    };
    assert_eq!(off.material_overrides(0.0).c_axis_second, None);
    // No Crystal Axis: no Second Axis either.
    let no_axis = RenderSettings {
        c_axis_override: false,
        ..user_axis()
    };
    let none = no_axis.material_overrides(0.0);
    assert_eq!((none.c_axis_override, none.c_axis_second), (None, None));
}

#[test]
fn the_second_axis_turn_is_clamped_to_a_half_turn() {
    let wild = RenderSettings {
        c_axis_second_deg: 400.0,
        ..RenderSettings::default()
    }
    .sanitized();
    assert_eq!(wild.c_axis_second_deg, 180.0);
    let not_a_number = RenderSettings {
        c_axis_second_deg: f32::NAN,
        ..RenderSettings::default()
    }
    .sanitized();
    assert_eq!(not_a_number.c_axis_second_deg, 0.0);
}

#[test]
fn only_a_biaxial_material_offers_a_second_axis() {
    let unlinked = |name: &str| RenderSettings {
        material: name.to_string(),
        link_material: false,
        ..RenderSettings::default()
    };
    assert!(render_material_is_biaxial(
        &unlinked("Tanzanite"),
        None,
        &[]
    ));
    assert!(!render_material_is_biaxial(
        &unlinked("Sapphire"),
        None,
        &[]
    ));
    assert!(!render_material_is_biaxial(&unlinked("Diamond"), None, &[]));
    assert!(!render_material_is_biaxial(
        &unlinked("Unobtainium"),
        None,
        &[]
    ));
    // Linked: the design's material decides, not the header's.
    let design = design_with(StoneRenderEffects::default());
    let linked = RenderSettings {
        material: "Diamond".to_string(),
        link_material: true,
        ..RenderSettings::default()
    };
    assert!(render_material_is_biaxial(&linked, Some(&design), &[]));
}

#[test]
fn an_older_payload_without_the_second_axis_loads_with_it_off() {
    let payload =
        SessionPayload::from_json(r#"{"render":{"c_axis_override":true,"c_axis_tilt_deg":30.0}}"#)
            .expect("parses");
    assert!(!payload.render.c_axis_second_on);
    assert_eq!(payload.render.c_axis_second_deg, 0.0);
}
