//! The isotropic (air, `n1`) -> uniaxial crystal entry interface's closed-form solve.
//!
//! Batched to share the incidence-side isotropic mode data across every hero channel
//! of one bounce.

use super::{
    complex::{CVec3, Cplx},
    frame::{
        ModeFields, UniaxialFrame, extraordinary_mode_fields, ordinary_mode_fields, poynting_z,
        uniaxial_q_roots,
    },
    linalg::{solve4_two_rhs, tangential_components},
};
use glam::Vec3;

/// The full closed-form solution at an isotropic (air, `n1`) -> uniaxial entry
/// interface, for ONE incident polarization (`s` or `p`, amplitude 1). Returns the
/// reflected amplitudes in the iso (s, p) basis and the transmitted amplitudes in the
/// crystal (o, e) basis -- Lekner's `r_ss/r_ps` (or `r_sp/r_pp`) and `t_so/t_eo` (or
/// `t_sp/t_ep`) for this incident polarization, plus each mode's own intrinsic Poynting
/// flux (amplitude-1 `poynting_z`, needed to turn a transmitted AMPLITUDE into a
/// transmitted POWER).
pub(crate) struct EntryPolarizationSolution {
    pub r_s: Cplx,
    pub r_p: Cplx,
    pub t_o: Cplx,
    pub t_e: Cplx,
    /// Intrinsic (amplitude-1) Poynting flux of the transmitted o and e modes. It does not
    /// depend on the incident polarization.
    pub flux_o: f32,
    pub flux_e: f32,
    /// World-space real E-field directions of the transmitted o/e modes -- always
    /// real (an entry interface's transmitted side never evanescesces: `n1 == 1.0` is
    /// less than every built-in `n_o`/`n_e`), for a caller's own Stokes-azimuth
    /// projection via [`azimuth2_in_frame`](super::azimuth2_in_frame).
    pub o_hat: Vec3,
    pub e_hat: Vec3,
}

/// Single-polarization form of [`entry_solve_pair`]: returns the solution for `incident_s`
/// and discards the other. Production code needs both polarizations, so it calls
/// [`entry_solve_pair`] directly.
#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "kept for tests/possible future single-polarization callers -- see doc comment above"
    )
)]
pub(crate) fn entry_solve(
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
    incident_s: bool,
) -> EntryPolarizationSolution {
    let (s_sol, p_sol) = entry_solve_pair(n1, n_o, n_e, c_axis, frame);
    if incident_s { s_sol } else { p_sol }
}

/// The incidence-side isotropic (s, p) modes and their tangential components, which depend
/// only on `n1` and `frame`, not on the crystal indices. Built once per bounce and shared
/// by all hero channels; the result is bit-identical to building it per channel.
#[derive(Clone, Copy)]
pub(crate) struct EntryIncidenceFrame {
    incident_s: [Cplx; 4],
    incident_p: [Cplx; 4],
    reflected_s: [Cplx; 4],
    reflected_p: [Cplx; 4],
}

