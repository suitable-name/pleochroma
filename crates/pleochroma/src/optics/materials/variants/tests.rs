//! Tests of the color variant type, its validation and the built-in data file (written, not run,
//! by the variants lane).

use super::{
    ColorVariant, MATERIAL_DEFAULT_LABEL, MAX_VARIANT_BANDS, TensorRank, VariantBandRow,
    VariantBandShape, VariantProblem, VariantProvenance, VariantTensorRows, built_in_variants,
    data, face_up_swatch, face_up_swatch_path, host_rank, material_slug, tensor_rank,
};
use crate::{
    color::body_color::{Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
        materials::{AbsorptionUnit, GemMaterial},
    },
};
use glam::Vec3;

/// The built-in material named `name`, cloned out of the shared table.
fn built_in(name: &str) -> GemMaterial {
    GemMaterial::built_in()
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no built-in material {name}"))
        .clone()
}

/// A variant of `material` carrying `absorption`, valid in every respect but possibly the rank.
fn variant_of(material: &GemMaterial, absorption: AbsorptionTensor) -> ColorVariant {
    ColorVariant {
        key: format!("{}.test", material_slug(&material.name)),
        label: "Test".to_string(),
        material: material.name.clone(),
        absorption,
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
        provenance: VariantProvenance::User,
    }
}

fn band(center: f32, width: f32, peak: f32) -> AbsorptionBand {
    AbsorptionBand::new(center, width, peak)
}

/// `count` distinct valid wavelength bands, 10 nm apart.
fn bands(count: u8) -> Vec<AbsorptionBand> {
    (0..count)
        .map(|i| band(10.0f32.mul_add(f32::from(i), 450.0), 20.0, 0.1))
        .collect()
}

fn iso_tensor() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![band(540.0, 45.0, 0.1)])
}

fn uniaxial_tensor() -> AbsorptionTensor {
    AbsorptionTensor::uniaxial(
        vec![band(540.0, 45.0, 0.2)],
        vec![AbsorptionBand::energy(600.0, 800.0, 0.1)],
    )
}

fn biaxial_tensor() -> AbsorptionTensor {
    AbsorptionTensor::biaxial(
        vec![band(540.0, 45.0, 0.2)],
        vec![band(600.0, 30.0, 0.1)],
        vec![band(450.0, 40.0, 0.3)],
    )
}

fn assert_bits_equal(a: &[AbsorptionBand], b: &[AbsorptionBand], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: band count");
    for (x, y) in a.iter().zip(b) {
        assert_eq!(
            x.center_nm.to_bits(),
            y.center_nm.to_bits(),
            "{what}: centre"
        );
        assert_eq!(x.width_nm.to_bits(), y.width_nm.to_bits(), "{what}: width");
        assert_eq!(x.peak.to_bits(), y.peak.to_bits(), "{what}: peak");
        assert_eq!(x.shape, y.shape, "{what}: shape");
    }
}

#[test]
fn rows_round_trip_bit_exact_for_every_built_in_tensor() {
    for material in GemMaterial::built_in() {
        let tensor = &material.absorption;
        let rows = VariantTensorRows::from_tensor(tensor);
        let back = rows.to_tensor();
        let name = &material.name;
        assert_bits_equal(&tensor.o_ray, &back.o_ray, name);
        assert_bits_equal(&tensor.e_ray, &back.e_ray, name);
        match (&tensor.beta_ray, &back.beta_ray) {
            (Some(a), Some(b)) => assert_bits_equal(a, b, name),
            (None, None) => {}
            _ => panic!("{name}: beta_ray Some/None changed"),
        }
        assert_eq!(tensor.is_pleochroic, back.is_pleochroic, "{name}");
        assert_eq!(rows.pleochroic, tensor.is_pleochroic, "{name}");
        assert_eq!(&back, tensor, "{name}");
        // The rows then round-trip through themselves too.
        assert_eq!(VariantTensorRows::from_tensor(&back), rows, "{name}");
    }
}

#[test]
fn a_band_row_maps_both_shapes_and_every_field() {
    for (shape, row_shape) in [
        (BandShape::GaussianWavelength, VariantBandShape::Wavelength),
        (BandShape::GaussianEnergy, VariantBandShape::Energy),
    ] {
        let band = AbsorptionBand {
            center_nm: 512.5,
            width_nm: 33.25,
            peak: 0.75,
            shape,
        };
        let row = VariantBandRow::from_band(&band);
        assert_eq!(row.shape, row_shape);
        assert_eq!(row.to_band(), band);
    }
}

