//! Environment/lighting sources a ray can sample when it misses the gemstone.
//!
//! The analytic gemological studio rig ([`LightingPreset`], [`sample_studio_environment`])
//! and the loaded-HDR-panorama alternative ([`EnvironmentSource::HdrMap`]).
//!
//! # Lighting models
//!
//! Every [`LightingPreset`] samples one [`LightingModel`]:
//! - `Studio` (`Daylight`, `Incandescent`, `RingLights`, `DarkSpotlight`): the analytic
//!   studio rig -- a charcoal backdrop, one key softbox, one fill and sixteen ring
//!   pinpoints. Its arithmetic is pinned by golden images and never changes.
//! - `IsoHemisphere`: a uniformly radiant upper hemisphere at radiance 1, nothing below
//!   the girdle plane -- the ISO-standard viewing geometry.
//! - `LightTent`: a jewellery light tent -- dim tent walls, one broad overhead softbox,
//!   three black cards on the ring positions away from the key (the contrast
//!   photographers add so a diamond reads as a facet pattern rather than a white blur),
//!   one small hard spark light for scintillation, black velvet below the girdle.
//! - `DaylightDome`: a clear sky only (no sun disc), brighter at the horizon than at the
//!   zenith with an aureole around the key direction, dark ground.
//! - `DaylightSun` (id 5): the same sky plus a physically bright 0.27 degree sun disc
//!   (about 82 % of the horizontal irradiance at the default key elevation), sampled by
//!   analytic next-event estimation with uniform cone sampling and balance-heuristic MIS
//!   (see `rig.rs`). The sun is the only analytic light with an NEE technique.
//! - `Aset` (id 4): the contrast view (zone-coloured spectral power by exit elevation).
//! - `Studio` also carries `IlluminantA` (2856 K); `LightTent` also carries `ShopLights`,
//!   `WindowDaylight` and `WhiteTray`, which differ only in their [`TentParams`].
//!
//! Every lit model except `Studio` and `Aset` darkens the exit directions inside the
//! observer's head-shadow cone ([`head_shadow_cosines`], default 16 degrees), which gives a
//! face-up stone its dark table reflections. `Studio` ignores the observer. Radiances are
//! chosen so that at exposure 1 the ambient terms land near middle grey after the ACES
//! curve, and only a direct reflection of a light source clips to white.
//!
//! Submodules: [`spectral`] (illuminant power curves), [`rig`] (per-direction radiance and
//! the origin-aware `_at` lookups), [`nee`] (light sampling), [`source`] (the
//! [`EnvironmentSource`] type and white balance), [`tray`], [`tray_shadow`], [`tray_mirror`]
//! and [`tray_plane`]. The public items are re-exported below.

use glam::Vec3;

mod nee;
mod rig;
mod source;
mod spectral;
mod sun;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_aset_zones;
#[cfg(test)]
mod tests_hdr_exposure;
#[cfg(test)]
mod tests_hdr_rotation;
#[cfg(test)]
mod tests_mirror;
#[cfg(test)]
mod tests_pins;
#[cfg(test)]
mod tests_shadow;
#[cfg(test)]
mod tests_tray;
mod tray;
mod tray_mirror;
mod tray_plane;
pub mod tray_shadow;

// tray_mirror.rs, tray_plane.rs: the glossy tray and the plane it shares with the shadow
pub use tray_mirror::{FloorHit, TrayMirror};
pub use tray_plane::tray_plane_y;

// The HDR map's yaw, re-exported beside the source that carries it.
pub use crate::renderer::env_map::{EnvRotation, HdrOrientation};

// source.rs
pub(crate) use source::environment_white_balance;
pub use source::{BACKDROP_GREY, EnvironmentSource, hdr_rgb_at_exposure};

// tray.rs
pub use tray::{
    METERED_PAPER_RADIANCE, TRAY_ALBEDO_BLACK, TRAY_ALBEDO_GREY, TRAY_ALBEDO_WHITE,
    TRAY_MODEL_VERSION, metered_exposure, rig_irradiance_over_pi, tray_radiance,
};
pub use tray_shadow::{
    TRAY_SHADOW_BAND_WIDTH_NM, TRAY_SHADOW_BANDS, TRAY_SHADOW_FIRST_CENTER_NM,
    TRAY_SHADOW_MODEL_VERSION, TrayShadowKey, TrayShadowMap, TrayShadowOff, TrayShadowQuality,
};

