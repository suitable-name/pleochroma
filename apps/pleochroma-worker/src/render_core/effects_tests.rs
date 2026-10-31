//! The CPU worker path honours the scene's render physics record, and an untouched record
//! changes nothing.

use super::{tests::tiny_scene, *};
use pleochroma::optics::{
    inclusions::{
        CrystalDir, CrystalFrame, Family, FamilyKind,
        bragg::{StructureKind, StructureSpec},
        spec::InclusionSpec,
    },
    raytracer::surface::{FacetSurface, PolishPreset},
};
use pleochroma_net::scene::material_inclusions::MaterialInclusions;

fn silk() -> InclusionSpec {
    InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 1.0,
        turn_deg: None,
        sheen: None,
        color: None,
    }
}

#[test]
fn ideal_facet_words_trace_bit_identically_to_no_record() {
    let plain = tiny_scene();
    let mut ideal_words = tiny_scene();
    ideal_words.effects.facet_surfaces = vec![0; ideal_words.planes.len()];
    assert!(ideal_words.effects.has_surface());
    assert_eq!(
        trace_samples(&plain, 0, 4, 1),
        trace_samples(&ideal_words, 0, 4, 1)
    );
}

#[test]
fn a_scene_with_a_surface_finish_routes_to_the_gpu_like_a_polished_one() {
    let mut rough = tiny_scene();
    rough.effects.facet_surfaces =
        vec![FacetSurface::from_preset(PolishPreset::Prepolish, 0.3).packed(); 3];
    assert!(rough.effects.has_surface());
    assert_eq!(scene_uses_gpu(&rough), scene_uses_gpu(&tiny_scene()));
}

#[test]
fn ground_facets_change_the_image() {
    let plain = tiny_scene();
    let mut ground = tiny_scene();
    ground.effects.facet_surfaces =
        vec![FacetSurface::ground(0.45, 1.0).packed(); ground.planes.len()];
    assert_ne!(
        trace_samples(&plain, 0, 4, 1),
        trace_samples(&ground, 0, 4, 1)
    );
}

#[test]
fn the_design_silk_reaches_the_traced_material() {
    let mut plain = tiny_scene();
    plain.material = GemMaterial::sapphire();
    let mut silky = plain.clone();
    silky.effects.inclusions = Some(silk());
    assert!(matches!(
        plain.traced_material(),
        std::borrow::Cow::Borrowed(_)
    ));
    assert!(silky.traced_material().inclusions.is_some());
    assert_ne!(
        trace_samples(&plain, 0, 8, 1),
        trace_samples(&silky, 0, 8, 1)
    );
}

/// A custom stone with strong silk along its c axis and a labradorite-like structure.
fn silky_custom() -> GemMaterial {
    let families = [Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 6.0,
        p: 0,
        lobe: 0.05,
        rho: 0.0,
    }];
    GemMaterial::new_custom("My Schiller", 1.56, 0.012, 0.008, [0.0; 3])
        .with_inclusions(CrystalFrame::new(glam::Vec3::Y, None).resolve(&families))
        .with_structure_spec_as(
            StructureKind::Labradorite,
            &StructureSpec::default_for("Labradorite").expect("labradorite has a default"),
        )
}

/// A custom material's own silk reaches the worker in the effects record (the material's serde
/// bytes skip it), and the rebuilt material scatters like the sender's: same families and the
/// same structure, and the same picture to rounding.
#[test]
fn a_custom_materials_own_silk_reaches_the_traced_material_and_the_picture() {
    let custom = silky_custom();
    let mut local = tiny_scene();
    local.material = custom.clone();

    // What the worker receives: the material without its set, the record with it.
    let mut stripped = tiny_scene();
    stripped.material = custom.clone();
    stripped.material.inclusions = None;
    let mut received = stripped.clone();
    received.effects.material_inclusions = MaterialInclusions::of_material(&custom);
    assert!(received.effects.material_inclusions.is_some());

    let (rebuilt, original) = (
        received.traced_material().inclusions.expect("rebuilt"),
        custom.inclusions.expect("carried"),
    );
    assert_eq!(rebuilt.families().len(), original.families().len());
    for (a, b) in rebuilt.families().iter().zip(original.families()) {
        assert_eq!(
            (a.kind, a.sigma, a.p, a.lobe),
            (b.kind, b.sigma, b.p, b.lobe)
        );
        assert!((a.dir - b.dir).length() < 1e-6);
    }
    assert_eq!(rebuilt.structure(), original.structure());

    let sum = |buffer: &[glam::Vec3]| buffer.iter().map(|v| v.abs().element_sum()).sum::<f32>();
    let reference = trace_samples(&local, 0, 8, 1);
    let carried = trace_samples(&received, 0, 8, 1);
    let lost = trace_samples(&stripped, 0, 8, 1);
    let difference: f32 = reference
        .iter()
        .zip(&carried)
        .map(|(a, b)| (*a - *b).abs().element_sum())
        .sum();
    assert!(
        difference <= 1e-3 * sum(&reference),
        "the carried set must trace like the local one ({difference} vs {})",
        sum(&reference)
    );
    assert_ne!(lost, reference, "without the record the silk is missing");
}
