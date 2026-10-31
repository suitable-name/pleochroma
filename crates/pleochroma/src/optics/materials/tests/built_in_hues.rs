//! Hue and shape relations of the built-in colored materials in the default (per model unit)
//! color mode, from the 2026-10-19 color check: the fits, the measured spectra they use
//! (`crates/pleochroma/data/reference_spectra/*.csv`) and the swatch values are documented in each
//! material's entry comment (`crates/pleochroma/src/optics/materials/*.rs`).
//!
//! No pixel is pinned: every assertion is a relation (a hue class, an ordering of two rays, a
//! path-length trend) on the 7 mm swatch. The swatch is the tensor's transmission over ONE model
//! unit of path (the band peaks are the total optical density over the face-up path of a 7 mm
//! stone, see `render_setup::materials::absorption_path_scale_for`), unpolarised, under D65, as
//! `color::body_color::body_colors` mixes it ((2 `T_o` + `T_e`) / 3 for uniaxial, the mean of the
//! three rays for biaxial).

use crate::{
    color::{
        Illuminant,
        body_color::{BodyColor, BodyColors, body_colors, lab_to_lch},
    },
    optics::{
        absorption::{AbsorptionBand, BandShape},
        materials::{AbsorptionUnit, GemMaterial, OpticalCharacter},
    },
    renderer::buffers::MAX_ABSORPTION_BANDS,
};

const PERIDOT: &str = "Peridot";
const SPESSARTINE: &str = "Spessartine Garnet";
const TANZANITE: &str = "Tanzanite";
const RUBY: &str = "Ruby";
const SAPPHIRE: &str = "Sapphire";
const EMERALD: &str = "Emerald";
const AQUAMARINE: &str = "Aquamarine";
const DEMANTOID: &str = "Andradite Garnet (Demantoid)";
const TSAVORITE: &str = "Grossular Garnet (Tsavorite)";
const BENITOITE: &str = "Benitoite";
const SPINEL: &str = "Spinel";
const AMETHYST: &str = "Amethyst";
const CITRINE: &str = "Citrine";
const MORGANITE: &str = "Morganite";
const CHRYSOBERYL: &str = "Chrysoberyl (Yellow)";
const TOPAZ: &str = "Topaz";
const ZIRCON: &str = "Zircon";
const TOURMALINE: &str = "Tourmaline";
const ANDALUSITE: &str = "Andalusite";

fn material(name: &str) -> GemMaterial {
    GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must be a built-in material"))
}

/// The swatch colors of `name` over `units` model units of path, under D65.
fn swatch(name: &str, units: f64) -> BodyColors {
    body_colors(&material(name).absorption, units, Illuminant::D65)
}

/// `[L*, C*ab, h_ab in degrees [0, 360)]` of one body color.
fn lch(color: &BodyColor) -> [f64; 3] {
    lab_to_lch(color.lab)
}

/// The unpolarised `[L*, C*ab, h_ab]` at `units` of path.
fn unpolarised(name: &str, units: f64) -> [f64; 3] {
    lch(&swatch(name, units).unpolarised)
}

/// Napierian absorption coefficient of a band set at `lambda_nm`.
fn alpha(bands: &[AbsorptionBand], lambda_nm: f32) -> f64 {
    f64::from(bands.iter().map(|b| b.evaluate(lambda_nm)).sum::<f32>())
}

/// Transmission of a band set over one model unit at `lambda_nm`.
fn transmission(bands: &[AbsorptionBand], lambda_nm: f32) -> f64 {
    (-alpha(bands, lambda_nm)).exp()
}

#[test]
fn peridot_swatch_is_yellow_green() {
    let [l, c, h] = unpolarised(PERIDOT, 1.0);
    assert!((100.0..=120.0).contains(&h), "Peridot hue {h:.1}");
    assert!(c >= 45.0, "Peridot chroma {c:.1}");
    assert!((65.0..=85.0).contains(&l), "Peridot lightness {l:.1}");
}

/// Caltech GRR 418 gives h 114 / 113 / 111 at 3 / 5 / 8.8 mm: yellow-green at every size.
#[test]
fn peridot_hue_is_stable_with_path() {
    let h1 = unpolarised(PERIDOT, 1.0)[2];
    let h2 = unpolarised(PERIDOT, 2.0)[2];
    assert!(
        (h2 - h1).abs() <= 6.0,
        "Peridot hue moved from {h1:.1} to {h2:.1} between 1 and 2 units"
    );
}

