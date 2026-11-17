//! The `[material.custom]` snapshot table of the native document: everything a custom
//! material needs to be rebuilt on load. [`super::schema`] re-exports the type.

/// The `[material.custom]` snapshot for a design's custom catalogue material.
///
/// Holds everything needed to rebuild the material on load. Without it, a file naming a
/// custom material resolves only on the machine that has that library row, and elsewhere
/// the name would silently fall back to Diamond. Every field is a plain primitive, since
/// this crate has no dependency on `pleochroma`. Building the snapshot from a
/// `GemMaterial`, and registering it again when the name fails to resolve, is
/// `pleochroma-cut-core`'s job.
///
/// `crystal_system` and `optical_character` hold `Debug`-style names (e.g. `"Trigonal"`,
/// `"BiaxialPositive"`). When a name is empty or unrecognised, `GemMaterial::new_custom`
/// infers the character from the sign of `birefringence_delta`. A usable
/// `biaxial_delta_beta_alpha` makes the reader pick the biaxial character itself (positive
/// when `2 * delta < |birefringence_delta|`, negative otherwise). `specific_gravity` is
/// `None` when the material carried no SG.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomMaterialSnapshot {
    /// Mean refractive index.
    pub mean_ri: f64,
    /// Dispersion offset from the reference value.
    pub dispersion_delta: f64,
    /// Birefringence offset from the reference value.
    pub birefringence_delta: f64,
    /// Specific gravity, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_gravity: Option<f64>,
    /// Crystal system of the material.
    pub crystal_system: String,
    /// Optical character of the material.
    pub optical_character: String,
    /// `n_beta - n_alpha` of a biaxial material (`biaxial_delta_beta_alpha = 0.004`). Written
    /// only for biaxial materials. A usable value (finite, positive, not above
    /// `|birefringence_delta|`) marks the material as biaxial even when
    /// [`Self::optical_character`] is empty or unrecognised. `None` for isotropic and uniaxial
    /// materials and for files without the key. Missing keys read as `None`, so the schema
    /// version does not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biaxial_delta_beta_alpha: Option<f64>,
    /// The material's body color, `absorption_rgb = [r, g, b]`: the isotropic absorption
    /// triple `GemMaterial::new_custom` takes (`None` = colorless). Stored as `f64` (TOML's
    /// only float) through each `f32` component's shortest round-trip decimal; see
    /// [`Self::with_body_color`]. Missing keys read as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_rgb: Option<[f64; 3]>,
    /// The N-band body color (path-aware L*C*h editor):
    /// `absorption_bands_per_mm = [[centre_nm, width_nm, amplitude_per_mm], ...]`. Written next
    /// to the fitted [`Self::absorption_rgb`] triple, the nearest three-band color, so an older
    /// build still shows a close color. When present and non-empty the bands win on load.
    /// Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_bands_per_mm: Option<Vec<[f64; 3]>>,
    /// Per-ray N-band body color of a direction-dependent material
    /// (`[material.custom.absorption_ray_bands_per_mm]`, see [`RayBandsTable`]): the ordinary
    /// and extraordinary rays and, for a biaxial material, a beta ray. Without it, a design
    /// opened on a machine that lacks the library row would color the stone isotropic.
    /// Written next to the triple. When present and usable for the material's optical
    /// character it wins over [`Self::absorption_bands_per_mm`] on load. Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_ray_bands_per_mm: Option<RayBandsTable>,
    /// Optional physically based color recipe and fallback tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_recipe: Option<ColorRecipeDto>,
    /// The material's dispersion curve as coefficients (`[material.custom.dispersion_model]`),
    /// when the author typed Sellmeier or Cauchy coefficients instead of a refractive index
    /// and an `n_F - n_C` figure.
    ///
    /// `None` is the plain path: the material is the Cauchy fit `GemMaterial::new_custom`
    /// builds from [`Self::mean_ri`] and [`Self::dispersion_delta`]. Nothing is written while
    /// it is `None`. With a model present, `mean_ri` and `dispersion_delta` still hold the
    /// model's `n_d` and `n_F - n_C`, so an older build that keeps the table (see
    /// [`Self::unknown`]) restores a close Cauchy stand-in. A table this build cannot read (a
    /// model kind from a newer build, or a malformed one) loads as `None`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_dispersion_model"
    )]
    pub dispersion_model: Option<DispersionModelDto>,
    /// The material's color pattern as JSON text, written by
    /// `pleochroma_cut_core::material::encode_pattern`. Stored as the plain key
    /// `pattern = "<json>"`. A string, so this crate needs no cut-core type. `None` for a
    /// material without a pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The material's default crystal-axis tilt in degrees (`c_axis_tilt_deg = 30.0`), with
    /// [`Self::c_axis_azimuth_deg`]: the orientation a design takes for this material when it
    /// does not set its own. `None` for a material without a default. Nothing is written
    /// while it is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_axis_tilt_deg: Option<f64>,
    /// The default crystal-axis azimuth in degrees; see [`Self::c_axis_tilt_deg`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_axis_azimuth_deg: Option<f64>,
    /// The default Second Axis turn in degrees (0 to 180) of a biaxial material; see
    /// [`Self::c_axis_tilt_deg`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_axis_second_azimuth_deg: Option<f64>,
    /// The extraordinary-ray curve of a uniaxial material
    /// (`[material.custom.dispersion_model_e]`), in the same form as
    /// [`Self::dispersion_model`] (the ordinary ray). `None` when the e-ray follows the
    /// ordinary curve plus `birefringence_delta`. Nothing is written while it is `None`. A
    /// table this build cannot read loads as `None`, like [`Self::dispersion_model`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_dispersion_model"
    )]
    pub dispersion_model_e: Option<DispersionModelDto>,
    /// The `n_alpha` curve of a biaxial material (`[material.custom.dispersion_model_alpha]`),
    /// with [`Self::dispersion_model_gamma`]; [`Self::dispersion_model`] is `n_beta`. Same rules
    /// as [`Self::dispersion_model_e`]. Both tables are needed to mean anything; one alone is
    /// ignored by the reader.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_dispersion_model"
    )]
    pub dispersion_model_alpha: Option<DispersionModelDto>,
    /// The `n_gamma` curve of a biaxial material; see [`Self::dispersion_model_alpha`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_dispersion_model"
    )]
    pub dispersion_model_gamma: Option<DispersionModelDto>,
    /// The material's inclusion families as JSON text (the vault's `inclusion_families_json`
    /// column), stored as `inclusions = "<json>"`. A string, so this crate needs no inclusion
    /// type. `None` for a material without families; nothing is written while it is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusions: Option<String>,
    /// The material's play-of-color structure: the JSON text of the vault's `structure_json`
    /// column, stored as `structure = "<json>"`. Same rules as [`Self::inclusions`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<String>,
    /// The gem data the author entered for the material
    /// (`[material.custom.gem_data]`, see [`CustomGemDataTable`]). `None` for a material
    /// without any; nothing is written while it is `None`. A table this build cannot read
    /// loads as `None`. Missing keys read as `None`, so the schema version does not change.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_gem_data"
    )]
    pub gem_data: Option<CustomGemDataTable>,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// The toughness words of the file, weakest first. The position of a word is its rank on the