// nee.rs, rig.rs
pub(super) use nee::{environment_nee_pdf, environment_supports_nee, sample_environment_for_nee};
pub use rig::aset_zone_amplitudes;
#[cfg(feature = "beam")]
pub(crate) use rig::horizon_blend;
pub(crate) use rig::{
    sample_environment_channel, sample_environment_channel_at, sample_environment_channels,
    sample_environment_channels_at,
};
// The sun disc's constants, for the beam tracer's exact disc integration (beam guide §11.4);
// the beam tracer is its only user, so the re-export exists only with the `beam` feature.
#[cfg(feature = "beam")]
pub(crate) use sun::{SUN_DISC_COS, SUN_RADIANCE};

/// The analytic sun's cone sampler for key direction `key_dir`, for the GPU Tier-2 twin check
/// (`renderer::gpu::environment_check::sun_nee`): the direction drawn from `(u0, u1)` and the
/// pdf of the sampling technique there.
#[cfg(feature = "gpu")]
#[must_use]
pub(crate) fn sun_nee_reference_sample(key_dir: Vec3, u0: f32, u1: f32) -> (Vec3, f32) {
    let dir = sun::sun_cone_direction(key_dir, u0, u1);
    (dir, sun::sun_nee_pdf(key_dir, dir))
}

/// The sun's wavelength-independent radiance factor (radiance 40 000 faded by the horizon
/// at the key direction), for the same GPU check.
#[cfg(feature = "gpu")]
#[must_use]
pub(crate) fn sun_nee_reference_factor(key_dir: Vec3) -> f32 {
    sun::sun_radiance_factor(key_dir)
}

/// The sun's NEE pdf at `dir` (`0.0` outside the disc), for the same GPU check.
#[cfg(feature = "gpu")]
#[must_use]
pub(crate) fn sun_nee_reference_pdf(key_dir: Vec3, dir: Vec3) -> f32 {
    sun::sun_nee_pdf(key_dir, dir)
}
pub use rig::{
    sample_studio_environment_with_rig, sample_studio_environment_with_rig_shadow,
    sample_studio_environment_with_rig_shadow_tray, sample_studio_environment_with_rig_spec_power,
    sample_studio_environment_with_rig_spec_power_shadow,
    sample_studio_environment_with_rig_spec_power_shadow_tray,
    sample_studio_environment_with_rig_spec_power_tray, studio_spec_power,
};

// spectral.rs
use spectral::{BlackbodyNorm, IlluminantSpectrum};
pub use spectral::{blackbody_spectrum, d65_relative_spectral_power};

/// Default observer head-shadow size, degrees: fully dark within 14 degrees of the eye
/// direction, gone by 18 (a 16 degree cone, softened so its edge does not alias).
pub const DEFAULT_HEAD_SHADOW_DEG: f32 = 16.0;
/// `[outer, inner]` cosines of [`DEFAULT_HEAD_SHADOW_DEG`] (14 and 18 degrees). Literal
/// values, never computed, so the CPU rig and the WGSL twins use identical bits.
pub const DEFAULT_HEAD_SHADOW_COSINES: [f32; 2] = [0.951_056_5, 0.970_295_7];

/// Largest head-shadow size accepted (the outer cone, `deg + 2`, stays under 90 degrees).
pub const MAX_HEAD_SHADOW_DEG: f32 = 88.0;
/// Smallest nonzero head-shadow size (the inner cone, `deg - 2`, stays at or above 1 degree).
pub const MIN_HEAD_SHADOW_DEG: f32 = 3.0;

