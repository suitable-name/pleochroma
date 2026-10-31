//! The color variant table: creation, round trips, label rename, material move, deletes, the
//! orphan prune and the read-only behaviour.

use super::{super::*, fixtures::temp_db_path};
use crate::model::zoning::MaterialColorVariantRow;

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

fn variant(key: &str, material: &str, label: &str, tag: &str) -> MaterialColorVariantRow {
    MaterialColorVariantRow {
        key: key.to_string(),
        material_name: material.to_string(),
        label: label.to_string(),
        version: 1,
        variant_json: format!("{{\"tag\":\"{tag}\"}}"),
        created_utc: String::new(),
        updated_utc: String::new(),
    }
}

fn custom_material(name: &str) -> CustomMaterialParams<'_> {
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
        absorption_bands_json: None,
        c_axis_tilt_deg: None,
        c_axis_azimuth_deg: None,
        c_axis_second_azimuth_deg: None,
        inclusion_families_json: None,
        structure_json: None,
    }
}

#[test]
fn a_variant_can_be_created_read_updated_and_deleted() {
    let db = in_memory_db();
    assert_eq!(db.load_color_variant("user.a").unwrap(), None);
    assert!(
        db.load_color_variants_of_material("Sapphire")
            .unwrap()
            .is_empty()
    );

    db.save_color_variant(&variant("user.a", "Sapphire", "Teal", "one"))
        .unwrap();
    let stored = db.load_color_variant("user.a").unwrap().unwrap();
    assert_eq!(stored.key, "user.a");
    assert_eq!(stored.material_name, "Sapphire");
    assert_eq!(stored.label, "Teal");
    assert_eq!(stored.version, 1);
    assert_eq!(stored.variant_json, "{\"tag\":\"one\"}");
    assert_eq!(stored.created_utc.len(), "2026-10-27T12:00:00Z".len());
    assert!(stored.created_utc.ends_with('Z'));
    assert_eq!(stored.created_utc, stored.updated_utc);

    // Updating keeps one row, keeps the creation time and replaces the content.
    db.save_color_variant(&variant("user.a", "sapphire", "Deep Teal", "two"))
        .unwrap();
    let all = db.load_color_variants_of_material("SAPPHIRE").unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].label, "Deep Teal");
    assert_eq!(all[0].variant_json, "{\"tag\":\"two\"}");
    assert_eq!(all[0].created_utc, stored.created_utc);

    assert_eq!(db.delete_color_variant("user.a").unwrap(), 1);
    assert_eq!(db.delete_color_variant("user.a").unwrap(), 0);
    assert_eq!(db.load_color_variant("user.a").unwrap(), None);
}

#[test]
fn the_listing_is_per_material_and_sorted_by_label() {
    let db = in_memory_db();
    db.save_color_variant(&variant("user.1", "Sapphire", "violet", "a"))
        .unwrap();
    db.save_color_variant(&variant("user.2", "Sapphire", "Azure", "b"))
        .unwrap();
    db.save_color_variant(&variant("user.3", "Ruby", "Pigeon", "c"))
        .unwrap();
    let labels: Vec<String> = db
        .load_color_variants_of_material("sapphire")
        .unwrap()
        .into_iter()
        .map(|row| row.label)
        .collect();
    assert_eq!(labels, ["Azure", "violet"]);
    assert_eq!(db.load_color_variants_of_material("Ruby").unwrap().len(), 1);
}

#[test]
fn the_label_can_be_renamed_by_key() {
    let db = in_memory_db();
    db.save_color_variant(&variant("user.a", "Sapphire", "Teal", "one"))
        .unwrap();
    assert_eq!(
        db.rename_color_variant_label("user.a", "Lagoon").unwrap(),
        1
    );
    assert_eq!(db.rename_color_variant_label("user.zz", "Nope").unwrap(), 0);
    let stored = db.load_color_variant("user.a").unwrap().unwrap();
    assert_eq!(stored.label, "Lagoon");
    assert_eq!(stored.variant_json, "{\"tag\":\"one\"}");
}

