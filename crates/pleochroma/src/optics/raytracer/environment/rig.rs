//! Numerically evaluates radiance for a sampled direction.
//!
//! The analytic studio rig's per-model falloff shapes
//! ([`sample_studio_environment_with_rig`] and its
//! `Studio`/`IsoHemisphere`/`LightTent`/`DaylightDome` arms), the observer head-shadow
//! and horizon blends they share, the lit backdrop tray every escape sees below the
//! horizon (`tray > 0.0`; see `tray.rs`). Next-event-estimation lives in `nee.rs`.
//!
//! The origin-aware lookups ([`sample_environment_channels_at`],
//! [`sample_environment_channel_at`]) add the stone's shadow on the paper: below the horizon
//! and with a tray they scale the tray radiance per channel by the `tray_shadow` map's ratio
//! where the ray lands, and are bit-identical to the directional lookups otherwise.

use super::{
    DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingModel, LightingPreset,
    LightingRigParams, d65_relative_spectral_power, head_shadow_cosines,
    sun::{SUN_DISC_COS, sun_radiance_factor},
};
use crate::optics::studio_rig::RING_LIGHT_COUNT;
use glam::Vec3;

/// Spectral radiance of channel `lambda_nm` for a missed ray in direction `dir`.
///
/// Keeps the `HdrMap` arm out of `trace_spectral_ray`'s miss branch.
///
/// The `HdrMap` arm is the map's bilinear RGB at `dir` (`EnvironmentMap::radiance_rgb_rotated`)
/// at the source's exposure ([`hdr_rgb_at_exposure`](super::hdr_rgb_at_exposure)), then the
/// spectral lift. This equals `EnvironmentMap::radiance_at` at exposure `1.0` with no
/// rotation. WGSL twin: `hdr_env_radiance_at`.
///
/// `studio_rig` is built once per ray by the trace and borrowed here. `observer` is the unit
/// direction from the stone towards the eye (see
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed)). Both
/// are unused for `HdrMap`.
#[inline]
pub fn sample_environment_channel(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambda_nm: f32,
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            head_shadow_deg,
            backdrop,
            ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channel: Studio environment needs a pre-built rig");
            sample_studio_environment_with_rig_shadow_tray(
                dir,
                lambda_nm,
                preset,
                exposure,
                rig,
                observer,
                head_shadow_cosines(head_shadow_deg),
                backdrop,
            )
        }
        EnvironmentSource::HdrMap {
            map,
            exposure,
            rotation,
        } => crate::renderer::env_map::rgb_to_spectral_radiance(
            super::hdr_rgb_at_exposure(map.radiance_rgb_rotated(dir, rotation), exposure),
            lambda_nm,
        ),
    }
}

/// Cone-half-angle cosines of the lit models. Literal values, never computed, so the CPU and
/// the WGSL twins use identical bits.
///
/// The observer head shadow is not one of these: it is the scene parameter
/// `EnvironmentSource::Studio::head_shadow_deg` (default 16 degrees), evaluated once to an
/// `[outer, inner]` pair by [`head_shadow_cosines`](super::head_shadow_cosines). The sun disc
/// is `sun::SUN_DISC_COS`.
///
/// Light tent overhead softbox: full within 20 degrees of the key, gone by 40.
const TENT_KEY_OUTER_COS: f32 = 0.766_044_4;
const TENT_KEY_INNER_COS: f32 = 0.939_692_6;
/// Light tent spark light (a bare bulb at the fill position): full within 2 degrees,
/// gone by 5.
const SPARK_OUTER_COS: f32 = 0.996_194_7;
const SPARK_INNER_COS: f32 = 0.999_390_8;
/// Light tent black cards: fully black within 16 degrees of a card centre, gone by 26.
const CARD_OUTER_COS: f32 = 0.898_794;
const CARD_INNER_COS: f32 = 0.961_261_7;
/// The ring positions (see `StudioRig::ring_dirs`) carrying the three black cards: 90,
/// 180 and 270 degrees around from the key light, at the ring's own elevation.
const CARD_RING_SLOTS: [usize; 3] = [4, 8, 12];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (-2.0f32).mul_add(t, 3.0)
}

