//! The needle phase function: scattering on the cone about a needle's axis.
//!
//! An infinitely long cylinder of axis `t` scatters only onto the cone that keeps the component
//! along its axis, `w_o . t = w_i . t`. That cone is the physical origin of asterism and
//! chatoyancy. A real needle is finite, so the cone is blurred: `s = w_o . t` is drawn from a
//! logistic kernel of scale `beta` centred on `xi = w_i . t` and truncated to `[-1, 1]` (the
//! closed-form CDF means no `erf`, and the normalisation is exact on the CPU and the GPU), and
//! the azimuth `phi` about `t` (`0` is forward, the side `w_i` itself lies on) from a wrapped
//! Cauchy distribution of forwardness `rho`.
//!
//! Because `dw = ds dphi`, the density of the sampled direction is
//! `g(s; xi) * (1 - rho^2) / (2 pi (1 + rho^2 - 2 rho cos phi))`, which [`needle_pdf`]
//! evaluates exactly, so next-event estimation and the MIS ratios can use it.
//!
//! `beta == 0.0` is the ideal cone: every sample lies exactly on it. Its density is a delta
//! and cannot be evaluated, so [`needle_pdf`] returns `0.0` there (a light-sampled direction
//! never lands on an exact cone).

use super::perpendicular_basis;
use glam::Vec3;
use std::f32::consts::PI;

/// The largest azimuthal forwardness `rho` used, which keeps the wrapped Cauchy lobe away from
/// a delta.
pub const MAX_RHO: f32 = 0.98;

/// The logistic CDF `1 / (1 + exp(-x))`, with `x` limited so `exp` cannot overflow.
fn logistic_cdf(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-40.0, 40.0)).exp())
}

/// The mass of the logistic kernel of scale `beta` centred on `xi` inside `[-1, 1]`, and the
/// kernel CDF at the lower end.
fn truncation(xi: f32, beta: f32) -> (f32, f32) {
    let lo = logistic_cdf((-1.0 - xi) / beta);
    let hi = logistic_cdf((1.0 - xi) / beta);
    ((hi - lo).max(1e-12), lo)
}

/// The density in `s` of the truncated logistic kernel of scale `beta` centred on `xi`.
#[must_use]
pub fn needle_s_density(xi: f32, s: f32, beta: f32) -> f32 {
    if beta <= 0.0 {
        return 0.0;
    }
    let (mass, _) = truncation(xi, beta);
    let e = (-((s - xi) / beta).abs().min(40.0)).exp();
    e / (beta * (1.0 + e) * (1.0 + e) * mass)
}

/// The wrapped Cauchy density in the azimuth `phi` (radians, `0` forward), given `cos(phi)`.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn wrapped_cauchy_density(cos_phi: f32, rho: f32) -> f32 {
    let rho = rho.clamp(0.0, MAX_RHO);
    (1.0 - rho * rho) / (2.0 * PI * (1.0 + rho * rho - 2.0 * rho * cos_phi))
}

/// The unit vector `u` the azimuth is measured from: the component of `w_i` perpendicular to
/// `t`, or an arbitrary perpendicular when `w_i` is (nearly) along `t`.
fn azimuth_origin(w_i: Vec3, t: Vec3) -> (Vec3, bool) {
    let perp = w_i - t * w_i.dot(t);
    let len = perp.length();
    if len < 1e-6 {
        (perpendicular_basis(t).0, false)
    } else {
        (perp / len, true)
    }
}

/// Draws a scattered direction from the needle cone of axis `t` for the incoming propagation
/// direction `w_i` (both unit), from two uniform `[0, 1)` randoms: `u1` picks `s` and `u2` the
/// azimuth.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "s, a, u, v, t are the names of the needle-cone sampling derivation"
)]
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn needle_sample(w_i: Vec3, t: Vec3, beta: f32, rho: f32, u1: f32, u2: f32) -> Vec3 {
    let xi = w_i.dot(t).clamp(-1.0, 1.0);
    let s = if beta <= 0.0 {
        xi
    } else {
        let (mass, lo) = truncation(xi, beta);
        let v = (lo + u1 * mass).clamp(1e-7, 1.0 - 1.2e-7);
        (xi + beta * (v / (1.0 - v)).ln()).clamp(-1.0, 1.0)
    };
    let rho = rho.clamp(0.0, MAX_RHO);
    let a = (1.0 - rho) / (1.0 + rho);
    let phi = 2.0 * (a * (PI * (u2.clamp(1e-6, 1.0 - 1e-6) - 0.5)).tan()).atan();
    let (u, _) = azimuth_origin(w_i, t);
    let v = t.cross(u);
    let (sin_phi, cos_phi) = phi.sin_cos();
    let radial = (1.0 - s * s).max(0.0).sqrt();
    (t * s + (u * cos_phi + v * sin_phi) * radial).normalize_or_zero()
}

/// The density, per steradian, with which [`needle_sample`] returns `w_o`.
#[must_use]
pub fn needle_pdf(w_i: Vec3, t: Vec3, beta: f32, rho: f32, w_o: Vec3) -> f32 {
    if beta <= 0.0 {
        return 0.0;
    }
    let xi = w_i.dot(t).clamp(-1.0, 1.0);
    let s = w_o.dot(t).clamp(-1.0, 1.0);
    let azimuth = {
        let (u, defined) = azimuth_origin(w_i, t);
        let perp = w_o - t * w_o.dot(t);
        let len = perp.length();
        if defined && len >= 1e-6 {
            wrapped_cauchy_density(perp.dot(u) / len, rho)
        } else {
            // The azimuth is undefined at the poles of the cone: uniform there.
            0.5 / PI
        }
    };
    needle_s_density(xi, s, beta) * azimuth
}
