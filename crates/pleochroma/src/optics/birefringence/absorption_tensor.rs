//! Symmetric second-rank absorption tensor [`AbsorptionTensor3`] (pleochroism) and the
//! pleochroic absorption coefficient functions built on it.

use super::{helpers::stable_orthonormal_basis, uniaxial::BirefringenceParams};
use crate::optics::polarization::{StokesVector, electric_field_direction};
use glam::{Mat3, Vec3};

/// Symmetric second-rank absorption tensor **A**, diagonal in the crystal's principal
/// axes.
///
/// Represented as three principal coefficients plus that orthonormal axis frame
/// (`axes`) rather than a dense 3x3 matrix -- cheaper to evaluate, and the natural form
/// gem chromophore data already takes (an ordinary/extraordinary pair via
/// [`Self::uniaxial`], or three independent coefficients for a fully biaxial tensor).
///
/// Absorption in an anisotropic crystal depends on the light's electric-field
/// polarization direction, not its propagation direction -- this is pleochroism (e.g.
/// tanzanite reading blue, violet or brownish by polarization). See
/// [`Self::quadratic_form`].
#[derive(Debug, Clone, Copy)]
pub struct AbsorptionTensor3 {
    /// Principal absorption coefficients, one per column of `axes`, same order.
    pub alpha: Vec3,
    /// Orthonormal principal-axis frame in world space; columns are the three
    /// principal directions the tensor is diagonal in.
    pub axes: Mat3,
}

impl AbsorptionTensor3 {
    /// Builds the uniaxial special case directly from a material's existing
    /// ordinary/extraordinary absorption coefficients and single optical c-axis: two
    /// degenerate principal coefficients (`alpha_o`) in the plane perpendicular to
    /// `c_axis`, and one (`alpha_e`) along it. The in-plane pair of principal axes is
    /// arbitrary -- the tensor is isotropic within that plane, so their specific
    /// directions don't affect `quadratic_form`'s result -- and filled in via
    /// `stable_orthonormal_basis`.
    #[must_use]
    pub fn uniaxial(alpha_o: f32, alpha_e: f32, c_axis: Vec3) -> Self {
        let c = c_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(c);
        Self {
            alpha: Vec3::new(alpha_o, alpha_o, alpha_e),
            axes: Mat3::from_cols(a1, a2, c),
        }
    }

    /// Trichroism: the genuinely biaxial case -- three independent principal absorption
    /// coefficients rather than `uniaxial`'s degenerate `(alpha_o, alpha_o, alpha_e)`
    /// pair.
    ///
    /// Builds its axis frame via the exact same `stable_orthonormal_basis(gamma_axis)`
    /// call, on the exact same input, that `BiaxialIndicatrix::from_gamma_axis` uses
    /// for the material's index frame -- load-bearing, since `eigen_polarizations`'
    /// eigenmodes (which `quadratic_form` is evaluated against) live in that same
    /// index frame; any drift between the two would silently score absorption against
    /// the wrong axes. `gamma_axis` alone determines the frame: `alpha` lands on the
    /// first `stable_orthonormal_basis` output, `beta` on the second, `gamma` on
    /// `gamma_axis` itself (e.g. `gamma_axis = +Y` gives `alpha` -> `+X`, `beta` ->
    /// `-Z`, `gamma` -> `+Y`).
    ///
    /// Degenerate case: `alpha == beta` reproduces `uniaxial(alpha, gamma, gamma_axis)`
    /// bit-identically (same `Vec3::new` arguments, same `stable_orthonormal_basis`
    /// call).
    #[must_use]
    pub fn biaxial(alpha: f32, beta: f32, gamma: f32, gamma_axis: Vec3) -> Self {
        let g = gamma_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(g);
        Self {
            alpha: Vec3::new(alpha, beta, gamma),
            axes: Mat3::from_cols(a1, a2, g),
        }
    }

    /// Evaluates the quadratic form `alpha = e_hat . A . e_hat` for a unit electric-
    /// field polarization direction `e_hat`. Since `A` is diagonal in `axes`, this is
    /// just the alpha-weighted sum of `e_hat`'s squared components along each
    /// principal axis -- no dense 3x3 matrix multiply needed.
    #[must_use]
    pub fn quadratic_form(&self, e_hat: Vec3) -> f32 {
        let local = self.axes.transpose() * e_hat;
        self.alpha.dot(local * local)
    }
}

/// Assigned-mode pleochroic absorption coefficient: `alpha = e_mode_hat . A . e_mode_hat`.
///
/// For light known to occupy exactly one eigenmode (`e_mode_hat`). No Stokes vector is read.
///
/// # Why not [`effective_pleochroic_alpha`] on the interior transport path
///
/// A traced path is assigned to ONE eigenmode (`is_extraordinary`) at its air->crystal entry,
/// and that mode's E-field follows from the geometry, so it need not come from the Stokes
/// vector. The Stokes-based direction fails in two ways: on camera-traced paths the Stokes
/// vector carries importance rather than the physical state, and its degree-of-polarization
/// blend is non-linear, which breaks the adjoint identity behind camera-side Mueller
/// products; and after internal reflections its azimuth drifts off the assigned mode's axis,
/// because `apply_tir_bounce` scales only the magnitude.
///
/// The per-channel driver is `raytracer::absorption::channel_absorption_alphas_assigned`.
/// An isotropic material has no meaningful eigenmode assignment, so that driver handles it
/// without this function.
#[must_use]
pub fn assigned_mode_alpha(tensor: &AbsorptionTensor3, e_mode_hat: Vec3) -> f32 {
    tensor.quadratic_form(e_mode_hat)
}

