//! A custom material's own silk and play-of-color reach the Worker's traced material
//! (`MaterialSpec::custom_material_inclusions`, protocol v29).
//!
//! `GemMaterial::inclusions` is `serde(skip)`, so a postcard round trip of the built custom
//! material loses them; these tests pin that loss and the record that restores them. Kept beside
//! `tests.rs` (not in it) to keep both files under the line cap.

use super::*;
use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::inclusions::{
        CrystalDir, CrystalFrame, Family, FamilyKind, MAX_FAMILIES,
        bragg::{StructureKind, StructureSpec},
    },
};

fn needle() -> Family {
    Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }
}

/// A custom stone with silk along its c axis (stored c axis `Y`).
fn needle_custom(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.56, 0.012, 0.008, [0.0; 3])
        .with_inclusions(CrystalFrame::new(Vec3::Y, None).resolve(&[needle()]))
}

/// A custom labradorite: the play-of-color structure alone.
fn labradorite_custom(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.56, 0.012, 0.008, [0.0; 3]).with_structure_spec_as(
        StructureKind::Labradorite,
        &StructureSpec::default_for("Labradorite").expect("labradorite has a default"),
    )
}

/// `customs` as the page sends them, tracing `name`.
fn spec_of(name: &str, customs: Vec<GemMaterial>) -> MaterialSpec {
    MaterialSpec {
        custom_material_inclusions: MaterialInclusionsSpec::list_of(&customs),
        custom_materials: customs,
        ..MaterialSpec::catalogue(name)
    }
}

fn over_the_wire(spec: &MaterialSpec) -> MaterialSpec {
    let bytes = postcard::to_allocvec(spec).expect("encodes");
    postcard::from_bytes(&bytes).expect("decodes")
}

#[test]
fn a_custom_materials_own_families_reach_the_worker_material() {
    let custom = needle_custom("My Needles");
    let sent = spec_of("My Needles", vec![custom.clone()]);
    assert_eq!(sent.custom_material_inclusions.len(), 1);

    let received = over_the_wire(&sent);
    assert!(
        received.custom_materials[0].oriented_inclusions().is_none(),
        "the serde(skip) field is what the wire drops"
    );
    let planes = StandardGemCuts::standard_round_brilliant();
    let traced = resolve_scene_material(&received, &planes).expect("resolves");
    let arrived = traced.oriented_inclusions().expect("the silk arrived");
    let sent_dir = custom.oriented_inclusions().expect("silk").families()[0].dir;
    assert!((arrived.families()[0].dir - sent_dir).length() < 1e-5);
    assert!(
        (sent_dir - Vec3::Y).length() < 1e-5,
        "along the stored c axis"
    );

    // Without the new field the Worker traces a plain stone (the defect).
    let without = MaterialSpec {
        custom_material_inclusions: Vec::new(),
        ..received
    };
    let plain = resolve_scene_material(&without, &planes).expect("resolves");
    assert!(plain.oriented_inclusions().is_none());
}

#[test]
fn a_custom_labradorite_structure_reaches_the_worker_material() {
    let custom = labradorite_custom("My Labradorite");
    let sent_structure = *custom
        .oriented_inclusions()
        .and_then(|set| set.structure())
        .expect("the structure");
    let received = over_the_wire(&spec_of("My Labradorite", vec![custom]));
    let planes = StandardGemCuts::standard_round_brilliant();

    let traced = resolve_scene_material(&received, &planes).expect("resolves");
    let structure = *traced
        .oriented_inclusions()
        .and_then(|set| set.structure())
        .expect("the structure arrived");
    assert_eq!(structure, sent_structure);

    let without = MaterialSpec {
        custom_material_inclusions: Vec::new(),
        ..received
    };
    let plain = resolve_scene_material(&without, &planes).expect("resolves");
    assert!(plain.oriented_inclusions().is_none());
}

#[test]
fn the_inclusions_follow_the_traced_customs_index() {
    let plain = GemMaterial::new_custom("Plain", 1.6, 0.01, 0.0, [0.0; 3]);
    let customs = vec![plain, needle_custom("Silky")];
    let planes = StandardGemCuts::standard_round_brilliant();
    let received = over_the_wire(&spec_of("silky", customs));
    assert_eq!(received.custom_material_inclusions.len(), 2);
    assert!(received.custom_material_inclusions[0].is_none());

    let silky = resolve_scene_material(&received, &planes).expect("resolves");
    assert!(silky.oriented_inclusions().is_some(), "matched by name");
    let other = MaterialSpec {
        name: "Plain".to_owned(),
        ..received
    };
    let plain = resolve_scene_material(&other, &planes).expect("resolves");
    assert!(plain.oriented_inclusions().is_none());
}

