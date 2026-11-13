//! Volumetric scattering (Henyey-Greenstein and oriented inclusions) and the frosted-facet BSDF.
//!
//! - [`hg`]: the Henyey-Greenstein per-bounce estimator, its phase function and sampler.
//! - [`hg_nee`]: the next-event estimate at a scattering point, with its shadow probe.
//! - [`oriented`]: the same estimator for a medium with oriented inclusions (silk, needles,
//!   platelets).
//! - [`frosted`]: the Lambertian frosted-facet BSDF kept as the GPU twin's reference, and its
//!   exterior-side next-event estimate.
//! - [`delta_tracking`]: reflection off a Bragg domain structure.
//!
//! This file holds the state the submodules share ([`NeeContext`], [`balance_heuristic`],
//! [`frosted_orthonormal_basis`]). Items used across the tree are re-exported below, so their
//! paths stay `scattering::X`.

use super::environment::EnvironmentSource;
use glam::Vec3;

// `pub`, not private: a `pub(crate)` item in a private module is only reachable through this
// file's re-export, which clippy's `redundant_pub_crate` (nursery) flags. `raytracer`'s `mod.rs`
// uses the same pattern.
pub mod delta_tracking;
#[cfg(test)]
mod delta_tracking_tests;
pub mod frosted;
pub mod hg;
pub mod hg_nee;
#[cfg(test)]
mod hg_nee_tests;
pub mod oriented;
#[cfg(test)]
mod oriented_furnace_tests;
#[cfg(test)]
mod oriented_tests;
pub mod oriented_zoned;
#[cfg(test)]
mod oriented_zoned_bragg_tests;
#[cfg(test)]
mod oriented_zoned_tests;
#[cfg(test)]
mod tests;

// hg.rs
pub(super) use hg::{ScatterStepOutcome, try_scatter_step};
pub(crate) use hg::{maybe_scatter_or_extinguish, sample_henyey_greenstein_direction};
// Used only by the GPU twin checks (`renderer::gpu::transport_check`) and tests.
#[cfg(any(test, feature = "gpu"))]
pub(crate) use hg::henyey_greenstein_phase;

// hg_nee.rs: the GPU Tier 2 harness drives the real estimator with its per-case fixed
// alphas (`ShadowAlphas::Fixed`, the standalone kernel's explicit `alphas` input).
#[cfg(feature = "gpu")]
pub(crate) use hg_nee::{ShadowAlphas, nee_contribution_hg_scatter};

// frosted.rs
#[cfg(feature = "gpu")]
pub(crate) use frosted::nee_contribution_frosted_exterior;
pub(crate) use frosted::{apply_frosted_bounce, cosine_weighted_hemisphere};

/// Most boundary crossings the Henyey-Greenstein NEE shadow probe follows on a concave
/// stone before it drops the sample. A groove or dimple costs two crossings (out and
/// back in) per exit, so four covers one cavity plus the final exit.
pub(crate) const HG_NEE_MAX_CROSSINGS: usize = 4;

/// The per-trace inputs of the next-event estimators, including their on/off switch.
///
/// The environment (HDR maps and the analytic sun have a light-sampling technique), the arena that
/// shadow rays probe, the tools, and the ground band of an abraded edge.
///
/// `enabled = false` skips every NEE draw without consuming RNG or touching an accumulator, so
/// the trace is bit for bit the one built without NEE. The struct is `Copy`, so passing it by
/// value costs no more than a reference.
///
/// `pub(crate)`: the GPU transport check builds an `enabled: false` instance directly.
#[derive(Clone, Copy)]
pub(crate) struct NeeContext<'a> {
    pub(crate) environment: EnvironmentSource<'a>,
    pub(crate) plane_soa: &'a crate::simd::PlanesSoA32,
    /// Convex volumes subtracted from the polyhedron `plane_soa` holds; empty for a
    /// planar stone, which keeps every NEE path on its old convex shortcut. With tools
    /// the shadow probes are real: the exterior NEE must find the sampled direction
    /// unoccluded, and the HG probe walks up to [`HG_NEE_MAX_CROSSINGS`] boundaries.
    pub(crate) tools: &'a [crate::geometry::tool::ToolPrimitive],
    /// `true` only at the public entry points when `environment` is `HdrMap` or the analytic
    /// `DaylightSun` (`environment_supports_nee`), or when a test forces it for an on/off
    /// comparison (as `ExitSplitCtx::enabled` does). Under the sun only the frosted exterior
    /// NEE fires: the Henyey-Greenstein estimator is HDR-only (see `nee_contribution_hg_scatter`).
    pub(crate) enabled: bool,
    /// The ground band of an abraded-edge scene (`None` for every other trace). A shadow ray
    /// that leaves through the band meets a ground surface, which the Henyey-Greenstein
    /// estimator drops like a frosted exit (see `nee_contribution_hg_scatter`).
    pub(crate) edge_band: Option<crate::optics::raytracer::surface::EdgeBand<'a>>,
}

/// Balance-heuristic MIS weight for a light-sampling technique with pdf `pdf_a` against a
/// competing (BSDF/phase) technique with pdf `pdf_b`, evaluated at a direction drawn from
/// technique `a` -- `w_a = pdf_a / (pdf_a + pdf_b)`. `0.0` if both densities are
/// non-positive (a genuinely degenerate case: e.g. an all-black environment row, or a
/// direction the phase function assigns zero density), contributing nothing rather than
/// dividing by zero. Symmetric in the sense Veach's balance heuristic requires:
/// `balance_heuristic(a, b) + balance_heuristic(b, a) == 1.0` for any `a, b > 0`.
#[must_use]
pub(super) fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    let denom = pdf_a + pdf_b;
    if denom > 1e-12 { pdf_a / denom } else { 0.0 }
}

/// A stable orthonormal basis `(t, b)` perpendicular to unit vector `n`. A local copy of the
/// branch-minimal construction in `birefringence`'s private helper of the same name.
pub(in crate::optics::raytracer) fn frosted_orthonormal_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = (a - n * n.dot(a)).normalize_or_zero();
    let b = n.cross(t);
    (t, b)
}
