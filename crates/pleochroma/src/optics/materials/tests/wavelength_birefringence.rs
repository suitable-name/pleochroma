//! F4 tests: birefringence that varies with wavelength -- the e-ray curves of Sapphire,
//! Ruby, Synthetic Moissanite and Benitoite, the principal-index curves of Alexandrite
//! and Chrysoberyl (Yellow), and the guarantee that every other material is untouched.

use crate::optics::{dispersion::DispersionModel, materials::GemMaterial};

/// `n = sqrt(1 + sum b_i l^2 / (l^2 - c_i))` in `f64`, `lambda` in nm.
fn sellmeier(b: &[f64], c: &[f64], lambda_nm: f64) -> f64 {
    let l2 = (lambda_nm * 1e-3).powi(2);
    b.iter()
        .zip(c)
        .fold(1.0, |n2, (b, c)| n2 + b * l2 / (l2 - c))
        .sqrt()
}

/// Malitson & Dodge (1972) sapphire e-ray, from the published (pole wavelength) form.
fn malitson_e(lambda_nm: f64) -> f64 {
    sellmeier(
        &[1.503_975_9, 0.550_691_41, 6.592_737_9],
        &[
            0.074_028_8f64.powi(2),
            0.121_652_9f64.powi(2),
            20.072_248f64.powi(2),
        ],
        lambda_nm,
    )
}

/// Wang (2013) 6H-SiC e-ray: `n^2 = 6.7452 + 0.15352/(l^2-0.03597) - 0.02249 l^2`.
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn wang_e(lambda_nm: f64) -> f64 {
    let l2 = (lambda_nm * 1e-3).powi(2);
    (6.7452 + 0.153_52 / (l2 - 0.035_97) - 0.022_49 * l2).sqrt()
}

#[test]
fn sapphire_and_ruby_e_curves_match_malitson_dodge() {
    for name in ["Sapphire", "Ruby"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        let e_curve = material
            .uniaxial_extraordinary_dispersion
            .unwrap_or_else(|| panic!("{name} must carry the Malitson & Dodge e-ray curve"));
        for lambda_nm in [400.0f32, 486.1, 589.3, 656.3, 700.0] {
            let got = f64::from(e_curve.evaluate(lambda_nm));
            let want = malitson_e(f64::from(lambda_nm));
            assert!(
                (got - want).abs() < 1e-5,
                "{name} n_e({lambda_nm}) = {got}, Malitson & Dodge says {want}"
            );
        }
    }
}

/// `n_e(D) - n_o(D)` is -0.008074 at 589.3 nm (the transcribed o-ray fit's rounded
/// coefficients move it by about 1e-6), and the spread runs from -0.00846 at 400 nm to
/// -0.00798 at 700 nm.
#[test]
fn sapphire_birefringence_at_the_d_line_and_across_the_band() {
    let sapphire = GemMaterial::by_name("Sapphire").expect("Sapphire");
    let spread = |lambda_nm: f32| {
        let n_o = sapphire.dispersion.evaluate(lambda_nm);
        sapphire.extraordinary_index_at(lambda_nm, n_o) - n_o
    };
    assert!(
        (spread(589.3) - (-0.008_074)).abs() < 5e-6,
        "n_e(D) - n_o(D) = {}",
        spread(589.3)
    );
    assert!(
        (spread(400.0) - (-0.008_46)).abs() < 2e-5,
        "{}",
        spread(400.0)
    );
    assert!(
        (spread(700.0) - (-0.007_98)).abs() < 2e-5,
        "{}",
        spread(700.0)
    );
}

#[test]
fn moissanite_e_curve_matches_wang_6h() {
    let moissanite = GemMaterial::by_name("Synthetic Moissanite").expect("Moissanite");
    let e_curve = moissanite
        .uniaxial_extraordinary_dispersion
        .expect("Moissanite must carry the Wang 6H e-ray curve");
    for lambda_nm in [400.0f32, 486.1, 589.3, 656.3, 780.0] {
        let got = f64::from(e_curve.evaluate(lambda_nm));
        let want = wang_e(f64::from(lambda_nm));
        assert!(
            (got - want).abs() < 1e-5,
            "n_e({lambda_nm}) = {got}, Wang says {want}"
        );
    }
    let n_o = moissanite.dispersion.evaluate(589.3);
    let n_e = e_curve.evaluate(589.3);
    assert!(
        (n_e - n_o - 0.041_532).abs() < 1e-4,
        "n_e(D) - n_o(D) = {}",
        n_e - n_o
    );
}

