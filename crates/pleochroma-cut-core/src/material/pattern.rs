//! Hand-made color patterns of a custom material: the stored form, its templates and the
//! conversion to the zoned absorption the tracers render.
//!
//! A [`ColorPattern`] is a base zone plus up to [`MAX_PATTERN_ZONES`] shaped zones, each with a
//! color. The color is stored as the picked `[L*, C*, h]` next to the seven-band rows the
//! body-color editor solved for it ([`ZoneColor`]); [`ColorPattern::to_zoned`] never solves, it
//! wraps the stored rows as per-millimeter tensors ([`bands_from_rows`], the same rule as
//! `GemMaterial::with_body_color_bands`). A zone can hold two colors ([`PatternColor::Dichroic`]):
//! the zone tensor is then uniaxial, `o_ray` = the Along color and `e_ray` = the Across color. A
//! zone of a biaxial stone can hold three ([`PatternColor::Trichroic`]), or a color variant of the
//! stone's material ([`PatternColor::Variant`], a per-millimeter snapshot of its tensor).
//!
//! # Geometry
//!
//! Zone shapes are written in the pattern's own frame, where the pattern axis is `+Y`. The
//! [`PatternAxis`] becomes [`ZonedAbsorption::frame`]: `rotation = R_y(turn) * R_x(tilt)`, i.e.
//! the tilt first rotates `+Y` about `+X` (towards `+Z`), then the turn rotates the result about
//! `+Y` (`+Z` towards `+X`). `0 / 0` is the table normal, which is also every custom material's
//! c axis. With `relative_to_stone` the lengths are for a stone one unit wide and resolve to
//! millimeters through the existing `rough_plan::zoned_plan::resolve_relative`.
//!
//! # What this module does not do
//!
//! It never solves a color: a template stores its colors as `[L*, C*, h]` only, with no band
//! rows ([`ColorPattern::unsolved_colors`] lists what the application still has to solve and
//! [`ColorPattern::set_color_bands`] stores the result), and [`ColorPattern::to_zoned`] refuses a
//! pattern with an unsolved color.

mod apply;
mod codec;
mod convert;
mod error;
mod follow;
mod templates;
mod typical;
mod variant;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod typical_tests;
#[cfg(test)]
mod variant_tests;

pub use apply::apply_color_pattern;
pub use codec::{MAX_PATTERN_TEXT_BYTES, PatternCodecError, decode_pattern, encode_pattern};
pub use convert::{
    EXACT_COLOR_KEY, EXACT_COLOR_LABEL, bands_from_rows, pattern_holds_tensor, rows_carry_bands,
    rows_carry_tensor, rows_from_bands,
};
pub use error::{PatternError, ZoneProblem};
pub use follow::{
    crystal_axis_frame, follow_c_axis, follow_noted_pattern, layers_across_axis,
    wants_axis_through_table,
};
pub use templates::template;
pub use typical::{OwnColor, TypicalPattern, typical_index, typical_patterns};
pub use variant::{
    PatternTensorRows, ZoneRankFit, allowed_rank, per_mm_factor, per_mm_rows, zone_rank_fit,
};

