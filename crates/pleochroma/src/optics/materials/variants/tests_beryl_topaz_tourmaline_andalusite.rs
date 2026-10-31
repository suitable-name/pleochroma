//! Tests of the `data/color_variants/beryl_topaz_tourmaline_andalusite.toml` entries: each entry's face-up color and
//! per-direction colors against the values recorded in its fit record
//! (`docs/color_variants/fit_records/beryl_topaz_tourmaline_andalusite.md`).
//!
//! The colors are `body_colors` (the app's own colorimetry, D65) of the variant's tensor over the face-up path of a
//! 7 mm stone, [`face_up_swatch_path`] (9.905 mm for a `PerMm` tensor): `MODEL_UNIT_FACE_UP_PATH` x 7 mm / 2.0.
//! The face-up color is the ORIENTED one the renderer shows (`color::principal_axes::face_up_body_color` with the
//! host's own default principal frame): `o` alone for Emerald and Morganite (c along the view), `(o + e) / 2` for
//! Aquamarine and Tourmaline (c in the table plane), `(alpha + beta) / 2` for Topaz and Andalusite (gamma along the
//! view); `o` is `o_ray` (E perpendicular to c / alpha), `e` is `e_ray` (E parallel to c / gamma). The per-direction
//! colors are `body_colors` of the single rays.

use std::collections::BTreeSet;

