//! Tests of the Color Pattern storage API against an in-memory vault.

use super::store::{
    copy_pattern, load_pattern, load_presets, remove_pattern, save_pattern, save_preset,
    write_back_pattern_base,
};
use crate::gui::rough_colour::{
    material_zones::with_base_bands,
    store::{load_material_zoning, save_material_zoning},
};
use pleochroma::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
    zoning::{ZoneAbsorption, ZonedAbsorption},
};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, PatternAxis, TemplateId, bands_from_rows, template,
};
use pleochroma_vault::db::sqlite::{CustomMaterialParams, Database};

const WIDTH_MM: f64 = 6.5;

fn db() -> Database {
    Database::new(Some(":memory:")).expect("in-memory vault")
}

fn add_material(db: &Database, name: &str) {
    db.save_custom_material(&CustomMaterialParams {
        name,
        refractive_index: 1.55,
        dispersion: 0.01,
        birefringence: 0.0,
        absorption_rgb: [0.1, 0.2, 0.3],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: None,
        color_recipe_json: Some("{}"),
        dispersion_model_json: None,
        absorption_bands_json: None,
        c_axis_tilt_deg: None,
        c_axis_azimuth_deg: None,
        c_axis_second_azimuth_deg: None,
        inclusion_families_json: None,
        structure_json: None,
    })
    .expect("material saved");
}

/// A template pattern with every color solved by a distinct band row.
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn solved(id: TemplateId) -> ColorPattern {
    let mut pattern = template(id, PatternAxis::TABLE_NORMAL);
    for (n, at) in pattern.unsolved_colors().into_iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a handful of zone indices, exact in f32"
        )]
        let centre = 450.0 + 40.0 * n as f32;
        assert!(pattern.set_color_bands(at, vec![[centre, 30.0, 0.5]]));
    }
    pattern
}

fn material_bands_json(db: &Database, name: &str) -> Option<String> {
    db.get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|row| row.name == name)
        .and_then(|row| row.absorption_bands_json)
}

#[test]
fn a_saved_pattern_round_trips_with_its_zones_and_the_base_color() {
    let db = db();
    add_material(&db, "Melon");
    let pattern = solved(TemplateId::Watermelon);
    save_pattern(&db, "melon", &pattern, WIDTH_MM).unwrap();

    assert_eq!(load_pattern(&db, "Melon").unwrap(), Some(pattern.clone()));
    let zoning = load_material_zoning(&db, "Melon").unwrap().unwrap();
    assert_eq!(zoning.relative_to_stone, pattern.relative_to_stone);
    assert_eq!(zoning.zoning.zones.len(), pattern.zones.len());

    // The material row shows the base color; the recipe and triple no longer take precedence.
    let row = db
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|row| row.name == "Melon")
        .unwrap();
    let base_rows = match &pattern.base.color {
        pleochroma_cut_core::material::pattern::PatternColor::Single(c) => c.bands.clone(),
        pleochroma_cut_core::material::pattern::PatternColor::Dichroic { along, .. } => {
            along.bands.clone()
        }
        other => panic!("the pattern of this test has a picked base color: {other:?}"),
    };
    let stored: Vec<[f32; 3]> =
        serde_json::from_str(row.absorption_bands_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored, base_rows);
    assert_eq!(row.absorption_rgb, [0.0; 3]);
    assert_eq!(row.color_recipe_json, None);
}

#[test]
fn a_uniform_pattern_removes_the_zones_and_an_unsolved_one_is_refused() {
    let db = db();
    add_material(&db, "Melon");
    save_pattern(&db, "Melon", &solved(TemplateId::Watermelon), WIDTH_MM).unwrap();
    assert!(load_material_zoning(&db, "Melon").unwrap().is_some());
    save_pattern(&db, "Melon", &solved(TemplateId::Uniform), WIDTH_MM).unwrap();
    assert!(load_material_zoning(&db, "Melon").unwrap().is_none());
    assert!(load_pattern(&db, "Melon").unwrap().is_some());

    let unsolved = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    let error = save_pattern(&db, "Melon", &unsolved, WIDTH_MM).unwrap_err();
    assert!(format!("{error:#}").contains("Pick a color"), "{error:#}");
}