use glam::{DQuat, DVec3};
use pleochroma::optics::{
    absorption::AbsorptionTensor,
    zoning::{MAX_ZONES, Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
};
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

/// The newest format version of a stored [`ColorPattern`] this build reads.
///
/// A reader refuses a pattern with a newer version instead of guessing, and accepts every version
/// from [`PATTERN_BASE_FORMAT_VERSION`] up to this one.
///
/// Version 2 added [`PatternColor::Variant`] and [`PatternColor::Trichroic`]. Serde reads a file
/// written before them unchanged (the old variants keep their positions), so a pattern only
/// carries version 2 while it holds one of them ([`ColorPattern::required_format_version`]); every
/// other pattern is still written as version 1 and an older build still reads it.
pub const PATTERN_FORMAT_VERSION: u32 = 2;

/// The format version of a pattern that holds only [`PatternColor::Single`] and
/// [`PatternColor::Dichroic`] colors.
///
/// It is the first release's, which every template, typical pattern and import without a new
/// color kind carries.
pub const PATTERN_BASE_FORMAT_VERSION: u32 = 1;

/// The `[L*, C*, h]` that stands in for a [`PatternColor::Variant`] where a slider seed is needed
/// ([`ColorPattern::base_lch`]): a neutral gray, since a variant has no sliders.
pub const VARIANT_SEED_LCH: [f64; 3] = [50.0, 0.0, 0.0];

/// The most zones a pattern holds besides the base (the zoning kernels' limit).
pub const MAX_PATTERN_ZONES: usize = MAX_ZONES;

/// A color pattern: a base zone plus ordered shaped zones, later ones overriding earlier ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorPattern {
    /// The format version: [`PATTERN_BASE_FORMAT_VERSION`], or [`PATTERN_FORMAT_VERSION`] while
    /// the pattern holds a variant or a three-color zone
    /// ([`ColorPattern::required_format_version`]).
    pub version: u32,
    /// The template this pattern started from ("Reset to Template"), if any.
    pub template: Option<TemplateId>,
    /// The base zone: everywhere that no other zone covers. Its shape is a placeholder and
    /// never used.
    pub base: PatternZone,
    /// The shaped zones, at most [`MAX_PATTERN_ZONES`]; later ones override earlier ones.
    pub zones: Vec<PatternZone>,
    /// Edge softness: `0` is sharp; otherwise a fraction of the stone width when
    /// [`Self::relative_to_stone`], else millimeters.
    pub softness: f32,
    /// How the pattern is oriented in the stone.
    pub axis: PatternAxis,
    /// Whether the zone lengths are for a stone one unit wide ("Scales With Stone Size") or in
    /// millimeters ("Fixed in mm"). This is the existing `material_zoning` flag.
    pub relative_to_stone: bool,
}

/// One zone of a pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternZone {
    /// The name shown in the editor ("Rim", "Core", "Top Half").
    pub name: String,
    /// Where the zone applies: for a unit-wide stone when the pattern is relative, else
    /// millimeters. Ignored for the base zone.
    pub shape: ZoneShape,
    /// What color the zone has.
    pub color: PatternColor,
}

/// The color of a zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatternColor {
    /// One color: the picked color and the band rows solved for it.
    Single(ZoneColor),
    /// Two colors: `along` is the ordinary-ray band set (seen alone along the crystal axis),
    /// `across` the extraordinary-ray band set (mixed with `along` when looking across the
    /// axis).
    Dichroic {
        /// The ordinary-ray color.
        along: ZoneColor,
        /// The extraordinary-ray color.
        across: ZoneColor,
    },
    // The variants below were added with format version 2, so they stay last: the wire form of
    // the two above (a variant index) does not move, and a stored pattern that holds only them
    // reads and writes exactly as it did.
    /// A color variant of the stone's material, copied into the pattern (a snapshot, like the
    /// design's `VariantChoice`, so the pattern renders without the variant library).
    /// `absorption` is per millimeter: a variant stored per model unit is converted when it
    /// becomes a zone color ([`PatternColor::from_variant`]).
    Variant {
        /// The variant's stable key (`"sapphire.yellow"`, `"user.<uuid>"`).
        key: String,
        /// The variant's display label ("Yellow").
        label: String,
        /// The per-axis tensor, per millimeter.
        absorption: PatternTensorRows,
    },
    /// Three colors, one per principal direction of a biaxial crystal: `alpha`, `beta` and
    /// `gamma` become the `o_ray`, `beta_ray` and `e_ray` band sets of a trichroic tensor.
    Trichroic {
        /// The alpha-direction color (`o_ray`).
        alpha: ZoneColor,
        /// The beta-direction color (`beta_ray`).
        beta: ZoneColor,
        /// The gamma-direction color (`e_ray`).
        gamma: ZoneColor,
    },
}

/// One picked color: its sliders and the band rows the body-color editor solved for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneColor {
    /// The picked `[L*, C*, h]` (so the editor reopens on the same sliders).
    pub lch: [f64; 3],
    /// The band rows exactly as the material row's `absorption_bands_json` rows:
    /// `[centre_nm, width_nm, amplitude_per_mm]` each, a width above 500 marking an energy band.
    /// Empty and not [`Self::solved`] means "not solved yet".
    pub bands: Vec<[f32; 3]>,
    /// `true` when the color was solved and the solution is no bands at all (a colorless
    /// zone). A color with band rows is solved whatever this says ([`Self::is_solved`]).
    #[serde(default)]
    pub solved: bool,
}

impl ZoneColor {
    /// A color with only its sliders: the band rows still have to be solved.
    #[must_use]
    pub const fn unsolved(lch: [f64; 3]) -> Self {
        Self {
            lch,
            bands: Vec::new(),
            solved: false,
        }
    }