/// The `[outer, inner]` cosine pair of a head shadow `deg` degrees wide, evaluated once
/// per scene. The cone is fully dark within `deg - 2` degrees of the eye direction and
/// gone by `deg + 2`.
///
/// `deg <= 0` switches the shadow off: the pair `[2.0, 3.0]` is never
/// reached by a dot product, so `smoothstep` is `0` and the visibility is exactly `1`.
/// The default 16 degrees returns [`DEFAULT_HEAD_SHADOW_COSINES`] bit for bit; NaN is the
/// default; other values are clamped to `3..=88`.
#[must_use]
pub fn head_shadow_cosines(deg: f32) -> [f32; 2] {
    if deg.is_nan() || deg.to_bits() == DEFAULT_HEAD_SHADOW_DEG.to_bits() {
        return DEFAULT_HEAD_SHADOW_COSINES;
    }
    if deg <= 0.0 {
        return [2.0, 3.0];
    }
    let deg = deg.clamp(MIN_HEAD_SHADOW_DEG, MAX_HEAD_SHADOW_DEG);
    [
        (deg + 2.0).to_radians().cos(),
        (deg - 2.0).to_radians().cos(),
    ]
}

/// Sanitises a head-shadow size for storage: NaN is the default, `<= 0` is off (`0.0`),
/// anything else is clamped to `3..=88`.
const fn clamp_head_shadow_deg(deg: f32) -> f32 {
    if deg.is_nan() {
        DEFAULT_HEAD_SHADOW_DEG
    } else if deg <= 0.0 {
        0.0
    } else if deg < MIN_HEAD_SHADOW_DEG {
        MIN_HEAD_SHADOW_DEG
    } else if deg > MAX_HEAD_SHADOW_DEG {
        MAX_HEAD_SHADOW_DEG
    } else {
        deg
    }
}

/// Color temperature and rig intensity of one studio lighting preset.
///
/// Returned by [`LightingPreset::params`], the lookup shared by `sample_studio_environment`
/// and `illuminant_temperature_k`, so the lighting and its white balance cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingRigParams {
    /// Blackbody color temperature in Kelvin, fed to [`blackbody_spectrum`].
    pub temp_k: f32,
    /// Multiplier on the key-softbox and ring-emitter intensity terms (does not affect
    /// the fill light or the ambient backdrop).
    pub spot_mult: f32,
    /// The light-tent model's per-preset knobs (ignored by every other model). The
    /// default ([`TentParams::DEFAULT`]) reproduces `LightTent` bit for bit.
    pub tent: TentParams,
}

/// Per-preset parameters of the [`LightingModel::LightTent`] model: the tent walls' scale,
/// the black cards' strength, the spark on/off flag and the ground radiance.
///
/// The default is an exact identity in f32 (scales of `1.0`, and a ground of `0.02`). The
/// CPU path and the WGSL twins carry the same four floats in `GpuTransportParams`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TentParams {
    /// Scale on the wall radiance (`0.14` at the girdle to `0.22` at the zenith before the
    /// scale).
    pub walls: f32,
    /// Strength of the three black cards: `1.0` darkens the walls to a tenth inside a card
    /// (the tent), `0.0` removes the cards.
    pub cards: f32,
    /// `1.0` keeps the spark (a bare bulb at the fill position), `0.0` removes it.
    pub spark: f32,
    /// Radiance of the ground (every direction below the girdle plane).
    pub ground: f32,
    /// `0.0..=1.0`: blends the wall elevation gradient towards one uniform value,
    /// `walls = mix(gradient, 0.18 * walls_scale, flat)` (`0.18` = the gradient at 30 degrees
    /// elevation). `0.0` skips the blend, so presets with `flat == 0` keep their exact bits.
    pub flat: f32,
}

impl TentParams {
    /// The light tent's own values: walls x1, cards on, spark on, ground `0.02`, no flattening.
    pub const DEFAULT: Self = Self {
        walls: 1.0,
        cards: 1.0,
        spark: 1.0,
        ground: 0.02,
        flat: 0.0,
    };

    /// The four floats in uniform order `[walls, cards, spark, ground]`.
    #[must_use]
    pub const fn to_array(self) -> [f32; 4] {
        [self.walls, self.cards, self.spark, self.ground]
    }
}

