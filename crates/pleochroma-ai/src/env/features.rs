//! Compact measured material and orbit features used by the observation.

use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::Design;

pub(super) fn absorption_summary(gem: &GemMaterial, wavelength_nm: f32) -> f32 {
    let sum = |bands: &[pleochroma::optics::absorption::AbsorptionBand]| {
        bands
            .iter()
            .map(|band| band.evaluate(wavelength_nm))
            .sum::<f32>()
    };
    let mut principal_sum = sum(&gem.absorption.o_ray) + sum(&gem.absorption.e_ray);
    let mut principal_count = 2.0;
    if let Some(beta) = &gem.absorption.beta_ray {
        principal_sum += sum(beta);
        principal_count = 3.0;
    }
    principal_sum / principal_count
}

pub(super) fn orbit_harmonics(design: &Design, tier_index: usize) -> (f32, f32, f32, f32) {
    let Some(tier) = design.tiers.get(tier_index) else {
        return (0.0, 1.0, 0.0, 1.0);
    };
    let indices = if tier.indices.is_empty() {
        &[0.0][..]
    } else {
        &tier.indices
    };
    let cheater = design.cheater_offset_deg(tier_index).unwrap_or(0.0);
    let (mut s2, mut c2, mut s4, mut c4) = (0.0, 0.0, 0.0, 0.0);
    for &index in indices {
        let Some(theta) = index_azimuth(
            index,
            design.meta.gear_teeth,
            design.meta.gear_reference_angle,
            cheater,
        ) else {
            return (0.0, 1.0, 0.0, 1.0);
        };
        s2 += (2.0 * theta).sin();
        c2 += (2.0 * theta).cos();
        s4 += (4.0 * theta).sin();
        c4 += (4.0 * theta).cos();
    }
    let n = indices.len() as f64;
    (
        (s2 / n) as f32,
        (c2 / n) as f32,
        (s4 / n) as f32,
        (c4 / n) as f32,
    )
}

/// Reference is in wheel teeth; the additional cheater is in degrees.
pub(super) fn index_azimuth(
    index: f64,
    gear: i32,
    reference: f64,
    cheater_deg: f64,
) -> Option<f64> {
    if gear == 0
        || ![index, reference, cheater_deg]
            .iter()
            .all(|value| value.is_finite())
    {
        return None;
    }
    let theta = std::f64::consts::TAU * (index + reference) / f64::from(gear.unsigned_abs())
        + cheater_deg.to_radians();
    theta.is_finite().then_some(theta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_uses_reference_teeth_signed_gears_and_implicit_index_zero() {
        let mut design = Design::concave_fixture();
        design.meta.gear_teeth = 96;
        design.meta.gear_reference_angle = 6.0;
        design.tiers[0].indices = vec![0.0];
        let explicit = orbit_harmonics(&design, 0);
        // Six teeth is 22.5 degrees; its second harmonic is 45 degrees.
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!((explicit.0 - expected).abs() < 1e-6);
        assert!((explicit.1 - expected).abs() < 1e-6);
        assert!((explicit.2 - 1.0).abs() < 1e-6 && explicit.3.abs() < 1e-6);
        design.meta.gear_teeth = -96;
        design.tiers[0].indices.clear();
        assert_eq!(orbit_harmonics(&design, 0), explicit);
    }
}
