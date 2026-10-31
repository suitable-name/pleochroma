//! `custom_gem_materials`' default crystal-axis columns and its inclusion columns
//! (`inclusion_families_json`, `structure_json`), plus the per-axis dispersion text, through the
//! migration and the `save_custom_material`/`get_custom_materials` round trip.

use super::{super::*, fixtures::temp_db_path, materials::seed_pre_color_recipe_custom_material};

/// A database from before the crystal-axis columns existed opens unchanged: the three columns
/// are added as `NULL` on every row (no default orientation), and a second open is a no-op.
#[test]
fn crystal_axis_migration_adds_the_columns_and_leaves_existing_rows_null() {
    let path = temp_db_path("crystal_axis_migration");
    seed_pre_color_recipe_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        for column in [
            "c_axis_tilt_deg",
            "c_axis_azimuth_deg",
            "c_axis_second_azimuth_deg",
        ] {
            assert!(
                Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
                "migration must add {column}"
            );
        }
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].c_axis_tilt_deg, None);
        assert_eq!(materials[0].c_axis_azimuth_deg, None);
        assert_eq!(materials[0].c_axis_second_azimuth_deg, None);
    }

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2.get_custom_materials().expect("read back again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].c_axis_tilt_deg, None);

    let _ = std::fs::remove_file(&path);
}

/// The default orientation round trips as stored, a row saved without one reads `None` (stays
/// `NULL`), and a re-save replaces or clears it.
#[test]
fn crystal_axis_columns_round_trip_and_none_stays_null() {
    let db = Database::new(Some(":memory:")).expect("create db");
    let save = |name: &str, axis: Option<(f32, f32, Option<f32>)>| {
        db.save_custom_material(&CustomMaterialParams {
            name,
            refractive_index: 1.768,
            dispersion: 0.018,
            birefringence: -0.008,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: Some("Trigonal"),
            optical_character: Some("UniaxialNegative"),
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
            c_axis_tilt_deg: axis.map(|a| a.0),
            c_axis_azimuth_deg: axis.map(|a| a.1),
            c_axis_second_azimuth_deg: axis.and_then(|a| a.2),
            inclusion_families_json: None,
            structure_json: None,
        })
        .expect("save custom material");
    };
    let stored = |name: &str| {
        db.get_custom_materials()
            .expect("read back")
            .into_iter()
            .find(|m| m.name == name)
            .expect("row present")
    };

    save("Oriented", Some((30.0, 45.5, Some(20.0))));
    save("Plain", None);
    let oriented = stored("Oriented");
    assert_eq!(oriented.c_axis_tilt_deg, Some(30.0));
    assert_eq!(oriented.c_axis_azimuth_deg, Some(45.5));
    assert_eq!(oriented.c_axis_second_azimuth_deg, Some(20.0));
    let plain = stored("Plain");
    assert_eq!(plain.c_axis_tilt_deg, None);
    assert_eq!(plain.c_axis_azimuth_deg, None);
    assert_eq!(plain.c_axis_second_azimuth_deg, None);

    save("Oriented", None);
    assert_eq!(stored("Oriented").c_axis_tilt_deg, None);
}

/// A database from before the inclusion columns existed opens unchanged: both columns are added
/// as `NULL` on every row (no inclusions, no structure), the row still reads, and a second open
/// is a no-op.
#[test]
fn inclusion_migration_adds_the_columns_and_leaves_existing_rows_null() {
    let path = temp_db_path("inclusion_migration");
    seed_pre_color_recipe_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        for column in ["inclusion_families_json", "structure_json"] {
            assert!(
                Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
                "migration must add {column}"
            );
        }
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].inclusion_families_json, None);
        assert_eq!(materials[0].structure_json, None);
    }

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2.get_custom_materials().expect("read back again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].structure_json, None);

    let _ = std::fs::remove_file(&path);
}

/// A fresh database has both columns in its CREATE TABLE.
#[test]
fn a_fresh_database_already_has_the_inclusion_columns() {
    let db = Database::new(Some(":memory:")).expect("create db");
    for column in ["inclusion_families_json", "structure_json"] {
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
            "a fresh database's CREATE TABLE must already include {column}"
        );
    }
}

/// The per-axis dispersion text, the inclusion families and the structure come back byte for byte
/// (the columns are opaque text to this crate), a row saved without them reads `None`, and a
/// re-save replaces or clears each.
#[test]
fn render_physics_columns_round_trip_and_none_stays_null() {
    let db = Database::new(Some(":memory:")).expect("create db");
    let save =
        |name: &str, per_axis: Option<&str>, families: Option<&str>, structure: Option<&str>| {
            db.save_custom_material(&CustomMaterialParams {
                name,
                refractive_index: 1.768,
                dispersion: 0.018,
                birefringence: -0.008,
                absorption_rgb: [0.0, 0.0, 0.0],
                crystal_system: Some("Trigonal"),
                optical_character: Some("UniaxialNegative"),
                biaxial_delta_beta_alpha: None,
                per_axis_dispersion_json: per_axis,
                specific_gravity: None,
                color_recipe_json: None,
                dispersion_model_json: None,
                absorption_bands_json: None,
                c_axis_tilt_deg: None,
                c_axis_azimuth_deg: None,
                c_axis_second_azimuth_deg: None,
                inclusion_families_json: families,
                structure_json: structure,
            })
            .expect("save custom material");
        };
    let stored = |name: &str| {
        db.get_custom_materials()
            .expect("read back")
            .into_iter()
            .find(|m| m.name == name)
            .expect("row present")
    };
    let curve = r#"{"e":{"kind":"cauchy","a":1.76,"b":0.006,"c":0.0}}"#;
    let families = r#"[{"kind":"Needle","dir":"AlongC","sigma":0.1,"p":2,"lobe":0.06,"rho":0.5}]"#;
    let structure = r#"{"pattern":"Harlequin","seed":12345}"#;

    save("Full", Some(curve), Some(families), Some(structure));
    save("Plain", None, None, None);
    let full = stored("Full");
    assert_eq!(full.per_axis_dispersion_json.as_deref(), Some(curve));
    assert_eq!(full.inclusion_families_json.as_deref(), Some(families));
    assert_eq!(full.structure_json.as_deref(), Some(structure));
    let plain = stored("Plain");
    assert_eq!(plain.per_axis_dispersion_json, None);
    assert_eq!(plain.inclusion_families_json, None);
    assert_eq!(plain.structure_json, None);

    // A re-save over the name replaces what it sets and clears what it leaves out.
    save("Full", None, Some("[]"), None);
    let again = stored("Full");
    assert_eq!(again.per_axis_dispersion_json, None);
    assert_eq!(again.inclusion_families_json.as_deref(), Some("[]"));
    assert_eq!(again.structure_json, None);
}