/// The assigned uniaxial eigenmode's world-space electric-field direction at the CURRENT
/// wave normal `k_hat`.
///
/// - Ordinary mode (`!is_extraordinary`): identical to
///   [`BirefringenceParams::ordinary_eigen_polarization`] -- perpendicular to the plane
///   containing `k_hat` and `c_axis`.
/// - Extraordinary mode: the e-mode's E-field does NOT lie perpendicular to `k_hat` (that
///   would be D, not E) -- it lies IN the `(k_hat, c_axis)` plane, perpendicular to the
///   Poynting vector `S` (`S` from [`BirefringenceParams::extraordinary_poynting_dir`],
///   evaluated at the hero channel's own `n_o_hero`/`n_e_hero` -- the walk-off direction
///   is a per-ray geometric quantity, not per-channel). Computed as
///   `normalize((k_hat x c_axis) x S)`: `k_hat x c_axis` is the ordinary axis direction
///   (unnormalized), so this is perpendicular to both the ordinary axis and `S`, which
///   places it exactly in the `(k_hat, c_axis)` plane and perpendicular to `S` as
///   required.
///
/// Degenerates to [`BirefringenceParams::extraordinary_eigen_polarization`] when `k_hat`
/// is parallel to `c_axis` (`|k_hat x c_axis|^2 < 1e-8`): both eigenmodes collapse to
/// ordinary there (no birefringence along the optic axis), and the `(k_hat, c_axis)`
/// plane itself is undefined.
#[must_use]
#[expect(
    clippy::similar_names,
    reason = "n_o_hero/n_e_hero are the established ordinary/extraordinary index pair"
)]
pub fn assigned_mode_e_field_uniaxial(
    k_hat: Vec3,
    c_axis: Vec3,
    is_extraordinary: bool,
    n_o_hero: f32,
    n_e_hero: f32,
) -> Vec3 {
    if !is_extraordinary {
        return BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis);
    }
    let k_cross_c = k_hat.cross(c_axis);
    if k_cross_c.length_squared() < 1e-8 {
        return BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis);
    }
    let s = BirefringenceParams::extraordinary_poynting_dir(k_hat, c_axis, n_o_hero, n_e_hero);
    k_cross_c.cross(s).normalize()
}

/// Effective pleochroic (polarization-dependent) absorption coefficient.
///
/// Combines the light's actual electric-field direction (for the polarized fraction)
/// with an unweighted average over the two eigenmodes (for the unpolarized fraction)
/// -- unpolarized light couples equally into both eigenmodes, so its absorption is
/// their average regardless of the crystal's own principal axes.
///
/// `degree_of_polarization` in `[0, 1]` (see `StokesVector::degree_of_polarization`)
/// linearly interpolates between the two limits. An isotropic tensor (all principal
/// coefficients equal) returns the same value regardless of direction, so isotropic
/// materials are automatically azimuth-independent with no special case needed here.
///
/// Not used by the interior transport path, which calls [`assigned_mode_alpha`] instead.
/// Kept for the unit tests and the Tier 2 GPU harness (`renderer::gpu::transport_check`),
/// which pin [`pleochroic_channel_alpha`] (built on this) against its WGSL translation.
#[must_use]
pub fn effective_pleochroic_alpha(
    tensor: &AbsorptionTensor3,
    e_hat: Vec3,
    eigenmode_a: Vec3,
    eigenmode_b: Vec3,
    degree_of_polarization: f32,
) -> f32 {
    let p = degree_of_polarization.clamp(0.0, 1.0);
    let alpha_polarized = tensor.quadratic_form(e_hat);
    let alpha_unpolarized = f32::midpoint(
        tensor.quadratic_form(eigenmode_a),
        tensor.quadratic_form(eigenmode_b),
    );
    p.mul_add(alpha_polarized - alpha_unpolarized, alpha_unpolarized)
}

/// Beer-Lambert absorption coefficient for one spectral channel, from its own Stokes vector.
///
/// The channel's electric-field direction comes from `stokes` (see
/// `electric_field_direction`); the tensor is built from `alpha_o`/`alpha_e` and, when
/// `alpha_beta` is `Some`, a biaxial third coefficient; the two eigenmode directions are
/// combined via [`effective_pleochroic_alpha`].
///
/// `alpha_beta` is `Some` only for a genuinely biaxial material, whose eigenmodes come from
/// `BiaxialIndicatrix::eigen_polarizations`. `None` builds the uniaxial tensor.
///
/// Not used by the interior transport path (see [`effective_pleochroic_alpha`]). Kept for the
/// unit tests and the Tier 2 GPU harness.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "argument order deliberately mirrors transport_physics.wgsl's \
              pleochroic_channel_alpha/pleochroic_channel_alpha_biaxial (alpha_o, \
              alpha_e, [alpha_beta,] c_axis, s_axis, propagation_dir, eigen_a, eigen_b, \
              stokes) one-for-one; bundling these into a context struct here would break \
              that correspondence and make it harder to check the CPU and GPU paths \
              against each other, which is the whole point of this function existing \
              as a direct WGSL mirror"
)]
pub fn pleochroic_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    alpha_beta: Option<f32>,
    c_axis: Vec3,
    s_axis: Vec3,
    propagation_dir: Vec3,
    eigenmode_a: Vec3,
    eigenmode_b: Vec3,
    stokes: &StokesVector,
) -> f32 {
    let tensor = alpha_beta.map_or_else(
        || AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis),
        |beta| AbsorptionTensor3::biaxial(alpha_o, beta, alpha_e, c_axis),
    );
    let e_hat = electric_field_direction(stokes, s_axis, propagation_dir);
    effective_pleochroic_alpha(
        &tensor,
        e_hat,
        eigenmode_a,
        eigenmode_b,
        stokes.degree_of_polarization(),
    )
}
