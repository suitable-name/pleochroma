//! Hue and shape relations of the built-in colored materials in the default (per model unit) color
//! mode.
//!
//! The fits, their measured spectra (`crates/pleochroma/data/reference_spectra/`) and the swatch
//! values are documented in each material's entry comment.
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
const PYROPE: &str = "Pyrope Garnet";
const ALMANDINE: &str = "Almandine Garnet";

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

/// Peridot at 0.6 of the San Carlos crystal's strength, to match the dealer photos' saturation.
#[test]
fn peridot_swatch_is_yellow_green() {
    let [l, c, h] = unpolarised(PERIDOT, 1.0);
    assert!((100.0..=120.0).contains(&h), "Peridot hue {h:.1}");
    assert!(c >= 40.0, "Peridot chroma {c:.1}");
    assert!((76.0..=90.0).contains(&l), "Peridot lightness {l:.1}");
}

/// The hue holds across path lengths (Caltech GRR 418 spectrum): yellow-green at every size.
#[test]
fn peridot_hue_is_stable_with_path() {
    let h1 = unpolarised(PERIDOT, 1.0)[2];
    let h2 = unpolarised(PERIDOT, 2.0)[2];
    assert!(
        (h2 - h1).abs() <= 6.0,
        "Peridot hue moved from {h1:.1} to {h2:.1} between 1 and 2 units"
    );
}

/// Fitted to Caltech GRR 43 (Fe-poor, "Mandarin" orange): a light, vivid orange.
#[test]
fn spessartine_swatch_is_orange() {
    let [l, c, h] = unpolarised(SPESSARTINE, 1.0);
    assert!((55.0..=75.0).contains(&h), "Spessartine hue {h:.1}");
    assert!(c >= 60.0, "Spessartine chroma {c:.1}");
    assert!((70.0..=85.0).contains(&l), "Spessartine lightness {l:.1}");
}

