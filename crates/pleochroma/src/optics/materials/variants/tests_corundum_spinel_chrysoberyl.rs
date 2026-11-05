//! Tests of the `data/color_variants/corundum_spinel_chrysoberyl.toml` entries: each entry's face-up color and
//! per-direction colors against the numbers its fit record
//! (`docs/color_variants/fit_records/corundum_spinel_chrysoberyl.md`) states.
//!
//! The colors are the app's own `color::body_color::body_colors` at the face-up path of a 7 mm stone
//! ([`face_up_swatch_path`]: 2.83 x 7 / 2.0 = 9.905 mm for a per-millimetre tensor), CIELAB, relative to
//! the illuminant's own white. Per direction: `o_ray` is the ordinary ray / alpha (E||c for biaxial),
//! `beta_ray` beta (E||a), `e_ray` the extraordinary ray / gamma (E||b). The pinned face-up color is the
//! ORIENTED one the renderer shows (`color::principal_axes::face_up_body_color` with the host's own
//! default principal frame: the o-ray alone for Sapphire and Ruby, whose c axis is the view, alpha and
//! beta for Alexandrite and Chrysoberyl, whose gamma axis is the view; the single color of cubic
//! Spinel); the unpolarised mean is the fit target the record also states. The reference spectra are
//! not read here: the pinned numbers are the fit record's.

use std::collections::BTreeSet;

