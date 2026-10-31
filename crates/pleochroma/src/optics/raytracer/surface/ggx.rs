//! The anisotropic GGX (Trowbridge-Reitz) microfacet distribution.
//!
//! Everything here works in the local frame of a facet: `+z` is the macroscopic normal on the
//! side the ray comes from, `+x` the direction across the scratches and `+y` the direction
//! along them. [`GgxLobe`] holds the two roughness widths; its methods are the normal
//! distribution `D`, the Smith masking function `G1` (and the height-correlated `G2`) and the
//! sampler for the distribution of VISIBLE normals (Heitz 2018), which is the micro-normal
//! source of both the polished halo and the ground surface.
//!
//! The WGSL twin (lane RP-G2, `13_surface_ggx.wgsl`) is written against these expressions.

use glam::Vec3;
use std::f32::consts::{PI, TAU};

/// The smallest roughness a lobe accepts; a narrower lobe is a mirror to f32 precision and only
/// invites division by zero.
pub const MIN_ALPHA: f32 = 1e-3;

/// The smallest cosine against the macro normal the samplers work with, so a grazing ray never
/// divides by zero.
pub const MIN_COSINE: f32 = 1e-4;

/// An anisotropic GGX lobe: roughness `alpha_x` across the scratches and `alpha_y` along them
/// (equal for an isotropic surface).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GgxLobe {
    /// Roughness along the local `x` axis (across the grooves).
    pub alpha_x: f32,
    /// Roughness along the local `y` axis (along the grooves).
    pub alpha_y: f32,
}

impl GgxLobe {
    /// A round lobe of width `alpha`.
    #[must_use]
    pub const fn isotropic(alpha: f32) -> Self {
        let alpha = alpha.max(MIN_ALPHA);
        Self {
            alpha_x: alpha,
            alpha_y: alpha,
        }
    }

    /// A lobe of geometric-mean width `alpha` whose widths across and along the grooves stand in
    /// the ratio `ratio` (at least 1): `alpha_x / alpha_y == ratio`, `alpha_x * alpha_y == alpha²`.
    #[must_use]
    pub fn anisotropic(alpha: f32, ratio: f32) -> Self {
        let alpha = alpha.max(MIN_ALPHA);
        let root = ratio.max(1.0).sqrt();
        Self {
            alpha_x: alpha * root,
            alpha_y: (alpha / root).max(MIN_ALPHA),
        }
    }

    /// The geometric mean of the two widths: the width of the round lobe of equal area.
    #[must_use]
    pub fn alpha_mean(&self) -> f32 {
        (self.alpha_x * self.alpha_y).sqrt()
    }

    /// The normal distribution `D(m)` per unit solid angle of `m`; `0` at and below the horizon.
    #[must_use]
    pub fn d(&self, m: Vec3) -> f32 {
        if m.z <= 0.0 {
            return 0.0;
        }
        let qx = m.x / self.alpha_x;
        let qy = m.y / self.alpha_y;
        let q = qx.mul_add(qx, qy.mul_add(qy, m.z * m.z));
        1.0 / (PI * self.alpha_x * self.alpha_y * q * q)
    }

    /// Smith's `Λ(w)`: the shadowing term the masking functions are built from.
    #[must_use]
    pub fn lambda(&self, w: Vec3) -> f32 {
        let z2 = (w.z * w.z).max(1e-12);
        let ax = self.alpha_x * w.x;
        let ay = self.alpha_y * w.y;
        let a2 = ax.mul_add(ax, ay * ay) / z2;
        0.5 * ((1.0 + a2).sqrt() - 1.0)
    }

    /// The masking function `G1(w) = 1 / (1 + Λ(w))`.
    #[must_use]
    pub fn g1(&self, w: Vec3) -> f32 {
        1.0 / (1.0 + self.lambda(w))
    }

    /// The height-correlated masking-shadowing function `G2(wi, wo) = 1 / (1 + Λ(wi) + Λ(wo))`.
    /// Only the squared `z` of each direction enters, so it serves reflection and transmission.
    #[must_use]
    pub fn g2(&self, wi: Vec3, wo: Vec3) -> f32 {
        1.0 / (1.0 + self.lambda(wi) + self.lambda(wo))
    }

    /// The density of the visible normals seen from `wi` (`wi.z > 0`):
    /// `D_vis(m) = G1(wi) max(0, wi·m) D(m) / wi.z`, per unit solid angle of `m`.
    #[must_use]
    pub fn visible_density(&self, wi: Vec3, m: Vec3) -> f32 {
        let cos_im = wi.dot(m).max(0.0);
        self.g1(wi) * cos_im * self.d(m) / wi.z.max(MIN_COSINE)
    }

    /// Draws a micro-normal from the visible-normal distribution seen from `wi` (a unit vector
    /// with `wi.z > 0`) with the uniform pair `(u1, u2)` (Heitz 2018, "Sampling the GGX
    /// Distribution of Visible Normals").
    ///
    /// The returned normal is a unit vector in the upper hemisphere with `wi·m >= 0`.
    #[must_use]
    #[expect(
        clippy::suboptimal_flops,
        clippy::manual_midpoint,
        reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
    )]
    pub fn sample_visible(&self, wi: Vec3, u1: f32, u2: f32) -> Vec3 {
        // Stretch the view direction into the hemisphere configuration.
        let vh = Vec3::new(self.alpha_x * wi.x, self.alpha_y * wi.y, wi.z).normalize();
        let len2 = vh.x.mul_add(vh.x, vh.y * vh.y);
        let t1 = if len2 > 0.0 {
            Vec3::new(-vh.y, vh.x, 0.0) / len2.sqrt()
        } else {
            Vec3::X
        };
        let t2 = vh.cross(t1);
        // A point on the projected disk, warped to the visible half.
        let r = u1.sqrt();
        let phi = TAU * u2;
        let (sin_phi, cos_phi) = phi.sin_cos();
        let p1 = r * cos_phi;
        let p2_disk = r * sin_phi;
        let s = 0.5 * (1.0 + vh.z);
        let p2 = (1.0 - s).mul_add((1.0 - p1 * p1).max(0.0).sqrt(), s * p2_disk);
        // Reproject onto the hemisphere and unstretch.
        let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
        Vec3::new(self.alpha_x * nh.x, self.alpha_y * nh.y, nh.z.max(0.0)).normalize()
    }
}
