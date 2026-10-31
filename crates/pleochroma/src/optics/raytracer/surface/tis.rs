//! Total integrated scatter of a polished surface.
//!
//! A polish of RMS roughness `Rq` far below the wavelength keeps its sharp specular peak and
//! sends a small fraction of the light into a halo. Scalar scattering theory (Davies 1954;
//! Bennett and Porteus 1961) gives that fraction; the saturating form
//! `1 - exp(-x)` is the published one for arbitrary roughness (Harvey et al.), so no cap is
//! needed. The scatter goes as `λ⁻²`: a poor polish hazes blue.
//!
//! - reflection, in the medium of index `n1`: `x = (4π n1 Rq cosθ / λ)²`;
//! - transmission into `n2`: `x = (2π Rq |n1 cosθ_i − n2 cosθ_t| / λ)²`.
//!
//! `Rq` and `λ` are both in nanometres. `Rq = 0` gives exactly `0`.

use std::f32::consts::PI;

/// `1 - exp(-x)` without the cancellation of the naive form for small `x`.
fn saturate(x: f32) -> f32 {
    -(-x).exp_m1()
}

/// The total integrated scatter of a reflection off the surface, seen from the medium of index
/// `n1` at incidence cosine `cos_i`, at wavelength `lambda_nm`.
#[must_use]
pub fn tis_reflection(rq_nm: f32, n1: f32, cos_i: f32, lambda_nm: f32) -> f32 {
    let phase = 4.0 * PI * n1 * rq_nm * cos_i / lambda_nm;
    saturate(phase * phase)
}

/// The total integrated scatter of a transmission from index `n1` into `n2` at cosine `cos_i`.
///
/// Where the light cannot be transmitted (total internal reflection)
/// the reflection value is returned, so the result is always a valid probability.
#[must_use]
pub fn tis_transmission(rq_nm: f32, n1: f32, n2: f32, cos_i: f32, lambda_nm: f32) -> f32 {
    let eta = n1 / n2;
    let sin2_t = eta * eta * cos_i.mul_add(-cos_i, 1.0);
    if sin2_t >= 1.0 {
        return tis_reflection(rq_nm, n1, cos_i, lambda_nm);
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    let phase = 2.0 * PI * rq_nm * n1.mul_add(cos_i, -(n2 * cos_t)).abs() / lambda_nm;
    saturate(phase * phase)
}