/// The gemological studio lighting presets as a closed set of variants.
///
/// `LightTent` (UI index `0`) is the [`Default`]. Unrecognised persisted labels fall back to
/// it via [`Self::from_label`]. The declaration order is the postcard variant index and must
/// not change; [`Self::index`] and [`Self::ALL`] give the UI order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LightingPreset {
    Daylight,
    Incandescent,
    RingLights,
    DarkSpotlight,
    IsoHemisphere,
    #[default]
    LightTent,
    DaylightDome,
    /// A 365 nm UV lamp: the `Studio` rig geometry lit by a narrow Gaussian line, no
    /// ambient backdrop and no white balance (see [`LightingPreset::uses_white_balance`]).
    /// Declared after the earlier variants, which keeps their postcard indices fixed.
    UvLamp365,
    /// A 395 nm UV LED lamp (same rig as [`Self::UvLamp365`]); its tail reaches into the
    /// violet, so a non-fluorescent stone looks faintly violet under it.
    UvLamp395,
    /// Clear-sky daylight with a physically bright, small direct sun
    /// ([`LightingModel::DaylightSun`]). Declared last, keeping earlier postcard indices fixed.
    DaylightSun,
    /// ASET-style contrast view ([`LightingModel::Aset`]): the environment is coloured by
    /// the exit elevation, identity white balance, no adaptation.
    Aset,
    /// Jewellery shop: bright diffuse walls plus pinpoint spots (`LightTent` model).
    ShopLights,
    /// Window daylight: one broad soft source, dim room (`LightTent` model).
    WindowDaylight,
    /// White tray lit from below: bright uniform walls and ground (`LightTent` model).
    WhiteTray,
    /// Illuminant A (2856 K), the colour-change standard: the `Studio` rig, Planckian,
    /// Bradford white balance.
    IlluminantA,
}

/// Which environment the preset samples -- see this module's "Lighting models" doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightingModel {
    Studio,
    IsoHemisphere,
    LightTent,
    DaylightDome,
    Aset,
    DaylightSun,
}

impl LightingModel {
    /// The `u32` discriminant bound in `GpuTransportParams::studio_model`.
    #[must_use]
    pub const fn gpu_id(self) -> u32 {
        match self {
            Self::Studio => 0,
            Self::IsoHemisphere => 1,
            Self::LightTent => 2,
            Self::DaylightDome => 3,
            Self::Aset => 4,
            Self::DaylightSun => 5,
        }
    }
}

impl LightingPreset {
    /// All fifteen presets in UI order.
    ///
    /// Matches the `lighting_options` combo box in `apps/pleochroma-cut/ui/models/viewport.slint`:
    /// the light tent (the default), the grading tray, the tent variants, the daylight models,
    /// the product-photography rigs, the contrast view, then the UV lamps. The enum declaration
    /// order (the postcard variant index) is separate.
    pub const ALL: [Self; 15] = [
        Self::LightTent,
        Self::IsoHemisphere,
        Self::WhiteTray,
        Self::ShopLights,
        Self::WindowDaylight,
        Self::DaylightDome,
        Self::DaylightSun,
        Self::Daylight,
        Self::Incandescent,
        Self::IlluminantA,
        Self::RingLights,
        Self::DarkSpotlight,
        Self::Aset,
        Self::UvLamp365,
        Self::UvLamp395,
    ];

