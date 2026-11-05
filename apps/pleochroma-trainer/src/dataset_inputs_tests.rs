use super::*;
use pleochroma_cut_core::{
    CrystalAxisPlan, StoneRenderEffects, TierTarget,
    crystal_axis::PlanRuleOrigin,
    native::{CustomMaterialSnapshot, DesignExtras},
};

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

fn unspecified_native_design() -> Design {
    let mut design = native_design();
    design.material = MaterialSelection::none();
    design.render_effects = StoneRenderEffects::default();
    design.girdle_diameter_mm = None;
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
    let loaded = load_design_data(&record(&design, None), 128).unwrap();
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.tiers, design.tiers);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.girdle_diameter_mm, Some(9.25));
    assert_eq!(loaded.design.render_effects, design.render_effects);
    assert_eq!(loaded.authored_material.unwrap().gem.name, "Tanzanite");
    assert!(!loaded.physical_size_augmented);
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
    let loaded = load_design_data(&record(&design, Some(&snapshot)), 128).unwrap();
    assert_eq!(
        loaded.authored_material.unwrap().gem,
        native::gem_material_from_custom_snapshot("Trainer Custom", &snapshot)
    );
    assert!(
        load_design_data(&record(&design, None), 128)
            .unwrap_err()
            .contains("no embedded snapshot")
    );
}

#[test]
fn a_custom_snapshot_without_a_selected_material_is_not_treated_as_unspecified() {
    let mut file =
        native::design_to_file(&unspecified_native_design(), None, &DesignExtras::default());
    file.material.custom = Some(CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    ));
    let input = DbDesignRecord {
        asc_bytes: pleochroma_formats::native::design::to_string(&file)
            .unwrap()
            .into_bytes(),
        ..record(&unspecified_native_design(), None)
    };
    let error = load_design_data(&input, 128).unwrap_err();
    assert!(error.contains("custom snapshot has no material name"));
}

#[test]
fn legacy_input_gets_inferred_rough_and_explicit_size_augmentation() {
    let legacy = DbDesignRecord {
        id: 2,
        title: "legacy".to_owned(),
        filename: "legacy.asc".to_owned(),
        asc_bytes: ASC.as_bytes().to_vec(),
    };
    let loaded = load_design_data(&legacy, 128).unwrap();
    assert!(loaded.authored_material.is_none());
    assert!(loaded.design.preform.half_width > 1.0 && loaded.design.preform.half_width < 2.0);
    assert!(loaded.design.girdle_diameter_mm.is_some());
    assert!(loaded.physical_size_augmented);
}

#[test]
fn unspecified_native_context_gets_repeatable_size_without_changing_authored_content() {
    let design = unspecified_native_design();
    let native_record = record(&design, None);
    let original_bytes = native_record.asc_bytes.clone();
    let loaded = load_design_data(&native_record, 128).unwrap();
    let again = load_design_data(&native_record, 128).unwrap();
    assert!(loaded.authored_material.is_none());
    assert!(loaded.physical_size_augmented);
    let width = loaded.design.girdle_diameter_mm.unwrap();
    assert!([4.0, 5.5, 7.0, 10.0, 14.0].contains(&width));
    assert_eq!(
        loaded.design.girdle_diameter_mm,
        again.design.girdle_diameter_mm
    );
    let mut restored = loaded.design;
    restored.girdle_diameter_mm = None;
    assert_eq!(
        restored, design,
        "only the missing physical size may change"
    );
    assert_eq!(native_record.asc_bytes, original_bytes);

    let asc = DbDesignRecord {
        id: native_record.id,
        title: "same source".to_owned(),
        filename: "same.asc".to_owned(),
        asc_bytes: ASC.as_bytes().to_vec(),
    };
    let imported = load_design_data(&asc, 128).unwrap();
    assert_eq!(imported.design.girdle_diameter_mm, Some(width));
}

