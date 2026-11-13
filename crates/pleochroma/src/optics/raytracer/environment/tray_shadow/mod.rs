//! Spectral light received by a horizontal tray touching the stone's culet.
//!
//! Each factor is irradiance with the stone divided by open-tray irradiance,
//! `T(p, λ) = E_with / E_open`. Every preset has one illuminant spectrum that cancels in this
//! ratio, so `T` depends only on the stone, its material and the light geometry; exposure,
//! paper albedo, head shadow, glare and the camera do not enter it. It splits as
//! `T = 1 - B + C`:
//!
//! - **Light tables.** The studio's radiance over the 32 768 hemisphere cells gives a gather
//!   table (weights include the cosine to the tray, so its total is the open irradiance) and an
//!   emission table (radiance alone). Daylight Sun adds one sun cell drawn inside the sun disc.
//!   A sampled cell is jittered uniformly inside its cone.
//! - **Blocked fraction `B`.** Per texel, stratified inverse-CDF samples of the gather table
//!   (with a per-texel Cranley-Patterson offset) are traced towards the light; `B` is the share
//!   that meets the bounding sphere and then the stone, tools included. It is the same for
//!   every band.
//! - **Redirected fraction `C`.** Forward photons drawn from the emission table cross a disc
//!   the size of the bounding sphere, enter the stone, and follow reflections and refractions
//!   with unbiased per-band Fresnel branching, absorption from the Ray-Traced model's binned
//!   absorption, and Russian roulette after eight bounces. The internal path follows the hero
//!   band's index (photon index modulo 20); each band's own direction is resolved only at the
//!   final refraction out of the stone (the exit-split approximation of the path tracer).
//!   Each band that leaves heading down lands on the tray and is splatted bilinearly in
//!   double precision.
//! - **Filter and fade.** Only `C` is filtered, by a separable Gaussian (sigma 0.75 texel,
//!   radius 2, clamped at the edges), so geometric shadow edges stay sharp. The build bakes a
//!   smoothstep border fade towards one into the texels; lookups interpolate texel and band
//!   centres with plain multiplies and adds that the GPU lookup repeats operation for operation.
//! - **Approximations.**A scattering material deposits nothing, so its shadow is
//!   opaque; frosted facets and edge rounding are treated as polished and sharp; zoned
//!   materials use the base absorption; there is no interreflection with the tray.
//! - **Determinism.**Seeds depend only on the texel or photon-chunk index, and photon
//!   chunks are reduced in chunk order, so the map's bits do not depend on the thread count.
//!
//! The map and its packing are available on every target, independently of precomputation.

use std::sync::Arc;

mod lookup;
mod pack;
#[cfg(test)]
mod tests_lookup;

pub use pack::{
    TRAY_SHADOW_BINDING, pack_disabled_gpu_words, pack_gpu_words, pack_gpu_words_with_mirror,
    pack_whitted_rgb,
};

/// Number of 20 nm bands spanning 380–780 nm.
pub const TRAY_SHADOW_BANDS: usize = 20;
/// Centre wavelength of the first band, in nanometres.
pub const TRAY_SHADOW_FIRST_CENTER_NM: f32 = 390.0;
/// Wavelength spacing between band centres, in nanometres.
pub const TRAY_SHADOW_BAND_WIDTH_NM: f32 = 20.0;
/// Bumped when the map's physics changes; part of every key.
pub const TRAY_SHADOW_MODEL_VERSION: u32 = 1;

/// Sampling budget for a live view or a final image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrayShadowQuality {
    /// A background-built map for live viewing.
    Preview,
    /// A higher-resolution map for exports and jobs.
    Final,
}

impl TrayShadowQuality {
    /// Number of texels along each side of the map.
    #[must_use]
    pub const fn resolution(self) -> u32 {
        match self {
            Self::Preview => 256,
            Self::Final => 512,
        }
    }

    /// Number of incident directions gathered at each texel.
    #[must_use]
    pub const fn gather_directions(self) -> u32 {
        match self {
            Self::Preview => 256,
            Self::Final => 1024,
        }
    }

    /// Number of forward photons emitted for the redirected term.
    #[must_use]
    pub const fn photons(self) -> u32 {
        match self {
            Self::Preview => 1_500_000,
            Self::Final => 12_000_000,
        }
    }
}

/// Identity of the stone, material, light geometry and precompute settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrayShadowKey(pub u64);

/// Why a requested stone shadow cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayShadowOff {
    /// The scene has no lit backdrop.
    AsLit,
    /// A panorama has no supported analytic light table.
    Hdr,
    /// The contrast light is outside the shadow model.
    Aset,
    /// The ultraviolet light is outside the shadow model.
    UvLamp,
    /// The geometry does not bound a usable stone.
    DegenerateStone,
    /// The caller cancelled the precompute.
    Cancelled,
}