use super::{
    ColorVariant, VariantProvenance, built_in_variants, face_up_swatch_path, material_slug,
};
use crate::{
    color::{
        body_color::{BodyColors, Illuminant, body_colors, delta_e_2000, lab_to_lch},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::materials::{AbsorptionUnit, GemMaterial},
};

/// Largest CIEDE2000 distance between the app's color and the pinned one.
const TOLERANCE: f64 = 1.0;

/// The built-in materials this group's data file carries variants for.
const MATERIALS: [&str; 5] = [
    "Sapphire",
    "Ruby",
    "Spinel",
    "Chrysoberyl (Yellow)",
    "Alexandrite",
];

/// Reference stone width of every entry, millimetres.
const STONE_WIDTH_MM: f32 = 7.0;

/// The fit record link every entry of this file carries.
const FIT_RECORD_PREFIX: &str = "docs/color_variants/fit_records/corundum_spinel_chrysoberyl.md#";

/// One entry's pinned colors (CIELAB, as the fit record states them).
struct Pinned {
    key: &'static str,
    estimated: bool,
    /// Face-up (oriented, as the renderer shows it), D65.
    face_up_d65: [f64; 3],
    /// Face-up (oriented), Planckian 3200 K (the Incandescent preset).
    face_up_3200k: [f64; 3],
    /// Ordinary ray / alpha, D65.
    o_ray: [f64; 3],
    /// Beta ray, D65 (biaxial entries only).
    beta_ray: Option<[f64; 3]>,
    /// Extraordinary ray / gamma, D65.
    e_ray: [f64; 3],
}

/// Every entry of the group file, in file order.
const PINNED: [Pinned; 21] = [
    Pinned {
        key: "sapphire.pink",
        estimated: false,
        face_up_d65: [68.96, 38.82, -24.33],
        face_up_3200k: [70.66, 43.07, -22.93],
        o_ray: [68.96, 38.82, -24.33],
        beta_ray: None,
        e_ray: [86.94, 13.02, 7.72],
    },
    Pinned {
        key: "sapphire.yellow",
        estimated: false,
        face_up_d65: [86.84, 5.91, 61.70],
        face_up_3200k: [89.52, 14.83, 62.99],
        o_ray: [86.84, 5.91, 61.70],
        beta_ray: None,
        e_ray: [91.65, -1.09, 51.01],
    },
    Pinned {
        key: "sapphire.golden_yellow",
        estimated: true,
        face_up_d65: [77.49, 20.15, 90.76],
        face_up_3200k: [81.68, 27.94, 92.94],
        o_ray: [77.49, 20.15, 90.76],
        beta_ray: None,
        e_ray: [85.14, 7.61, 80.00],
    },
    Pinned {
        key: "sapphire.padparadscha",
        estimated: true,
        face_up_d65: [73.54, 30.55, 18.26],
        face_up_3200k: [76.82, 36.69, 22.77],
        o_ray: [73.54, 30.55, 18.26],
        beta_ray: None,
        e_ray: [86.89, 12.53, 25.81],
    },
    Pinned {
        key: "sapphire.orange",
        estimated: true,
        face_up_d65: [81.12, 21.90, 72.81],
        face_up_3200k: [85.31, 27.88, 78.11],
        o_ray: [81.12, 21.90, 72.81],
        beta_ray: None,
        e_ray: [85.80, 12.36, 52.59],
    },
    Pinned {
        key: "sapphire.cornflower",
        estimated: true,
        face_up_d65: [63.72, 11.20, -39.46],
        face_up_3200k: [61.90, -0.05, -41.03],
        o_ray: [63.72, 11.20, -39.46],
        beta_ray: None,
        e_ray: [81.17, -5.01, -10.12],
    },
    Pinned {
        key: "sapphire.medium_blue",
        estimated: true,
        face_up_d65: [47.32, 21.49, -54.57],
        face_up_3200k: [44.63, 2.11, -57.00],
        o_ray: [47.32, 21.49, -54.57],
        beta_ray: None,
        e_ray: [70.51, -6.38, -15.08],
    },
    Pinned {
        key: "sapphire.deep_blue",
        estimated: true,
        face_up_d65: [32.28, 35.55, -65.22],
        face_up_3200k: [28.82, 8.45, -68.47],
        o_ray: [32.28, 35.55, -65.22],
        beta_ray: None,
        e_ray: [58.93, -7.26, -19.69],
    },
    Pinned {
        key: "sapphire.purple",
        estimated: true,
        face_up_d65: [58.76, 29.44, -35.55],
        face_up_3200k: [58.78, 28.95, -36.15],
        o_ray: [58.76, 29.44, -35.55],
        beta_ray: None,
        e_ray: [80.15, 5.40, 1.82],
    },
    Pinned {
        key: "sapphire.green",
        estimated: true,
        face_up_d65: [46.49, -13.40, 15.66],
        face_up_3200k: [45.96, -3.84, 11.51],
        o_ray: [46.49, -13.40, 15.66],
        beta_ray: None,
        e_ray: [70.00, -19.62, 44.81],
    },
    Pinned {
        key: "sapphire.colorless",
        estimated: true,
        face_up_d65: [100.00, 0.00, 0.00],
        face_up_3200k: [100.00, 0.00, 0.00],
        o_ray: [100.00, 0.00, 0.00],
        beta_ray: None,
        e_ray: [100.00, 0.00, 0.00],
    },
    Pinned {
        key: "ruby.pinkish_red",
        estimated: false,
        face_up_d65: [46.21, 64.85, -31.65],
        face_up_3200k: [50.14, 68.50, -27.08],
        o_ray: [46.21, 64.85, -31.65],
        beta_ray: None,
        e_ray: [69.04, 39.22, 7.08],
    },
    Pinned {
        key: "ruby.thai",
        estimated: true,
        face_up_d65: [29.72, 59.62, 4.01],
        face_up_3200k: [35.60, 67.10, 10.08],
        o_ray: [29.72, 59.62, 4.01],
        beta_ray: None,
        e_ray: [42.21, 59.65, 17.96],
    },
    Pinned {
        key: "ruby.pigeons_blood",
        estimated: true,
        face_up_d65: [27.26, 64.35, -10.43],
        face_up_3200k: [33.17, 67.08, -2.15],
        o_ray: [27.26, 64.35, -10.43],
        beta_ray: None,
        e_ray: [44.70, 68.19, 15.56],
    },
    Pinned {
        key: "spinel.pink",
        estimated: true,
        face_up_d65: [74.32, 33.12, 2.46],
        face_up_3200k: [77.24, 35.95, 6.27],
        o_ray: [74.32, 33.12, 2.46],
        beta_ray: None,
        e_ray: [74.32, 33.12, 2.46],
    },
    Pinned {
        key: "spinel.cobalt_blue",
        estimated: true,
        face_up_d65: [55.44, -1.94, -64.78],
        face_up_3200k: [49.10, -17.12, -75.42],
        o_ray: [55.44, -1.94, -64.78],
        beta_ray: None,
        e_ray: [55.44, -1.94, -64.78],
    },
    Pinned {
        key: "spinel.lavender",
        estimated: false,
        face_up_d65: [71.68, 3.71, -4.92],
        face_up_3200k: [71.67, 5.42, -5.48],
        o_ray: [71.68, 3.71, -4.92],
        beta_ray: None,
        e_ray: [71.68, 3.71, -4.92],
    },
    Pinned {
        key: "spinel.purple",
        estimated: true,
        face_up_d65: [61.74, 15.97, -24.59],
        face_up_3200k: [61.53, 18.25, -25.91],
        o_ray: [61.74, 15.97, -24.59],
        beta_ray: None,
        e_ray: [61.74, 15.97, -24.59],
    },
    Pinned {
        key: "chrysoberyl__yellow_.golden",
        estimated: true,
        face_up_d65: [73.74, 2.03, 47.98],
        face_up_3200k: [75.64, 12.64, 46.32],
        o_ray: [71.57, 5.95, 56.15],
        beta_ray: Some([75.80, -1.60, 41.83]),
        e_ray: [68.69, 15.51, 64.84],
    },
    Pinned {
        key: "alexandrite.brazilian",
        estimated: true,
        face_up_d65: [54.50, 7.41, 25.97],
        face_up_3200k: [55.89, 26.44, 21.11],
        o_ray: [38.65, 21.89, -1.45],
        beta_ray: Some([65.31, 0.58, 44.70]),
        e_ray: [40.49, -68.60, -2.95],
    },
    Pinned {
        key: "alexandrite.sri_lankan",
        estimated: true,
        face_up_d65: [69.01, -0.21, 34.04],
        face_up_3200k: [70.16, 15.66, 29.58],
        o_ray: [57.81, 9.49, 17.15],
        beta_ray: Some([77.84, -6.50, 46.96]),
        e_ray: [53.00, -49.14, 14.65],
    },
];

/// The variant `key` of its material, from the shipped data.
fn variant(key: &str) -> ColorVariant {
    MATERIALS
        .iter()
        .flat_map(|material| built_in_variants(material))
        .find(|v| v.key == key)
        .unwrap_or_else(|| panic!("no built-in variant {key}"))
}

/// The app's colors of `variant` at the face-up path of a 7 mm stone, under `illuminant`.
fn colors(variant: &ColorVariant, illuminant: Illuminant) -> BodyColors {
    let path = face_up_swatch_path(variant.unit, STONE_WIDTH_MM);
    body_colors(&variant.absorption, path, illuminant)
}

/// The face-up color the renderer shows for `variant` on its host (D-G of the 2026-10-27 fix round): the
/// two eigenmodes of the host's own default principal frame at the wave normal `k = -Y`, so c along the
/// view shows the o-ray alone, c in the table plane the mean of o and e, and a biaxial host with gamma
/// along the view the mean of alpha and beta; an isotropic or non-pleochroic tensor shows its single
/// color. The fit records state the unpolarised mean as the fit target and this color as the reached
/// face-up color.
fn face_up_oriented(variant: &ColorVariant, illuminant: Illuminant) -> [f64; 3] {
    let host = GemMaterial::built_in()
        .iter()
        .find(|m| m.name == variant.material)
        .unwrap_or_else(|| panic!("no built-in host {}", variant.material));
    let frame = PrincipalFrame::of_material(host);
    face_up_body_color(
        &variant.absorption,
        Some(&frame),
        face_up_swatch_path(variant.unit, STONE_WIDTH_MM),
        illuminant,
    )
    .lab
}

fn assert_close(what: &str, key: &str, app: [f64; 3], pinned: [f64; 3]) {
    let distance = delta_e_2000(app, pinned);
    assert!(
        distance < TOLERANCE,
        "{key} {what}: app {app:?} vs record {pinned:?} (dE2000 {distance:.2})"
    );
}

/// Hue angle of a CIELAB color in degrees, `[0, 360)`.
fn hue(lab: [f64; 3]) -> f64 {
    lab_to_lch(lab)[2]
}

/// Chroma of a CIELAB color.
fn chroma(lab: [f64; 3]) -> f64 {
    lab_to_lch(lab)[1]
}

#[test]
fn the_pinned_keys_are_exactly_the_entries_of_the_group() {
    let listed: BTreeSet<String> = MATERIALS
        .iter()
        .flat_map(|material| built_in_variants(material).into_iter().skip(1))
        .map(|v| v.key)
        .collect();
    let pinned: BTreeSet<String> = PINNED.iter().map(|p| p.key.to_string()).collect();
    assert_eq!(listed, pinned);
    // Dropped by owner decision 2026-10-27: Spinel Grey and Chrysoberyl Honey have no usable data,
    // and the Yellow chrysoberyl variant became the Chrysoberyl (Yellow) Material Default (owner
    // decision O1). Strong Change became the Alexandrite Material Default on 2026-11-03, and Greenish
    // Yellow the Chrysoberyl (Yellow) one the same day (the dealer photos), the former default now Golden.
    for dropped in [
        "spinel.grey",
        "chrysoberyl__yellow_.yellow",
        "chrysoberyl__yellow_.honey",
        "alexandrite.strong_change",
        "chrysoberyl__yellow_.greenish_yellow",
    ] {
        assert!(!pinned.contains(dropped), "{dropped} is dropped");
    }
}

#[test]
fn every_entry_is_per_mm_at_7_mm_with_its_flag_and_a_fit_record() {
    for pinned in &PINNED {
        let v = variant(pinned.key);
        assert_eq!(v.unit, AbsorptionUnit::PerMm, "{}", pinned.key);
        assert!(
            (v.reference_path_mm - STONE_WIDTH_MM).abs() < f32::EPSILON,
            "{}",
            pinned.key
        );
        let slug = material_slug(&v.material);
        assert!(
            pinned.key.starts_with(&format!("{slug}.")),
            "{}",
            pinned.key
        );
        let VariantProvenance::BuiltIn {
            sources,
            fit_record,
            estimated,
        } = &v.provenance
        else {
            panic!("{}: a data entry is BuiltIn", pinned.key);
        };
        assert_eq!(*estimated, pinned.estimated, "{}", pinned.key);
        assert!(!sources.is_empty(), "{}", pinned.key);
        let link = fit_record.as_deref().unwrap_or_default();
        assert!(
            link.starts_with(FIT_RECORD_PREFIX),
            "{}: {link}",
            pinned.key
        );
        assert!(link.len() > FIT_RECORD_PREFIX.len(), "{}", pinned.key);
    }
}

#[test]
fn face_up_and_per_direction_colors_match_the_fit_records() {
    for pinned in &PINNED {
        let v = variant(pinned.key);
        let d65 = colors(&v, Illuminant::D65);
        assert_close(
            "face-up D65",
            pinned.key,
            face_up_oriented(&v, Illuminant::D65),
            pinned.face_up_d65,
        );
        assert_close("o ray D65", pinned.key, d65.o_ray.lab, pinned.o_ray);
        assert_close("e ray D65", pinned.key, d65.e_ray.lab, pinned.e_ray);
        match (&d65.beta_ray, pinned.beta_ray) {
            (Some(beta), Some(expected)) => {
                assert_close("beta ray D65", pinned.key, beta.lab, expected);
            }
            (None, None) => {}
            _ => panic!("{}: beta ray presence differs from the record", pinned.key),
        }
        assert_close(
            "face-up 3200 K",
            pinned.key,
            face_up_oriented(&v, Illuminant::Planckian(3200.0)),
            pinned.face_up_3200k,
        );
    }
}

#[test]
fn a_corundum_ordinary_ray_is_darker_than_the_extraordinary_one() {
    // Documented pleochroism of Cr3+, Fe2+-Ti4+ and the hole centers (GIA 2020): the E perp c ray
    // absorbs more over the visible than the E par c ray.
    for pinned in PINNED
        .iter()
        .filter(|p| p.key.starts_with("sapphire.") || p.key.starts_with("ruby."))
        .filter(|p| p.key != "sapphire.colorless")
    {
        let c = colors(&variant(pinned.key), Illuminant::D65);
        assert!(
            c.o_ray.lab[0] < c.e_ray.lab[0],
            "{}: o {:?} vs e {:?}",
            pinned.key,
            c.o_ray.lab,
            c.e_ray.lab
        );
    }
}

#[test]
fn the_sapphire_blue_family_brackets_the_default_swatch() {
    let default = &built_in_variants("Sapphire")[0];
    let swatch = body_colors(
        &default.absorption,
        face_up_swatch_path(default.unit, STONE_WIDTH_MM),
        Illuminant::D65,
    )
    .unpolarised
    .lab;
    // The built-in Sapphire swatch the Fe2+-Ti4+ density of 12.5 ppma*cm gives (7 mm, D65:
    // L* 50), the royal blue default since 2026-11-03.
    assert!((swatch[0] - 49.9).abs() < 1.0, "{swatch:?}");
    let cornflower = colors(&variant("sapphire.cornflower"), Illuminant::D65)
        .unpolarised
        .lab;
    let medium = colors(&variant("sapphire.medium_blue"), Illuminant::D65)
        .unpolarised
        .lab;
    let deep = colors(&variant("sapphire.deep_blue"), Illuminant::D65)
        .unpolarised
        .lab;
    // Lightest to darkest: Cornflower (6), Medium Blue (10), the default (12.5), Deep Blue (15
    // ppma*cm), each step at least 5 L* below the last but the last (about 6).
    assert!(
        cornflower[0] > medium[0] + 5.0,
        "{cornflower:?} vs {medium:?}"
    );
    assert!(medium[0] > swatch[0] + 5.0, "{medium:?} vs {swatch:?}");
    assert!(deep[0] < swatch[0] - 3.0, "{deep:?} vs {swatch:?}");
    assert!(chroma(swatch) > chroma(medium), "{swatch:?} vs {medium:?}");
    // Violetish blue along, greenish blue across (hue 250-300 deg, the o hue above the e hue).
    for lab in [cornflower, medium, deep] {
        assert!((260.0..310.0).contains(&hue(lab)), "{lab:?}");
    }
    for key in [
        "sapphire.cornflower",
        "sapphire.medium_blue",
        "sapphire.deep_blue",
    ] {
        let c = colors(&variant(key), Illuminant::D65);
        assert!(hue(c.o_ray.lab) > hue(c.e_ray.lab), "{key}");
    }
}

#[test]
fn sapphire_pink_and_yellow_are_the_gia_colors() {
    // GIA Appendix 2 (calculated, D65, unpolarised): Cr3+ at 100 ppma*cm 78.05 / 25.96 / -9.92 and
    // h.-Fe3+ at 1.7 ppma*cm 88.42 / 1.58 / 60.78 (L*a*b*). The pipeline differs a little (CIE 1931
    // table and D65 of this crate), so the distance is a few units, not one.
    let pink = colors(&variant("sapphire.pink"), Illuminant::D65)
        .unpolarised
        .lab;
    assert!(delta_e_2000(pink, [78.05, 25.96, -9.92]) < 3.5, "{pink:?}");
    let yellow = colors(&variant("sapphire.yellow"), Illuminant::D65)
        .unpolarised
        .lab;
    assert!(
        delta_e_2000(yellow, [88.42, 1.58, 60.78]) < 3.5,
        "{yellow:?}"
    );
}

#[test]
fn the_colorless_sapphire_is_empty_and_white() {
    let v = variant("sapphire.colorless");
    assert!(v.absorption.o_ray.is_empty() && v.absorption.e_ray.is_empty());
    assert!(
        !v.absorption.is_pleochroic,
        "an all-empty tensor has no pleochroism to lose (valid on any host)"
    );
    let c = colors(&v, Illuminant::D65);
    for lab in [c.unpolarised.lab, c.o_ray.lab, c.e_ray.lab] {
        assert!(lab[0] > 99.9 && chroma(lab) < 0.1, "{lab:?}");
    }
}

#[test]
fn the_cubic_spinel_variants_are_isotropic_with_equal_sets() {
    for key in [
        "spinel.pink",
        "spinel.cobalt_blue",
        "spinel.lavender",
        "spinel.purple",
    ] {
        let v = variant(key);
        assert!(!v.absorption.is_pleochroic, "{key}");
        assert_eq!(v.absorption.o_ray, v.absorption.e_ray, "{key}");
        assert!(v.absorption.beta_ray.is_none(), "{key}");
        let c = colors(&v, Illuminant::D65);
        assert_eq!(c.unpolarised.lab, c.o_ray.lab, "{key}");
    }
    let hue_of = |key: &str| hue(colors(&variant(key), Illuminant::D65).unpolarised.lab);
    assert!(hue_of("spinel.pink") < 20.0 || hue_of("spinel.pink") > 340.0);
    assert!((250.0..285.0).contains(&hue_of("spinel.cobalt_blue")));
    assert!((290.0..320.0).contains(&hue_of("spinel.purple")));
    assert!((290.0..325.0).contains(&hue_of("spinel.lavender")));
}

#[test]
fn the_golden_chrysoberyl_is_three_ray_and_darker_and_more_golden_than_the_default() {
    let golden = variant("chrysoberyl__yellow_.golden");
    assert!(
        golden.absorption.is_pleochroic && golden.absorption.beta_ray.is_some(),
        "{}",
        golden.key
    );
    let g = colors(&golden, Illuminant::D65);
    // Yellow stones: the gamma (E||b) ray is the strongest absorber in the blue.
    let beta = g.beta_ray.as_ref().expect("a beta ray");
    assert!(chroma(g.e_ray.lab) > chroma(beta.lab));
    // The default is the greenish yellow of the dealer photos since 2026-11-03 (the former Greenish
    // Yellow variant, 0.3 x GRR 171); Golden is the former 0.7 x default: darker, and less green (a
    // smaller hue angle).
    let default = built_in_variants("Chrysoberyl (Yellow)").remove(0);
    let d = colors(&default, Illuminant::D65).unpolarised.lab;
    assert!(g.unpolarised.lab[0] < d[0], "{d:?}");
    assert!(
        hue(d) > hue(g.unpolarised.lab),
        "greener = larger hue angle: {d:?}"
    );
}

#[test]
fn the_alexandrite_rays_follow_the_documented_pleochroism() {
    // Farrell & Newnham: yellow along a (beta), green along b (gamma), red along c (alpha).
    for key in [
        "alexandrite.default",
        "alexandrite.brazilian",
        "alexandrite.sri_lankan",
    ] {
        let c = colors(&variant(key), Illuminant::D65);
        let beta = c.beta_ray.as_ref().expect("a beta ray");
        assert!(
            (80.0..110.0).contains(&hue(beta.lab)),
            "{key}: beta is yellow"
        );
        assert!(
            (140.0..220.0).contains(&hue(c.e_ray.lab)),
            "{key}: gamma is blue-green"
        );
        let alpha = hue(c.o_ray.lab);
        assert!(
            alpha > 300.0 || alpha < 70.0,
            "{key}: alpha is red-orange ({alpha})"
        );
    }
}

#[test]
fn the_alexandrite_variants_change_color_between_daylight_and_incandescent_light() {
    let change = |key: &str| {
        let v = variant(key);
        let day = colors(&v, Illuminant::D65).unpolarised.lab;
        let lamp = colors(&v, Illuminant::Planckian(3200.0)).unpolarised.lab;
        (day, lamp, delta_e_2000(day, lamp))
    };
    // The Material Default is the strong change (the former Strong Change variant, 2026-11-03).
    let (strong_day, strong_lamp, strong) = change("alexandrite.default");
    let (brazil_day, brazil_lamp, brazilian) = change("alexandrite.brazilian");
    let (_, _, sri_lankan) = change("alexandrite.sri_lankan");
    // Green by day, red-purple under the lamp.
    assert!((110.0..170.0).contains(&hue(strong_day)), "{strong_day:?}");
    assert!(
        hue(strong_lamp) > 340.0 || hue(strong_lamp) < 40.0,
        "{strong_lamp:?}"
    );
    // Brazilian: yellower green by day, orange-red under the lamp.
    assert!((95.0..125.0).contains(&hue(brazil_day)), "{brazil_day:?}");
    assert!((15.0..60.0).contains(&hue(brazil_lamp)), "{brazil_lamp:?}");
    // The strong change is the strongest, the Sri Lankan one the weakest.
    assert!(strong > 15.0 && brazilian > 15.0, "{strong} {brazilian}");
    assert!(
        sri_lankan < strong && sri_lankan < brazilian && sri_lankan > 5.0,
        "{sri_lankan}"
    );
}