/// Fitted to Caltech GRR 43 (Fe-poor, "Mandarin" orange): the measured 7 mm swatch is a light
/// vivid orange (L* 77, C* 72, h 66), not the darker, over-saturated tuned stone.
#[test]
fn spessartine_swatch_is_orange() {
    let [l, c, h] = unpolarised(SPESSARTINE, 1.0);
    assert!((55.0..=75.0).contains(&h), "Spessartine hue {h:.1}");
    assert!(c >= 60.0, "Spessartine chroma {c:.1}");
    assert!((70.0..=85.0).contains(&l), "Spessartine lightness {l:.1}");
}

/// Heated tanzanite, fitted to Caltech GRR 2292 (blue zone): violet-blue, and the three rays are
/// ordered as in every measured crystal (GRR 1265, GRR 2292, GRR 3045 after heating): gamma
/// (the `e_ray`) is the lightest and bluest, alpha (the `o_ray`) the darkest and most purplish.
#[test]
fn tanzanite_swatch_is_violet_blue() {
    let colors = swatch(TANZANITE, 1.0);
    let [l, _, h] = lch(&colors.unpolarised);
    assert!((280.0..=305.0).contains(&h), "Tanzanite hue {h:.1}");
    assert!((38.0..=55.0).contains(&l), "Tanzanite lightness {l:.1}");
    let [l_alpha, _, h_alpha] = lch(&colors.o_ray);
    let [l_beta, _, h_beta] = lch(&colors.beta_ray.expect("Tanzanite is biaxial"));
    let [l_gamma, _, h_gamma] = lch(&colors.e_ray);
    assert!(
        l_gamma > l_beta && l_beta > l_alpha,
        "lightness must run gamma {l_gamma:.1} > beta {l_beta:.1} > alpha {l_alpha:.1}"
    );
    assert!(
        h_alpha > h_beta && h_beta > h_gamma,
        "hue must run alpha {h_alpha:.1} (most purplish) > beta {h_beta:.1} > gamma {h_gamma:.1}"
    );
}

#[test]
fn ruby_swatch_is_red_not_mauve() {
    let [l, c, h] = unpolarised(RUBY, 1.0);
    assert!(
        (345.0..360.0).contains(&h) || (0.0..=12.0).contains(&h),
        "Ruby hue {h:.1} must be red, not the old mauve (340)"
    );
    assert!(c >= 55.0, "Ruby chroma {c:.1}");
    assert!((33.0..=50.0).contains(&l), "Ruby lightness {l:.1}");
}

/// GIA Appendix 2: the unpolarised hue goes 348 -> 1 -> 11 with growing density.
#[test]
fn ruby_hue_rotates_toward_red_with_path() {
    let h1 = unpolarised(RUBY, 1.0)[2];
    let h2 = unpolarised(RUBY, 2.0)[2];
    let turn = (h2 - h1).rem_euclid(360.0);
    assert!(
        (5.0..=25.0).contains(&turn),
        "Ruby hue turned {turn:.1} degrees from {h1:.1} to {h2:.1}, expected 5 to 25"
    );
}

/// The o-ray (E perp c) carries the stronger Cr3+ U band (GIA sample 1110).
#[test]
fn ruby_o_ray_absorbs_more_than_e_ray_in_the_green() {
    let ruby = material(RUBY);
    let t_o = transmission(&ruby.absorption.o_ray, 560.0);
    let t_e = transmission(&ruby.absorption.e_ray, 560.0);
    assert!(
        t_o < t_e,
        "T_o(560) {t_o:.2e} must be below T_e(560) {t_e:.2e}"
    );
}

