//! Tests of the `data/color_variants/nanosital.toml` entries: each colored entry's face-up hue
//! class, and the provenance every entry carries.
//!
//! Nanosital is isotropic, so every entry is one color in every direction. The path is the
//! face-up path of a 7 mm stone for a per-mm tensor, `face_up_swatch_path(PerMm, 7.0)` = 9.905 mm.
//! The expected hues are the colors GIA reports for each chromophore (Gems & Gemology 62(2), 2026,
//! figure 8); the strengths behind them are estimates (`docs/chromophores/calibration-records.md`,
//! "Nanosital").

use super::{VariantProvenance, built_in_variants, face_up_swatch_path};
use crate::{
    color::body_color::{Illuminant, body_colors, lab_to_lch},
    optics::materials::AbsorptionUnit,
};

/// `[L*, C*, h]` of the variant `key` at 7 mm, face-up.
fn lch_of(key: &str) -> [f64; 3] {
    let variant = built_in_variants("Nanosital")
        .into_iter()
        .find(|v| v.key == key)
        .unwrap_or_else(|| panic!("no variant {key}"));
    assert_eq!(variant.unit, AbsorptionUnit::PerMm, "{key}");
    let path = face_up_swatch_path(variant.unit, variant.reference_path_mm);
    lab_to_lch(
        body_colors(&variant.absorption, path, Illuminant::D65)
            .unpolarised
            .lab,
    )
}

/// The hue window, the lowest chroma and the lightness window of each colored entry.
const EXPECTED: [(&str, (f64, f64), f64, (f64, f64)); 6] = [
    ("nanosital.ni_green", (140.0, 170.0), 50.0, (60.0, 80.0)),
    ("nanosital.mn_yellow", (75.0, 95.0), 40.0, (80.0, 95.0)),
    ("nanosital.mn_orange", (55.0, 78.0), 60.0, (55.0, 75.0)),
    ("nanosital.au_red", (5.0, 35.0), 35.0, (35.0, 55.0)),
    ("nanosital.nd_er_pink", (300.0, 345.0), 15.0, (72.0, 90.0)),
    (
        "nanosital.pr_yellow_green",
        (110.0, 135.0),
        10.0,
        (90.0, 100.0),
    ),
];

#[test]
fn each_colored_entry_has_its_chromophores_hue() {
    for (key, (h_low, h_high), c_min, (l_low, l_high)) in EXPECTED {
        let [l, c, h] = lch_of(key);
        assert!((h_low..=h_high).contains(&h), "{key}: hue {h:.1}");
        assert!(c >= c_min, "{key}: chroma {c:.1}");
        assert!((l_low..=l_high).contains(&l), "{key}: lightness {l:.1}");
    }
}

/// Orange is the yellow's manganese at five times the amount: darker and redder.
#[test]
fn more_manganese_turns_yellow_into_orange() {
    let [l_yellow, _, h_yellow] = lch_of("nanosital.mn_yellow");
    let [l_orange, _, h_orange] = lch_of("nanosital.mn_orange");
    assert!(
        h_orange < h_yellow && l_orange < l_yellow,
        "orange (L* {l_orange:.1}, h {h_orange:.1}) must be darker and redder than yellow (L* {l_yellow:.1}, h {h_yellow:.1})"
    );
}

/// Every entry cites the GIA study and links the calibration record; every colored entry is an
/// estimate, and the colorless one has no band.
#[test]
fn every_entry_cites_gia_and_the_calibration_record() {
    let variants: Vec<_> = built_in_variants("Nanosital")
        .into_iter()
        .filter(|v| matches!(v.provenance, VariantProvenance::BuiltIn { .. }))
        .collect();
    assert_eq!(variants.len(), 7);
    for variant in &variants {
        let VariantProvenance::BuiltIn {
            sources,
            fit_record,
            estimated,
        } = &variant.provenance
        else {
            unreachable!("filtered to built-in entries");
        };
        assert!(
            sources.iter().any(|s| s.contains("Gems & Gemology 62(2)")),
            "{}",
            variant.key
        );
        assert_eq!(
            fit_record.as_deref(),
            Some("docs/chromophores/calibration-records.md#nanosital"),
            "{}",
            variant.key
        );
        let colorless = variant.absorption.o_ray.is_empty();
        assert_eq!(*estimated, !colorless, "{}", variant.key);
        assert_eq!(colorless, variant.key == "nanosital.colorless");
    }
}