/// scale (`Toughness::rank` in `pleochroma-cut-core`).
pub const GEM_TOUGHNESS_WORDS: [&str; 6] = [
    "very_poor",
    "poor",
    "fair",
    "good",
    "excellent",
    "exceptional",
];

/// The Mohs scale's lower bound.
const MOHS_MIN: f64 = 1.0;
/// The Mohs scale's upper bound.
const MOHS_MAX: f64 = 10.0;

/// The gem data of a custom material as plain values, written as the
/// `[material.custom.gem_data]` sub-table:
///
/// ```toml
/// [material.custom.gem_data]
/// mohs = [7.5, 8.0]
/// toughness = ["good", "good"]
/// cleavage = "Imperfect basal"
/// fracture = "Conchoidal"
/// care = "Avoid sudden temperature changes while dopping."
/// source = "My notes, 2026"
/// ```
///
/// Every key is optional. The table keeps what the file says; the checked views
/// ([`Self::mohs_range`], [`Self::toughness_ranks`]) return `None` for a value that is not on
/// its scale, and [`Self::load_notes`] names each such value. A file therefore never fails
/// to load over a gem figure.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomGemDataTable {
    /// Mohs hardness as `[from, to]`, either order.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub mohs: Option<[f64; 2]>,
    /// Toughness as `[from, to]` words of [`GEM_TOUGHNESS_WORDS`], either order.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub toughness: Option<[String; 2]>,
    /// Cleavage as the author wrote it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub cleavage: Option<String>,
    /// Fracture as the author wrote it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub fracture: Option<String>,
    /// Care notes as the author wrote them.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub care: Option<String>,
    /// Where the author's figures come from.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_value"
    )]
    pub source: Option<String>,
}