    /// A solved color. `bands` may be empty (a colorless zone).
    #[must_use]
    pub const fn solved(lch: [f64; 3], bands: Vec<[f32; 3]>) -> Self {
        Self {
            lch,
            bands,
            solved: true,
        }
    }

    /// Whether the band rows are known (see [`Self::solved`]).
    #[must_use]
    pub const fn is_solved(&self) -> bool {
        self.solved || !self.bands.is_empty()
    }
}

/// How the pattern is oriented in the stone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PatternAxis {
    /// Tilt and turn relative to the stone: `rotation = R_y(turn) * R_x(tilt)` (see the module
    /// notes). `0 / 0` is the table normal (`+Y`).
    Fixed {
        /// Rotation of the pattern axis about `+X`, radians.
        tilt_rad: f64,
        /// Rotation about `+Y` applied after the tilt, radians.
        turn_rad: f64,
    },
    /// An arbitrary frame, as imported zones (the Rough Color wizard's) have one:
    /// `rotation` is the unit quaternion `[x, y, z, w]` and `translation` is in the same
    /// length unit as the pattern's geometry.
    Frame {
        /// The unit quaternion, `[x, y, z, w]`.
        rotation: [f64; 4],
        /// The translation.
        translation: [f64; 3],
    },
    /// The pattern axis is the stone's Crystal Axis ("Follows Crystal Axis"): the zones turn
    /// with the axis the stone is oriented to (the design's own setting, else the material's
    /// default), see [`crystal_axis_frame`]. The stored zones carry [`Self::frame`], the table
    /// normal, for every consumer that does not know the stone's axis; the application
    /// re-frames them wherever it does ([`follow_c_axis`]).
    ///
    /// A pattern file written before this mode existed never holds it, so every old pattern
    /// reads and writes exactly as it did.
    FollowsCrystalAxis,
}

impl PatternAxis {
    /// The pattern axis along the table normal (every custom material's c axis).
    pub const TABLE_NORMAL: Self = Self::Fixed {
        tilt_rad: 0.0,
        turn_rad: 0.0,
    };
    /// The pattern axis in the table plane, along `+Z`.
    pub const ACROSS: Self = Self::Fixed {
        tilt_rad: FRAC_PI_2,
        turn_rad: 0.0,
    };
    /// The pattern axis in the table plane, turned a quarter turn from [`Self::ACROSS`] (along
    /// `+X`).
    pub const ACROSS_TURNED: Self = Self::Fixed {
        tilt_rad: FRAC_PI_2,
        turn_rad: FRAC_PI_2,
    };

    /// Whether the zones turn with the stone's Crystal Axis ([`Self::FollowsCrystalAxis`]).
    #[must_use]
    pub const fn follows_crystal_axis(&self) -> bool {
        matches!(self, Self::FollowsCrystalAxis)
    }

    /// The zone frame this axis stands for. [`Self::FollowsCrystalAxis`] stands for the table
    /// normal here (the stored default); the stone's axis re-frames it ([`follow_c_axis`]).
    #[must_use]
    pub fn frame(&self) -> ZoneFrame {
        match *self {
            Self::FollowsCrystalAxis => ZoneFrame::IDENTITY,
            Self::Fixed { tilt_rad, turn_rad } => ZoneFrame {
                rotation: (DQuat::from_axis_angle(DVec3::Y, turn_rad)
                    * DQuat::from_axis_angle(DVec3::X, tilt_rad))
                .normalize(),
                translation: DVec3::ZERO,
            },
            Self::Frame {
                rotation,
                translation,
            } => ZoneFrame {
                rotation: DQuat::from_array(rotation),
                translation: DVec3::from_array(translation),
            },
        }
    }

    /// The axis of an existing frame: exactly [`Self::TABLE_NORMAL`] for the identity, else
    /// [`Self::Frame`] (the frame is kept bit for bit).
    #[must_use]
    pub fn from_frame(frame: &ZoneFrame) -> Self {
        if frame.rotation == DQuat::IDENTITY && frame.translation == DVec3::ZERO {
            Self::TABLE_NORMAL
        } else {
            Self::Frame {
                rotation: frame.rotation.to_array(),
                translation: frame.translation.to_array(),
            }
        }
    }
}