/// Heated tanzanite, fitted to Caltech GRR 2292 (blue zone): violet-blue. The three rays are
/// ordered as in every measured crystal (GRR 1265, GRR 2292, GRR 3045 after heating): gamma
/// (the `e_ray`) is the lightest and bluest, alpha (the `o_ray`) the darkest and most purplish.
/// Strength 0.7 of the crystal, to match the dealer photos' saturation.
#[test]
fn tanzanite_swatch_is_violet_blue() {
    let colors = swatch(TANZANITE, 1.0);
    let [l, _, h] = lch(&colors.unpolarised);
    assert!((280.0..=305.0).contains(&h), "Tanzanite hue {h:.1}");
    assert!((48.0..=64.0).contains(&l), "Tanzanite lightness {l:.1}");
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

/// The GIA Cr3+ + Fe3+ ruby (Cr 400, Fe 1000 ppma*cm): a red, not the magenta-pink of Cr3+
/// alone (hue about 356).
#[test]
fn ruby_swatch_is_red_not_mauve() {
    let [l, c, h] = unpolarised(RUBY, 1.0);
    assert!(
        (8.0..=28.0).contains(&h),
        "Ruby hue {h:.1} must be red, not the old mauve (340) or the pink of Cr3+ alone (356)"
    );
    assert!(c >= 48.0, "Ruby chroma {c:.1}");
    assert!((38.0..=54.0).contains(&l), "Ruby lightness {l:.1}");
}

/// The hue turns toward red with path length, as in GIA's Cr3+ data (Appendix 2).
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

/// Fitted to Caltech GRR 1488 and mixed with the Greenish variant to match the dealer photos.
/// The e-ray (E parallel to c) is a violet-blue and the o-ray nearly colorless, so the
/// unpolarised (2 o + e) / 3 swatch is a pale blue, not cyan.
#[test]
fn aquamarine_swatch_is_light_blue_with_a_colorless_o_ray() {
    let colors = swatch(AQUAMARINE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((225.0..=260.0).contains(&h), "Aquamarine hue {h:.1}");
    assert!((6.0..=16.0).contains(&c), "Aquamarine chroma {c:.1}");
    assert!((88.0..=96.0).contains(&l), "Aquamarine lightness {l:.1}");
    let [l_o, c_o, _] = lch(&colors.o_ray);
    assert!(
        l_o >= 93.0 && c_o <= 20.0,
        "the o-ray must be near-colorless, got L* {l_o:.1} C* {c_o:.1}"
    );
    let [l_e, c_e, _] = lch(&colors.e_ray);
    assert!(
        l_e + 12.0 <= l_o && c_e >= 3.0 * c_o,
        "the e-ray must be the blue one, got L* {l_e:.1} C* {c_e:.1} against the o-ray's \
         L* {l_o:.1} C* {c_o:.1}"
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

/// Fitted to Caltech GRR 229 (tsavorite) and GIA 2952 (demantoid). Both absorb at 650 nm (the
/// 606 nm V3+ band, the 629 nm Cr3+ band) and keep a deep-red window beyond 700 nm. Tsavorite
/// and demantoid are scaled down to match the lighter dealer photos, so the 650 nm dip only
/// dims the window. Demantoid is the yellower.
#[test]
fn demantoid_and_tsavorite_are_green_with_no_window_at_650_nm() {
    let [_, _, h_demantoid] = unpolarised(DEMANTOID, 1.0);
    let [_, _, h_tsavorite] = unpolarised(TSAVORITE, 1.0);
    assert!(
        (105.0..=128.0).contains(&h_demantoid),
        "{DEMANTOID} hue {h_demantoid:.1}"
    );
    assert!(
        (130.0..=150.0).contains(&h_tsavorite),
        "{TSAVORITE} hue {h_tsavorite:.1}"
    );
    for name in [DEMANTOID, TSAVORITE] {
        let bands = &material(name).absorption.o_ray;
        let t_650 = transmission(bands, 650.0);
        assert!(t_650 < 0.75, "{name} T(650) {t_650:.3} must be below 0.75");
        let t_740 = transmission(bands, 740.0);
        assert!(
            t_740 > 0.6 && t_740 - t_650 > 0.2,
            "{name} T(740) {t_740:.3} must show the measured deep-red window above T(650) \
             {t_650:.3}"
        );
    }
    assert!(
        h_demantoid < h_tsavorite,
        "demantoid ({h_demantoid:.1}) must be yellower than tsavorite ({h_tsavorite:.1})"
    );
}

/// The measured-shape fits keep the hue class (blue sapphire, green emerald) and add the
/// measured o/e dichroism.
#[test]
fn sapphire_and_emerald_refits_keep_their_hue_class() {
    let [l, _, h] = unpolarised(SAPPHIRE, 1.0);
    assert!((275.0..=292.0).contains(&h), "Sapphire hue {h:.1}");
    // The royal blue default (12.5 ppma*cm).
    assert!((44.0..=56.0).contains(&l), "Sapphire lightness {l:.1}");

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

/// The colored built-ins carry energy-domain (wavenumber Gaussian) band sets, as fitted to
/// their measured spectra.
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
        PYROPE,
        ALMANDINE,
        SPINEL,
    ] {
        assert!(all_energy(name), "{name} must carry energy-domain bands");
    }
}

/// Benitoite is cut with its c axis in the table: its blue is the e-ray, so a c axis along the
/// table normal would show face-up only the nearly colorless o-ray.
#[test]
fn benitoite_lies_with_its_c_axis_in_the_table() {
    let c = material(BENITOITE).c_axis;
    assert!(c.y.abs() < 1e-6, "c axis {c:?} must lie in the table plane");
    assert!(
        (c.length() - 1.0).abs() < 1e-6,
        "c axis {c:?} is a unit vector"
    );
}

/// Pyrope (Caltech GRR 1055) and Almandine (GRR 1776) are a deep red, slightly orange (they
/// carry the Mn2+ of the Spessartine built-in). The ultraviolet edge keeps the blue closed while
/// the red window stays open.
#[test]
fn pyrope_and_almandine_swatches_are_deep_red_not_magenta() {
    for name in [PYROPE, ALMANDINE] {
        let [l, c, h] = unpolarised(name, 1.0);
        assert!(h < 40.0 || h > 352.0, "{name} hue {h:.1} must be red");
        assert!((20.0..=50.0).contains(&l), "{name} lightness {l:.1}");
        assert!(c >= 20.0, "{name} chroma {c:.1}");
        let bands = material(name).absorption.o_ray;
        assert!(
            transmission(&bands, 450.0) < transmission(&bands, 650.0) * 0.5,
            "{name} must absorb the blue far more than the red"
        );
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

/// Amethyst: a free four-band fit of the Caltech Anahi o-ray spectrum (model / data 0.98-1.02),
/// with its main band at 549 nm to match the bluer dealer purple. The swatch is purple, and the
/// weaker-absorbing e-ray is the lighter one.
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

/// Citrine: the Fe3+ charge-transfer edge, equal in both rays (the pleochroism of citrine is
/// negligible) but stored pleochroic. The strength matches golden dealer photos.
#[test]
fn citrine_is_golden_yellow_with_equal_rays_stored_pleochroic() {
    let [l, c, h] = unpolarised(CITRINE, 1.0);
    assert!((78.0..=100.0).contains(&h), "Citrine hue {h:.1}");
    assert!((79.0..=91.0).contains(&l), "Citrine lightness {l:.1}");
    assert!((65.0..=88.0).contains(&c), "Citrine chroma {c:.1}");
    let t = material(CITRINE).absorption;
    assert!(t.is_pleochroic, "an isotropic tensor on a uniaxial host");
    assert_eq!(t.o_ray, t.e_ray);
    assert!(t.beta_ray.is_none());
}

/// Morganite: the free fit of the Caltech red beryl GRR 602 (UV edge and the 440 / 496 / 555 nm
/// Mn3+ structure in the o-ray, 440 / 544 nm in the e-ray), which gives the documented light red
/// to light violet pleochroism. The o-ray is the pinker one.
#[test]
fn morganite_is_pink_and_its_o_ray_is_pinker_than_its_e_ray() {
    // Mixed with the Peach variant (the dealer photos, a salmon pink) and scaled to the photos'
    // saturation: the unpolarised color is a salmon pink, the o-ray pink and the e-ray peach.
    let colors = swatch(MORGANITE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!(
        (5.0..=30.0).contains(&h),
        "Morganite hue {h:.1} must be a salmon pink"
    );
    assert!((84.0..=94.0).contains(&l), "Morganite lightness {l:.1}");
    assert!((8.0..=18.0).contains(&c), "Morganite chroma {c:.1}");
    // Pinker = nearer the pink hue 350, on the circle.
    let from_pink = |hue: f64| ((hue - 350.0 + 540.0) % 360.0 - 180.0).abs();
    let (h_o, h_e) = (lch(&colors.o_ray)[2], lch(&colors.e_ray)[2]);
    assert!(
        from_pink(h_o) < from_pink(h_e),
        "the o-ray hue {h_o:.1} must be pinker than the e-ray's {h_e:.1}"
    );
    let t = material(MORGANITE).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
    assert_eq!((t.o_ray.len(), t.e_ray.len()), (6, 6));
}

/// Chrysoberyl (Yellow), biaxial: the Fe3+ bands of Caltech GRR 171 on beta (E par a) and gamma
/// (E par b), with the alpha ray (E par c) estimated as their mean, all at one factor 0.7 (a
/// golden yellow). The gamma ray, with the strong 441 nm band, is the most saturated.
#[test]
fn chrysoberyl_yellow_is_a_trichroic_greenish_yellow() {
    // Mixed from the Greenish Yellow variant and a little Alexandrite Cr3+ to match the dealer
    // photos (median hue 109): a greenish yellow, with the gamma ray green.
    let colors = swatch(CHRYSOBERYL, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((108.0..=130.0).contains(&h), "Chrysoberyl hue {h:.1}");
    assert!((74.0..=88.0).contains(&l), "Chrysoberyl lightness {l:.1}");
    assert!((18.0..=34.0).contains(&c), "Chrysoberyl chroma {c:.1}");
    let beta = colors.beta_ray.expect("Chrysoberyl (Yellow) is biaxial");
    let (c_beta, c_gamma) = (lch(&beta)[1], lch(&colors.e_ray)[1]);
    assert!(
        c_gamma > c_beta,
        "gamma (C* {c_gamma:.1}) must be more saturated than beta (C* {c_beta:.1})"
    );
    let t = material(CHRYSOBERYL).absorption;
    assert!(t.is_pleochroic);
}

/// Topaz: the electron-irradiated blue topaz GRR 825c (the Swiss and sky blue type). The O-
/// hole-centre band peaks near 626 nm and is strongest along ALPHA, about 2.2 x beta at 660 nm.
/// Beta is measured; gamma is taken equal to beta (no spectrum). The brown irradiation centre
/// near 400 nm is left out, since heating of commercial blue topaz removes it.
#[test]
fn topaz_alpha_is_the_deep_blue_ray() {
    let colors = swatch(TOPAZ, 1.0);
    let beta = colors.beta_ray.expect("Topaz is biaxial");
    let [l_alpha, c_alpha, h_alpha] = lch(&colors.o_ray);
    let [l_beta, c_beta, h_beta] = lch(&beta);
    assert!(
        l_alpha + 2.0 < l_beta && c_alpha > c_beta,
        "alpha (L* {l_alpha:.1} C* {c_alpha:.1}) must be deeper than beta (L* {l_beta:.1} C* {c_beta:.1})"
    );
    assert!(c_alpha > 30.0, "alpha chroma {c_alpha:.1}");
    for (ray, h) in [("alpha", h_alpha), ("beta", h_beta)] {
        assert!(
            (220.0..=260.0).contains(&h),
            "{ray} hue {h:.1} must be blue"
        );
    }
    let h = lch(&colors.unpolarised)[2];
    assert!((230.0..=252.0).contains(&h), "Topaz unpolarised hue {h:.1}");
    let (alpha_660, beta_660, gamma_660) = (
        ray_alpha(TOPAZ, Ray::O, 660.0),
        ray_alpha(TOPAZ, Ray::Beta, 660.0),
        ray_alpha(TOPAZ, Ray::E, 660.0),
    );
    assert!(
        (1.5..=3.5).contains(&(alpha_660 / beta_660)) && (beta_660 - gamma_660).abs() < 1e-9,
        "alpha {alpha_660:.2}, beta {beta_660:.2}, gamma {gamma_660:.2} at 660 nm"
    );
}

/// Zircon: the U4+ lines are measured per ray (Caltech GRR 3106). The two rays differ: 653 nm is
/// about twice as strong along o, the 691 nm line is o only, the 616 nm line e only.
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
    // Lines scaled to 0.05 of the green Mogok crystal, as the dealer photos of white zircon are
    // colorless. The split stays, and the stone is nearly colorless.
    assert!(ray_alpha(ZIRCON, Ray::O, 690.9) > 0.25);
    assert!(ray_alpha(ZIRCON, Ray::E, 690.9) < 0.005);
    assert!(ray_alpha(ZIRCON, Ray::E, 616.0) > 0.15);
    assert!(ray_alpha(ZIRCON, Ray::O, 616.0) < 0.005);
    let [l, c, _] = unpolarised(ZIRCON, 1.0);
    assert!(l >= 95.0, "Zircon lightness {l:.1}");
    assert!(c <= 3.0, "Zircon chroma {c:.1} must be nearly colorless");
}

/// Tourmaline: the Fe2+ band at 711 nm is NOT strongly dichroic (o : e about 1.2; the blue and
/// green Caltech crystals all measure o : e about 1). The polarisation lives in the UV / blue
/// edge (o about 3 x e at 400 nm).
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
    // A mix of the elbaite, Yellow and Pink variant bands, matched to the dealer photos of green
    // tourmaline (a dark olive to yellowish green, median hue 121, L* 52).
    let colors = swatch(TOURMALINE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((116.0..=130.0).contains(&h), "Tourmaline hue {h:.1}");
    assert!((45.0..=60.0).contains(&l), "Tourmaline lightness {l:.1}");
    assert!((38.0..=54.0).contains(&c), "Tourmaline chroma {c:.1}");
    let (l_o, l_e) = (lch(&colors.o_ray)[0], lch(&colors.e_ray)[0]);
    assert!(l_o < l_e, "the o-ray (L* {l_o:.1}) is the deeper ray");
    let t = material(TOURMALINE).absorption;
    assert!(t.is_pleochroic && t.beta_ray.is_none());
}

/// Andalusite: ALPHA (E par c, the `o_ray` slot, `n_alpha`) carries the Fe2+-Ti4+ color, a
/// brownish red, while beta and gamma are nearly colorless.
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
    // Mixed from the crystal and the Greener variant to match the dealer photos (median hue
    // 71): beta and gamma are a pale yellow-green.
    let beta = colors.beta_ray.expect("Andalusite is biaxial");
    for (name, ray) in [("beta", lch(&beta)), ("gamma", lch(&colors.e_ray))] {
        assert!(
            ray[1] < 35.0 && ray[0] > l_alpha + 20.0 && (100.0..=135.0).contains(&ray[2]),
            "{name} must be a pale yellow-green: L* {:.1} C* {:.1} h {:.1}",
            ray[0],
            ray[1],
            ray[2]
        );
    }
}

/// A colored stone on an anisotropic host never carries an isotropic tensor: every colored
/// built-in on a uniaxial host has a pleochroic `o_ray` / `e_ray` tensor and no `beta_ray`,
/// every one on a biaxial host a pleochroic tensor with a `beta_ray`.
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
/// no pleochroic flag): Quartz (rock crystal), Synthetic Moissanite and the feldspar Moonstone.
/// Labradorite is the olive-grey exception, see the next test.
#[test]
fn colorless_stones_on_anisotropic_hosts_stay_empty_and_isotropic() {
    for name in ["Quartz", "Synthetic Moissanite", "Moonstone"] {
        let t = material(name).absorption;
        assert!(!t.is_pleochroic, "{name} keeps the isotropic tensor");
        assert!(
            t.o_ray.is_empty() && t.e_ray.is_empty() && t.beta_ray.is_none(),
            "{name} stays colorless"
        );
    }
}

/// Labradorite's olive-grey body is the same in all three
/// directions, in the per-axis form of a colored biaxial stone: no body pleochroism was found.
/// It absorbs the blue end most (the yellow edge that tints the grey toward olive).
#[test]
fn labradorite_has_one_olive_grey_body_in_every_direction() {
    let t = material("Labradorite").absorption;
    let beta = t.beta_ray.as_ref().expect("a biaxial host has a beta ray");
    assert!(t.is_pleochroic && !t.o_ray.is_empty());
    assert_eq!(&t.o_ray, beta, "alpha = beta");
    assert_eq!(t.o_ray, t.e_ray, "alpha = gamma");
    let k = |nm: f32| t.o_ray.iter().map(|b| b.evaluate(nm)).sum::<f32>();
    assert!(
        k(430.0) > k(530.0) && k(430.0) > k(650.0),
        "the blue end absorbs most: {} {} {}",
        k(430.0),
        k(530.0),
        k(650.0)
    );
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

/// Benitoite (Caltech GRR 561-D4): the e-ray (E parallel to c) is blue, the o-ray (E
/// perpendicular to c) is nearly colorless and lighter, and the unpolarised mix is blue.
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
/// 0.22 x exp(-0.5 x (2 257 / 3 665)^2) = 0.182, i.e. 0.046 of the e-ray's 4.0.
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

/// Red spinel (Caltech JQ-13): the 4A2 -> 4T1 band near 400 nm closes the violet leak, so the
/// swatch is red, and the absorption at 400 nm exceeds the valley at 470 nm.
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
