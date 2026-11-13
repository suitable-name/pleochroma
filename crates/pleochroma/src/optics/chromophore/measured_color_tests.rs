//! Reference tables in `data/reference_spectra/*.csv` against the primary data (GIA 2020
//! cross-sections, Caltech spectra) and the catalogue recipes.
//!
//! The tables' stated "measured" CIELAB values are not valid targets. No sample has both a
//! measured spectrum and a measured color (`docs/chromophores/research-2026-10-primary-data.md`
//! section 0), and the stated colors of several rows disagree with their own spectra by
//! dE00 10-38 (`reference_validation`). The recipes are fitted to the primary data
//! (`primary_data_tests`). These tests record where the tables and stated colors differ.
//!
//! Recipe compositions are converted to catalogue units through the host number densities
//! (ppmw -> ppm of cation sites, wt% oxide as is).

#![expect(
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    reason = "test code quoting the research's symbols"
)]

use super::{
    primary_data_tests::{PPMA_CM3, data_alpha, hue_deg, ray_lab, sigma_lab, table, tensor},
    reference_validation::{FIXTURES, LabRef, Spectrum, parse},
    *,
};
use crate::{
    color::body_color::{Illuminant, body_color, body_colors, delta_e_2000},
    optics::absorption::AbsorptionTensor,
};

const N_AVOGADRO: f64 = 6.022_140_76e23;

fn cat() -> &'static ChromophoreCatalogue {
    ChromophoreCatalogue::global()
}

fn fixture(name: &str) -> Spectrum {
    let (n, t) = FIXTURES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("fixture {name}"));
    parse(n, t)
}

/// ppmw of an element of molar mass `m` in host `host` -> amount in `unit` (per-site basis).
fn ppmw_to(host: &str, ppmw: f64, m: f64, unit: &str) -> f64 {
    let h = cat().host(host).expect("host");
    let n = ppmw * 1e-6 * h.density_g_cm3 / m * N_AVOGADRO;
    n / h.n_site_for_unit(unit)
}

fn recipe(host: &str, entries: &[(&str, f64)], treatments: &[&str]) -> ColorRecipe {
    let mut r = ColorRecipe::new(host, cat().data_version);
    for (id, a) in entries {
        assert!(r.set_amount(id, *a));
    }
    r.treatments = treatments.iter().map(|t| (*t).to_string()).collect();
    r
}

/// color of one mode of `t` (`"o"`, `"e"`, `"b"`), or the mean of the named modes' transmittances.
fn lab_of_modes(t: &AbsorptionTensor, modes: &str, path_cm: f64, ill: Illuminant) -> [f64; 3] {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Band::evaluate takes f32 nanometres; l is a wavelength in 380..=780"
    )]
    let alpha = |m: char, l: f64| -> f64 {
        let bands = match m {
            'o' => &t.o_ray,
            'e' => &t.e_ray,
            _ => t.beta_ray.as_ref().expect("beta ray"),
        };
        f64::from(bands.iter().map(|b| b.evaluate(l as f32)).sum::<f32>())
    };
    let path_mm = path_cm * 10.0;
    let weights: Vec<(char, f64)> = match modes {
        "uniax" => vec![('o', 2.0 / 3.0), ('e', 1.0 / 3.0)],
        other => other
            .chars()
            .map(|c| (c, 1.0 / other.chars().count() as f64))
            .collect(),
    };
    let eq = |l: f64| {
        let tr: f64 = weights
            .iter()
            .map(|(m, w)| w * (-alpha(*m, l).max(0.0) * path_mm).exp())
            .sum();
        -tr.max(1e-300).ln() / path_mm
    };
    body_color(eq, path_mm, ill).lab
}

fn stated(s: &Spectrum, spec: &str, ill: &str, path: f64) -> LabRef {
    s.labs
        .iter()
        .find(|l| l.spec == spec && l.illuminant_name == ill && (l.path_cm - path).abs() < 1e-9)
        .unwrap_or_else(|| panic!("{} {spec} {ill} {path}", s.name))
        .clone()
}