/// The built-in starting points of a pattern ([`template`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TemplateId {
    /// One color everywhere (stores no zones).
    Uniform,
    /// Two halves.
    Bicolor,
    /// Three layers.
    Tricolor,
    /// Four stripes on a background.
    Banded,
    /// A triangular core inside a rim.
    Watermelon,
    /// [`Self::Watermelon`] with the colors swapped.
    ReverseWatermelon,
    /// A round core inside a rim.
    CoreAndRim,
    /// Two halves split by a plane through the axis.
    Split,
    /// Two wedges around the axis.
    Sectors,
    // The variants below were added after the first release, so they stay last: the wire form
    // of the earlier ones (a variant index) does not move, and a stored pattern that names one
    // of them (`"template":"Watermelon"`) reads as it always did.
    /// Concentric 6-sided growth zones around the axis (corundum, beryl). Not in [`Self::ALL`]:
    /// it is offered as typical zoning of the species ([`typical_patterns`]).
    HexagonalBanding,
    /// A 6-sided core in a rim of another hue (parti-color sapphire). Typical zoning only.
    PartiColor,
    /// One band through the axis, off center (sapphire). Typical zoning only.
    ColorBand,
}

impl TemplateId {
    /// The templates that only exist as typical zoning of a species. They are not in
    /// [`Self::ALL`] (the generic template and preset lists), but [`template`] still builds them
    /// for a blue sapphire.
    pub const TYPICAL_ONLY: [Self; 3] = [Self::HexagonalBanding, Self::PartiColor, Self::ColorBand];

    /// Every generic template, in menu order.
    pub const ALL: [Self; 9] = [
        Self::Uniform,
        Self::Bicolor,
        Self::Tricolor,
        Self::Banded,
        Self::Watermelon,
        Self::ReverseWatermelon,
        Self::CoreAndRim,
        Self::Split,
        Self::Sectors,
    ];

    /// The name shown in the editor.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Uniform => "Uniform",
            Self::Bicolor => "Bicolor",
            Self::Tricolor => "Tricolor",
            Self::Banded => "Banded",
            Self::Watermelon => "Watermelon",
            Self::ReverseWatermelon => "Reverse Watermelon",
            Self::CoreAndRim => "Core and Rim",
            Self::Split => "Split",
            Self::Sectors => "Sectors",
            Self::HexagonalBanding => "Hexagonal Banding",
            Self::PartiColor => "Parti Color",
            Self::ColorBand => "Color Band",
        }
    }
}

/// One color of a pattern, addressed for [`ColorPattern::color`] and the solver hand-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorRef {
    /// The zone: 0 is the base, then the shaped zones in order (the zoning length-array index).
    pub zone: usize,
    /// Which of the zone's colors.
    pub slot: ColorSlot,
}

/// Which color of a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSlot {
    /// The one color of a [`PatternColor::Single`] zone.
    Single,
    /// The Along color of a [`PatternColor::Dichroic`] zone.
    Along,
    /// The Across color of a [`PatternColor::Dichroic`] zone.
    Across,
    /// The alpha color of a [`PatternColor::Trichroic`] zone.
    Alpha,
    /// The beta color of a [`PatternColor::Trichroic`] zone.
    Beta,
    /// The gamma color of a [`PatternColor::Trichroic`] zone.
    Gamma,
}

/// How many picked colors a zone holds: the custom color forms of the zone color picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorForm {
    /// One color ([`PatternColor::Single`]).
    Single,
    /// Along and Across ([`PatternColor::Dichroic`]).
    Dichroic,
    /// Alpha, Beta and Gamma ([`PatternColor::Trichroic`]).
    Trichroic,
}

impl PatternZone {
    /// A base zone: `shape` is a placeholder that is never used.
    #[must_use]
    pub fn base(name: &str, color: PatternColor) -> Self {
        Self {
            name: name.to_owned(),
            shape: ZoneShape::HalfSpace {
                normal: DVec3::Y,
                offset: 0.0,
            },
            color,
        }
    }
}

impl ColorPattern {
    /// The zone at `index` (0 is the base, then the shaped zones in order).
    #[must_use]
    pub fn zone(&self, index: usize) -> Option<&PatternZone> {
        if index == 0 {
            Some(&self.base)
        } else {
            self.zones.get(index - 1)
        }
    }

    /// As [`Self::zone`], mutable.
    pub fn zone_mut(&mut self, index: usize) -> Option<&mut PatternZone> {
        if index == 0 {
            Some(&mut self.base)
        } else {
            self.zones.get_mut(index - 1)
        }
    }

