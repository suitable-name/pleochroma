//! The render-physics columns (per-axis curves, inclusion families, structure) through the
//! editor's save and the row-to-material conversion.

use super::*;
use pleochroma::optics::{
    dispersion::DispersionModel,
    inclusions::{CrystalDir, Family, FamilyKind},
};
use pleochroma_cut_core::material::{
    PerAxisDispersion, inclusion_families_to_json, per_axis_dispersion_to_json,
};

fn fields() -> MaterialSaveFields<'static> {
    MaterialSaveFields {
        ri: 1.77,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [0.0; 3],
        specific_gravity: None,
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
        c_axis_tilt_deg: None,
        c_axis_azimuth_deg: None,
        c_axis_second_azimuth_deg: None,
        per_axis_dispersion_json: None,
        inclusion_families_json: None,
        structure_json: None,
    }
}

fn material() -> GemMaterial {
    GemMaterial::new_custom("Silky", 1.77, 0.018, -0.008, [0.0; 3])
}

fn e_curve() -> DispersionModel {
    DispersionModel::Cauchy {
        a: 1.762,
        b: 0.006,
        c: 0.0,
    }
}

fn stored(db: &Database) -> CustomMaterialRow {
    db.get_custom_materials()
        .expect("read")
        .into_iter()
        .next()
        .expect("one row")
}

/// A save that says nothing about the render-physics columns keeps what the row holds, text
/// replaces it, and blank text clears it.
#[test]
fn a_save_keeps_replaces_or_clears_the_render_physics_columns() {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    let curve = per_axis_dispersion_to_json(&PerAxisDispersion::Uniaxial(e_curve())).unwrap();
    let families = inclusion_families_to_json(&[Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }])
    .unwrap();
    save_gem_material_fields(
        &db,
        &material(),
        &MaterialSaveFields {
            per_axis_dispersion_json: Some(&curve),
            inclusion_families_json: Some(&families),
            structure_json: Some("{}"),
            ..fields()
        },
    )
    .expect("first save");
    let row = stored(&db);
    assert_eq!(
        row.per_axis_dispersion_json.as_deref(),
        Some(curve.as_str())
    );
    assert_eq!(
        row.inclusion_families_json.as_deref(),
        Some(families.as_str())
    );
    assert_eq!(row.structure_json.as_deref(), Some("{}"));

    // The dialog's ordinary save does not author them: the stored values stay.
    save_gem_material_fields(&db, &material(), &fields()).expect("plain re-save");
    let row = stored(&db);
    assert_eq!(
        row.per_axis_dispersion_json.as_deref(),
        Some(curve.as_str())
    );
    assert_eq!(
        row.inclusion_families_json.as_deref(),
        Some(families.as_str())
    );
    assert_eq!(row.structure_json.as_deref(), Some("{}"));

    // Blank text is an explicit clear, one column at a time.
    save_gem_material_fields(
        &db,
        &material(),
        &MaterialSaveFields {
            per_axis_dispersion_json: Some("  "),
            ..fields()
        },
    )
    .expect("clearing re-save");
    let row = stored(&db);
    assert_eq!(row.per_axis_dispersion_json, None);
    assert_eq!(
        row.inclusion_families_json.as_deref(),
        Some(families.as_str())
    );
}

/// A stored row builds the stone with its curve and its families; the same row without them is
/// what `new_custom` infers.
#[test]
fn a_row_with_render_physics_builds_the_curve_and_the_families() {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    let curve = per_axis_dispersion_to_json(&PerAxisDispersion::Uniaxial(e_curve())).unwrap();
    let families = inclusion_families_to_json(&[Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }])
    .unwrap();
    save_gem_material_fields(
        &db,
        &material(),
        &MaterialSaveFields {
            per_axis_dispersion_json: Some(&curve),
            inclusion_families_json: Some(&families),
            ..fields()
        },
    )
    .expect("save");
    let built = gem_material_from_row(&stored(&db));
    assert_eq!(built.uniaxial_extraordinary_dispersion, Some(e_curve()));
    assert!(built.oriented_inclusions().is_some());

    save_gem_material_fields(
        &db,
        &material(),
        &MaterialSaveFields {
            per_axis_dispersion_json: Some(""),
            inclusion_families_json: Some(""),
            ..fields()
        },
    )
    .expect("clear");
    let plain = gem_material_from_row(&stored(&db));
    assert_eq!(plain.uniaxial_extraordinary_dispersion, None);
    assert!(plain.oriented_inclusions().is_none());
    assert_eq!(
        plain,
        GemMaterial::new_custom("Silky", 1.77, 0.018, -0.008, [0.0; 3])
    );
}

/// The biaxial curve seed of the Material Editor (`material_editor/axis_curves.slint`,
/// `seed_index`) reads the dialog's fields as the material stores them: the refractive index is
/// `n_beta`, `delta` is `n_beta - n_alpha` and the birefringence is `n_gamma - n_alpha`. This pins that
/// reading to `GemMaterial::biaxial_indicatrix`, so the seed and the optics cannot drift apart.
#[test]
fn the_biaxial_curve_seed_reads_the_dialog_fields_as_the_material_does() {
    let (ri, delta, biref) = (1.60_f32, 0.010_f32, 0.030_f32);
    let mut material = GemMaterial::new_custom("Biaxial Seed", ri, 0.015, biref, [0.0; 3]);
    material.optical_character = OpticalCharacter::BiaxialPositive;
    material.biaxial_delta_beta_alpha = Some(delta);
    let indicatrix = material.biaxial_indicatrix(589.3).expect("biaxial");
    // The seed's two curves start from these indices.
    let seed_alpha = ri - delta;
    let seed_gamma = ri - delta + biref;
    assert!((indicatrix.n_alpha - seed_alpha).abs() < 1e-3);
    assert!((indicatrix.n_beta - ri).abs() < 1e-3);
    assert!((indicatrix.n_gamma - seed_gamma).abs() < 1e-3);
}
