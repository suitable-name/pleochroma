//! The Color Pattern side tables: the one-transaction material write, the deletes, the prune,
//! the presets and the read-only behaviour.

use super::{super::*, fixtures::temp_db_path};
use crate::model::zoning::{ColorPatternPresetRow, MaterialPatternRow, MaterialZoningRow};

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

fn plain_material<'a>(name: &'a str, bands: Option<&'a str>) -> CustomMaterialParams<'a> {
    CustomMaterialParams {
        name,
        refractive_index: 1.55,
        dispersion: 0.01,
        birefringence: 0.0,
        absorption_rgb: [0.0; 3],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: None,
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: bands,
        c_axis_tilt_deg: None,
        c_axis_azimuth_deg: None,
        c_axis_second_azimuth_deg: None,
        inclusion_families_json: None,
        structure_json: None,
    }
}

fn pattern_of(name: &str, tag: &str) -> MaterialPatternRow {
    MaterialPatternRow {
        material_name: name.to_string(),
        pattern_json: format!("{{\"pattern\":\"{tag}\"}}"),
        version: 1,
    }
}

fn zoning_of(name: &str) -> MaterialZoningRow {
    MaterialZoningRow {
        material_name: name.to_string(),
        zoned_json: "{\"zones\":1}".to_string(),
        relative_to_stone: true,
        version: 1,
    }
}

fn preset(name: &str, tag: &str) -> ColorPatternPresetRow {
    ColorPatternPresetRow {
        name: name.to_string(),
        pattern_json: format!("{{\"preset\":\"{tag}\"}}"),
        version: 1,
    }
}

#[test]
fn the_transaction_writes_the_material_the_pattern_and_the_zones() {
    let db = in_memory_db();
    db.save_material_pattern_with_zoning(
        &plain_material("Melon", Some("[[1.0,2.0,3.0]]")),
        &pattern_of("Melon", "a"),
        Some(&zoning_of("Melon")),
    )
    .unwrap();
    let materials = db.get_custom_materials().unwrap();
    assert_eq!(materials.len(), 1);
    assert_eq!(
        materials[0].absorption_bands_json.as_deref(),
        Some("[[1.0,2.0,3.0]]")
    );
    assert_eq!(
        db.load_material_pattern("MELON").unwrap(),
        Some(pattern_of("Melon", "a"))
    );
    assert_eq!(
        db.load_material_zoning("Melon").unwrap(),
        Some(zoning_of("Melon"))
    );

    // Replacing keeps one row of each.
    db.save_material_pattern_with_zoning(
        &plain_material("Melon", Some("[[4.0,5.0,6.0]]")),
        &pattern_of("Melon", "b"),
        Some(&zoning_of("Melon")),
    )
    .unwrap();
    assert_eq!(db.get_custom_materials().unwrap().len(), 1);
    assert_eq!(db.all_material_zonings().unwrap().len(), 1);
    assert_eq!(
        db.load_material_pattern("Melon").unwrap(),
        Some(pattern_of("Melon", "b"))
    );
}

#[test]
fn a_none_zoning_deletes_the_existing_zoning_row() {
    let db = in_memory_db();
    db.save_material_pattern_with_zoning(
        &plain_material("Melon", None),
        &pattern_of("Melon", "a"),
        Some(&zoning_of("Melon")),
    )
    .unwrap();
    db.save_material_pattern_with_zoning(
        &plain_material("Melon", None),
        &pattern_of("Melon", "uniform"),
        None,
    )
    .unwrap();
    assert_eq!(db.load_material_zoning("Melon").unwrap(), None);
    assert_eq!(
        db.load_material_pattern("Melon").unwrap(),
        Some(pattern_of("Melon", "uniform"))
    );
}

#[test]
fn a_mismatching_name_is_refused_before_anything_is_written() {
    let db = in_memory_db();
    assert!(
        db.save_material_pattern_with_zoning(
            &plain_material("A", None),
            &pattern_of("B", "x"),
            None
        )
        .is_err()
    );
    assert!(
        db.save_material_pattern_with_zoning(
            &plain_material("A", None),
            &pattern_of("A", "x"),
            Some(&zoning_of("B"))
        )
        .is_err()
    );
    assert_eq!(db.get_custom_materials().unwrap().len(), 0);
}

#[test]
fn a_refused_write_rolls_the_material_back() {
    let db = in_memory_db();
    db.conn
        .execute_batch("DROP TABLE material_pattern;")
        .unwrap();
    assert!(
        db.save_material_pattern_with_zoning(
            &plain_material("Melon", None),
            &pattern_of("Melon", "a"),
            Some(&zoning_of("Melon"))
        )
        .is_err()
    );
    assert_eq!(db.get_custom_materials().unwrap().len(), 0);
    assert_eq!(db.load_material_zoning("Melon").unwrap(), None);
}