/// Fitted to Caltech GRR 1488: the e-ray (E parallel to c) is a deep violet-blue and the o-ray is
/// nearly colorless, so the unpolarised (2 o + e) / 3 swatch is a light blue (h 247), not cyan.
#[test]
fn aquamarine_swatch_is_light_blue_with_a_colorless_o_ray() {
    let colors = swatch(AQUAMARINE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((230.0..=260.0).contains(&h), "Aquamarine hue {h:.1}");
    assert!((10.0..=25.0).contains(&c), "Aquamarine chroma {c:.1}");
    assert!((80.0..=90.0).contains(&l), "Aquamarine lightness {l:.1}");
    let [l_o, c_o, _] = lch(&colors.o_ray);
    assert!(
        l_o >= 95.0 && c_o <= 20.0,
        "the o-ray must be near-colorless, got L* {l_o:.1} C* {c_o:.1}"
    );
    let l_e = lch(&colors.e_ray)[0];
    assert!(
        l_e <= 55.0,
        "the e-ray must be the deep blue one, got L* {l_e:.1}"
    );
    // The Fe2+-Fe3+ IVCT band (600-680 nm) is polarised along c: the e-ray is the dark one.
    let aquamarine = material(AQUAMARINE);
    let t_o = transmission(&aquamarine.absorption.o_ray, 680.0);
    let t_e = transmission(&aquamarine.absorption.e_ray, 680.0);
    assert!(
        t_e < t_o,
        "e-ray T(680) {t_e:.3} must be below o-ray T(680) {t_o:.3}"
    );
}

/// Fitted to Caltech GRR 229 (tsavorite) and GIA 2952 (demantoid): both green stones have no
/// transmission window at 650 nm (the 606 nm V3+ band, the 629 nm Cr3+ band) but a measured
/// deep-red window beyond 700 nm (T(740) 0.89 and 0.75); demantoid is the yellower.
#[test]
fn demantoid_and_tsavorite_are_green_with_no_window_at_650_nm() {
    let [_, _, h_demantoid] = unpolarised(DEMANTOID, 1.0);
    let [_, _, h_tsavorite] = unpolarised(TSAVORITE, 1.0);
    for (name, h) in [(DEMANTOID, h_demantoid), (TSAVORITE, h_tsavorite)] {
        assert!((130.0..=160.0).contains(&h), "{name} hue {h:.1}");
        let bands = &material(name).absorption.o_ray;
        let t_650 = transmission(bands, 650.0);
        assert!(t_650 < 0.2, "{name} T(650) {t_650:.3} must be below 0.2");
        let t_740 = transmission(bands, 740.0);
        assert!(
            t_740 > 0.6,
            "{name} T(740) {t_740:.3} must show the measured deep-red window"
        );
    }
    assert!(
        h_demantoid < h_tsavorite,
        "demantoid ({h_demantoid:.1}) must be yellower than tsavorite ({h_tsavorite:.1})"
    );
}

/// The measured-shape refits (owner decision 2026-10-19): the hue class stays blue / green and the
/// refits add the measured o/e dichroism.
#[test]
fn sapphire_and_emerald_refits_keep_their_hue_class() {
    let [l, _, h] = unpolarised(SAPPHIRE, 1.0);
    assert!((275.0..=292.0).contains(&h), "Sapphire hue {h:.1}");
    assert!((50.0..=65.0).contains(&l), "Sapphire lightness {l:.1}");

    let colors = swatch(EMERALD, 1.0);
    let [l, _, h] = lch(&colors.unpolarised);
    assert!((160.0..=175.0).contains(&h), "Emerald hue {h:.1}");
    assert!((58.0..=72.0).contains(&l), "Emerald lightness {l:.1}");
    // Wood & Nassau 1968: o-ray yellowish green, e-ray bluish green.
    let (h_o, h_e) = (lch(&colors.o_ray)[2], lch(&colors.e_ray)[2]);
    assert!(
        h_o < h_e,
        "Emerald o-ray hue {h_o:.1} must be yellower than the e-ray's {h_e:.1}"
    );
}

/// The refitted and rewritten entries are energy-domain (wavenumber Gaussian) band sets, as
/// fitted to the measured spectra; the entries the owner kept are still wavelength Gaussians.
///
/// 2026-10-27: Amethyst, Citrine, Morganite, Chrysoberyl (Yellow), Topaz, Zircon, Tourmaline and
/// Andalusite moved to the first list when they got their per-axis tensors (they were wavelength
/// Gaussians, and Amethyst, Citrine and Tourmaline were the "kept" ones of this test).
#[test]
fn refitted_entries_use_energy_bands_and_kept_entries_do_not() {
    let all_energy = |name: &str| {
        let t = material(name).absorption;
        t.o_ray
            .iter()
            .chain(&t.e_ray)
            .chain(t.beta_ray.iter().flatten())
            .all(|b| b.shape == BandShape::GaussianEnergy)
    };
    for name in [
        PERIDOT,
        SPESSARTINE,
        TANZANITE,
        RUBY,
        SAPPHIRE,
        EMERALD,
        AQUAMARINE,
        DEMANTOID,
        TSAVORITE,
        AMETHYST,
        CITRINE,
        MORGANITE,
        CHRYSOBERYL,
        TOPAZ,
        ZIRCON,
        TOURMALINE,
        ANDALUSITE,
    ] {
        assert!(all_energy(name), "{name} must carry energy-domain bands");
    }
    for name in ["Pyrope Garnet", SPINEL] {
        assert!(!all_energy(name), "{name} must keep its wavelength bands");
    }
}

/// Alpha of the `o_ray`, `e_ray` or `beta_ray` set of `name` at `lambda_nm`.
fn ray_alpha(name: &str, ray: Ray, lambda_nm: f32) -> f64 {
    let t = material(name).absorption;
    let bands = match ray {
        Ray::O => t.o_ray,
        Ray::E => t.e_ray,
        Ray::Beta => t.beta_ray.expect("a biaxial material has a beta ray"),
    };
    alpha(&bands, lambda_nm)
}

/// Which band set of a tensor.
#[derive(Clone, Copy)]
enum Ray {
    O,
    E,
    Beta,
}

/// Amethyst (2026-10-27, per-axis): the free four-band fit of the Caltech Anahi o-ray spectrum at one
/// factor 2.47 (model / data 0.98-1.02), the e-ray at the Report's weights. The swatch stays purple
/// (the old tuned swatch was L* 59 C* 66 h 321; the Python replica of the app's colorimetry, not a
/// run, gives unpolarised L* 54 C* 35 h 331, o-ray L* 50 C* 41 h 329, e-ray L* 61 C* 27 h 336), and
/// the weaker-absorbing e-ray is the lighter one.
#[test]
fn amethyst_is_purple_and_its_e_ray_is_the_lighter_ray() {
    let colors = swatch(AMETHYST, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((305.0..=340.0).contains(&h), "Amethyst hue {h:.1}");
    assert!((48.0..=62.0).contains(&l), "Amethyst lightness {l:.1}");
    assert!((28.0..=45.0).contains(&c), "Amethyst chroma {c:.1}");
    let (l_o, l_e) = (lch(&colors.o_ray)[0], lch(&colors.e_ray)[0]);
    assert!(
        l_o < l_e,
        "the o-ray (L* {l_o:.1}) must absorb more than the e-ray (L* {l_e:.1})"
    );
    let t = material(AMETHYST).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
    assert_eq!((t.o_ray.len(), t.e_ray.len()), (4, 4));
}

/// Citrine (2026-10-27, per-axis): the Fe3+ charge-transfer edge of the citrine table, equal in
/// both rays (the pleochroism of citrine is negligible) but stored pleochroic. Hand evaluation, not
/// a run: L* 92 C* 51 h 94 against the old L* 93 C* 49 h 91.
#[test]
fn citrine_is_golden_yellow_with_equal_rays_stored_pleochroic() {
    let [l, c, h] = unpolarised(CITRINE, 1.0);
    assert!((85.0..=105.0).contains(&h), "Citrine hue {h:.1}");
    assert!((86.0..=96.0).contains(&l), "Citrine lightness {l:.1}");
    assert!((40.0..=62.0).contains(&c), "Citrine chroma {c:.1}");
    let t = material(CITRINE).absorption;
    assert!(t.is_pleochroic, "an isotropic tensor on a uniaxial host");
    assert_eq!(t.o_ray, t.e_ray);
    assert!(t.beta_ray.is_none());
}

/// Morganite (2026-10-27, per-axis): the free fit of the Caltech red beryl GRR 602 (UV edge and the
/// 440 / 496 / 555 nm Mn3+ structure in the o-ray, 440 / 544 nm in the e-ray) at one factor 0.202,
/// which is the documented "light red to light violet" pleochroism: the o-ray is the pinker one.
/// Python replica of the app's colorimetry, not a run: unpolarised L* 79 C* 32 h 342 (the old swatch
/// was L* 77 C* 47 h 325), o-ray L* 81 C* 33 h 347 (what the stone shows with c along the view),
/// e-ray L* 75 C* 31 h 332.
#[test]
fn morganite_is_pink_and_its_o_ray_is_pinker_than_its_e_ray() {
    let colors = swatch(MORGANITE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((330.0..=355.0).contains(&h), "Morganite hue {h:.1}");
    assert!((74.0..=85.0).contains(&l), "Morganite lightness {l:.1}");
    assert!((26.0..=45.0).contains(&c), "Morganite chroma {c:.1}");
    let (h_o, h_e) = (lch(&colors.o_ray)[2], lch(&colors.e_ray)[2]);
    assert!(
        h_o > h_e,
        "the o-ray hue {h_o:.1} must be pinker (higher) than the e-ray's {h_e:.1}"
    );
    let t = material(MORGANITE).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
    assert_eq!((t.o_ray.len(), t.e_ray.len()), (4, 3));
}

/// Chrysoberyl (Yellow) (2026-10-27, per-axis, biaxial): the Fe3+ bands of Caltech GRR 171 on beta
/// (E par a) and gamma (E par b), the alpha ray (E par c) estimated as their mean, at ONE factor 0.7
/// over every band (owner decision 2026-10-27: a golden yellow; the earlier 0.25 duplicated the
/// Greenish Yellow variant). Python replica of the app's colorimetry, not a run: unpolarised
/// L* 72 C* 53 h 83 (old tuned swatch L* 97 C* 56 h 103), alpha L* 72 C* 56, beta L* 76 C* 42,
/// gamma L* 69 C* 67; with gamma along the view the stone shows alpha and beta, L* 74 C* 48 h 88.
/// The gamma ray, with the strong 441 nm band, is the most saturated.
#[test]
fn chrysoberyl_yellow_is_a_trichroic_golden_yellow() {
    let colors = swatch(CHRYSOBERYL, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((78.0..=96.0).contains(&h), "Chrysoberyl hue {h:.1}");
    assert!((68.0..=80.0).contains(&l), "Chrysoberyl lightness {l:.1}");
    assert!((45.0..=60.0).contains(&c), "Chrysoberyl chroma {c:.1}");
    let beta = colors.beta_ray.expect("Chrysoberyl (Yellow) is biaxial");
    let (c_beta, c_gamma) = (lch(&beta)[1], lch(&colors.e_ray)[1]);
    assert!(
        c_gamma > c_beta,
        "gamma (C* {c_gamma:.1}) must be more saturated than beta (C* {c_beta:.1})"
    );
    let t = material(CHRYSOBERYL).absorption;
    assert!(t.is_pleochroic);
}

/// Topaz (2026-10-27, per-axis, biaxial): the O- hole-centre band of the neutron-irradiated blue
/// topaz GRR 1632 is strongest along ALPHA (about 5 x gamma at 660 nm), beta is nearly colorless;
/// the old "maximum along beta" is not supported. Hand evaluation, not a run: alpha L* 33 C* 43
/// h 241, beta L* 93 C* 4, gamma L* 74 C* 21 h 215; unpolarised L* 74 C* 14 h 227 (the old
/// isotropic swatch was L* 69 C* 45 h 266: the measured pleochroism makes the mean paler).
#[test]
fn topaz_alpha_is_the_deep_blue_ray_and_beta_is_colorless() {
    let colors = swatch(TOPAZ, 1.0);
    let beta = colors.beta_ray.expect("Topaz is biaxial");
    let [l_alpha, c_alpha, h_alpha] = lch(&colors.o_ray);
    let [l_beta, c_beta, _] = lch(&beta);
    let l_gamma = lch(&colors.e_ray)[0];
    assert!(
        l_alpha < l_gamma && l_gamma < l_beta,
        "lightness must run alpha {l_alpha:.1} < gamma {l_gamma:.1} < beta {l_beta:.1}"
    );
    assert!(c_beta < 8.0, "beta chroma {c_beta:.1} must be near zero");
    assert!(c_alpha > 30.0, "alpha chroma {c_alpha:.1}");
    assert!(
        (225.0..=255.0).contains(&h_alpha),
        "alpha hue {h_alpha:.1} must be blue"
    );
    let h = lch(&colors.unpolarised)[2];
    assert!((205.0..=250.0).contains(&h), "Topaz unpolarised hue {h:.1}");
    let (alpha_660, beta_660, gamma_660) = (
        ray_alpha(TOPAZ, Ray::O, 660.0),
        ray_alpha(TOPAZ, Ray::Beta, 660.0),
        ray_alpha(TOPAZ, Ray::E, 660.0),
    );
    assert!(
        (3.0..=8.0).contains(&(alpha_660 / gamma_660)) && beta_660 < 0.1 * alpha_660,
        "alpha {alpha_660:.2}, beta {beta_660:.2}, gamma {gamma_660:.2} at 660 nm"
    );
}

/// Zircon (2026-10-27, per-axis): the U4+ lines are measured per ray (Caltech GRR 3106), and the
/// two rays differ: 653 nm is about twice as strong along o, the 691 nm line is o only, the 616 nm
/// line e only. Hand evaluation, not a run: unpolarised L* 93 C* 12 h 303 (the old swatch was
/// L* 93 C* 13 h 279), o-ray h 318, e-ray h 256.
#[test]
fn zircon_carries_the_measured_o_and_e_line_split() {
    let t = material(ZIRCON).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
    assert_ne!(t.o_ray, t.e_ray, "the measured rays differ");
    assert!(t.o_ray.len() <= MAX_ABSORPTION_BANDS && t.e_ray.len() <= MAX_ABSORPTION_BANDS);
    let (o_653, e_653) = (
        ray_alpha(ZIRCON, Ray::O, 653.4),
        ray_alpha(ZIRCON, Ray::E, 653.4),
    );
    assert!(
        o_653 > 1.5 * e_653,
        "653 nm: o {o_653:.2} against e {e_653:.2}"
    );
    assert!(ray_alpha(ZIRCON, Ray::O, 690.9) > 5.0);
    assert!(ray_alpha(ZIRCON, Ray::E, 690.9) < 0.1);
    assert!(ray_alpha(ZIRCON, Ray::E, 616.0) > 3.0);
    assert!(ray_alpha(ZIRCON, Ray::O, 616.0) < 0.1);
    let [l, c, h] = unpolarised(ZIRCON, 1.0);
    assert!((285.0..=320.0).contains(&h), "Zircon hue {h:.1}");
    assert!(l >= 90.0, "Zircon lightness {l:.1}");
    assert!((5.0..=20.0).contains(&c), "Zircon chroma {c:.1}");
}

/// Tourmaline (2026-10-27, refitted to the Caltech elbaite GRR 1935): the Fe2+ band at 711 nm is
/// NOT strongly dichroic (o : e about 1.2, not the old 3 : 1; blue GRR 1935, GRR 3770 and the
/// green crystal all measure o : e about 1), the polarisation lives in the UV / blue edge (o about
/// 3 x e at 400 nm). Every band at the measured spectrum's one strength (owner 2026-10-27: measured
/// values win). Python replica of the pipeline, not a run: unpolarised L* 91.4 C* 20.4 h 148.1 (the
/// old swatch was L* 93 C* 49 h 121), o-ray L* 89.9 C* 27.2 h 142.3, e-ray L* 94.2 C* 9.8 h 174.9.
#[test]
fn tourmaline_iron_band_is_not_dichroic_but_the_blue_edge_is() {
    let fe_ratio = ray_alpha(TOURMALINE, Ray::O, 711.0) / ray_alpha(TOURMALINE, Ray::E, 711.0);
    assert!(
        (1.0..=1.4).contains(&fe_ratio),
        "Fe2+ o : e at 711 nm is {fe_ratio:.2}, measured about 1.1 to 1.2"
    );
    let edge_ratio = ray_alpha(TOURMALINE, Ray::O, 400.0) / ray_alpha(TOURMALINE, Ray::E, 400.0);
    assert!(
        (2.0..=4.0).contains(&edge_ratio),
        "edge o : e at 400 nm is {edge_ratio:.2}, measured about 2"
    );
    let colors = swatch(TOURMALINE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((140.0..=156.0).contains(&h), "Tourmaline hue {h:.1}");
    assert!((88.0..=95.0).contains(&l), "Tourmaline lightness {l:.1}");
    assert!((14.0..=27.0).contains(&c), "Tourmaline chroma {c:.1}");
    let (l_o, l_e) = (lch(&colors.o_ray)[0], lch(&colors.e_ray)[0]);
    assert!(l_o < l_e, "the o-ray (L* {l_o:.1}) is the deeper ray");
    let t = material(TOURMALINE).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
}

/// Andalusite (2026-10-27, refitted to the Caltech Fe-Ti crystal): ALPHA (E par c, the `o_ray`
/// slot, `n_alpha`) carries the Fe2+-Ti4+ colour, beta and gamma are nearly colorless. The old
/// qualitative entry had its strongest band on beta. Hand evaluation, not a run: alpha L* 63 C* 39
/// h 41 (brownish red), beta L* 98 C* 7 h 108, gamma L* 99 C* 3 h 111, unpolarised L* 89 C* 9 h 60
/// (the old swatch was L* 82 C* 22 h 356, a pink: the research shows it was wrong).
#[test]
fn andalusite_alpha_carries_the_iron_titanium_color() {
    let (alpha_486, beta_486, gamma_486) = (
        ray_alpha(ANDALUSITE, Ray::O, 486.0),
        ray_alpha(ANDALUSITE, Ray::Beta, 486.0),
        ray_alpha(ANDALUSITE, Ray::E, 486.0),
    );
    assert!(
        alpha_486 > 10.0 * beta_486 && alpha_486 > 10.0 * gamma_486,
        "alpha {alpha_486:.2} must dwarf beta {beta_486:.2} and gamma {gamma_486:.2} at 486 nm"
    );
    let colors = swatch(ANDALUSITE, 1.0);
    let [l_alpha, c_alpha, h_alpha] = lch(&colors.o_ray);
    assert!(
        (15.0..=70.0).contains(&h_alpha) && c_alpha > 25.0,
        "alpha must be a brownish red, got C* {c_alpha:.1} h {h_alpha:.1}"
    );
    let beta = colors.beta_ray.expect("Andalusite is biaxial");
    for (name, ray) in [("beta", lch(&beta)), ("gamma", lch(&colors.e_ray))] {
        assert!(
            ray[1] < 12.0 && ray[0] > l_alpha + 20.0,
            "{name} must be pale: L* {:.1} C* {:.1}",
            ray[0],
            ray[1]
        );
    }
}

/// A colored stone on an anisotropic host never carries an isotropic tensor (owner decision
/// 2026-10-27): every colored built-in on a uniaxial host has a pleochroic `o_ray` / `e_ray`
/// tensor and no `beta_ray`, every one on a biaxial host a pleochroic tensor with a `beta_ray`.
/// A colorless stone (all band sets empty) has no pleochroism to keep and stays the empty
/// isotropic tensor, so its render path is unchanged. Isotropic hosts keep the isotropic tensor.
/// The zoned Ametrine's color is its zones and is checked by its own tests.
#[test]
fn every_built_in_on_an_anisotropic_host_has_a_pleochroic_tensor() {
    for material in GemMaterial::all_materials() {
        let t = &material.absorption;
        let colorless = t.o_ray.is_empty()
            && t.e_ray.is_empty()
            && t.beta_ray.as_ref().is_none_or(Vec::is_empty);
        if colorless || material.zoning.is_some() {
            continue;
        }
        match material.optical_character {
            OpticalCharacter::Isotropic => assert!(
                !t.is_pleochroic && t.beta_ray.is_none(),
                "{}: an isotropic host keeps the isotropic tensor",
                material.name
            ),
            OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => assert!(
                t.is_pleochroic && t.beta_ray.is_none(),
                "{}: a uniaxial host needs a pleochroic o / e tensor, got pleochroic {} beta {}",
                material.name,
                t.is_pleochroic,
                t.beta_ray.is_some()
            ),
            OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => assert!(
                t.is_pleochroic && t.beta_ray.is_some(),
                "{}: a biaxial host needs a pleochroic alpha / beta / gamma tensor, got \
                 pleochroic {} beta {}",
                material.name,
                t.is_pleochroic,
                t.beta_ray.is_some()
            ),
        }
    }
}

/// The colorless stones on anisotropic hosts keep the empty isotropic tensor (no stand-in bands,
/// no pleochroic flag): Quartz (rock crystal), Synthetic Moissanite, and the feldspars Labradorite
/// and Moonstone. Their render path and every pin that uses them stay as they were.
#[test]
fn colorless_stones_on_anisotropic_hosts_stay_empty_and_isotropic() {
    for name in ["Quartz", "Synthetic Moissanite", "Labradorite", "Moonstone"] {
        let t = material(name).absorption;
        assert!(!t.is_pleochroic, "{name} keeps the isotropic tensor");
        assert!(
            t.o_ray.is_empty() && t.e_ray.is_empty() && t.beta_ray.is_none(),
            "{name} stays colorless"
        );
    }
}

/// The Ametrine does not move with Citrine's tensor: its zones are the catalogue's quartz recipes
/// and its plain absorption is the amethyst zone, never Citrine's bands.
#[test]
fn ametrine_is_untouched_by_the_citrine_and_amethyst_per_axis_tensors() {
    let ametrine = material("Ametrine");
    let zoning = ametrine.zoning.as_ref().expect("Ametrine is zoned");
    assert_eq!(ametrine.absorption, zoning.base.tensor);
    assert_ne!(ametrine.absorption, material(CITRINE).absorption);
    assert_ne!(ametrine.absorption, material(AMETHYST).absorption);
}

/// The GPU material buffer holds [`MAX_ABSORPTION_BANDS`] bands per ray; a longer set would be
/// truncated silently.
#[test]
fn every_built_in_fits_the_gpu_band_limit() {
    for material in GemMaterial::all_materials() {
        let t = &material.absorption;
        let sets = [Some(&t.o_ray), Some(&t.e_ray), t.beta_ray.as_ref()];
        for (ray, set) in ["o", "e", "beta"].into_iter().zip(sets) {
            let count = set.map_or(0, Vec::len);
            assert!(
                count <= MAX_ABSORPTION_BANDS,
                "{}: the {ray} ray has {count} bands, the GPU holds {MAX_ABSORPTION_BANDS}",
                material.name
            );
        }
    }
}

/// The per-millimetre built-in (the zoned Ametrine) is not a band-data entry: the color check
/// must not have turned it into a per-model-unit one, and its optics stay Citrine's.
#[test]
fn per_mm_built_ins_keep_their_unit_and_zoning() {
    let ametrine = material("Ametrine");
    assert_eq!(ametrine.absorption_unit, AbsorptionUnit::PerMm);
    assert!(ametrine.zoning.is_some(), "Ametrine keeps its zoning");
    let citrine = material("Citrine");
    assert_eq!(ametrine.dispersion, citrine.dispersion);
    assert_eq!(ametrine.c_axis, citrine.c_axis);
    assert_eq!(
        ametrine.birefringence_delta.to_bits(),
        citrine.birefringence_delta.to_bits()
    );
}

/// Benitoite (Caltech GRR 561-D4, refitted 2026-10-26): the e-ray (E parallel to c) is blue, the
/// o-ray (E perpendicular to c) is nearly colorless and lighter, and the unpolarised mix is blue.
///
/// Expected on the 7 mm swatch (a hand evaluation of the fitted bands, not a run): e-ray L* 70,
/// C* 45, h 263; o-ray L* 96, C* 4; unpolarised (2 o + e) / 3 L* 89, C* 15, h 268.
#[test]
fn benitoite_e_ray_is_blue_and_o_ray_is_nearly_colorless() {
    let colors = swatch(BENITOITE, 1.0);
    let [l_e, _, h_e] = lch(&colors.e_ray);
    let [l_o, c_o, _] = lch(&colors.o_ray);
    assert!(
        (240.0..=300.0).contains(&h_e),
        "Benitoite e-ray hue {h_e:.1}"
    );
    assert!(
        c_o < 12.0,
        "Benitoite o-ray chroma {c_o:.1} must be near zero"
    );
    assert!(
        l_o > l_e,
        "the o-ray (L* {l_o:.1}) must be lighter than the e-ray (L* {l_e:.1})"
    );
    let h = lch(&colors.unpolarised)[2];
    assert!(
        (240.0..=300.0).contains(&h),
        "Benitoite unpolarised hue {h:.1}"
    );
}

/// The fitted e-ray band is the measured one: a Gaussian in energy centred at 698 nm with a FWHM
/// of 0.88 eV, so the absorption at the half-maximum wavelengths 559 and 929 nm (the measured
/// half-maximum points) is about half the peak, and the o-ray is about 5 % of the e-ray at 698 nm.
///
/// Arithmetic: the e-ray half width is 3550 cm^-1 around 1e7 / 698 = 14 327 cm^-1, so the half
/// maximum sits at 17 877 cm^-1 (559.4 nm) and 10 777 cm^-1 (927.9 nm); the o-ray at 698 nm is
/// 0.11 x exp(-0.5 x (2 257 / 3 665)^2) = 0.091, i.e. 0.046 of the e-ray's 2.0.
#[test]
fn benitoite_band_shape_is_the_measured_one() {
    let benitoite = material(BENITOITE);
    let e_peak = alpha(&benitoite.absorption.e_ray, 698.0);
    for lambda_nm in [559.0, 929.0] {
        let ratio = alpha(&benitoite.absorption.e_ray, lambda_nm) / e_peak;
        assert!(
            (0.40..=0.60).contains(&ratio),
            "e-ray absorption at {lambda_nm} nm is {ratio:.3} of the 698 nm peak, expected about 0.5"
        );
    }
    let o_ratio = alpha(&benitoite.absorption.o_ray, 698.0) / e_peak;
    assert!(
        (0.03..=0.08).contains(&o_ratio),
        "o-ray / e-ray absorption at 698 nm is {o_ratio:.3}, expected about 0.05"
    );
}

/// Red spinel (Caltech JQ-13, second Cr3+ band added 2026-10-26): the 4A2 -> 4T1 band near 395 nm
/// closes the violet leak that the one-band preset had (the unpolarised swatch was mauve, h about
/// 325, because blue and violet passed with the red), so the swatch is a red, and the absorption
/// at 400 nm exceeds the valley at 470 nm, which the one-band preset (0.09 against 1.07) fails.
///
/// Expected (a hand evaluation, not a run): L* 50, C* 45, h 345; the absorption is 3.87 at 400 nm
/// against 1.15 at 470 nm.
#[test]
fn red_spinel_swatch_is_red_with_the_violet_leak_closed() {
    let [_, c, h] = unpolarised(SPINEL, 1.0);
    assert!(
        (338.0..360.0).contains(&h) || (0.0..=25.0).contains(&h),
        "Spinel hue {h:.1} must be red, not the one-band mauve (about 325)"
    );
    assert!(c > 30.0, "Spinel chroma {c:.1}");
    let bands = &material(SPINEL).absorption.o_ray;
    let (a_400, a_470) = (alpha(bands, 400.0), alpha(bands, 470.0));
    assert!(
        a_400 >= 1.2 * a_470,
        "the 400 nm absorption {a_400:.2} must be at least 1.2 x the 470 nm valley {a_470:.2}"
    );
}

#[test]
fn colourless_built_ins_stay_empty() {
    for name in ["Diamond", "Synthetic Moissanite", "Cubic Zirconia"] {
        let t = material(name).absorption;
        assert!(
            t.o_ray.is_empty() && t.e_ray.is_empty() && t.beta_ray.is_none(),
            "{name} must stay colorless"
        );
    }
}