#[test]
fn rows_keep_a_pleochroic_tensor_with_equal_sets_pleochroic() {
    let bands = vec![band(540.0, 45.0, 0.1)];
    let tensor = AbsorptionTensor::uniaxial(bands.clone(), bands);
    let back = VariantTensorRows::from_tensor(&tensor).to_tensor();
    assert!(back.is_pleochroic);
    assert_eq!(back.beta_ray, None);
}

#[test]
fn with_color_variant_tensor_keeps_every_other_field() {
    for material in GemMaterial::built_in() {
        let replaced = material
            .clone()
            .with_color_variant_tensor(uniaxial_tensor(), AbsorptionUnit::PerMm);
        assert_eq!(replaced.absorption, uniaxial_tensor(), "{}", material.name);
        assert_eq!(replaced.absorption_unit, AbsorptionUnit::PerMm);
        assert!(replaced.zoning.is_none(), "{}", material.name);
        // Reset the four replaced fields: nothing else may differ.
        let mut restored = replaced;
        restored.absorption = material.absorption.clone();
        restored.absorption_unit = material.absorption_unit;
        restored.absorption_path_scale = material.absorption_path_scale;
        restored.zoning = material.zoning.clone();
        assert_eq!(&restored, material, "{}", material.name);
    }
}

#[test]
fn a_per_mm_variant_on_a_model_unit_host_gets_the_default_path_scale() {
    let sapphire = built_in("Sapphire");
    assert_eq!(sapphire.absorption_unit, AbsorptionUnit::ModelUnit);
    assert!((sapphire.absorption_path_scale - 1.0).abs() < f32::EPSILON);
    // The scale of a 7 mm stone (two model units wide): a consumer that traces without
    // `render_setup` choosing the scale still gets a sensible stone.
    let per_mm = sapphire
        .clone()
        .with_color_variant_tensor(uniaxial_tensor(), AbsorptionUnit::PerMm);
    assert!((per_mm.absorption_path_scale - 3.5).abs() < f32::EPSILON);
    // A per-model-unit variant leaves the host's own scale.
    let model_unit =
        sapphire.with_color_variant_tensor(uniaxial_tensor(), AbsorptionUnit::ModelUnit);
    assert!((model_unit.absorption_path_scale - 1.0).abs() < f32::EPSILON);
    // A host that is already per millimetre keeps the scale it chose.
    let mut chosen = per_mm;
    chosen.absorption_path_scale = 2.25;
    let again = chosen.with_color_variant_tensor(uniaxial_tensor(), AbsorptionUnit::PerMm);
    assert!((again.absorption_path_scale - 2.25).abs() < f32::EPSILON);
}

/// A variant swapped into a material that is already sized for its stone (a render job's
/// scene) shows at the same width: the scale `render_setup` gives the variant's unit for that
/// stone, not the 7 mm default.
#[test]
fn a_variant_on_a_sized_material_keeps_the_stone_width() {
    use crate::render_setup::absorption_path_scale_for;

    let (width_mm, model_width) = (9.0_f32, Some(2.2_f64));
    let ruby = built_in("Ruby");
    assert_eq!(ruby.absorption_unit, AbsorptionUnit::ModelUnit);
    let sized_scale = absorption_path_scale_for(AbsorptionUnit::ModelUnit, width_mm, model_width)
        .expect("a scale");
    let sized = ruby.with_absorption_path_scale(sized_scale);

    let thai = built_in_variants("Ruby")
        .into_iter()
        .find(|v| v.key == "ruby.thai")
        .expect("the Thai ruby variant");
    assert_eq!(thai.unit, AbsorptionUnit::PerMm);
    let swapped = sized
        .clone()
        .with_color_variant_resized(&thai, model_width)
        .expect("a width is recovered");
    let expected =
        absorption_path_scale_for(AbsorptionUnit::PerMm, width_mm, model_width).expect("a scale");
    assert!(
        (swapped.absorption_path_scale - expected).abs() < 1e-5 * expected,
        "{} against {expected}",
        swapped.absorption_path_scale
    );
    assert_eq!(swapped.absorption, thai.absorption);
    assert_eq!(swapped.absorption_unit, AbsorptionUnit::PerMm);

    // The same unit keeps the scale bit for bit.
    let default = built_in_variants("Ruby")
        .into_iter()
        .next()
        .expect("the Material Default");
    let same = sized
        .clone()
        .with_color_variant_resized(&default, model_width)
        .expect("same unit");
    assert_eq!(same.absorption_path_scale.to_bits(), sized_scale.to_bits());

    // A per-millimetre side needs the design's width.
    assert!(sized.with_color_variant_resized(&thai, None).is_none());
}

