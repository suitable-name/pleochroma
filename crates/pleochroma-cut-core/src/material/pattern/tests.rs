//! Tests of the color pattern model (written, not run, by lane CP-C1).

use super::{
    ColorPattern, ColorRef, ColorSlot, PATTERN_BASE_FORMAT_VERSION, PATTERN_FORMAT_VERSION,
    PatternAxis, PatternCodecError, PatternColor, PatternError, PatternZone, TemplateId, ZoneColor,
    ZoneProblem, bands_from_rows, decode_pattern, encode_pattern, rows_from_bands, template,
};
use crate::rough_plan::zoned_plan::{decode_zoned, encode_zoned, resolve_relative};
use glam::DVec3;
use pleochroma::optics::{
    absorption::{AbsorptionBand, BandShape},
    materials::GemMaterial,
    zoning::{MAX_ZONES, ZoneShape},
};

/// Seven-band style rows: two energy bands (width above 500) and one wavelength band.
const ROWS: [[f32; 3]; 3] = [
    [480.0, 2100.0, 0.22],
    [560.0, 1500.0, 0.11],
    [620.0, 38.0, 0.05],
];
const OTHER_ROWS: [[f32; 3]; 2] = [[430.0, 2600.0, 0.4], [520.0, 1800.0, 0.07]];

/// Gives every unsolved color of `pattern` the rows `ROWS`.
fn filled(mut pattern: ColorPattern) -> ColorPattern {
    for at in pattern.unsolved_colors() {
        assert!(pattern.set_color_bands(at, ROWS.to_vec()));
    }
    assert_eq!(
        pattern.unsolved_colors(),
        [] as [crate::material::pattern::ColorRef; 0]
    );
    pattern
}

fn zone_count(id: TemplateId) -> usize {
    match id {
        TemplateId::Uniform => 0,
        TemplateId::Bicolor
        | TemplateId::Watermelon
        | TemplateId::ReverseWatermelon
        | TemplateId::CoreAndRim
        | TemplateId::Split
        | TemplateId::PartiColor
        | TemplateId::ColorBand => 1,
        TemplateId::Tricolor | TemplateId::Sectors => 2,
        TemplateId::HexagonalBanding => 3,
        TemplateId::Banded => 4,
    }
}

#[test]
fn every_template_validates_once_its_colors_have_bands() {
    for id in TemplateId::ALL.into_iter().chain(TemplateId::TYPICAL_ONLY) {
        for axis in [
            PatternAxis::TABLE_NORMAL,
            PatternAxis::ACROSS,
            PatternAxis::ACROSS_TURNED,
        ] {
            let pattern = template(id, axis);
            assert_eq!(pattern.template, Some(id));
            assert!(pattern.relative_to_stone, "{}", id.label());
            assert_eq!(pattern.zones.len(), zone_count(id), "{}", id.label());
            assert_eq!(pattern.version, PATTERN_BASE_FORMAT_VERSION);
            let colored = filled(pattern);
            let zoned = colored
                .to_zoned()
                .unwrap_or_else(|e| panic!("{}: {e}", id.label()));
            match zoned {
                None => assert_eq!(id, TemplateId::Uniform),
                Some(zoned) => {
                    assert_eq!(zoned.validate(), Ok(()), "{}", id.label());
                    assert_eq!(zoned.zones.len(), zone_count(id));
                    assert!(zoned.zones.len() <= MAX_ZONES);
                }
            }
        }
    }
}

#[test]
fn template_labels_are_title_case_and_distinct() {
    let labels: Vec<&str> = TemplateId::ALL
        .iter()
        .chain(&TemplateId::TYPICAL_ONLY)
        .map(|id| id.label())
        .collect();
    for (i, a) in labels.iter().enumerate() {
        for b in &labels[i + 1..] {
            assert_ne!(a, b);
        }
        assert!(a.chars().next().is_some_and(char::is_uppercase));
    }
}

#[test]
fn a_uniform_pattern_stores_no_zones() {
    let uniform = template(TemplateId::Uniform, PatternAxis::TABLE_NORMAL);
    assert_eq!(
        uniform.zones,
        [] as [crate::material::pattern::PatternZone; 0]
    );
    // Unsolved or solved, it is the plain material.
    assert_eq!(uniform.to_zoned(), Ok(None));
    assert_eq!(filled(uniform).to_zoned(), Ok(None));
}