/// `1.0` where direction `d` sees past the observer, falling to `0.0` inside the
/// head-shadow cone around `observer` (see
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed)).
fn observer_visibility(d: Vec3, observer: Vec3, shadow: [f32; 2]) -> f32 {
    1.0 - smoothstep(shadow[0], shadow[1], d.dot(observer))
}

/// `0.0` below the girdle plane, `1.0` above, blended over `-0.05..0.05` in `d.y` so
/// the horizon never aliases.
pub fn horizon_blend(d: Vec3) -> f32 {
    smoothstep(-0.05, 0.05, d.y)
}

/// One direction's lighting with every wavelength-independent factor already evaluated.
///
/// Only the illuminant's spectral power differs between the channels that look at the
/// same direction, so the geometry (normalisation, dot products, `powi`s, smoothsteps) is
/// built once per direction and [`Self::radiance`] finishes each channel with the same
/// multiplies and fused multiply-adds, in the same order, the one-shot evaluation used.
#[derive(Clone, Copy)]
enum DirectionLighting {
    /// The analytic studio rig: the backdrop term plus the key, fill and ring intensities
    /// that passed their thresholds, each already scaled by exposure.
    Rig {
        /// Backdrop intensity (multiplied by the spectral power, not fused).
        bg_val: f32,
        /// Key softbox intensity, `None` when `d` is not in the key's forward hemisphere.
        softbox: Option<f32>,
        /// Fill softbox intensity, `None` when `d` is not in the fill's forward hemisphere.
        fill: Option<f32>,
        /// Intensities of the ring emitters whose `dot > 0.96`, in ring order.
        ring: [f32; RING_LIGHT_COUNT],
        /// How many leading entries of `ring` are live.
        ring_len: usize,
        /// The tray, `Some((horizon, ground))` only with `tray > 0.0`: `horizon` is the
        /// horizon blend and `ground` is `((1 - horizon) * tray) * exposure`, which the
        /// radiance finishes as `fma(horizon, rig, ground * spec_power)`.
        ground: Option<(f32, f32)>,
    },
    /// A lit model: a geometric factor scaled by `spec_power * exposure`.
    Scaled(f32),
    /// The ASET contrast view: three zone amplitudes (they sum to the horizon blend)
    /// lit by one narrow spectral band each. For this variant the `spec_power` argument
    /// of [`Self::radiance`] is the wavelength in nm, not an illuminant power (see
    /// [`aset_spec_input`]).
    /// `tray_ground` is `(1 - horizon) * tray`, `0.0` for As Lit.
    Banded {
        red: f32,
        green: f32,
        blue: f32,
        tray_ground: f32,
    },
}

/// Centre wavelengths (nm) of the three ASET zone bands.
const ASET_RED_NM: f32 = 610.0;
const ASET_GREEN_NM: f32 = 540.0;
const ASET_BLUE_NM: f32 = 460.0;
/// Gaussian sigma of an ASET band: 20 nm FWHM, `20 / (2 * sqrt(2 ln 2))`.
const ASET_SIGMA_NM: f32 = 8.493_218;
/// Per-zone radiance gains. A 20 nm line carries far less energy than the broad D65 curve,
/// and the eye is less sensitive at 460 nm than at 540 nm, so the gains bring the bands to
/// comparable perceived brightness (estimated from V(lambda) at the band centres, then tuned
/// by eye).
const ASET_RED_GAIN: f32 = 8.0;
const ASET_GREEN_GAIN: f32 = 5.0;
const ASET_BLUE_GAIN: f32 = 16.0;
/// Elevation zone edges as `sin(elevation)` (= `d.y`): 45 and 75 degrees, each blended over
/// +-0.02 so a zone boundary never aliases.
const ASET_EDGE_45_LO: f32 = 0.687_106_8;
const ASET_EDGE_45_HI: f32 = 0.727_106_8;
const ASET_EDGE_75_LO: f32 = 0.945_925_8;
const ASET_EDGE_75_HI: f32 = 0.985_925_8;

