//! Tests of the Titanite (Sphene) built-in (2026-11-03): its index curves follow Busz's measured
//! values and stay ordered, its fire is above diamond's, its 2V is the measured one, refraction
//! into it converges at its birefringence (the check it waited for), and its three rays have the
//! published pleochroic order.

use crate::{
    color::{
        Illuminant,
        body_color::{BodyColor, body_colors, lab_to_lch},
    },
    optics::{
        dispersion::DispersionModel,
        materials::{GemMaterial, OpticalCharacter},
    },
};
use glam::Vec3;

const NAME: &str = "Titanite (Sphene)";

fn titanite() -> GemMaterial {
    GemMaterial::by_name(NAME).expect("Titanite (Sphene) is a built-in")
}

/// `[n_alpha, n_beta, n_gamma]` straight from the three curves (not sorted).
fn curves_at(material: &GemMaterial, lambda_nm: f32) -> [f32; 3] {
    let [alpha, gamma] = material
        .biaxial_principal_dispersion
        .as_ref()
        .expect("titanite carries its principal curves");
    [
        alpha.evaluate(lambda_nm),
        material.dispersion.evaluate(lambda_nm),
        gamma.evaluate(lambda_nm),
    ]
}

/// The B-G interval (430.8 to 686.7 nm), the gemological dispersion figure.
fn b_minus_g(curve: &DispersionModel) -> f32 {
    curve.evaluate(430.8) - curve.evaluate(686.7)
}

/// Busz 1887 (St. Gotthard, via Dana 1892 p. 714): the curves meet the nine measured indices
/// within 0.0015, and `n_alpha` < `n_beta` < `n_gamma` over the whole rendered range.
#[test]
fn titanite_indices_follow_busz_and_never_cross() {
    let material = titanite();
    assert_eq!(
        material.optical_character,
        OpticalCharacter::BiaxialPositive
    );
    for (lambda, measured) in [
        (670.8, [1.8766, 1.8839, 1.9987]),
        (589.3, [1.8879, 1.8940, 2.0093]),
        (535.0, [1.8989, 1.9041, 2.0232]),
    ] {
        let curves = curves_at(&material, lambda);
        for (ray, (n, m)) in ["alpha", "beta", "gamma"]
            .iter()
            .zip(curves.iter().zip(measured))
        {
            assert!(
                (n - m).abs() < 0.0015,
                "n_{ray} {n} against Busz {m} at {lambda} nm"
            );
        }
    }
    for step in 0..=40_u16 {
        let lambda = 10.0_f32.mul_add(f32::from(step), 380.0);
        let [a, b, g] = curves_at(&material, lambda);
        assert!(a < b && b < g, "{a} < {b} < {g} must hold at {lambda} nm");
    }
    let [a, b, g] = curves_at(&material, 589.3);
    assert!((g - a - material.birefringence_delta).abs() < 1e-5);
    let beta_alpha = material.biaxial_delta_beta_alpha.expect("biaxial");
    assert!((b - a - beta_alpha).abs() < 1e-5);
}

/// Sphene's fire is above diamond's (B-G 0.051 in the gem tables against 0.044); the measured
/// curves give 0.057 for `n_beta`.
#[test]
fn titanite_disperses_more_than_diamond() {
    let titanite = b_minus_g(&titanite().dispersion);
    let diamond = b_minus_g(&GemMaterial::diamond().dispersion);
    assert!(
        titanite > 1.2 * diamond,
        "titanite B-G {titanite:.4} must clearly exceed diamond's {diamond:.4}"
    );
    assert!(
        (0.045..=0.065).contains(&titanite),
        "titanite B-G {titanite:.4}"
    );
}

/// `2V_z` at the D line is the measured 27 deg (Busz; the Handbook gives 17-40 deg).
#[test]
fn titanite_two_v_is_the_measured_one() {
    let [a, b, g] = curves_at(&titanite(), 589.3).map(f64::from);
    let cos2 = a * a * b.mul_add(-b, g * g) / (b * b * a.mul_add(-a, g * g));
    let two_v = 2.0 * cos2.sqrt().acos().to_degrees();
    assert!((24.0..=31.0).contains(&two_v), "2V_z {two_v:.1} deg");
}