#[test]
fn unsolved_colors_are_listed_and_refused() {
    let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
    let unsolved = pattern.unsolved_colors();
    assert_eq!(
        unsolved,
        vec![
            ColorRef {
                zone: 0,
                slot: ColorSlot::Single
            },
            ColorRef {
                zone: 1,
                slot: ColorSlot::Single
            },
        ]
    );
    let error = pattern.to_zoned().expect_err("no bands yet");
    assert_eq!(
        error,
        PatternError::NeedsColor {
            zone: "Rim".to_owned()
        }
    );
    assert_eq!(error.to_string(), "Pick a color for every zone first.");
    assert_eq!(error.zone(), Some("Rim"));

    // The rim alone is not enough.
    assert!(pattern.set_color_bands(unsolved[0], ROWS.to_vec()));
    assert_eq!(
        pattern.to_zoned().expect_err("core still unsolved"),
        PatternError::NeedsColor {
            zone: "Core".to_owned()
        }
    );
    // A solution with no bands at all is a colorless zone, not an unsolved one.
    assert!(pattern.set_color_bands(unsolved[1], Vec::new()));
    assert_eq!(
        pattern.unsolved_colors(),
        [] as [crate::material::pattern::ColorRef; 0]
    );
    let zoned = pattern.to_zoned().expect("solved").expect("zones");
    assert_eq!(
        zoned.zones[0].absorption.tensor.o_ray,
        [] as [pleochroma::optics::absorption::AbsorptionBand; 0]
    );
    // The sliders stay when the bands are set.
    assert_eq!(
        pattern.zones[0].color,
        PatternColor::Single(ZoneColor::solved([60.0, 35.0, 0.0], Vec::new()))
    );
    // A color that does not exist cannot be set.
    assert!(!pattern.set_color_bands(
        ColorRef {
            zone: 1,
            slot: ColorSlot::Along
        },
        Vec::new()
    ));
    assert!(!pattern.set_color_bands(
        ColorRef {
            zone: 9,
            slot: ColorSlot::Single
        },
        Vec::new()
    ));
}

#[test]
fn json_round_trip_keeps_the_pattern_and_the_pleochroic_flag() {
    let mut pattern = filled(template(TemplateId::Watermelon, PatternAxis::ACROSS));
    assert!(pattern.set_two_colors(1, true));
    assert!(pattern.set_color_bands(
        ColorRef {
            zone: 1,
            slot: ColorSlot::Across
        },
        OTHER_ROWS.to_vec()
    ));
    pattern.softness = 0.02;

    let text = encode_pattern(&pattern).expect("encodes");
    assert_eq!(decode_pattern(&text).expect("decodes"), pattern);

    // The zones survive their own codec with the pleochroic flag.
    let zoned = pattern.to_zoned().expect("valid").expect("zones");
    assert!(zoned.zones[0].absorption.tensor.is_pleochroic);
    assert!(!zoned.base.tensor.is_pleochroic);
    let again = decode_zoned(&encode_zoned(&zoned).expect("encodes")).expect("decodes");
    assert_eq!(again, zoned);
    assert!(again.zones[0].absorption.tensor.is_pleochroic);
}

#[test]
fn decoding_refuses_newer_versions_garbage_and_bad_structure() {
    let pattern = filled(template(TemplateId::Split, PatternAxis::TABLE_NORMAL));
    let text = encode_pattern(&pattern).expect("encodes");

    let too_new = PATTERN_FORMAT_VERSION + 1;
    let newer = text.replacen("\"version\":1", &format!("\"version\":{too_new}"), 1);
    assert_ne!(newer, text);
    assert_eq!(
        decode_pattern(&newer),
        Err(PatternCodecError::UnsupportedVersion(too_new))
    );
    assert!(matches!(
        decode_pattern("not json"),
        Err(PatternCodecError::Json(_))
    ));
    assert!(matches!(
        decode_pattern(&"x".repeat(2 * 1024 * 1024)),
        Err(PatternCodecError::TooLarge(_))
    ));

    let mut crowded = pattern.clone();
    for _ in 0..MAX_ZONES {
        crowded.zones.push(crowded.zones[0].clone());
    }
    assert!(matches!(
        encode_pattern(&crowded),
        Err(PatternCodecError::Invalid(_))
    ));
    let mut negative = pattern;
    negative.softness = -1.0;
    assert!(matches!(
        encode_pattern(&negative),
        Err(PatternCodecError::Invalid(_))
    ));
}

