//! The full crystal frame of a biaxial material.
//!
//! It is the second principal direction (`alpha`) beside the `gamma` axis, and the framed
//! constructors of [`BiaxialIndicatrix`] and [`AbsorptionTensor3`] that take it.
//!
//! Until the second direction existed, every biaxial construction site filled the
//! frame in with `stable_orthonormal_basis(gamma)`: alpha and beta landed on an
//! arbitrary pair perpendicular to gamma. The constructors here are the opt-in
//! alternative (`GemMaterial::alpha_axis == Some`); the `None` path never reaches
//! this file, so a material without a second direction renders bit-identically to before.

use super::{
    absorption_tensor::AbsorptionTensor3, biaxial::BiaxialIndicatrix,
    helpers::stable_orthonormal_basis,
};
use glam::{Mat3, Vec3};

/// The alpha direction for a gamma axis turned by `turn_deg` about it.
///
/// Turn `0.0` is exactly the alpha direction the gamma-only completion produces for
/// `gamma` (`stable_orthonormal_basis(gamma).0`, the one
/// `BiaxialIndicatrix::from_gamma_axis` and `AbsorptionTensor3::biaxial` both use), bit
/// for bit; a positive turn is a right-handed rotation of that direction about `gamma`.
/// A turn of 180 degrees gives the negated vector, the same line (the indicatrix
/// ellipsoid is symmetric under `alpha -> -alpha`).
///
/// `gamma` need not be normalised; the result is a unit vector perpendicular to it.
#[must_use]
pub fn alpha_axis_from_turn(gamma: Vec3, turn_deg: f32) -> Vec3 {
    let g = gamma.normalize_or_zero();
    let (alpha0, _) = stable_orthonormal_basis(g);
    if turn_deg == 0.0 {
        // The exact completion, no rotation arithmetic (`x * 1 + y * 0` could turn a
        // `-0.0` component into `0.0`).
        return alpha0;
    }
    let (sin, cos) = turn_deg.to_radians().sin_cos();
    // Rodrigues' rotation of `alpha0` about the unit axis `g`.
    let rotated = alpha0 * cos + g.cross(alpha0) * sin + g * (g.dot(alpha0) * (1.0 - cos));
    rotated.normalize_or_zero()
}

/// The orthonormal frame columns `(alpha, beta, gamma)` for a gamma axis and a requested
/// alpha direction.
///
/// `alpha_axis` that is already unit and perpendicular to `gamma_axis` (to `1e-6`, which
/// is what [`alpha_axis_from_turn`] returns) is used as given; anything else is projected
/// perpendicular to gamma and renormalised, so a stale second direction (the material's
/// c-axis was overridden after it was set) still yields an orthonormal frame. A request
/// that is parallel to gamma, or zero, falls back to the gamma-only completion.
/// Beta is `gamma x alpha`, the same right-handed construction
/// `stable_orthonormal_basis` uses.
fn framed_axes(gamma_axis: Vec3, alpha_axis: Vec3) -> Mat3 {
    let g = gamma_axis.normalize_or_zero();
    let along = g.dot(alpha_axis);
    let alpha = if along.abs() < 1e-6 && (alpha_axis.length_squared() - 1.0).abs() < 1e-6 {
        alpha_axis
    } else {
        let projected = alpha_axis - g * along;
        if projected.length_squared() > 1e-10 {
            projected.normalize()
        } else {
            stable_orthonormal_basis(g).0
        }
    };
    Mat3::from_cols(alpha, g.cross(alpha), g)
}

impl BiaxialIndicatrix {
    /// Builds a genuinely biaxial indicatrix from the three principal indices, the
    /// `n_gamma` axis and the `n_alpha` direction (the full crystal frame): the
    /// counterpart of [`Self::from_gamma_axis`] for a material that states its second
    /// direction. `alpha_axis` is made perpendicular to `gamma_axis` if it is not
    /// already; see `framed_axes`.
    #[must_use]
    pub fn from_frame(
        n_alpha: f32,
        n_beta: f32,
        n_gamma: f32,
        gamma_axis: Vec3,
        alpha_axis: Vec3,
    ) -> Self {
        Self::new(
            n_alpha,
            n_beta,
            n_gamma,
            framed_axes(gamma_axis, alpha_axis),
        )
    }

    /// [`Self::from_frame`] when `alpha_axis` is `Some`, else exactly
    /// [`Self::from_gamma_axis`].
    #[must_use]
    pub fn from_oriented_frame(
        n_alpha: f32,
        n_beta: f32,
        n_gamma: f32,
        gamma_axis: Vec3,
        alpha_axis: Option<Vec3>,
    ) -> Self {
        alpha_axis.map_or_else(
            || Self::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis),
            |alpha| Self::from_frame(n_alpha, n_beta, n_gamma, gamma_axis, alpha),
        )
    }
}

impl AbsorptionTensor3 {
    /// Trichroism with the full crystal frame: [`Self::biaxial`] with the alpha principal
    /// axis given instead of completed arbitrarily. Builds its axes through the same
    /// `framed_axes` as [`BiaxialIndicatrix::from_frame`], so the tensor frame and the
    /// index frame stay identical bit for bit (the eigenmodes `quadratic_form` is
    /// evaluated against live in the index frame).
    #[must_use]
    pub fn biaxial_framed(
        alpha: f32,
        beta: f32,
        gamma: f32,
        gamma_axis: Vec3,
        alpha_axis: Vec3,
    ) -> Self {
        Self {
            alpha: Vec3::new(alpha, beta, gamma),
            axes: framed_axes(gamma_axis, alpha_axis),
        }
    }

    /// [`Self::biaxial_framed`] when `alpha_axis` is `Some`, else exactly
    /// [`Self::biaxial`].
    #[must_use]
    pub fn biaxial_oriented(
        alpha: f32,
        beta: f32,
        gamma: f32,
        gamma_axis: Vec3,
        alpha_axis: Option<Vec3>,
    ) -> Self {
        alpha_axis.map_or_else(
            || Self::biaxial(alpha, beta, gamma, gamma_axis),
            |a| Self::biaxial_framed(alpha, beta, gamma, gamma_axis, a),
        )
    }
}
