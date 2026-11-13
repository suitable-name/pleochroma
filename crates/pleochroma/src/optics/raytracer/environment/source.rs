//! [`EnvironmentSource`] (the analytic studio rig or an HDR panorama), the fixed backdrop
//! level and the white-balance lookup. Re-exported from the parent `environment` module.

use super::{
    super::color::illuminant_white_balance, DEFAULT_HEAD_SHADOW_DEG, LightingPreset,
    clamp_head_shadow_deg, tray_mirror::TrayMirror, tray_shadow::TrayShadowMap,
};
use crate::renderer::env_map::{EnvRotation, EnvironmentMap};
use glam::Vec3;

/// Selects what `trace_spectral_ray` samples when a ray misses the gemstone.
///
/// Either the analytic studio rig (`Studio`, the default -- see
/// `sample_studio_environment`) or a loaded HDR equirectangular panorama (`HdrMap`, via
/// [`crate::renderer::env_map::EnvironmentMap`]). The analytic rig stays useful for
/// controlled comparisons where a real photograph would introduce variables (its own
/// exposure, white balance, capture noise) a study wants held constant.
///
/// Both variants carry the scene's exposure and apply it where the light is looked up, so
/// every engine, the denoiser, adaptive sampling and the tone map downstream see the same
/// exposed radiance and nothing past the lighting applies it a second time.
#[derive(Clone, Copy)]
pub enum EnvironmentSource<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
        /// Radiance at exposure 1 of the tray below the girdle plane, seen by every escape
        /// and by the camera: a matte surface lit by the rig, `rho * E_rig / pi`, in the
        /// preset's own spectral-power units (so it renders neutral after white balance).
        /// The lighting arms apply the exposure. `0.0` = the preset's own ground (As Lit).
        /// Resolved by `render_setup::Backdrop::resolve` (see `tray.rs`); the stone's
        /// windows show it exactly as the surround does.
        backdrop: f32,
        /// Scale of the stone's first-surface specular (Fresnel) reflection, `1.0`
        /// leaving it as is and `0.0` removing the mirror image of the light so only
        /// light that entered the stone remains (the effect of cross-polarised
        /// viewing). Everything that went into the stone is untouched. See
        /// [`EnvironmentSource::with_surface_glare`].
        surface_glare: f32,
        /// Width of the observer head shadow the lit models darken around the eye direction,
        /// in degrees (`0.0`: off; default [`DEFAULT_HEAD_SHADOW_DEG`]). `Studio` ignores it. See
        /// [`head_shadow_cosines`](super::head_shadow_cosines) and [`Self::with_head_shadow`].
        head_shadow_deg: f32,
        /// The stone's shadow on the paper: per band, the ratio `T = E_with / E_open` of the
        /// light on the paper with the stone present to the open-paper light. `None` (the
        /// usual case) leaves every lookup exactly as it was without the field; read only
        /// below the horizon and only with a tray (`backdrop > 0`).
        tray_shadow: Option<&'a TrayShadowMap>,
        /// The glossy tray: a mirror plane at the culet with Schlick reflectance (see
        /// [`TrayMirror`]). `None` (the usual case) leaves every lookup, every trace and every
        /// pixel exactly as it was without the field; read only with a tray (`backdrop > 0`),
        /// see [`EnvironmentSource::tray_mirror`].
        tray_mirror: Option<TrayMirror>,
    },
    /// A loaded HDR panorama, in its own orientation turned about the vertical axis by
    /// `rotation` (the identity: as decoded).
    HdrMap {
        /// The decoded map.
        map: &'a EnvironmentMap,
        /// The scene's exposure, a plain multiplier on every radiance the map lights the
        /// scene with: the miss lookups, the next-event-estimation draws and the
        /// Ray-Traced engine's leaves ([`hdr_rgb_at_exposure`]). The importance
        /// distribution and its pdf do not depend on it. `1.0` (what [`Self::hdr`] gives)
        /// skips the multiply. Not metered: a map is its own light.
        exposure: f32,
        /// The map's yaw about `+Y`. Every lookup rotates a world direction into the map's
        /// frame first ([`EnvRotation::to_map`]) and every direction drawn from the map back
        /// to the world ([`EnvRotation::to_world`]); texel colors and pdfs are unchanged.
        /// [`EnvRotation::IDENTITY`] (what [`Self::hdr`] and [`Self::hdr_at`] give) returns
        /// every direction untouched.
        rotation: EnvRotation,
    },
}

