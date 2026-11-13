//! Fresnel reflection/transmission and Total Internal Reflection.
//!
//! The per-bounce refractive-index/incidence-angle geometry ([`RayMaterialContext`],
//! [`BounceRefractionGeometry`]) and the TIR/partial-reflect/refract bounce appliers
//! built on top of it.
//!
//! ## Uniaxial dispatch
//!
//! Every uniaxial (`is_anisotropic && !is_biaxial`, [`BounceRefractionGeometry::
//! uniaxial_frame`] `Some`) bounce runs the exact closed-form [`uniaxial_fresnel`]
//! solve rather than a scalar "effective index" approximation: [`apply_uniaxial_entry_bounce`]
//! at an isotropic-air -> crystal entry, [`apply_tir_bounce`]'s own branch at a
//! hero-forced (`sin2_t > 1.0`) internal reflection, and [`apply_uniaxial_internal_bounce`]
//! at every other internal event -- the general sub-critical partial reflection and
//! the uniaxial -> isotropic exit transmission, both from one
//! [`uniaxial_fresnel::internal_solve`] call per channel. [`apply_partial_reflect_bounce`]/
//! [`apply_refract_bounce`]/[`apply_refract_channel`]'s scalar-Fresnel machinery is
//! reached only by an isotropic material, a biaxial material, or the degenerate
//! wave-normal-parallel-to-optic-axis limit (see [`apply_partial_fresnel_bounce`] for
//! where each guard sits).
//!
//! ## Exit-event spectral splitting
//!
//! Implemented in [`apply_refract_channel`] (isotropic/biaxial exit, and the degenerate
//! uniaxial wave-normal-parallel-to-optic-axis case sharing its scalar machinery) and
//! in [`apply_uniaxial_internal_transmit_channels`] (the exact uniaxial exit);
//! [`try_split_exit_channel`] is the bounded fan-out primitive both share, and
//! [`ExitSplitCtx`] carries the per-trace state. Gated on [`ExitSplitCtx::enabled`];
//! `false` gives plain chromatic termination, bit for bit.
//!
//! **Interior path unchanged.** Every bounce up to, but not including, the one where
//! the ray leaves the gem back into air is still one shared, hero-driven geometric
//! path. A channel whose own refracted direction at an interior dispersive event (an
//! entry into the gem) diverges from the hero's beyond `DIRECTION_MATCH_COS_TOL` still
//! loses its radiance for the rest of the trace.
//!
//! **The estimator, in three parts (they only work together):**
//!
//! 1. *The exit event.* A still-alive companion `k` whose own Snell direction
//!    diverges from the hero's, at the bounce where the hero-driven path leaves the
//!    gem, gets its own Fresnel transmission at its own index instead of being zeroed
//!    (the same formula the matching-direction case always uses, factored into
//!    [`compute_channel_transmission`] / [`compute_uniaxial_exit_transmission`]), and
//!    [`try_split_exit_channel`] resolves its own continuation with exactly one extra
//!    intersection test: a deterministic environment lookup at `k`'s own wavelength
//!    along `k`'s own direction (a re-entry probe declines rather than recursing).
//!    Staged in [`ExitSplitCtx::split_radiance`] and committed into `radiance` only if
//!    the shared path itself goes on to escape, rescaled on every Russian-roulette
//!    survival exactly like `stokes` (`transport::apply_russian_roulette`) so a split
//!    contribution committed with probability `q` is compensated by the same `1/q`.
//!
//! 2. *`path_pdf` never stops accumulating for a live technique.* `path_pdf[k]` is
//!    technique k's (channel k as hero) density of having produced this geometric
//!    path. A companion that lost its radiance at an interior mismatch is still a
//!    technique that would have produced this path for every channel it remains
//!    compatible with (see 3), so its density keeps accumulating -- the mismatch site
//!    folds in the same transmit factor the matching case does, and the exit event
//!    folds its own transmit factor (`1 - r_unpol_k`, or `t_unpol_k` on the uniaxial
//!    path) into split and matching channels alike. The only genuine zero stays a
//!    genuine zero: a channel that cannot transmit at all where the hero did
//!    (`sin2_t_k > 1`) has density 0 for any path through that exit.
//!
//! 3. *Per-channel MIS families.* `color::spectral_mis_weight`'s balance heuristic
//!    `N * p_hero / sum_j p_j` is Veach's one-sample combination over the N techniques
//!    that could have produced the realized path. With splitting, every live companion
//!    contributes past the exit, and the set of techniques under which channel `c` is
//!    alive is NOT the hero's own alive set (direction tolerance keeps only channels
//!    within some spectral distance of a given hero alive, and that set shifts with
//!    the hero). Normalising `c`'s weight over the hero's set instead of its own makes
//!    weights summed over the heroes that see it exceed 1, which biases the estimate. The fix:
//!    [`ExitSplitCtx::compat`] tracks the pairwise compatibility of all channels
//!    through every interior dispersive event ([`narrow_compat`], reusing the same
//!    `direction_matches` verdict for every pair involving the hero), and
//!    `color::integrate_channels_to_xyz_families` weights channel `c` by
//!    `N * path_pdf[hero] / sum_{j in compat[c]} path_pdf[j]` -- which sums to exactly
//!    1 over `c`'s own family, making the combined estimator unbiased. The exit event
//!    never narrows a family: every technique in `c`'s family produces `c`'s
//!    (deterministic) exit path.
//!
//! **Tray-floor termination.** With a glossy tray under the stone, an escape that meets
//! the tray plane is not the end of a path: the hero follows the reflected ray (see
//! `transport::inner::bounce_body`'s `handle_miss`). The exit probe of point 1 cannot follow
//! such a bounce -- it is one extra intersection test and one environment lookup. So at an
//! exit event channel `k` takes the plain chromatic termination (`stokes[k] = 0`,
//! `path_pdf[k] = 0`, no probe, see [`exit_leaves_via_floor`]) whenever the hero's exit
//! direction or `k`'s own meets the tray plane. A channel that matches the hero's direction
//! is untouched (it rides the hero, floor bounce included). Without a mirror none of this
//! runs, bit for bit.
//!
//! The rule stays unbiased because its decision, `C(j, k) = floor(dir_j) || floor(dir_k)`
//! (the exit direction of `j` or of `k` meets the plane), is symmetric in `j` and `k`: in the
//! `j`-hero trace channel `k` is terminated exactly when, in the `k`-hero trace, channel `j`
//! is (`path_pdf[j] = 0`). So technique `j` produces `k`'s probe contribution exactly when it
//! is in `k`'s live family, the techniques outside that family carry zero density, and the
//! weights over the live family still sum to one. The GPU twin applies the same rule to
//! every mismatched exit channel.
//!
//! **Other choices do not help.** Leaving `path_pdf[k]` at its prefix value, at 0, or
//! applying the exit factor some other way does not fix the normalization of point 3.
//!
//! **Verification.** `transport::exit_splitting_tests` compares splitting on and off on
//! paired draws (same seed and hero), so the shared path noise cancels. The means agree
//! within the shared-geometry approximation: a companion rides the hero's path only while
//! its refracted direction is within `DIRECTION_MATCH_COS_TOL` of the hero's, and the split
//! estimator leans on that tolerance harder. Splitting does not raise the Y variance beyond
//! the noise of its estimate. Both arms are the same tracer, so a shared bias would not
//! show; no independent ground truth
//! is compared against. The GPU twin in `renderer/shaders/spectral_transport.wgsl`
//! implements the same three points.
//!
//! # Module layout
//!
//! By responsibility: [`context`] holds the
//! shared per-ray/per-sample/per-bounce state types, [`geometry`] builds
//! [`BounceRefractionGeometry`] each bounce, [`exit_split`] and [`wavelength_cache`]
//! are small focused helpers, [`tir`] is the Total Internal Reflection bounce,
//! [`reflect_refract`] is the scalar-Fresnel reflect/refract machinery,
//! [`uniaxial_entry`]/[`uniaxial_internal`] are the closed-form uniaxial bounces, and
//! [`dispatch`] is the top-level per-bounce entry point
//! ([`apply_partial_fresnel_bounce`]). The re-exports below give the public paths.