// ---------------------------------------------------------------------------------------------
// Ruby
// ---------------------------------------------------------------------------------------------

/// The ruby table R-1 (0.05 wt% Cr2O3, E_perp_c) matches the GIA sample-1110 cross-section
/// within 25 % at 420-575 nm (alpha(555 nm) 2.56 cm^-1 in both). Its stated CIELAB (b* +5.4)
/// does not: the GIA colors of the same absorber are purplish red (b* -20).
#[test]
fn research_ruby_r1_table_is_the_gia_cross_section_but_its_stated_color_is_not() {
    let r1 = fixture("ruby_r1_synthetic_verneuil_0p05wt");
    let g = table("gia_cr3plus_sample1110_sigma");
    let c = g.col("sigma_E_perp_c_cm2");
    let n_wt = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("wt_pct_oxide:Cr2O3");
    for nm in [420.0, 450.0, 475.0, 500.0, 525.0, 555.0, 575.0] {
        let ratio = r1.alpha("E_perp_c", nm, true) / (g.at(c, nm) * n_wt * 0.05);
        assert!(
            (ratio - 1.0).abs() < 0.25,
            "{nm} nm: table / GIA = {ratio:.2}"
        );
    }
    let stated_b = stated(&r1, "mean_uniax", "D65", 0.5).lab[2];
    assert!(stated_b > 0.0);
    // 0.05 wt% over 0.5 cm in GIA's units: 67 ppma*cm; the GIA color is purplish red.
    let d = 0.05 * n_wt * 0.5 / PPMA_CM3;
    let cols = (c, g.col("sigma_E_par_c_cm2"));
    let gia = sigma_lab(&g, cols, "both", d, Illuminant::D65);
    assert!(gia[2] < -5.0, "GIA color {gia:.1?}");
}

/// The ruby table R-2 (0.25 wt% Cr2O3, 0.5 cm = 335 ppma*cm) has a stated CIELAB of
/// 37.8 / 69.5 / 24.8, an orange-red far from GIA's color. The recipe gives GIA's purplish
/// red: within dE00 2.5 of the GIA cross-section color, and over 15 dE00 from the stated one.
#[test]
fn ruby_r2_composition_gives_the_gia_purplish_red_not_the_stated_orange_red() {
    let r2 = fixture("ruby_r2_synthetic_flux_0p25wt");
    let stated = stated(&r2, "mean_uniax", "D65", 0.5);
    let mut r = ColorRecipe::new("corundum", cat().data_version);
    assert!(r.set_amount("Cr", 0.25));
    let t = tensor(&r);
    let g = table("gia_cr3plus_sample1110_sigma");
    let cols = (g.col("sigma_E_perp_c_cm2"), g.col("sigma_E_par_c_cm2"));
    let n_wt = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("wt_pct_oxide:Cr2O3");
    let d = 0.25 * n_wt * 0.5 / PPMA_CM3;
    let unpol = body_colors(&t, 5.0, Illuminant::D65).unpolarised.lab;
    let gia = sigma_lab(&g, cols, "both", d, Illuminant::D65);
    eprintln!(
        "R-2: recipe {unpol:.1?}, GIA sigma {gia:.1?}, stated {:.1?}",
        stated.lab
    );
    assert!(unpol[2] < 0.0 && gia[2] < 0.0 && stated.lab[2] > 0.0);
    assert!(
        delta_e_2000(unpol, gia) <= 2.5,
        "{:.2}",
        delta_e_2000(unpol, gia)
    );
    assert!(delta_e_2000(unpol, stated.lab) > 15.0);
    // Hue near 360 deg (purplish red) for the recipe; near 0 deg (orange-red) for the stated one.
    assert!(hue_deg(unpol) > 330.0 && hue_deg(stated.lab) < 30.0);
}

