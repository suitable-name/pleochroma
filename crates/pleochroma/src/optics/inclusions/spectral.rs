//! The spectral dependence of an inclusion family's extinction.
//!
//! A family of small scatterers extinguishes light roughly as `lambda^-p`: grey for coarse
//! particles (`p = 0`), bluish for fine silk (`p = 1` or `2`), Rayleigh-like for the finest
//! (`p = 4`). The factor is `(550 nm / lambda)^p`, computed by multiplication only, so the
//! WGSL twin never needs `pow` (whose GPU lowering is inaccurate; see the note at
//! `scattering::hg::henyey_greenstein_phase`).

/// The wavelength the family extinction `sigma` is quoted at, in nanometres.
pub const REFERENCE_WAVELENGTH_NM: f32 = 550.0;

/// The exponents a family may use: Grey, Slightly Blue, Blue, Rayleigh.
pub const ALLOWED_EXPONENTS: [u8; 4] = [0, 1, 2, 4];

/// `(550 / lambda_nm)^p` for `p` in `{0, 1, 2, 4}`.
///
/// Any other exponent is snapped down to the nearest allowed one (3 becomes 2, anything above
/// 4 becomes 4). A non-positive or non-finite wavelength gives `1.0`.
#[must_use]
pub fn spectral_factor(lambda_nm: f32, p: u8) -> f32 {
    if p == 0 || !lambda_nm.is_finite() || lambda_nm <= 0.0 {
        return 1.0;
    }
    let r = REFERENCE_WAVELENGTH_NM / lambda_nm;
    match p {
        1 => r,
        2 | 3 => r * r,
        _ => {
            let r2 = r * r;
            r2 * r2
        }
    }
}

/// `p` snapped to the nearest allowed exponent at or below it.
#[must_use]
pub const fn snap_exponent(p: u8) -> u8 {
    match p {
        0 => 0,
        1 => 1,
        2 | 3 => 2,
        _ => 4,
    }
}
