//! Tests of the typical color zoning of the species.

use super::{
    ColorPattern, MAX_PATTERN_ZONES, OwnColor, PATTERN_BASE_FORMAT_VERSION, PATTERN_FORMAT_VERSION,
    PatternAxis, PatternColor, TemplateId, TypicalPattern, ZoneColor, apply_color_pattern,
    decode_pattern, encode_pattern, template, typical_index, typical_patterns,
};
use glam::DVec3;
use pleochroma::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::{
        AbsorptionUnit, GemMaterial,
        variants::{ColorVariant, VariantProvenance, built_in_variants},
    },
    zoning::ZoneShape,
};
use std::f64::consts::{FRAC_PI_3, TAU};

const ROWS: [[f32; 3]; 2] = [[480.0, 2100.0, 0.22], [560.0, 1500.0, 0.11]];

/// Stone colors the entries are built from: a blue, a pale, a yellow, a pink, a green and a
/// colorless one.
const OWN_COLORS: [[f64; 3]; 6] = [
    [40.0, 45.0, 265.0],
    [85.0, 12.0, 250.0],
    [80.0, 60.0, 90.0],
    [60.0, 35.0, 0.0],
    [55.0, 35.0, 140.0],
    [95.0, 0.0, 0.0],
];

const SPECIES: [&str; 11] = [
    "Sapphire",
    "Ruby",
    "Emerald",
    "Aquamarine",
    "Morganite",
    "Tourmaline",
    "Amethyst",
    "Citrine",
    "Ametrine",
    "Fluorite",
    "Topaz",
];

fn ids(name: &str) -> Vec<TemplateId> {
    typical_patterns(name)
        .iter()
        .map(|entry| entry.template)
        .collect()
}

fn filled(mut pattern: ColorPattern) -> ColorPattern {
    for at in pattern.unsolved_colors() {
        assert!(pattern.set_color_bands(at, ROWS.to_vec()));
    }
    pattern
}

fn single_lch(pattern: &ColorPattern, zone: usize) -> [f64; 3] {
    match &pattern.zone(zone).expect("zone").color {
        PatternColor::Single(color) => color.lch,
        other => panic!("a typical pattern without variants has one color per zone: {other:?}"),
    }
}

fn follows() -> PatternAxis {
    PatternAxis::FollowsCrystalAxis
}

/// `entry` built without variants for a stone with one color, `own`, in every direction.
fn built(entry: &TypicalPattern, own: [f64; 3], axis: PatternAxis) -> ColorPattern {
    (entry.build)(&OwnColor::single(own), &[], axis)
}

#[test]
fn every_built_in_species_resolves_to_its_typical_list() {
    use TemplateId::{
        Banded, Bicolor, ColorBand, CoreAndRim, HexagonalBanding, PartiColor, ReverseWatermelon,
        Sectors, Split, Tricolor, Watermelon,
    };
    let expected: [(&str, &[TemplateId]); 7] = [
        ("Sapphire", &[HexagonalBanding, PartiColor, ColorBand]),
        ("Ruby", &[HexagonalBanding]),
        (
            "Tourmaline",
            &[Watermelon, ReverseWatermelon, Bicolor, Tricolor],
        ),
        ("Amethyst", &[Sectors, Split]),
        ("Citrine", &[Split, Sectors]),
        ("Ametrine", &[Split, Sectors]),
        ("Topaz", &[Bicolor]),
    ];
    let beryls = ["Emerald", "Aquamarine", "Morganite", "Heliodor", "Beryl"];
    let none: &[TemplateId] = &[];
    let mut resolved = 0;
    for material in GemMaterial::all_materials() {
        let name = material.name.as_str();
        let want: &[TemplateId] = if beryls.contains(&name) {
            &[HexagonalBanding, CoreAndRim]
        } else {
            expected
                .iter()
                .find(|(species, _)| *species == name)
                .map_or(none, |(_, list)| *list)
        };
        assert_eq!(ids(name), want, "{name}");
        resolved += usize::from(!want.is_empty());
    }
    // Sapphire, Ruby, Emerald, Aquamarine, Morganite, Tourmaline, Amethyst, Citrine, Ametrine,
    // Topaz: the built-in table has no Fluorite or Heliodor.
    assert_eq!(resolved, 10);
    // The names that are not in the table resolve too (a custom material of that name).
    assert_eq!(ids("fluorite"), [Banded, Tricolor]);
    assert_eq!(ids("Heliodor"), [HexagonalBanding, CoreAndRim]);
}