use super::environment::TrayMirror;

pub mod context;
mod dispatch;
mod exit_split;
#[doc(hidden)]
pub use exit_split::exit_split_truncated_energy_take;
pub mod geometry;
mod reflect_refract;
#[cfg(test)]
mod tests;
pub mod tir;
mod uniaxial_entry;
mod uniaxial_internal;
mod wavelength_cache;

/// Smallest cosine between two channels' refracted directions for them to count as the
/// same geometric path (`1 - 1e-6`, an angle of 1.414 mrad).
///
/// A channel whose own direction at a dispersive event is farther than this from the hero's
/// has diverged from the shared hero-driven path. This is the tolerance as a cosine; the test
/// itself is [`directions_match`], which evaluates it as a squared distance.
pub const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

/// The direction-match tolerance as the squared distance of two unit directions: `2e-6`
/// (`|a - b|^2 = 2 (1 - cos)`, so the cosine is [`DIRECTION_MATCH_COS_TOL`]).
///
/// The GPU kernels carry the identical constant and test (`directions_match`,
/// `shaders/transport_physics/08_exit_splitting.wgsl`).
pub const DIRECTION_MATCH_DISTANCE_SQ: f32 = 2.0e-6;

/// Whether unit directions `a` and `b` are within the direction-match tolerance.
///
/// Tested as the squared distance, not as the cosine: next to 1 an `f32` cosine is too coarse
/// to resolve `1 - 1e-6`, so a dot-product test flips on one-ULP rounding. The difference
/// vector keeps the full relative precision.
#[must_use]
pub fn directions_match(a: glam::Vec3, b: glam::Vec3) -> bool {
    (a - b).length_squared() <= DIRECTION_MATCH_DISTANCE_SQ
}