/// Benitoite's per-ray Cauchy fits reproduce the gemological table of Laurs et al.
/// (1997): `n_omega` 1.757, `n_epsilon` 1.804, B-G dispersion 0.039 (o) and 0.046 (e).
#[test]
fn benitoite_rays_carry_their_own_dispersion() {
    let benitoite = GemMaterial::by_name("Benitoite").expect("Benitoite");
    let e_curve = benitoite
        .uniaxial_extraordinary_dispersion
        .expect("Benitoite must carry a per-ray e curve");
    let o_curve = benitoite.dispersion;
    assert!((o_curve.evaluate(589.3) - 1.757).abs() < 1e-4);
    assert!((e_curve.evaluate(589.3) - 1.804).abs() < 1e-4);
    let b_minus_g = |curve: DispersionModel| curve.evaluate(430.8) - curve.evaluate(686.7);
    assert!(
        (b_minus_g(o_curve) - 0.039).abs() < 5e-4,
        "{}",
        b_minus_g(o_curve)
    );
    assert!(
        (b_minus_g(e_curve) - 0.046).abs() < 5e-4,
        "{}",
        b_minus_g(e_curve)
    );
    let n_o = o_curve.evaluate(589.3);
    assert!(
        (e_curve.evaluate(589.3) - n_o - benitoite.birefringence_delta).abs() < 1e-4,
        "the stored birefringence_delta stays the D-line n_e - n_o"
    );
}

/// The Walling curves never cross inside the rendered band, so the magnitude labels
/// never swap, and `n_gamma - n_alpha` at D is the comment's 0.007581.
#[test]
fn principal_curves_do_not_cross_between_380_and_780_nm() {
    for name in ["Alexandrite", "Chrysoberyl (Yellow)"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        let [alpha_curve, gamma_curve] = material
            .biaxial_principal_dispersion
            .unwrap_or_else(|| panic!("{name} must carry the principal curves"));
        for step in 0..=80u16 {
            let lambda_nm = f32::from(step).mul_add(5.0, 380.0);
            let indicatrix = material
                .biaxial_indicatrix(lambda_nm)
                .expect("a biaxial indicatrix");
            assert!(
                indicatrix.n_alpha < indicatrix.n_beta && indicatrix.n_beta < indicatrix.n_gamma,
                "{name} at {lambda_nm} nm: ({}, {}, {}) is not strictly ordered",
                indicatrix.n_alpha,
                indicatrix.n_beta,
                indicatrix.n_gamma
            );
            // The indicatrix must be the curves themselves, never a re-sorted mix.
            assert_eq!(
                indicatrix.n_alpha.to_bits(),
                alpha_curve.evaluate(lambda_nm).to_bits(),
                "{name} at {lambda_nm} nm: n_alpha must be the alpha curve"
            );
            assert_eq!(
                indicatrix.n_gamma.to_bits(),
                gamma_curve.evaluate(lambda_nm).to_bits(),
                "{name} at {lambda_nm} nm: n_gamma must be the gamma curve"
            );
        }
    }
}

#[test]
fn alexandrite_spreads_match_walling_and_vary_with_color() {
    let alexandrite = GemMaterial::by_name("Alexandrite").expect("Alexandrite");
    let at = |lambda_nm: f32| {
        alexandrite
            .biaxial_indicatrix(lambda_nm)
            .expect("indicatrix")
    };
    let d = at(589.3);
    assert!(
        (d.n_gamma - d.n_alpha - 0.007_581).abs() < 2e-6,
        "n_gamma - n_alpha at D = {}",
        d.n_gamma - d.n_alpha
    );
    assert!(
        (d.n_beta - d.n_alpha - 0.001_951).abs() < 2e-6,
        "n_beta - n_alpha at D = {}",
        d.n_beta - d.n_alpha
    );
    // The spread falls from 0.00845 at 380 nm to 0.00735 at 780 nm, so 2V changes.
    let (violet, red) = (at(380.0), at(780.0));
    assert!(
        (violet.n_gamma - violet.n_alpha) - (red.n_gamma - red.n_alpha) > 5e-4,
        "the total birefringence must be larger in the violet than in the red"
    );
    assert!(
        (alexandrite.birefringence_delta - 0.007_581).abs() < 1e-6
            && alexandrite.biaxial_delta_beta_alpha == Some(0.001_951),
        "the stored spreads are the D-line values of the curves"
    );
}

/// Chrysoberyl (Yellow) takes the Walling spreads (owner decision 2026-10-22): the
/// total birefringence is 0.0076, not the earlier gem figure 0.0090.
#[test]
fn chrysoberyl_yellow_takes_the_walling_spreads() {
    let chrysoberyl = GemMaterial::by_name("Chrysoberyl (Yellow)").expect("Chrysoberyl");
    let d = chrysoberyl.biaxial_indicatrix(589.3).expect("indicatrix");
    assert!((d.n_beta - 1.746).abs() < 0.002, "n_beta(D) = {}", d.n_beta);
    assert!(
        (d.n_gamma - d.n_alpha - chrysoberyl.birefringence_delta).abs() < 1e-5,
        "n_gamma - n_alpha = {}, birefringence_delta = {}",
        d.n_gamma - d.n_alpha,
        chrysoberyl.birefringence_delta
    );
    let delta_beta_alpha = chrysoberyl.biaxial_delta_beta_alpha.expect("spread");
    assert!((d.n_beta - d.n_alpha - delta_beta_alpha).abs() < 1e-5);
}