#[test]
fn matching_ignores_case_and_surrounding_space_and_never_guesses() {
    assert_eq!(ids("  SAPPHIRE "), ids("Sapphire"));
    assert!(!ids("sapphire").is_empty());
    for name in [
        "",
        "Diamond",
        "My Sapphire",
        "Blue Sapphire",
        "Glass (N-BK7)",
        "Spinel",
    ] {
        assert!(typical_patterns(name).is_empty(), "{name}");
    }
}

#[test]
fn the_entries_of_a_species_are_distinct_and_named_after_their_template() {
    for name in SPECIES {
        let list = typical_patterns(name);
        for (index, entry) in list.iter().enumerate() {
            assert_eq!(entry.label, entry.template.label(), "{name}");
            assert!(
                list[index + 1..]
                    .iter()
                    .all(|other| other.template != entry.template),
                "{name}: {} twice",
                entry.label
            );
            assert_eq!(typical_index(name, Some(entry.template)), Some(index));
        }
        assert_eq!(typical_index(name, None), None);
    }
    // A template the species does not list is not "from" an entry.
    assert_eq!(
        typical_index("Sapphire", Some(TemplateId::Watermelon)),
        None
    );
    assert_eq!(typical_index("Diamond", Some(TemplateId::Bicolor)), None);
}

#[test]
fn every_typical_pattern_validates_for_stones_of_any_color() {
    for name in SPECIES {
        for entry in typical_patterns(name) {
            for own in OWN_COLORS {
                for axis in [follows(), PatternAxis::TABLE_NORMAL, PatternAxis::ACROSS] {
                    let pattern = built(entry, own, axis);
                    let context = format!("{name} / {} / {own:?}", entry.label);
                    assert_eq!(pattern.template, Some(entry.template), "{context}");
                    assert_eq!(pattern.axis, axis, "{context}");
                    assert_eq!(pattern.version, PATTERN_BASE_FORMAT_VERSION, "{context}");
                    assert!(pattern.relative_to_stone, "{context}");
                    assert!(
                        (1..=MAX_PATTERN_ZONES).contains(&pattern.zones.len()),
                        "{context}"
                    );
                    assert!(
                        !pattern.unsolved_colors().is_empty(),
                        "{context}: the colors are for the application to solve"
                    );
                    for index in 0..=pattern.zones.len() {
                        let [l, c, h] = single_lch(&pattern, index);
                        assert!(
                            (0.0..=100.0).contains(&l)
                                && (0.0..=100.0).contains(&c)
                                && (0.0..360.0).contains(&h),
                            "{context}: zone {index} {l} {c} {h}"
                        );
                    }
                    let zoned = filled(pattern)
                        .to_zoned()
                        .unwrap_or_else(|error| panic!("{context}: {error}"))
                        .unwrap_or_else(|| panic!("{context}: no zones"));
                    assert_eq!(zoned.validate(), Ok(()), "{context}");
                    assert_eq!(zoned.follows_axis, axis.follows_crystal_axis(), "{context}");
                }
            }
        }
    }
}

/// Whether some part of `shape` lies in the stone: the cube of half a unit around the center.
fn touches_stone(shape: &ZoneShape) -> bool {
    match shape {
        ZoneShape::HalfSpace { offset, .. } => offset.abs() < 0.5,
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => *offset_min < 0.5 && *offset_max > -0.5,
        ZoneShape::CoaxialCylinder { r_in, .. } | ZoneShape::CoaxialPrism { r_in, .. } => {
            *r_in < 0.5
        }
        ZoneShape::Sector { .. } => true,
        ZoneShape::MeshShell { .. } => false,
    }
}

#[test]
fn the_zones_of_every_typical_pattern_reach_into_a_unit_wide_stone() {
    for name in SPECIES {
        for entry in typical_patterns(name) {
            let pattern = built(entry, OWN_COLORS[0], follows());
            for zone in &pattern.zones {
                assert!(
                    touches_stone(&zone.shape),
                    "{name} / {} / {}: {:?}",
                    entry.label,
                    zone.name,
                    zone.shape
                );
            }
        }
    }
}