/// Refraction into the stone: the two-step fixed point of `resolve_entry_mode` against the same
/// iteration run to convergence, for both modes over a fan of incidences, at the F line where the
/// indices spread most. At titanite's birefringence each step shrinks the error by about
/// `delta_n tan(theta) / n`, so two steps leave far less than the dispersion that shows as fire.
#[test]
fn refraction_into_titanite_converges_at_its_birefringence() {
    let indicatrix = titanite()
        .biaxial_indicatrix(486.1)
        .expect("titanite is biaxial");
    let normal = Vec3::Y;
    let converged = |incident: Vec3, cos_i: f32, slow: bool| {
        let mut n = indicatrix.n_beta;
        for _ in 0..60 {
            let eta = 1.0 / n;
            let sin2 = eta * eta * cos_i.mul_add(-cos_i, 1.0);
            let cos_t = (1.0 - sin2).max(0.0).sqrt();
            let wave = (eta * incident + eta.mul_add(cos_i, -cos_t) * normal).normalize();
            let (n_slow, n_fast) = indicatrix.wave_indices(wave);
            n = if slow { n_slow } else { n_fast };
        }
        n
    };
    let mut widest = 0.0_f32;
    for tilt_deg in [0.0_f32, 20.0, 40.0, 60.0, 80.0] {
        for azimuth_deg in [0.0_f32, 45.0, 90.0, 135.0] {
            let (st, ct) = tilt_deg.to_radians().sin_cos();
            let (sa, ca) = azimuth_deg.to_radians().sin_cos();
            let incident = Vec3::new(st * ca, -ct, st * sa).normalize();
            let cos_i = -incident.dot(normal);
            let mut pair = [0.0_f32; 2];
            for (slot, slow) in [true, false].into_iter().enumerate() {
                let (n, _) =
                    indicatrix.resolve_entry_mode(incident, normal, cos_i, indicatrix.n_beta, slow);
                let reference = converged(incident, cos_i, slow);
                assert!(
                    (n - reference).abs() < 5e-4,
                    "tilt {tilt_deg} azimuth {azimuth_deg} slow {slow}: {n} against {reference}"
                );
                pair[slot] = n;
            }
            widest = widest.max(pair[0] - pair[1]);
        }
    }
    assert!(
        widest > 0.05,
        "the two modes split by up to {widest}: the birefringence shows"
    );
}

/// Pleochroism (Dana after Rosenbusch; Handbook of Mineralogy): X nearly colorless, Y yellow to
/// green, Z red-yellow to orange. Face-up the stone is a yellow to yellow-green.
#[test]
fn titanite_rays_have_the_published_pleochroic_order() {
    let colors = body_colors(&titanite().absorption, 1.0, Illuminant::D65);
    let lch = |color: &BodyColor| lab_to_lch(color.lab);
    let [l_x, _, _] = lch(&colors.o_ray);
    let [_, _, h_y] = lch(&colors.beta_ray.expect("titanite is biaxial"));
    let [l_z, _, h_z] = lch(&colors.e_ray);
    assert!(
        l_x > l_z + 10.0,
        "X (L* {l_x:.1}) must be far lighter than Z (L* {l_z:.1})"
    );
    assert!(
        h_z < h_y,
        "Z (h {h_z:.1}) must be more orange than Y (h {h_y:.1})"
    );
    let [_, c, h] = lch(&colors.unpolarised);
    assert!((70.0..=120.0).contains(&h), "face-up hue {h:.1}");
    assert!(c >= 20.0, "face-up chroma {c:.1}");
}

/// A design title or a query naming either name finds the entry.
#[test]
fn sphene_and_titanite_both_find_the_entry() {
    // A whole second name, for an exact lookup such as the command line's `--material`.
    assert_eq!(GemMaterial::alias_target(" SPHENE "), Some(NAME));
    assert_eq!(GemMaterial::alias_target("titanite"), Some(NAME));
    assert_eq!(GemMaterial::alias_target("Sphene Oval"), None);
    for query in ["Sphene", "titanite", "Sphene Oval", "Fine Titanite Cushion"] {
        assert_eq!(
            GemMaterial::by_name(query).map(|m| m.name),
            Some(NAME.to_string()),
            "{query}"
        );
    }
}