/// An HDR texel's linear RGB radiance at `exposure`.
///
/// Each channel times `exposure`, or the texel itself when `exposure` is exactly `1.0` (the
/// multiply is skipped, not merely an identity, so the default keeps its bits on every
/// backend).
///
/// The one place an HDR map's exposure is applied. Its WGSL twins are `hdr_rgb_at_exposure`
/// (`transport_bounce/04_studio_and_hdr_environment.wgsl`, reading
/// `GpuTransportParams::hdr_exposure`) and `wt_hdr_at_exposure`
/// (`whitted/30_environment.wgsl`, reading the Ray-Traced uniform's exposure slot), with the
/// same early return. Every spectral lift of an exposed texel
/// ([`crate::renderer::env_map::rgb_to_spectral_radiance`]) starts with a `max(.., 0)` per
/// channel, so the multiply never fuses with the arithmetic after it.
#[must_use]
#[inline]
pub const fn hdr_rgb_at_exposure(rgb: [f32; 3], exposure: f32) -> [f32; 3] {
    if exposure == 1.0 {
        return rgb;
    }
    [rgb[0] * exposure, rgb[1] * exposure, rgb[2] * exposure]
}

/// A fixed tray radiance of 0.23 (about sRGB 160 at exposure 1), for tests and PGO scenes.
/// User backdrops resolve through `render_setup::Backdrop::resolve`.
pub const BACKDROP_GREY: f32 = 0.23;

impl<'a> EnvironmentSource<'a> {
    /// Attaches the stone's shadow map to a studio rig for the lifetime of this render. An
    /// HDR map is its own backdrop and is returned unchanged (`shadow` is dropped).
    #[must_use]
    pub const fn with_tray_shadow<'b>(
        self,
        shadow: Option<&'b TrayShadowMap>,
    ) -> EnvironmentSource<'b>
    where
        'a: 'b,
    {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_mirror,
                ..
            } => EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_shadow: shadow,
                tray_mirror,
            },
            Self::HdrMap {
                map,
                exposure,
                rotation,
            } => EnvironmentSource::HdrMap {
                map,
                exposure,
                rotation,
            },
        }
    }

    /// Attaches the glossy tray to a studio rig (see the `Studio` variant's field); `None`
    /// removes it. An HDR map is its own backdrop and is returned unchanged (`mirror` is
    /// dropped).
    #[must_use]
    pub const fn with_tray_mirror(self, mirror: Option<TrayMirror>) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_shadow,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_shadow,
                tray_mirror: mirror,
            },
            hdr @ Self::HdrMap { .. } => hdr,
        }
    }

    /// The glossy tray carried by a studio rig with paper under the stone: `Some` only for
    /// `Studio` with `backdrop > 0.0`, so an As Lit scene or an HDR map never reflects.
    #[must_use]
    pub const fn tray_mirror(&self) -> Option<TrayMirror> {
        match *self {
            Self::Studio {
                backdrop,
                tray_mirror,
                ..
            } if backdrop > 0.0 => tray_mirror,
            _ => None,
        }
    }

    /// The spectral shadow carried by a studio rig with paper under the stone.
    #[must_use]
    pub const fn tray_shadow(&self) -> Option<&'a TrayShadowMap> {
        match *self {
            Self::Studio {
                backdrop,
                tray_shadow,
                ..
            } if backdrop > 0.0 => tray_shadow,
            _ => None,
        }
    }

    /// The HDR panorama `map` at exposure `1.0`: its texels exactly as decoded.
    #[must_use]
    pub const fn hdr(map: &'a EnvironmentMap) -> Self {
        Self::HdrMap {
            map,
            exposure: 1.0,
            rotation: EnvRotation::IDENTITY,
        }
    }

    /// The HDR panorama `map` lit at the scene's `exposure` (see the `HdrMap` variant's
    /// field). What every renderer route builds for an HDR-lit scene; `1.0` equals
    /// [`Self::hdr`].
    #[must_use]
    pub const fn hdr_at(map: &'a EnvironmentMap, exposure: f32) -> Self {
        Self::HdrMap {
            map,
            exposure,
            rotation: EnvRotation::IDENTITY,
        }
    }

    /// The same source with its HDR map turned by `rotation`; the studio rig has no map and
    /// is returned unchanged.
    #[must_use]
    pub const fn with_hdr_rotation(self, rotation: EnvRotation) -> Self {
        match self {
            Self::HdrMap { map, exposure, .. } => Self::HdrMap {
                map,
                exposure,
                rotation,
            },
            studio @ Self::Studio { .. } => studio,
        }
    }

    /// The HDR map's yaw; [`EnvRotation::IDENTITY`] for the analytic rig.
    #[must_use]
    pub const fn hdr_rotation(&self) -> EnvRotation {
        match *self {
            Self::Studio { .. } => EnvRotation::IDENTITY,
            Self::HdrMap { rotation, .. } => rotation,
        }
    }

    /// The loaded HDR panorama, `None` for the analytic rig.
    #[must_use]
    pub const fn hdr_map(&self) -> Option<&'a EnvironmentMap> {
        match *self {
            Self::Studio { .. } => None,
            Self::HdrMap { map, .. } => Some(map),
        }
    }

    /// The exposure the lighting applies: the `Studio` rig's or the HDR map's.
    #[must_use]
    pub const fn exposure(&self) -> f32 {
        match *self {
            Self::Studio { exposure, .. } | Self::HdrMap { exposure, .. } => exposure,
        }
    }
}