#[test]
fn hexagonal_banding_is_three_concentric_hexagonal_shells_in_the_stronger_own_hue() {
    for name in ["Sapphire", "Ruby", "Emerald"] {
        let entry = &typical_patterns(name)[0];
        assert_eq!(entry.template, TemplateId::HexagonalBanding);
        let own = [40.0, 45.0, 265.0];
        let pattern = built(entry, own, follows());
        assert_eq!(pattern.zones.len(), 3, "{name}");
        let mut outer_before = 0.0;
        for zone in &pattern.zones {
            let ZoneShape::CoaxialPrism {
                axis_dir,
                n_sides,
                r_in,
                r_out,
                ..
            } = zone.shape
            else {
                panic!("{name}: {} is not a prism", zone.name);
            };
            assert_eq!((n_sides, axis_dir), (6, DVec3::Y), "{name}");
            assert!(
                r_in > outer_before,
                "{name}: shells overlap or touch the axis"
            );
            assert!(r_out > r_in && r_out <= 0.5, "{name}: {r_in} {r_out}");
            outer_before = r_out;
        }
        let index = pattern.zones.len();
        let body = single_lch(&pattern, 0);
        for shell in 1..=index {
            let strong = single_lch(&pattern, shell);
            assert!((strong[2] - own[2]).abs() < 1e-9 && (body[2] - own[2]).abs() < 1e-9);
            assert!(strong[1] > own[1], "stronger = more chroma than the stone");
            assert!(strong[0] < own[0], "stronger = darker than the stone");
            assert!(body[0] > own[0] && body[1] < own[1], "the body is paler");
        }
    }
}

#[test]
fn a_parti_color_sapphire_has_a_hexagonal_core_in_its_own_color_and_a_yellow_rim() {
    let entry = &typical_patterns("Sapphire")[1];
    let own = [40.0, 45.0, 265.0];
    let pattern = built(entry, own, follows());
    assert_eq!(single_lch(&pattern, 1), own);
    assert_eq!(single_lch(&pattern, 0), [80.0, 60.0, 90.0]);
    assert!(matches!(
        pattern.zones[0].shape,
        ZoneShape::CoaxialPrism {
            n_sides: 6,
            r_in,
            r_out,
            ..
        } if r_in == 0.0 && r_out > 0.0 && r_out <= 0.5
    ));
    // A yellow sapphire keeps its yellow in the core and gets a blue rim instead.
    let yellow = [80.0, 60.0, 90.0];
    let pattern = built(entry, yellow, follows());
    assert_eq!(single_lch(&pattern, 1), yellow);
    assert!(
        (single_lch(&pattern, 0)[2] - 265.0).abs() < 1.0,
        "the rim is blue"
    );
}

#[test]
fn a_color_band_runs_along_the_axis_and_sits_off_center() {
    let entry = &typical_patterns("Sapphire")[2];
    let own = [40.0, 45.0, 265.0];
    let pattern = built(entry, own, follows());
    assert_eq!(pattern.zones.len(), 1);
    let ZoneShape::Slab {
        normal,
        offset_min,
        offset_max,
    } = pattern.zones[0].shape
    else {
        panic!("the band is a slab");
    };
    assert_eq!(normal.dot(DVec3::Y), 0.0, "the band contains the c axis");
    assert!(offset_min > 0.0, "off center");
    assert!(offset_max > offset_min && offset_max <= 0.5);
    assert_eq!(single_lch(&pattern, 0), own);
    assert!(single_lch(&pattern, 1)[1] > own[1], "the band is stronger");
}

#[test]
fn a_beryl_has_a_stronger_hexagonal_core_in_a_paler_rim() {
    let entry = &typical_patterns("Aquamarine")[1];
    assert_eq!(entry.template, TemplateId::CoreAndRim);
    let own = [75.0, 30.0, 220.0];
    let pattern = built(entry, own, follows());
    assert!(matches!(
        pattern.zones[0].shape,
        ZoneShape::CoaxialPrism { n_sides: 6, r_in, .. } if r_in == 0.0
    ));
    assert!(single_lch(&pattern, 1)[1] > own[1]);
    assert!(single_lch(&pattern, 0)[1] < own[1]);
}