/// The value the ASET model reads as its `spec_power`: the wavelength itself, so the zone
/// bands can be evaluated where the model runs. Every other preset gets its illuminant's
/// relative power.
const fn aset_spec_input(lambda_nm: f32) -> f32 {
    lambda_nm
}

/// Unit-peak Gaussian band at `centre_nm`.
fn aset_band(lambda_nm: f32, centre_nm: f32) -> f32 {
    let z = (lambda_nm - centre_nm) / ASET_SIGMA_NM;
    (-0.5 * z * z).exp()
}

impl DirectionLighting {
    /// The radiance of this direction under an illuminant of relative power `spec_power`.
    fn radiance(self, spec_power: f32, exposure: f32) -> f32 {
        match self {
            Self::Rig {
                bg_val,
                softbox,
                fill,
                ring,
                ring_len,
                ground,
            } => {
                let mut radiance = bg_val * spec_power;
                if let Some(softbox) = softbox {
                    radiance = softbox.mul_add(spec_power, radiance);
                }
                if let Some(fill) = fill {
                    radiance = fill.mul_add(spec_power, radiance);
                }
                for &intensity in &ring[..ring_len] {
                    radiance = intensity.mul_add(spec_power, radiance);
                }
                ground.map_or(radiance, |(horizon, ground)| {
                    horizon.mul_add(radiance, ground * spec_power)
                })
            }
            Self::Scaled(factor) => factor * (spec_power * exposure),
            Self::Banded {
                red,
                green,
                blue,
                tray_ground,
            } => {
                let lambda_nm = spec_power;
                let r = (red * aset_band(lambda_nm, ASET_RED_NM)) * ASET_RED_GAIN;
                let g = (green * aset_band(lambda_nm, ASET_GREEN_NM)) * ASET_GREEN_GAIN;
                let b = (blue * aset_band(lambda_nm, ASET_BLUE_NM)) * ASET_BLUE_GAIN;
                let zones = ((r + g) + b) * exposure;
                // A neutral D65 tray (`E / pi := 1`); the branch keeps As Lit's bits.
                if tray_ground > 0.0 {
                    (tray_ground * d65_relative_spectral_power(lambda_nm)).mul_add(exposure, zones)
                } else {
                    zones
                }
            }
        }
    }
}

/// The ground term of the lit models. With a tray (`tray > 0.0`) the tray replaces the
/// model's own ground and gets the model's observer treatment: `((1 - horizon) * vis) * tray`;
/// As Lit keeps `own_ground * (1 - horizon)` exactly (a branch, never a multiply by zero).
fn lit_ground(horizon: f32, vis: f32, tray: f32, own_ground: f32) -> f32 {
    if tray > 0.0 {
        ((1.0 - horizon) * vis) * tray
    } else {
        own_ground * (1.0 - horizon)
    }
}

fn iso_hemisphere_lighting(
    d: Vec3,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let vis = observer_visibility(d, observer, shadow);
    if tray > 0.0 {
        // fma((1 - horizon) * vis, tray, horizon * vis)
        DirectionLighting::Scaled(((1.0 - horizon) * vis).mul_add(tray, horizon * vis))
    } else {
        DirectionLighting::Scaled(horizon * vis)
    }
}

