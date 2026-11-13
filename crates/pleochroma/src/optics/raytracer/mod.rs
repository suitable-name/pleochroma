//! The spectral path tracer.
//!
//! Camera/ray generation, polyhedron intersection, Fresnel/TIR refraction,
//! Henyey-Greenstein scattering, pleochroic absorption, environment sampling,
//! low-discrepancy sampling, and spectral-to-tristimulus color conversion.
//!
//! This file is a pure re-export hub. Each submodule's own doc comment says what it owns.
//! Every path `pleochroma::optics::raytracer::X` resolves here through the `pub use` and
//! `pub(crate) use` lines below, and submodules reach each other's `pub(super)` items directly.

// `pub` modules: a `pub(crate)` item in a private module would be flagged by clippy's
// `redundant_pub_crate` (nursery). Making the module `pub` keeps every item's own visibility.
pub mod absorption;
pub mod camera;
pub mod color;
pub mod environment;
mod estimator;
pub mod intersect;
pub mod intersect_stone;
pub mod refraction;
pub mod sampling;
pub mod scattering;
pub mod surface;
pub mod transport;
pub mod uniaxial_fresnel;
mod zoned;
#[cfg(test)]
mod zoned_tests;

/// Number of spectral channels `trace_spectral_ray` traces per ray (8-channel stratified
/// hero-wavelength sampling). Module-level so the per-bounce helpers can share it.
/// Crate-visible for the beam renderer's birefringence split (`renderer::beam::birefringence`).
pub(crate) const NUM_CHANNELS: usize = 8;

/// The bounce cap every front end starts a fresh session with.
///
/// The desktop's settings and live viewport, its batch and compare engines and the web
/// app all seed their `max_bounces` from it.
pub const DEFAULT_MAX_BOUNCES: u32 = 12;

// Re-exports keep the `raytracer::X` paths stable. `pub(crate)` ones carry
// `#[allow(unused_imports)]`: the path must resolve for consumers this build may not compile
// (feature="gpu" harnesses, crates/pleochroma/tests/), though submodules never go through this hub.

// camera.rs
pub use camera::{Camera, DEFAULT_FOV_DEG, DEFAULT_POSE, FacetFinish, HitRecord, OrbitPose, Ray};

// estimator.rs
pub use estimator::{Estimator, default_estimator, set_default_estimator};

// intersect.rs
pub use intersect::intersect_polyhedron;
// `pub`: a batch caller (e.g. `examples/pgo_train/`) builds the `PlanesSoA32` arena once and
// drives `trace_spectral_ray_with_finish_soa` with it instead of rebuilding it per sample.
pub use intersect::build_plane_soa;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::transport_check's \
              Tier 2 harness (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use intersect::{intersect_polyhedron_soa, shading_normal_near_edge};

// intersect_stone.rs
pub use intersect_stone::intersect_stone;

// sampling.rs
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::rng_check's Phase 0 \
              self-test (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use sampling::{
    BIREFRINGENT_SPLIT_STREAM, DISTANCE_SAMPLE_STREAM, FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM,
    FROSTED_DIR_V_STREAM, MODE_COUPLING_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM,
    RUSSIAN_ROULETTE_STREAM,
};
pub use sampling::{
    HERO_WAVELENGTH_ROTATION_STREAM, PIXEL_JITTER_X_ROTATION_STREAM,
    PIXEL_JITTER_Y_ROTATION_STREAM, PixelRotations, SampleDraws, add_finite_sample,
    cranley_patterson_rotate, hash_u32, low_discrepancy_base2, pixel_rotations,
    radical_inverse_base, sample_draws,
};

pub use environment::{
    BACKDROP_GREY, DEFAULT_HEAD_SHADOW_COSINES, DEFAULT_HEAD_SHADOW_DEG, EnvironmentSource,
    LightingModel, LightingPreset, LightingRigParams, METERED_PAPER_RADIANCE, TRAY_ALBEDO_BLACK,
    TRAY_ALBEDO_GREY, TRAY_ALBEDO_WHITE, TRAY_MODEL_VERSION, TentParams, blackbody_spectrum,
    hdr_rgb_at_exposure, head_shadow_cosines, metered_exposure, rig_irradiance_over_pi,
    sample_studio_environment, sample_studio_environment_observed,
    sample_studio_environment_with_rig, sample_studio_environment_with_rig_shadow,
    sample_studio_environment_with_rig_shadow_tray, sample_studio_environment_with_rig_spec_power,
    sample_studio_environment_with_rig_spec_power_tray, studio_spec_power, tray_radiance,
};
#[cfg(feature = "gpu")]
pub(crate) use environment::{
    sun_nee_reference_factor, sun_nee_reference_pdf, sun_nee_reference_sample,
};

// color.rs
pub use color::{aces_tonemap, cie_1931_cmf, xyz_to_rgb_in_space, xyz_to_srgb_gamma};
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for crates/pleochroma/tests/ and same-crate \
              callers outside this module tree"
)]
pub(crate) use color::{
    apply_von_kries_white_balance, compute_illuminant_white_balance, illuminant_temperature_k,
    integrate_channels_to_xyz, integrate_channels_to_xyz_families, spectral_mis_weight,
};

// absorption.rs
pub use absorption::spectral_absorption;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for same-crate callers outside this \
              module tree"
)]
pub(crate) use absorption::{channel_absorption_alphas_assigned, signed_frame_rotation_psi};
// `channel_absorption_alphas` is used only by this crate's tests; the interior transport
// path uses `channel_absorption_alphas_assigned`. It needs no `feature = "gpu"` gate, unlike
// `henyey_greenstein_phase` just below.
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::channel_absorption_alphas path; only \
              named by this crate's own tests"
)]
pub(crate) use absorption::channel_absorption_alphas;

// refraction.rs
#[cfg(feature = "gpu")]
pub(crate) use refraction::per_channel_uniaxial_indices;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for same-crate callers outside this \
              module tree"
)]
pub(crate) use refraction::{
    BounceRefractionGeometry, RayMaterialContext, theta_c_for_bounce, tir_phase_delta,
};

// scattering.rs
#[cfg(any(test, feature = "gpu"))]
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::henyey_greenstein_phase path; only named by \
              this crate's own tests or the gpu-feature Tier 2 harness, not always both"
)]
pub(crate) use scattering::henyey_greenstein_phase;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::transport_check's \
              Tier 2 harness (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use scattering::{
    apply_frosted_bounce, cosine_weighted_hemisphere, maybe_scatter_or_extinguish,
    sample_henyey_greenstein_direction,
};

// Re-export of `scattering::balance_heuristic`, used by `renderer::gpu::transport_check`'s
// Tier 2 harness (`nee.rs`), which needs `feature = "gpu"`.
#[cfg(feature = "gpu")]
#[inline]
#[must_use]
pub(crate) fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    scattering::balance_heuristic(pdf_a, pdf_b)
}

// transport.rs
pub use transport::{
    ExitEvent, PathEvents, PathTermination, trace_spectral_ray,
    trace_spectral_ray_attributed_soa_geom, trace_spectral_ray_attributed_soa_geom_surface,
    trace_spectral_ray_geom, trace_spectral_ray_with_finish,
    trace_spectral_ray_with_finish_instrumented, trace_spectral_ray_with_finish_soa,
    trace_spectral_ray_with_finish_soa_geom, trace_spectral_ray_with_finish_soa_geom_surface,
    wrapped_hero_wavelengths,
};