#[test]
fn a_swatch_is_the_oriented_face_up_color_of_the_hosts_default_axis() {
    let blue = built_in_variants("Zircon")
        .into_iter()
        .find(|v| v.key == "zircon.blue")
        .expect("zircon.blue");
    assert!(
        blue.absorption.is_pleochroic,
        "premise: Zircon Blue is dichroic"
    );
    let width = f64::from(blue.reference_path_mm);
    let path = face_up_swatch_path(blue.unit, blue.reference_path_mm);
    let colors = body_colors(&blue.absorption, path, Illuminant::D65);

    // c along the table normal: the face-up color is the ordinary ray's alone.
    let mut zircon = built_in("Zircon");
    zircon.c_axis = Vec3::Y;
    let along = face_up_swatch(&blue.absorption, blue.unit, width, &zircon);
    for (a, b) in along.lab.iter().zip(colors.o_ray.lab) {
        assert!(
            (a - b).abs() < 1e-3,
            "{:?} vs {:?}",
            along.lab,
            colors.o_ray.lab
        );
    }

    // c in the table plane: the two modes mix, which is not the ordinary color.
    zircon.c_axis = Vec3::X;
    let across = face_up_swatch(&blue.absorption, blue.unit, width, &zircon);
    assert_ne!(across.lab, along.lab);
}

#[test]
fn the_swatch_of_a_tensor_that_is_not_pleochroic_is_the_unpolarised_color() {
    let diamond = built_in("Diamond");
    let sapphire = built_in("Sapphire");
    for host in [&diamond, &sapphire] {
        for unit in [AbsorptionUnit::PerMm, AbsorptionUnit::ModelUnit] {
            let tensor = iso_tensor();
            let path = face_up_swatch_path(unit, 7.0);
            let unpolarised = body_colors(&tensor, path, Illuminant::D65).unpolarised;
            let swatch = face_up_swatch(&tensor, unit, 7.0, host);
            assert_eq!(swatch.lab, unpolarised.lab, "{} {unit:?}", host.name);
            assert_eq!(swatch.srgb, unpolarised.srgb, "{} {unit:?}", host.name);
        }
    }
}

#[test]
fn with_color_variant_drops_the_zones_of_a_zoned_material() {
    let ametrine = built_in("Ametrine");
    assert!(ametrine.zoning.is_some(), "premise: Ametrine is zoned");
    let variants = built_in_variants("Ametrine");
    let default = &variants[0];
    let colored = ametrine.with_color_variant(default);
    assert!(colored.zoning.is_none());
    assert_eq!(colored.absorption, default.absorption);
    assert_eq!(colored.absorption_unit, default.unit);
}

#[test]
fn a_pleochroic_variant_applied_keeps_is_pleochroic_and_its_sets() {
    let sapphire = built_in("Sapphire");
    for variant in built_in_variants("Sapphire") {
        // A colorless variant is an all-empty tensor stored as not pleochroic (owner decision
        // D-C): it has no pleochroism to keep.
        let colorless = variant.absorption.o_ray.is_empty()
            && variant.absorption.e_ray.is_empty()
            && variant
                .absorption
                .beta_ray
                .as_ref()
                .is_none_or(Vec::is_empty);
        assert_eq!(
            variant.absorption.is_pleochroic, !colorless,
            "{}",
            variant.key
        );
        let applied = sapphire.clone().with_color_variant(&variant);
        assert_eq!(
            applied.absorption.is_pleochroic, !colorless,
            "{}",
            variant.key
        );
        assert_eq!(applied.absorption, variant.absorption, "{}", variant.key);
        assert_eq!(applied.absorption_unit, variant.unit, "{}", variant.key);
        // The o and e sets stay different, except for a colorless variant (both empty).
        if !colorless {
            assert_ne!(
                applied.absorption.o_ray, applied.absorption.e_ray,
                "{}: the o and e sets stay different",
                variant.key
            );
        }
        // Optics untouched.
        assert_eq!(applied.dispersion, sapphire.dispersion);
        assert_eq!(applied.birefringence_delta, sapphire.birefringence_delta);
        assert_eq!(applied.c_axis, sapphire.c_axis);
    }
}