fn light_tent_lighting(
    d: Vec3,
    spot_mult: f32,
    tent: super::TentParams,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    // Tent walls: 0.14 at the girdle plane rising to 0.22 at the zenith -- middle grey
    // after the ACES curve, so a facet that sees nothing but the tent is grey, not
    // white. The black cards cut that to a tenth.
    //
    // The parameters are exact identities at `TentParams::DEFAULT` (`x * 1.0` is `x` in
    // f32), so the light tent keeps its bits; other tent presets change only the four floats.
    let mut walls = 0.08f32.mul_add(d.y.max(0.0), 0.14) * tent.walls;
    // `flat` blends the gradient towards its value at 30 degrees elevation; the branch keeps
    // `flat == 0` bit-identical.
    if tent.flat > 0.0 {
        walls = (0.18 * tent.walls).mul_add(tent.flat, walls * (1.0 - tent.flat));
    }
    let mut card = 0.0f32;
    for slot in CARD_RING_SLOTS {
        card = card.max(smoothstep(
            CARD_OUTER_COS,
            CARD_INNER_COS,
            d.dot(rig.ring_dirs[slot]),
        ));
    }
    walls *= (card * tent.cards).mul_add(-0.9, 1.0);
    let key =
        smoothstep(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, d.dot(rig.key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep(SPARK_OUTER_COS, SPARK_INNER_COS, d.dot(rig.fill_dir))
        * (5.0 * spot_mult)
        * tent.spark;
    let vis = observer_visibility(d, observer, shadow);
    let above = ((walls + key) + spark) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, tent.ground);
    DirectionLighting::Scaled(above + ground)
}

fn daylight_dome_lighting(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    // A clear sky is brightest at the horizon and around the sun, darkest at the zenith.
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    // `sun_dot^8`, written as three squarings so the GPU twin multiplies identically.
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    // Sky only: the sun disc belongs to `DaylightSun`, not to the dome.
    let vis = observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, 0.04);
    DirectionLighting::Scaled(above + ground)
}

fn studio_rig_lighting(
    d: Vec3,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    ambient_fill: bool,
    tray: f32,
) -> DirectionLighting {
    // 1. Ambient backdrop (neutral dark charcoal); absent for the UV lamps (dark room).
    let bg_val = if ambient_fill {
        0.012f32.mul_add(d.y.mul_add(0.5, 0.5), 0.015).max(0.005) * exposure
    } else {
        0.0
    };

    // 2. Main Key Softbox Light
    let key_dot = d.dot(rig.key_dir).max(0.0);
    let softbox = (key_dot > 0.0).then(|| key_dot.powi(28) * 12.0 * spot_mult * exposure);

    // 3. Fill Softbox Light (side reflector offset by 140 deg)
    let fill_dot = d.dot(rig.fill_dir).max(0.0);
    let fill = (fill_dot > 0.0).then(|| fill_dot.powi(18) * 4.5 * exposure);

    // 4. Ring lights: up to 16 pinpoint sources that rotate with the rig.
    let mut sparks = [0.0f32; RING_LIGHT_COUNT];
    let mut ring_len = 0;
    for ring_dir in rig.ring_dirs {
        let ring_dot = d.dot(ring_dir).max(0.0);
        if ring_dot > 0.96 {
            let spark = (ring_dot - 0.96) / 0.04;
            sparks[ring_len] = spark.powi(6) * 22.0 * spot_mult * exposure;
            ring_len += 1;
        }
    }

    // The tray replaces everything below the horizon: `fma(h, rig, ((g * tray) * exposure) * S)`.
    // The rig ignores the observer, so the tray does too.
    let ground = (tray > 0.0).then(|| {
        let horizon = horizon_blend(d);
        (horizon, ((1.0 - horizon) * tray) * exposure)
    });

    DirectionLighting::Rig {
        bg_val,
        softbox,
        fill,
        ring: sparks,
        ring_len,
        ground,
    }
}

/// Evaluates `lighting_preset`'s wavelength-independent lighting of `dir` once.
///
/// `tray` is the lit backdrop's radiance at exposure 1. `0.0` means As Lit: the preset's own
/// ground (see `tray.rs`).
fn direction_lighting(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let d = dir.normalize();
    let LightingRigParams {
        spot_mult, tent, ..
    } = lighting_preset.params();
    match lighting_preset.model() {
        LightingModel::Studio => studio_rig_lighting(
            d,
            spot_mult,
            exposure,
            rig,
            lighting_preset.has_ambient_fill(),
            tray,
        ),
        LightingModel::IsoHemisphere => iso_hemisphere_lighting(d, observer, shadow, tray),
        LightingModel::LightTent => {
            light_tent_lighting(d, spot_mult, tent, rig, observer, shadow, tray)
        }
        LightingModel::DaylightDome => daylight_dome_lighting(d, rig, observer, shadow, tray),
        LightingModel::Aset => aset_radiance(d, rig, observer, shadow, tray),
        LightingModel::DaylightSun => daylight_sun_radiance(d, rig, observer, shadow, tray),
    }
}