// ---------------------------------------------------------------------------------------------
// Sapphire and V-corundum
// ---------------------------------------------------------------------------------------------

/// The sapphire tables S-Met and S-Bas imply a Fe2+-Ti4+ cross-section of 0.4-0.7 of the GIA
/// value (1.98e-18 cm^2 +- 25 %), from the net 578 nm peak over the 500 nm continuum per
/// Ti_active (Ti - Mg, in ppm of Al sites). The pair law is linear in Ti_active, so the two
/// tables should agree within 25 %. The catalogue uses the GIA value. GIA reports that non-Yogo
/// stones deviate by factors 2-3, so the gap is not a contradiction.
#[test]
fn sapphire_tables_imply_a_smaller_pair_cross_section_than_gia() {
    let host = cat().host("corundum").expect("corundum");
    let n_site = host.n_site_for_unit("ppm_site");
    let net = |s: &Spectrum| s.alpha("E_perp_c", 578.0, true) - s.alpha("E_perp_c", 500.0, true);
    let sigma_eff = |name: &str, ti_ppmw: f64, mg_ppmw: f64| {
        let s = fixture(name);
        let ti = ppmw_to("corundum", ti_ppmw, 47.867, "ppm_site");
        let mg = ppmw_to("corundum", mg_ppmw, 24.305, "ppm_site");
        net(&s) / ((ti - mg) * n_site)
    };
    let met = sigma_eff("sapphire_s_met_metamorphic", 160.0, 12.0);
    let bas = sigma_eff("sapphire_s_bas_basaltic", 180.0, 22.0);
    let g = table("gia_v3plus_fe2plus_ti4plus_sigma_5nm");
    let gia = g.at(g.col("sigma_FeTi_E_perp_c_cm2"), 580.0);
    eprintln!("sigma_eff S-Met {met:.2e}, S-Bas {bas:.2e}, GIA {gia:.2e}");
    for (name, sigma) in [("S-Met", met), ("S-Bas", bas)] {
        assert!(
            (0.4..=0.7).contains(&(sigma / gia)),
            "{name}: {:.2}",
            sigma / gia
        );
    }
    assert!(
        (bas / met - 1.0).abs() < 0.25,
        "equal Ti_active, similar pair peak"
    );
    // The catalogue carries the GIA value (see primary_data_tests).
    let ours = data_alpha("corundum", "Fe2+-Ti4+", "o", 580.0, 1.0) / n_site;
    assert!((ours / gia - 1.0).abs() < 0.03);
}

/// The V-corundum table follows the paper text sigma (1.0e-19 cm^2 at 580 nm) at its label
/// composition, 0.26 wt% V2O3 = 706 ppma. At 575 nm it is more than twice the xlsx value the
/// catalogue uses, while its 400 nm peak agrees with the xlsx.
#[test]
fn v_corundum_report_table_follows_the_paper_text_not_the_xlsx() {
    let v = fixture("corundum_v_0p26wt_v2o3");
    let n_706 = 706.0
        * cat()
            .host("corundum")
            .expect("corundum")
            .n_site_for_unit("ppma_all");
    let sigma = |nm: f64| v.alpha("E_perp_c", nm, true) / n_706;
    let g = table("gia_v3plus_fe2plus_ti4plus_sigma_5nm");
    let xlsx = |nm: f64| g.at(g.col("sigma_V3_E_perp_c_cm2"), nm);
    assert!(
        (sigma(575.0) / 1.0e-19 - 1.0).abs() < 0.15,
        "{:.2e}",
        sigma(575.0)
    );
    assert!(sigma(575.0) > 2.0 * xlsx(575.0));
    assert!(
        (sigma(400.0) / xlsx(400.0) - 1.0).abs() < 0.1,
        "{:.2}",
        sigma(400.0) / xlsx(400.0)
    );
}

// ---------------------------------------------------------------------------------------------
// Alexandrite
// ---------------------------------------------------------------------------------------------