#[test]
fn built_in_variants_of_sapphire_start_with_the_material_default() {
    let variants = built_in_variants("Sapphire");
    let sapphire = built_in("Sapphire");
    let first = &variants[0];
    assert_eq!(first.key, "sapphire.default");
    assert_eq!(first.label, MATERIAL_DEFAULT_LABEL);
    assert_eq!(first.material, "Sapphire");
    assert_eq!(first.absorption, sapphire.absorption);
    assert_eq!(first.unit, sapphire.absorption_unit);
    assert_eq!(first.provenance, VariantProvenance::MaterialDefault);
    assert!((first.reference_path_mm - 7.0).abs() < f32::EPSILON);

    let keys: Vec<&str> = variants.iter().map(|v| v.key.as_str()).collect();
    assert!(keys.contains(&"sapphire.pink"), "{keys:?}");
    assert!(keys.contains(&"sapphire.yellow"), "{keys:?}");
    // File order: pink before yellow.
    let pink = keys.iter().position(|k| *k == "sapphire.pink");
    let yellow = keys.iter().position(|k| *k == "sapphire.yellow");
    assert!(pink < yellow);
    assert!(variants.iter().all(|v| v.material == "Sapphire"));
}

#[test]
fn the_name_match_ignores_ascii_case_and_unknown_names_are_empty() {
    assert_eq!(
        built_in_variants("sapphire"),
        built_in_variants("SAPPHIRE"),
        "case is ignored"
    );
    assert_eq!(built_in_variants("Sapphire"), built_in_variants("sapphire"));
    assert!(built_in_variants("Unobtainium").is_empty());
    assert!(built_in_variants("").is_empty());
}

#[test]
fn every_built_in_material_has_a_valid_material_default_first() {
    for material in GemMaterial::built_in() {
        let variants = built_in_variants(&material.name);
        let first = variants.first().expect("a material default");
        assert_eq!(first.provenance, VariantProvenance::MaterialDefault);
        assert_eq!(
            first.key,
            format!("{}.default", material_slug(&material.name))
        );
        assert_eq!(
            first.validate_for(material),
            Ok(()),
            "{}: its Material Default must validate against itself",
            material.name
        );
        // Applying the default is the identity on the material (a zoned one loses its zones).
        let mut applied = material.clone().with_color_variant(first);
        applied.zoning = material.zoning.clone();
        assert_eq!(&applied, material);
    }
}

#[test]
fn every_data_file_entry_validates_against_its_material() {
    let entries = data::file_entries();
    assert!(!entries.is_empty());
    let mut keys = std::collections::BTreeSet::new();
    for entry in entries {
        let host = built_in(&entry.material);
        assert_eq!(entry.validate_for(&host), Ok(()), "{}", entry.key);
        assert!(keys.insert(entry.key.clone()), "duplicate {}", entry.key);
        assert!(
            entry
                .key
                .starts_with(&format!("{}.", material_slug(&entry.material)))
        );
        let VariantProvenance::BuiltIn { sources, .. } = &entry.provenance else {
            panic!("{}: a data entry is BuiltIn", entry.key);
        };
        assert!(!sources.is_empty(), "{}", entry.key);
        // The entry is listed under its material, after the default.
        let listed = built_in_variants(&entry.material);
        assert!(listed.iter().skip(1).any(|v| v == entry), "{}", entry.key);
    }
}

#[test]
fn every_built_in_variant_has_at_most_the_gpu_band_budget_per_ray() {
    assert_eq!(
        MAX_VARIANT_BANDS,
        crate::renderer::buffers::MAX_ABSORPTION_BANDS
    );
    for material in GemMaterial::built_in() {
        for variant in built_in_variants(&material.name) {
            let t = &variant.absorption;
            assert!(t.o_ray.len() <= MAX_VARIANT_BANDS, "{}", variant.key);
            assert!(t.e_ray.len() <= MAX_VARIANT_BANDS, "{}", variant.key);
            if let Some(beta) = &t.beta_ray {
                assert!(beta.len() <= MAX_VARIANT_BANDS, "{}", variant.key);
            }
        }
    }
}