/// The wavelength-independent geometry of `lighting_preset` along `dir` at exposure 1 with no
/// observer and no tray, `radiance(1.0, 1.0)`: the `G(d)` the tray's irradiance integrates.
pub(super) fn unit_radiance(
    dir: Vec3,
    lighting_preset: LightingPreset,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    direction_lighting(
        dir,
        lighting_preset,
        1.0,
        rig,
        Vec3::ZERO,
        DEFAULT_HEAD_SHADOW_COSINES,
        0.0,
    )
    .radiance(1.0, 1.0)
}

/// ASET-style contrast view (model id 4), after the Angular Spectrum Evaluation Tool.
///
/// Zones by elevation above the horizon: green 0-45 degrees, red 45-75 and blue 75-90. Blue
/// is a fixed elevation cap, not a cone around the view ray, so the zones do not move with
/// the camera. Black below the horizon. Each zone emits one narrow band, so the facets show
/// which zone they return light from. The head shadow is not applied (`observer` and `shadow`
/// are unused).
fn aset_radiance(
    d: Vec3,
    _rig: &crate::optics::studio_rig::StudioRig,
    _observer: Vec3,
    _shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let s45 = smoothstep(ASET_EDGE_45_LO, ASET_EDGE_45_HI, d.y);
    let s75 = smoothstep(ASET_EDGE_75_LO, ASET_EDGE_75_HI, d.y);
    DirectionLighting::Banded {
        red: (s45 * (1.0 - s75)) * horizon,
        green: (1.0 - s45) * horizon,
        blue: s75 * horizon,
        tray_ground: if tray > 0.0 {
            (1.0 - horizon) * tray
        } else {
            0.0
        },
    }
}

/// Daylight sky plus a direct sun (model id 5).
///
/// It adds to `DaylightDome`'s sky, aureole, ground and head shadow the hard-edged 0.27 degree
/// sun disc of
/// [`SUN_RADIANCE`](super::sun::SUN_RADIANCE) (derivation in `sun.rs`). The sun is not scaled
/// by the head shadow and carries its own horizon fade ([`sun_radiance_factor`]). Its disc is
/// sampled by next-event estimation ([`sample_environment_for_nee`]).
fn daylight_sun_radiance(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let vis = observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, 0.04);
    let sun = if sun_dot >= SUN_DISC_COS {
        sun_radiance_factor(rig.key_dir)
    } else {
        0.0
    };
    DirectionLighting::Scaled((above + ground) + sun)
}

