//! The local incidence frame and the closed-form mode fields built from it.
//!
//! The ordinary/extraordinary wavevector normal-component roots, the per-mode field
//! vectors built from them, and the Poynting flux those fields carry through the
//! interface.

use super::complex::{CVec3, Cplx};
use glam::Vec3;

/// The local orthonormal frame and optic-axis direction cosines shared by every channel at
/// one bounce. Purely geometric, so built once per bounce and reused for each channel's
/// own `n_o`/`n_e` solve.
#[derive(Clone, Copy)]
pub(crate) struct UniaxialFrame {
    pub that: Vec3,
    pub zhat: Vec3,
    pub s_axis: Vec3,
    /// `k_hat x s_axis`, perpendicular to the incident wave normal only. Transmitted and
    /// reflected modes need their own transverse axis (Snell's law changes their
    /// direction), so callers use `p_axis_transmitted` from `apply_uniaxial_entry_bounce`.
    #[expect(
        dead_code,
        reason = "kept for API completeness -- see doc comment above"
    )]
    pub p_axis: Vec3,
    pub cos_i: f32,
    pub sin_i: f32,
    pub alpha: f32,
    pub beta: f32,
    pub gamma: f32,
}

impl UniaxialFrame {
    /// Builds the frame from the `k_hat`, `normal`, `cos_i` and `sin_i` of the bounce.
    /// `normal` faces the incident ray (`(-k_hat).dot(normal) == cos_i`, as in
    /// [`compute_bounce_refraction_geometry`]). At normal incidence (`sin_i ~ 0`) the
    /// in-plane axes are only defined up to a rotation about `normal`, so a fixed fallback
    /// axis is used there.
    #[must_use]
    pub(crate) fn build(k_hat: Vec3, normal: Vec3, c_axis: Vec3, cos_i: f32, sin_i: f32) -> Self {
        let zhat = -normal;
        let that = if sin_i > 1e-5 {
            ((k_hat + normal * cos_i) / sin_i).normalize_or_zero()
        } else {
            // Arbitrary but stable in-plane axis at normal incidence.
            let fallback = if zhat.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            (fallback - zhat * zhat.dot(fallback)).normalize_or_zero()
        };
        let s_axis = k_hat.cross(normal).normalize_or_zero();
        let s_axis = if s_axis.length_squared() > 1e-8 {
            s_axis
        } else {
            zhat.cross(that).normalize_or_zero()
        };
        let p_axis = k_hat.cross(s_axis);
        Self {
            that,
            zhat,
            s_axis,
            p_axis,
            cos_i,
            sin_i,
            alpha: c_axis.dot(that),
            beta: c_axis.dot(s_axis),
            gamma: c_axis.dot(zhat),
        }
    }
}

/// Closed-form normal-component (`q`, along `zhat`) roots of the ordinary and
/// extraordinary waves, after Lekner (1991). `_plus` is the root that propagates or
/// decays along `+zhat`, into the medium. `_minus` is the other root. For the extraordinary
/// wave it is not simply `-qe_plus`, because of the walk-off term `alpha * gamma`.
///
/// `tangential` is the shared tangential wavevector magnitude `K = n1 * sin_i`, where `n1`
/// is the current medium's index (`1.0` for air, or the incident mode's index at an
/// internal or exit interface).
#[derive(Clone, Copy)]
pub(crate) struct QRoots {
    pub qo_plus: Cplx,
    pub qo_minus: Cplx,
    pub qe_plus: Cplx,
    pub qe_minus: Cplx,
}