impl TrayShadowOff {
    /// Sentence shown when the requested shadow is unavailable.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::AsLit | Self::Hdr | Self::Aset | Self::UvLamp => {
                "Stone shadow needs a studio light with a backdrop."
            }
            Self::DegenerateStone => "Stone shadow needs a closed, nondegenerate stone.",
            Self::Cancelled => "Computing the stone shadow was cancelled.",
        }
    }
}

/// Irradiance ratios on a square tray, with the border fade already applied.
///
/// Cloning shares the immutable texel allocations, so camera and GPU preparation snapshots
/// retain a map without copying its spectral bands.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayShadowMap {
    plane_y: f32,
    center: [f32; 2],
    half_extent: f32,
    resolution: u32,
    /// Texel-major, band-fastest irradiance ratios.
    factors: Arc<Vec<f32>>,
    /// Per-texel linear-sRGB ratios for the Ray-Traced engine.
    factors_rgb: Arc<Vec<[f32; 3]>>,
    key: TrayShadowKey,
}

impl TrayShadowMap {
    /// Constructs a map from already faded ratios. Built with the `stone-shadow` precompute and in
    /// tests, its only callers; the checks on its storage and geometry are debug assertions, so a
    /// render thread never panics in release. The precompute declines nonfinite data before it
    /// calls this.
    #[cfg(any(test, feature = "stone-shadow"))]
    #[must_use]
    pub(crate) fn from_parts(
        plane_y: f32,
        center: [f32; 2],
        half_extent: f32,
        resolution: u32,
        factors: Vec<f32>,
        factors_rgb: Vec<[f32; 3]>,
        key: TrayShadowKey,
    ) -> Self {
        debug_assert!(plane_y.is_finite() && center.iter().all(|x| x.is_finite()));
        debug_assert!(half_extent.is_finite() && half_extent > 0.0 && resolution > 0);
        let side = usize::try_from(resolution).unwrap_or(usize::MAX);
        let texels = side.saturating_mul(side);
        debug_assert_eq!(factors.len(), texels.saturating_mul(TRAY_SHADOW_BANDS));
        debug_assert_eq!(factors_rgb.len(), texels);
        debug_assert!(factors.iter().all(|x| x.is_finite() && *x >= 0.0));
        debug_assert!(factors_rgb.iter().flatten().all(|x| x.is_finite()));
        Self {
            plane_y,
            center,
            half_extent,
            resolution,
            factors: Arc::new(factors),
            factors_rgb: Arc::new(factors_rgb),
            key,
        }
    }

    /// Height of the horizontal tray plane.
    #[must_use]
    pub const fn plane_y(&self) -> f32 {
        self.plane_y
    }
    /// World-space x/z coordinates at the map centre.
    #[must_use]
    pub const fn center(&self) -> [f32; 2] {
        self.center
    }
    /// World-space half-width of the square map.
    #[must_use]
    pub const fn half_extent(&self) -> f32 {
        self.half_extent
    }
    /// Number of texels along each side.
    #[must_use]
    pub const fn resolution(&self) -> u32 {
        self.resolution
    }
    /// Texel-major, band-fastest spectral irradiance ratios.
    #[must_use]
    pub fn factors(&self) -> &[f32] {
        self.factors.as_slice()
    }
    /// Texel-major linear-sRGB irradiance ratios.
    #[must_use]
    pub fn factors_rgb(&self) -> &[[f32; 3]] {
        self.factors_rgb.as_slice()
    }
    /// Identity used by CPU and GPU upload caches.
    #[must_use]
    pub const fn key(&self) -> TrayShadowKey {
        self.key
    }
}

#[cfg(feature = "stone-shadow")]
mod build;
#[cfg(feature = "stone-shadow")]
mod cache;
#[cfg(feature = "stone-shadow")]
mod gather;
#[cfg(feature = "stone-shadow")]
mod geometry;
#[cfg(feature = "stone-shadow")]
mod lights;
#[cfg(feature = "stone-shadow")]
mod photons;
#[cfg(feature = "stone-shadow")]
mod rng;

#[cfg(feature = "stone-shadow")]
pub use build::{
    TrayShadowInput, build_tray_shadow, build_tray_shadow_with_threads, tray_shadow_key,
};
#[cfg(feature = "stone-shadow")]
pub use cache::{shared_tray_shadow, shared_tray_shadow_with_threads};
#[cfg(all(test, feature = "stone-shadow"))]
mod tests_build;
#[cfg(all(test, feature = "stone-shadow"))]
mod tests_physics;
#[cfg(all(test, feature = "stone-shadow"))]
mod tests_plane;