/// If curves ever cross, `biaxial_indicatrix` keeps the magnitude order: swapping the
/// two curves of Alexandrite gives the same indicatrix.
#[test]
fn crossed_curves_are_sorted_into_magnitude_order() {
    let alexandrite = GemMaterial::by_name("Alexandrite").expect("Alexandrite");
    let [alpha, gamma] = alexandrite.biaxial_principal_dispersion.expect("curves");
    let mut swapped = alexandrite.clone();
    swapped.biaxial_principal_dispersion = Some([gamma, alpha]);
    for lambda_nm in [380.0f32, 589.3, 780.0] {
        let want = alexandrite
            .biaxial_indicatrix(lambda_nm)
            .expect("indicatrix");
        let got = swapped.biaxial_indicatrix(lambda_nm).expect("indicatrix");
        assert_eq!(got.n_alpha.to_bits(), want.n_alpha.to_bits());
        assert_eq!(got.n_beta.to_bits(), want.n_beta.to_bits());
        assert_eq!(got.n_gamma.to_bits(), want.n_gamma.to_bits());
    }
}

/// Exactly these materials gained curves; every other material keeps the achromatic
/// offsets bit for bit, so its renders and figures cannot move.
#[test]
fn materials_without_curve_data_keep_the_constant_offsets_bit_for_bit() {
    let with_e_curve = [
        "Quartz",
        "Amethyst",
        "Citrine",
        "Ametrine",
        "Rutile",
        "Sapphire",
        "Ruby",
        "Synthetic Moissanite",
        "Benitoite",
    ];
    // Titanite (2026-11-03) carries its measured n_alpha / n_gamma curves (Busz, after Dana).
    let with_principal_curves = ["Alexandrite", "Chrysoberyl (Yellow)", "Titanite (Sphene)"];
    for material in GemMaterial::all_materials() {
        let name = material.name.as_str();
        assert_eq!(
            material.uniaxial_extraordinary_dispersion.is_some(),
            with_e_curve.contains(&name),
            "{name}: unexpected e-curve state"
        );
        assert_eq!(
            material.biaxial_principal_dispersion.is_some(),
            with_principal_curves.contains(&name),
            "{name}: unexpected principal-curve state"
        );
        if material.uniaxial_extraordinary_dispersion.is_none() {
            for lambda_nm in [380.0f32, 486.1, 589.3, 656.3, 780.0] {
                let n_o = material.dispersion.evaluate(lambda_nm);
                assert_eq!(
                    material.extraordinary_index_at(lambda_nm, n_o).to_bits(),
                    (n_o + material.birefringence_delta).to_bits(),
                    "{name} at {lambda_nm} nm"
                );
            }
        }
        let Some(delta_beta_alpha) = material.biaxial_delta_beta_alpha else {
            continue;
        };
        if material.biaxial_principal_dispersion.is_some() {
            continue;
        }
        for lambda_nm in [380.0f32, 589.3, 780.0] {
            let n_beta = material.dispersion.evaluate(lambda_nm);
            let n_alpha = n_beta - delta_beta_alpha;
            let indicatrix = material.biaxial_indicatrix(lambda_nm).expect("indicatrix");
            assert_eq!(indicatrix.n_alpha.to_bits(), n_alpha.to_bits(), "{name}");
            assert_eq!(indicatrix.n_beta.to_bits(), n_beta.to_bits(), "{name}");
            assert_eq!(
                indicatrix.n_gamma.to_bits(),
                (n_alpha + material.birefringence_delta).to_bits(),
                "{name}"
            );
        }
    }
}

/// The validator accepts every new curve (finite, no resonance in 300-800 nm, `n > 1`).
#[test]
fn every_new_curve_validates() {
    for material in GemMaterial::all_materials() {
        if let Some([alpha, gamma]) = material.biaxial_principal_dispersion {
            assert_eq!(alpha.validate(), Ok(()), "{} (alpha)", material.name);
            assert_eq!(gamma.validate(), Ok(()), "{} (gamma)", material.name);
        }
        if let Some(e_curve) = material.uniaxial_extraordinary_dispersion {
            assert_eq!(e_curve.validate(), Ok(()), "{} (e-ray)", material.name);
        }
    }
}
