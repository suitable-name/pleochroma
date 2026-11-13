//! The worker's Stone Shadow: the quality a request is traced at, the map a shadowed scene is
//! lit with, and the refusal of a scene whose map cannot be built; and the Tray Reflection's
//! mirror, its refusal and its GPU routing.

use super::{tests::tiny_scene, *};
use crate::assets::{TrayPin, TrayRef, TrayStop, mirror_for_scene, resolve_tray, shadow_quality};
use pleochroma::{
    optics::raytracer::environment::tray_shadow::{TrayShadowOff, TrayShadowQuality},
    render_setup::TrayMirrorOff,
};
use pleochroma_net::messages::RequestIntent;
use std::sync::Arc;

/// A studio-lit scene with the shadow on and a backdrop to cast it on.
fn shadowed() -> SceneState {
    SceneState {
        width: 24,
        height: 24,
        stone_shadow: true,
        backdrop: 1.0,
        ..tiny_scene()
    }
}

/// The live viewport (`Interactive`) is the only request that takes the Preview map, because its
/// local half does; every finished-picture request (a `Batch` render, final image, batch item,
/// job and live pass, which pass no intent) takes the Final map.
#[test]
fn the_quality_follows_the_request_kind() {
    assert_eq!(
        shadow_quality(Some(RequestIntent::Interactive)),
        TrayShadowQuality::Preview
    );
    assert_eq!(
        shadow_quality(Some(RequestIntent::Batch)),
        TrayShadowQuality::Final
    );
    assert_eq!(shadow_quality(None), TrayShadowQuality::Final);
}

/// A scene without the shadow resolves to no map, whatever the intent; a shadowed one resolves
/// to a map the render holds, and the map changes the picture.
#[test]
fn a_shadowed_scene_is_traced_with_its_map() {
    let never = AtomicBool::new(false);
    let plain = SceneState {
        stone_shadow: false,
        ..shadowed()
    };
    let none = resolve_tray(&plain, Some(RequestIntent::Interactive), &never).unwrap();
    assert!(none.shadow.is_none() && none.mirror.is_none());

    let scene = shadowed();
    let tray = resolve_tray(&scene, Some(RequestIntent::Interactive), &never)
        .expect("a closed stone under a studio light has a shadow");
    assert!(tray.shadow.is_some() && tray.mirror.is_none());
    let gpu = GpuBackend::disabled();
    let with = trace_samples_with_gpu_cancellable(&gpu, &scene, (0, 4), 2, &never, tray.view())
        .expect("not cancelled");
    let without = trace_samples_with_gpu_cancellable(&gpu, &plain, (0, 4), 2, &never, none.view())
        .expect("not cancelled");
    assert_eq!(with.len(), without.len());
    assert!(with.iter().all(|pixel| pixel.is_finite()));
    assert_ne!(with, without, "the shadow changes the picture");
}

/// A pin resolves the map once and hands the same allocation to every later call.
#[test]
fn a_pin_builds_the_map_once() {
    let never = AtomicBool::new(false);
    let scene = shadowed();
    let pin = TrayPin::new(Some(RequestIntent::Interactive));
    let first = pin.resolve(&scene, &never).unwrap().shadow.expect("a map");
    let second = pin.resolve(&scene, &never).unwrap().shadow.expect("a map");
    assert!(Arc::ptr_eq(&first, &second));
}

/// A stone that is not closed (one plane) has no shadow: the request is refused with the
/// sentence of `TrayShadowOff::DegenerateStone`, never traced without the shadow it asked for.
/// A cancelled build is a cancellation, not a refusal.
#[test]
fn a_shadow_that_cannot_be_built_is_refused_with_its_message() {
    let mut open = shadowed();
    open.planes.truncate(1);
    assert_eq!(
        resolve_tray(&open, None, &AtomicBool::new(false)),
        Err(TrayStop::Refused(TrayShadowOff::DegenerateStone.message()))
    );
    assert_eq!(
        resolve_tray(&open, None, &AtomicBool::new(true)),
        Err(TrayStop::Cancelled)
    );
    let pin = TrayPin::new(None);
    assert_eq!(
        pin.resolve(&open, &AtomicBool::new(false)),
        Err(TrayStop::Refused(TrayShadowOff::DegenerateStone.message()))
    );
}

/// A studio-lit scene with the Tray Reflection on (a 50 % floor) and a backdrop to reflect.
fn reflective() -> SceneState {
    SceneState {
        width: 24,
        height: 24,
        tray_reflection: 0.5,
        backdrop: 1.0,
        ..tiny_scene()
    }
}

