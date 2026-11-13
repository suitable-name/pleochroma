//! The HDR map rotation in the render settings.

use super::*;
use pleochroma::renderer::env_map::HdrOrientation;

/// A payload without `hdr_yaw_deg` and `hdr_follows_camera` loads the map unturned and fixed in
/// the room.
#[test]
fn a_payload_from_before_the_map_rotation_loads_it_unturned() {
    let old = SessionPayload::from_json(r#"{"render":{"exposure":1.25,"use_hdr":true}}"#)
        .expect("parses");
    assert_eq!(old.render.hdr_yaw_deg.to_bits(), 0.0_f32.to_bits());
    assert!(!old.render.hdr_follows_camera);
    let spec = old.render.lighting_spec();
    assert_eq!(spec.hdr_yaw_deg.to_bits(), 0.0_f32.to_bits());
    assert!(!spec.hdr_follows_camera);
    assert_eq!(spec.hdr_orientation(), HdrOrientation::default());
}

#[test]
fn the_map_rotation_round_trips_through_the_payload_and_into_the_scene() {
    let mut payload = SessionPayload::default();
    payload.render.hdr_yaw_deg = 135.0;
    payload.render.hdr_follows_camera = true;
    let back = SessionPayload::from_json(&payload.to_json().expect("writes")).expect("parses");
    assert_eq!(back.render.hdr_yaw_deg.to_bits(), 135.0_f32.to_bits());
    assert!(back.render.hdr_follows_camera);
    let spec = back.render.lighting_spec();
    assert_eq!(spec.hdr_yaw_deg.to_bits(), 135.0_f32.to_bits());
    assert!(spec.hdr_follows_camera);
    assert_eq!(back, payload);
}

#[test]
fn the_map_rotation_is_wrapped_and_repaired() {
    for (written, expected) in [
        (400.0_f32, 40.0_f32),
        (-90.0, 270.0),
        (f32::NAN, 0.0),
        (f32::INFINITY, 0.0),
    ] {
        let settings = RenderSettings {
            hdr_yaw_deg: written,
            ..RenderSettings::default()
        }
        .sanitized();
        assert!(
            (settings.hdr_yaw_deg - expected).abs() < 1e-3,
            "{written} became {}",
            settings.hdr_yaw_deg
        );
    }
}

/// Switching "Turns with the Camera" keeps the picture: the map's total yaw at the current
/// camera does not move, orbiting afterwards moves it with the camera, and switching back
/// restores the map's own turn.
#[test]
fn switching_follow_keeps_the_picture() {
    let total = |s: &RenderSettings| s.lighting_spec().hdr_yaw_for(s.camera_yaw);
    let same_angle = |a: f32, b: f32| {
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();
        (sa - sb).abs() < 1e-4 && (ca - cb).abs() < 1e-4
    };
    let fixed = RenderSettings {
        hdr_yaw_deg: 100.0,
        camera_yaw: 0.5,
        ..RenderSettings::default()
    };
    let following = fixed.clone().with_hdr_follow(true);
    assert!(following.hdr_follows_camera);
    assert!(same_angle(total(&fixed), total(&following)));

    let orbited = RenderSettings {
        camera_yaw: 0.9,
        ..following.clone()
    };
    assert!(same_angle(total(&orbited), total(&fixed) + 0.4));

    let back = following.with_hdr_follow(false);
    assert!(!back.hdr_follows_camera);
    assert!(same_angle(total(&back), total(&fixed)));
    assert!(
        (back.hdr_yaw_deg - 100.0).abs() < 1e-2,
        "{}",
        back.hdr_yaw_deg
    );

    assert_eq!(fixed.clone().with_hdr_follow(false), fixed);
}
