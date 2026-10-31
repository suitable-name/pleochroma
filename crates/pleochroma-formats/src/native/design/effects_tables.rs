//! The two nested tables of `[render_effects]`: `[render_effects.inclusions]` (silk and other
//! oriented inclusions) and `[render_effects.structure]` (play-of-color).
//!
//! Plain mirrors with string names for the enumerated choices; `pleochroma-cut-core` converts
//! them to and from its own types, since only it can see both sides. Each table is written only
//! when its feature is on, so a design without the feature keeps its bytes. Every key has a serde
//! default and unknown keys are kept, so a table from an older or newer revision still loads.

/// The default density of an inclusion preset (the catalogue strengths apply unchanged).
const DEFAULT_DENSITY: f64 = 0.35;

/// The default seed of a play-of-color map.
const DEFAULT_SEED: u32 = 12_345;

const fn default_density() -> f64 {
    DEFAULT_DENSITY
}

const fn default_seed() -> u32 {
    DEFAULT_SEED
}

const fn default_color_low_nm() -> f64 {
    440.0
}

const fn default_color_high_nm() -> f64 {
    680.0
}

const fn default_purity() -> f64 {
    0.05
}

const fn default_coverage() -> f64 {
    0.8
}

/// The `[render_effects.inclusions]` table: the design's inclusion choice.
///
/// The families themselves are per material (the catalogue, or a custom material's own); the
/// design stores the preset name, how dense it is and a few look controls.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InclusionEffectsTable {
    /// The catalogue preset by name, for example `"Fine Silk"`.
    #[serde(default)]
    pub preset: String,
    /// `0..=1`; `0.35` is the catalogue strength.
    #[serde(default = "default_density")]
    pub density: f64,
    /// The Silk Turn in degrees about the c axis; written only when it differs from the stored
    /// Second Axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_deg: Option<f64>,
    /// `"Soft"` or `"Sharp"`; written only when the preset's own cone width is overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheen: Option<String>,
    /// `"Grey"`, `"Slightly Blue"` or `"Blue"`; written only when the preset's own color is
    /// overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// The `[render_effects.structure]` table: the design's play-of-color choice.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructureEffectsTable {
    /// `"Pinfire"`, `"Harlequin"` or `"Broad Flash"`.
    #[serde(default)]
    pub pattern: String,
    /// The seed of the domain map, stored with the design (a Reseed draws a new one).
    #[serde(default = "default_seed")]
    pub seed: u32,
    /// The domain cell size in model units; written only when it overrides the pattern's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// The shortest normal-incidence Bragg wavelength, nanometres.
    #[serde(default = "default_color_low_nm")]
    pub color_low_nm: f64,
    /// The longest normal-incidence Bragg wavelength, nanometres.
    #[serde(default = "default_color_high_nm")]
    pub color_high_nm: f64,
    /// The relative bandwidth ("Color Purity").
    #[serde(default = "default_purity")]
    pub purity: f64,
    /// The fraction of domains that reflect ("Coverage").
    #[serde(default = "default_coverage")]
    pub coverage: f64,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}