#[test]
fn a_tourmaline_keeps_its_own_pink_or_green_and_adds_the_other() {
    let list = typical_patterns("Tourmaline");
    let green = [55.0, 35.0, 140.0];
    let pink = [60.0, 35.0, 0.0];
    let blue = [40.0, 45.0, 265.0];
    // Watermelon: green rim (base), pink core.
    let watermelon = &list[0];
    let pattern = built(watermelon, blue, follows());
    assert_eq!(
        (single_lch(&pattern, 0), single_lch(&pattern, 1)),
        (green, pink)
    );
    let own_green = [48.0, 40.0, 150.0];
    let pattern = built(watermelon, own_green, follows());
    assert_eq!(
        single_lch(&pattern, 0),
        own_green,
        "a green stone keeps its rim"
    );
    assert_eq!(single_lch(&pattern, 1), pink);
    let own_pink = [58.0, 42.0, 350.0];
    let pattern = built(watermelon, own_pink, follows());
    assert_eq!(
        single_lch(&pattern, 1),
        own_pink,
        "a pink stone keeps its core"
    );
    assert_eq!(single_lch(&pattern, 0), green);
    // Reverse Watermelon swaps them.
    let pattern = built(&list[1], blue, follows());
    assert_eq!(
        (single_lch(&pattern, 0), single_lch(&pattern, 1)),
        (pink, green)
    );
    // Bicolor splits along the axis: the half space's normal is the pattern axis.
    let pattern = built(&list[2], blue, follows());
    assert!(matches!(
        pattern.zones[0].shape,
        ZoneShape::HalfSpace { normal, .. } if normal == DVec3::Y
    ));
    // Tricolor stacks two layers across the axis.
    let pattern = built(&list[3], blue, follows());
    assert_eq!(pattern.zones.len(), 2);
    assert!(
        pattern
            .zones
            .iter()
            .all(|zone| matches!(zone.shape, ZoneShape::Slab { normal, .. } if normal == DVec3::Y))
    );
}

#[test]
fn quartz_sectors_are_three_wedges_of_sixty_degrees() {
    for (name, wedge_hue) in [("Amethyst", 265.0), ("Ametrine", 305.0), ("Citrine", 305.0)] {
        let entry = typical_patterns(name)
            .iter()
            .find(|entry| entry.template == TemplateId::Sectors)
            .expect("sectors");
        let own = [40.0, 45.0, 265.0];
        let pattern = built(entry, own, follows());
        assert_eq!(pattern.zones.len(), 3, "{name}");
        for (k, zone) in pattern.zones.iter().enumerate() {
            let ZoneShape::Sector {
                axis_dir,
                angle_from,
                angle_to,
                ..
            } = zone.shape
            else {
                panic!("{name}: {} is not a sector", zone.name);
            };
            assert_eq!(axis_dir, DVec3::Y);
            assert!((angle_to - angle_from - FRAC_PI_3).abs() < 1e-12, "{name}");
            let start = TAU / 3.0 * f64::from(u8::try_from(k).expect("small"));
            assert!((angle_from - start).abs() < 1e-12, "{name}: {k}");
            assert!(
                (single_lch(&pattern, k + 1)[2] - wedge_hue).abs() < 1e-9,
                "{name}"
            );
        }
    }
    // Amethyst: own hue on a paler body.
    let pattern = built(
        &typical_patterns("Amethyst")[0],
        [48.0, 42.0, 305.0],
        follows(),
    );
    assert!(single_lch(&pattern, 0)[1] < single_lch(&pattern, 1)[1]);
}

#[test]
fn citrine_and_ametrine_split_in_purple_and_yellow_and_amethyst_in_strong_and_pale() {
    for name in ["Citrine", "Ametrine"] {
        let pattern = built(&typical_patterns(name)[0], [95.0, 0.0, 0.0], follows());
        assert_eq!(pattern.template, Some(TemplateId::Split));
        assert_eq!(single_lch(&pattern, 0), [48.0, 42.0, 305.0], "{name}");
        assert_eq!(single_lch(&pattern, 1), [82.0, 72.0, 88.0], "{name}");
    }
    let own = [48.0, 42.0, 305.0];
    let pattern = built(&typical_patterns("Amethyst")[1], own, follows());
    assert_eq!(single_lch(&pattern, 1), own);
    assert!(single_lch(&pattern, 0)[0] > own[0]);
}

#[test]
fn fluorite_bands_across_the_axis_and_topaz_is_own_hue_over_near_colorless() {
    let own = [45.0, 40.0, 300.0];
    let pattern = built(&typical_patterns("Fluorite")[0], own, follows());
    assert_eq!(pattern.zones.len(), 4);
    assert!(
        pattern
            .zones
            .iter()
            .all(|zone| matches!(zone.shape, ZoneShape::Slab { normal, .. } if normal == DVec3::Y))
    );
    assert!(single_lch(&pattern, 0)[1] < own[1]);
    let pattern = built(&typical_patterns("Topaz")[0], own, follows());
    assert_eq!(single_lch(&pattern, 1), own);
    assert!(single_lch(&pattern, 0)[1] <= 4.0 && single_lch(&pattern, 0)[0] > 90.0);
}

