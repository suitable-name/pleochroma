//! Tests of the `data/color_variants/diamond.toml` entries: each entry's face-up color against
//! the target recorded in its fit record.
//!
//! Diamond is cubic, so every entry is isotropic (o ray = e ray, not pleochroic) and the
//! face-up, o-ray, e-ray and unpolarised colors are one color. The path is the face-up path of
//! a 7 mm stone for a per-mm tensor, `face_up_swatch_path(PerMm, 7.0)` = 9.905 mm, exactly what
//! the swatch and the render use. The expected values are the reached L*C*h of
//! `docs/color_variants/fit_records/diamond.md`; the targets are that record's grade-name
//! targets, which the reached colors must stay close to.

use super::{VariantProvenance, built_in_variants, face_up_swatch_path};
use crate::{
    color::body_color::{Illuminant, body_colors, delta_e_2000, lch_to_lab},
    optics::materials::AbsorptionUnit,
};

/// One diamond entry: its key, the reached L*C*h of the fit record and the target L*C*h.
struct Expected {
    key: &'static str,
    reached: [f64; 3],
    target: [f64; 3],
}

/// The seven entries, in data-file order.
const EXPECTED: [Expected; 7] = [
    Expected {
        key: "diamond.cape_yellow",
        reached: [99.18, 21.94, 108.15],
        target: [98.0, 22.0, 103.0],
    },
    Expected {
        key: "diamond.canary",
        reached: [88.55, 71.73, 88.55],
        target: [86.0, 72.0, 90.0],
    },
    Expected {
        key: "diamond.brown",
        reached: [49.70, 40.85, 68.06],
        target: [50.0, 40.0, 65.0],
    },
    Expected {
        key: "diamond.pink",
        reached: [79.04, 39.33, 329.27],
        target: [78.0, 38.0, 330.0],
    },
    Expected {
        key: "diamond.purple_pink",
        reached: [76.01, 45.78, 306.85],
        target: [76.0, 46.0, 308.0],
    },
    Expected {
        key: "diamond.blue",
        reached: [76.53, 39.44, 244.78],
        target: [76.0, 38.0, 244.0],
    },
    Expected {
        key: "diamond.green",
        reached: [82.00, 61.85, 170.54],
        target: [80.0, 58.0, 170.0],
    },
];

#[test]
fn the_diamond_file_lists_the_seven_variants_in_order() {
    let keys: Vec<String> = built_in_variants("Diamond")
        .into_iter()
        .skip(1) // the Material Default
        .map(|v| v.key)
        .collect();
    let expected: Vec<&str> = EXPECTED.iter().map(|e| e.key).collect();
    assert_eq!(keys, expected);
}

#[test]
fn every_diamond_variant_is_estimated_isotropic_and_documented() {
    let variants = built_in_variants("Diamond");
    for e in &EXPECTED {
        let v = variants
            .iter()
            .find(|v| v.key == e.key)
            .unwrap_or_else(|| panic!("{} is missing", e.key));
        assert_eq!(v.unit, AbsorptionUnit::PerMm, "{}", e.key);
        assert!((v.reference_path_mm - 7.0).abs() < 1e-6, "{}", e.key);
        assert!(!v.absorption.is_pleochroic, "{}: diamond is cubic", e.key);
        assert!(v.absorption.beta_ray.is_none(), "{}", e.key);
        assert_eq!(
            v.absorption.o_ray, v.absorption.e_ray,
            "{}: o and e must be equal",
            e.key
        );
        assert!(!v.absorption.o_ray.is_empty(), "{}", e.key);
        let VariantProvenance::BuiltIn {
            sources,
            fit_record,
            estimated,
        } = &v.provenance
        else {
            panic!("{}: not a built-in entry", e.key);
        };
        assert!(*estimated, "{}: no measured spectrum exists", e.key);
        assert!(sources.len() >= 2, "{}", e.key);
        assert!(
            fit_record
                .as_deref()
                .is_some_and(|r| r.starts_with("docs/color_variants/fit_records/diamond.md#")),
            "{}",
            e.key
        );
    }
}

#[test]
fn the_diamond_face_up_color_matches_the_fit_record() {
    let variants = built_in_variants("Diamond");
    let path = face_up_swatch_path(AbsorptionUnit::PerMm, 7.0);
    // 2.83 x 7 / 2 in f32 constants: 9.904999733 in f64, so a f32-sized tolerance.
    assert!((path - 9.905).abs() < 1e-6, "face-up path {path}");
    for e in &EXPECTED {
        let v = variants
            .iter()
            .find(|v| v.key == e.key)
            .unwrap_or_else(|| panic!("{} is missing", e.key));
        let colors = body_colors(&v.absorption, path, Illuminant::D65);
        let expected = lch_to_lab(e.reached);
        // Isotropic: the face-up (unpolarised), o-ray and e-ray colors are the same color.
        for (what, lab) in [
            ("face-up", colors.unpolarised.lab),
            ("o-ray", colors.o_ray.lab),
            ("e-ray", colors.e_ray.lab),
        ] {
            let de = delta_e_2000(lab, expected);
            assert!(
                de < 1.0,
                "{} {what}: Lab {lab:?} is dE2000 {de:.2} from the record {expected:?}",
                e.key
            );
        }
        assert!(colors.beta_ray.is_none(), "{}", e.key);
    }
}

#[test]
fn the_diamond_reached_colors_stay_close_to_their_targets() {
    // The record's targets are estimates for the grade names; the fit must stay within dE2000 3.
    for e in &EXPECTED {
        let de = delta_e_2000(lch_to_lab(e.reached), lch_to_lab(e.target));
        assert!(de < 3.0, "{}: reached vs target dE2000 {de:.2}", e.key);
    }
}
