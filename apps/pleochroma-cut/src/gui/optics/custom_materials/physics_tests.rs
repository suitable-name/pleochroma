//! The material editor's save path for the render-physics sections: the per-axis curves, the
//! inclusion families and the play-of-color structure reach the vault row and the live copy, a
//! rename carries them, and a save that authored none keeps what the row has.

use super::{
    CustomMaterialForm, apply_custom_material_save,
    crystal_axis::DefaultAxisForm,
    curves::CurvesForm,
    inclusions::{PhysicsForm, StructureForm},
};
use crate::{FamilyRow, bridge::render_thread::RenderContext};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_vault::{db::sqlite::Database, model::material::CustomMaterialRow};
use slint::SharedString;
use std::sync::{Arc, Mutex};

fn setup() -> (Arc<Mutex<Database>>, Arc<Mutex<RenderContext>>) {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    (
        Arc::new(Mutex::new(db)),
        Arc::new(Mutex::new(RenderContext::default())),
    )
}

/// A uniaxial (negative) material's form carrying `physics`.
fn form(name: &str, physics: Option<PhysicsForm>) -> CustomMaterialForm {
    CustomMaterialForm {
        name: SharedString::from(name),
        ri: 1.768,
        disp: 0.018,
        biref: -0.008,
        color_idx: 3,
        crystal_system_idx: 3,
        optical_character_idx: 2,
        biaxial_delta_beta_alpha: 0.0,
        specific_gravity: 0.0,
        apply_to_live_render: false,
        color_recipe_json: SharedString::default(),
        dispersion_model_json: SharedString::default(),
        body_bands: None,
        body_rays: None,
        default_axis: DefaultAxisForm::default(),
        physics,
    }
}

fn texts(a: &str, b: &str) -> Vec<String> {
    vec![
        a.to_owned(),
        b.to_owned(),
        "0".to_owned(),
        String::new(),
        String::new(),
        String::new(),
    ]
}

fn family() -> FamilyRow {
    FamilyRow {
        kind: 0,
        dir: 0,
        turn: 0.0,
        tilt: 0.0,
        strength: 0.2,
        color: 0,
        cone: 0.1,
        forward: 0.3,
    }
}

/// Curves, one family and an opal structure.
fn authored() -> PhysicsForm {
    PhysicsForm {
        curves: CurvesForm {
            on: true,
            texts: [texts("1.76", "0.004"), texts("", "")],
            ..CurvesForm::default()
        },
        families: vec![family()],
        structure: StructureForm::with_defaults(0, true),
    }
}

fn stored(db: &Arc<Mutex<Database>>, name: &str) -> CustomMaterialRow {
    db.lock()
        .unwrap()
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|r| r.name == name)
        .expect("row saved")
}

fn live(ctx: &Arc<Mutex<RenderContext>>, name: &str) -> GemMaterial {
    ctx.lock()
        .unwrap()
        .custom_materials
        .iter()
        .find(|m| m.name == name)
        .cloned()
        .expect("the material is in the live list")
}

#[test]
fn the_three_sections_reach_the_row_and_the_live_copy() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Physics Probe", Some(authored()))).expect("saves");
    let row = stored(&db, "Physics Probe");
    assert!(row.per_axis_dispersion_json.is_some());
    assert!(row.inclusion_families_json.is_some());
    let structure = row.structure_json.as_deref().expect("structure stored");
    assert!(structure.contains("opal"), "the kind is stored with it");
    let material = live(&ctx, "Physics Probe");
    assert!(material.uniaxial_extraordinary_dispersion.is_some());
    let set = material
        .oriented_inclusions()
        .expect("silk on the live copy");
    assert!(set.structure().is_some(), "the named kind is applied");
}

#[test]
fn a_rename_carries_what_the_dialog_showed() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Old Name", Some(authored()))).expect("saves");
    // The dialog opens on the stored row and the user only changes the name.
    let opened = PhysicsForm::from_row(Some(&stored(&db, "Old Name")));
    apply_custom_material_save(&db, &ctx, form("New Name", Some(opened))).expect("saves");
    let old = stored(&db, "Old Name");
    let new = stored(&db, "New Name");
    assert_eq!(new.per_axis_dispersion_json, old.per_axis_dispersion_json);
    assert_eq!(new.inclusion_families_json, old.inclusion_families_json);
    assert_eq!(new.structure_json, old.structure_json);
}

#[test]
fn an_unusable_curve_refuses_the_save_and_writes_nothing() {
    let (db, ctx) = setup();
    let mut physics = authored();
    physics.curves.texts[0] = vec!["abc".to_owned(); 6];
    let Err(refused) = apply_custom_material_save(&db, &ctx, form("Refused", Some(physics))) else {
        panic!("the curve cannot be used, so the save is refused");
    };
    assert!(refused.contains("extraordinary-ray"));
    assert!(
        db.lock()
            .unwrap()
            .get_custom_materials()
            .unwrap()
            .iter()
            .all(|r| r.name != "Refused")
    );
}

#[test]
fn a_save_that_authored_nothing_keeps_the_row_and_the_live_copy() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Kept", Some(authored()))).expect("saves");
    apply_custom_material_save(&db, &ctx, form("Kept", None)).expect("saves again");
    let row = stored(&db, "Kept");
    assert!(row.per_axis_dispersion_json.is_some());
    assert!(row.inclusion_families_json.is_some());
    assert!(row.structure_json.is_some());
    assert!(live(&ctx, "Kept").oriented_inclusions().is_some());
}

#[test]
fn blank_sections_clear_the_columns() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Cleared", Some(authored()))).expect("saves");
    apply_custom_material_save(
        &db,
        &ctx,
        form("Cleared", Some(PhysicsForm::from_row(None))),
    )
    .expect("saves again");
    let row = stored(&db, "Cleared");
    assert_eq!(row.per_axis_dispersion_json, None);
    assert_eq!(row.inclusion_families_json, None);
    assert_eq!(row.structure_json, None);
    assert!(live(&ctx, "Cleared").oriented_inclusions().is_none());
}

#[test]
fn a_structure_without_a_kind_is_stored_but_not_applied() {
    let (db, ctx) = setup();
    let mut physics = PhysicsForm::from_row(None);
    physics.structure = StructureForm::with_defaults(0, true);
    physics.structure.kind = 2;
    apply_custom_material_save(&db, &ctx, form("Kindless", Some(physics))).expect("saves");
    let row = stored(&db, "Kindless");
    assert!(row.structure_json.is_some());
    assert!(live(&ctx, "Kindless").oriented_inclusions().is_none());
}
