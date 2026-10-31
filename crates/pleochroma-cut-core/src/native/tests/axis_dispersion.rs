//! A custom material's per-axis curves, inclusion families and structure through the design
//! file: written as `[material.custom.dispersion_model_e]` (or `_alpha` / `_gamma`) and the plain
//! `inclusions` / `structure` string keys, read back, and a snapshot without them writes none of
//! the keys (the bytes of every earlier file are unchanged).

use super::fixtures::simple_design;
use crate::material::{
    CustomStructure, custom_structure_to_json, inclusion_families_to_json, structure_spec_to_json,
};
use pleochroma::optics::{
    dispersion::DispersionModel,
    inclusions::{
        CrystalDir, Family, FamilyKind,
        bragg::{StructureKind, StructureSpec},
    },
    materials::GemMaterial,
};

use crate::native::{
    CustomMaterialSnapshot, DesignExtras, design_from_str, design_to_string, dispersion_model_dto,
    gem_material_from_custom_snapshot,
};

const NAME: &str = "Axis Sample";

const E_CURVE: DispersionModel = DispersionModel::Cauchy {
    a: 1.765,
    b: 0.0095,
    c: 0.0,
};

// The biaxial snapshot below (nd 1.75, dispersion 0.015) builds the base curve a = 1.727392,
// b = 0.007852 (1.7818 at 380 nm, 1.7403 at 780 nm). `ALPHA_CURVE` runs 0.0124 below it and
// `GAMMA_CURVE` 0.0126 above it at every wavelength (b differs by under 5e-5), so the pair keeps
// alpha < beta < gamma across 380-780 nm and is applied.
const ALPHA_CURVE: DispersionModel = DispersionModel::Cauchy {
    a: 1.715,
    b: 0.0079,
    c: 0.0,
};

const GAMMA_CURVE: DispersionModel = DispersionModel::Cauchy {
    a: 1.74,
    b: 0.0079,
    c: 0.0,
};

fn plain() -> CustomMaterialSnapshot {
    CustomMaterialSnapshot::new(1.76, 0.02, -0.008, None, "Trigonal", "UniaxialNegative")
}

fn round_trip(snapshot: &CustomMaterialSnapshot) -> (String, CustomMaterialSnapshot) {
    let mut design = simple_design();
    design.material.name = Some(NAME.to_string());
    let extras = DesignExtras {
        custom_material: Some(snapshot),
        ..DesignExtras::default()
    };
    let text = design_to_string(&design, None, &extras).expect("must save");
    let loaded = design_from_str(&text).expect("must load back");
    let restored = loaded
        .restorable_custom_material
        .expect("the snapshot is restorable");
    (text, restored)
}

#[test]
fn a_snapshot_without_the_new_keys_writes_none_of_them() {
    let (text, restored) = round_trip(&plain());
    assert!(!text.contains("dispersion_model_e"), "{text}");
    assert!(!text.contains("dispersion_model_alpha"), "{text}");
    assert!(!text.contains("dispersion_model_gamma"), "{text}");
    assert!(!text.contains("inclusions"), "{text}");
    assert!(!text.contains("structure"), "{text}");
    assert_eq!(restored, plain());
}

#[test]
fn the_e_ray_curve_survives_a_save_and_open_and_rebuilds_the_material() {
    let snapshot = plain().with_axis_dispersion(Some(dispersion_model_dto(&E_CURVE)), None, None);
    let (text, restored) = round_trip(&snapshot);
    assert!(
        text.contains("[material.custom.dispersion_model_e]"),
        "{text}"
    );
    assert_eq!(restored, snapshot);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    assert_eq!(rebuilt.uniaxial_extraordinary_dispersion, Some(E_CURVE));
    // Without the table the material is the one the plain numbers build.
    let without = gem_material_from_custom_snapshot(NAME, &plain());
    assert_eq!(without.uniaxial_extraordinary_dispersion, None);
    assert_eq!(
        without,
        GemMaterial::new_custom(NAME, 1.76, 0.02, -0.008, [0.0; 3])
    );
}

#[test]
fn the_biaxial_pair_and_the_family_texts_survive_a_save_and_open() {
    let snapshot =
        CustomMaterialSnapshot::new(1.75, 0.015, 0.009, None, "Orthorhombic", "BiaxialPositive")
            .with_biaxial_delta(Some(0.002))
            .with_axis_dispersion(
                None,
                Some(dispersion_model_dto(&ALPHA_CURVE)),
                Some(dispersion_model_dto(&GAMMA_CURVE)),
            )
            .with_inclusion_families(
                Some(r#"[{"kind":"needle"}]"#.to_owned()),
                Some(r#"{"pattern":"Harlequin"}"#.to_owned()),
            );
    let (text, restored) = round_trip(&snapshot);
    assert!(
        text.contains("[material.custom.dispersion_model_alpha]"),
        "{text}"
    );
    assert!(
        text.contains("[material.custom.dispersion_model_gamma]"),
        "{text}"
    );
    assert!(text.contains("inclusions = "), "{text}");
    assert!(text.contains("structure = "), "{text}");
    assert_eq!(restored, snapshot);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    assert_eq!(
        rebuilt.biaxial_principal_dispersion,
        Some([ALPHA_CURVE, GAMMA_CURVE])
    );
    assert_eq!(rebuilt.uniaxial_extraordinary_dispersion, None);
}

#[test]
fn an_opened_design_without_its_library_row_gets_the_silk_and_the_named_structure() {
    let families = inclusion_families_to_json(&[Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }]);
    let spec = StructureSpec::default_for("Labradorite").expect("has a default");
    let unnamed = structure_spec_to_json(&spec);
    let named = custom_structure_to_json(&CustomStructure {
        kind: Some(StructureKind::Labradorite),
        spec,
    });

    let silk = plain().with_inclusion_families(families, None);
    let mat = gem_material_from_custom_snapshot(NAME, &silk);
    assert!(mat.scatters());
    assert!(mat.oriented_inclusions().is_some());

    // A structure text without a kind is carried but not applied; one that names it is.
    let without_kind = plain().with_inclusion_families(None, unnamed);
    assert_eq!(
        gem_material_from_custom_snapshot(NAME, &without_kind),
        gem_material_from_custom_snapshot(NAME, &plain())
    );
    let with_kind = plain().with_inclusion_families(None, named);
    assert_ne!(
        gem_material_from_custom_snapshot(NAME, &with_kind),
        gem_material_from_custom_snapshot(NAME, &plain())
    );
}

#[test]
fn blank_family_texts_are_not_written() {
    let snapshot = plain().with_inclusion_families(Some("  ".to_owned()), Some(String::new()));
    assert_eq!(snapshot.inclusions, None);
    assert_eq!(snapshot.structure, None);
}
