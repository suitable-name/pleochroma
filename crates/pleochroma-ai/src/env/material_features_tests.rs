use super::*;
use glam::Vec3;
use pleochroma::optics::absorption::AbsorptionTensor;

#[test]
fn spectra_keep_narrow_bands_between_the_old_three_probe_wavelengths() {
    let mut material = GemMaterial::diamond();
    let empty = MaterialFeatures::capture(&material).expect("valid clear material");
    material.absorption = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(555.0, 0.25, 2.0)]);
    for wavelength in [450.0, 550.0, 650.0] {
        assert_eq!(
            super::super::features::absorption_summary(&material, wavelength),
            0.0
        );
    }
    let features = MaterialFeatures::capture(&material).expect("valid narrow band");
    assert_ne!(features, empty);
    let offset = MATERIAL_HEADER_DIM + (555 - 380);
    assert_eq!(features.values[offset], 2.0);
    assert_eq!(features.to_feature_vector()[offset], 2.0_f32.ln_1p());
}

#[test]
fn every_tone_wavelength_and_principal_absorption_slot_is_retained() {
    let mut material = GemMaterial::sapphire();
    material.absorption = AbsorptionTensor::biaxial(
        vec![AbsorptionBand::new(420.0, 11.0, 0.7)],
        vec![AbsorptionBand::new(550.0, 27.0, 1.4)],
        vec![AbsorptionBand::new(690.0, 62.0, 2.1)],
    );
    let features = MaterialFeatures::capture(&material).expect("valid three-axis absorption");
    for (offset, wavelength) in (380_u16..=780).enumerate() {
        let expected = pleochroma::color::principal_axes::principal_alphas(
            &material.absorption,
            f64::from(wavelength),
        );
        for (axis, alpha) in expected.into_iter().enumerate() {
            let actual =
                features.values[MATERIAL_HEADER_DIM + axis * MATERIAL_SPECTRUM_SAMPLES + offset];
            assert_eq!(f64::from(actual), alpha);
        }
    }
    assert_eq!(features.values.len(), MATERIAL_FEATURE_DIM);
    assert_eq!(features.values[7], 1.0);
}

#[test]
fn permuting_absorption_axes_changes_features_even_with_the_same_average() {
    let mut a = GemMaterial::sapphire();
    a.absorption = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::new(570.0, 40.0, 0.8)],
        vec![AbsorptionBand::new(440.0, 30.0, 1.3)],
    );
    let mut b = a.clone();
    std::mem::swap(&mut b.absorption.o_ray, &mut b.absorption.e_ray);
    for wavelength in [450.0, 550.0, 650.0] {
        assert_eq!(
            super::super::features::absorption_summary(&a, wavelength),
            super::super::features::absorption_summary(&b, wavelength)
        );
    }
    assert_ne!(
        MaterialFeatures::capture(&a).unwrap(),
        MaterialFeatures::capture(&b).unwrap()
    );
}

#[test]
fn principal_dispersion_and_both_crystal_axes_are_observed() {
    let mut material = GemMaterial::built_in()
        .iter()
        .find(|material| material.name == "Tanzanite")
        .expect("catalog includes Tanzanite")
        .clone();
    let before = MaterialFeatures::capture(&material).unwrap();
    material.c_axis = Vec3::Z;
    material.alpha_axis = Some(Vec3::Y);
    let after = MaterialFeatures::capture(&material).unwrap();
    assert_ne!(
        before.values[FRAME_START..INDICES_START],
        after.values[FRAME_START..INDICES_START]
    );
    for (slot, wavelength) in METRIC_LINES_NM.into_iter().enumerate() {
        let principal = material.biaxial_indicatrix(wavelength).expect("biaxial");
        let start = INDICES_START + 3 * slot;
        assert_eq!(
            after.values[start..start + 3],
            [principal.n_alpha, principal.n_beta, principal.n_gamma]
        );
    }
}

#[test]
fn catalog_materials_produce_finite_features_and_share_storage_on_clone() {
    for material in GemMaterial::built_in() {
        let features = MaterialFeatures::capture(material)
            .unwrap_or_else(|reason| panic!("{}: {reason}", material.name));
        assert!(
            features
                .to_feature_vector()
                .iter()
                .all(|value| value.is_finite())
        );
        assert!(Arc::ptr_eq(&features.values, &features.clone().values));
    }
}

