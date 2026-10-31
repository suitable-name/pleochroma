//! The platelet phase function: a microflake mirror.
//!
//! A family of thin platelets of mean normal `m` reflects light like a field of tiny mirrors
//! (Jakob et al. 2010, Heitz et al. 2015). A flake normal `h` is drawn from an isotropic GGX
//! distribution of width `alpha` about `m`, and the propagation direction is mirrored in it,
//! `w_o = w_i - 2 (w_i . h) h`. The density of the result follows from the change of variables
//! `dw_o = 4 |w_i . h| dh`:
//!
//! ```text
//! p(w_o) = D(h) |h . m| / (4 |w_i . h|),     h = normalise(w_o - w_i)
//! ```
//!
//! It integrates to one over the sphere by construction and is exactly what [`platelet_sample`]
//! draws, so sampling and evaluation agree. The projected-area weighting of a full microflake
//! model (which would favour flakes facing `w_i`) is approximated by the extinction's own
//! `|w_i . m|` factor, which is exact as `alpha` goes to zero and a documented approximation
//! for `alpha <= 0.3`; light that passes through a platelet is not an event, so the platelet's
//! reflectance is folded into the family's `sigma`.

use super::perpendicular_basis;
use glam::Vec3;
use std::f32::consts::PI;

/// The GGX normal distribution `D` at flake-normal cosine `cos_m` (to the mean normal), with
/// `integral D cos dh = 1` over the hemisphere.
#[must_use]
pub fn ggx_d(cos_m: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = (cos_m * cos_m).mul_add(a2 - 1.0, 1.0);
    a2 / (PI * d * d)
}

/// Draws a scattered direction for the incoming propagation direction `w_i` off platelets of
/// mean normal `m` (both unit), from two uniform `[0, 1)` randoms: `u1` picks the flake tilt,
/// `u2` its azimuth.
#[must_use]
pub fn platelet_sample(w_i: Vec3, m: Vec3, alpha: f32, u1: f32, u2: f32) -> Vec3 {
    let a2 = alpha * alpha;
    let u1 = u1.clamp(0.0, 1.0 - 1e-6);
    let tan2 = a2 * u1 / (1.0 - u1);
    let cos_m = 1.0 / (1.0 + tan2).sqrt();
    // Separate multiply and subtract, not `mul_add`: the WGSL twin
    // (`transport_physics/11_oriented_scattering.wgsl`) computes it the same way, and a fused
    // operation would round differently. `allow`, not `expect`: older clippy does not raise it.
    #[allow(clippy::suboptimal_flops)]
    let sin_m = (1.0 - cos_m * cos_m).max(0.0).sqrt();
    let (sin_p, cos_p) = (2.0 * PI * u2).sin_cos();
    let (e1, e2) = perpendicular_basis(m);
    let h = e1 * (sin_m * cos_p) + e2 * (sin_m * sin_p) + m * cos_m;
    (w_i - h * (2.0 * w_i.dot(h))).normalize_or_zero()
}

/// The density, per steradian, with which [`platelet_sample`] returns `w_o`.
///
/// `0.0` for `w_o == w_i` (no flake turns the ray by nothing).
#[must_use]
pub fn platelet_pdf(w_i: Vec3, m: Vec3, alpha: f32, w_o: Vec3) -> f32 {
    let half = w_o - w_i;
    let len = half.length();
    if len < 1e-6 {
        return 0.0;
    }
    let h = half / len;
    let cos_m = h.dot(m).abs();
    let facing = w_i.dot(h).abs().max(1e-5);
    ggx_d(cos_m, alpha) * cos_m / (4.0 * facing)
}