#[test]
fn the_face_up_swatch_path_matches_the_render_scale_per_unit() {
    use super::face_up_swatch_path;
    use crate::render_setup::{MODEL_UNIT_FACE_UP_PATH, REFERENCE_MODEL_WIDTH};
    assert!((face_up_swatch_path(AbsorptionUnit::ModelUnit, 7.0) - 1.0).abs() < 1e-12);
    assert!((face_up_swatch_path(AbsorptionUnit::ModelUnit, 0.0) - 1.0).abs() < 1e-12);
    assert!((face_up_swatch_path(AbsorptionUnit::ModelUnit, 14.0) - 2.0).abs() < 1e-12);
    let per_mm_7 = f64::from(MODEL_UNIT_FACE_UP_PATH) * 7.0 / REFERENCE_MODEL_WIDTH;
    assert!((face_up_swatch_path(AbsorptionUnit::PerMm, 7.0) - per_mm_7).abs() < 1e-9);
    assert!((face_up_swatch_path(AbsorptionUnit::PerMm, 3.5) - per_mm_7 / 2.0).abs() < 1e-9);
    // One per-model-unit coefficient over one unit equals the same light as its per-mm
    // equivalent (x REFERENCE_MODEL_WIDTH / (7 mm x MODEL_UNIT_FACE_UP_PATH)) over the per-mm path.
    let per_mm_factor = REFERENCE_MODEL_WIDTH / (7.0 * f64::from(MODEL_UNIT_FACE_UP_PATH));
    let model_unit_depth = face_up_swatch_path(AbsorptionUnit::ModelUnit, 9.0);
    let per_mm_depth = per_mm_factor * face_up_swatch_path(AbsorptionUnit::PerMm, 9.0);
    assert!((model_unit_depth - per_mm_depth).abs() < 1e-9);
}

#[test]
fn the_rank_rule_follows_the_host() {
    let sapphire = built_in("Sapphire"); // uniaxial, pleochroic tensor
    let diamond = built_in("Diamond"); // isotropic, colorless tensor
    assert_eq!(host_rank(&sapphire), TensorRank::Uniaxial);
    assert_eq!(host_rank(&diamond), TensorRank::Isotropic);

    // A pleochroic uniaxial host takes a uniaxial variant and refuses the other ranks.
    assert_eq!(
        variant_of(&sapphire, uniaxial_tensor()).validate_for(&sapphire),
        Ok(())
    );
    assert_eq!(
        variant_of(&sapphire, iso_tensor()).validate_for(&sapphire),
        Err(VariantProblem::RankMismatch {
            host: TensorRank::Uniaxial,
            variant: TensorRank::Isotropic
        })
    );
    assert_eq!(
        variant_of(&sapphire, biaxial_tensor()).validate_for(&sapphire),
        Err(VariantProblem::RankMismatch {
            host: TensorRank::Uniaxial,
            variant: TensorRank::Biaxial
        })
    );

    // An isotropic host takes only isotropic variants.
    assert_eq!(
        variant_of(&diamond, iso_tensor()).validate_for(&diamond),
        Ok(())
    );
    for tensor in [uniaxial_tensor(), biaxial_tensor()] {
        assert!(matches!(
            variant_of(&diamond, tensor).validate_for(&diamond),
            Err(VariantProblem::RankMismatch {
                host: TensorRank::Isotropic,
                ..
            })
        ));
    }

    // A uniaxial host whose OWN stored tensor is isotropic still refuses an isotropic variant:
    // only its Material Default (its own tensor) may keep that rank.
    let mut flat = sapphire.clone();
    flat.absorption = iso_tensor();
    assert_eq!(
        variant_of(&flat, iso_tensor()).validate_for(&flat),
        Err(VariantProblem::RankMismatch {
            host: TensorRank::Uniaxial,
            variant: TensorRank::Isotropic
        })
    );
    let own_default = ColorVariant {
        provenance: VariantProvenance::MaterialDefault,
        ..variant_of(&flat, iso_tensor())
    };
    assert_eq!(own_default.validate_for(&flat), Ok(()));
    assert_eq!(
        variant_of(&flat, uniaxial_tensor()).validate_for(&flat),
        Ok(())
    );

    // A pleochroic variant with equal o and e sets is still uniaxial and fits a uniaxial host.
    let equal =
        AbsorptionTensor::uniaxial(vec![band(540.0, 45.0, 0.1)], vec![band(540.0, 45.0, 0.1)]);
    assert_eq!(variant_of(&sapphire, equal).validate_for(&sapphire), Ok(()));
}

