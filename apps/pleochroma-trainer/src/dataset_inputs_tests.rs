use super::*;
use pleochroma_cut_core::native::{CustomMaterialSnapshot, DesignExtras};

const ASC: &str = "GemCad 5.0\ng 4 0.0\ny 1 n\nI 1.62\n\
a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
a 0.000000 0.60000000 G Set stone size\n\
a -0.000000 0.55000000 G Set stone size\n";

fn native_design() -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(ASC).unwrap();
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.4, 3.0), &schedule);
    design.material.name = Some("Tanzanite".to_owned());
    design.girdle_diameter_mm = Some(9.25);
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 24.0;
    design.render_effects.c_axis_azimuth_deg = 113.0;
    design
}

fn record(design: &Design, custom: Option<&CustomMaterialSnapshot>) -> DbDesignRecord {
    let extras = DesignExtras {
        custom_material: custom,
        ..DesignExtras::default()
    };
    let text = native::design_to_string(design, None, &extras).unwrap();
    DbDesignRecord {
        id: 1,
        title: "native".to_owned(),
        filename: "native.psdd".to_owned(),
        asc_bytes: text.into_bytes(),
    }
}

#[test]
fn native_loading_preserves_authored_geometry_rough_material_size_and_axes() {
    let design = native_design();
    let (loaded, material) = load_design_data(&record(&design, None), 128).unwrap();
    assert_eq!(loaded.preform, design.preform);
    assert_eq!(loaded.tiers, design.tiers);
    assert_eq!(loaded.material, design.material);
    assert_eq!(loaded.girdle_diameter_mm, Some(9.25));
    assert_eq!(loaded.render_effects, design.render_effects);
    assert_eq!(material.unwrap().gem.name, "Tanzanite");
}

#[test]
fn embedded_custom_material_is_restored_and_missing_snapshot_is_not_diamond() {
    let mut design = native_design();
    design.material.name = Some("Trainer Custom".to_owned());
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    let (_, material) = load_design_data(&record(&design, Some(&snapshot)), 128).unwrap();
    assert_eq!(
        material.unwrap().gem,
        native::gem_material_from_custom_snapshot("Trainer Custom", &snapshot)
    );
    assert!(
        load_design_data(&record(&design, None), 128)
            .unwrap_err()
            .contains("no embedded snapshot")
    );
}

#[test]
fn legacy_input_gets_inferred_rough_and_explicit_size_augmentation() {
    let legacy = DbDesignRecord {
        id: 2,
        title: "legacy".to_owned(),
        filename: "legacy.asc".to_owned(),
        asc_bytes: ASC.as_bytes().to_vec(),
    };
    let (asc, authored) = load_design_data(&legacy, 128).unwrap();
    assert!(authored.is_none());
    assert!(asc.preform.half_width > 1.0 && asc.preform.half_width < 2.0);
    assert!(asc.girdle_diameter_mm.is_some());
}

#[test]
fn native_and_asc_geometry_siblings_cannot_leak_across_splits() {
    let native = record(&native_design(), None);
    let asc = DbDesignRecord {
        id: 2,
        title: "legacy".to_owned(),
        filename: "legacy.asc".to_owned(),
        asc_bytes: ASC.as_bytes().to_vec(),
    };
    let split = super::super::DatasetSplit::from_records(&[native, asc], 0, 128).unwrap();
    assert_eq!(split.coverage().geometric_families, 1);
    assert_eq!(split.train.len(), 2);
    assert!(split.val.is_empty() && split.test.is_empty());
}