#[test]
fn a_pattern_needs_a_saved_material() {
    let db = db();
    let error = save_pattern(&db, "Nowhere", &solved(TemplateId::Bicolor), WIDTH_MM).unwrap_err();
    assert!(format!("{error:#}").contains("Nowhere"), "{error:#}");
    assert_eq!(load_pattern(&db, "Nowhere").unwrap(), None);
}

#[test]
fn removing_drops_pattern_and_zones_but_keeps_the_material() {
    let db = db();
    add_material(&db, "Melon");
    save_pattern(&db, "Melon", &solved(TemplateId::Bicolor), WIDTH_MM).unwrap();
    remove_pattern(&db, "melon").unwrap();
    assert_eq!(load_pattern(&db, "Melon").unwrap(), None);
    assert!(load_material_zoning(&db, "Melon").unwrap().is_none());
    assert!(
        material_bands_json(&db, "Melon").is_some(),
        "the base color stays"
    );
    remove_pattern(&db, "Melon").unwrap();
}

#[test]
fn a_copy_gets_the_pattern_unless_it_has_its_own() {
    let db = db();
    add_material(&db, "Melon");
    add_material(&db, "Copy");
    add_material(&db, "Other");
    let pattern = solved(TemplateId::Banded);
    save_pattern(&db, "Melon", &pattern, WIDTH_MM).unwrap();
    save_pattern(&db, "Other", &solved(TemplateId::Bicolor), WIDTH_MM).unwrap();

    assert!(!copy_pattern(&db, "Plain", "Copy").unwrap());
    assert!(!copy_pattern(&db, "Melon", "melon").unwrap());
    assert!(copy_pattern(&db, "Melon", "Copy").unwrap());
    assert_eq!(load_pattern(&db, "Copy").unwrap(), Some(pattern));
    assert!(!copy_pattern(&db, "Melon", "Other").unwrap());
    assert_ne!(
        load_pattern(&db, "Other").unwrap().unwrap().template,
        load_pattern(&db, "Melon").unwrap().unwrap().template
    );
}

#[test]
fn the_base_color_write_back_updates_the_pattern() {
    let db = db();
    add_material(&db, "Melon");
    let pattern = solved(TemplateId::Watermelon);
    let old_lch = pattern.base.color.clone();
    save_pattern(&db, "Melon", &pattern, WIDTH_MM).unwrap();

    let edited = [[520.0_f32, 35.0, 0.4]];
    assert!(write_back_pattern_base(&db, "Melon", &edited, None).unwrap());
    let stored = load_pattern(&db, "Melon").unwrap().unwrap();
    let color = stored
        .color(pleochroma_cut_core::material::pattern::ColorRef {
            zone: 0,
            slot: pleochroma_cut_core::material::pattern::ColorSlot::Single,
        })
        .unwrap();
    assert_eq!(color.bands, edited.to_vec());
    assert!(color.is_solved());
    assert_eq!(
        stored.zones, pattern.zones,
        "the shaped zones are untouched"
    );
    assert_ne!(stored.base.color, old_lch);

    // The same color again changes nothing; a material without a pattern is not touched.
    assert!(!write_back_pattern_base(&db, "Melon", &edited, None).unwrap());
    assert!(!write_back_pattern_base(&db, "Plain", &edited, None).unwrap());
    assert!(write_back_pattern_base(&db, "Melon", &[[f32::NAN, 30.0, 0.2]], None).is_err());
}

#[test]
fn a_two_color_base_is_not_written_back() {
    let db = db();
    add_material(&db, "Melon");
    let mut pattern = solved(TemplateId::Bicolor);
    assert!(pattern.set_two_colors(0, true));
    for at in pattern.unsolved_colors() {
        assert!(pattern.set_color_bands(at, vec![[500.0, 30.0, 0.5]]));
    }
    save_pattern(&db, "Melon", &pattern, WIDTH_MM).unwrap();
    assert!(!write_back_pattern_base(&db, "Melon", &[[600.0, 30.0, 0.2]], None).unwrap());
    assert_eq!(load_pattern(&db, "Melon").unwrap(), Some(pattern));
}