#[test]
fn the_typical_only_templates_build_for_a_blue_sapphire_and_stay_out_of_the_generic_list() {
    for id in TemplateId::TYPICAL_ONLY {
        assert!(!TemplateId::ALL.contains(&id), "{}", id.label());
        let pattern = template(id, PatternAxis::TABLE_NORMAL);
        assert_eq!(pattern.template, Some(id));
        assert!(!pattern.zones.is_empty());
        assert!(pattern.relative_to_stone);
    }
}

#[test]
fn patterns_written_before_the_typical_templates_still_load_and_the_new_ones_round_trip() {
    // The names an older build wrote are unchanged.
    for id in TemplateId::ALL {
        let text = encode_pattern(&template(id, PatternAxis::TABLE_NORMAL)).expect("encodes");
        assert!(
            text.contains(&format!("\"template\":\"{id:?}\"")),
            "{}: {text}",
            id.label()
        );
        assert_eq!(decode_pattern(&text).expect("decodes").template, Some(id));
    }
    // The layout of the text did not move either: the version and the template lead it.
    let text = encode_pattern(&template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL))
        .expect("encodes");
    assert!(
        text.starts_with(r#"{"version":1,"template":"Watermelon","base":"#),
        "{text}"
    );
    // The new ones round trip by name.
    for id in TemplateId::TYPICAL_ONLY {
        let pattern = template(id, follows());
        let text = encode_pattern(&pattern).expect("encodes");
        assert!(text.contains(&format!("\"template\":\"{id:?}\"")), "{text}");
        assert_eq!(decode_pattern(&text).expect("decodes"), pattern);
    }
    // Every typical pattern survives the codec too.
    for name in SPECIES {
        for entry in typical_patterns(name) {
            let pattern = built(entry, OWN_COLORS[0], follows());
            let text = encode_pattern(&pattern).expect("encodes");
            assert_eq!(decode_pattern(&text).expect("decodes"), pattern);
        }
    }
}

#[test]
fn a_pattern_made_for_a_stone_turns_with_its_crystal_axis() {
    for name in SPECIES {
        // A cubic species has no Crystal Axis: its bands stand upright in the stone instead.
        let want = if name == "Fluorite" {
            PatternAxis::ACROSS
        } else {
            follows()
        };
        for entry in typical_patterns(name) {
            let pattern = entry.for_stone(OWN_COLORS[0], &[]);
            assert_eq!(pattern.axis, want, "{name}");
            assert_eq!(entry.axis, want, "{name}");
            assert_eq!(pattern, built(entry, OWN_COLORS[0], want), "{name}");
        }
    }
}

#[test]
fn fluorite_bands_stand_upright_so_they_show_face_up() {
    for entry in typical_patterns("Fluorite") {
        let frame = entry.for_stone(OWN_COLORS[0], &[]).axis.frame();
        // The layers are across the pattern axis; that axis lies in the table plane, so every
        // layer boundary contains the table normal.
        let axis = frame.direction(glam::DVec3::Y);
        assert!(axis.y.abs() < 1e-12, "{}: {axis:?}", entry.label);
    }
}

/// A pleochroic per-millimeter variant of `material` under `key`.
fn variant(material: &str, key: &str, peak: f32) -> ColorVariant {
    ColorVariant {
        key: key.to_owned(),
        label: "Test".to_owned(),
        material: material.to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(540.0, 40.0, peak)],
            vec![AbsorptionBand::new(600.0, 50.0, peak * 0.5)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
        provenance: VariantProvenance::User,
    }
}

/// The built-in `sapphire.yellow` variant (per millimeter, pleochroic).
fn sapphire_yellow() -> ColorVariant {
    built_in_variants("Sapphire")
        .into_iter()
        .find(|variant| variant.key == "sapphire.yellow")
        .expect("the data file lists the yellow sapphire")
}

fn variant_key(pattern: &ColorPattern, zone: usize) -> Option<String> {
    pattern
        .zone(zone)
        .expect("zone")
        .color
        .variant_key()
        .map(str::to_owned)
}

