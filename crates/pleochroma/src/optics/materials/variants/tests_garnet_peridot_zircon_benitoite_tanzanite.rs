//! Tests of the `data/color_variants/garnet_peridot_zircon_benitoite_tanzanite.toml` entries: each entry's face-up color and
//! per-direction colors against the numbers recorded in its fit record
//! (`docs/color_variants/fit_records/garnet_peridot_zircon_benitoite_tanzanite.md`).
//!
//! The colors are computed the way the app does for a 7 mm stone: `body_colors` of the variant's
//! tensor at `face_up_swatch_path(unit, 7.0)` (9.905 mm for a per-millimetre tensor: 2.83 model
//! units of face-up path times 3.5 mm per model unit), D65. The pinned numbers come from the fit
//! lane's Python replica of that pipeline (checked against the Spessartine, Tsavorite, Peridot and
//! Demantoid swatches of `docs/chromophores/calibration-records.md`).

use super::{TensorRank, VariantProvenance, built_in_variants, face_up_swatch_path, tensor_rank};
use crate::{
    color::{
        body_color::{Illuminant, body_colors, delta_e_2000, lab_to_lch},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::materials::{AbsorptionUnit, GemMaterial},
};

/// Largest accepted color difference between the app's color and the recorded one.
const TOLERANCE_DE2000: f64 = 1.0;

/// `(material, key, face-up Lab, direction Labs)`, D65, 7 mm stone. The face-up Lab is the ORIENTED color the
/// renderer shows (host's default principal frame: o alone for Zircon and Benitoite, alpha and beta for Peridot and
/// Tanzanite). Directions are in the order o (alpha), e (gamma), then beta; empty for the cubic garnets (one band set).
type Entry = (&'static str, &'static str, [f64; 3], &'static [[f64; 3]]);

const ENTRIES: [Entry; 21] = [
    (
        "Pyrope Garnet",
        "pyrope_garnet.rhodolite",
        [47.746, 39.476, -3.734],
        &[],
    ),
    (
        "Pyrope Garnet",
        "pyrope_garnet.chrome_pyrope",
        [39.759, 28.937, 6.776],
        &[],
    ),
    (
        "Almandine Garnet",
        "almandine_garnet.purplish",
        [35.286, 45.861, 9.345],
        &[],
    ),
    (
        "Spessartine Garnet",
        "spessartine_garnet.reddish_orange",
        [61.368, 29.562, 67.257],
        &[],
    ),
    (
        "Spessartine Garnet",
        "spessartine_garnet.pale_golden_orange",
        [88.542, 9.712, 38.630],
        &[],
    ),
    (
        "Grossular Garnet (Tsavorite)",
        "grossular_garnet__tsavorite_.mint",
        // half the default since 2026-11-04 (it was [79.289, -45.937, 42.995], the default's color)
        [89.0, -28.396, 29.938],
        &[],
    ),
    (
        "Grossular Garnet (Tsavorite)",
        "grossular_garnet__tsavorite_.hessonite",
        [68.513, 24.879, 70.256],
        &[],
    ),
    (
        "Andradite Garnet (Demantoid)",
        "andradite_garnet__demantoid_.topazolite",
        [74.251, -7.812, 52.886],
        &[],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.tunduru",
        [57.145, -13.342, -1.359],
        &[],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.malaya",
        [58.779, 30.732, 56.530],
        &[],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.weaker_change",
        [68.055, -9.692, 16.819],
        &[],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.stronger_change",
        [43.520, -15.787, -23.828],
        &[],
    ),
    (
        "Peridot",
        "peridot.deep_green",
        [67.216, -23.743, 69.357],
        &[
            [78.969, -25.427, 74.726],
            [72.326, -26.954, 65.126],
            [50.644, -22.465, 67.121],
        ],
    ),
    (
        "Zircon",
        "zircon.blue",
        [60.106, -19.139, -52.896],
        &[[60.106, -19.139, -52.896], [94.062, -8.079, 2.484]],
    ),
    (
        "Zircon",
        "zircon.golden_brown",
        [62.224, 10.137, 29.425],
        &[[62.224, 10.137, 29.425], [60.275, 1.850, 41.071]],
    ),
    (
        "Zircon",
        "zircon.red_brown",
        [50.512, 43.991, 56.805],
        &[[50.512, 43.991, 56.805], [50.518, 44.247, 54.746]],
    ),
    (
        "Zircon",
        "zircon.green",
        [58.167, 4.686, 27.400],
        &[[58.167, 4.686, 27.400], [51.744, -11.381, 28.990]],
    ),
    (
        "Zircon",
        "zircon.colorless",
        [100.000, 0.000, 0.000],
        &[[100.000, 0.000, 0.000], [100.000, 0.000, 0.000]],
    ),
    // Face-up oriented by the host stone: since 2026-11-03 Benitoite's c axis lies in the table,
    // so face up shows the blue e-ray beside the near-colorless o-ray (with c through the table
    // it was the o-ray alone, [99.598, 0.069, -0.415]). The direction colors are unchanged.
    (
        "Benitoite",
        "benitoite.light_blue",
        [90.022, -1.988, -14.181],
        &[[99.598, 0.069, -0.415], [78.309, -5.173, -31.554]],
    ),
    (
        "Tanzanite",
        "tanzanite.unheated",
        [38.583, 15.712, -16.098],
        &[
            [36.351, 34.447, -19.651],
            [57.642, -30.034, 30.596],
            [40.646, -3.010, -12.834],
        ],
    ),
    (
        "Tanzanite",
        "tanzanite.green_zoisite",
        [48.467, -2.646, 34.207],
        &[
            [60.805, -0.659, 46.168],
            [67.314, -22.521, 12.724],
            [27.582, -9.946, 10.562],
        ],
    ),
];

/// `(material, key, face-up Lab under the Incandescent preset's 3200 K Planckian)` of the four
/// color-change garnets.
const INCANDESCENT: [(&str, &str, [f64; 3]); 4] = [
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.tunduru",
        [55.803, 0.809, -7.844],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.malaya",
        [63.140, 36.833, 61.896],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.weaker_change",
        [67.915, 3.801, 12.053],
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        "color_change_garnet__pyrope_spessartine_.stronger_change",
        [40.373, -3.761, -32.914],
    ),
];

/// The face-up color the renderer shows for `variant` on its host (D-G of the 2026-10-27 fix round): the
/// two eigenmodes of the host's own default principal frame at the wave normal `k = -Y`, so c along the
/// view shows the o-ray alone, c in the table plane the mean of o and e, and a biaxial host with gamma
/// along the view the mean of alpha and beta; an isotropic or non-pleochroic tensor shows its single
/// color. The fit records state the unpolarised mean as the fit target and this color as the reached
/// face-up color.
fn face_up_oriented(variant: &super::ColorVariant, illuminant: Illuminant) -> [f64; 3] {
    let host = GemMaterial::built_in()
        .iter()
        .find(|m| m.name == variant.material)
        .unwrap_or_else(|| panic!("no built-in host {}", variant.material));
    let frame = PrincipalFrame::of_material(host);
    face_up_body_color(
        &variant.absorption,
        Some(&frame),
        face_up_swatch_path(variant.unit, 7.0),
        illuminant,
    )
    .lab
}

/// The variant `key` of `material`, from the shipped data.
fn variant(material: &str, key: &str) -> super::ColorVariant {
    built_in_variants(material)
        .into_iter()
        .find(|v| v.key == key)
        .unwrap_or_else(|| panic!("no built-in variant {key} of {material}"))
}

/// The unpolarised and per-direction Labs of `key` under `illuminant` at the 7 mm face-up path.
fn app_labs(material: &str, key: &str, illuminant: Illuminant) -> (Vec<[f64; 3]>, [f64; 3]) {
    let v = variant(material, key);
    let path = face_up_swatch_path(v.unit, 7.0);
    let colors = body_colors(&v.absorption, path, illuminant);
    let mut dirs = vec![colors.o_ray.lab, colors.e_ray.lab];
    if let Some(beta) = colors.beta_ray {
        dirs.push(beta.lab);
    }
    (dirs, colors.unpolarised.lab)
}

#[test]
fn every_entry_is_present_and_per_mm() {
    for (material, key, _, _) in ENTRIES {
        let v = variant(material, key);
        assert_eq!(v.unit, AbsorptionUnit::PerMm, "{key}");
        assert!((v.reference_path_mm - 7.0).abs() < 1e-6, "{key}");
        assert!(
            matches!(v.provenance, VariantProvenance::BuiltIn { .. }),
            "{key}"
        );
    }
    assert_eq!(ENTRIES.len(), 21);
}

#[test]
fn face_up_and_direction_colors_match_the_fit_records() {
    for (material, key, face, directions) in ENTRIES {
        let (dirs, _) = app_labs(material, key, Illuminant::D65);
        let unpolarised = face_up_oriented(&variant(material, key), Illuminant::D65);
        let de = delta_e_2000(unpolarised, face);
        assert!(
            de < TOLERANCE_DE2000,
            "{key}: face-up {unpolarised:?} vs recorded {face:?}, dE2000 {de}"
        );
        if directions.is_empty() {
            continue;
        }
        assert_eq!(dirs.len(), directions.len(), "{key}: direction count");
        for (i, (got, want)) in dirs.iter().zip(directions).enumerate() {
            let de = delta_e_2000(*got, *want);
            assert!(
                de < TOLERANCE_DE2000,
                "{key}: direction {i} {got:?} vs recorded {want:?}, dE2000 {de}"
            );
        }
    }
}

#[test]
fn tensor_rank_follows_the_host() {
    for (material, key, _, directions) in ENTRIES {
        let v = variant(material, key);
        let expected = match directions.len() {
            0 => TensorRank::Isotropic,
            // The colorless zircon is an all-empty tensor: no pleochroism, so it is stored isotropic.
            _ if key == "zircon.colorless" => TensorRank::Isotropic,
            3 => TensorRank::Biaxial,
            _ => TensorRank::Uniaxial,
        };
        assert_eq!(tensor_rank(&v.absorption), expected, "{key}");
    }
}

#[test]
fn estimated_flags_are_as_recorded() {
    for (material, key, _, _) in ENTRIES {
        let v = variant(material, key);
        let VariantProvenance::BuiltIn {
            estimated,
            fit_record,
            sources,
        } = v.provenance
        else {
            panic!("{key} is not a built-in entry");
        };
        // Owner rule O3: a strength chosen for the look (or an unmeasured shape) is an estimate; every entry of
        // this group is one (the measured spectra are scaled to a typical face-up color).
        assert!(estimated, "{key}");
        assert!(!sources.is_empty(), "{key}");
        assert!(fit_record.is_some(), "{key}");
    }
}

#[test]
fn color_change_garnets_under_the_incandescent_lamp() {
    for (material, key, lamp) in INCANDESCENT {
        let (_, got) = app_labs(material, key, Illuminant::Planckian(3200.0));
        let de = delta_e_2000(got, lamp);
        assert!(
            de < TOLERANCE_DE2000,
            "{key}: {got:?} vs recorded {lamp:?}, dE2000 {de}"
        );
    }
    // Tunduru: grey-blue-green in daylight, purple under the lamp, a hue shift of about 90 degrees.
    let tunduru = "color_change_garnet__pyrope_spessartine_.tunduru";
    let material = "Color-Change Garnet (Pyrope-Spessartine)";
    let (_, day) = app_labs(material, tunduru, Illuminant::D65);
    let (_, lamp) = app_labs(material, tunduru, Illuminant::Planckian(3200.0));
    let shift = (lab_to_lch(lamp)[2] - lab_to_lch(day)[2]).rem_euclid(360.0);
    assert!((70.0..110.0).contains(&shift), "Tunduru hue shift {shift}");
    assert!(delta_e_2000(day, lamp) > 10.0);
    // Malaya barely changes.
    let malaya = "color_change_garnet__pyrope_spessartine_.malaya";
    let (_, day) = app_labs(material, malaya, Illuminant::D65);
    let (_, lamp) = app_labs(material, malaya, Illuminant::Planckian(3200.0));
    assert!(delta_e_2000(day, lamp) < 6.0);
}

#[test]
fn documented_pleochroism_holds() {
    // Zircon Blue: blue along E perp c, almost colorless along c.
    let (dirs, _) = app_labs("Zircon", "zircon.blue", Illuminant::D65);
    let (o, e) = (lab_to_lch(dirs[0]), lab_to_lch(dirs[1]));
    assert!(
        o[1] > 3.0 * e[1] && (200.0..280.0).contains(&o[2]),
        "o {o:?} e {e:?}"
    );
    // Benitoite Light Blue: blue along c, colorless across.
    let (dirs, _) = app_labs("Benitoite", "benitoite.light_blue", Illuminant::D65);
    let (o, e) = (lab_to_lch(dirs[0]), lab_to_lch(dirs[1]));
    assert!(e[1] > 10.0 * o[1] && o[0] > 98.0, "o {o:?} e {e:?}");
    // Unheated tanzanite: alpha red-violet, beta blue, gamma yellow-green.
    let (dirs, _) = app_labs("Tanzanite", "tanzanite.unheated", Illuminant::D65);
    let (alpha, gamma, beta) = (
        lab_to_lch(dirs[0]),
        lab_to_lch(dirs[1]),
        lab_to_lch(dirs[2]),
    );
    assert!((300.0..360.0).contains(&alpha[2]), "alpha {alpha:?}");
    assert!((230.0..290.0).contains(&beta[2]), "beta {beta:?}");
    assert!((100.0..160.0).contains(&gamma[2]), "gamma {gamma:?}");
    // Deep green peridot: the beta ray is the darkest, the hue is the same in all three.
    let (dirs, _) = app_labs("Peridot", "peridot.deep_green", Illuminant::D65);
    assert!(dirs[2][0] < dirs[0][0] && dirs[2][0] < dirs[1][0]);
    // Red-brown zircon is not dichroic in the visible: both rays agree to dE2000 below 2.
    let (dirs, _) = app_labs("Zircon", "zircon.red_brown", Illuminant::D65);
    assert!(delta_e_2000(dirs[0], dirs[1]) < 2.0);
}

#[test]
fn scaled_copies_show_the_default_scaled_at_the_seven_mm_swatch() {
    // (material, key, factor): the entry is the Material Default's model-unit peaks x factor,
    // stored per mm, so its face-up color at the per-mm path equals the default's factor-scaled
    // swatch at one model unit.
    for (material, key, factor) in [
        (
            "Spessartine Garnet",
            "spessartine_garnet.pale_golden_orange",
            0.4_f32,
        ),
        // Mint is half the default since 2026-11-04 (0.0875 x the GRR 229 vanadium): the default
        // went to 0.175 x that day (the photos' saturation), where the earlier Mint (0.18 x) sat,
        // so Mint was made paler to stay a different stone (dE2000 about 9 from the default).
        (
            "Grossular Garnet (Tsavorite)",
            "grossular_garnet__tsavorite_.mint",
            0.5,
        ),
        // Deep Green is 1.4 x the San Carlos crystal; the default is x 0.6 of it since 2026-11-04
        // (the photos' saturation), so Deep Green is 1.4 / 0.6 of the default.
        ("Peridot", "peridot.deep_green", 1.4 / 0.6),
    ] {
        let default = built_in_variants(material).remove(0);
        let mut scaled = default.absorption.clone();
        for band in scaled
            .o_ray
            .iter_mut()
            .chain(scaled.e_ray.iter_mut())
            .chain(scaled.beta_ray.iter_mut().flatten())
        {
            band.peak *= factor;
        }
        let want = body_colors(
            &scaled,
            face_up_swatch_path(default.unit, 7.0),
            Illuminant::D65,
        );
        let (_, got) = app_labs(material, key, Illuminant::D65);
        let de = delta_e_2000(got, want.unpolarised.lab);
        assert!(
            de < 0.5,
            "{key}: {got:?} vs scaled default {:?}, dE2000 {de}",
            want.unpolarised.lab
        );
    }
}

#[test]
fn colorless_zircon_is_empty_and_not_pleochroic() {
    let v = variant("Zircon", "zircon.colorless");
    assert!(!v.absorption.is_pleochroic);
    assert!(v.absorption.o_ray.is_empty() && v.absorption.e_ray.is_empty());
}

#[test]
fn keys_listed_in_the_tests_are_the_data_file_keys() {
    let expected: [&str; 21] = [
        "pyrope_garnet.rhodolite",
        "pyrope_garnet.chrome_pyrope",
        "almandine_garnet.purplish",
        "spessartine_garnet.reddish_orange",
        "spessartine_garnet.pale_golden_orange",
        "grossular_garnet__tsavorite_.mint",
        "grossular_garnet__tsavorite_.hessonite",
        "andradite_garnet__demantoid_.topazolite",
        "color_change_garnet__pyrope_spessartine_.tunduru",
        "color_change_garnet__pyrope_spessartine_.malaya",
        "color_change_garnet__pyrope_spessartine_.weaker_change",
        "color_change_garnet__pyrope_spessartine_.stronger_change",
        "peridot.deep_green",
        "zircon.blue",
        "zircon.golden_brown",
        "zircon.red_brown",
        "zircon.green",
        "zircon.colorless",
        "benitoite.light_blue",
        "tanzanite.unheated",
        "tanzanite.green_zoisite",
    ];
    for (i, (_, key, _, _)) in ENTRIES.iter().enumerate() {
        assert_eq!(*key, expected[i]);
    }
}