#[test]
fn an_all_empty_tensor_fits_every_host_in_any_rank() {
    // No band in any ray: no pleochroism to lose, so no `pleochroic = true` trick is needed.
    for name in ["Sapphire", "Tanzanite", "Diamond", "Quartz"] {
        let host = built_in(name);
        for tensor in [
            AbsorptionTensor::isotropic(vec![]),
            AbsorptionTensor::uniaxial(vec![], vec![]),
            AbsorptionTensor::biaxial(vec![], vec![], vec![]),
        ] {
            assert_eq!(
                variant_of(&host, tensor).validate_for(&host),
                Ok(()),
                "{name}"
            );
        }
    }
    // A colored tensor of the wrong rank is still refused.
    let sapphire = built_in("Sapphire");
    assert!(
        variant_of(&sapphire, iso_tensor())
            .validate_for(&sapphire)
            .is_err()
    );
}

#[test]
fn the_material_default_exception_belongs_to_its_own_material() {
    let mut flat = built_in("Sapphire");
    flat.absorption = iso_tensor();
    let own = ColorVariant {
        provenance: VariantProvenance::MaterialDefault,
        ..variant_of(&flat, iso_tensor())
    };
    assert_eq!(own.validate_for(&flat), Ok(()));
    // The host's name is compared with the ASCII case ignored.
    let cased = ColorVariant {
        material: flat.name.to_ascii_uppercase(),
        ..own.clone()
    };
    assert_eq!(cased.validate_for(&flat), Ok(()));
    // The Material Default of another material does not pass on this host's stored rank.
    let foreign = ColorVariant {
        material: "Ruby".to_string(),
        ..own
    };
    assert_eq!(
        foreign.validate_for(&flat),
        Err(VariantProblem::RankMismatch {
            host: TensorRank::Uniaxial,
            variant: TensorRank::Isotropic
        })
    );
}

#[test]
fn a_biaxial_host_takes_biaxial_and_uniaxial_only_without_its_own_beta_set() {
    let alexandrite = built_in("Alexandrite");
    assert_eq!(host_rank(&alexandrite), TensorRank::Biaxial);
    assert!(
        alexandrite.absorption.beta_ray.is_some(),
        "premise: Alexandrite has a beta set"
    );
    assert_eq!(
        variant_of(&alexandrite, biaxial_tensor()).validate_for(&alexandrite),
        Ok(())
    );
    assert_eq!(
        variant_of(&alexandrite, uniaxial_tensor()).validate_for(&alexandrite),
        Err(VariantProblem::RankMismatch {
            host: TensorRank::Biaxial,
            variant: TensorRank::Uniaxial
        })
    );
    assert!(
        variant_of(&alexandrite, iso_tensor())
            .validate_for(&alexandrite)
            .is_err()
    );

    // The same host with only two stored sets accepts a uniaxial variant.
    let mut two_sets = alexandrite;
    two_sets.absorption = uniaxial_tensor();
    assert_eq!(
        variant_of(&two_sets, uniaxial_tensor()).validate_for(&two_sets),
        Ok(())
    );
    assert_eq!(
        variant_of(&two_sets, biaxial_tensor()).validate_for(&two_sets),
        Ok(())
    );
}

#[test]
fn tensor_rank_reads_the_flags() {
    assert_eq!(tensor_rank(&iso_tensor()), TensorRank::Isotropic);
    assert_eq!(tensor_rank(&uniaxial_tensor()), TensorRank::Uniaxial);
    assert_eq!(tensor_rank(&biaxial_tensor()), TensorRank::Biaxial);
    // A beta set on a tensor that is not pleochroic is still isotropic.
    let mut odd = biaxial_tensor();
    odd.is_pleochroic = false;
    assert_eq!(tensor_rank(&odd), TensorRank::Isotropic);
}