#[test]
fn the_pattern_alone_can_be_saved_loaded_and_deleted() {
    let db = in_memory_db();
    assert_eq!(db.load_material_pattern("x").unwrap(), None);
    db.save_material_pattern(&pattern_of("X", "a")).unwrap();
    db.save_material_pattern(&pattern_of("x", "b")).unwrap();
    assert_eq!(
        db.load_material_pattern("X").unwrap(),
        Some(pattern_of("X", "b")),
        "an upsert ignoring ASCII case keeps one row"
    );
    assert_eq!(db.delete_material_pattern("x").unwrap(), 1);
    assert_eq!(db.delete_material_pattern("x").unwrap(), 0);
}

#[test]
fn deleting_removes_the_pattern_and_the_zones_but_not_the_material() {
    let db = in_memory_db();
    db.save_material_pattern_with_zoning(
        &plain_material("Melon", None),
        &pattern_of("Melon", "a"),
        Some(&zoning_of("Melon")),
    )
    .unwrap();
    db.delete_material_pattern_and_zoning("melon").unwrap();
    assert_eq!(db.load_material_pattern("Melon").unwrap(), None);
    assert_eq!(db.load_material_zoning("Melon").unwrap(), None);
    assert_eq!(db.get_custom_materials().unwrap().len(), 1);
    // Nothing stored is a no-op.
    db.delete_material_pattern_and_zoning("Melon").unwrap();
}

#[test]
fn pruning_removes_the_patterns_of_deleted_materials() {
    let db = in_memory_db();
    db.save_material_pattern_with_zoning(
        &plain_material("Kept", None),
        &pattern_of("Kept", "a"),
        None,
    )
    .unwrap();
    db.save_material_pattern(&pattern_of("Deleted", "b"))
        .unwrap();
    let removed = db.prune_zoning_orphans().unwrap();
    assert_eq!(removed, 1);
    assert!(db.load_material_pattern("Kept").unwrap().is_some());
    assert!(db.load_material_pattern("Deleted").unwrap().is_none());
}

#[test]
fn presets_round_trip_sorted_and_rename() {
    let db = in_memory_db();
    assert_eq!(
        db.load_color_pattern_presets().unwrap(),
        [] as [ColorPatternPresetRow; 0]
    );
    db.save_color_pattern_preset(&preset("beta", "1")).unwrap();
    db.save_color_pattern_preset(&preset("Alpha", "2")).unwrap();
    db.save_color_pattern_preset(&preset("beta", "3")).unwrap();
    assert_eq!(
        db.load_color_pattern_presets().unwrap(),
        vec![preset("Alpha", "2"), preset("beta", "3")]
    );

    assert_eq!(db.rename_color_pattern_preset("alpha", "Gamma").unwrap(), 1);
    assert_eq!(db.rename_color_pattern_preset("missing", "Zed").unwrap(), 0);
    // Renaming onto another preset is refused and changes nothing.
    assert!(db.rename_color_pattern_preset("Gamma", "BETA").is_err());
    assert_eq!(
        db.load_color_pattern_presets().unwrap(),
        vec![preset("beta", "3"), preset("Gamma", "2")]
    );

    assert_eq!(db.delete_color_pattern_preset("GAMMA").unwrap(), 1);
    assert_eq!(db.delete_color_pattern_preset("GAMMA").unwrap(), 0);
    assert_eq!(db.load_color_pattern_presets().unwrap().len(), 1);
}

#[test]
fn a_read_only_connection_reads_nothing_and_refuses_writes() {
    let path = temp_db_path("pattern_read_only");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.save_material_pattern(&pattern_of("Ro", "a")).unwrap();
        db.save_color_pattern_preset(&preset("Ro", "a")).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE material_pattern; DROP TABLE color_pattern_presets;
                 DROP TABLE rough_colour; DROP TABLE rough_colour_photo;
                 DROP TABLE stone_pose_choice; DROP TABLE material_zoning;
                 DROP TABLE render_job_zoning;",
            )
            .unwrap();
    }
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(ro.load_material_pattern("Ro").unwrap(), None);
    assert_eq!(
        ro.load_color_pattern_presets().unwrap(),
        [] as [ColorPatternPresetRow; 0]
    );
    assert_eq!(ro.delete_material_pattern("Ro").unwrap(), 0);
    assert_eq!(ro.delete_color_pattern_preset("Ro").unwrap(), 0);
    assert_eq!(ro.rename_color_pattern_preset("Ro", "No").unwrap(), 0);
    ro.delete_material_pattern_and_zoning("Ro").unwrap();
    assert_eq!(ro.prune_zoning_orphans().unwrap(), 0);
    assert!(ro.save_material_pattern(&pattern_of("No", "x")).is_err());
    drop(ro);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