#[test]
fn renaming_a_material_moves_its_variants() {
    let db = in_memory_db();
    db.save_color_variant(&variant("user.1", "Old Name", "A", "a"))
        .unwrap();
    db.save_color_variant(&variant("user.2", "old name", "B", "b"))
        .unwrap();
    db.save_color_variant(&variant("user.3", "Other", "C", "c"))
        .unwrap();
    assert_eq!(
        db.rename_color_variant_material("OLD NAME", "New Name")
            .unwrap(),
        2
    );
    assert!(
        db.load_color_variants_of_material("Old Name")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.load_color_variants_of_material("New Name")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        db.load_color_variants_of_material("Other").unwrap().len(),
        1
    );
    assert_eq!(
        db.rename_color_variant_material("Missing", "Nowhere")
            .unwrap(),
        0
    );
}

#[test]
fn deleting_a_material_removes_its_variants_only() {
    let db = in_memory_db();
    db.save_color_variant(&variant("user.1", "Gone", "A", "a"))
        .unwrap();
    db.save_color_variant(&variant("user.2", "gone", "B", "b"))
        .unwrap();
    db.save_color_variant(&variant("user.3", "Kept", "C", "c"))
        .unwrap();
    assert_eq!(db.delete_color_variants_of_material("GONE").unwrap(), 2);
    assert_eq!(db.delete_color_variants_of_material("GONE").unwrap(), 0);
    assert!(db.load_color_variant("user.3").unwrap().is_some());
}

#[test]
fn pruning_removes_the_variants_of_deleted_custom_materials_but_not_of_built_ins() {
    let db = in_memory_db();
    db.save_custom_material(&custom_material("Kept")).unwrap();
    db.save_color_variant(&variant("user.1", "Kept", "A", "a"))
        .unwrap();
    db.save_color_variant(&variant("user.2", "Deleted", "B", "b"))
        .unwrap();
    db.save_color_variant(&variant("user.3", "sapphire", "C", "c"))
        .unwrap();
    let removed = db
        .prune_color_variant_orphans(&["Sapphire", "Ruby"])
        .unwrap();
    assert_eq!(removed, 1);
    assert!(db.load_color_variant("user.1").unwrap().is_some());
    assert!(db.load_color_variant("user.2").unwrap().is_none());
    assert!(
        db.load_color_variant("user.3").unwrap().is_some(),
        "a variant of a built-in material is not an orphan"
    );
    assert_eq!(
        db.prune_color_variant_orphans(&["Sapphire", "Ruby"])
            .unwrap(),
        0
    );
}

#[test]
fn a_newer_version_is_stored_and_read_back_unchanged() {
    let db = in_memory_db();
    let mut row = variant("user.a", "Sapphire", "Teal", "one");
    row.version = 7;
    db.save_color_variant(&row).unwrap();
    assert_eq!(db.load_color_variant("user.a").unwrap().unwrap().version, 7);
}

#[test]
fn a_read_only_connection_reads_nothing_and_refuses_writes() {
    let path = temp_db_path("color_variant_read_only");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.save_color_variant(&variant("user.ro", "Ro", "A", "a"))
            .unwrap();
        db.conn
            .execute_batch("DROP TABLE material_color_variants;")
            .unwrap();
    }
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(ro.load_color_variant("user.ro").unwrap(), None);
    assert!(ro.load_color_variants_of_material("Ro").unwrap().is_empty());
    assert_eq!(ro.delete_color_variant("user.ro").unwrap(), 0);
    assert_eq!(ro.delete_color_variants_of_material("Ro").unwrap(), 0);
    assert_eq!(ro.rename_color_variant_label("user.ro", "B").unwrap(), 0);
    assert_eq!(ro.rename_color_variant_material("Ro", "No").unwrap(), 0);
    assert_eq!(ro.prune_color_variant_orphans(&["Sapphire"]).unwrap(), 0);
    assert!(
        ro.save_color_variant(&variant("user.no", "No", "A", "a"))
            .is_err()
    );
    drop(ro);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn a_read_only_connection_with_the_table_prunes_nothing_when_there_are_no_orphans() {
    let path = temp_db_path("color_variant_read_only_with_table");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.save_color_variant(&variant("user.ro", "Sapphire", "A", "a"))
            .unwrap();
    }
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(ro.prune_color_variant_orphans(&["Sapphire"]).unwrap(), 0);
    assert_eq!(
        ro.load_color_variants_of_material("Sapphire")
            .unwrap()
            .len(),
        1
    );
    drop(ro);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