#[test]
fn an_empty_inclusion_list_changes_nothing() {
    let customs = vec![GemMaterial::new_custom(
        "Garnet 1.74",
        1.74,
        0.024,
        0.0,
        [0.1, 0.0, 0.2],
    )];
    let spec = spec_of("Garnet 1.74", customs);
    assert!(
        spec.custom_material_inclusions.is_empty(),
        "no material carries inclusions, so nothing is listed"
    );
    let bytes = postcard::to_allocvec(&spec).expect("encodes");
    assert_eq!(bytes.last(), Some(&0), "the empty list is one length byte");
    let back: MaterialSpec = postcard::from_bytes(&bytes).expect("decodes");
    assert_eq!(back, spec);

    let planes = StandardGemCuts::standard_round_brilliant();
    let traced = resolve_scene_material(&back, &planes).expect("resolves");
    assert!(traced.oriented_inclusions().is_none());
    // The same as a list of explicit "none" entries (the pre-v29 behaviour for this stone).
    let explicit = MaterialSpec {
        custom_material_inclusions: vec![None],
        ..back
    };
    assert_eq!(
        resolve_scene_material(&explicit, &planes).expect("resolves"),
        traced
    );
}

#[test]
fn a_mismatched_list_length_is_refused() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let mut spec = spec_of("My Needles", vec![needle_custom("My Needles")]);
    spec.custom_material_inclusions.push(None);
    assert!(spec.validate().is_err());
    assert!(matches!(
        resolve_scene_material(&spec, &planes),
        Err(SceneError::InvalidMaterialInclusions(_))
    ));

    // Entries without any custom material to follow are refused too.
    let orphan = MaterialSpec {
        custom_material_inclusions: vec![None],
        ..MaterialSpec::catalogue("Sapphire")
    };
    assert!(orphan.validate().is_err());
}

/// The rules of `pleochroma_net::scene::material_inclusions::MaterialInclusions::validate`, by
/// example (web-core cannot depend on the net crate): keep the two in lock-step.
#[test]
fn the_record_refuses_what_the_net_twin_refuses_and_takes_what_it_takes() {
    let valid = MaterialInclusionsSpec::of_material(&needle_custom("A")).expect("silk");
    assert!(valid.validate().is_ok());
    let structured = MaterialInclusionsSpec::of_material(&labradorite_custom("B")).expect("set");
    assert!(structured.validate().is_ok());

    let too_many = MaterialInclusionsSpec {
        families: vec![valid.families[0]; MAX_FAMILIES + 1],
        structure: None,
    };
    assert!(too_many.validate().is_err());
    let at_most = MaterialInclusionsSpec {
        families: vec![valid.families[0]; MAX_FAMILIES],
        structure: None,
    };
    assert!(at_most.validate().is_ok());

    let mut negative = valid.clone();
    negative.families[0].sigma = -0.1;
    assert!(negative.validate().is_err());
    let mut not_finite = valid.clone();
    not_finite.families[0].lobe = f32::NAN;
    assert!(not_finite.validate().is_err());
    let mut bad_dir = valid;
    bad_dir.families[0].dir = Vec3::new(f32::INFINITY, 0.0, 0.0);
    assert!(bad_dir.validate().is_err());

    let tweak = |edit: fn(&mut pleochroma::optics::inclusions::bragg::BraggStructure)| {
        let mut record = structured.clone();
        edit(record.structure.as_mut().expect("structure"));
        record.validate()
    };
    assert!(tweak(|s| s.purity = 0.9).is_err());
    assert!(tweak(|s| s.cell_size = f32::NAN).is_err());
    assert!(tweak(|s| s.lambda_low_nm = 100.0).is_err());
    assert!(tweak(|s| s.coverage = 1.5).is_err());
    assert!(tweak(|s| s.peak_reflectance = 0.0).is_err());
    assert!(tweak(|s| s.mosaic = 0.9).is_err());
    assert!(tweak(|s| s.normal = Vec3::new(0.0, 2.0, 0.0)).is_err());

    // A material with no inclusions has no record.
    assert_eq!(
        MaterialInclusionsSpec::of_material(&GemMaterial::sapphire()),
        None
    );
}
