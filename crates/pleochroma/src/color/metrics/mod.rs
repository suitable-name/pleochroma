//! GIA/AGSL-style optical gemological metrics (brilliance, fire,
//! scintillation, windowing, extinction).
//!
//! Evaluated by firing an analytical ray fan through a stone's facet geometry from the
//! observer's point of view, under the lighting the renderer itself uses.
//!
//! # What is measured
//!
//! Each evaluation traces an 18x18 grid of rays (the cells inside a disc, see below), each
//! with a five-ray sub-aperture bundle, at the d-line. Every ray is classified once:
//!
//! - **Windowing**: the ray leaks out through the pavilion instead of returning.
//! - **Brilliance**: the ray leaves upward through the crown and is *visibly returned* --
//!   outside the observer's 16 degree head-shadow cone and looking at a light source (see
//!   the lighting model below).
//! - **Extinction**: every other ray -- trapped, absorbed, head-shadowed or looking at the
//!   dim surround. The three percentages are shares of all the rays that hit the stone;
//!   a ray blocked at entry counts in the total but in none of the three.
//! - **Fire**: for each returned ray, the angle between its F-line and C-line exit
//!   directions, weighted by the product of the three wavelengths' Fresnel transmittances
//!   and the two exit cosines, summed over the rays and divided by the total rays. Only
//!   pairs that exit through the same facet after the same number of bounces count.
//! - **Scintillation**: the spatial contrast (coefficient of variation) of the per-cell
//!   return fraction, squashed into 0-100, blended 60/40 with the temporal flicker of the
//!   cells' return status as the camera yaws through +/-3 degrees.
//!
//! # Birefringent stones and Doubling
//!
//! For a material the renderer treats as anisotropic (not cubic and `|delta n| > 1e-4`, so
//! sapphire and tourmaline as much as calcite) every ray of the grid is traced as BOTH eigenmodes
//! of the crystal, each carrying half of it (private `birefringent` module): the ordinary
//! mode exactly as an isotropic ray, the extraordinary one with its effective index and its
//! Poynting walk-off (a biaxial crystal: its fast and slow modes), the mode kept at every
//! interior hit. Brilliance, windowing, extinction, Fire, the tone and the scintillation
//! cells then take those weights instead of the integer counts (private `doubling` module).
//! An isotropic material keeps the integer path bit for bit. The cost is twice the traces on an
//! anisotropic material, nothing on an isotropic one.
//!
//! [`GemOpticalMetrics::doubling_arcmin`] is the one new figure: the mean angular separation,
//! in arc-minutes, of the two modes' exit directions over the ray pairs that leave through the
//! same facet, weighted like Fire. It is `0.0` for an isotropic stone. The image metrics
//! ([`evaluate_optical_image`], the image scintillation) still trace the ordinary mode only.
//! The crystal axis is the material's own `c_axis`, which the callers set from the stone's
//! orientation (`render_setup::apply_material_overrides`).
//!
//! # Face-up tone
//!
//! [`evaluate_gem_optical_metrics_with_tone`] adds the colour of the light the stone returns
//! to the observer ([`FaceUpTone`]), from the same grid pass. The returned rays' Fresnel
//! weights are binned by internal path length (128 bins over 12 girdle half-widths), and the
//! transmittance is the Beer-Lambert *mixture* over those paths, not Beer-Lambert of the mean
//! path; the pose is meant to be table-up. The colorimetry runs under the lighting preset's
//! own spectrum and white point ([`ToneIlluminant`]): Lab is relative to that white, so a
//! colourless stone is neutral under every preset, and the sRGB swatch is adapted to the
//! screen's D65 the way the Live Render white-balances that preset. UV lamps and HDR maps
//! have no usable white point and fall back to D65. The stone's size enters only through
//! `GemMaterial::absorption_path_scale` in the exponent. A head-shadow setting (a separate
//! lighting plan may add one) enters only the Returned classification, never the tone's
//! colorimetry.
//!
//! # Lighting model
//!
//! The metrics take the same [`EnvironmentSource`](crate::optics::raytracer::EnvironmentSource)
//! the tracer renders with, so a preset or light change moves the numbers the way it moves
//! the picture. A ray counts as looking at
//! a light source when the environment's radiance along its exit direction exceeds a
//! multiple of the environment's ambient level (the median radiance over the upper
//! hemisphere): for the four studio presets that is the very key softbox, fill softbox and
//! ring pinpoint lobes the tracer evaluates, for the light tent its overhead softbox and
//! spark light, for the daylight dome its sun, for an HDR panorama its bright regions.
//! The ISO hemisphere is uniformly radiant, so every direction it radiates into counts.
//! The observer's head-shadow cone is applied on top, separately. The details and their
//! edge cases are on the private `lighting` module.
//!
//! # Facet finishes
//!
//! Every face-up entry point has a `_finished` twin that takes a [`FinishedStone`]: the planes
//! and tools plus a per-facet finish list. The twins are the pose metrics
//! ([`evaluate_gem_optical_metrics_finished`], its `_core` and `_with_tone` forms), the 19-point
//! angular profile ([`evaluate_angular_profile_finished`]), the cache
//! ([`compute_or_reuse_metrics_finished`], [`MetricsCacheKey::new_finished`]), the tilt sweeps
//! ([`evaluate_all_axes_profiles_finished_threaded`],
//! [`evaluate_full_axis_profile_at_azimuth_finished_threaded`] and the stepped variants) and the
//! diagnostic image ([`OpticalImageRequest::finishes`]).
//!
//! A frosted facet scatters the way the render tracer's frosted bounce does (a broadband Fresnel
//! reflect/transmit split and a cosine-weighted direction, see the private `frosted` module), so
//! a frosted design's figures differ from its polished ones. An empty list, or one with no
//! frosted facet, is the polished stone: the plain-geometry entry points are exactly the twins
//! called with [`FinishedStone::polished`], bit for bit. The cache key carries the finishes
//! ([`finishes_hash`], `0` when nothing is frosted), so a frosting edit recomputes the pose and
//! the profile while a polished stone keys as it always did.
//!
//! A frosted figure is a Monte Carlo estimate over the fan's rays. It is reproducible (each
//! draw is seeded from its ray, independent of thread count) but not smooth: a tiny change of a
//! facet angle re-rolls the draws. Fire and Scintillation draw independently in each of the
//! temporal sub-poses, so they carry that noise too. Only the frosted finish is traced; the
//! Ground Fine and Coarse surface grades stay polished here (the metrics tracer has a single
//! roughness).
//!
//! # Fan scale
//!
//! The grid is scaled to the stone: the sampled disc reaches 95% of the measured girdle
//! half-width ([`render_setup::measure_model_width`] halved; 1.0 model unit when the
//! design does not measure), and the rays start two and a half half-widths from the
//! stone's centre. An elongated outline is covered along its short axis only. See the
//! private `fan` module.
//!
//! [`render_setup::measure_model_width`]: crate::render_setup::measure_model_width
//!
//! # Layout
//!
//! Split by responsibility: [`camera`] (the shared observer-PoV basis, read off the render
//! camera), [`fan`] (where the rays start), [`lighting`] (the illumination model),
//! [`types`] (the [`GemOpticalMetrics`] result and the 19-point elevation grid),
//! [`ray_trace`] (the single-wavelength refract-then-bounce physics core), [`visibility`]
//! (the head-shadow/illumination "is this ray visibly returned" test), [`scintillation`]
//! (the spatial+temporal sparkle terms), [`classify`] (per-aperture-sample ray
//! classification and the Fire bifurcation gate), [`evaluate`] (the main grid loop,
//! [`evaluate_gem_optical_metrics`]), [`profile`] (the angular-profile sweeps built on top
//! of it), [`sweep`] (the four-axis tilt sweep) and [`cache`] (reuse of the results while
//! their inputs, lighting included, do not change).