#[test]
fn size_and_material_missingness_are_independent() {
    let mut sized = unspecified_native_design();
    sized.girdle_diameter_mm = Some(6.125);
    let loaded = load_design_data(&record(&sized, None), 128).unwrap();
    assert_eq!(loaded.design.girdle_diameter_mm, Some(6.125));
    assert!(!loaded.physical_size_augmented);
    assert!(loaded.authored_material.is_none());

    let mut materialled = unspecified_native_design();
    materialled.material.name = Some("Tanzanite".to_owned());
    let loaded = load_design_data(&record(&materialled, None), 128).unwrap();
    assert!(loaded.physical_size_augmented);
    assert_eq!(loaded.authored_material.unwrap().gem.name, "Tanzanite");
    assert_eq!(loaded.design.material, materialled.material);
}

#[test]
fn unnamed_authored_material_overrides_are_never_catalog_augmented() {
    let selections = [
        MaterialSelection {
            refractive_index_override: Some(1.71),
            ..MaterialSelection::none()
        },
        MaterialSelection {
            specific_gravity_override: Some(3.5),
            ..MaterialSelection::none()
        },
        MaterialSelection {
            body_color_override: Some([0.2, 0.5, 0.8]),
            ..MaterialSelection::none()
        },
        MaterialSelection {
            body_color_bands_override: Some(vec![[450.0, 20.0, 0.1]]),
            absorption_path_scale_override: Some(4.5),
            ..MaterialSelection::none()
        },
    ];
    for selection in selections {
        let mut design = unspecified_native_design();
        design.material = selection;
        let loaded = load_design_data(&record(&design, None), 128).unwrap();
        assert!(loaded.authored_material.is_some());
        assert_eq!(loaded.design.material, design.material);
    }
}

#[test]
fn axis_and_render_context_preserve_the_original_source_without_a_preset_name() {
    let mut design = unspecified_native_design();
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 23.0;
    let loaded = load_design_data(&record(&design, None), 128).unwrap();
    assert!(loaded.authored_material.is_some());
    assert_eq!(loaded.design.render_effects, design.render_effects);

    design.render_effects = StoneRenderEffects::default();
    design.crystal_axis_plan = Some(CrystalAxisPlan {
        rule: None,
        origin: PlanRuleOrigin::Design,
        reason: "Use the original source orientation".to_owned(),
    });
    let loaded = load_design_data(&record(&design, None), 128).unwrap();
    assert!(loaded.authored_material.is_some());
    assert_eq!(loaded.design.crystal_axis_plan, design.crystal_axis_plan);
}

#[test]
fn unknown_material_and_effect_fields_prevent_source_material_sampling() {
    let base = record(&unspecified_native_design(), None);
    let text = String::from_utf8(base.asc_bytes.clone()).unwrap();
    let material = text.replace("[material]", "[material]\nfuture_absorption = true");
    assert_ne!(
        material, text,
        "fixture must contain the native material table"
    );
    for text in [
        material,
        format!("{text}\n[render_effects]\nfuture_scattering = true\n"),
        format!("{text}\n[crystal_axis_plan]\nfuture_axis = true\n"),
    ] {
        let input = DbDesignRecord {
            asc_bytes: text.into_bytes(),
            ..base.clone()
        };
        let loaded = load_design_data(&input, 128).unwrap();
        assert!(loaded.authored_material.is_some());
    }
}

fn raw_native_with_context(material_fields: &str, context: &str) -> DbDesignRecord {
    let base = record(&unspecified_native_design(), None);
    let text = String::from_utf8(base.asc_bytes.clone()).unwrap();
    let text = text.replace("[material]", &format!("[material]\n{material_fields}"));
    DbDesignRecord {
        asc_bytes: format!("{text}\n{context}\n").into_bytes(),
        ..base
    }
}

fn raw_variant(unit: &str, shape: &str, fields: &str) -> String {
    format!(
        r#"[material.color_variant]
key = "custom"
label = "Authored Variant"
unit = "{unit}"
reference_path_mm = 7.0
pleochroic = false
{fields}
[[material.color_variant.o_ray]]
center_nm = 500.0
width_nm = 20.0
peak = 0.2
shape = "{shape}"
"#
    )
}