    /// This preset's color temperature and rig-intensity multiplier, read by both
    /// `sample_studio_environment` and `illuminant_temperature_k`.
    #[must_use]
    pub const fn params(self) -> LightingRigParams {
        match self {
            Self::Incandescent => LightingRigParams {
                temp_k: 3200.0,
                spot_mult: 1.2,
                tent: TentParams::DEFAULT,
            },
            Self::RingLights => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.6,
                tent: TentParams::DEFAULT,
            },
            Self::DarkSpotlight => LightingRigParams {
                temp_k: 6000.0,
                spot_mult: 2.4,
                tent: TentParams::DEFAULT,
            },
            // The light tent: the default look, with the identity `TentParams`.
            Self::LightTent => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
                tent: TentParams::DEFAULT,
            },
            // The three tent variants are D65, so `temp_k` does not affect their spectrum.
            // They differ in `tent` and in `spot_mult`, which scales the key and the spark.
            //
            // Jewellery shop: bright diffuse walls (0.42 at the girdle to 0.66 at the zenith),
            // the key as an overhead spot (1.4) and the spark as a pinpoint (5.0) on top, no
            // cards, a lit floor (0.05).
            Self::ShopLights => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
                tent: TentParams {
                    walls: 3.0,
                    cards: 0.0,
                    spark: 1.0,
                    ground: 0.05,
                    flat: 0.0,
                },
            },
            // Window daylight: a dim room (walls 0.056 to 0.088) lit by ONE broad soft
            // source, the key softbox cone (20 to 40 degrees wide at 1.4); no cards, no
            // spark. The key's elevation comes from the light pose (set it to 20-40 degrees).
            Self::WindowDaylight => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
                tent: TentParams {
                    walls: 0.4,
                    cards: 0.0,
                    spark: 0.0,
                    ground: 0.03,
                    flat: 0.0,
                },
            },
            // White tray lit from below: perfectly uniform walls (`flat` 1: 0.18 * 5.0 = 0.9
            // at every elevation) and a bright ground (0.8), so a window shows white as on
            // paper. `spot_mult` 0 removes the key softbox and the spark, which would be hot
            // spots on a uniform tray.
            Self::WhiteTray => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 0.0,
                tent: TentParams {
                    walls: 5.0,
                    cards: 0.0,
                    spark: 0.0,
                    ground: 0.8,
                    flat: 1.0,
                },
            },
            Self::IlluminantA => LightingRigParams {
                temp_k: 2856.0,
                spot_mult: 1.2,
                tent: TentParams::DEFAULT,
            },
            // The UV lamps are not Planckian: `temp_k` is unused by their spectrum and white
            // balance, so 6500 K is a placeholder.
            Self::Daylight
            | Self::IsoHemisphere
            | Self::DaylightDome
            | Self::DaylightSun
            | Self::Aset
            | Self::UvLamp365
            | Self::UvLamp395 => LightingRigParams {
                temp_k: 6500.0,
                spot_mult: 1.0,
                tent: TentParams::DEFAULT,
            },
        }
    }

    /// The user-facing display label. The Daylight preset is D65 (6500 K), and its label says so.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daylight => "D65 Daylight (6500K)",
            Self::Incandescent => "Incandescent (3200K)",
            Self::RingLights => "Gem Studio Ring Lights",
            Self::DarkSpotlight => "Dramatic Dark Spotlight",
            Self::IsoHemisphere => "Grading tray (D65 hemisphere + head shadow)",
            Self::LightTent => "Light tent + black cards",
            Self::DaylightDome => "Daylight sky (no sun)",
            Self::DaylightSun => "Daylight sky + direct sun",
            Self::Aset => "Contrast view (ASET-style)",
            Self::ShopLights => "Jewellery shop (diffuse + spots)",
            Self::WindowDaylight => "Window daylight",
            Self::WhiteTray => "White tray (lit from below)",
            Self::IlluminantA => "Incandescent A (2856K)",
            Self::UvLamp365 => "UV lamp 365 nm",
            Self::UvLamp395 => "UV lamp 395 nm",
        }
    }

    /// Parses a persisted or UI label into a preset. Unrecognised labels fall back to
    /// [`Self::LightTent`]. Older labels still resolve so saved settings load: both D65 labels
    /// map to [`Self::Daylight`] (the `5500K` one was mislabelled), and the earlier names of
    /// the lit models map to their current presets.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "D65 Daylight (6500K)" | "D65 Daylight (5500K)" => Self::Daylight,
            "Incandescent (3200K)" => Self::Incandescent,
            "Gem Studio Ring Lights" => Self::RingLights,
            "Dramatic Dark Spotlight" => Self::DarkSpotlight,
            "Grading tray (D65 hemisphere + head shadow)"
            | "ISO hemisphere"
            | "ISO hemisphere (GemRay-style)" => Self::IsoHemisphere,
            // "Light tent + black cards" and the legacy "Soft dome + ring lights" map to
            // `LightTent`, which is the fallback arm below.
            // A saved "Daylight sky + sun" label meant the dome.
            "Daylight sky (no sun)" | "Daylight sky + sun" | "Daylight dome + sun" => {
                Self::DaylightDome
            }
            "Daylight sky + direct sun" => Self::DaylightSun,
            "Contrast view (ASET-style)" => Self::Aset,
            "Jewellery shop (diffuse + spots)" => Self::ShopLights,
            "Window daylight" => Self::WindowDaylight,
            "White tray (lit from below)" => Self::WhiteTray,
            "Incandescent A (2856K)" => Self::IlluminantA,
            "UV lamp 365 nm" => Self::UvLamp365,
            "UV lamp 395 nm" => Self::UvLamp395,
            _ => Self::LightTent,
        }
    }

    /// The index into [`Self::ALL`] / the UI combo box's `lighting_options` list.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::LightTent => 0,
            Self::IsoHemisphere => 1,
            Self::WhiteTray => 2,
            Self::ShopLights => 3,
            Self::WindowDaylight => 4,
            Self::DaylightDome => 5,
            Self::DaylightSun => 6,
            Self::Daylight => 7,
            Self::Incandescent => 8,
            Self::IlluminantA => 9,
            Self::RingLights => 10,
            Self::DarkSpotlight => 11,
            Self::Aset => 12,
            Self::UvLamp365 => 13,
            Self::UvLamp395 => 14,
        }
    }

    /// Inverse of [`Self::index`]; out-of-range indices fall back to [`Self::LightTent`]
    /// (the default), matching [`Self::from_label`]'s fallback. Index `0` is `LightTent`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            // Index 0 is `LightTent`, which is the fallback arm below.
            1 => Self::IsoHemisphere,
            2 => Self::WhiteTray,
            3 => Self::ShopLights,
            4 => Self::WindowDaylight,
            5 => Self::DaylightDome,
            6 => Self::DaylightSun,
            7 => Self::Daylight,
            8 => Self::Incandescent,
            9 => Self::IlluminantA,
            10 => Self::RingLights,
            11 => Self::DarkSpotlight,
            12 => Self::Aset,
            13 => Self::UvLamp365,
            14 => Self::UvLamp395,
            _ => Self::LightTent,
        }
    }

    /// Which environment model this preset samples.
    #[must_use]
    pub const fn model(self) -> LightingModel {
        match self {
            Self::Daylight
            | Self::Incandescent
            | Self::RingLights
            | Self::DarkSpotlight
            | Self::IlluminantA
            | Self::UvLamp365
            | Self::UvLamp395 => LightingModel::Studio,
            Self::IsoHemisphere => LightingModel::IsoHemisphere,
            // The three variants differ from the tent only in `params().tent`.
            Self::LightTent | Self::ShopLights | Self::WindowDaylight | Self::WhiteTray => {
                LightingModel::LightTent
            }
            Self::DaylightDome => LightingModel::DaylightDome,
            Self::Aset => LightingModel::Aset,
            Self::DaylightSun => LightingModel::DaylightSun,
        }
    }

    /// Whether the illuminant is the tabulated CIE D65 curve rather than a Planckian fit.
    #[must_use]
    pub const fn uses_d65(self) -> bool {
        matches!(
            self,
            Self::Daylight
                | Self::IsoHemisphere
                | Self::DaylightDome
                | Self::DaylightSun
                | Self::ShopLights
                | Self::WindowDaylight
                | Self::WhiteTray
                // The contrast view keeps the D65 flag; its zone bands are evaluated from the
                // wavelength (`rig::aset_spec_input`), not from this curve.
                | Self::Aset
        )
    }

    /// Whether this preset is a UV lamp ([`Self::UvLamp365`], [`Self::UvLamp395`]): a
    /// narrow Gaussian line source, CPU-only (`scene_routes_to_gpu` is `false`).
    #[must_use]
    pub const fn is_uv_lamp(self) -> bool {
        matches!(self, Self::UvLamp365 | Self::UvLamp395)
    }

    /// Centre wavelength and FWHM (nm) of a UV lamp's Gaussian line, `None` for every
    /// other preset: 365 nm / 10 nm and 395 nm / 12 nm.
    #[must_use]
    pub const fn uv_line(self) -> Option<(f32, f32)> {
        match self {
            Self::UvLamp365 => Some((365.0, 10.0)),
            Self::UvLamp395 => Some((395.0, 12.0)),
            _ => None,
        }
    }

    /// Whether the von-Kries white balance toward D65 applies: only the Planckian
    /// presets (`Incandescent`, `IlluminantA`, `RingLights`, `DarkSpotlight`, `LightTent`). The D65
    /// presets are already D65-white (identity), and the UV lamps have no meaningful
    /// white point at all (identity: the stone shows the lamp's own color).
    #[must_use]
    pub const fn uses_white_balance(self) -> bool {
        !self.uses_d65() && !self.is_uv_lamp() && !matches!(self, Self::Aset)
    }

    /// Whether the `Studio` rig adds its dim ambient backdrop term. `false` for the UV
    /// lamps: a dark room with only the lamp lit.
    #[must_use]
    pub const fn has_ambient_fill(self) -> bool {
        !self.is_uv_lamp()
    }

    /// Relative spectral power of this preset's illuminant at `lambda_nm`: the
    /// tabulated CIE D65 curve where [`Self::uses_d65`], a unit-peak Gaussian line for
    /// the UV lamps, else a Planckian fit at the preset's color temperature.
    #[must_use]
    pub fn spectral_power(self, lambda_nm: f32) -> f32 {
        self.illuminant_spectrum().power(lambda_nm)
    }

    /// This preset's illuminant curve with the temperature-only Planck constants resolved once.
    ///
    /// `.power(l)` equals `Self::spectral_power(l)` but reuses the per-temperature constants.
    /// Hoisting them can change the last bits of the transcendental evaluations.
    fn illuminant_spectrum(self) -> IlluminantSpectrum {
        if self.uses_d65() {
            IlluminantSpectrum::D65
        } else if let Some((centre_nm, fwhm_nm)) = self.uv_line() {
            IlluminantSpectrum::Gaussian {
                centre_nm,
                sigma_nm: fwhm_nm / (2.0 * (2.0 * std::f32::consts::LN_2).sqrt()),
            }
        } else {
            IlluminantSpectrum::Blackbody(BlackbodyNorm::new(self.params().temp_k))
        }
    }

    /// An [`EnvironmentSource::Studio`] for this preset with the default backdrop, glare and
    /// head shadow, e.g. `LightingPreset::RingLights.studio(1.0, 0.85, 0.95)`.
    #[must_use]
    pub const fn studio(
        self,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
    ) -> EnvironmentSource<'static> {
        EnvironmentSource::Studio {
            preset: self,
            exposure,
            light_yaw,
            light_pitch,
            backdrop: 0.0,
            surface_glare: 1.0,
            head_shadow_deg: DEFAULT_HEAD_SHADOW_DEG,
            tray_shadow: None,
            tray_mirror: None,
        }
    }
}