/// The position of `word` in [`GEM_TOUGHNESS_WORDS`], compared without case and with spaces
/// and hyphens read as underscores ("Very poor" is `very_poor`).
#[must_use]
pub fn gem_toughness_rank(word: &str) -> Option<u8> {
    let key = word.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    GEM_TOUGHNESS_WORDS
        .iter()
        .position(|w| *w == key)
        .and_then(|i| u8::try_from(i).ok())
}

impl CustomGemDataTable {
    /// `true` when no field holds anything; such a table is not worth writing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.mohs.is_none()
            && self.toughness.is_none()
            && self.cleavage.is_none()
            && self.fracture.is_none()
            && self.care.is_none()
            && self.source.is_none()
    }

    /// The Mohs figures as `f32`, `None` when absent or when either end is not finite or lies
    /// outside 1 to 10.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Mohs figures have one or two decimals; f32 holds them exactly enough for display"
    )]
    pub fn mohs_range(&self) -> Option<(f32, f32)> {
        let [a, b] = self.mohs?;
        let on_scale = |v: f64| v.is_finite() && (MOHS_MIN..=MOHS_MAX).contains(&v);
        (on_scale(a) && on_scale(b)).then_some((a as f32, b as f32))
    }

    /// The toughness ends as ranks on the scale of [`GEM_TOUGHNESS_WORDS`] (0 very poor to 5
    /// exceptional), `None` when absent or when either word is unknown.
    #[must_use]
    pub fn toughness_ranks(&self) -> Option<(u8, u8)> {
        let [a, b] = self.toughness.as_ref()?;
        Some((gem_toughness_rank(a)?, gem_toughness_rank(b)?))
    }

    /// One sentence for each stored value that the checked views leave out, for the load
    /// report.
    #[must_use]
    pub fn load_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.mohs.is_some() && self.mohs_range().is_none() {
            notes.push(
                "The custom material's Mohs hardness is outside 1 to 10 and is read as not recorded."
                    .to_owned(),
            );
        }
        if self.toughness.is_some() && self.toughness_ranks().is_none() {
            notes.push(
                "The custom material's toughness uses an unknown word and is read as not recorded."
                    .to_owned(),
            );
        }
        notes
    }
}

/// Reads one optional field, turning a value of the wrong type into `None` instead of failing
/// the whole file.
#[expect(
    clippy::unnecessary_wraps,
    reason = "serde's `deserialize_with` requires this exact `Result` signature"
)]
fn lenient_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<T>::deserialize(deserializer).ok().flatten())
}

/// Reads [`CustomMaterialSnapshot::gem_data`], turning a table this build cannot parse into
/// `None` instead of failing the whole file.
#[expect(
    clippy::unnecessary_wraps,
    reason = "serde's `deserialize_with` requires this exact `Result` signature"
)]
fn lenient_gem_data<'de, D>(deserializer: D) -> Result<Option<CustomGemDataTable>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<CustomGemDataTable>::deserialize(deserializer)
        .ok()
        .flatten())
}

