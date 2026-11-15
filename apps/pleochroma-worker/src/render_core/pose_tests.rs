//! Tests for the worker's application of a scene's stone pose.

use super::*;
use crate::serve::tests::fixtures::tiny_scene;
use glam::Vec3;
use pleochroma::optics::{
    inclusions::spec::InclusionSpec, materials::GemMaterial, raytracer::surface::ScratchBasis,
};

const UPRIGHT: [f32; 3] = [90.0, 0.0, 0.0];

fn silk() -> InclusionSpec {
    InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 0.35,
        turn_deg: None,
        sheen: None,
        color: None,
    }
}

fn silky_sapphire() -> pleochroma_net::SceneState {
    let mut scene = tiny_scene();
    scene.material = GemMaterial::sapphire();
    scene.effects.inclusions = Some(silk());
    scene
}

#[test]
fn an_unposed_scene_is_borrowed_and_unchanged() {
    let scene = tiny_scene();
    assert!(matches!(posed_scene(&scene), Cow::Borrowed(_)));
    let mut applied = scene.clone();
    apply_pose(&mut applied);
    assert_eq!(applied.planes, scene.planes);
    assert_eq!(applied.material, scene.material);
    assert_eq!(applied.effects, scene.effects);
}

#[test]
fn a_posed_scene_has_rotated_planes_and_a_cleared_pose() {
    let mut scene = tiny_scene();
    scene.stone_pose = UPRIGHT;
    let rotation = StonePose::from_array(UPRIGHT).rotation();
    let posed = posed_scene(&scene).into_owned();
    assert_eq!(posed.stone_pose, [0.0, 0.0, 0.0]);
    assert_eq!(posed.planes.len(), scene.planes.len());
    for (before, after) in scene.planes.iter().zip(&posed.planes) {
        let expected = rotation * Vec3::from_array(before.normal);
        assert!(
            (Vec3::from_array(after.normal) - expected).length() < 1e-5,
            "{:?} -> {:?}, expected {expected:?}",
            before.normal,
            after.normal
        );
        assert!((after.d - before.d).abs() < 1e-6);
    }
    // Tilt 90 stands the table upright, facing +Z: the scratch up direction follows the stone.
    let basis = posed.effects.scratch;
    assert!((basis.up - Vec3::Z).length() < 1e-5, "{:?}", basis.up);
    assert!(!basis.is_model());
}

#[test]
fn the_pose_is_applied_once() {
    let mut scene = silky_sapphire();
    scene.stone_pose = [40.0, 100.0, 25.0];
    let mut once = scene;
    apply_pose(&mut once);
    let mut twice = once.clone();
    apply_pose(&mut twice);
    assert_eq!(twice.planes, once.planes);
    assert_eq!(twice.material, once.material);
    assert_eq!(twice.effects, once.effects);
    assert_eq!(twice.stone_pose, [0.0, 0.0, 0.0]);
}

#[test]
fn the_inclusions_are_kept_in_memory_and_not_applied_again() {
    let mut scene = silky_sapphire();
    assert!(scene.traced_material().inclusions.is_some());
    scene.stone_pose = [60.0, 30.0, 0.0];
    apply_pose(&mut scene);
    assert!(scene.effects.inclusions.is_none());
    assert!(scene.effects.inclusion_second_axis.is_none());
    assert!(scene.effects.structure.is_none());
    assert!(scene.effects.material_inclusions.is_none());
    assert!(scene.material.inclusions.is_some());
    // Nothing is left to build an unrotated set: the traced material is the material itself.
    assert!(matches!(scene.traced_material(), Cow::Borrowed(_)));
}

#[test]
fn the_crystal_axis_turns_with_the_stone() {
    let mut scene = tiny_scene();
    scene.material = GemMaterial::sapphire();
    let before = scene.material.c_axis;
    scene.stone_pose = UPRIGHT;
    apply_pose(&mut scene);
    let expected = StonePose::from_array(UPRIGHT).rotation() * before;
    assert!((scene.material.c_axis - expected).length() < 1e-5);
}

#[test]
fn the_girdle_frosting_is_folded_into_explicit_planes_on_the_unrotated_stone() {
    let mut scene = tiny_scene();
    scene.girdle_frosted = true;
    scene.frosted_planes = Vec::new();
    let designed = crate::render_core::resolve_facet_finishes(&scene);
    assert!(designed.contains(&FacetFinish::Frosted));
    scene.stone_pose = UPRIGHT;
    apply_pose(&mut scene);
    assert!(!scene.girdle_frosted);
    assert!(!scene.frosted_planes.is_empty());
    assert_eq!(crate::render_core::resolve_facet_finishes(&scene), designed);
}

#[test]
fn the_posed_scene_ignores_the_pose_in_the_tilt_paths() {
    let mut scene = tiny_scene();
    scene.stone_pose = [30.0, 200.0, 10.0];
    let cleared = without_pose(&scene);
    assert_eq!(cleared.stone_pose, [0.0, 0.0, 0.0]);
    assert_eq!(cleared.planes, scene.planes);
    assert!(matches!(without_pose(&tiny_scene()), Cow::Borrowed(_)));
    clear_pose(&mut scene);
    assert_eq!(scene.stone_pose, [0.0, 0.0, 0.0]);
}

#[test]
fn an_identity_pose_in_a_raw_form_leaves_the_scene_as_designed() {
    let mut scene = tiny_scene();
    scene.stone_pose = [0.0, 360.0, 720.0];
    let designed = tiny_scene();
    apply_pose(&mut scene);
    assert_eq!(scene.planes, designed.planes);
    assert_eq!(scene.stone_pose, [0.0, 0.0, 0.0]);
    assert_eq!(scene.effects.scratch, ScratchBasis::MODEL);
}

#[test]
fn the_shadow_and_mirror_see_the_posed_planes() {
    // The tray plane follows the posed stone's lowest vertex, so a tilted stone's mirror sits
    // elsewhere than the designed stone's.
    let mut scene = tiny_scene();
    scene.backdrop = 1.0;
    scene.tray_reflection = 0.2;
    let designed = crate::assets::mirror_for_scene(&scene).unwrap().unwrap();
    scene.stone_pose = UPRIGHT;
    apply_pose(&mut scene);
    let posed = crate::assets::mirror_for_scene(&scene).unwrap().unwrap();
    assert_ne!(posed, designed);
}