use super::{VariantProvenance, built_in_variants, face_up_swatch, face_up_swatch_path};
use crate::{
    color::{
        Illuminant, body_colors, delta_e_2000,
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::materials::{AbsorptionUnit, GemMaterial},
};

/// The tolerance of the pins in CIEDE2000.
const TOLERANCE: f64 = 1.0;

/// The 7 mm stone the entries are calibrated for.
const STONE_WIDTH_MM: f32 = 7.0;

/// One pinned entry: the CIELAB values of its fit record.
struct Pin {
    key: &'static str,
    material: &'static str,
    face: [f64; 3],
    o: [f64; 3],
    e: [f64; 3],
    beta: Option<[f64; 3]>,
}

/// Every entry of the group file with the colors its fit record states (`face` is the oriented face-up color).
const PINS: &[Pin] = &[
    Pin {
        key: "emerald.zambian",
        material: "Emerald",
        face: [35.15, -77.19, 31.01],
        o: [35.15, -77.19, 31.01],
        e: [60.70, -113.35, 5.22],
        beta: None,
    },
    Pin {
        key: "emerald.light",
        material: "Emerald",
        face: [83.03, -20.61, 7.50],
        o: [83.03, -20.61, 7.50],
        e: [89.79, -29.30, 1.86],
        beta: None,
    },
    Pin {
        key: "emerald.heliodor",
        material: "Emerald",
        face: [94.22, -9.74, 63.25],
        o: [94.22, -9.74, 63.25],
        e: [61.56, 23.76, 90.31],
        beta: None,
    },
    Pin {
        key: "emerald.red_beryl",
        material: "Emerald",
        face: [44.60, 81.14, 4.74],
        o: [44.60, 81.14, 4.74],
        e: [23.77, 56.83, -13.38],
        beta: None,
    },
    Pin {
        key: "emerald.goshenite",
        material: "Emerald",
        face: [100.00, 0.00, 0.00],
        o: [100.00, 0.00, 0.00],
        e: [100.00, 0.00, 0.00],
        beta: None,
    },
    Pin {
        key: "aquamarine.santa_maria",
        material: "Aquamarine",
        face: [79.97, -8.07, -9.88],
        o: [98.97, -7.63, 17.90],
        e: [46.85, -12.35, -61.71],
        beta: None,
    },
    Pin {
        key: "aquamarine.greenish",
        material: "Aquamarine",
        face: [79.10, -14.30, -11.13],
        o: [89.95, -8.76, 5.06],
        e: [64.97, -25.16, -32.89],
        beta: None,
    },
    Pin {
        key: "aquamarine.heliodor",
        material: "Aquamarine",
        face: [80.65, 1.99, 64.67],
        o: [94.22, -9.74, 63.25],
        e: [61.56, 23.76, 90.31],
        beta: None,
    },
    Pin {
        key: "aquamarine.red_beryl",
        material: "Aquamarine",
        face: [36.26, 71.02, -1.60],
        o: [44.60, 81.14, 4.74],
        e: [23.77, 56.83, -13.38],
        beta: None,
    },
    Pin {
        key: "aquamarine.goshenite",
        material: "Aquamarine",
        face: [100.00, 0.00, 0.00],
        o: [100.00, 0.00, 0.00],
        e: [100.00, 0.00, 0.00],
        beta: None,
    },
    Pin {
        key: "morganite.pink",
        material: "Morganite",
        face: [90.55, 16.15, -3.28],
        o: [90.55, 16.15, -3.28],
        e: [85.87, 15.77, -8.25],
        beta: None,
    },
    Pin {
        key: "morganite.peach",
        material: "Morganite",
        face: [88.41, 15.87, 4.87],
        o: [88.41, 15.87, 4.87],
        e: [79.18, 7.70, 29.86],
        beta: None,
    },
    Pin {
        key: "morganite.heliodor",
        material: "Morganite",
        face: [94.22, -9.74, 63.25],
        o: [94.22, -9.74, 63.25],
        e: [61.56, 23.76, 90.31],
        beta: None,
    },
    Pin {
        key: "morganite.red_beryl",
        material: "Morganite",
        face: [44.60, 81.14, 4.74],
        o: [44.60, 81.14, 4.74],
        e: [23.77, 56.83, -13.38],
        beta: None,
    },
    Pin {
        key: "morganite.goshenite",
        material: "Morganite",
        face: [100.00, 0.00, 0.00],
        o: [100.00, 0.00, 0.00],
        e: [100.00, 0.00, 0.00],
        beta: None,
    },
    Pin {
        key: "topaz.imperial",
        material: "Topaz",
        face: [56.41, 30.42, 38.61],
        o: [54.48, 26.88, 43.00],
        e: [58.09, 29.50, 34.75],
        beta: Some([58.25, 33.61, 35.24]),
    },
    Pin {
        key: "topaz.sherry",
        material: "Topaz",
        face: [70.27, 19.18, 37.23],
        o: [67.67, 20.28, 38.74],
        e: [72.73, 18.22, 36.05],
        beta: Some([72.73, 18.22, 36.05]),
    },
    Pin {
        key: "topaz.pink",
        material: "Topaz",
        face: [74.02, 19.92, -2.74],
        o: [76.87, 2.70, 9.96],
        e: [71.64, 24.11, -14.14],
        beta: Some([70.98, 37.87, -15.07]),
    },
    Pin {
        key: "topaz.sky",
        material: "Topaz",
        face: [83.43, -6.18, -5.85],
        o: [58.02, -23.21, -19.53],
        e: [92.10, -4.58, 1.09],
        beta: Some([99.99, -0.17, 0.34]),
    },
    Pin {
        key: "topaz.swiss",
        material: "Topaz",
        face: [72.94, -5.47, -9.18],
        o: [32.36, -20.62, -37.81],
        e: [73.67, -17.86, -12.33],
        beta: Some([92.97, -3.54, -1.63]),
    },
    Pin {
        key: "topaz.london",
        material: "Topaz",
        face: [66.69, -5.52, -5.23],
        o: [13.30, -10.37, -29.75],
        e: [57.21, -25.78, -17.26],
        beta: Some([87.41, -6.27, -2.82]),
    },
    Pin {
        key: "topaz.colorless",
        material: "Topaz",
        face: [100.00, 0.00, 0.00],
        o: [100.00, 0.00, 0.00],
        e: [100.00, 0.00, 0.00],
        beta: Some([100.00, 0.00, 0.00]),
    },
    Pin {
        key: "tourmaline.rubellite",
        material: "Tourmaline",
        face: [45.34, 62.24, 4.77],
        o: [39.98, 71.88, 0.26],
        e: [49.90, 55.03, 8.30],
        beta: None,
    },
    Pin {
        key: "tourmaline.pink",
        material: "Tourmaline",
        face: [80.04, 26.66, -1.96],
        o: [75.79, 34.12, -5.80],
        e: [83.95, 20.18, 1.43],
        beta: None,
    },
    Pin {
        key: "tourmaline.indicolite",
        material: "Tourmaline",
        face: [82.02, -68.05, 33.61],
        o: [82.13, -70.83, 31.69],
        e: [81.91, -65.29, 35.60],
        beta: None,
    },
    Pin {
        key: "tourmaline.verdelite",
        material: "Tourmaline",
        face: [73.21, -33.90, 16.49],
        o: [65.83, -49.49, 38.64],
        e: [79.54, -23.98, 4.62],
        beta: None,
    },
    Pin {
        key: "tourmaline.chrome",
        material: "Tourmaline",
        face: [63.05, -81.73, 15.02],
        o: [52.87, -111.05, 21.19],
        e: [71.12, -70.67, 12.03],
        beta: None,
    },
    Pin {
        key: "tourmaline.paraiba",
        material: "Tourmaline",
        face: [78.35, -69.57, -15.73],
        o: [83.52, -55.99, -21.46],
        e: [72.55, -90.29, -8.21],
        beta: None,
    },
    Pin {
        key: "tourmaline.paraiba_violet",
        material: "Tourmaline",
        face: [51.25, 66.06, -34.51],
        o: [61.23, 87.13, -49.92],
        e: [36.86, 24.62, 3.36],
        beta: None,
    },
    Pin {
        key: "tourmaline.yellow",
        material: "Tourmaline",
        face: [98.95, -16.03, 45.40],
        o: [98.03, -22.60, 87.94],
        e: [99.85, -9.84, 19.55],
        beta: None,
    },
    Pin {
        key: "andalusite.greener",
        material: "Andalusite",
        face: [73.07, -12.45, 44.48],
        o: [31.04, 34.08, 39.47],
        e: [94.95, -25.42, 41.06],
        beta: Some([93.39, -24.67, 52.46]),
    },
    Pin {
        key: "andalusite.vanadian",
        material: "Andalusite",
        face: [80.12, -1.68, 15.46],
        o: [60.41, -2.89, 53.46],
        e: [96.50, -2.61, 5.91],
        beta: Some([93.97, -1.18, 4.86]),
    },
];

/// The variant of `key` of the built-in `material`.
fn variant(material: &str, key: &str) -> super::ColorVariant {
    built_in_variants(material)
        .into_iter()
        .find(|v| v.key == key)
        .unwrap_or_else(|| panic!("no built-in variant {key} of {material}"))
}

fn assert_close(what: &str, got: [f64; 3], want: [f64; 3]) {
    let de = delta_e_2000(got, want);
    assert!(
        de <= TOLERANCE,
        "{what}: got Lab {got:?}, the fit record says {want:?} (dE2000 {de:.2})"
    );
}

/// The face-up color the renderer shows for `variant` on its host (D-G of the 2026-10-27 fix round): the
/// two eigenmodes of the host's own default principal frame at the wave normal `k = -Y`, so c along the
/// view shows the o-ray alone, c in the table plane the mean of o and e, and a biaxial host with gamma
/// along the view the mean of alpha and beta; an isotropic or non-pleochroic tensor shows its single
/// color. The fit records state the unpolarised mean as the fit target and this color as the reached
/// face-up color.
fn face_up_oriented(v: &super::ColorVariant, illuminant: Illuminant) -> [f64; 3] {
    let host = GemMaterial::built_in()
        .iter()
        .find(|m| m.name == v.material)
        .unwrap_or_else(|| panic!("no built-in host {}", v.material));
    let frame = PrincipalFrame::of_material(host);
    face_up_body_color(
        &v.absorption,
        Some(&frame),
        face_up_swatch_path(v.unit, STONE_WIDTH_MM),
        illuminant,
    )
    .lab
}

/// The face-up and per-direction colors of every entry are the recorded ones.
#[test]
fn every_entry_matches_its_fit_record() {
    for pin in PINS {
        let v = variant(pin.material, pin.key);
        assert_eq!(v.unit, AbsorptionUnit::PerMm, "{}: unit", pin.key);
        let path = face_up_swatch_path(v.unit, STONE_WIDTH_MM);
        let colors = body_colors(&v.absorption, path, Illuminant::D65);
        assert_close(
            &format!("{} face-up", pin.key),
            face_up_oriented(&v, Illuminant::D65),
            pin.face,
        );
        assert_close(&format!("{} o", pin.key), colors.o_ray.lab, pin.o);
        assert_close(&format!("{} e", pin.key), colors.e_ray.lab, pin.e);
        match (colors.beta_ray, pin.beta) {
            (Some(got), Some(want)) => assert_close(&format!("{} beta", pin.key), got.lab, want),
            (None, None) => {}
            (got, want) => panic!("{}: beta ray {got:?} against pinned {want:?}", pin.key),
        }
    }
}

/// The shared `variants::face_up_swatch` (what every swatch and chip shows) is the oriented color pinned here.
#[test]
fn the_shared_face_up_swatch_is_the_pinned_color() {
    for pin in PINS {
        let v = variant(pin.material, pin.key);
        let host = GemMaterial::built_in()
            .iter()
            .find(|m| m.name == pin.material)
            .expect("a built-in host");
        let swatch = face_up_swatch(&v.absorption, v.unit, f64::from(STONE_WIDTH_MM), host);
        assert_close(&format!("{} swatch", pin.key), swatch.lab, pin.face);
    }
}

/// `estimated = false` only where a published number fixes the strength of a measured spectrum (Topaz Pink: the
/// Katlang `Cr2O3` range); every other entry is a chosen strength, an unmeasured shape, or both (owner rule O3).
#[test]
fn only_a_published_anchor_leaves_an_entry_measured() {
    for pin in PINS {
        let v = variant(pin.material, pin.key);
        let VariantProvenance::BuiltIn { estimated, .. } = &v.provenance else {
            panic!("{}: not a built-in entry", pin.key);
        };
        assert_eq!(*estimated, pin.key != "topaz.pink", "{}", pin.key);
    }
}

/// One factor over the whole measured spectrum (owner rule): the refitted Indicolite, London and Swiss keep the
/// ratio between two fitted bands of their ray (the factor scales every band alike), so the ratio of two
/// peaks is the fit record's.
#[test]
fn the_refitted_entries_keep_the_measured_band_ratios() {
    // (material, key, first band center, second band center, ratio of their fitted peaks)
    for (material, key, c1, c2, ratio) in [
        (
            "Tourmaline",
            "tourmaline.indicolite",
            720.0,
            1161.9,
            2.503 / 0.6144,
        ),
        ("Topaz", "topaz.london", 622.1, 719.2, 0.9402 / 0.5244),
        ("Topaz", "topaz.swiss", 622.1, 719.2, 0.5088 / 0.2838),
    ] {
        let v = variant(material, key);
        let peak = |c: f32| {
            v.absorption
                .o_ray
                .iter()
                .find(|b| (b.center_nm - c).abs() < 0.05)
                .unwrap_or_else(|| panic!("{key}: no band at {c}"))
                .peak
        };
        let got = f64::from(peak(c1) / peak(c2));
        assert!(
            (got - ratio).abs() < 0.01 * ratio,
            "{key}: {got} vs {ratio}"
        );
    }
}

/// The pins and the group file list exactly the same entries (a new entry needs a fit record and a pin).
#[test]
fn pins_cover_the_group_file() {
    let mut in_file = BTreeSet::new();
    for material in [
        "Emerald",
        "Aquamarine",
        "Morganite",
        "Topaz",
        "Tourmaline",
        "Andalusite",
    ] {
        for v in built_in_variants(material) {
            if matches!(v.provenance, VariantProvenance::BuiltIn { .. }) {
                in_file.insert(v.key);
            }
        }
    }
    let pinned: BTreeSet<String> = PINS.iter().map(|p| p.key.to_string()).collect();
    // The other groups own other materials, so `in_file` is exactly this group's file.
    assert_eq!(in_file, pinned);
}

/// Every entry of this group carries a fit record link into the group's record file.
#[test]
fn every_entry_links_its_fit_record() {
    for pin in PINS {
        let v = variant(pin.material, pin.key);
        let VariantProvenance::BuiltIn { fit_record, .. } = &v.provenance else {
            panic!("{}: not a built-in entry", pin.key);
        };
        let link = fit_record.as_deref().unwrap_or_default();
        assert!(
            link.starts_with(
                "docs/color_variants/fit_records/beryl_topaz_tourmaline_andalusite.md#"
            ),
            "{}: fit_record {link:?}",
            pin.key
        );
    }
}

/// The ray that absorbs most (lowest L*) is the one the research documents for the variety.
const DARKEST_RAY: &[(&str, &str)] = &[
    ("emerald.zambian", "o"),
    ("emerald.light", "o"),
    ("aquamarine.santa_maria", "e"),
    ("tourmaline.rubellite", "o"),
    ("tourmaline.paraiba", "e"),
    ("tourmaline.paraiba_violet", "e"),
    ("topaz.london", "o"),
    ("topaz.sky", "o"),
    ("topaz.swiss", "o"),
    ("andalusite.greener", "o"),
    ("andalusite.vanadian", "o"),
];

#[test]
fn the_darkest_ray_is_the_documented_one() {
    for (key, ray) in DARKEST_RAY {
        let pin = PINS
            .iter()
            .find(|p| p.key == *key)
            .unwrap_or_else(|| panic!("no pin for {key}"));
        let candidates = [("o", pin.o[0]), ("e", pin.e[0])];
        let darkest = candidates
            .iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|c| c.0)
            .unwrap_or_default();
        assert_eq!(darkest, *ray, "{key}: darkest ray");
    }
}

/// The colorless entries are exactly colorless (white) in every direction.
#[test]
fn colorless_entries_are_white() {
    for pin in PINS
        .iter()
        .filter(|p| p.key.ends_with(".goshenite") || p.key == "topaz.colorless")
    {
        for lab in [pin.face, pin.o, pin.e] {
            assert!(
                lab[0] > 99.9 && lab[1].abs() < 0.1 && lab[2].abs() < 0.1,
                "{}: {lab:?}",
                pin.key
            );
        }
    }
}