/// The per-ray body-color rows of a custom material, as plain numbers.
///
/// One list of `[centre_nm, width_nm, amplitude_per_mm]` rows per ray, in the form of
/// [`CustomMaterialSnapshot::absorption_bands_per_mm`] (a width above 500 is an energy band, a
/// sigma in cm^-1; otherwise a wavelength band, a sigma in nm). A uniaxial material has `o`
/// and `e`; a biaxial one also has `beta` (alpha is `o`, gamma is `e`). Values are `f64`
/// written through each `f32`'s shortest round-trip decimal, like every other color number of
/// the file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RayBandsTable {
    /// The ordinary ray (alpha for a biaxial material).
    #[serde(default)]
    pub o: Vec<[f64; 3]>,
    /// The extraordinary ray (gamma for a biaxial material).
    #[serde(default)]
    pub e: Vec<[f64; 3]>,
    /// The beta ray of a biaxial material; omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta: Option<Vec<[f64; 3]>>,
}

/// `f32` rows as the `f64` rows the file stores, each number through its shortest round-trip
/// decimal (see [`CustomMaterialSnapshot::with_body_color`]).
fn stored_rows(rows: &[[f32; 3]]) -> Vec<[f64; 3]> {
    rows.iter()
        .map(|r| {
            r.map(|v| {
                v.to_string()
                    .parse::<f64>()
                    .unwrap_or_else(|_| f64::from(v))
            })
        })
        .collect()
}

/// Stored `f64` rows at the `f32` precision the editor uses (the inverse of [`stored_rows`]).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the file stores absorption as shortest round-trip f32 text widened to f64, so narrowing returns the same f32"
)]
fn narrowed_rows(rows: &[[f64; 3]]) -> Vec<[f32; 3]> {
    rows.iter().map(|r| r.map(|v| v as f32)).collect()
}

impl RayBandsTable {
    /// The table of three `f32` row lists (`beta` only for a biaxial material).
    #[must_use]
    pub fn from_rows(o: &[[f32; 3]], e: &[[f32; 3]], beta: Option<&[[f32; 3]]>) -> Self {
        Self {
            o: stored_rows(o),
            e: stored_rows(e),
            beta: beta.map(stored_rows),
        }
    }

    /// The ordinary ray's rows as `f32`.
    #[must_use]
    pub fn o_rows(&self) -> Vec<[f32; 3]> {
        narrowed_rows(&self.o)
    }

    /// The extraordinary ray's rows as `f32`.
    #[must_use]
    pub fn e_rows(&self) -> Vec<[f32; 3]> {
        narrowed_rows(&self.e)
    }

    /// The beta ray's rows as `f32`, `None` for a uniaxial material.
    #[must_use]
    pub fn beta_rows(&self) -> Option<Vec<[f32; 3]>> {
        self.beta.as_deref().map(narrowed_rows)
    }
}

/// A custom material's dispersion curve in the native file, as plain numbers.
///
/// The same three closed forms as `pleochroma::optics::dispersion::DispersionModel` (this
/// crate has no dependency on `pleochroma`; `pleochroma-cut-core` converts both ways).
///
/// Wavelengths in the formulas are in micrometers: Sellmeier `n^2 = 1 + sum(B_i * l^2 /
/// (l^2 - C_i))` with `C` in square micrometers, Cauchy `n = a + b / l^2 + c / l^4`. Written
/// as a table with a `kind` key, for example:
///
/// ```toml
/// [material.custom.dispersion_model]
/// kind = "cauchy"
/// a = 1.7
/// b = 0.006
/// c = 0.0
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispersionModelDto {
    /// One Sellmeier term.
    Sellmeier1 {
        /// The term's weight (dimensionless).
        b1: f64,
        /// The term's resonance, in square micrometers.
        c1: f64,
    },
    /// Three Sellmeier terms.
    Sellmeier3 {
        /// The three weights (dimensionless).
        b: [f64; 3],
        /// The three resonances, in square micrometers.
        c: [f64; 3],
    },
    /// A Cauchy fit.
    Cauchy {
        /// The constant term.
        a: f64,
        /// The `1 / l^2` coefficient, in square micrometers.
        b: f64,
        /// The `1 / l^4` coefficient, in micrometers to the fourth.
        c: f64,
    },
}

