//! Tests of the `data/color_variants/quartz_diamond_opal_synthetics_glass.toml` entries: each entry's
//! face-up color and per-direction colors against the CIELAB its fit record
//! (`docs/color_variants/fit_records/quartz_diamond_opal_synthetics_glass.md`) states.
//!
//! The colors are `color::body_color::body_colors` under D65 over `face_up_swatch_path` of a 7 mm
//! stone: every entry is `per_mm`, so the path is `MODEL_UNIT_FACE_UP_PATH` x 7 / `REFERENCE_MODEL_WIDTH`
//! = 9.905 mm (what `render_setup::absorption_path_scale_for` times the face-up constant gives in the
//! renderer). The pinned face-up color is the ORIENTED one the renderer shows
//! (`color::principal_axes::face_up_body_color` with the host's default principal frame: the o-ray alone
//! for Amethyst, Citrine, Quartz, Rutile and Moissanite, whose c axis is the view); the o-ray and e-ray
//! colors are the single rays. The pinned Lab values are the fit lane's own computation of the same
//! pipeline (checked against the Sapphire, Ruby and Emerald swatches of
//! `docs/chromophores/calibration-records.md`); no spectrum CSV is read here.

use super::{
    TensorRank, VariantProvenance, built_in_variants, data, face_up_swatch_path, material_slug,
    tensor_rank,
};
use crate::{
    color::{
        Illuminant,
        body_color::{BodyColors, body_colors, delta_e_2000, lab_to_lch},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::materials::{AbsorptionUnit, GemMaterial},
};

/// The built-in materials this group file carries entries for (GGG stays colorless; Diamond is the
/// lead's own section below).
const GROUP_MATERIALS: [&str; 11] = [
    "Amethyst",
    "Citrine",
    "Quartz",
    "Opal",
    "Cubic Zirconia",
    "YAG",
    "GGG",
    "Synthetic Moissanite",
    "Glass (N-BK7)",
    "Glass (F2)",
    "Rutile",
];

/// The fit record the entries point at.
const FIT_RECORD: &str = "docs/color_variants/fit_records/quartz_diamond_opal_synthetics_glass.md";

/// Largest CIEDE2000 difference between an app color and the one a fit record states.
const MAX_DELTA_E: f64 = 1.0;

/// One entry's pinned colors (CIELAB, D65, 7 mm stone): the face-up (oriented) color and the
/// o-ray and e-ray colors (equal to the face-up color for an isotropic host).
struct Pin {
    key: &'static str,
    /// The entry's `estimated` flag.
    estimated: bool,
    face_up: [f64; 3],
    o_ray: [f64; 3],
    e_ray: [f64; 3],
}

/// The entries in file order, with the colors the fit record states.
const PINS: [Pin; 31] = [
    Pin {
        key: "amethyst.deep_siberian",
        estimated: true,
        face_up: [30.96, 41.62, -23.56],
        o_ray: [30.96, 41.62, -23.56],
        e_ray: [44.25, 32.98, -14.09],
    },
    Pin {
        key: "amethyst.rose_de_france",
        estimated: true,
        face_up: [86.95, 10.34, -6.67],
        o_ray: [86.95, 10.34, -6.67],
        e_ray: [90.66, 6.50, -3.04],
    },
    Pin {
        key: "citrine.madeira",
        estimated: true,
        face_up: [49.25, 45.03, 83.64],
        o_ray: [49.25, 45.03, 83.64],
        e_ray: [49.25, 45.03, 83.64],
    },
    Pin {
        key: "quartz.smoky",
        estimated: true,
        face_up: [58.08, 6.89, 11.97],
        o_ray: [58.08, 6.89, 11.97],
        e_ray: [47.48, 5.28, 20.62],
    },
    Pin {
        key: "quartz.pink_crystalline",
        estimated: true,
        face_up: [80.28, 13.65, 0.37],
        o_ray: [80.28, 13.65, 0.37],
        e_ray: [81.84, 6.89, 0.66],
    },
    Pin {
        key: "quartz.prasiolite",
        estimated: true,
        face_up: [93.82, -24.62, 26.26],
        o_ray: [93.82, -24.62, 26.26],
        e_ray: [93.82, -24.62, 26.26],
    },
    Pin {
        key: "opal.fire_orange",
        estimated: true,
        face_up: [67.94, 42.39, 95.22],
        o_ray: [67.94, 42.39, 95.22],
        e_ray: [67.94, 42.39, 95.22],
    },
    Pin {
        key: "opal.fire_red",
        estimated: true,
        face_up: [38.40, 59.56, 66.08],
        o_ray: [38.40, 59.56, 66.08],
        e_ray: [38.40, 59.56, 66.08],
    },
    Pin {
        key: "rutile.brownish_red_orange",
        estimated: true,
        face_up: [42.16, 29.08, 70.41],
        o_ray: [42.16, 29.08, 70.41],
        e_ray: [34.06, 27.30, 57.81],
    },
    Pin {
        key: "rutile.orange",
        estimated: true,
        face_up: [55.26, 30.53, 88.31],
        o_ray: [55.26, 30.53, 88.31],
        e_ray: [52.83, 33.64, 87.41],
    },
    Pin {
        key: "rutile.blue_reduced",
        estimated: true,
        face_up: [37.92, 10.61, -63.28],
        o_ray: [37.92, 10.61, -63.28],
        e_ray: [48.99, -6.79, -49.59],
    },
    Pin {
        key: "cubic_zirconia.ce_yellow",
        estimated: true,
        face_up: [85.04, 3.59, 45.04],
        o_ray: [85.04, 3.59, 45.04],
        e_ray: [85.04, 3.59, 45.04],
    },
    Pin {
        key: "cubic_zirconia.ce_orange",
        estimated: true,
        face_up: [68.38, 19.54, 62.34],
        o_ray: [68.38, 19.54, 62.34],
        e_ray: [68.38, 19.54, 62.34],
    },
    Pin {
        key: "cubic_zirconia.er_pink",
        estimated: true,
        face_up: [93.68, 19.15, -8.16],
        o_ray: [93.68, 19.15, -8.16],
        e_ray: [93.68, 19.15, -8.16],
    },
    Pin {
        key: "cubic_zirconia.nd_lilac",
        estimated: true,
        face_up: [92.48, 5.62, -12.54],
        o_ray: [92.48, 5.62, -12.54],
        e_ray: [92.48, 5.62, -12.54],
    },
    Pin {
        key: "cubic_zirconia.cr_green",
        estimated: true,
        face_up: [81.10, -28.27, 2.76],
        o_ray: [81.10, -28.27, 2.76],
        e_ray: [81.10, -28.27, 2.76],
    },
    Pin {
        key: "yag.nd_lilac",
        estimated: true,
        face_up: [92.66, -1.50, -12.52],
        o_ray: [92.66, -1.50, -12.52],
        e_ray: [92.66, -1.50, -12.52],
    },
    Pin {
        key: "yag.cr_green",
        estimated: true,
        face_up: [69.15, -73.15, 14.27],
        o_ray: [69.15, -73.15, 14.27],
        e_ray: [69.15, -73.15, 14.27],
    },
    Pin {
        key: "yag.ce_yellow",
        estimated: true,
        face_up: [91.71, 0.62, 103.80],
        o_ray: [91.71, 0.62, 103.80],
        e_ray: [91.71, 0.62, 103.80],
    },
    Pin {
        key: "synthetic_moissanite.near_colorless",
        estimated: true,
        face_up: [98.86, -2.08, 9.00],
        o_ray: [98.86, -2.08, 9.00],
        e_ray: [98.86, -2.08, 9.00],
    },
    Pin {
        key: "synthetic_moissanite.green_tint",
        estimated: true,
        face_up: [85.20, -35.92, 13.79],
        o_ray: [85.20, -35.92, 13.79],
        e_ray: [90.90, -18.83, 23.57],
    },
    Pin {
        key: "glass__n_bk7_.co_blue",
        estimated: true,
        face_up: [47.90, 25.67, -88.41],
        o_ray: [47.90, 25.67, -88.41],
        e_ray: [47.90, 25.67, -88.41],
    },
    Pin {
        key: "glass__n_bk7_.cr_green",
        estimated: true,
        face_up: [77.27, -83.66, 74.04],
        o_ray: [77.27, -83.66, 74.04],
        e_ray: [77.27, -83.66, 74.04],
    },
    Pin {
        key: "glass__n_bk7_.mn_purple",
        estimated: true,
        face_up: [50.41, 86.15, -24.00],
        o_ray: [50.41, 86.15, -24.00],
        e_ray: [50.41, 86.15, -24.00],
    },
    Pin {
        key: "glass__n_bk7_.se_ruby",
        estimated: true,
        face_up: [40.43, 75.13, 69.14],
        o_ray: [40.43, 75.13, 69.14],
        e_ray: [40.43, 75.13, 69.14],
    },
    Pin {
        key: "glass__n_bk7_.se_pink",
        estimated: true,
        face_up: [94.08, 13.83, 6.60],
        o_ray: [94.08, 13.83, 6.60],
        e_ray: [94.08, 13.83, 6.60],
    },
    Pin {
        key: "glass__f2_.co_blue",
        estimated: true,
        face_up: [47.90, 25.67, -88.41],
        o_ray: [47.90, 25.67, -88.41],
        e_ray: [47.90, 25.67, -88.41],
    },
    Pin {
        key: "glass__f2_.cr_green",
        estimated: true,
        face_up: [77.27, -83.66, 74.04],
        o_ray: [77.27, -83.66, 74.04],
        e_ray: [77.27, -83.66, 74.04],
    },
    Pin {
        key: "glass__f2_.mn_purple",
        estimated: true,
        face_up: [50.41, 86.15, -24.00],
        o_ray: [50.41, 86.15, -24.00],
        e_ray: [50.41, 86.15, -24.00],
    },
    Pin {
        key: "glass__f2_.se_ruby",
        estimated: true,
        face_up: [40.43, 75.13, 69.14],
        o_ray: [40.43, 75.13, 69.14],
        e_ray: [40.43, 75.13, 69.14],
    },
    Pin {
        key: "glass__f2_.se_pink",
        estimated: true,
        face_up: [94.08, 13.83, 6.60],
        o_ray: [94.08, 13.83, 6.60],
        e_ray: [94.08, 13.83, 6.60],
    },
];

/// The face-up path of a 7 mm stone for a `per_mm` tensor, in millimetres, as the renderer uses it.
fn face_up_path_mm() -> f64 {
    face_up_swatch_path(AbsorptionUnit::PerMm, 7.0)
}

/// The entry `key` of the group file, looked up through its material's variant list.
fn entry(key: &str) -> super::ColorVariant {
    GROUP_MATERIALS
        .iter()
        .flat_map(|name| built_in_variants(name))
        .find(|v| v.key == key)
        .unwrap_or_else(|| panic!("no color variant {key}"))
}

/// The D65 body colors of entry `key` over the face-up path of a 7 mm stone.
fn colors(key: &str) -> BodyColors {
    body_colors(&entry(key).absorption, face_up_path_mm(), Illuminant::D65)
}

/// The face-up color the renderer shows for entry `key` on its host (see the module docs).
fn face_up_oriented(key: &str) -> [f64; 3] {
    let variant = entry(key);
    let host = GemMaterial::built_in()
        .iter()
        .find(|m| m.name == variant.material)
        .unwrap_or_else(|| panic!("no built-in host {}", variant.material));
    let frame = PrincipalFrame::of_material(host);
    face_up_body_color(
        &variant.absorption,
        Some(&frame),
        face_up_path_mm(),
        Illuminant::D65,
    )
    .lab
}

/// `[L*, C*, h]` of a CIELAB triple.
fn lch(lab: [f64; 3]) -> [f64; 3] {
    lab_to_lch(lab)
}

#[test]
fn the_face_up_path_is_the_renderers_for_a_seven_mm_stone() {
    assert!(
        (face_up_path_mm() - 9.905).abs() < 1e-3,
        "{}",
        face_up_path_mm()
    );
}

#[test]
fn every_entry_reproduces_the_colors_its_fit_record_states() {
    for pin in &PINS {
        let c = colors(pin.key);
        for (what, got, want) in [
            ("face-up", face_up_oriented(pin.key), pin.face_up),
            ("o-ray", c.o_ray.lab, pin.o_ray),
            ("e-ray", c.e_ray.lab, pin.e_ray),
        ] {
            let delta = delta_e_2000(got, want);
            assert!(
                delta <= MAX_DELTA_E,
                "{} {what}: Lab {got:?} is dE2000 {delta:.2} from the recorded {want:?}",
                pin.key
            );
        }
    }
}

#[test]
fn the_file_lists_exactly_the_pinned_entries_in_order() {
    let keys: Vec<&str> = data::file_entries()
        .iter()
        .filter(|v| GROUP_MATERIALS.contains(&v.material.as_str()))
        .map(|v| v.key.as_str())
        .collect();
    let pinned: Vec<&str> = PINS.iter().map(|p| p.key).collect();
    assert_eq!(keys, pinned);
}

#[test]
fn every_entry_is_per_mm_for_a_7_mm_stone_with_the_recorded_provenance() {
    for pin in &PINS {
        let variant = entry(pin.key);
        assert_eq!(variant.unit, AbsorptionUnit::PerMm, "{}", pin.key);
        assert!(
            (variant.reference_path_mm - 7.0).abs() < f32::EPSILON,
            "{}",
            pin.key
        );
        let VariantProvenance::BuiltIn {
            sources,
            fit_record,
            estimated,
        } = &variant.provenance
        else {
            panic!("{}: a data entry is BuiltIn", pin.key);
        };
        assert!(!sources.is_empty(), "{}", pin.key);
        assert_eq!(*estimated, pin.estimated, "{}", pin.key);
        let record = fit_record.as_deref().expect("a fit record link");
        assert!(
            record.starts_with(&format!("{FIT_RECORD}#")),
            "{}: {record}",
            pin.key
        );
    }
}

#[test]
fn keys_follow_the_material_slug_and_no_entry_is_a_diamond_one() {
    for pin in &PINS {
        let variant = entry(pin.key);
        assert!(
            pin.key
                .starts_with(&format!("{}.", material_slug(&variant.material))),
            "{}",
            pin.key
        );
        assert!(!pin.key.starts_with("diamond."), "{}", pin.key);
        assert!(!variant.label.trim().is_empty(), "{}", pin.key);
    }
}

#[test]
fn isotropic_hosts_store_equal_non_pleochroic_sets() {
    for name in [
        "Opal",
        "Cubic Zirconia",
        "YAG",
        "Glass (N-BK7)",
        "Glass (F2)",
    ] {
        for variant in built_in_variants(name).into_iter().skip(1) {
            let t = &variant.absorption;
            assert_eq!(tensor_rank(t), TensorRank::Isotropic, "{}", variant.key);
            assert_eq!(t.o_ray, t.e_ray, "{}", variant.key);
            assert!(t.beta_ray.is_none(), "{}", variant.key);
            assert!(!t.o_ray.is_empty(), "{}", variant.key);
        }
    }
}

#[test]
fn uniaxial_hosts_store_pleochroic_tensors_with_equal_sets_only_where_there_is_no_pleochroism() {
    for name in [
        "Amethyst",
        "Citrine",
        "Quartz",
        "Synthetic Moissanite",
        "Rutile",
    ] {
        for variant in built_in_variants(name).into_iter().skip(1) {
            let t = &variant.absorption;
            assert_eq!(tensor_rank(t), TensorRank::Uniaxial, "{}", variant.key);
            let equal = t.o_ray == t.e_ray;
            // Only the entries whose research shows no visible pleochroism carry equal sets.
            assert_eq!(
                equal,
                matches!(
                    variant.key.as_str(),
                    "citrine.madeira" | "quartz.prasiolite" | "synthetic_moissanite.near_colorless"
                ),
                "{}",
                variant.key
            );
        }
    }
}

#[test]
fn the_rays_follow_the_documented_pleochroism() {
    // Amethyst: E perpendicular c bluish violet, E parallel c reddish violet (and lighter).
    for key in ["amethyst.deep_siberian", "amethyst.rose_de_france"] {
        let c = colors(key);
        assert!(lch(c.e_ray.lab)[2] > lch(c.o_ray.lab)[2], "{key}: hue");
        assert!(c.e_ray.lab[0] > c.o_ray.lab[0], "{key}: lightness");
    }
    // Smoky quartz (measured): the e-ray is the stronger absorber and the browner one.
    let smoky = colors("quartz.smoky");
    assert!(smoky.e_ray.lab[0] < smoky.o_ray.lab[0] - 5.0);
    assert!(lch(smoky.e_ray.lab)[1] > lch(smoky.o_ray.lab)[1]);
    // Crystalline pink quartz (measured): the o-ray absorbs about twice as much at 525 nm.
    let pink = colors("quartz.pink_crystalline");
    assert!(lch(pink.o_ray.lab)[1] > 1.5 * lch(pink.e_ray.lab)[1]);
    // Rutile: the e-ray is the darker one (measured), the reduced blue one the lighter (Report 0.7).
    for key in ["rutile.brownish_red_orange", "rutile.orange"] {
        let c = colors(key);
        assert!(c.e_ray.lab[0] < c.o_ray.lab[0], "{key}");
    }
    let blue = colors("rutile.blue_reduced");
    assert!(blue.e_ray.lab[0] > blue.o_ray.lab[0] + 5.0);
    // Green-tint moissanite: the o-ray carries the free-carrier band (e : o = 0.4).
    let green = colors("synthetic_moissanite.green_tint");
    assert!(green.o_ray.lab[0] < green.e_ray.lab[0]);
    assert!(lch(green.o_ray.lab)[1] > lch(green.e_ray.lab)[1]);
}

#[test]
fn the_grades_are_ordered_by_density_and_the_hues_are_the_varieties() {
    let l = |key: &str| colors(key).unpolarised.lab[0];
    assert!(l("amethyst.rose_de_france") > l("amethyst.deep_siberian") + 40.0);
    assert!(l("opal.fire_orange") > l("opal.fire_red") + 20.0);
    assert!(l("cubic_zirconia.ce_yellow") > l("cubic_zirconia.ce_orange"));
    let h = |key: &str| lch(colors(key).unpolarised.lab)[2];
    assert!((75.0..100.0).contains(&h("cubic_zirconia.ce_yellow")));
    assert!((60.0..80.0).contains(&h("cubic_zirconia.ce_orange")));
    assert!((60.0..75.0).contains(&h("opal.fire_orange")));
    assert!((40.0..55.0).contains(&h("opal.fire_red")));
    assert!((320.0..350.0).contains(&h("cubic_zirconia.er_pink")));
    assert!((280.0..310.0).contains(&h("cubic_zirconia.nd_lilac")));
    assert!((160.0..180.0).contains(&h("yag.cr_green")));
    assert!((270.0..300.0).contains(&h("glass__n_bk7_.co_blue")));
    assert!((330.0..355.0).contains(&h("glass__f2_.mn_purple")));
    assert!((30.0..55.0).contains(&h("glass__f2_.se_ruby")));
    assert!((260.0..290.0).contains(&h("rutile.blue_reduced")));
    assert!((150.0..190.0).contains(&h("cubic_zirconia.cr_green")));
    assert!((130.0..170.0).contains(&h("synthetic_moissanite.green_tint")));
    assert!(l("synthetic_moissanite.near_colorless") > 97.0);
}

#[test]
fn both_glasses_carry_the_same_colorants() {
    for name in ["co_blue", "cr_green", "mn_purple", "se_ruby", "se_pink"] {
        let a = entry(&format!("glass__n_bk7_.{name}"));
        let b = entry(&format!("glass__f2_.{name}"));
        assert_eq!(a.absorption, b.absorption, "{name}");
        assert_eq!(a.label, b.label, "{name}");
    }
}

// Diamond: lead