/// Whether an exit event must not split channel `k` because an escape meets the tray plane.
///
/// `true` when a glossy tray is attached (`mirror`) and either the hero's outgoing exit
/// direction `hero_dir` or channel `k`'s own `dir_k` meets the tray plane when launched from
/// `hit_point + dir * 1e-4` (the start the exit probe uses). Both directions are unit length.
/// `false` without a mirror, so nothing changes bit for bit. See the "Exit-event spectral
/// splitting" section above for why the estimator needs this.
#[must_use]
pub(in crate::optics::raytracer) fn exit_leaves_via_floor(
    mirror: Option<TrayMirror>,
    hit_point: glam::Vec3,
    hero_dir: glam::Vec3,
    dir_k: glam::Vec3,
) -> bool {
    mirror.is_some_and(|mirror| {
        mirror.hit(hit_point + hero_dir * 1e-4, hero_dir).is_some()
            || mirror.hit(hit_point + dir_k * 1e-4, dir_k).is_some()
    })
}

/// How a channel's refracted direction is matched against the hero's at a dispersive event.
///
/// Production uses [`DirectionMatch::Tolerance`] with [`DIRECTION_MATCH_DISTANCE_SQ`]. The beam
/// tracer's reference tracer uses [`DirectionMatch::Exact`]: a channel stays
/// live only while its refractive indices are bitwise the hero's, so every live channel is
/// bitwise on the hero's path and one branch signature describes the sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DirectionMatch {
    /// The squared distance between the channel's and the hero's unit direction is at most
    /// this value.
    Tolerance(f32),
    /// The channel's indices equal the hero's bit for bit; the hero channel always matches.
    Exact,
}

impl Default for DirectionMatch {
    fn default() -> Self {
        Self::Tolerance(DIRECTION_MATCH_DISTANCE_SQ)
    }
}

/// The match verdict of one channel against the hero at one dispersive event.
///
/// `Tolerance` compares the squared distance of the two directions with its bound
/// ([`directions_match`] for the default bound). `Exact` ignores the directions and compares
/// the three index slots (`idx_k`, `idx_hero`) bitwise, then the mode-dependent indices `extra`
/// names ([`ExactExtra`]); `is_hero` is `true` for the hero's own channel, which always
/// matches. The WGSL reference variant (`gpu/beam/reference_source.rs`) compares the same
/// indices at the same sites.
#[must_use]
pub(crate) fn direction_matches(
    mode: DirectionMatch,
    dir_k: glam::Vec3,
    dir_hero: glam::Vec3,
    is_hero: bool,
    idx_k: [f32; 3],
    idx_hero: [f32; 3],
    extra: ExactExtra<'_>,
) -> bool {
    match mode {
        DirectionMatch::Tolerance(max_distance_sq) => {
            (dir_k - dir_hero).length_squared() <= max_distance_sq
        }
        DirectionMatch::Exact => {
            is_hero
                || (idx_k.map(f32::to_bits) == idx_hero.map(f32::to_bits) && extra.indices_match())
        }
    }
}