#[test]
fn presets_save_load_rename_and_delete() {
    let db = db();
    assert_eq!(load_presets(&db).unwrap().len(), 0);
    let pattern = solved(TemplateId::Tricolor);
    save_preset(&db, "  Beta ", &pattern).unwrap();
    save_preset(&db, "Alpha", &solved(TemplateId::Bicolor)).unwrap();
    assert!(save_preset(&db, "  ", &pattern).is_err());

    let presets = load_presets(&db).unwrap();
    assert_eq!(
        presets
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert_eq!(presets[1].1, pattern);
}

#[test]
fn a_preset_that_cannot_be_read_is_skipped() {
    let db = db();
    save_preset(&db, "Good", &solved(TemplateId::Bicolor)).unwrap();
    db.save_color_pattern_preset(&pleochroma_vault::model::zoning::ColorPatternPresetRow {
        name: "Broken".to_string(),
        pattern_json: "{ not json".to_string(),
        version: 1,
    })
    .unwrap();
    let presets = load_presets(&db).unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].0, "Good");
}

#[test]
fn a_pattern_with_a_variant_and_three_colors_is_stored_as_version_two_and_a_plain_one_as_one() {
    use pleochroma::optics::materials::variants::built_in_variants;
    use pleochroma_cut_core::material::pattern::{
        PATTERN_BASE_FORMAT_VERSION, PATTERN_FORMAT_VERSION, PatternColor, ZoneColor,
    };
    let db = db();
    add_material(&db, "Melon");

    // A pattern of the first release's colors is still a version 1 row.
    save_pattern(&db, "Melon", &solved(TemplateId::Bicolor), WIDTH_MM).unwrap();
    let row = db.load_material_pattern("Melon").unwrap().unwrap();
    assert_eq!(row.version, PATTERN_BASE_FORMAT_VERSION);
    assert!(row.pattern_json.starts_with(r#"{"version":1,"#));

    // A variant zone and a three-color base need version 2, in the row and in the text.
    let yellow = built_in_variants("Sapphire")
        .into_iter()
        .find(|variant| variant.key == "sapphire.yellow")
        .unwrap();
    let mut rich = solved(TemplateId::Bicolor);
    let color = |centre: f32| ZoneColor::solved([50.0, 30.0, 100.0], vec![[centre, 40.0, 0.3]]);
    rich.base.color = PatternColor::Trichroic {
        alpha: color(450.0),
        beta: color(520.0),
        gamma: color(600.0),
    };
    rich.zones[0].color = PatternColor::from_variant(&yellow);
    let rich = rich.with_required_version();
    save_pattern(&db, "Melon", &rich, WIDTH_MM).unwrap();
    let row = db.load_material_pattern("Melon").unwrap().unwrap();
    assert_eq!(row.version, PATTERN_FORMAT_VERSION);
    assert!(row.pattern_json.starts_with(r#"{"version":2,"#));
    assert_eq!(load_pattern(&db, "Melon").unwrap(), Some(rich));

    // The material row shows the alpha color of the base zone.
    let material = db
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|row| row.name == "Melon")
        .unwrap();
    let stored: Vec<[f32; 3]> =
        serde_json::from_str(material.absorption_bands_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored, vec![[450.0, 40.0, 0.3]]);
}

#[test]
fn energy_rows_stay_energy_bands_in_the_edited_base_zone() {
    let wavelength =
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            480.0, 40.0, 0.05,
        )]));
    let zoning = ZonedAbsorption::new(wavelength);
    let rows = [[560.0_f32, 1800.0, 0.3], [450.0, 40.0, 0.1]];
    let updated = with_base_bands(&zoning, &rows).expect("a new base color");
    assert_eq!(updated.base.tensor.o_ray, bands_from_rows(&rows));
    assert_eq!(
        updated.base.tensor.o_ray[0].shape,
        BandShape::GaussianEnergy
    );
    assert_eq!(
        updated.base.tensor.o_ray[1].shape,
        BandShape::GaussianWavelength
    );

    // Saved and read back, the same rows give the same base zone.
    let db = db();
    save_material_zoning(&db, "Energy", &updated, false).unwrap();
    let stored = load_material_zoning(&db, "Energy").unwrap().unwrap();
    assert_eq!(stored.zoning.base.tensor.o_ray, bands_from_rows(&rows));
}