/// Reads [`CustomMaterialSnapshot::dispersion_model`], turning a table this build cannot
/// parse into `None` instead of failing the whole file.
#[expect(
    clippy::unnecessary_wraps,
    reason = "serde's `deserialize_with` requires this exact `Result` signature"
)]
fn lenient_dispersion_model<'de, D>(deserializer: D) -> Result<Option<DispersionModelDto>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<DispersionModelDto>::deserialize(deserializer)
        .ok()
        .flatten())
}

/// Serialized DTO for a physically based color recipe in the native format.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ColorRecipeDto {
    /// Raw recipe JSON string containing recipe data, `resolved_bands`, and `data_version`.
    pub recipe_json: String,
    /// The fallback absorption RGB triple written alongside the recipe.
    #[serde(default)]
    pub fallback_rgb: [f64; 3],
}

impl CustomMaterialSnapshot {
    /// Builds a fresh snapshot with no unknown/future fields carried over -- see
    /// [`PreformTable::new`]'s own doc comment for why this lives here rather than
    /// in `pleochroma-cut-core`.
    #[must_use]
    pub fn new(
        mean_ri: f64,
        dispersion_delta: f64,
        birefringence_delta: f64,
        specific_gravity: Option<f64>,
        crystal_system: impl Into<String>,
        optical_character: impl Into<String>,
    ) -> Self {
        Self {
            mean_ri,
            dispersion_delta,
            birefringence_delta,
            specific_gravity,
            crystal_system: crystal_system.into(),
            optical_character: optical_character.into(),
            biaxial_delta_beta_alpha: None,
            absorption_rgb: None,
            absorption_bands_per_mm: None,
            absorption_ray_bands_per_mm: None,
            color_recipe: None,
            dispersion_model: None,
            pattern: None,
            c_axis_tilt_deg: None,
            c_axis_azimuth_deg: None,
            c_axis_second_azimuth_deg: None,
            dispersion_model_e: None,
            dispersion_model_alpha: None,
            dispersion_model_gamma: None,
            inclusions: None,
            structure: None,
            gem_data: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::gem_data`]. A table with no field set stores `None`.
    #[must_use]
    pub fn with_gem_data(mut self, gem_data: Option<CustomGemDataTable>) -> Self {
        self.gem_data = gem_data.filter(|table| !table.is_empty());
        self
    }

    /// Attaches the per-axis curves ([`Self::dispersion_model_e`], and the pair
    /// [`Self::dispersion_model_alpha`] / [`Self::dispersion_model_gamma`]).
    #[must_use]
    pub const fn with_axis_dispersion(
        mut self,
        e: Option<DispersionModelDto>,
        alpha: Option<DispersionModelDto>,
        gamma: Option<DispersionModelDto>,
    ) -> Self {
        self.dispersion_model_e = e;
        self.dispersion_model_alpha = alpha;
        self.dispersion_model_gamma = gamma;
        self
    }

    /// Attaches [`Self::inclusions`] and [`Self::structure`], the JSON texts of the vault
    /// columns. Blank text stores `None`.
    #[must_use]
    pub fn with_inclusion_families(
        mut self,
        inclusions: Option<String>,
        structure: Option<String>,
    ) -> Self {
        let keep = |text: Option<String>| text.filter(|t| !t.trim().is_empty());
        self.inclusions = keep(inclusions);
        self.structure = keep(structure);
        self
    }

    /// Attaches the default crystal-axis orientation ([`Self::c_axis_tilt_deg`],
    /// [`Self::c_axis_azimuth_deg`], [`Self::c_axis_second_azimuth_deg`]) from the library
    /// row's `f32` degrees, each through its shortest round-trip decimal (see
    /// [`Self::with_body_color`]).
    #[must_use]
    pub fn with_default_orientation(
        mut self,
        tilt_deg: Option<f32>,
        azimuth_deg: Option<f32>,
        second_azimuth_deg: Option<f32>,
    ) -> Self {
        let stored = |v: f32| {
            v.to_string()
                .parse::<f64>()
                .unwrap_or_else(|_| f64::from(v))
        };
        self.c_axis_tilt_deg = tilt_deg.map(stored);
        self.c_axis_azimuth_deg = azimuth_deg.map(stored);
        self.c_axis_second_azimuth_deg = second_azimuth_deg.map(stored);
        self
    }

    /// The default crystal-axis orientation as the library row's `f32` degrees: tilt, azimuth
    /// and Second Axis turn, each `None` when absent.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the file stores degrees as shortest round-trip f32 text widened to f64, so narrowing returns the same f32"
    )]
    pub fn default_orientation(&self) -> (Option<f32>, Option<f32>, Option<f32>) {
        (
            self.c_axis_tilt_deg.map(|v| v as f32),
            self.c_axis_azimuth_deg.map(|v| v as f32),
            self.c_axis_second_azimuth_deg.map(|v| v as f32),
        )
    }

    /// Attaches [`Self::pattern`], the encoded color pattern text.
    #[must_use]
    pub fn with_pattern(mut self, pattern: Option<String>) -> Self {
        self.pattern = pattern;
        self
    }

    /// Attaches [`Self::biaxial_delta_beta_alpha`].
    #[must_use]
    pub const fn with_biaxial_delta(mut self, delta: Option<f64>) -> Self {
        self.biaxial_delta_beta_alpha = delta;
        self
    }

    /// Attaches [`Self::dispersion_model`].
    #[must_use]
    pub const fn with_dispersion_model(mut self, model: Option<DispersionModelDto>) -> Self {
        self.dispersion_model = model;
        self
    }

    /// Attaches [`Self::color_recipe`].
    #[must_use]
    pub fn with_color_recipe(mut self, recipe: Option<ColorRecipeDto>) -> Self {
        self.color_recipe = recipe;
        self
    }

    /// Attaches [`Self::absorption_rgb`] as the `f64` triple stored on disk.
    #[must_use]
    pub const fn with_absorption_rgb(mut self, rgb: Option<[f64; 3]>) -> Self {
        self.absorption_rgb = rgb;
        self
    }

    /// Attaches [`Self::absorption_bands_per_mm`] from `f32` rows
    /// (`[centre_nm, width_nm, amplitude_per_mm]`), each component through its shortest
    /// round-trip decimal (see [`Self::with_body_color`]). An empty list stores `None`.
    #[must_use]
    pub fn with_absorption_bands(mut self, rows: &[[f32; 3]]) -> Self {
        self.absorption_bands_per_mm = (!rows.is_empty()).then(|| stored_rows(rows));
        self
    }

    /// Attaches [`Self::absorption_ray_bands_per_mm`] (`None` stores nothing).
    #[must_use]
    pub fn with_absorption_ray_bands(mut self, rays: Option<RayBandsTable>) -> Self {
        self.absorption_ray_bands_per_mm = rays;
        self
    }

    /// The N-band body color as `f32` rows (`None` when absent or empty).
    #[must_use]
    pub fn absorption_bands(&self) -> Option<Vec<[f32; 3]>> {
        self.absorption_bands_per_mm
            .as_ref()
            .filter(|b| !b.is_empty())
            .map(|rows| narrowed_rows(rows))
    }

    /// Attaches the body color from an editor's `f32` absorption triple.
    ///
    /// Each component is stored as its shortest round-trip decimal rather than a plain
    /// widening cast: `f64::from(0.2_f32)` would be written as `0.20000000298023224`,
    /// which is what a cutter reading the raw file would then see for a color picked
    /// as "0.2". The decimal text identifies the `f32` uniquely, so
    /// [`Self::body_color`] returns the identical bits.
    #[must_use]
    pub fn with_body_color(self, rgb: Option<[f32; 3]>) -> Self {
        let stored = rgb.map(|rgb| {
            rgb.map(|v| {
                v.to_string()
                    .parse::<f64>()
                    .unwrap_or_else(|_| f64::from(v))
            })
        });
        self.with_absorption_rgb(stored)
    }

    /// The body color as the editor's `f32` absorption triple (`None` = colorless).
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the file stores absorption as shortest round-trip f32 text widened to f64, so narrowing returns the same f32"
    )]
    pub fn body_color(&self) -> Option<[f32; 3]> {
        self.absorption_rgb.map(|rgb| rgb.map(|v| v as f32))
    }
}