#[test]
fn validation_reports_each_problem() {
    let host = built_in("Sapphire");
    let check = |tensor: AbsorptionTensor| variant_of(&host, tensor).validate_for(&host);
    let with_o = |bands: Vec<AbsorptionBand>| {
        AbsorptionTensor::uniaxial(bands, vec![band(500.0, 20.0, 0.1)])
    };

    assert_eq!(check(uniaxial_tensor()), Ok(()));
    for bad in [f32::NAN, f32::INFINITY] {
        assert_eq!(
            check(with_o(vec![band(bad, 20.0, 0.1)])),
            Err(VariantProblem::NonFinite)
        );
        assert_eq!(
            check(with_o(vec![band(500.0, bad, 0.1)])),
            Err(VariantProblem::NonFinite)
        );
        assert_eq!(
            check(with_o(vec![band(500.0, 20.0, bad)])),
            Err(VariantProblem::NonFinite)
        );
    }
    for bad in [
        band(500.0, 20.0, -0.1),
        band(0.0, 20.0, 0.1),
        band(-5.0, 20.0, 0.1),
        band(500.0, 0.0, 0.1),
        band(500.0, -3.0, 0.1),
    ] {
        assert_eq!(check(with_o(vec![bad])), Err(VariantProblem::Negative));
    }

    let nine = bands(9);
    assert_eq!(
        check(with_o(nine.clone())),
        Err(VariantProblem::TooManyBands {
            ray: "o_ray",
            count: 9
        })
    );
    assert_eq!(
        check(AbsorptionTensor::uniaxial(
            vec![band(500.0, 20.0, 0.1)],
            nine
        )),
        Err(VariantProblem::TooManyBands {
            ray: "e_ray",
            count: 9
        })
    );
    assert_eq!(
        check(with_o(bands(
            u8::try_from(MAX_VARIANT_BANDS).expect("the band budget fits a u8")
        ))),
        Ok(())
    );

    // A half-defined tensor is Empty; an all-empty (colorless) pleochroic one is valid.
    assert_eq!(
        check(AbsorptionTensor::uniaxial(
            vec![band(500.0, 20.0, 0.1)],
            vec![]
        )),
        Err(VariantProblem::Empty)
    );
    assert_eq!(
        check(AbsorptionTensor::uniaxial(vec![], vec![])),
        Ok(()),
        "a colorless pleochroic variant is valid"
    );

    // The reference path is checked too.
    let mut v = variant_of(&host, uniaxial_tensor());
    v.reference_path_mm = 0.0;
    assert_eq!(v.validate_for(&host), Err(VariantProblem::Negative));
    v.reference_path_mm = f32::NAN;
    assert_eq!(v.validate_for(&host), Err(VariantProblem::NonFinite));
}

#[test]
fn the_slug_lowercases_and_replaces_every_non_alphanumeric() {
    assert_eq!(material_slug("Sapphire"), "sapphire");
    assert_eq!(
        material_slug("Chrysoberyl (Yellow)"),
        "chrysoberyl__yellow_"
    );
    assert_eq!(
        material_slug("Precious Opal (White)"),
        "precious_opal__white_"
    );
    assert_eq!(
        material_slug("Synthetic Moissanite"),
        "synthetic_moissanite"
    );
}

#[test]
fn the_pink_entry_is_a_measured_cr3_tensor_much_weaker_than_the_ruby_default() {
    // The measured pink entry (GIA Cr3+ cross sections at 100 ppma*cm) replaced the estimated seed
    // that was the Ruby tensor at one fifth of its peaks.
    let ruby = built_in("Ruby");
    let pink = built_in_variants("Sapphire")
        .into_iter()
        .find(|v| v.key == "sapphire.pink")
        .expect("sapphire.pink");
    assert_eq!(pink.unit, AbsorptionUnit::PerMm);
    assert_eq!(pink.label, "Pink");
    let VariantProvenance::BuiltIn { estimated, .. } = pink.provenance else {
        panic!("a data entry is BuiltIn");
    };
    assert!(
        !estimated,
        "pink is fitted to the measured Cr3+ cross sections"
    );
    // Face-up optical depth in the Cr3+ U band (560 nm): the per-mm tensor over the 9.905 mm
    // face-up path against the Ruby default's model-unit peak sum (one unit at 7 mm). The default
    // carries Cr 400 ppma*cm since 2026-11-03 (it was 800), so pink's 100 is about a quarter.
    let depth = |bands: &[AbsorptionBand], scale: f64| -> f64 {
        bands
            .iter()
            .map(|b| f64::from(b.evaluate(560.0)))
            .sum::<f64>()
            * scale
    };
    let path = face_up_swatch_path(AbsorptionUnit::PerMm, 7.0);
    let pink_o = depth(&pink.absorption.o_ray, path);
    let pink_e = depth(&pink.absorption.e_ray, path);
    let ruby_o = depth(&ruby.absorption.o_ray, 1.0);
    assert!(
        pink_o > pink_e,
        "the ordinary ray absorbs more: {pink_o} vs {pink_e}"
    );
    assert!(
        (0.05..0.3).contains(&(pink_o / ruby_o)),
        "pink is about a quarter of the ruby default ({pink_o} vs {ruby_o})"
    );
}