#[test]
fn unsupported_declared_effects_and_axis_rules_are_not_silently_replaced() {
    for (context, message) in [
        (
            "[render_effects.inclusions]\npreset = 'future_inclusion'",
            "native inclusion preset",
        ),
        (
            "[render_effects.structure]\npattern = 'future_structure'",
            "native structure pattern",
        ),
        (
            "[crystal_axis_plan]\nrule = 'future_axis_rule'",
            "native crystal-axis rule",
        ),
        (
            "[crystal_axis_plan]\nrule = 'face_up'\npreset = 'future_preset'",
            "native crystal-axis rule",
        ),
    ] {
        let error = load_design_data(&raw_native_with_context("", context), 128).unwrap_err();
        assert!(error.contains(message), "{context}: {error}");
    }
}

#[test]
fn current_unknown_color_variants_require_a_supported_legacy_fallback() {
    for variant in [
        raw_variant("future_unit", "wavelength", ""),
        raw_variant("per_mm", "future_shape", ""),
    ] {
        let error = load_design_data(&raw_native_with_context("", &variant), 128).unwrap_err();
        assert!(error.contains("has no supported color fallback"), "{error}");

        let input = raw_native_with_context("body_color_override = [0.2, 0.5, 0.8]", &variant);
        let loaded = load_design_data(&input, 128).unwrap();
        assert!(loaded.authored_material.is_some());
        assert!(loaded.design.material.color_variant_override.is_none());
        assert_eq!(
            loaded.design.material.body_color_override,
            Some([0.2, 0.5, 0.8])
        );
    }
}

#[test]
fn native_color_variants_keep_supported_tensors_and_older_editor_color_edits() {
    let variant = raw_variant("per_mm", "wavelength", "");
    let loaded = load_design_data(&raw_native_with_context("", &variant), 128).unwrap();
    assert!(loaded.authored_material.is_some());
    let choice = loaded.design.material.color_variant_override.unwrap();
    assert_eq!(choice.label, "Authored Variant");
    assert_eq!(choice.reference_path_mm, 7.0);

    let variant = raw_variant("per_mm", "wavelength", "fallback_rgb = [0.8, 0.4, 0.1]");
    let changed = raw_native_with_context("body_color_override = [0.2, 0.5, 0.8]", &variant);
    let loaded = load_design_data(&changed, 128).unwrap();
    assert!(loaded.authored_material.is_some());
    assert!(loaded.design.material.color_variant_override.is_none());
    assert_eq!(
        loaded.design.material.body_color_override,
        Some([0.2, 0.5, 0.8])
    );

    // Removing the recorded fallback means the older editor chose Material Default.
    // The core intentionally discards the stale variant and keeps that source reset.
    let loaded = load_design_data(&raw_native_with_context("", &variant), 128).unwrap();
    assert!(loaded.authored_material.is_some());
    assert_eq!(loaded.design.material, MaterialSelection::none());
}

#[test]
fn raw_context_survives_classification_when_conversion_reduces_it_to_defaults() {
    for input in [
        raw_native_with_context("body_color_bands_override = []", ""),
        raw_native_with_context("", "[crystal_axis_plan]"),
    ] {
        let loaded = load_design_data(&input, 128).unwrap();
        assert!(loaded.authored_material.is_some());
    }
}

#[test]
fn dimensional_targets_require_an_authored_physical_anchor() {
    let mut design = unspecified_native_design();
    let tier = design.tier_id_at(1).unwrap();
    design.tier_targets.insert(tier, TierTarget::DepthMm(1.2));
    let error = load_design_data(&record(&design, None), 128).unwrap_err();
    assert!(error.contains("dimensional tier targets require an authored girdle_diameter_mm"));
    design.girdle_diameter_mm = Some(8.5);
    let loaded = load_design_data(&record(&design, None), 128).unwrap();
    assert!(!loaded.physical_size_augmented);
    assert_eq!(loaded.design.girdle_diameter_mm, Some(8.5));
    assert_eq!(loaded.design.tier_targets, design.tier_targets);
    assert_eq!(loaded.design.tiers, design.tiers);
}

#[test]
fn invalid_authored_sizes_are_refused_instead_of_replaced() {
    for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut design = unspecified_native_design();
        design.girdle_diameter_mm = Some(width);
        let error = augment_missing_size(&mut design, 17).unwrap_err();
        assert!(error.contains("finite and positive"));
        assert_eq!(
            design.girdle_diameter_mm.unwrap().to_bits(),
            width.to_bits()
        );
    }
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
