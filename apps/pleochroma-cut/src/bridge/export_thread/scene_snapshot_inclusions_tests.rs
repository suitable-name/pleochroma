//! A custom material's own inclusions (its stored silk and play-of-color) are not part of the
//! material's serde bytes. The capture carries them beside the material in the effects record, so
//! a remote worker (and a render job) traces the same scatterers the desktop does.

use super::{remote::scene_state_from_snapshot, scene_snapshot::SceneSnapshot};
use crate::bridge::{remote::inclusions_refusal, render_thread::RenderContext};
use glam::Vec3;
use pleochroma::{
    optics::{
        inclusions::{
            CrystalDir, CrystalFrame, Family, FamilyKind,
            bragg::{StructureKind, StructureSpec},
        },
        materials::GemMaterial,
    },
    renderer::engine::RenderEngine,
};
use pleochroma_net::SceneState;
use std::sync::Mutex;

/// A custom material with silk along its c axis and a labradorite-like structure.
fn silky_custom() -> GemMaterial {
    let families = [Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }];
    GemMaterial::new_custom("My Schiller", 1.56, 0.012, 0.008, [0.0; 3])
        .with_inclusions(CrystalFrame::new(Vec3::Y, Some(Vec3::X)).resolve(&families))
        .with_structure_spec_as(
            StructureKind::Labradorite,
            &StructureSpec::default_for("Labradorite").expect("labradorite has a default"),
        )
}

fn capture(ctx: RenderContext) -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(ctx)).expect("the custom material resolves")
}

/// The scatterers of two materials agree: same families (directions to a float's rounding, the
/// worker normalises them once more) and the same structure.
fn assert_same_scatterers(rebuilt: &GemMaterial, local: &GemMaterial) {
    let rebuilt = rebuilt.inclusions.expect("the worker rebuilt a set");
    let local = local.inclusions.expect("the desktop carries a set");
    assert_eq!(rebuilt.families().len(), local.families().len());
    for (a, b) in rebuilt.families().iter().zip(local.families()) {
        assert_eq!(
            (a.kind, a.sigma, a.p, a.lobe),
            (b.kind, b.sigma, b.p, b.lobe)
        );
        assert!(
            (a.dir - b.dir).length() < 1e-6,
            "{:?} vs {:?}",
            a.dir,
            b.dir
        );
    }
    assert_eq!(rebuilt.structure(), local.structure());
}

#[test]
fn a_custom_materials_own_inclusions_reach_the_worker_and_scatter_like_the_local_set() {
    let custom = silky_custom();
    let snapshot = capture(RenderContext {
        material_override: Some(custom.clone()),
        ..RenderContext::default()
    });
    assert!(snapshot.material.inclusions.is_some());
    assert!(
        snapshot.effects.material_inclusions.is_some(),
        "the capture carries the set beside the material"
    );
    assert_eq!(
        inclusions_refusal(false, &snapshot.material, &snapshot.effects),
        Ok(())
    );
    // The local tracers keep the set the material already has.
    assert_same_scatterers(&snapshot.traced_material(), &custom);

    let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
    let bytes = postcard::to_allocvec(&state).expect("the scene encodes");
    let received: SceneState = postcard::from_bytes(&bytes).expect("the scene decodes");
    assert!(
        received.material.inclusions.is_none(),
        "the material's own bytes leave the set out"
    );
    assert_same_scatterers(&received.traced_material(), &custom);
    assert!(received.traced_material().scatters());
}

#[test]
fn a_material_without_inclusions_and_a_ray_traced_scene_carry_none() {
    let plain = capture(RenderContext::default());
    assert!(plain.effects.material_inclusions.is_none());
    assert!(plain.effects.is_none());

    let mut ray = RenderContext {
        material_override: Some(silky_custom()),
        ..RenderContext::default()
    };
    ray.engine.engine = RenderEngine::RayTraced;
    let snapshot = capture(ray);
    assert!(
        snapshot.effects.is_none(),
        "the image engine ignores the physics"
    );
    assert_eq!(
        inclusions_refusal(true, &snapshot.material, &snapshot.effects),
        Ok(())
    );
}

#[test]
fn a_set_the_record_could_not_carry_keeps_the_render_local() {
    let mut snapshot = capture(RenderContext {
        material_override: Some(silky_custom()),
        ..RenderContext::default()
    });
    snapshot.effects.material_inclusions = None;
    assert!(inclusions_refusal(false, &snapshot.material, &snapshot.effects).is_err());
}