/// Studio-rig radiance along `dir` at wavelength `lambda_nm`, with no observer. The lit
/// models' head shadow is off here; see [`sample_studio_environment_observed`].
///
/// Builds a [`StudioRig`](crate::optics::studio_rig::StudioRig) on every call. For many
/// directions under one light pose, build the rig once and call
/// [`sample_studio_environment_with_rig`].
#[must_use]
pub fn sample_studio_environment(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> f32 {
    sample_studio_environment_observed(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        light_yaw,
        light_pitch,
        Vec3::ZERO,
    )
}

/// [`sample_studio_environment`] with an observer in the scene.
///
/// `observer` is the unit direction from the stone towards the eye (the reverse of the
/// pixel's primary ray). The lit models darken exit directions inside the head-shadow cone
/// around it, which gives the dark table reflections of a face-up stone. `Studio` and `Aset`
/// ignore it, and `Vec3::ZERO` disables the shadow (every dot product is `0.0`, outside the
/// cone).
#[must_use]
pub fn sample_studio_environment_observed(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    observer: Vec3,
) -> f32 {
    // Light directions come from the shared `StudioRig`, the same construction the GPU
    // check and the per-ray trace use.
    let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
    sample_studio_environment_with_rig(dir, lambda_nm, lighting_preset, exposure, &rig, observer)
}