#[must_use]
#[expect(
    clippy::similar_names,
    reason = "k_fwd/k_bwd and ep_fwd/ep_bwd are the established forward/backward mode names"
)]
pub(crate) fn entry_incidence_frame(n1: f32, frame: &UniaxialFrame) -> EntryIncidenceFrame {
    let k_tan = n1 * frame.sin_i;
    // Isotropic incidence-side modes (forward = incident, backward = reflected), with the
    // scalar r_s/r_p sign convention, checked by the zero-birefringence test in `tests.rs`.
    let k_fwd = CVec3::wavevector(frame.that, frame.zhat, k_tan, Cplx::re(n1 * frame.cos_i));
    let k_bwd = CVec3::wavevector(frame.that, frame.zhat, k_tan, Cplx::re(-n1 * frame.cos_i));
    let s_axis_c = CVec3::from_real(frame.s_axis);
    let es_fwd = s_axis_c;
    let hs_fwd = k_fwd.cross(es_fwd);
    let ep_bwd = k_bwd.cross(s_axis_c).scale_real(-1.0 / (n1 * n1));
    let ep_fwd = k_fwd.cross(s_axis_c).scale_real(-1.0 / (n1 * n1));

    let inc_s = ModeFields {
        k: k_fwd,
        e: es_fwd,
        h: hs_fwd,
    };
    let inc_p = ModeFields {
        k: k_fwd,
        e: ep_fwd,
        h: k_fwd.cross(ep_fwd),
    };
    let refl_s = ModeFields {
        k: k_bwd,
        e: s_axis_c,
        h: k_bwd.cross(s_axis_c),
    };
    let refl_p = ModeFields {
        k: k_bwd,
        e: ep_bwd,
        h: k_bwd.cross(ep_bwd),
    };

    EntryIncidenceFrame {
        incident_s: tangential_components(&inc_s, frame.that, frame.s_axis),
        incident_p: tangential_components(&inc_p, frame.that, frame.s_axis),
        reflected_s: tangential_components(&refl_s, frame.that, frame.s_axis),
        reflected_p: tangential_components(&refl_p, frame.that, frame.s_axis),
    }
}

/// Solves the entry interface for both incident polarizations, returning
/// `(s_incident, p_incident)`. Equivalent to [`entry_incidence_frame`] followed by
/// [`entry_solve_pair_with_incidence`].
#[must_use]
pub(crate) fn entry_solve_pair(
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
) -> (EntryPolarizationSolution, EntryPolarizationSolution) {
    let inc = entry_incidence_frame(n1, frame);
    entry_solve_pair_with_incidence(&inc, n1, n_o, n_e, c_axis, frame)
}

/// The per-channel part of [`entry_solve_pair`]: the q-roots, the crystal-side mode fields
/// and the boundary solve, which depend on this channel's `n_o` and `n_e`. Takes an
/// [`EntryIncidenceFrame`] already built for the bounce.
#[must_use]
pub(crate) fn entry_solve_pair_with_incidence(
    inc: &EntryIncidenceFrame,
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
) -> (EntryPolarizationSolution, EntryPolarizationSolution) {
    let k_tan = n1 * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);
    let fo = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    let fe = extraordinary_mode_fields(
        n_o,
        n_e,
        c_axis,
        frame.that,
        frame.zhat,
        k_tan,
        roots.qe_plus,
    );

    let ord_c = tangential_components(&fo, frame.that, frame.s_axis);
    let ext_c = tangential_components(&fe, frame.that, frame.s_axis);

    let mut a = [[Cplx::ZERO; 4]; 4];
    let mut b_s = [Cplx::ZERO; 4];
    let mut b_p = [Cplx::ZERO; 4];
    for i in 0..4 {
        a[i] = [
            inc.reflected_s[i],
            inc.reflected_p[i],
            Cplx::ZERO.sub(ord_c[i]),
            Cplx::ZERO.sub(ext_c[i]),
        ];
        b_s[i] = Cplx::ZERO.sub(inc.incident_s[i]);
        b_p[i] = Cplx::ZERO.sub(inc.incident_p[i]);
    }
    let (sol_s, sol_p) = solve4_two_rhs(a, b_s, b_p);
    let flux_o = poynting_z(&fo, Cplx::re(1.0), frame.zhat).abs();
    let flux_e = poynting_z(&fe, Cplx::re(1.0), frame.zhat).abs();
    let o_hat = fo.e.re.normalize_or_zero();
    let e_hat = fe.e.re.normalize_or_zero();
    let s_sol = EntryPolarizationSolution {
        r_s: sol_s[0],
        r_p: sol_s[1],
        t_o: sol_s[2],
        t_e: sol_s[3],
        flux_o,
        flux_e,
        o_hat,
        e_hat,
    };
    let p_sol = EntryPolarizationSolution {
        r_s: sol_p[0],
        r_p: sol_p[1],
        t_o: sol_p[2],
        t_e: sol_p[3],
        flux_o,
        flux_e,
        o_hat,
        e_hat,
    };
    (s_sol, p_sol)
}
