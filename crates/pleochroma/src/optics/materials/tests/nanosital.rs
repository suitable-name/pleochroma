//! Tests of the Nanosital built-in.
//!
//! Its index is GIA's measured one, its dispersion the manufacturer's B-G figure, it is
//! isotropic, its default color is a cobalt blue, its physics host and color variants resolve, and
//! its second names find it.

use crate::{
    color::{
        Illuminant,
        body_color::{body_colors, lab_to_lch},
    },
    optics::{
        chromophore::ChromophoreCatalogue,
        materials::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter},
    },
};

const NAME: &str = "Nanosital";

fn nanosital() -> GemMaterial {
    GemMaterial::by_name(NAME).expect("Nanosital is a built-in")
}

/// Gems & Gemology 62(2), 2026: RI 1.60-1.63 for every sample; the entry takes 1.620.
#[test]
fn nanosital_index_is_the_measured_one() {
    let material = nanosital();
    let n_d = material.dispersion.evaluate(589.3);
    assert!((n_d - 1.620).abs() < 1e-4, "n_d {n_d:.5}");
    for lambda in [486.1, 589.3, 656.3] {
        let n = material.dispersion.evaluate(lambda);
        assert!(
            (1.600..=1.630).contains(&n),
            "n({lambda} nm) = {n:.4} must stay in GIA's measured 1.60-1.63"
        );
    }
}

/// The manufacturer's B-G 0.015 through the house ratio 0.579: F-C 0.008685, Abbe 71.4.
#[test]
fn nanosital_dispersion_is_the_manufacturers_figure() {
    let curve = nanosital().dispersion;
    let b_g = curve.delta_b_g();
    assert!((0.0145..=0.0152).contains(&b_g), "B-G {b_g:.5}");
    let f_c = curve.delta_f_c();
    assert!((f_c - 0.008_685).abs() < 2e-5, "F-C {f_c:.6}");
    let abbe = (curve.evaluate(589.3) - 1.0) / f_c;
    assert!((70.9..=71.9).contains(&abbe), "V_d {abbe:.2}");
}

/// Glass with cubic spinel crystals: isotropic, no birefringence, no extra index curves, and an
/// isotropic absorption tensor.
#[test]
fn nanosital_is_isotropic() {
    let material = nanosital();
    assert_eq!(material.crystal_system, CrystalSystem::Cubic);
    assert_eq!(material.optical_character, OpticalCharacter::Isotropic);
    assert!(material.birefringence_delta.abs() < f32::EPSILON);
    assert!(material.biaxial_delta_beta_alpha.is_none());
    assert!(material.uniaxial_extraordinary_dispersion.is_none());
    assert!(material.biaxial_principal_dispersion.is_none());
    assert!(material.biaxial_indicatrix(589.3).is_none());
    assert!(!material.absorption.is_pleochroic);
    assert!(material.absorption.beta_ray.is_none());
    assert_eq!(material.absorption.o_ray, material.absorption.e_ray);
}

/// The default is the cobalt blue of a sapphire imitation: three Co2+ bands at the GIA positions,
/// a royal blue swatch near the Sapphire default.
#[test]
fn nanosital_default_is_cobalt_blue() {
    let material = nanosital();
    assert_eq!(material.absorption_unit, AbsorptionUnit::ModelUnit);
    let centres: Vec<f32> = material
        .absorption
        .o_ray
        .iter()
        .map(|band| band.center_nm)
        .collect();
    assert_eq!(centres, vec![545.0, 585.0, 625.0]);
    let colors = body_colors(&material.absorption, 1.0, Illuminant::D65);
    let [l, c, h] = lab_to_lch(colors.unpolarised.lab);
    assert!((268.0..=292.0).contains(&h), "Nanosital hue {h:.1}");
    assert!((42.0..=60.0).contains(&l), "Nanosital lightness {l:.1}");
    assert!(c >= 60.0, "Nanosital chroma {c:.1}");
}

/// The physics editor finds the glass-ceramic host, which offers the GIA chromophores.
#[test]
fn nanosital_has_its_own_chromophore_host() {
    let catalogue = ChromophoreCatalogue::global();
    let host = catalogue
        .host_for_material(NAME)
        .expect("Nanosital names a host");
    assert_eq!(host.id, "nanosital");
    assert_eq!(host.optical, "isotropic");
    let offered = catalogue.selectable_elements(&host.id);
    for element in ["Co", "Ni", "Mn", "Au", "Nd", "Er", "Pr"] {
        assert!(
            offered.iter().any(|id| id == element),
            "{element} is offered: {offered:?}"
        );
    }
}

/// The Material Default comes first, then the seven variants of the data file, and each one
/// is valid for the host.
#[test]
fn nanosital_color_variants_resolve() {
    use crate::optics::materials::variants::built_in_variants;
    let host = nanosital();
    let variants = built_in_variants(NAME);
    let keys: Vec<&str> = variants.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "nanosital.default",
            "nanosital.ni_green",
            "nanosital.mn_yellow",
            "nanosital.mn_orange",
            "nanosital.au_red",
            "nanosital.nd_er_pink",
            "nanosital.pr_yellow_green",
            "nanosital.colorless",
        ]
    );
    for variant in &variants {
        assert_eq!(variant.validate_for(&host), Ok(()), "{}", variant.key);
        assert!(!variant.absorption.is_pleochroic, "{}", variant.key);
    }
}

/// A design title or a query naming the trade names finds the entry.
#[test]
fn nano_gem_and_glass_ceramic_find_the_entry() {
    assert_eq!(GemMaterial::alias_target(" Nano Gem "), Some(NAME));
    assert_eq!(GemMaterial::alias_target("glass-ceramic"), Some(NAME));
    assert_eq!(GemMaterial::alias_target("Glass Ceramic"), Some(NAME));
    assert_eq!(GemMaterial::alias_target("Nano Gem Oval"), None);
    for query in [
        "Nanosital",
        "nanosital",
        "Blue Nanosital Oval",
        "Nano Gem Round",
    ] {
        assert_eq!(
            GemMaterial::by_name(query).map(|m| m.name),
            Some(NAME.to_string()),
            "{query}"
        );
    }
    // A full material name still wins over a second name.
    assert_eq!(
        GemMaterial::by_name("Nano Gem Sapphire").map(|m| m.name),
        Some("Sapphire".to_string())
    );
}
