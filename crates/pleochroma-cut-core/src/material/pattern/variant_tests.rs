//! Tests of the variant and three-color zone colors, the per-millimeter rule, the format version
//! and the rank rule (written, not run, by lane P1).

use super::{
    ColorForm, ColorPattern, ColorRef, ColorSlot, PATTERN_BASE_FORMAT_VERSION,
    PATTERN_FORMAT_VERSION, PatternAxis, PatternCodecError, PatternColor, PatternError,
    PatternTensorRows, TemplateId, ZoneColor, ZoneProblem, ZoneRankFit, allowed_rank,
    apply_color_pattern, bands_from_rows, decode_pattern, encode_pattern, per_mm_factor,
    per_mm_rows, template, zone_rank_fit,
};
use crate::material::built_in_material_by_exact_name;
use glam::DVec3;
use pleochroma::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{
                ColorVariant, TensorRank, VariantBandRow, VariantProblem, built_in_variants,
            },
        },
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    render_setup::absorption_path_scale_for,
};

const ROWS: [[f32; 3]; 2] = [[480.0, 2100.0, 0.22], [560.0, 1500.0, 0.11]];
const OTHER_ROWS: [[f32; 3]; 2] = [[430.0, 2600.0, 0.4], [520.0, 1800.0, 0.07]];
const THIRD_ROWS: [[f32; 3]; 1] = [[640.0, 44.0, 0.31]];

/// A pattern stored by the first release (format version 1): a one-color base and a two-color
/// zone, no new color kind.
const STORED_V1: &str = r#"{"version":1,"template":null,"base":{"name":"Body","shape":{"HalfSpace":{"normal":[0.0,1.0,0.0],"offset":0.0}},"color":{"Single":{"lch":[50.0,40.0,310.0],"bands":[[480.0,2100.0,0.22]],"solved":true}}},"zones":[{"name":"Core","shape":{"HalfSpace":{"normal":[1.0,0.0,0.0],"offset":0.0}},"color":{"Dichroic":{"along":{"lch":[50.0,40.0,310.0],"bands":[[480.0,2100.0,0.22]],"solved":true},"across":{"lch":[60.0,30.0,90.0],"bands":[[430.0,2600.0,0.4]],"solved":true}}}}],"softness":0.0,"axis":{"Fixed":{"tilt_rad":0.0,"turn_rad":0.0}},"relative_to_stone":true}"#;

fn filled(mut pattern: ColorPattern) -> ColorPattern {
    for at in pattern.unsolved_colors() {
        assert!(pattern.set_color_bands(at, ROWS.to_vec()));
    }
    pattern
}

fn solved(rows: &[[f32; 3]]) -> ZoneColor {
    ZoneColor::solved([50.0, 30.0, 100.0], rows.to_vec())
}

fn trichroic() -> PatternColor {
    PatternColor::Trichroic {
        alpha: solved(&ROWS),
        beta: solved(&OTHER_ROWS),
        gamma: solved(&THIRD_ROWS),
    }
}

/// The built-in variant of Sapphire with `key`.
fn sapphire(key: &str) -> ColorVariant {
    built_in_variants("Sapphire")
        .into_iter()
        .find(|variant| variant.key == key)
        .unwrap_or_else(|| panic!("the data file lists {key}"))
}

fn rows_of(color: &PatternColor) -> &PatternTensorRows {
    match color {
        PatternColor::Variant { absorption, .. } => absorption,
        other => panic!("not a variant color: {other:?}"),
    }
}

fn assert_rows_bit_equal(a: &[VariantBandRow], b: &[VariantBandRow]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.center_nm.to_bits(), y.center_nm.to_bits());
        assert_eq!(x.width_nm.to_bits(), y.width_nm.to_bits());
        assert_eq!(x.peak.to_bits(), y.peak.to_bits());
        assert_eq!(x.shape, y.shape);
    }
}

// ---- format version ------------------------------------------------------------------------

