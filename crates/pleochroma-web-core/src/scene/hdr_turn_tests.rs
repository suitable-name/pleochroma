//! The HDR map's turn in a built scene and in the lighting spec.

use super::*;
use pleochroma::geometry::cuts::StandardGemCuts;

fn lighting() -> LightingSpec {
    LightingSpec::new(
        LightingPreset::LightTent,
        1.0,
        0.4,
        0.35,
        Backdrop::Grey,
        16.0,
    )
}

fn scene_spec(lighting: LightingSpec, camera_yaw: f32, hdr_id: Option<u64>) -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: camera_yaw,
            pitch: 0.35,
            distance: 4.2,
        },
        lighting,
        max_bounces: 4,
        width: 4,
        height: 4,
        hdr_id,
    }
}

/// The rotation the scene's HDR environment is built with.
fn rotation_bits(lighting: LightingSpec, camera_yaw: f32) -> [u32; 2] {
    let map = Arc::new(EnvironmentMap::uniform(8, 4, [0.5, 0.4, 0.3]));
    let scene = OwnedScene::build(&scene_spec(lighting, camera_yaw, Some(7)), Some(map))
        .expect("builds with its map");
    match scene.frame_scene().environment {
        EnvironmentSource::HdrMap { rotation, .. } => rotation.bits(),
        EnvironmentSource::Studio { .. } => panic!("expected an HDR environment"),
    }
}

#[test]
fn an_unturned_hdr_scene_has_the_identity_rotation() {
    assert_eq!(
        rotation_bits(lighting(), 0.6),
        EnvRotation::IDENTITY.bits(),
        "the default spec is the map as loaded"
    );
}

#[test]
fn a_turned_hdr_scene_carries_the_map_yaw_whatever_the_camera_does() {
    let turned = lighting().with_hdr_turn(90.0, false);
    let expected = EnvRotation::from_yaw(90.0_f32.to_radians()).bits();
    assert_ne!(expected, EnvRotation::IDENTITY.bits());
    assert_eq!(rotation_bits(turned, 0.6), expected);
    assert_eq!(rotation_bits(turned, 1.4), expected);
}

#[test]
fn a_map_that_follows_the_camera_adds_the_camera_yaw() {
    let following = lighting().with_hdr_turn(30.0, true);
    let expected = |camera: f32| EnvRotation::from_yaw(30.0_f32.to_radians() + camera).bits();
    assert_eq!(rotation_bits(following, 0.6), expected(0.6));
    assert_eq!(rotation_bits(following, 1.4), expected(1.4));
    assert_ne!(rotation_bits(following, 0.6), rotation_bits(following, 1.4));
}

#[test]
fn a_studio_scene_ignores_the_turn() {
    let turned = lighting().with_hdr_turn(90.0, true);
    let scene = OwnedScene::build(&scene_spec(turned, 0.6, None), None).expect("builds");
    assert!(matches!(
        scene.frame_scene().environment,
        EnvironmentSource::Studio { .. }
    ));
}

#[test]
fn a_bad_map_yaw_reads_as_no_turn_and_a_large_one_wraps() {
    let nan = lighting().with_hdr_turn(f32::NAN, false);
    assert_eq!(nan.hdr_orientation(), HdrOrientation::default());
    assert!(nan.hdr_rotation_for(0.6).is_identity());
    let wrapped = lighting().with_hdr_turn(450.0, false).hdr_yaw_for(0.0);
    assert!((wrapped - 90.0_f32.to_radians()).abs() < 1e-5, "{wrapped}");
}