    /// The color at `at`, or `None` when the zone does not exist or does not hold that slot.
    #[must_use]
    pub fn color(&self, at: ColorRef) -> Option<&ZoneColor> {
        match (&self.zone(at.zone)?.color, at.slot) {
            (PatternColor::Single(color), ColorSlot::Single)
            | (PatternColor::Dichroic { along: color, .. }, ColorSlot::Along)
            | (PatternColor::Dichroic { across: color, .. }, ColorSlot::Across)
            | (PatternColor::Trichroic { alpha: color, .. }, ColorSlot::Alpha)
            | (PatternColor::Trichroic { beta: color, .. }, ColorSlot::Beta)
            | (PatternColor::Trichroic { gamma: color, .. }, ColorSlot::Gamma) => Some(color),
            _ => None,
        }
    }

    /// As [`Self::color`], mutable.
    pub fn color_mut(&mut self, at: ColorRef) -> Option<&mut ZoneColor> {
        match (&mut self.zone_mut(at.zone)?.color, at.slot) {
            (PatternColor::Single(color), ColorSlot::Single)
            | (PatternColor::Dichroic { along: color, .. }, ColorSlot::Along)
            | (PatternColor::Dichroic { across: color, .. }, ColorSlot::Across)
            | (PatternColor::Trichroic { alpha: color, .. }, ColorSlot::Alpha)
            | (PatternColor::Trichroic { beta: color, .. }, ColorSlot::Beta)
            | (PatternColor::Trichroic { gamma: color, .. }, ColorSlot::Gamma) => Some(color),
            _ => None,
        }
    }

    /// The picked `[L*, C*, h]` of the base zone (the Along or Alpha color of a two- or
    /// three-color base): the color the stone has outside every zone. A variant has no sliders:
    /// [`VARIANT_SEED_LCH`], a neutral gray.
    #[must_use]
    pub const fn base_lch(&self) -> [f64; 3] {
        match &self.base.color {
            PatternColor::Single(color)
            | PatternColor::Dichroic { along: color, .. }
            | PatternColor::Trichroic { alpha: color, .. } => color.lch,
            PatternColor::Variant { .. } => VARIANT_SEED_LCH,
        }
    }

    /// Replaces the color at `at`. `false` when there is no such color.
    pub fn set_color(&mut self, at: ColorRef, color: ZoneColor) -> bool {
        self.color_mut(at).map(|slot| *slot = color).is_some()
    }

    /// Stores the solved band rows of the color at `at` (the sliders stay). An empty `bands`
    /// is a solved colorless zone. `false` when there is no such color.
    pub fn set_color_bands(&mut self, at: ColorRef, bands: Vec<[f32; 3]>) -> bool {
        self.color_mut(at)
            .map(|color| {
                color.bands = bands;
                color.solved = true;
            })
            .is_some()
    }

    /// Every color whose band rows are not known yet (a template's colors, until the
    /// application solved them), base zone first.
    #[must_use]
    pub fn unsolved_colors(&self) -> Vec<ColorRef> {
        let mut out = Vec::new();
        for index in 0..=self.zones.len() {
            let Some(zone) = self.zone(index) else {
                continue;
            };
            let slots: Vec<(ColorSlot, &ZoneColor)> = match &zone.color {
                PatternColor::Single(color) => vec![(ColorSlot::Single, color)],
                PatternColor::Dichroic { along, across } => {
                    vec![(ColorSlot::Along, along), (ColorSlot::Across, across)]
                }
                PatternColor::Trichroic { alpha, beta, gamma } => vec![
                    (ColorSlot::Alpha, alpha),
                    (ColorSlot::Beta, beta),
                    (ColorSlot::Gamma, gamma),
                ],
                // A variant is always solved: its rows are stored.
                PatternColor::Variant { .. } => Vec::new(),
            };
            for (slot, color) in slots {
                if !color.is_solved() {
                    out.push(ColorRef { zone: index, slot });
                }
            }
        }
        out
    }

    /// Turns zone `index` into a two-color zone (`on`, both colors start as the current one)
    /// or back into a one-color zone (the Along color stays). `false` when there is no such
    /// zone.
    pub fn set_two_colors(&mut self, index: usize, on: bool) -> bool {
        let Some(zone) = self.zone_mut(index) else {
            return false;
        };
        zone.color = match std::mem::replace(
            &mut zone.color,
            PatternColor::Single(ZoneColor::unsolved([0.0; 3])),
        ) {
            PatternColor::Single(color) if on => PatternColor::Dichroic {
                along: color.clone(),
                across: color,
            },
            PatternColor::Dichroic { along, .. } if !on => PatternColor::Single(along),
            other => other,
        };
        true
    }

