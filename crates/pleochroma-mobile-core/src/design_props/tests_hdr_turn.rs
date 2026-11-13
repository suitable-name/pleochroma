//! The HDR map's turn on the Light tab: the azimuth slider turns the map while it is the
//! lighting, the follow switch keeps the picture, and a different map starts unturned.

use std::sync::Arc;

use super::*;
use crate::{
    app_state::AppState,
    platform::{PickedFile, Platform},
    platform_mock::MockPlatform,
    settings::MobileSettings,
};

fn state() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    AppState::new(platform)
}

/// A flat (uncompressed) Radiance file.
fn radiance_bytes(width: usize, height: usize) -> Vec<u8> {
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.push(100 + u8::try_from(x * 3 % 100).expect("below 100"));
            bytes.push(100 + u8::try_from(y * 2 % 100).expect("below 100"));
            bytes.push(120);
            bytes.push(128 + u8::try_from(x % 3).expect("below 3"));
        }
    }
    bytes
}

/// Imports a map, which makes it the chosen lighting.
fn import_map(state: &mut AppState, width: usize) {
    let file = PickedFile {
        display_name: "sky.hdr".to_owned(),
        bytes: radiance_bytes(width, width / 2),
    };
    crate::hdr::import(state, &file).expect("imports");
    assert!(state.hdr.is_selected());
}

fn turn_to(state: &mut AppState, azimuth_deg: f32) {
    let values = LightValues {
        azimuth_deg,
        ..lighting_view(state).values
    };
    set_light_values(state, values);
}

/// Whether two angles in radians are the same direction.
fn same_angle(a: f32, b: f32) -> bool {
    let (sa, ca) = a.sin_cos();
    let (sb, cb) = b.sin_cos();
    (sa - sb).abs() < 1e-4 && (ca - cb).abs() < 1e-4
}

#[test]
fn a_settings_file_from_before_the_map_turn_loads_it_unturned() {
    let old: MobileSettings = toml::from_str("[lighting]\nexposure = 1.5\n").expect("parses");
    assert_eq!(old.lighting.exposure, 1.5);
    assert_eq!(old.lighting.hdr_yaw_deg.to_bits(), 0.0_f32.to_bits());
    assert!(!old.lighting.hdr_follows_camera);

    let mut turned = MobileSettings::default();
    turned.lighting.hdr_yaw_deg = 135.0;
    turned.lighting.hdr_follows_camera = true;
    let text = toml::to_string_pretty(&turned).expect("serializes");
    let back: MobileSettings = toml::from_str(&text).expect("parses");
    assert_eq!(back.lighting, turned.lighting);
}

#[test]
fn the_azimuth_moves_the_light_without_a_map_and_turns_the_map_with_one() {
    let mut state = state();
    turn_to(&mut state, 120.0);
    assert_eq!(state.settings.lighting.azimuth_deg, 120.0);
    assert_eq!(state.settings.lighting.hdr_yaw_deg, 0.0);

    import_map(&mut state, 64);
    assert_eq!(
        lighting_view(&state).values.azimuth_deg,
        0.0,
        "the slider shows the map's rotation"
    );
    turn_to(&mut state, 200.0);
    assert_eq!(state.settings.lighting.hdr_yaw_deg, 200.0);
    assert_eq!(
        state.settings.lighting.azimuth_deg, 120.0,
        "the light stays"
    );
    assert_eq!(lighting_view(&state).values.azimuth_deg, 200.0);

    let settings = render_settings(&state, true);
    assert_eq!(settings.hdr_yaw_deg, 200.0);
    assert_eq!(settings.light_yaw_deg, 120.0);

    // Choosing a rig brings the light's direction back and keeps the map's turn for later.
    set_lighting(&mut state, "Window daylight").expect("an offered rig");
    assert_eq!(lighting_view(&state).values.azimuth_deg, 120.0);
    assert_eq!(state.settings.lighting.hdr_yaw_deg, 200.0);

    turn_to(&mut state, f32::NAN);
    assert_eq!(
        state.settings.lighting.azimuth_deg, 48.0,
        "NaN is the default"
    );
}

#[test]
fn switching_follow_keeps_the_picture_and_the_view_reports_it() {
    let mut state = state();
    import_map(&mut state, 64);
    turn_to(&mut state, 100.0);
    state.viewport.camera.yaw = 0.5;
    let total = |state: &AppState| {
        render_settings(state, true)
            .lighting_spec()
            .hdr_yaw_for(0.5)
    };
    let before = total(&state);

    set_hdr_follows_camera(&mut state, true);
    assert!(state.settings.lighting.hdr_follows_camera);
    assert!(lighting_view(&state).hdr_follows_camera);
    assert!(same_angle(before, total(&state)));

    set_hdr_follows_camera(&mut state, false);
    assert!(!lighting_view(&state).hdr_follows_camera);
    assert!(same_angle(before, total(&state)));
    assert!((state.settings.lighting.hdr_yaw_deg - 100.0).abs() < 1e-2);
}

#[test]
fn a_different_map_starts_unturned() {
    let mut state = state();
    import_map(&mut state, 64);
    turn_to(&mut state, 200.0);
    set_hdr_follows_camera(&mut state, true);
    assert_ne!(state.settings.lighting.hdr_yaw_deg, 0.0);

    import_map(&mut state, 32);
    assert_eq!(state.settings.lighting.hdr_yaw_deg, 0.0);
    assert!(
        state.settings.lighting.hdr_follows_camera,
        "the switch stays as it was"
    );
}
