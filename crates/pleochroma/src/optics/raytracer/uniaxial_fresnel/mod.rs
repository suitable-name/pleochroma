//! Exact anisotropic-interface Fresnel solution for uniaxial crystals.
//!
//! Replaces the scalar "effective index" Fresnel approximation at every
//! `is_anisotropic && !is_biaxial` interface in [`super::refraction`].
//!
//! # Physics and derivation
//!
//! At an isotropic/uniaxial interface, an incident s- or p-polarized wave does NOT
//! reflect/transmit as if the crystal had one scalar "effective index" -- it couples
//! into the ordinary (o) AND extraordinary (e) eigenmodes simultaneously, and (except
//! when the optic axis lies in the plane of incidence) the reflected wave in the
//! isotropic medium is itself a genuine s/p mixture. This module implements the
//! closed-form solution for that boundary-value problem: Lekner, J. Phys.: Condens.
//! Matter 3 (1991) 6121-6133 ("Reflection and refraction by uniaxial crystals"),
//! cross-checked against Yeh, J. Opt. Soc. Am. 69 (1979) 742 and Yeh, "Optical Waves in
//! Layered Media" (1988) ch. 9 -- both derive the same boundary-matched amplitudes for
//! a uniaxial half-space; this module solves the identical 4-equation boundary system
//! Lekner's `r_ss, r_sp, r_ps, r_pp, t_so, t_se, t_po, t_pe` name, via the SAME
//! closed-form uniaxial dispersion relation (not a general numerical Berreman 4x4
//! eigenvalue solve -- the ordinary root is a plain square root, the extraordinary root
//! solves an explicit quadratic, both closed-form; see [`uniaxial_q_roots`]).
//!
//! ## Units and frame convention
//!
//! Wavevectors are measured in units of `k0 = omega/c`, so a wavevector's magnitude
//! equals the local refractive index directly (`|k| = n`), and the universal relation
//! `H = k x E` holds in ANY medium (isotropic or anisotropic alike) given Maxwell's
//! equations `curl E = i*omega*mu0*H`, `curl H = -i*omega*eps0*eps*E` with `mu0 = eps0 =
//! c = 1` -- see each function's own doc comment. All geometry is built directly from
//! the SAME world-space `k_hat` (wave normal) / `normal` (facet normal, oriented to
//! face the incident ray, i.e. `(-k_hat).dot(normal) == cos_i >= 0` -- exactly
//! `compute_bounce_refraction_geometry`'s own convention) / `c_axis` vectors the rest of
//! this crate already uses, projected onto the local orthonormal frame `(that, s_axis,
//! zhat)`:
//!   - `zhat = -normal` (forward propagation direction, incidence medium -> far medium)
//!   - `that` = the in-plane (tangential) component of `k_hat`, i.e. the propagation
//!     azimuth
//!   - `s_axis = k_hat.cross(normal).normalize()` -- IDENTICAL to
//!     `rotate_stokes_to_plane_of_incidence`'s `current_plane_normal` (see
//!     `absorption::rotate_stokes_to_plane_of_incidence`'s doc comment), so a caller's
//!     already-`current_plane_normal`-referenced Stokes vector needs no extra rotation
//!     to feed into this module's Jones/Mueller machinery, and this module's own output
//!     Mueller matrices apply directly in that same frame.
//!
//! `(alpha, beta, gamma) = (c_axis.dot(that), c_axis.dot(s_axis), c_axis.dot(zhat))` are
//! the optic-axis direction cosines Lekner's own notation uses.
//!
//! ## Validation
//!
//! The closed form is checked against R + T == 1 (Poynting-projected) at both the entry and
//! the internal/exit interface, including TIR; against the scalar Fresnel formulas in the
//! isotropic limit `n_o == n_e`; and against normal incidence with the optic axis in the
//! surface plane. See the tests in this module tree.
//!
//! ## Wiring
//!
//! - **Entry** (`entry_solve_pair`, `entry_incidence_frame`,
//!   `entry_solve_pair_with_incidence`): used by `refraction::apply_uniaxial_entry_bounce`.
//! - **Internal reflection and exit transmission** (`internal_solve`): used by
//!   `refraction::apply_tir_bounce` (hero-forced TIR) and by
//!   `refraction::apply_uniaxial_internal_bounce` (sub-critical path). The scalar Fresnel
//!   path in `apply_partial_fresnel_bounce` serves isotropic and biaxial materials and the
//!   degenerate wave normal parallel to the optic axis.
//! - **o<->e re-coupling** (`apply_internal_mode_coupling` in `transport::bounce`): takes the
//!   Poynting-weighted `R_o/(R_o+R_e)` split from the same `internal_solve` result. A
//!   polarization projection remains the fallback for biaxial materials.
//! - **Performance**: `EntryIncidenceFrame` shares the incidence-side data, which does not
//!   depend on `n_o(lambda)`/`n_e(lambda)`, across the hero channels of one bounce.
//!   [`uniaxial_q_roots`] returns early when `n_o == n_e`, and
//!   `apply_uniaxial_internal_bounce` reuses the hero channel's `internal_solve` result.
//! - **WGSL mirror**: `renderer/shaders/transport_physics/06_uniaxial_fullwave_complex.wgsl`
//!   and `07_uniaxial_fullwave_solve.wgsl` port this module op for op. GPU parity is checked
//!   by `renderer::gpu::transport_check::p2_uniaxial_fresnel`.
//!
//! The module is split by seam: [`complex`] holds the `Cplx`/`CVec3` arithmetic, [`frame`]
//! the incidence frame, the closed-form mode fields and the Poynting flux, [`linalg`] the
//! shared 4x4 boundary solve, [`entry`] and [`internal`] the two interfaces, and [`mueller`]
//! the Stokes/Mueller integration. Items used outside the module are re-exported below.

// `pub` for the submodules that hold `pub(crate)` items: a `pub(crate)` item in a private
// module trips clippy's `redundant_pub_crate`. `linalg` holds only `pub(super)` items, so
// it stays private.
pub mod complex;
pub mod entry;
pub mod frame;
pub mod internal;
mod linalg;
pub mod mueller;
#[cfg(test)]
mod tests;

// complex.rs
// Used only by the GPU twin checks (`renderer::gpu::transport_check`) and tests.
#[cfg(feature = "gpu")]
pub(crate) use complex::Cplx;

// frame.rs
pub(crate) use frame::UniaxialFrame;

// entry.rs
#[cfg(feature = "gpu")]
pub(crate) use entry::EntryPolarizationSolution;
#[cfg(any(test, feature = "gpu"))]
pub(crate) use entry::entry_solve_pair;
pub(crate) use entry::{
    EntryIncidenceFrame, entry_incidence_frame, entry_solve_pair_with_incidence,
};

// internal.rs
pub(crate) use internal::{InternalPolarizationSolution, internal_solve};

// mueller.rs
pub(crate) use mueller::{azimuth2_in_frame, jones_to_mueller, mode_power};