/// A scene without the reflection has no mirror; one with it has the plane of its stone, and the
/// request resolves that mirror, and no shadow map, once.
#[test]
fn a_reflective_scene_has_a_mirror_and_no_shadow_map() {
    let plain = SceneState {
        tray_reflection: 0.0,
        ..reflective()
    };
    assert!(matches!(mirror_for_scene(&plain), Ok(None)));
    let scene = reflective();
    let Ok(Some(mirror)) = mirror_for_scene(&scene) else {
        panic!("a closed stone under a studio light has a plane for the mirror it asked for");
    };
    assert!((mirror.reflectance() - 0.5).abs() < f32::EPSILON);
    assert!(mirror.plane_y().is_finite());
    let never = AtomicBool::new(false);
    let tray = resolve_tray(&scene, None, &never).unwrap();
    assert!(tray.shadow.is_none());
    assert_eq!(tray.mirror, Some(mirror));
}

/// The mirror changes the picture: the scene traced with `tray_reflection` 0 and 0.5 differs,
/// and both are finite.
#[test]
fn the_tray_reflection_changes_the_picture() {
    let never = AtomicBool::new(false);
    let gpu = GpuBackend::disabled();
    let scene = reflective();
    let plain = SceneState {
        tray_reflection: 0.0,
        ..reflective()
    };
    let tray = resolve_tray(&scene, None, &never).unwrap();
    assert!(tray.mirror.is_some());
    let with = trace_samples_with_gpu_cancellable(&gpu, &scene, (0, 4), 2, &never, tray.view())
        .expect("not cancelled");
    let without =
        trace_samples_with_gpu_cancellable(&gpu, &plain, (0, 4), 2, &never, TrayRef::default())
            .expect("not cancelled");
    assert_eq!(with.len(), without.len());
    assert!(with.iter().all(|pixel| pixel.is_finite()));
    assert_ne!(with, without, "the mirror changes the picture");
}

/// A stone that is not closed has no plane: the request is refused with the sentence of
/// `TrayMirrorOff::DegenerateStone`, never traced with a matte tray; an HDR scene with the
/// sentence of `TrayMirrorOff::NeedsStudioBackdrop`.
#[test]
fn a_mirror_that_cannot_be_placed_is_refused_with_its_message() {
    let mut open = reflective();
    open.planes.truncate(1);
    let refused = Err(TrayStop::Refused(TrayMirrorOff::DegenerateStone.message()));
    assert_eq!(resolve_tray(&open, None, &AtomicBool::new(false)), refused);
    assert_eq!(
        TrayPin::new(None).resolve(&open, &AtomicBool::new(false)),
        refused
    );

    let hdr = SceneState {
        environment: pleochroma_net::scene::SceneEnvironment::Hdr(
            pleochroma_net::scene::HdrEnvironment {
                content_hash: [0; 32],
                width: 8,
                height: 4,
            },
        ),
        ..reflective()
    };
    assert_eq!(
        resolve_tray(&hdr, None, &AtomicBool::new(false)),
        Err(TrayStop::Refused(
            TrayMirrorOff::NeedsStudioBackdrop.message()
        ))
    );
}

/// A scene with the Tray Reflection goes to the GPU only when the backend traces the mirror; the
/// disabled backend leaves it to the CPU, and a scene without the reflection is not affected.
#[test]
fn a_reflective_scene_needs_a_gpu_that_traces_the_mirror() {
    let gpu = GpuBackend::disabled();
    assert!(!gpu.supports_tray_mirror());
    assert!(!scene_uses_gpu(&reflective(), &gpu));
    let plain = SceneState {
        tray_reflection: 0.0,
        ..reflective()
    };
    assert_eq!(
        scene_uses_gpu(&plain, &gpu),
        scene_uses_gpu(&tiny_scene(), &gpu)
    );
}

/// A shadowed scene goes to the GPU only when the backend can sample the map; the disabled
/// backend (and any adapter without that support) leaves it to the CPU, and a scene without the
/// shadow is not affected by it.
#[test]
fn a_shadowed_scene_needs_a_gpu_that_samples_the_map() {
    let gpu = GpuBackend::disabled();
    assert!(!gpu.supports_tray_shadow());
    assert!(!scene_uses_gpu(&shadowed(), &gpu));
    let plain = SceneState {
        stone_shadow: false,
        ..shadowed()
    };
    assert_eq!(
        scene_uses_gpu(&plain, &gpu),
        scene_uses_gpu(&tiny_scene(), &gpu)
    );
}
