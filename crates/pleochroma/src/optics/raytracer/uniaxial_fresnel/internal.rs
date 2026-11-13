//! The uniaxial internal-reflection/exit-transmission interface's closed-form solve.
//!
//! Reflection (with o<->e coupling) and exit transmission from ONE boundary-value
//! solve, including genuine total-internal-reflection (evanescent) cases.

use super::{
    complex::{CVec3, Cplx},
    frame::{
        ModeFields, UniaxialFrame, extraordinary_mode_fields, ordinary_mode_fields, poynting_z,
        uniaxial_q_roots,
    },
    linalg::{solve4, tangential_components},
};
use glam::Vec3;

/// The full closed-form solution at a uniaxial (incident mode `o` or `e`, from inside
/// the crystal) -> isotropic (air, `n2 = 1`) interface: internal reflection
/// (o<->e coupling included) AND exit transmission from ONE boundary-value solve.
/// `n_mode_inc` is the incident mode's OWN effective index at this direction/channel
/// (`n_o` for an ordinary incidence, the direction-dependent effective extraordinary
/// index for an extraordinary incidence).
///
/// `Copy` so that `refraction::apply_uniaxial_internal_bounce` can reuse the hero channel's
/// solution, which it already computes to choose reflect or transmit, instead of solving the
/// same boundary system again.
#[derive(Clone, Copy)]
pub(crate) struct InternalPolarizationSolution {
    /// Reflected amplitude into the SAME uniaxial medium's ordinary mode (backward root).
    pub r_o: Cplx,
    /// Reflected amplitude into the extraordinary mode (backward root).
    pub r_e: Cplx,
    /// Transmitted amplitude into the isotropic exit medium's s-polarization. Read by
    /// `refraction::apply_uniaxial_internal_transmit_channels` (the exit-transmission
    /// half of `apply_uniaxial_internal_bounce`, reached from
    /// `apply_partial_fresnel_bounce` whenever `inside_gem &&
    /// geo.uniaxial_frame.is_some()`) -- see that function's own doc comment for the
    /// flux-normalized-amplitude-to-Stokes derivation. Also exercised directly by this
    /// module tree's own `internal_exit_energy_conservation_holds_including_tir` test.
    pub t_s: Cplx,
    /// See `t_s`'s doc comment.
    pub t_p: Cplx,
    /// Intrinsic (amplitude-1) Poynting flux of the ordinary BACKWARD mode -- turns
    /// `r_o` into a reflected power fraction.
    pub flux_ro: f32,
    pub flux_re: f32,
    pub flux_ts: f32,
    pub flux_tp: f32,
    /// The incident mode's own intrinsic Poynting flux. The extraordinary flux is not
    /// `n_mode_inc * cos_i` (its Poynting vector walks off the wave normal), so callers
    /// must use this value to turn `r_o`, `r_e`, `t_s` and `t_p` into power fractions.
    pub flux_inc: f32,
    /// World-space real E-field directions of the reflected o and e modes. The bounce
    /// wirings take the next mode label from `apply_internal_mode_coupling`'s
    /// Poynting-weighted split instead, so bounce dispatch does not read these. The GPU
    /// parity check `renderer::gpu::transport_check::p2_uniaxial_fresnel` does.
    #[cfg_attr(
        not(feature = "gpu"),
        expect(
            dead_code,
            reason = "validated reflected-mode-axis data, not wired into any bounce \
                      dispatch -- see doc comment above; read by the gpu-feature-gated \
                      Tier 2 parity check"
        )
    )]
    pub o_hat: Vec3,
    #[cfg_attr(
        not(feature = "gpu"),
        expect(
            dead_code,
            reason = "validated reflected-mode-axis data, not wired into any bounce \
                      dispatch -- see o_hat's doc comment"
        )
    )]
    pub e_hat: Vec3,
}

#[must_use]
#[expect(
    clippy::similar_names,
    reason = "ordinary/extraordinary and s/p pairs (f_o_bwd/f_e_bwd, es_t/ep_t, flux_ro/flux_re, flux_ts/flux_tp) are the established mode names"
)]
pub(crate) fn internal_solve(
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
    incident_is_ordinary: bool,
) -> InternalPolarizationSolution {
    let k_tan = n_mode_inc * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);

    let f_inc = if incident_is_ordinary {
        ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus)
    } else {
        extraordinary_mode_fields(
            n_o,
            n_e,
            c_axis,
            frame.that,
            frame.zhat,
            k_tan,
            roots.qe_plus,
        )
    };
    let f_o_bwd = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_minus);
    let f_e_bwd = extraordinary_mode_fields(
        n_o,
        n_e,
        c_axis,
        frame.that,
        frame.zhat,
        k_tan,
        roots.qe_minus,
    );

    // Isotropic exit side (n2 = 1), forward (away from interface, into air).
    let n2 = 1.0f32;
    let zeta = k_tan / n2;
    let cos_t = if zeta.abs() <= 1.0 {
        Cplx::re(zeta.mul_add(-zeta, 1.0).max(0.0).sqrt())
    } else {
        Cplx {
            re: 0.0,
            im: zeta.mul_add(zeta, -1.0).sqrt(),
        }
    };
    let k_iso = CVec3::wavevector(frame.that, frame.zhat, k_tan, cos_t.scale(n2));
    let s_axis_c = CVec3::from_real(frame.s_axis);
    let es_t = s_axis_c;
    let hs_t = k_iso.cross(es_t);
    let ep_t = k_iso.cross(s_axis_c).scale_real(-1.0 / (n2 * n2));
    let hp_t = k_iso.cross(ep_t);
    let f_s_t = ModeFields {
        k: k_iso,
        e: es_t,
        h: hs_t,
    };
    let f_p_t = ModeFields {
        k: k_iso,
        e: ep_t,
        h: hp_t,
    };

    let inc_c = tangential_components(&f_inc, frame.that, frame.s_axis);
    let eo_c = tangential_components(&f_o_bwd, frame.that, frame.s_axis);
    let ee_c = tangential_components(&f_e_bwd, frame.that, frame.s_axis);
    let es_c = tangential_components(&f_s_t, frame.that, frame.s_axis);
    let ep_c = tangential_components(&f_p_t, frame.that, frame.s_axis);

    let mut a = [[Cplx::ZERO; 4]; 4];
    let mut b = [Cplx::ZERO; 4];
    for i in 0..4 {
        a[i] = [
            eo_c[i],
            ee_c[i],
            Cplx::ZERO.sub(es_c[i]),
            Cplx::ZERO.sub(ep_c[i]),
        ];
        b[i] = Cplx::ZERO.sub(inc_c[i]);
    }
    let sol = solve4(a, b);
    let [r_o, r_e, t_s, t_p] = sol;

    let flux_ro = poynting_z(&f_o_bwd, Cplx::re(1.0), frame.zhat).abs();
    let flux_re = poynting_z(&f_e_bwd, Cplx::re(1.0), frame.zhat).abs();
    let flux_ts = poynting_z(&f_s_t, Cplx::re(1.0), frame.zhat).abs();
    let flux_tp = poynting_z(&f_p_t, Cplx::re(1.0), frame.zhat).abs();
    let flux_inc = poynting_z(&f_inc, Cplx::re(1.0), frame.zhat).abs();

    let o_hat = f_o_bwd.e.re.normalize_or_zero();
    let e_hat = f_e_bwd.e.re.normalize_or_zero();

    InternalPolarizationSolution {
        r_o,
        r_e,
        t_s,
        t_p,
        flux_ro,
        flux_re,
        flux_ts,
        flux_tp,
        flux_inc,
        o_hat,
        e_hat,
    }
}