#[test]
fn rows_become_the_same_bands_as_the_material_builder_makes() {
    let bands = bands_from_rows(&ROWS);
    assert_eq!(bands.len(), 3);
    assert_eq!(bands[0].shape, BandShape::GaussianEnergy);
    assert_eq!(bands[1].shape, BandShape::GaussianEnergy);
    assert_eq!(bands[2].shape, BandShape::GaussianWavelength);

    // Lock-step with `GemMaterial::with_body_color_bands`, the rule this repeats.
    let material = GemMaterial::new_custom("probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_body_color_bands(&ROWS, 3.5);
    assert_eq!(material.absorption.o_ray, bands);

    // Unusable rows are dropped by both.
    let rows = [
        [500.0, 30.0, 0.2],
        [f32::NAN, 30.0, 0.2],
        [500.0, -1.0, 0.2],
        [500.0, 30.0, 0.0],
    ];
    let material = GemMaterial::new_custom("probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_body_color_bands(&rows, 3.5);
    assert_eq!(material.absorption.o_ray, bands_from_rows(&rows));
    assert_eq!(bands_from_rows(&rows).len(), 1);

    // And back without loss: energy rows stay energy rows.
    assert_eq!(rows_from_bands(&bands), ROWS.to_vec());
    assert_eq!(bands_from_rows(&rows_from_bands(&bands)), bands);
}

#[test]
fn energy_rows_stay_energy_rows_in_the_zones() {
    let zoned = filled(template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL))
        .to_zoned()
        .expect("valid")
        .expect("zones");
    for tensor in [&zoned.base.tensor, &zoned.zones[0].absorption.tensor] {
        assert_eq!(tensor.o_ray, bands_from_rows(&ROWS));
        assert_eq!(tensor.o_ray[0].shape, BandShape::GaussianEnergy);
        assert!(!tensor.is_pleochroic);
        assert_eq!(tensor.o_ray, tensor.e_ray, "a one-color zone is isotropic");
    }
}

#[test]
fn a_dichroic_zone_is_o_along_and_e_across() {
    let mut pattern = filled(template(TemplateId::CoreAndRim, PatternAxis::TABLE_NORMAL));
    assert!(pattern.set_two_colors(1, true));
    // Both colors start as the one they came from.
    assert_eq!(pattern.unsolved_colors(), Vec::new());
    assert!(pattern.set_color_bands(
        ColorRef {
            zone: 1,
            slot: ColorSlot::Across
        },
        OTHER_ROWS.to_vec()
    ));
    let zoned = pattern.to_zoned().expect("valid").expect("zones");
    let tensor = &zoned.zones[0].absorption.tensor;
    assert!(tensor.is_pleochroic);
    assert_eq!(tensor.o_ray, bands_from_rows(&ROWS));
    assert_eq!(tensor.e_ray, bands_from_rows(&OTHER_ROWS));
    assert_ne!(tensor.o_ray, tensor.e_ray);
    // The rim stays a one-color zone.
    assert!(!zoned.base.tensor.is_pleochroic);

    // Switching back keeps the Along color.
    assert!(pattern.set_two_colors(1, false));
    let single = pattern.to_zoned().expect("valid").expect("zones");
    assert!(!single.zones[0].absorption.tensor.is_pleochroic);
    assert_eq!(
        single.zones[0].absorption.tensor.o_ray,
        bands_from_rows(&ROWS)
    );
    assert!(!pattern.set_two_colors(9, true));
}

#[test]
fn an_unsolved_second_color_is_refused() {
    let mut pattern = filled(template(TemplateId::CoreAndRim, PatternAxis::TABLE_NORMAL));
    pattern.zones[0].color = PatternColor::Dichroic {
        along: ZoneColor::solved([50.0, 40.0, 300.0], ROWS.to_vec()),
        across: ZoneColor::unsolved([50.0, 40.0, 200.0]),
    };
    assert_eq!(
        pattern.unsolved_colors(),
        vec![ColorRef {
            zone: 1,
            slot: ColorSlot::Across
        }]
    );
    assert!(matches!(
        pattern.to_zoned(),
        Err(PatternError::NeedsColor { .. })
    ));
}

#[test]
fn relative_geometry_scaled_to_a_width_equals_the_millimeter_form() {
    let width = 6.5;
    for id in TemplateId::ALL {
        let mut relative = filled(template(id, PatternAxis::ACROSS_TURNED));
        relative.softness = 0.03;
        let fixed = relative.to_fixed_mm(width).expect("positive width");
        assert!(!fixed.relative_to_stone);
        assert!(relative.relative_to_stone);

        let (Some(from_relative), Some(from_fixed)) = (
            relative.to_zoned().expect("valid"),
            fixed.to_zoned().expect("valid"),
        ) else {
            assert_eq!(id, TemplateId::Uniform);
            continue;
        };
        // The same scaling the stored-zones path applies (`resolve_relative`).
        let resolved = resolve_relative(&from_relative, width).expect("valid");
        assert_eq!(resolved, from_fixed, "{}", id.label());
        // And back.
        assert_eq!(
            fixed
                .to_relative(width)
                .expect("positive width")
                .zones
                .len(),
            relative.zones.len()
        );
    }
    let relative = filled(template(TemplateId::Banded, PatternAxis::TABLE_NORMAL));
    let back = relative
        .to_fixed_mm(8.0)
        .and_then(|fixed| fixed.to_relative(8.0))
        .expect("positive width");
    for (a, b) in back.zones.iter().zip(&relative.zones) {
        let (
            ZoneShape::Slab {
                offset_min: a_min,
                offset_max: a_max,
                ..
            },
            ZoneShape::Slab {
                offset_min: b_min,
                offset_max: b_max,
                ..
            },
        ) = (&a.shape, &b.shape)
        else {
            panic!("banded zones are layers");
        };
        assert!((a_min - b_min).abs() < 1e-12 && (a_max - b_max).abs() < 1e-12);
    }
    assert!(relative.to_fixed_mm(0.0).is_none());
    assert!(relative.to_relative(f64::NAN).is_none());
}

#[test]
fn the_axis_presets_turn_the_pattern_axis_as_documented() {
    let along = |axis: PatternAxis| axis.frame().direction(DVec3::Y);
    let close = |a: DVec3, b: DVec3| (a - b).length() < 1e-12;
    assert!(close(along(PatternAxis::TABLE_NORMAL), DVec3::Y));
    // Tilt about +X takes +Y to +Z; the turn about +Y then takes +Z to +X.
    assert!(close(along(PatternAxis::ACROSS), DVec3::Z));
    assert!(close(along(PatternAxis::ACROSS_TURNED), DVec3::X));
    let frame = PatternAxis::ACROSS_TURNED.frame();
    assert_eq!(frame.translation, DVec3::ZERO);
    assert!((frame.rotation.length() - 1.0).abs() < 1e-12);

    // The template carries it into the zones.
    let zoned = filled(template(TemplateId::Split, PatternAxis::ACROSS))
        .to_zoned()
        .expect("valid")
        .expect("zones");
    assert_eq!(zoned.frame, PatternAxis::ACROSS.frame());
}

#[test]
fn validation_problems_are_plain_sentences() {
    let mut tube = filled(template(TemplateId::CoreAndRim, PatternAxis::TABLE_NORMAL));
    tube.zones[0].shape = ZoneShape::CoaxialCylinder {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Y,
        r_in: 0.4,
        r_out: 0.2,
    };
    let error = tube.to_zoned().expect_err("radii are reversed");
    assert_eq!(
        error,
        PatternError::BadZone {
            zone: Some("Core".to_owned()),
            problem: ZoneProblem::BadRadii
        }
    );
    assert_eq!(
        error.to_string(),
        "A zone's outer radius must be larger than its inner radius."
    );
    assert_eq!(error.zone(), Some("Core"));

    let mut crowded = filled(template(TemplateId::Banded, PatternAxis::TABLE_NORMAL));
    crowded.zones.push(crowded.zones[0].clone());
    let error = crowded.to_zoned().expect_err("five zones");
    assert_eq!(error, PatternError::TooManyZones);
    assert_eq!(
        error.to_string(),
        "A pattern holds at most 4 zones besides the base."
    );

    let mut soft = filled(template(TemplateId::Split, PatternAxis::TABLE_NORMAL));
    soft.softness = -0.1;
    let error = soft.to_zoned().expect_err("negative softness");
    assert_eq!(
        error,
        PatternError::BadZone {
            zone: None,
            problem: ZoneProblem::BadSoftness
        }
    );

    let mut slab = filled(template(TemplateId::Tricolor, PatternAxis::TABLE_NORMAL));
    slab.zones[1].shape = ZoneShape::Slab {
        normal: DVec3::Y,
        offset_min: 0.5,
        offset_max: 0.1,
    };
    assert_eq!(
        slab.to_zoned().expect_err("slab"),
        PatternError::BadZone {
            zone: Some("Bottom".to_owned()),
            problem: ZoneProblem::BadLayer
        }
    );
}

#[test]
fn importing_zones_and_converting_back_reproduces_them_exactly() {
    let mut pattern = filled(template(TemplateId::Watermelon, PatternAxis::ACROSS));
    assert!(pattern.set_two_colors(1, true));
    assert!(pattern.set_color_bands(
        ColorRef {
            zone: 1,
            slot: ColorSlot::Across
        },
        OTHER_ROWS.to_vec()
    ));
    pattern.softness = 0.01;
    let original = pattern.to_zoned().expect("valid").expect("zones");

    // The seeding function only has to be a function of the rows.
    let lch_of = |rows: &[[f32; 3]]| [f64::from(rows[0][0]) / 10.0, 30.0, 120.0];
    let imported = ColorPattern::from_zoned(&original, true, lch_of);
    assert_eq!(imported.template, None);
    assert!(imported.relative_to_stone);
    assert_eq!(imported.zones.len(), 1);
    assert_eq!(imported.zones[0].name, "Zone 1");
    assert_eq!(imported.base.name, "Base Zone");
    assert_eq!(
        imported.unsolved_colors(),
        [] as [crate::material::pattern::ColorRef; 0]
    );
    assert!(matches!(imported.base.color, PatternColor::Single(_)));
    let PatternColor::Dichroic { along, across } = &imported.zones[0].color else {
        panic!("a pleochroic zone imports as two colors");
    };
    assert_eq!(along.bands, ROWS.to_vec());
    assert_eq!(across.bands, OTHER_ROWS.to_vec());
    assert_eq!(along.lch, [48.0, 30.0, 120.0]);
    assert_eq!(across.lch, [43.0, 30.0, 120.0]);

    assert_eq!(imported.to_zoned().expect("valid"), Some(original.clone()));

    // An identity frame imports as the table normal, an unrelated one is kept bit for bit.
    let mut plain = original.clone();
    plain.frame = PatternAxis::TABLE_NORMAL.frame();
    assert_eq!(
        ColorPattern::from_zoned(&plain, false, lch_of).axis,
        PatternAxis::TABLE_NORMAL
    );
    assert!(!ColorPattern::from_zoned(&plain, false, lch_of).relative_to_stone);
    let mut moved = original;
    moved.frame.translation = DVec3::new(0.25, -1.5, 3.0);
    let imported = ColorPattern::from_zoned(&moved, false, lch_of);
    assert!(matches!(imported.axis, PatternAxis::Frame { .. }));
    assert_eq!(imported.to_zoned().expect("valid"), Some(moved));
}

#[test]
fn a_base_only_zoning_imports_as_a_uniform_pattern() {
    let zoned = filled(template(TemplateId::Split, PatternAxis::TABLE_NORMAL))
        .to_zoned()
        .expect("valid")
        .expect("zones");
    let mut base_only = zoned;
    base_only.zones.clear();
    let pattern = ColorPattern::from_zoned(&base_only, true, |_| [50.0, 0.0, 0.0]);
    assert_eq!(
        pattern.zones,
        [] as [crate::material::pattern::PatternZone; 0]
    );
    assert_eq!(pattern.to_zoned(), Ok(None));
}

#[test]
fn zone_accessors_address_the_base_as_zone_zero() {
    let mut pattern = template(TemplateId::Tricolor, PatternAxis::TABLE_NORMAL);
    assert_eq!(pattern.zone(0).map(|z| z.name.as_str()), Some("Middle"));
    assert_eq!(pattern.zone(1).map(|z| z.name.as_str()), Some("Top"));
    assert_eq!(pattern.zone(2).map(|z| z.name.as_str()), Some("Bottom"));
    assert!(pattern.zone(3).is_none());
    pattern.zone_mut(2).expect("exists").name = "Underside".to_owned();
    assert_eq!(pattern.zones[1].name, "Underside");
    let at = ColorRef {
        zone: 1,
        slot: ColorSlot::Single,
    };
    assert!(pattern.set_color(at, ZoneColor::solved([70.0, 20.0, 10.0], ROWS.to_vec())));
    assert_eq!(pattern.color(at).map(|c| c.lch), Some([70.0, 20.0, 10.0]));
    let base: &PatternZone = &pattern.base;
    assert_eq!(base.name, "Middle");
}

fn band(shape: BandShape, center_nm: f32, width_nm: f32) -> AbsorptionBand {
    AbsorptionBand {
        center_nm,
        width_nm,
        peak: 0.2,
        shape,
    }
}

#[test]
fn rows_from_bands_refuses_bands_that_would_change_shape() {
    let energy = |sigma| band(BandShape::GaussianEnergy, 520.0, sigma);
    let wavelength = |sigma| band(BandShape::GaussianWavelength, 520.0, sigma);

    // An energy sigma of 500 cm^-1 or less would read back as a wavelength Gaussian.
    assert!(rows_from_bands(&[energy(400.0)]).is_empty());
    assert!(rows_from_bands(&[energy(500.0)]).is_empty());
    assert_eq!(rows_from_bands(&[energy(501.0)]), vec![[520.0, 501.0, 0.2]]);

    // A wavelength sigma above 500 nm would read back as an energy band.
    assert_eq!(
        rows_from_bands(&[wavelength(500.0)]),
        vec![[520.0, 500.0, 0.2]]
    );
    assert!(rows_from_bands(&[wavelength(501.0)]).is_empty());

    // Every kept row decodes to the shape it was written from.
    let kept = [energy(501.0), wavelength(500.0), wavelength(30.0)];
    let decoded = bands_from_rows(&rows_from_bands(&kept));
    assert_eq!(decoded, kept.to_vec());

    // A non-positive or non-finite centre is dropped like any other unusable band.
    for centre in [0.0, -10.0, f32::NAN, f32::INFINITY] {
        let bad = band(BandShape::GaussianEnergy, centre, 2000.0);
        assert!(rows_from_bands(&[bad]).is_empty(), "centre {centre}");
    }
}

#[test]
fn a_non_positive_centre_is_dropped_by_both_codecs_in_lock_step() {
    let rows = [
        [0.0, 2000.0, 0.2],
        [-10.0, 40.0, 0.2],
        [f32::NAN, 40.0, 0.2],
        [f32::INFINITY, 40.0, 0.2],
        [520.0, 2000.0, 0.2],
    ];
    assert_eq!(bands_from_rows(&rows).len(), 1);
    let material = GemMaterial::new_custom("probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_body_color_bands(&rows, 3.5);
    assert_eq!(material.absorption.o_ray, bands_from_rows(&rows));
}

#[test]
fn round_trip_keeps_evaluated_spectrum_for_kept_bands() {
    let bands = [
        band(BandShape::GaussianEnergy, 460.0, 2100.0),
        band(BandShape::GaussianEnergy, 580.0, 1300.0),
        band(BandShape::GaussianWavelength, 620.0, 38.0),
    ];
    let back = bands_from_rows(&rows_from_bands(&bands));
    assert_eq!(back.len(), bands.len());
    for i in 0..10_u8 {
        let lambda = 35.0_f32.mul_add(f32::from(i), 400.0);
        let before: f32 = bands.iter().map(|b| b.evaluate(lambda)).sum();
        let after: f32 = back.iter().map(|b| b.evaluate(lambda)).sum();
        assert!(
            (before - after).abs() <= 1e-6,
            "{lambda} nm: {before} vs {after}"
        );
    }
}