/// The alexandrite table's rays disagree with Caltech GRR 874 in the U/Y ratio of the 4T2 and
/// 4T1 peaks: beta 0.65 and gamma 0.81 in the table against 0.18 and 1.6 in the primary
/// spectrum (net of the pedestal). Its stated colors do not match its spectrum (dE00 20-38).
/// The recipe is checked to follow Caltech.
#[test]
fn alexandrite_table_rays_disagree_with_the_caltech_u_over_y_ratio() {
    let tab = fixture("alexandrite_natural_russian");
    #[expect(
        clippy::cast_possible_truncation,
        reason = "lo and hi are whole-nanometre bounds in 380..=780; truncation to i32 is exact"
    )]
    let peak = |s: &Spectrum, col: &str, lo: f64, hi: f64| {
        (lo as i32..=hi as i32)
            .map(|nm| s.alpha(col, f64::from(nm), true))
            .fold(0.0, f64::max)
    };
    let ratio = |col: &str| peak(&tab, col, 560.0, 600.0) / peak(&tab, col, 400.0, 430.0);
    let (tb, tg) = (ratio("beta_par_b"), ratio("gamma_par_a"));
    assert!(
        (tb - 0.65).abs() < 0.02 && (tg - 0.81).abs() < 0.02,
        "{tb:.2} {tg:.2}"
    );
    let mut r = ColorRecipe::new("chrysoberyl", cat().data_version);
    assert!(r.set_amount("Cr", 1.0));
    let t = tensor(&r);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Band::evaluate takes f32 nanometres; nm is a wavelength in 380..=780"
    )]
    let at = |ray: char, nm: f64| match ray {
        'b' => t.beta_ray.as_ref().map_or(0.0, |b| {
            f64::from(b.iter().map(|x| x.evaluate(nm as f32)).sum::<f32>())
        }),
        _ => f64::from(t.e_ray.iter().map(|x| x.evaluate(nm as f32)).sum::<f32>()),
    };
    let peak_of =
        |ray: char, lo: i32, hi: i32| (lo..=hi).map(|n| at(ray, f64::from(n))).fold(0.0, f64::max);
    let (rb, rg) = (
        peak_of('b', 560, 600) / peak_of('b', 400, 450),
        peak_of('e', 560, 620) / peak_of('e', 400, 450),
    );
    eprintln!("U/Y beta: table {tb:.2}, recipe {rb:.2}; gamma: table {tg:.2}, recipe {rg:.2}");
    assert!(
        rb < 0.3 && rg > 1.2,
        "recipe follows Caltech: {rb:.2} {rg:.2}"
    );
}

// ---------------------------------------------------------------------------------------------
// Amethyst
// ---------------------------------------------------------------------------------------------

/// Fe 80 ppmw, with gamma irradiation turning 10 % of it into [FeO4]0. The stated CIELAB
/// agrees with the table's own spectrum (dE00 3.4), and the recipe stays within dE00 5 of it.
#[test]
fn recipe_vs_measured_amethyst_e_perp_c() {
    let s = fixture("amethyst_80ppmw_fe");
    let fe = ppmw_to("quartz", 80.0, 55.845, "ppma_all");
    let r = recipe("quartz", &[("Fe", fe)], &["gamma_irradiation"]);
    let m = stated(&s, "E_perp_c", "D65", 0.6);
    let ours = lab_of_modes(&tensor(&r), "o", 0.6, m.illuminant);
    let de = delta_e_2000(m.lab, ours);
    eprintln!(
        "amethyst: stated {:.1?}, recipe {ours:.1?}, dE00 {de:.2}",
        m.lab
    );
    assert!(de <= 5.0, "amethyst E-perp-c dE00 {de:.2}");
    assert!(
        ray_lab(&tensor(&r), 'o', 6.0, Illuminant::D65)[1] > 0.0,
        "violet"
    );
}