#[must_use]
#[expect(
    clippy::float_cmp,
    clippy::similar_names,
    reason = "n_o == n_e is the exact isotropic limit (a stored zero birefringence); qo/qe and n_o2/n_e2 are the established ordinary/extraordinary names"
)]
pub(crate) fn uniaxial_q_roots(
    n_o: f32,
    n_e: f32,
    frame: &UniaxialFrame,
    tangential: f32,
) -> QRoots {
    let n_o2 = n_o * n_o;
    let k2 = tangential * tangential;

    let qo2 = Cplx::re(n_o2 - k2);
    let qo_plus = qo2.sqrt_forward_branch();
    let qo_minus = Cplx::ZERO.sub(qo_plus);

    // Isotropic limit (`n_o == n_e`, e.g. a zero stored birefringence): the extraordinary
    // roots equal the ordinary ones, so the extraordinary square root is skipped.
    if n_o == n_e {
        return QRoots {
            qo_plus,
            qo_minus,
            qe_plus: qo_plus,
            qe_minus: qo_minus,
        };
    }

    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    let denom = (frame.gamma * frame.gamma).mul_add(deleps, n_o2);
    let d_val = n_o2
        * (frame.beta * frame.beta).mul_add(-deleps, n_e2).mul_add(
            -k2,
            n_e2 * (frame.gamma * frame.gamma).mul_add(deleps, n_o2),
        );
    let sqrt_d = Cplx::re(d_val).sqrt_forward_branch();
    let shift = Cplx::re(frame.alpha * frame.gamma * tangential * deleps);
    let denom_c = Cplx::re(denom.max(1e-12));
    let qe_plus = sqrt_d.sub(shift).div(denom_c);
    let qe_minus = Cplx::ZERO.sub(sqrt_d).sub(shift).div(denom_c);

    QRoots {
        qo_plus,
        qo_minus,
        qe_plus,
        qe_minus,
    }
}

/// One uniaxial eigenmode's `(k, E, H)` fields at wavevector normal-component `q`,
/// amplitude 1. `D_o = k x c_axis` (always `perp c_axis` by construction -- the
/// ordinary eigenmode's defining property), `E_o = D_o / n_o^2` (the ordinary
/// permittivity is scalar `n_o^2` in the plane `perp c_axis`, which `D_o` lies in
/// entirely). `D_e = k x D_o` (the unique direction `perp k` AND `perp D_o`, the
/// extraordinary eigenmode's defining property), `E_e = eps^-1 D_e` via the uniaxial
/// tensor inverse `eps^-1 = I/n_o^2 - ((n_e^2-n_o^2)/(n_o^2*n_e^2)) * c_axis c_axis^T`.
/// `H = k x E` in both cases.
pub(crate) struct ModeFields {
    /// Wavevector of the mode. No current caller reads it: directions come from the real
    /// part of `e`.
    #[expect(
        dead_code,
        reason = "public field kept for API completeness -- see doc comment above"
    )]
    pub k: CVec3,
    pub e: CVec3,
    pub h: CVec3,
}

#[must_use]
pub(crate) fn ordinary_mode_fields(
    n_o: f32,
    c_axis: Vec3,
    that: Vec3,
    zhat: Vec3,
    tangential: f32,
    q: Cplx,
) -> ModeFields {
    let k = CVec3::wavevector(that, zhat, tangential, q);
    let d_o = k.cross_real(c_axis);
    let e = d_o.scale_real(1.0 / (n_o * n_o));
    let h = k.cross(e);
    ModeFields { k, e, h }
}

#[must_use]
#[expect(
    clippy::similar_names,
    reason = "n_o2/n_e2 are the established ordinary/extraordinary squared-index names"
)]
pub(crate) fn extraordinary_mode_fields(
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    that: Vec3,
    zhat: Vec3,
    tangential: f32,
    q: Cplx,
) -> ModeFields {
    let n_o2 = n_o * n_o;
    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    let k = CVec3::wavevector(that, zhat, tangential, q);
    let d_o = k.cross_real(c_axis);
    let d_e = k.cross(d_o);
    let c_dot_de = d_e.dot_real(c_axis);
    let e = d_e
        .scale_real(1.0 / n_o2)
        .add(CVec3::from_real(c_axis).scale_complex(c_dot_de.scale(-(deleps / (n_o2 * n_e2)))));
    let h = k.cross(e);
    ModeFields { k, e, h }
}

/// `0.5 * Re(E x H*) . zhat` -- the time-averaged Poynting flux (in `k0`-normalized
/// units) through the interface, for `ModeFields` scaled by complex amplitude `amp`.
#[must_use]
pub(crate) fn poynting_z(fields: &ModeFields, amp: Cplx, zhat: Vec3) -> f32 {
    let e = fields.e.scale_complex(amp);
    let h = fields.h.scale_complex(amp);
    let hc = h.conj();
    // (E x H*).zhat, complex; the physical flux is 0.5*Re(...).
    let s = e.cross(hc);
    0.5 * s.dot_real(zhat).re
}