    /// Turns zone `index` into a picked color of `form`, keeping the colors it has: a one-color
    /// zone repeats its color in every slot, a two-color zone becomes alpha = Along,
    /// gamma = Across with beta = Along (a uniaxial tensor, which `AbsorptionTensor::biaxial`
    /// reproduces exactly when alpha equals beta), a three-color zone going down keeps alpha
    /// (and gamma for two colors). `false` when there is no such zone or it holds a variant
    /// (turn it into a picked color first, [`PatternColor::to_custom`]).
    pub fn set_color_form(&mut self, index: usize, form: ColorForm) -> bool {
        let Some(zone) = self.zone_mut(index) else {
            return false;
        };
        let (first, last) = match &zone.color {
            PatternColor::Single(color) => (color.clone(), color.clone()),
            PatternColor::Dichroic { along, across } => (along.clone(), across.clone()),
            PatternColor::Trichroic { alpha, gamma, .. } => (alpha.clone(), gamma.clone()),
            PatternColor::Variant { .. } => return false,
        };
        zone.color = match form {
            ColorForm::Single => PatternColor::Single(first),
            ColorForm::Dichroic => PatternColor::Dichroic {
                along: first,
                across: last,
            },
            ColorForm::Trichroic => PatternColor::Trichroic {
                alpha: first.clone(),
                beta: first,
                gamma: last,
            },
        };
        true
    }

    /// The lowest format version that can hold this pattern: [`PATTERN_FORMAT_VERSION`] while a
    /// zone holds a [`PatternColor::Variant`] or [`PatternColor::Trichroic`] color (an older
    /// build refuses it with the "newer version" sentence instead of failing to parse),
    /// [`PATTERN_BASE_FORMAT_VERSION`] otherwise.
    #[must_use]
    pub fn required_format_version(&self) -> u32 {
        let new_kind = std::iter::once(&self.base).chain(&self.zones).any(|zone| {
            matches!(
                zone.color,
                PatternColor::Variant { .. } | PatternColor::Trichroic { .. }
            )
        });
        if new_kind {
            PATTERN_FORMAT_VERSION
        } else {
            PATTERN_BASE_FORMAT_VERSION
        }
    }

    /// The version this pattern is written with: its own, never below
    /// [`Self::required_format_version`] (a pattern that was loaded as a newer version keeps it).
    #[must_use]
    pub fn format_version(&self) -> u32 {
        self.version.max(self.required_format_version())
    }

    /// This pattern with [`Self::version`] set to [`Self::required_format_version`]: what a
    /// builder or an editor calls after it changed zone colors.
    #[must_use]
    pub fn with_required_version(mut self) -> Self {
        self.version = self.required_format_version();
        self
    }

    /// The same pattern with every length (zone geometry and softness) multiplied by `factor`
    /// and the "relative to stone" flag set to `relative`. Colors and the axis are unchanged.
    fn rescaled(&self, factor: f64, relative: bool) -> Self {
        let clear = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()));
        let mut probe = ZonedAbsorption::new(clear.clone());
        probe.boundary_softness_mm = self.softness;
        probe.zones = self
            .zones
            .iter()
            .map(|zone| Zone {
                shape: zone.shape.clone(),
                absorption: clear.clone(),
            })
            .collect();
        // The one scaling rule of zone geometry, so the two forms cannot drift apart.
        let scaled = probe.scaled(factor);
        let mut out = self.clone();
        out.softness = scaled.boundary_softness_mm;
        out.relative_to_stone = relative;
        for (zone, scaled_zone) in out.zones.iter_mut().zip(scaled.zones) {
            zone.shape = scaled_zone.shape;
        }
        out
    }

    /// The pattern with its geometry in millimeters for a stone `stone_width_mm` wide, flagged
    /// "Fixed in mm". `None` for a width that is not positive and finite.
    #[must_use]
    pub fn to_fixed_mm(&self, stone_width_mm: f64) -> Option<Self> {
        (stone_width_mm.is_finite() && stone_width_mm > 0.0)
            .then(|| self.rescaled(stone_width_mm, false))
    }

    /// The inverse of [`Self::to_fixed_mm`]: geometry for a unit-wide stone, flagged "Scales
    /// With Stone Size". `None` for a width that is not positive and finite.
    #[must_use]
    pub fn to_relative(&self, stone_width_mm: f64) -> Option<Self> {
        (stone_width_mm.is_finite() && stone_width_mm > 0.0)
            .then(|| self.rescaled(1.0 / stone_width_mm, true))
    }
}
