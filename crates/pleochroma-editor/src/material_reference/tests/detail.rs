//! Tests of the details model: chart paths, series counts, colors, variants, effects and weight.

use pleochroma::{
    color::{
        body_color::{Illuminant, body_colors},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::{
        dispersion::DispersionModel,
        inclusions::catalogue::presets_for,
        materials::{
            GemMaterial, MaterialLook, OpticalCharacter,
            variants::{built_in_variants, face_up_swatch, face_up_swatch_path},
        },
    },
};
use pleochroma_cut_core::material::{BuiltinMaterials, pattern::typical_patterns};

use crate::material_reference::{
    DetailContext, MaterialDetail,
    curves::{index_curve_path_fitted, shared_index_range},
    detail::material_detail_for_variant,
    index_curve_path, material_detail,
};

fn detail(name: &str) -> MaterialDetail {
    material_detail(name, &BuiltinMaterials, &DetailContext::default())
        .unwrap_or_else(|| panic!("{name} has a details page"))
}

/// The `(x, y)` points of an `M x y L x y ...` path.
fn points(path: &str) -> Vec<(f32, f32)> {
    let tokens: Vec<&str> = path.split_whitespace().collect();
    tokens
        .chunks(3)
        .map(|c| (c[1].parse().expect("x"), c[2].parse().expect("y")))
        .collect()
}

#[test]
fn index_paths_stay_inside_the_viewbox_and_fall_with_wavelength() {
    for material in GemMaterial::built_in() {
        let d = detail(&material.name);
        for series in &d.ri.series {
            let pts = points(&series.path);
            assert_eq!(pts.len(), 81, "{}", material.name);
            for (x, y) in &pts {
                assert!((0.0..=100.0).contains(x), "{} x {x}", material.name);
                assert!((0.0..=100.0).contains(y), "{} y {y}", material.name);
            }
            // The index falls toward red, so y (downward) never decreases.
            for pair in pts.windows(2) {
                assert!(
                    pair[1].1 >= pair[0].1 - 0.006,
                    "{} {}: the curve rises with wavelength",
                    material.name,
                    series.label
                );
            }
        }
    }
}

#[test]
fn shared_index_path_matches_the_editor_sparkline_rule() {
    let model = GemMaterial::by_name("Sapphire")
        .expect("Sapphire")
        .dispersion;
    let (fitted, top, bottom) = index_curve_path_fitted(&model);
    assert_eq!(fitted, index_curve_path(&model, bottom, top));
    assert!(top > bottom);
    let samples: Vec<f32> = (0_i16..=80)
        .map(|s| model.evaluate(f32::from(s).mul_add(5.0, DispersionModel::BAND_MIN_NM)))
        .collect();
    assert_eq!((bottom, top), shared_index_range(&[samples]));
}

#[test]
fn series_count_follows_the_optic_character() {
    for material in GemMaterial::built_in() {
        let d = detail(&material.name);
        let expected = if material.biaxial_delta_beta_alpha.is_some() {
            3
        } else {
            match material.optical_character {
                OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => 2,
                _ => 1,
            }
        };
        assert_eq!(d.ri.series.len(), expected, "{}", material.name);
    }
    assert_eq!(detail("Diamond").ri.series.len(), 1);
    assert_eq!(detail("Sapphire").ri.series.len(), 2);
    assert_eq!(detail("Alexandrite").ri.series.len(), 3);
    assert_eq!(detail("Alexandrite").ri.guides.len(), 5);
}

#[test]
fn colors_equal_the_apps_own_color_code() {
    for material in GemMaterial::built_in() {
        let d = detail(&material.name);
        let face_up = d.colors.rows.last().expect("face-up row");
        assert!(face_up.face_up);
        let expected = face_up_swatch(
            &material.absorption,
            material.absorption_unit,
            7.0,
            material,
        );
        assert_eq!(face_up.daylight.srgb, expected.srgb, "{}", material.name);
        assert_eq!(d.header.swatch_daylight, expected.srgb, "{}", material.name);

        let path = face_up_swatch_path(material.absorption_unit, 7.0);
        let frame = material
            .absorption
            .is_pleochroic
            .then(|| PrincipalFrame::of_material(material));
        let lamp = face_up_body_color(
            &material.absorption,
            frame.as_ref(),
            path,
            Illuminant::Planckian(3200.0),
        );
        assert_eq!(face_up.incandescent.srgb, lamp.srgb, "{}", material.name);
        assert_eq!(d.header.swatch_incandescent, lamp.srgb, "{}", material.name);
    }
    // Per-direction rows are `body_colors`.
    let alexandrite = GemMaterial::by_name("Alexandrite").expect("Alexandrite");
    let path = face_up_swatch_path(alexandrite.absorption_unit, 7.0);
    let day = body_colors(&alexandrite.absorption, path, Illuminant::D65);
    let lamp = body_colors(&alexandrite.absorption, path, Illuminant::Planckian(3200.0));
    let rows = detail("Alexandrite").colors.rows;
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].daylight.srgb, day.o_ray.srgb);
    assert_eq!(rows[1].daylight.srgb, day.beta_ray.expect("beta").srgb);
    assert_eq!(rows[2].daylight.srgb, day.e_ray.srgb);
    assert_eq!(rows[1].incandescent.srgb, lamp.beta_ray.expect("beta").srgb);
    assert_eq!(rows[0].label, "\u{3b1} along c");
}