/// Radiance along `dir` for a pre-built [`StudioRig`](crate::optics::studio_rig::StudioRig).
///
/// Same arithmetic as [`super::sample_studio_environment_observed`], which builds its own
/// rig. Dispatches on the preset's [`LightingModel`]. For many directions under one light
/// pose, build the rig once and pass it to every call.
#[must_use]
pub fn sample_studio_environment_with_rig(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> f32 {
    sample_studio_environment_with_rig_shadow(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        rig,
        observer,
        DEFAULT_HEAD_SHADOW_COSINES,
    )
}

/// [`sample_studio_environment_with_rig`] with an explicit head-shadow cone.
///
/// `shadow` is the `[outer, inner]` pair from
/// [`head_shadow_cosines`](super::head_shadow_cosines) (the default pair reproduces the
/// unparameterised call bit for bit).
#[must_use]
pub fn sample_studio_environment_with_rig_shadow(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> f32 {
    sample_studio_environment_with_rig_shadow_tray(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        rig,
        observer,
        shadow,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_shadow`] over a lit backdrop tray.
///
/// `tray` is [`EnvironmentSource::Studio`]'s `backdrop`: the tray's radiance at exposure 1
/// (`rho * E_rig / pi`, see `tray.rs`), seen below the horizon in place of the preset's own
/// ground. `0.0` is As Lit and is bit-identical to the unsuffixed entry point.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the unsuffixed entry point plus the tray, kept flat to mirror its WGSL twin"
)]
pub fn sample_studio_environment_with_rig_shadow_tray(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> f32 {
    direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray)
        .radiance(studio_spec_power(lighting_preset, lambda_nm), exposure)
}

/// The `spec_power` input [`sample_studio_environment_with_rig_shadow`] feeds the radiance
/// at wavelength `lambda_nm`: the wavelength itself for the ASET view, the preset's
/// spectral power otherwise.
///
/// A pure function of `(lighting_preset, lambda_nm)`, so a caller probing many directions at
/// one wavelength computes it once and passes it to
/// [`sample_studio_environment_with_rig_spec_power`].
#[must_use]
pub fn studio_spec_power(lighting_preset: LightingPreset, lambda_nm: f32) -> f32 {
    if lighting_preset == LightingPreset::Aset {
        aset_spec_input(lambda_nm)
    } else {
        lighting_preset.spectral_power(lambda_nm)
    }
}

/// [`sample_studio_environment_with_rig`] with the wavelength's `spec_power` already evaluated.
///
/// `spec_power` is [`studio_spec_power`]'s value. The result is bit-identical to the wrapper
/// when `spec_power == studio_spec_power(lighting_preset, lambda_nm)`, since the radiance only
/// ever reads the wavelength through that one value.
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
) -> f32 {
    sample_studio_environment_with_rig_spec_power_tray(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        spec_power,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_spec_power`] with an explicit head-shadow cone.
///
/// `shadow` is the `[outer, inner]` pair from
/// [`head_shadow_cosines`](super::head_shadow_cosines);
/// the default pair gives the same bits as the unsuffixed spec-power entry.
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power_shadow(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    shadow: [f32; 2],
) -> f32 {
    sample_studio_environment_with_rig_spec_power_shadow_tray(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        spec_power,
        shadow,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_spec_power_shadow`] over a lit backdrop tray.
///
/// `tray` is [`EnvironmentSource::Studio`]'s `backdrop` (radiance at exposure 1, see
/// `tray.rs`); `0.0` is As Lit and bit-identical to the unsuffixed entry point.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the unsuffixed entry point plus the tray, kept flat to mirror its WGSL twin"
)]
pub fn sample_studio_environment_with_rig_spec_power_shadow_tray(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    shadow: [f32; 2],
    tray: f32,
) -> f32 {
    direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray)
        .radiance(spec_power, exposure)
}

/// [`sample_studio_environment_with_rig_spec_power`] over a lit backdrop tray (default head
/// shadow).
///
/// `tray = 0.0` is bit-identical to the unsuffixed entry point. See
/// [`sample_studio_environment_with_rig_shadow_tray`].
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power_tray(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    tray: f32,
) -> f32 {
    direction_lighting(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        DEFAULT_HEAD_SHADOW_COSINES,
        tray,
    )
    .radiance(spec_power, exposure)
}

/// Samples the studio rig at every wavelength in `lambdas` along the same `dir`.
///
/// The direction's geometry and the preset's Planck constants are evaluated once, then each
/// channel applies its own spectral power. Element `k` matches the scalar
/// [`sample_studio_environment_with_rig_shadow_tray`] up to Planck rounding, so comparisons
/// with separately evaluated channels need a small tolerance.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the single-wavelength entry's parameters plus the lambda array and the tray"
)]
fn sample_studio_environment_channels<const N: usize>(
    dir: Vec3,
    lambdas: &[f32; N],
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> [f32; N] {
    let lighting = direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray);
    let spectrum = lighting_preset.illuminant_spectrum();
    let aset = lighting_preset == LightingPreset::Aset;
    std::array::from_fn(|k| {
        let spec_power = if aset {
            aset_spec_input(lambdas[k])
        } else {
            spectrum.power(lambdas[k])
        };
        lighting.radiance(spec_power, exposure)
    })
}

/// Samples the escaped ray's environment at every wavelength in `lambdas`.
///
/// All channels share the exit direction `dir`. Element
/// `k` matches `sample_environment_channel` at `lambdas[k]`, up to the Planck rounding
/// noted on [`sample_studio_environment_channels`].
#[inline]
pub fn sample_environment_channels<const N: usize>(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambdas: &[f32; N],
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> [f32; N] {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            head_shadow_deg,
            backdrop,
            ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channels: Studio environment needs a pre-built rig");
            sample_studio_environment_channels(
                dir,
                lambdas,
                preset,
                exposure,
                rig,
                observer,
                head_shadow_cosines(head_shadow_deg),
                backdrop,
            )
        }
        EnvironmentSource::HdrMap {
            map,
            exposure,
            rotation,
        } => {
            // The direction -> (u, v) -> bilinear RGB part (and its exposure) is
            // channel-invariant: evaluate it once and convert per channel. Exactly
            // `sample_environment_channel`'s two steps, so each element stays bit-identical to
            // it (and to `map.radiance_at(dir, lambdas[k])` at exposure 1, unrotated).
            let rgb = super::hdr_rgb_at_exposure(map.radiance_rgb_rotated(dir, rotation), exposure);
            std::array::from_fn(|k| {
                crate::renderer::env_map::rgb_to_spectral_radiance(rgb, lambdas[k])
            })
        }
    }
}

/// Spectral radiance seen by a ray from `origin`. Each channel reads the light on the paper
/// where the ray lands.
///
/// Without a tray shadow map (none attached, an HDR source, no tray, or `backdrop == 0`), and
/// for a ray that does not land inside the map (including every upward ray), the result is
/// [`sample_environment_channels`] bit for bit. Inside the map the tray radiance is
/// `backdrop * factor`, floored at `f32::MIN_POSITIVE`: a tray of exactly `0.0` would select
/// As Lit's own ground, so a fully shadowed spot must stay positive. That branch evaluates the
/// channels one by one and may differ from the hoisted function by Planck rounding.
#[must_use]
pub fn sample_environment_channels_at<const N: usize>(
    environment: EnvironmentSource<'_>,
    origin: Vec3,
    dir: Vec3,
    lambdas: &[f32; N],
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> [f32; N] {
    if let EnvironmentSource::Studio { backdrop, .. } = environment
        && let Some(factors) = environment
            .tray_shadow()
            .and_then(|map| map.factors_for(origin, dir, lambdas))
    {
        return std::array::from_fn(|k| {
            sample_environment_channel(
                environment.with_backdrop((backdrop * factors[k]).max(f32::MIN_POSITIVE)),
                dir,
                lambdas[k],
                studio_rig,
                observer,
            )
        });
    }
    sample_environment_channels(environment, dir, lambdas, studio_rig, observer)
}

/// Radiance of one wavelength escaping from `origin`, including its landing on the paper.
///
/// Equals [`sample_environment_channel`] bit for bit without a map, and for a ray that lands
/// outside it. Inside the map the tray radiance is floored at `f32::MIN_POSITIVE`, as in
/// [`sample_environment_channels_at`].
#[must_use]
pub fn sample_environment_channel_at(
    environment: EnvironmentSource<'_>,
    origin: Vec3,
    dir: Vec3,
    lambda_nm: f32,
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> f32 {
    let environment = if let EnvironmentSource::Studio { backdrop, .. } = environment
        && let Some(map) = environment.tray_shadow()
        && let Some(uv) = map.plane_uv(origin, dir)
    {
        environment.with_backdrop((backdrop * map.factor_at(uv, lambda_nm)).max(f32::MIN_POSITIVE))
    } else {
        environment
    };
    sample_environment_channel(environment, dir, lambda_nm, studio_rig, observer)
}