#[test]
fn malformed_samples_do_not_silently_become_zero_or_alias_a_valid_material() {
    let mut material = GemMaterial::sapphire();
    material.absorption.o_ray[0].peak = f32::NAN;
    assert!(MaterialFeatures::capture(&material).is_err());
    let malformed = MaterialFeatures {
        values: vec![0.0].into(),
    };
    assert!(malformed.validate().is_err());
    let mut values = MaterialFeatures::default().values.to_vec();
    values[MATERIAL_HEADER_DIM] = f32::INFINITY;
    assert!(
        MaterialFeatures {
            values: values.into()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn deserialization_rejects_malformed_width_and_negative_absorption() {
    assert!(serde_json::from_str::<MaterialFeatures>(r#"{"values":[0.0]}"#).is_err());
    let mut values = MaterialFeatures::default().values.to_vec();
    values[MATERIAL_HEADER_DIM] = -0.1;
    let text = serde_json::to_string(&serde_json::json!({ "values": values })).unwrap();
    assert!(serde_json::from_str::<MaterialFeatures>(&text).is_err());
    let valid = MaterialFeatures::capture(&GemMaterial::sapphire()).unwrap();
    let text = serde_json::to_string(&valid).unwrap();
    assert_eq!(
        serde_json::from_str::<MaterialFeatures>(&text).unwrap(),
        valid
    );
}

#[test]
fn design_axis_and_physical_size_reach_the_same_resolved_material() {
    let mut design = Design::concave_fixture();
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 90.0;
    design.render_effects.c_axis_azimuth_deg = 0.0;
    design.girdle_diameter_mm = Some(7.0);
    let gem = GemMaterial::sapphire();
    let one = material_for_design(&design, &gem, Some(2.0));
    let orientation = pleochroma_editor::stone_orientation::oriented_for_effects(
        gem.clone(),
        &design.render_effects,
    );
    assert_eq!(one.c_axis, orientation.c_axis);
    design.girdle_diameter_mm = Some(14.0);
    let twice = material_for_design(&design, &gem, Some(2.0));
    assert_eq!(twice.absorption_path_scale, 2.0 * one.absorption_path_scale);
    let same_physical = material_for_design(&design, &gem, Some(4.0));
    assert_eq!(
        same_physical.absorption_path_scale,
        one.absorption_path_scale
    );
    assert_ne!(
        MaterialFeatures::capture(&one).unwrap(),
        MaterialFeatures::capture(&twice).unwrap()
    );
}

#[test]
fn render_only_effects_have_explicit_scope_notes() {
    use pleochroma_cut_core::design::{SurfaceFinish, SurfaceGrade, TierSurface};

    let mut design = Design::concave_fixture();
    design.render_effects.inclusion_sigma_s = 0.5;
    design.render_effects.girdle_frosted = true;
    design.tier_surfaces.insert(
        design.tier_ids[0],
        TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Fine)),
    );
    let gem = GemMaterial::built_in()
        .iter()
        .find(|material| material.name == "Ametrine")
        .expect("catalog includes Ametrine");
    let notes = material_scope_notes(&design, gem);
    assert!(notes.iter().any(|note| note.contains("zoning")));
    assert!(notes.iter().any(|note| note.contains("scattering")));
    assert!(
        notes
            .iter()
            .any(|note| note.contains("surface polish grades"))
    );
    assert!(
        !notes
            .iter()
            .any(|note| note.contains("including the girdle"))
    );
}

#[test]
fn environment_observations_share_prepared_episode_spectra() {
    use crate::env::{OptimizerEnv, OptimizerEnvConfig};

    let mut design = Design::concave_fixture();
    design.girdle_diameter_mm = Some(12.0);
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 45.0;
    let source = super::super::test_fixtures::sapphire();
    let env = OptimizerEnv::new(
        design,
        OptimizerEnvConfig {
            source_material_override: Some(source),
            ..OptimizerEnvConfig::default()
        },
    )
    .expect("valid sized, oriented sapphire");
    let a = env.build_observation();
    let b = env.build_observation();
    assert!(Arc::ptr_eq(
        &a.global.source_material.values,
        &b.global.source_material.values
    ));
    assert!(Arc::ptr_eq(
        &a.global.source_material.values,
        &a.global.target_material.values
    ));
    assert_ne!(
        a.global.source_material,
        MaterialFeatures::capture(&env.source_material().gem).unwrap()
    );
}

#[test]
fn scoring_uses_the_sized_material_and_authored_frosted_finishes() {
    use crate::env::common::{evaluate_design, preform_volume, to_gpu_planes};
    use pleochroma::{
        color::metrics::FinishedStone,
        geometry::StoneGeometry,
        optics::raytracer::{FacetFinish, LightingPreset},
    };
    use pleochroma_cut_core::{
        ObjectiveWeights,
        design::TierFrosting,
        optimize::{ObjectiveFidelity, evaluate_objective_with_tone_finished_under_threaded},
    };

    let mut design = Design::concave_fixture();
    design.girdle_diameter_mm = Some(13.0);
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 65.0;
    design.render_effects.c_axis_azimuth_deg = 20.0;
    design.render_effects.girdle_frosted = true;
    design
        .tier_frosting
        .insert(design.tier_ids[0], TierFrosting::Tier);
    let gem = GemMaterial::sapphire();
    let evaluated = evaluate_design(
        &design,
        &gem,
        &ObjectiveWeights::default(),
        ObjectiveFidelity::Fast,
        preform_volume(&design),
        LightingPreset::IsoHemisphere,
        None,
    )
    .expect("closed design");
    let solved = design.solve().unwrap();
    let planes = design.planes_from_solved(&solved);
    let tools = design.tools_for_scoring(&planes);
    let gpu = to_gpu_planes(&planes);
    let finishes = design.facet_finishes_from_solved(&solved, &gpu);
    assert!(finishes.contains(&FacetFinish::Frosted));
    let prepared = material_for_design(&design, &gem, evaluated.width_axis);
    let (expected, tone) = evaluate_objective_with_tone_finished_under_threaded(
        FinishedStone::new(
            StoneGeometry {
                planes: &gpu,
                tools: &tools,
            },
            &finishes,
        ),
        &prepared,
        ObjectiveFidelity::Fast,
        LightingPreset::IsoHemisphere,
        Some(1),
    );
    assert_eq!(evaluated.components, expected);
    assert_eq!(evaluated.tone, tone);
}