#[test]
fn spectrum_series_follow_the_tensor() {
    assert_eq!(detail("Alexandrite").spectrum.series.len(), 3);
    assert_eq!(detail("Sapphire").spectrum.series.len(), 2);
    let diamond = detail("Diamond");
    assert_eq!(diamond.spectrum.series.len(), 1);
    assert!(diamond.spectrum.series[0].bands.is_empty());
    let alexandrite = detail("Alexandrite");
    for series in &alexandrite.spectrum.series {
        let peaks: Vec<f32> = series.bands.iter().map(|b| b.peak).collect();
        assert!(peaks.windows(2).all(|w| w[0] >= w[1]), "sorted by peak");
        for path in [&series.absorption_path, &series.transmission_path] {
            assert!(points(path).iter().all(|(_, y)| (0.0..=100.0).contains(y)));
        }
    }
}

#[test]
fn variants_equal_the_built_in_list_and_effects_equal_the_catalogues() {
    for material in GemMaterial::built_in() {
        let d = detail(&material.name);
        let keys: Vec<String> = built_in_variants(&material.name)
            .into_iter()
            .map(|v| v.key)
            .collect();
        let shown: Vec<String> = d.variants.iter().map(|v| v.key.clone()).collect();
        assert_eq!(shown, keys, "{}", material.name);
        assert!(d.variants[0].selected);

        let presets: Vec<String> = presets_for(&material.name)
            .iter()
            .map(|p| p.name.to_owned())
            .collect();
        assert_eq!(d.effects.inclusion_presets, presets, "{}", material.name);
        let look = MaterialLook::of(&material.name);
        assert_eq!(d.effects.structure.is_some(), look.structure.is_some());
        assert_eq!(d.effects.sheen.as_deref(), look.inclusion_preset);
        assert_eq!(d.effects.haze_body.is_some(), look.haze_sigma_s.is_some());
        let zoning: Vec<&str> = typical_patterns(&material.name)
            .iter()
            .map(|p| p.label)
            .collect();
        assert_eq!(d.effects.typical_zoning, zoning, "{}", material.name);
    }
}

#[test]
fn selecting_a_variant_replaces_the_spectrum() {
    let variants = built_in_variants("Sapphire");
    assert!(variants.len() > 1);
    let key = &variants[1].key;
    let d = material_detail_for_variant(
        "Sapphire",
        &BuiltinMaterials,
        &DetailContext::default(),
        key,
    )
    .expect("detail");
    assert!(d.variants.iter().any(|v| v.selected && &v.key == key));
    assert_eq!(d.spectrum.variant_label, variants[1].label);
}

#[test]
fn weight_line_compares_with_diamond() {
    let diamond = detail("Diamond").weight;
    assert_eq!(diamond.vs_diamond_percent, Some(0));
    let zirconia = detail("Cubic Zirconia").weight;
    assert_eq!(zirconia.vs_diamond_percent, Some(65));
    assert!(zirconia.line.contains("65 % heavier"), "{}", zirconia.line);
    assert!(zirconia.design_line.is_none());

    let with_design = material_detail(
        "Diamond",
        &BuiltinMaterials,
        &DetailContext {
            stone_width_mm: 6.5,
            design_volume: Some(100.0),
            ..DetailContext::default()
        },
    )
    .expect("detail");
    // 100 mm^3 at SG 3.52 is 1.76 ct.
    assert_eq!(
        with_design.weight.design_line.as_deref(),
        Some("This design: about 1.76 ct at 6.5 mm")
    );
}

#[test]
fn every_built_in_has_a_complete_page() {
    for material in GemMaterial::built_in() {
        let d = detail(&material.name);
        assert!(!d.is_custom);
        assert_eq!(d.optics.rows.len(), 10, "{}", material.name);
        assert!(d.header.chips.len() >= 4, "{}", material.name);
        assert!(!d.durability.rows.is_empty());
        assert!(!d.sources.is_empty(), "{}", material.name);
        let text = format!("{d:?}").to_lowercase();
        assert!(!text.contains("fluoresc"), "{}", material.name);
    }
}