#[test]
fn a_parti_color_sapphire_takes_the_yellow_variant_for_its_rim() {
    let yellow = sapphire_yellow();
    let entry = &typical_patterns("Sapphire")[1];
    let own = [40.0, 45.0, 265.0];
    let pattern = entry.for_stone(own, std::slice::from_ref(&yellow));
    // The rim (the base zone) is the yellow variant, the blue core stays the stone's own color.
    assert_eq!(variant_key(&pattern, 0).as_deref(), Some("sapphire.yellow"));
    assert_eq!(single_lch(&pattern, 1), own);
    assert_eq!(pattern.version, PATTERN_FORMAT_VERSION);
    assert_eq!(
        pattern.unsolved_colors(),
        [crate::material::pattern::ColorRef {
            zone: 1,
            slot: crate::material::pattern::ColorSlot::Single
        }],
        "the variant is solved by construction, only the core still has to be solved"
    );
    // Once the core is solved the pattern renders, and the rim's tensor is the variant's.
    let pattern = filled(pattern);
    let zoned = pattern.to_zoned().expect("valid").expect("zones");
    assert_eq!(zoned.base.tensor, yellow.absorption);
    assert!(zoned.base.tensor.is_pleochroic);
    let host = apply_color_pattern(GemMaterial::sapphire(), &pattern, 7.0).expect("applies");
    assert_eq!(host.absorption_unit, AbsorptionUnit::PerMm);

    // Without the variant the rim is today's `[L*, C*, h]` seed, and the version stays the first.
    let plain = entry.for_stone(own, &[]);
    assert_eq!(single_lch(&plain, 0), [80.0, 60.0, 90.0]);
    assert_eq!(plain.version, PATTERN_BASE_FORMAT_VERSION);
    // A variant of another key is not picked.
    let other = variant("Sapphire", "sapphire.other", 0.1);
    assert_eq!(
        entry.for_stone(own, &[other]),
        plain,
        "only the table's keys are used"
    );
}

#[test]
fn a_yellow_sapphire_gets_the_yellow_variant_in_its_core_and_a_blue_rim_from_the_blue_variant() {
    let yellow = sapphire_yellow();
    let blue = variant("Sapphire", "sapphire.default", 0.2);
    let entry = &typical_patterns("Sapphire")[1];
    let own_yellow = [80.0, 60.0, 90.0];
    let pattern = entry.for_stone(own_yellow, &[yellow, blue]);
    assert_eq!(
        variant_key(&pattern, 0).as_deref(),
        Some("sapphire.default")
    );
    assert_eq!(variant_key(&pattern, 1).as_deref(), Some("sapphire.yellow"));
    assert!(
        pattern.unsolved_colors().is_empty(),
        "both zones are variants"
    );
    pattern.to_zoned().expect("valid").expect("zones");
}

#[test]
fn a_tourmaline_takes_rubellite_and_verdelite_variants_where_it_has_them() {
    let rubellite = variant("Tourmaline", "tourmaline.rubellite", 0.3);
    let verdelite = variant("Tourmaline", "tourmaline.verdelite", 0.2);
    let both = [rubellite.clone(), verdelite];
    let list = typical_patterns("Tourmaline");
    let blue = [40.0, 45.0, 265.0];
    // Watermelon: the green rim (base) is verdelite, the pink core rubellite.
    let pattern = list[0].for_stone(blue, &both);
    assert_eq!(
        variant_key(&pattern, 0).as_deref(),
        Some("tourmaline.verdelite")
    );
    assert_eq!(
        variant_key(&pattern, 1).as_deref(),
        Some("tourmaline.rubellite")
    );
    assert!(pattern.unsolved_colors().is_empty());
    assert_eq!(pattern.version, PATTERN_FORMAT_VERSION);
    // Reverse Watermelon swaps them, Tricolor keeps the pale middle as a seed.
    let reverse = list[1].for_stone(blue, &both);
    assert_eq!(
        variant_key(&reverse, 0).as_deref(),
        Some("tourmaline.rubellite")
    );
    let tricolor = list[3].for_stone(blue, &both);
    assert_eq!(tricolor.base.color.variant_key(), None);
    assert_eq!(single_lch(&tricolor, 0), [90.0, 8.0, 90.0]);
    // A green stone keeps its own green as the rim and only takes the rubellite for the core.
    let own_green = [48.0, 40.0, 150.0];
    let pattern = list[0].for_stone(own_green, &both);
    assert_eq!(single_lch(&pattern, 0), own_green);
    assert_eq!(
        variant_key(&pattern, 1).as_deref(),
        Some("tourmaline.rubellite")
    );
    // With only the rubellite, the green rim stays the seed.
    let pattern = list[0].for_stone(blue, &[rubellite]);
    assert_eq!(single_lch(&pattern, 0), [55.0, 35.0, 140.0]);
}