#[test]
fn the_yellow_entry_is_a_strong_light_yellow_at_the_7_mm_face_up_path() {
    // The measured yellow entry (GIA h.-Fe3+ pair cross sections at 1.7 ppma*cm) replaced the
    // estimated Fe3+ seed whose strongest visible band had an optical density of 0.3.
    let yellow = built_in_variants("Sapphire")
        .into_iter()
        .find(|v| v.key == "sapphire.yellow")
        .expect("sapphire.yellow");
    assert_eq!(yellow.unit, AbsorptionUnit::PerMm);
    assert!((yellow.reference_path_mm - 7.0).abs() < f32::EPSILON);
    let path = face_up_swatch_path(yellow.unit, yellow.reference_path_mm);
    let lab = crate::color::body_color::body_colors(
        &yellow.absorption,
        path,
        crate::color::body_color::Illuminant::D65,
    )
    .unpolarised
    .lab;
    let chroma = lab[1].hypot(lab[2]);
    assert!(lab[0] > 85.0, "light: {lab:?}");
    assert!(lab[2] > 45.0 && chroma > 45.0, "strongly yellow: {lab:?}");
}

#[test]
fn malformed_data_is_rejected_with_a_message() {
    let entry = |material: &str, key: &str| {
        format!(
            "data_version = 1\n[[variant]]\nmaterial = \"{material}\"\nkey = \"{key}\"\n\
             label = \"X\"\nunit = \"per_mm\"\nreference_path_mm = 7.0\nestimated = true\n\
             sources = [\"s\"]\n[variant.absorption]\npleochroic = true\n\
             o_ray = [{{ center_nm = 500.0, width_nm = 30.0, peak = 0.1, shape = \"wavelength\" }}]\n\
             e_ray = [{{ center_nm = 500.0, width_nm = 30.0, peak = 0.2, shape = \"wavelength\" }}]\n"
        )
    };
    assert!(data::parse(&entry("Sapphire", "sapphire.ok")).is_ok());
    assert!(data::parse(&entry("Unobtainium", "unobtainium.x")).is_err());
    assert!(
        data::parse(&entry("Sapphire", "ruby.x")).is_err(),
        "wrong slug"
    );
    assert!(
        data::parse(&entry("Sapphire", "sapphire.default")).is_err(),
        "reserved"
    );
    assert!(
        data::parse(&entry("Sapphire", "sapphire.")).is_err(),
        "empty name"
    );
    // An isotropic host refuses a pleochroic entry.
    assert!(data::parse(&entry("Diamond", "diamond.x")).is_err());
    // Duplicate keys.
    let twice = format!(
        "{}{}",
        entry("Sapphire", "sapphire.dup"),
        entry("Sapphire", "sapphire.dup").replace("data_version = 1\n", "")
    );
    assert!(data::parse(&twice).is_err());
    // A wrong data version and an unknown shape.
    assert!(
        data::parse(
            &entry("Sapphire", "sapphire.v").replace("data_version = 1", "data_version = 2")
        )
        .is_err()
    );
    assert!(
        data::parse(&entry("Sapphire", "sapphire.s").replace("wavelength", "lorentzian")).is_err()
    );
    // An empty file is valid and has no entries.
    assert_eq!(data::parse("data_version = 1\n").map(|v| v.len()), Ok(0));
    // Keys are unique across the group files too, and the error names the second file.
    let one = entry("Sapphire", "sapphire.across");
    assert_eq!(
        data::parse_all(&[("a.toml", one.as_str()), ("b.toml", "data_version = 1\n")])
            .map(|v| v.len()),
        Ok(1)
    );
    let error = data::parse_all(&[("a.toml", one.as_str()), ("b.toml", one.as_str())])
        .expect_err("a key in two files");
    assert!(error.starts_with("b.toml: sapphire.across"), "{error}");
}