impl EnvironmentSource<'_> {
    /// Puts a lit tray of radiance `backdrop` (at exposure 1) under the stone (see the
    /// `Studio` variant's field); an HDR map is its own backdrop and is returned unchanged.
    #[must_use]
    pub const fn with_backdrop(self, backdrop: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                surface_glare,
                head_shadow_deg,
                tray_shadow,
                tray_mirror,
                ..
            } => Self::Studio {
                tray_shadow,
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_mirror,
            },
            hdr @ Self::HdrMap { .. } => hdr,
        }
    }

    /// Sets the head-shadow size in degrees (see the `Studio` variant's field): `0.0`
    /// turns it off, NaN means the default, other values are clamped to `3..=88`. An HDR
    /// map is returned unchanged.
    #[must_use]
    pub const fn with_head_shadow(self, head_shadow_deg: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                tray_shadow,
                tray_mirror,
                ..
            } => Self::Studio {
                tray_shadow,
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg: clamp_head_shadow_deg(head_shadow_deg),
                tray_mirror,
            },
            hdr @ Self::HdrMap { .. } => hdr,
        }
    }

    /// The head-shadow size in force, degrees: the `Studio` field, the default for an
    /// HDR map (which has no head shadow).
    #[must_use]
    pub const fn head_shadow_deg(&self) -> f32 {
        match *self {
            Self::Studio {
                head_shadow_deg, ..
            } => head_shadow_deg,
            Self::HdrMap { .. } => DEFAULT_HEAD_SHADOW_DEG,
        }
    }

    /// Sets the surface-glare scale (see the `Studio` variant's field), clamped to
    /// `0.0..=1.0`; a NaN is treated as `1.0`. `1.0` is the default and leaves the
    /// render bit-identical to one without the field. An HDR map is returned unchanged
    /// (glare is only applied for the analytic lighting presets).
    #[must_use]
    pub const fn with_surface_glare(self, surface_glare: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                head_shadow_deg,
                tray_shadow,
                tray_mirror,
                ..
            } => Self::Studio {
                tray_shadow,
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare: clamp_surface_glare(surface_glare),
                head_shadow_deg,
                tray_mirror,
            },
            hdr @ Self::HdrMap { .. } => hdr,
        }
    }

    /// The analytic lighting preset, `None` for an HDR panorama.
    #[must_use]
    pub const fn lighting_preset(&self) -> Option<LightingPreset> {
        match *self {
            Self::Studio { preset, .. } => Some(preset),
            Self::HdrMap { .. } => None,
        }
    }

    /// The surface-glare scale in force: the `Studio` field, `1.0` for an HDR map.
    #[must_use]
    pub const fn surface_glare(&self) -> f32 {
        match *self {
            Self::Studio { surface_glare, .. } => surface_glare,
            Self::HdrMap { .. } => 1.0,
        }
    }
}

/// Clamps a surface-glare scale to `0.0..=1.0`, mapping NaN to `1.0` (off).
const fn clamp_surface_glare(value: f32) -> f32 {
    if value >= 1.0 || value.is_nan() {
        1.0
    } else if value > 0.0 {
        value
    } else {
        0.0
    }
}

/// The von-Kries white-balance scale (Bradford LMS-space, see
/// [`compute_illuminant_white_balance`]) that [`trace_spectral_ray`] applies, via
/// [`apply_von_kries_white_balance`], to its final XYZ for a `Studio` `environment`.
///
/// Only the studio rig has one illuminant temperature to neutralize against, so `HdrMap`
/// returns `Vec3::ONE`. The trace also skips the call for `HdrMap`: the XYZ to LMS to XYZ
/// round trip is not exactly the identity at `Vec3::ONE` in f32, so skipping it is exact.
/// The WGSL kernel gates the same transform on `Studio` only.
#[inline]
#[expect(
    clippy::redundant_pub_crate,
    reason = "re-exported as `pub(crate)` from `environment` for color::metrics, the transport code and the GPU estimator checks"
)]
pub(crate) fn environment_white_balance(environment: EnvironmentSource<'_>) -> Vec3 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => illuminant_white_balance(preset),
        EnvironmentSource::HdrMap { .. } => Vec3::ONE,
    }
}