mod accumulate;
mod attribution;
mod birefringent;
#[cfg(test)]
mod birefringent_tests;
mod cache;
mod camera;
mod classify;
mod doubling;
mod evaluate;
mod fan;
mod finished;
#[cfg(test)]
mod finished_api_tests;
mod frosted;
#[cfg(test)]
mod frosted_tests;
mod image;
mod image_scint;
mod image_types;
mod lighting;
pub use attribution::{
    AttributionLighting, SPECTRAL_ATTRIBUTION_VERSION, SpectralClass, classify_path,
};
pub use image::{evaluate_optical_image, evaluate_optical_image_threaded};
pub use image_scint::{
    SCINTILLATION_WINDOW_RADIUS, scintillation_index, scintillation_index_at,
    scintillation_spatial, scintillation_spatial_at,
};
pub use image_types::*;
#[cfg(test)]
mod pleochroic_tone_tests;
mod pose_batch;
mod prepared;
mod profile;
mod ray_trace;
mod scintillation;
mod sweep;
mod tone;
mod types;
mod visibility;

pub use cache::{
    MetricsCache, MetricsCacheKey, compute_or_reuse_metrics, compute_or_reuse_metrics_finished,
    compute_or_reuse_metrics_finished_cancellable, compute_or_reuse_metrics_geom,
    compute_or_reuse_metrics_geom_cancellable, compute_or_reuse_pose_metrics,
    compute_or_reuse_pose_metrics_finished, compute_or_reuse_pose_metrics_geom, finishes_hash,
};
pub use camera::camera_view_basis;
pub use evaluate::{
    evaluate_gem_optical_metrics, evaluate_gem_optical_metrics_finished,
    evaluate_gem_optical_metrics_finished_core, evaluate_gem_optical_metrics_geom,
    evaluate_gem_optical_metrics_geom_core, evaluate_gem_optical_metrics_with_tone,
    evaluate_gem_optical_metrics_with_tone_finished,
    evaluate_gem_optical_metrics_with_tone_finished_core,
    evaluate_gem_optical_metrics_with_tone_geom, evaluate_gem_optical_metrics_with_tone_geom_core,
};
pub use finished::FinishedStone;
pub use profile::{
    EVALUATIONS_PER_AXIS, PROFILE_AZIMUTHS_DEG, TILT_ANGLES_DEG, evaluate_angular_profile,
    evaluate_angular_profile_at_azimuth, evaluate_angular_profile_at_azimuth_finished,
    evaluate_angular_profile_at_azimuth_finished_stepped, evaluate_angular_profile_at_azimuth_geom,
    evaluate_angular_profile_at_azimuth_geom_stepped, evaluate_angular_profile_finished,
    evaluate_angular_profile_finished_stepped, evaluate_angular_profile_geom,
    evaluate_angular_profile_geom_stepped, evaluate_full_axis_profile_at_azimuth,
    evaluate_full_axis_profile_at_azimuth_finished_threaded,
    evaluate_full_axis_profile_at_azimuth_geom,
    evaluate_full_axis_profile_at_azimuth_geom_threaded,
    evaluate_full_axis_profile_at_azimuth_stepped,
    evaluate_full_axis_profile_at_azimuth_stepped_finished_threaded,
    evaluate_full_axis_profile_at_azimuth_stepped_geom,
    evaluate_full_axis_profile_at_azimuth_stepped_geom_threaded,
};
pub use sweep::{
    AxisProfile, SweepProgress, evaluate_all_axes_profiles,
    evaluate_all_axes_profiles_finished_threaded, evaluate_all_axes_profiles_geom,
    evaluate_all_axes_profiles_geom_threaded, evaluate_all_axes_profiles_stepped,
    evaluate_all_axes_profiles_stepped_finished_threaded, evaluate_all_axes_profiles_stepped_geom,
    evaluate_all_axes_profiles_stepped_geom_threaded, total_evaluations,
};
pub use tone::{FaceUpTone, ToneIlluminant};
pub use types::{GemOpticalMetrics, PROFILE_ANGLES_DEG};