/// The mode-dependent indices an `Exact` match also compares, beyond the three slots: the raw
/// extraordinary index (uniaxial e-mode) and the three principal indices (biaxial).
#[derive(Clone, Copy)]
pub(crate) struct ExactExtra<'a> {
    /// The material whose indices are compared.
    pub(crate) material: &'a crate::optics::materials::GemMaterial,
    /// The channel's wavelength and the hero's.
    pub(crate) lambdas: (f32, f32),
    /// Compare the raw extraordinary index.
    pub(crate) uniaxial_e: bool,
    /// Compare the biaxial principal indices.
    pub(crate) biaxial: bool,
}

impl ExactExtra<'_> {
    /// `true` when every named index is bitwise the hero's. Evaluated only under `Exact`.
    fn indices_match(&self) -> bool {
        let (lambda_k, lambda_hero) = self.lambdas;
        let material = self.material;
        if self.uniaxial_e {
            let n_e = |lambda: f32| {
                material.extraordinary_index_at(lambda, material.dispersion.evaluate(lambda))
            };
            if n_e(lambda_k).to_bits() != n_e(lambda_hero).to_bits() {
                return false;
            }
        }
        if self.biaxial {
            let bits = |lambda: f32| {
                material
                    .biaxial_indicatrix(lambda)
                    .map(|b| [b.n_alpha, b.n_beta, b.n_gamma].map(f32::to_bits))
            };
            if bits(lambda_k) != bits(lambda_hero) {
                return false;
            }
        }
        true
    }
}

/// Lower clamp on the per-channel unpolarised reflectance or transmittance.
///
/// It scales `path_pdf` (never divides `stokes` directly), keeping the density strictly
/// positive for every live technique.
pub const R_UNPOL_PDF_MIN: f32 = 1e-4;

/// Upper clamp paired with [`R_UNPOL_PDF_MIN`]: `1 - 1e-4`.
pub const R_UNPOL_PDF_MAX: f32 = 1.0 - 1e-4;

/// Lower clamp on the reflect-vs-transmit selection probability.
///
/// `[0.02, 0.98]` caps the `1/p` and `1/(1-p)` weights at 50x where the unclamped
/// reflectance approaches 0 or 1 at grazing incidence, without biasing the estimator
/// (the contribution is weighted by the true `R`/`T` over this same `p`).
pub const R_UNPOL_SELECT_MIN: f32 = 0.02;

/// Upper clamp paired with [`R_UNPOL_SELECT_MIN`].
pub const R_UNPOL_SELECT_MAX: f32 = 0.98;

pub(in crate::optics::raytracer) use context::{
    BounceContext, BounceRay, BounceState, ExitEvent, ExitSplitCtx, PathModeState, RngDraw,
};
pub(crate) use context::{RayMaterialContext, RayWavelengthCache};
pub(in crate::optics::raytracer) use dispatch::{
    apply_collect_bounce, apply_partial_fresnel_bounce,
};
#[cfg(feature = "gpu")]
pub(crate) use geometry::per_channel_uniaxial_indices;
pub(crate) use geometry::{
    BounceRefractionGeometry, compute_bounce_refraction_geometry, poynting_dir_for_mode,
    theta_c_for_bounce,
};
pub(in crate::optics::raytracer) use tir::apply_tir_bounce;
pub(crate) use tir::tir_phase_delta;
pub(crate) use uniaxial_entry::entry_eigenmode_selection;
pub(in crate::optics::raytracer) use wavelength_cache::{
    build_ray_wavelength_cache, build_ray_wavelength_cache_prepared,
    build_ray_wavelength_cache_tabled,
};