#[test]
fn a_stored_version_one_pattern_still_loads_applies_and_is_written_back_as_version_one() {
    let pattern = decode_pattern(STORED_V1).expect("a version 1 file reads");
    assert_eq!(pattern.version, 1);
    assert_eq!(pattern.zones.len(), 1);
    assert!(matches!(
        pattern.zones[0].color,
        PatternColor::Dichroic { .. }
    ));

    let material = apply_color_pattern(GemMaterial::sapphire(), &pattern, 7.0)
        .expect("apply accepts every version up to the current one");
    let zoning = material.zoning.expect("the pattern has a zone");
    assert_eq!(zoning.zones.len(), 1);
    assert!(zoning.zones[0].absorption.tensor.is_pleochroic);
    assert_eq!(zoning.base.tensor.o_ray, bands_from_rows(&[ROWS[0]]));

    // Nothing in it needs the new format, so it is written back exactly as a version 1 file.
    let text = encode_pattern(&pattern).expect("encodes");
    assert!(text.starts_with(r#"{"version":1,"#), "{text}");
    assert!(
        !text.contains("Variant") && !text.contains("Trichroic"),
        "{text}"
    );
    assert_eq!(decode_pattern(&text).expect("decodes"), pattern);
}

#[test]
fn apply_accepts_every_version_up_to_the_current_and_refuses_the_rest() {
    let mut pattern = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    assert_eq!(pattern.version, PATTERN_BASE_FORMAT_VERSION);
    for version in PATTERN_BASE_FORMAT_VERSION..=PATTERN_FORMAT_VERSION {
        pattern.version = version;
        assert!(
            apply_color_pattern(GemMaterial::sapphire(), &pattern, 7.0).is_ok(),
            "version {version}"
        );
    }
    for version in [0, PATTERN_FORMAT_VERSION + 1] {
        pattern.version = version;
        assert_eq!(
            apply_color_pattern(GemMaterial::sapphire(), &pattern, 7.0),
            Err(PatternError::UnsupportedVersion(version))
        );
    }
}

#[test]
fn only_a_pattern_with_a_new_color_kind_is_written_as_version_two() {
    let plain = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    assert_eq!(plain.required_format_version(), PATTERN_BASE_FORMAT_VERSION);
    assert!(
        encode_pattern(&plain)
            .expect("encodes")
            .starts_with(r#"{"version":1,"#)
    );

    let mut with_variant = plain.clone();
    with_variant.zones[0].color = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    assert_eq!(
        with_variant.required_format_version(),
        PATTERN_FORMAT_VERSION
    );
    // The pattern was loaded as version 1 and then edited: the file is still written as 2.
    assert_eq!(with_variant.version, PATTERN_BASE_FORMAT_VERSION);
    let text = encode_pattern(&with_variant).expect("encodes");
    assert!(text.starts_with(r#"{"version":2,"#), "{text}");
    assert_eq!(decode_pattern(&text).expect("decodes").version, 2);

    let mut with_three = plain;
    with_three.base.color = trichroic();
    assert_eq!(with_three.format_version(), PATTERN_FORMAT_VERSION);

    // A text with a newer version than this build reads is refused with the version sentence.
    let newer = text.replacen(r#""version":2"#, r#""version":3"#, 1);
    assert_eq!(
        decode_pattern(&newer),
        Err(PatternCodecError::UnsupportedVersion(3))
    );
}

// ---- codec ---------------------------------------------------------------------------------

#[test]
fn variant_and_trichroic_colors_round_trip_the_codec_bit_exactly() {
    let yellow = sapphire("sapphire.yellow");
    let pink = sapphire("sapphire.pink");
    let mut pattern = template(TemplateId::Tricolor, PatternAxis::TABLE_NORMAL);
    pattern.base.color = trichroic();
    pattern.zones[0].color = PatternColor::from_variant(&yellow);
    pattern.zones[1].color = PatternColor::from_variant(&pink);
    let pattern = pattern.with_required_version();
    assert_eq!(pattern.version, PATTERN_FORMAT_VERSION);
    assert!(pattern.unsolved_colors().is_empty());

    let text = encode_pattern(&pattern).expect("encodes");
    assert!(
        text.contains("Variant") && text.contains("Trichroic"),
        "{text}"
    );
    let back = decode_pattern(&text).expect("decodes");
    assert_eq!(back, pattern);
    for zone in 1..=2 {
        let (a, b) = (
            rows_of(&pattern.zone(zone).expect("zone").color),
            rows_of(&back.zone(zone).expect("zone").color),
        );
        assert_rows_bit_equal(&a.o_ray, &b.o_ray);
        assert_rows_bit_equal(&a.e_ray, &b.e_ray);
        assert_eq!(a.pleochroic, b.pleochroic);
        assert_eq!(a.beta_ray, b.beta_ray);
    }
    // A per-millimeter variant is stored exactly: the tensor of the stored rows is the variant's.
    assert_eq!(
        rows_of(&pattern.zones[0].color).to_tensor(),
        yellow.absorption
    );
    // Writing it again gives the same text.
    assert_eq!(encode_pattern(&back).expect("re-encodes"), text);
}

#[test]
fn a_variant_with_a_band_that_is_not_a_number_is_refused() {
    let mut pattern = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    let mut color = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    if let PatternColor::Variant { absorption, .. } = &mut color {
        absorption.o_ray[0].peak = f32::NAN;
    }
    pattern.zones[0].color = color;
    assert!(matches!(
        encode_pattern(&pattern),
        Err(PatternCodecError::Invalid(_))
    ));
    assert_eq!(
        apply_color_pattern(GemMaterial::sapphire(), &pattern, 7.0),
        Err(PatternError::BadZone {
            zone: Some("Upper Half".to_owned()),
            problem: ZoneProblem::BadBands
        })
    );
}

// ---- zone tensors --------------------------------------------------------------------------

#[test]
fn zone_tensors_are_pleochroic_for_dichroic_trichroic_and_pleochroic_variants() {
    let yellow = sapphire("sapphire.yellow");
    let mut pattern = filled(template(TemplateId::Tricolor, PatternAxis::TABLE_NORMAL));
    pattern.base.color = trichroic();
    pattern.zones[0].color = PatternColor::from_variant(&yellow);
    pattern.zones[1].color = PatternColor::Dichroic {
        along: solved(&ROWS),
        across: solved(&OTHER_ROWS),
    };
    let zoned = pattern.to_zoned().expect("valid").expect("zones");

    // Trichroic: alpha, beta and gamma are o_ray, beta_ray and e_ray.
    let base = &zoned.base.tensor;
    assert!(base.is_pleochroic);
    assert_eq!(base.o_ray, bands_from_rows(&ROWS));
    assert_eq!(base.beta_ray, Some(bands_from_rows(&OTHER_ROWS)));
    assert_eq!(base.e_ray, bands_from_rows(&THIRD_ROWS));
    assert_eq!(
        base,
        &AbsorptionTensor::biaxial(
            bands_from_rows(&ROWS),
            bands_from_rows(&OTHER_ROWS),
            bands_from_rows(&THIRD_ROWS)
        )
    );

    // A pleochroic variant keeps its tensor, bit for bit.
    let variant_tensor = &zoned.zones[0].absorption.tensor;
    assert!(variant_tensor.is_pleochroic);
    assert_ne!(variant_tensor.o_ray, variant_tensor.e_ray);
    assert_eq!(variant_tensor, &yellow.absorption);

    // Along and Across.
    let dichroic = &zoned.zones[1].absorption.tensor;
    assert!(dichroic.is_pleochroic && dichroic.beta_ray.is_none());
    assert_eq!(dichroic.o_ray, bands_from_rows(&ROWS));
    assert_eq!(dichroic.e_ray, bands_from_rows(&OTHER_ROWS));

    // Every zone stays per millimeter.
    for zone in std::iter::once(&zoned.base).chain(zoned.zones.iter().map(|zone| &zone.absorption))
    {
        assert_eq!(zone.unit, AbsorptionUnit::PerMm);
    }
}

#[test]
fn an_unsolved_third_color_is_refused_and_listed_by_slot() {
    let mut pattern = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    pattern.base.color = PatternColor::Trichroic {
        alpha: solved(&ROWS),
        beta: ZoneColor::unsolved([50.0, 30.0, 100.0]),
        gamma: solved(&THIRD_ROWS),
    };
    assert_eq!(
        pattern.unsolved_colors(),
        vec![ColorRef {
            zone: 0,
            slot: ColorSlot::Beta
        }]
    );
    assert!(matches!(
        pattern.to_zoned(),
        Err(PatternError::NeedsColor { .. })
    ));
    let at = ColorRef {
        zone: 0,
        slot: ColorSlot::Beta,
    };
    assert!(pattern.set_color_bands(at, OTHER_ROWS.to_vec()));
    assert!(pattern.unsolved_colors().is_empty());
    assert_eq!(
        pattern.color(at).map(|c| c.bands.clone()),
        Some(OTHER_ROWS.to_vec())
    );
    // The Single slot of a trichroic zone does not exist.
    assert!(
        pattern
            .color(ColorRef {
                zone: 0,
                slot: ColorSlot::Single
            })
            .is_none()
    );
    pattern.to_zoned().expect("valid").expect("zones");
}

#[test]
fn importing_a_biaxial_zone_keeps_its_third_color() {
    let alexandrite = AbsorptionTensor::biaxial(
        vec![AbsorptionBand::new(640.0, 44.4, 0.31)],
        vec![AbsorptionBand::new(540.0, 30.0, 0.17)],
        vec![AbsorptionBand::new(460.0, 39.9, 0.55)],
    );
    let mut original = ZonedAbsorption::new(ZoneAbsorption::per_mm(alexandrite.clone()));
    original.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::Y,
            offset: 0.0,
        },
        absorption: ZoneAbsorption::per_mm(alexandrite),
    });
    let imported = ColorPattern::from_zoned(&original, true, |_| [50.0, 20.0, 80.0]);
    let PatternColor::Trichroic { alpha, beta, gamma } = &imported.base.color else {
        panic!("a biaxial tensor imports as three colors");
    };
    assert_eq!(alpha.bands, vec![[640.0, 44.4, 0.31]]);
    assert_eq!(beta.bands, vec![[540.0, 30.0, 0.17]]);
    assert_eq!(gamma.bands, vec![[460.0, 39.9, 0.55]]);
    assert_eq!(imported.version, PATTERN_FORMAT_VERSION);
    assert!(imported.unsolved_colors().is_empty());
    assert_eq!(imported.to_zoned().expect("valid"), Some(original));
}

// ---- per millimeter ------------------------------------------------------------------------

#[test]
fn a_model_unit_variant_becomes_per_millimeter_by_the_render_rule() {
    // The Sapphire's Material Default is per model unit and pleochroic (the measured Pink of
    // 2026-10-27 is per millimetre).
    let pink = sapphire("sapphire.default");
    assert_eq!(pink.unit, AbsorptionUnit::ModelUnit, "the premise");

    // The factor is the ratio of the render's two path scales, at any stone and model width.
    let factor = per_mm_factor(AbsorptionUnit::ModelUnit);
    for (width, model_width) in [(7.0_f32, 2.0), (12.0, 2.4), (4.5, 1.7)] {
        let model = absorption_path_scale_for(AbsorptionUnit::ModelUnit, width, Some(model_width))
            .expect("scale");
        let per_mm = absorption_path_scale_for(AbsorptionUnit::PerMm, width, Some(model_width))
            .expect("scale");
        let (model, per_mm) = (f64::from(model), f64::from(per_mm));
        assert!(
            factor.mul_add(per_mm, -model).abs() <= 1e-5 * model,
            "{width} mm / {model_width}: {} vs {model}",
            factor * per_mm
        );
    }
    assert_eq!(
        per_mm_factor(AbsorptionUnit::PerMm).to_bits(),
        1.0_f64.to_bits()
    );

    // Only the peaks change; centres, widths, shapes and the pleochroism do not.
    let rows = per_mm_rows(&pink);
    let stored = PatternTensorRows::from_tensor(&pink.absorption);
    assert_eq!(rows.pleochroic, stored.pleochroic);
    for (new, old) in rows
        .o_ray
        .iter()
        .zip(&stored.o_ray)
        .chain(rows.e_ray.iter().zip(&stored.e_ray))
    {
        assert_eq!(new.center_nm.to_bits(), old.center_nm.to_bits());
        assert_eq!(new.width_nm.to_bits(), old.width_nm.to_bits());
        assert_eq!(new.shape, old.shape);
        let expected = f64::from(old.peak) * factor;
        assert!((f64::from(new.peak) - expected).abs() <= 1e-6 * expected);
        assert!(new.peak < old.peak, "per mm numbers are the smaller ones");
    }
    // The zone color carries the converted rows.
    assert_eq!(rows_of(&PatternColor::from_variant(&pink)), &rows);
}

#[test]
fn a_per_millimeter_variant_is_stored_as_it_is() {
    let yellow = sapphire("sapphire.yellow");
    assert_eq!(yellow.unit, AbsorptionUnit::PerMm, "the premise");
    let rows = per_mm_rows(&yellow);
    let stored = PatternTensorRows::from_tensor(&yellow.absorption);
    assert_rows_bit_equal(&rows.o_ray, &stored.o_ray);
    assert_rows_bit_equal(&rows.e_ray, &stored.e_ray);
    assert_eq!(rows.to_tensor(), yellow.absorption);
    assert_eq!(rows.to_variant_rows().to_tensor(), yellow.absorption);
    assert_eq!(
        PatternTensorRows::from_variant_rows(&rows.to_variant_rows()),
        rows
    );
}

// ---- the rank rule ---------------------------------------------------------------------------

#[test]
fn a_zone_color_must_fit_the_rank_of_the_host() {
    let sapphire_host = GemMaterial::sapphire();
    let diamond = GemMaterial::diamond();
    let alexandrite = built_in_material_by_exact_name("Alexandrite").expect("a built-in");
    let one = PatternColor::Single(solved(&ROWS));
    let two = PatternColor::Dichroic {
        along: solved(&ROWS),
        across: solved(&OTHER_ROWS),
    };
    let three = trichroic();
    let yellow = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    let fit = |color: &PatternColor, host: &GemMaterial| {
        zone_rank_fit(color, host, allowed_rank(host, false))
    };

    // Isotropic host: one color only.
    assert_eq!(fit(&one, &diamond), ZoneRankFit::Fits);
    assert_eq!(fit(&two, &diamond), ZoneRankFit::TooRich);
    assert_eq!(fit(&three, &diamond), ZoneRankFit::TooRich);
    assert!(matches!(
        fit(&yellow, &diamond),
        ZoneRankFit::BadVariant(VariantProblem::RankMismatch { .. })
    ));

    // Uniaxial host: Along and Across; a stored one-color zone is the legacy case.
    assert_eq!(fit(&two, &sapphire_host), ZoneRankFit::Fits);
    assert_eq!(fit(&one, &sapphire_host), ZoneRankFit::Legacy);
    assert!(fit(&one, &sapphire_host).is_usable());
    assert_eq!(fit(&three, &sapphire_host), ZoneRankFit::TooRich);
    assert!(!fit(&three, &sapphire_host).is_usable());
    assert_eq!(fit(&yellow, &sapphire_host), ZoneRankFit::Fits);

    // Biaxial host: Alpha, Beta and Gamma; two colors are the two-set approximation.
    assert_eq!(fit(&three, &alexandrite), ZoneRankFit::Fits);
    assert_eq!(fit(&one, &alexandrite), ZoneRankFit::Legacy);
    assert_eq!(fit(&two, &alexandrite), ZoneRankFit::Fits);

    // A custom material the model calls isotropic but that may show two colors.
    assert_eq!(allowed_rank(&diamond, true), TensorRank::Uniaxial);
    assert_eq!(allowed_rank(&diamond, false), TensorRank::Isotropic);
    assert_eq!(
        zone_rank_fit(&two, &diamond, allowed_rank(&diamond, true)),
        ZoneRankFit::Fits
    );
}

#[test]
fn a_variant_zone_is_checked_with_the_variants_own_validation() {
    let mut color = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    let host = GemMaterial::sapphire();
    let allowed = allowed_rank(&host, false);
    assert_eq!(zone_rank_fit(&color, &host, allowed), ZoneRankFit::Fits);
    if let PatternColor::Variant { absorption, .. } = &mut color {
        absorption.o_ray[0].peak = -1.0;
    }
    assert_eq!(
        zone_rank_fit(&color, &host, allowed),
        ZoneRankFit::BadVariant(VariantProblem::Negative)
    );
}

// ---- converting between forms ---------------------------------------------------------------

#[test]
fn the_color_form_of_a_zone_keeps_its_colors_when_it_changes() {
    let mut pattern = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    let one = pattern.zones[0].color.clone();
    let PatternColor::Single(first) = one else {
        panic!("a template zone has one color");
    };

    assert!(pattern.set_color_form(1, ColorForm::Dichroic));
    assert_eq!(
        pattern.zones[0].color,
        PatternColor::Dichroic {
            along: first.clone(),
            across: first.clone()
        }
    );
    // Two colors to three: the uniaxial tensor is reproduced (alpha = beta).
    assert!(pattern.set_color(
        ColorRef {
            zone: 1,
            slot: ColorSlot::Across
        },
        solved(&OTHER_ROWS)
    ));
    assert!(pattern.set_color_form(1, ColorForm::Trichroic));
    let PatternColor::Trichroic { alpha, beta, gamma } = pattern.zones[0].color.clone() else {
        panic!("three colors");
    };
    assert_eq!((alpha.clone(), beta), (first.clone(), first.clone()));
    assert_eq!(gamma.bands, OTHER_ROWS.to_vec());
    let zoned = pattern.to_zoned().expect("valid").expect("zones");
    assert_eq!(
        zoned.zones[0].absorption.tensor.beta_ray.as_ref(),
        Some(&zoned.zones[0].absorption.tensor.o_ray)
    );
    // Three colors back to two keep alpha and gamma, then to one keep alpha.
    assert!(pattern.set_color_form(1, ColorForm::Dichroic));
    assert_eq!(
        pattern.zones[0].color,
        PatternColor::Dichroic {
            along: alpha,
            across: gamma
        }
    );
    assert!(pattern.set_color_form(1, ColorForm::Single));
    assert_eq!(pattern.zones[0].color, PatternColor::Single(first));
    assert!(!pattern.set_color_form(9, ColorForm::Single));

    // A variant has no form to change: it is turned into a picked color first.
    pattern.zones[0].color = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    assert!(!pattern.set_color_form(1, ColorForm::Dichroic));
}

#[test]
fn a_variant_becomes_a_picked_color_of_the_same_rank() {
    let pink = sapphire("sapphire.pink");
    let variant = PatternColor::from_variant(&pink);
    assert_eq!(variant.rank(), TensorRank::Uniaxial);
    assert!(!variant.is_custom());
    assert_eq!(variant.variant_key(), Some("sapphire.pink"));
    let custom = variant.to_custom(
        |rows| [f64::from(rows[0][0]) / 10.0, 30.0, 100.0],
        |_| unreachable!("the pink's bands are carried by rows"),
    );
    let PatternColor::Dichroic { along, across } = &custom else {
        panic!("a pleochroic uniaxial variant becomes Along and Across");
    };
    // The pink's bands are all wide energy bands, so the row encoding carries them.
    assert_eq!(along.bands.len(), pink.absorption.o_ray.len());
    assert_eq!(across.bands.len(), pink.absorption.e_ray.len());
    assert!(along.is_solved() && across.is_solved());
    assert_eq!(
        along.lch,
        [
            f64::from(pink.absorption.o_ray[0].center_nm) / 10.0,
            30.0,
            100.0
        ]
    );
    assert!(custom.is_custom() && custom.variant_key().is_none());
    // A picked color is its own custom form.
    assert_eq!(custom.to_custom(|_| [0.0; 3], |_| Vec::new()), custom);
    assert_eq!(custom.zone_colors().len(), 2);
    assert!(variant.zone_colors().is_empty());
}

/// A pleochroic tensor with an energy band of sigma 301 cm^-1, which a row (width at most 500
/// reads back as nanometers) cannot carry.
fn narrow_energy_tensor() -> AbsorptionTensor {
    let narrow = AbsorptionBand {
        center_nm: 428.3,
        width_nm: 301.4,
        peak: 0.02,
        shape: pleochroma::optics::absorption::BandShape::GaussianEnergy,
    };
    AbsorptionTensor::uniaxial(
        vec![narrow, AbsorptionBand::energy(550.0, 3_000.0, 0.3)],
        vec![AbsorptionBand::energy(500.0, 4_000.0, 0.2)],
    )
}

#[test]
fn a_narrow_energy_band_is_not_carried_by_rows() {
    use super::{rows_carry_bands, rows_carry_tensor};
    let tensor = narrow_energy_tensor();
    assert!(!rows_carry_bands(&tensor.o_ray));
    assert!(rows_carry_bands(&tensor.e_ray));
    assert!(!rows_carry_tensor(&tensor));
    assert!(super::pattern_holds_tensor(&tensor));
    // The wide measured sapphire variants are carried; a zero-peak band needs no row.
    assert!(rows_carry_tensor(&sapphire("sapphire.pink").absorption));
    assert!(rows_carry_bands(&[AbsorptionBand::energy(
        400.0, 100.0, 0.0
    )]));
}

#[test]
fn a_zone_tensor_rows_cannot_carry_is_kept_exactly_as_a_variant() {
    let tensor = narrow_energy_tensor();
    let zone = ZoneAbsorption::per_mm(tensor.clone());
    let color = super::convert::pattern_color(&zone, &|_| [50.0, 20.0, 80.0]);
    let PatternColor::Variant {
        key, absorption, ..
    } = &color
    else {
        panic!("a tensor the rows cannot carry is kept as exact rows");
    };
    assert_eq!(key, super::EXACT_COLOR_KEY);
    assert_eq!(absorption.to_tensor(), tensor);
    // A carried tensor stays a picked color.
    let carried = ZoneAbsorption::per_mm(sapphire("sapphire.pink").absorption);
    assert!(super::convert::pattern_color(&carried, &|_| [50.0, 20.0, 80.0]).is_custom());
}

#[test]
fn a_variant_the_rows_cannot_carry_becomes_a_picked_color_of_its_own_color() {
    let variant = PatternColor::Variant {
        key: "sapphire.test".to_owned(),
        label: "Test".to_owned(),
        absorption: PatternTensorRows::from_tensor(&narrow_energy_tensor()),
    };
    let custom = variant.to_custom(
        |_| unreachable!("a tensor rows cannot carry has no rows to read"),
        |tensor| {
            assert_eq!(tensor, &narrow_energy_tensor());
            vec![[40.0, 30.0, 250.0], [60.0, 20.0, 80.0]]
        },
    );
    let PatternColor::Dichroic { along, across } = &custom else {
        panic!("a pleochroic uniaxial variant becomes Along and Across");
    };
    // The color of each direction is kept; the bands are solved for it afterwards, not left out.
    assert_eq!(along.lch, [40.0, 30.0, 250.0]);
    assert_eq!(across.lch, [60.0, 20.0, 80.0]);
    assert!(!along.is_solved() && !across.is_solved());
}

#[test]
fn a_variant_base_has_a_neutral_slider_seed() {
    let mut pattern = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL));
    pattern.base.color = PatternColor::from_variant(&sapphire("sapphire.yellow"));
    assert_eq!(pattern.base_lch(), super::VARIANT_SEED_LCH);
    pattern.base.color = trichroic();
    assert_eq!(pattern.base_lch(), solved(&ROWS).lch);
}