#[test]
fn an_ametrine_takes_amethyst_and_citrine_variants_where_it_has_them() {
    let amethyst = variant("Ametrine", "ametrine.amethyst", 0.3);
    let citrine = variant("Ametrine", "ametrine.citrine", 0.2);
    let both = [amethyst, citrine];
    let list = typical_patterns("Ametrine");
    let split = list[0].for_stone([95.0, 0.0, 0.0], &both);
    assert_eq!(variant_key(&split, 0).as_deref(), Some("ametrine.amethyst"));
    assert_eq!(variant_key(&split, 1).as_deref(), Some("ametrine.citrine"));
    let sectors = list[1].for_stone([95.0, 0.0, 0.0], &both);
    assert_eq!(
        variant_key(&sectors, 0).as_deref(),
        Some("ametrine.citrine")
    );
    for zone in 1..=3 {
        assert_eq!(
            variant_key(&sectors, zone).as_deref(),
            Some("ametrine.amethyst")
        );
    }
    sectors.to_zoned().expect("valid").expect("zones");
}

#[test]
fn typical_patterns_with_variants_survive_the_codec() {
    let both = [
        variant("Tourmaline", "tourmaline.rubellite", 0.3),
        variant("Tourmaline", "tourmaline.verdelite", 0.2),
    ];
    let pattern = typical_patterns("Tourmaline")[0].for_stone([40.0, 45.0, 265.0], &both);
    let text = encode_pattern(&pattern).expect("encodes");
    assert!(text.starts_with(r#"{"version":2,"#), "{text}");
    assert_eq!(decode_pattern(&text).expect("decodes"), pattern);
}

#[test]
fn the_base_lch_is_the_base_zones_color() {
    let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
    assert_eq!(pattern.base_lch(), [55.0, 35.0, 140.0]);
    pattern.set_two_colors(0, true);
    assert_eq!(pattern.base_lch(), [55.0, 35.0, 140.0]);
}

// ---- pleochroic stones ----------------------------------------------------------------------

/// The Along and Across `[L*, C*, h]` of a blue sapphire's directions.
const UNIAXIAL_AXES: [[f64; 3]; 2] = [[33.0, 34.0, 289.0], [54.0, 21.0, 276.0]];
/// The Alpha, Beta and Gamma `[L*, C*, h]` of a biaxial stone.
const BIAXIAL_AXES: [[f64; 3]; 3] = [[45.0, 30.0, 20.0], [60.0, 25.0, 100.0], [35.0, 40.0, 260.0]];

/// The Sapphire's Material Default variant (the first of its list).
fn sapphire_default() -> ColorVariant {
    built_in_variants("Sapphire")
        .into_iter()
        .next()
        .expect("the Material Default comes first")
}

/// The colors of a pattern's zones, base first.
fn zone_colors(pattern: &ColorPattern) -> Vec<&PatternColor> {
    std::iter::once(&pattern.base)
        .chain(&pattern.zones)
        .map(|zone| &zone.color)
        .collect()
}

/// Whether two `[L*, C*, h]` are the same to a thousandth.
fn same_lch(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

/// The `[L*, C*, h]` of the Along and Across colors of a two-color zone color.
fn along_across(color: &PatternColor) -> [[f64; 3]; 2] {
    let PatternColor::Dichroic { along, across } = color else {
        panic!("a uniaxial stone's zone has Along and Across colors: {color:?}");
    };
    [along.lch, across.lch]
}

#[test]
fn an_own_color_has_one_to_three_directions_and_a_default() {
    let one = OwnColor::single([40.0, 45.0, 265.0]);
    assert_eq!(one.axes(), [[40.0, 45.0, 265.0]]);
    assert_eq!(one.directions(), 1);
    assert_eq!(one.variant(), None);
    let two = OwnColor::per_axis(&UNIAXIAL_AXES, None);
    assert_eq!((two.directions(), two.lch()), (2, UNIAXIAL_AXES[0]));
    let three = OwnColor::per_axis(&BIAXIAL_AXES, None);
    assert_eq!(three.directions(), 3);
    // More than three directions cannot be told apart: the first three count.
    let many = [
        BIAXIAL_AXES[0],
        BIAXIAL_AXES[1],
        BIAXIAL_AXES[2],
        [10.0, 5.0, 5.0],
    ];
    assert_eq!(OwnColor::per_axis(&many, None).axes(), BIAXIAL_AXES);
    // No direction at all is the default blue sapphire.
    assert_eq!(OwnColor::per_axis(&[], None).axes(), [[40.0, 45.0, 265.0]]);
}

#[test]
fn hexagonal_banding_on_a_sapphire_has_no_single_zone() {
    let default = sapphire_default();
    let own = OwnColor::per_axis(&UNIAXIAL_AXES, Some(PatternColor::from_variant(&default)));
    let entry = &typical_patterns("Sapphire")[0];
    assert_eq!(entry.template, TemplateId::HexagonalBanding);
    let pattern = entry.for_stone_on(&own, std::slice::from_ref(&default));
    let colors = zone_colors(&pattern);
    assert_eq!(colors.len(), 4);
    assert!(
        colors
            .iter()
            .all(|color| !matches!(color, PatternColor::Single(_))),
        "a pleochroic stone has no one-color zone: {colors:?}"
    );
    // The paler body and the stronger growth zones are built direction by direction.
    let [along, across] = along_across(colors[0]);
    assert!(same_lch(along, [65.0, 15.3, 289.0]), "{along:?}");
    assert!(same_lch(across, [75.5, 9.45, 276.0]), "{across:?}");
    for color in &colors[1..] {
        let [along, across] = along_across(color);
        assert!(same_lch(along, [28.05, 44.2, 289.0]), "{along:?}");
        assert!(same_lch(across, [45.9, 27.3, 276.0]), "{across:?}");
    }
    // Two colors per zone are still a first-release pattern.
    assert_eq!(pattern.version, PATTERN_BASE_FORMAT_VERSION);
    assert_eq!(pattern.unsolved_colors().len(), 8);
    let zoned = filled(pattern).to_zoned().expect("valid").expect("zones");
    assert_eq!(zoned.validate(), Ok(()));
}

#[test]
fn a_typical_pattern_of_a_pleochroic_stone_never_has_a_single_zone() {
    let default = PatternColor::from_variant(&sapphire_default());
    for name in SPECIES {
        for entry in typical_patterns(name) {
            for (axes, directions) in [(&UNIAXIAL_AXES[..], 2), (&BIAXIAL_AXES[..], 3)] {
                for own_variant in [None, Some(default.clone())] {
                    let own = OwnColor::per_axis(axes, own_variant);
                    let pattern = entry.for_stone_on(&own, &[]);
                    let context = format!("{name} / {} / {directions} colors", entry.label);
                    for color in zone_colors(&pattern) {
                        match (color, directions) {
                            (PatternColor::Dichroic { .. }, 2)
                            | (PatternColor::Trichroic { .. }, 3)
                            | (PatternColor::Variant { .. }, _) => {}
                            (other, _) => panic!("{context}: {other:?}"),
                        }
                    }
                    let zoned = filled(pattern)
                        .to_zoned()
                        .unwrap_or_else(|error| panic!("{context}: {error}"))
                        .unwrap_or_else(|| panic!("{context}: no zones"));
                    assert_eq!(zoned.validate(), Ok(()), "{context}");
                }
            }
        }
    }
}

#[test]
fn a_stone_whose_color_is_a_variant_keeps_it_in_the_zones_of_its_own_color() {
    let default = sapphire_default();
    let own = OwnColor::per_axis(&UNIAXIAL_AXES, Some(PatternColor::from_variant(&default)));
    // Color Band: the body is the stone's own variant, only the band is a stronger shade.
    let band = typical_patterns("Sapphire")[2].for_stone_on(&own, &[]);
    assert_eq!(variant_key(&band, 0).as_deref(), Some("sapphire.default"));
    assert!(matches!(band.zones[0].color, PatternColor::Dichroic { .. }));
    assert_eq!(band.version, PATTERN_FORMAT_VERSION);
    // Parti Color: the core is the stone's variant, the rim is a fixed yellow in both directions.
    let parti = typical_patterns("Sapphire")[1].for_stone_on(&own, &[]);
    assert_eq!(variant_key(&parti, 1).as_deref(), Some("sapphire.default"));
    assert_eq!(along_across(&parti.base.color), [[80.0, 60.0, 90.0]; 2]);
}

#[test]
fn a_one_color_stone_builds_the_same_pattern_either_way() {
    let own = [40.0, 45.0, 265.0];
    for name in SPECIES {
        for entry in typical_patterns(name) {
            assert_eq!(
                entry.for_stone_on(&OwnColor::single(own), &[]),
                entry.for_stone(own, &[]),
                "{name}"
            );
        }
    }
    // A stone without pleochroism has one-color zones only (the first release's patterns).
    let pattern = typical_patterns("Sapphire")[0].for_stone(own, &[]);
    assert!(
        zone_colors(&pattern)
            .iter()
            .all(|color| matches!(color, PatternColor::Single(ZoneColor { .. })))
    );
}
