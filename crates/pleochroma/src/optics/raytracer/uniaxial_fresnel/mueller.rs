//! Stokes/Mueller integration.
//!
//! Projecting a real direction onto the Stokes azimuth frame, converting a complex
//! Jones matrix to this crate's real Mueller-matrix convention, and the transmitted
//! power fraction a single output mode receives from a 2-component Stokes input.

use super::complex::Cplx;
use crate::optics::polarization::StokesVector;
use glam::{Mat4, Vec3};

/// Doubled-angle azimuth `(cos(2*psi), sin(2*psi))` of a real world-space direction
/// `dir` (assumed already `perp` to the propagation direction implicit in `s_axis`/
/// `p_axis`) within the `(s_axis, p_axis)` Stokes frame -- the same construction
/// `entry_eigenmode_selection` already uses for `ordinary_eigen_polarization`'s output.
#[must_use]
pub(crate) fn azimuth2_in_frame(dir: Vec3, s_axis: Vec3, p_axis: Vec3) -> (f32, f32) {
    let s_comp = dir.dot(s_axis);
    let p_comp = dir.dot(p_axis);
    let norm = p_comp.mul_add(p_comp, s_comp * s_comp).max(1e-12);
    let cos_2psi = p_comp.mul_add(-p_comp, s_comp * s_comp) / norm;
    let sin_2psi = 2.0 * s_comp * p_comp / norm;
    (cos_2psi, sin_2psi)
}

/// Complex Jones matrix to real Mueller matrix, in this crate's `(I, Q, U, V)` convention
/// (`Q = |Es|^2 - |Ep|^2`, `U = 2*Re(Es*Ep_conj)`, `V = -2*Im(Es*Ep_conj)`). For a real
/// diagonal Jones matrix `diag(r_s, r_p)` it matches `MuellerMatrix::fresnel_reflection`.
/// `j = [[j_ss, j_sp], [j_ps, j_pp]]` maps incident `(E_s, E_p)` to outgoing `(E_s',
/// E_p')`.
#[must_use]
#[expect(
    clippy::similar_names,
    reason = "j_ss/j_sp/j_ps/j_pp and the m_/a_/b_/c_ products are the established Jones and Mueller element names"
)]
pub(crate) fn jones_to_mueller(j_ss: Cplx, j_sp: Cplx, j_ps: Cplx, j_pp: Cplx) -> Mat4 {
    let m_ss = j_ss.norm_sqr();
    let m_sp = j_sp.norm_sqr();
    let m_ps = j_ps.norm_sqr();
    let m_pp = j_pp.norm_sqr();

    let a_ssp = j_ss.mul(j_sp.conj());
    let a_pep = j_ps.mul(j_pp.conj());
    let a_ssp_plus = a_ssp.add(a_pep);
    let a_ssp_minus = a_ssp.sub(a_pep);

    let b_sp = j_ss.mul(j_ps.conj());
    let b_pp = j_sp.mul(j_pp.conj());

    let c_sspp = j_ss.mul(j_pp.conj());
    let c_spps = j_sp.mul(j_ps.conj());

    // Row I
    let i_i = 0.5 * (m_ss + m_ps + m_sp + m_pp);
    let i_q = 0.5 * (m_ss + m_ps - m_sp - m_pp);
    let i_u = a_ssp_plus.re;
    let i_v = a_ssp_plus.im;
    // Row Q
    let q_i = 0.5 * (m_ss - m_ps + m_sp - m_pp);
    let q_q = f32::midpoint(m_ss - m_ps - m_sp, m_pp);
    let q_u = a_ssp_minus.re;
    let q_v = a_ssp_minus.im;
    // Row U
    let u_i = b_sp.re + b_pp.re;
    let u_q = b_sp.re - b_pp.re;
    let u_u = c_sspp.re + c_spps.re;
    let u_v = c_sspp.im - c_spps.im;
    // Row V
    let v_i = -(b_sp.im + b_pp.im);
    let v_q = -(b_sp.im) + b_pp.im;
    let v_u = -(c_sspp.im + c_spps.im);
    let v_v = c_sspp.re - c_spps.re;

    // glam::Mat4::from_cols_array reads COLUMN-major.
    Mat4::from_cols_array(&[
        i_i, q_i, u_i, v_i, // column 0 (contributions from input I)
        i_q, q_q, u_q, v_q, // column 1 (input Q)
        i_u, q_u, u_u, v_u, // column 2 (input U)
        i_v, q_v, u_v, v_v, // column 3 (input V)
    ])
}

/// The transmitted power fraction, in `[0, 1]` for a physical state, that a single output
/// mode receives from a 2-component `(s, p)` input: `T_mode(stokes) = (mode_flux /
/// inc_flux) * [|t_s|^2*(I+Q)/2 + |t_p|^2*(I-Q)/2 + Re(t_s*conj(t_p))*U +
/// Im(t_s*conj(t_p))*V]`. The division by the incident mode's own flux is what makes the
/// result a true power fraction. It drives the mode-selection probability in
/// `apply_partial_fresnel_bounce`.
#[must_use]
pub(crate) fn mode_power(
    t_s: Cplx,
    t_p: Cplx,
    mode_flux: f32,
    inc_flux: f32,
    stokes: StokesVector,
) -> f32 {
    let m_s = t_s.norm_sqr();
    let m_p = t_p.norm_sqr();
    let cross = t_s.mul(t_p.conj());
    let raw = cross.im.mul_add(
        stokes.v,
        cross.re.mul_add(
            stokes.u,
            0.5 * m_s.mul_add(stokes.i + stokes.q, m_p * (stokes.i - stokes.q)),
        ),
    );
    (mode_flux / inc_flux.max(1e-12)) * raw
}
