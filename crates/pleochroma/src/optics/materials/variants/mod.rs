//! Named color variants of a material.
//!
//! A [`ColorVariant`] is a complete per-axis spectral absorption tensor for one material
//! ("Sapphire / Pink"), with its unit and provenance. Applying one to a [`GemMaterial`]
//! ([`GemMaterial::with_color_variant`]) replaces ONLY the absorption tensor and its unit: the
//! stone keeps its dispersion, birefringence, axes, scattering and inclusions, and keeps the rank
//! of its absorption (a pleochroic host never gets an isotropic color from here, see
//! [`ColorVariant::validate_for`]).
//!
//! * [`rows`]: the explicit stored form of a tensor ([`VariantTensorRows`]), exact round trip.
//! * [`validate`]: [`TensorRank`], [`VariantProblem`], [`ColorVariant::validate_for`].
//! * `data`: the built-in entries of `data/color_variants.toml`, parsed once.
//!
//! [`built_in_variants`] lists the variants of one built-in material: its Material Default (its
//! own tensor) first, then the data file's entries in file order.

mod data;
pub mod rows;
#[cfg(test)]
mod tests;
// One test file per data group (`data/color_variants/<group>.toml`), owned by its fit lane.
#[cfg(test)]
mod tests_beryl_topaz_tourmaline_andalusite;
#[cfg(test)]
mod tests_corundum_spinel_chrysoberyl;
#[cfg(test)]
mod tests_diamond;
#[cfg(test)]
mod tests_garnet_peridot_zircon_benitoite_tanzanite;
#[cfg(test)]
mod tests_quartz_diamond_opal_synthetics_glass;
pub mod validate;

pub use data::COLOR_VARIANTS_DATA_VERSION;
pub use rows::{VariantBandRow, VariantBandShape, VariantTensorRows};
pub use validate::{MAX_VARIANT_BANDS, TensorRank, VariantProblem, host_rank, tensor_rank};

use super::{AbsorptionUnit, GemMaterial};
use crate::{
    color::{
        body_color::{BodyColor, Illuminant},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::absorption::AbsorptionTensor,
};

/// Reference stone width of a Material Default (7 mm).
///
/// It is the width the built-in tensors are calibrated for. Like every `reference_path_mm` it is
/// a stone WIDTH, not a light path (see [`ColorVariant::reference_path_mm`]).
pub const MATERIAL_DEFAULT_REFERENCE_PATH_MM: f32 = 7.0;

/// Label of the entry that carries a material's own tensor.
pub const MATERIAL_DEFAULT_LABEL: &str = "Material Default";

/// Where a variant comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantProvenance {
    /// An entry of the shipped data file.
    BuiltIn {
        /// Citations: papers, spectra files, catalogue entries the numbers come from.
        sources: Vec<String>,
        /// Link to the calibration record of the fit, if there is one.
        fit_record: Option<String>,
        /// Whether the numbers are an estimate rather than a fit to measured spectra.
        estimated: bool,
    },
    /// The material's own tensor, listed as the first variant.
    MaterialDefault,
    /// Saved by the cutter.
    User,
}

/// A named, complete per-axis absorption tensor for one material.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorVariant {
    /// Stable key: `"sapphire.padparadscha"`, `"sapphire.default"` or `"user.<uuid>"`.
    pub key: String,
    /// Display label in Title Case (`"Padparadscha"`, `"Material Default"`).
    pub label: String,
    /// The material it belongs to: the exact built-in name (or a custom material's name for a
    /// user variant).
    pub material: String,
    /// The per-axis tensor: o / e (/ beta), at most [`MAX_VARIANT_BANDS`] bands per ray.
    pub absorption: AbsorptionTensor,
    /// What the tensor's numbers are measured per.
    pub unit: AbsorptionUnit,
    /// The STONE WIDTH in millimetres the numbers are calibrated for (7 mm for the built-ins).
    ///
    /// This is a width, never a light path: every swatch and fallback color of a variant is
    /// evaluated over the path [`face_up_swatch_path`] gives for `(unit, reference_path_mm)`,
    /// the face-up path the render itself shows a stone of that width. (A saved Custom Color stores the width of the
    /// stone it was solved on.)
    pub reference_path_mm: f32,
    /// Where the variant comes from.
    pub provenance: VariantProvenance,
}

/// The light path a face-up swatch of a tensor in `unit` is evaluated over.
///
/// It is the `path_mm` argument of `color::body_color::body_colors`, for a stone
/// `stone_width_mm` wide (0.0 or less: the 7 mm default), so the swatch shows what the render
/// shows face-up.
///
/// * [`AbsorptionUnit::ModelUnit`]: one model unit at 7 mm (the swatch convention every
///   per-model-unit color is tuned to, `render_setup::MODEL_UNIT_FACE_UP_PATH`), scaled by
///   `width / 7`.
/// * [`AbsorptionUnit::PerMm`]: the face-up path in millimetres, `MODEL_UNIT_FACE_UP_PATH` model
///   units of a `REFERENCE_MODEL_WIDTH`-wide stone, i.e. `2.83 x width / 2.0` (9.905 mm at 7 mm).
///
/// A variant's `reference_path_mm` is the stone width its numbers are calibrated for, not this
/// path: pass it here as `stone_width_mm`.
#[must_use]
pub fn face_up_swatch_path(unit: AbsorptionUnit, stone_width_mm: f32) -> f64 {
    let width_mm = f64::from(crate::render_setup::effective_stone_width_mm(
        if stone_width_mm.is_nan() {
            0.0
        } else {
            stone_width_mm
        },
    ));
    let default_mm = f64::from(crate::render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM);
    match unit {
        AbsorptionUnit::ModelUnit => width_mm / default_mm,
        AbsorptionUnit::PerMm => {
            f64::from(crate::render_setup::MODEL_UNIT_FACE_UP_PATH) * width_mm
                / crate::render_setup::REFERENCE_MODEL_WIDTH
        }
    }
}

/// The face-up D65 color of a variant tensor on `host`: what a stone of this color shows looking
/// straight down the table.
///
/// The tensor is evaluated by [`face_up_body_color`] over the path [`face_up_swatch_path`] gives
/// for `(unit, stone_width_mm)`, with the host's default principal frame,
/// `PrincipalFrame::of_material(host)`:
/// the biaxial indicatrix of the host, else the uniaxial frame about its own `c_axis`. That is
/// the frame the render builds (`PrincipalFrame::for_pleochroic`, which is `of_material` for a
/// pleochroic tensor) for a stone with no axis override of its own, so the Crystal Axis defaults
/// of the material decide which rays the table shows: `c` along the table normal gives the
/// ordinary color alone, `c` in the table plane the mean of the two modes' transmittances, a
/// biaxial host its `k = -Y` eigenmodes.
///
/// A tensor that is not pleochroic has no orientation: its swatch is the unpolarised color
/// (`body_colors(..).unpolarised`, bit for bit). Tooltips keep showing the per-direction colors
/// of `body_colors` next to this one.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a stone width in millimetres is a few dozen at most; f32 is the precision of every stored width"
)]
pub fn face_up_swatch(
    absorption: &AbsorptionTensor,
    unit: AbsorptionUnit,
    stone_width_mm: f64,
    host: &GemMaterial,
) -> BodyColor {
    let path = face_up_swatch_path(unit, stone_width_mm as f32);
    let frame = absorption
        .is_pleochroic
        .then(|| PrincipalFrame::of_material(host));
    face_up_body_color(absorption, frame.as_ref(), path, Illuminant::D65)
}

/// The key prefix of a material: its lowercase name with every non-alphanumeric character
/// replaced by `_` (`"Chrysoberyl (Yellow)"` gives `"chrysoberyl__yellow_"`).
#[must_use]
pub fn material_slug(material_name: &str) -> String {
    material_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The Material Default of a built-in material: its own tensor and unit as the first variant.
fn material_default(material: &GemMaterial) -> ColorVariant {
    ColorVariant {
        key: format!(
            "{}.{}",
            material_slug(&material.name),
            data::DEFAULT_KEY_SUFFIX
        ),
        label: MATERIAL_DEFAULT_LABEL.to_string(),
        material: material.name.clone(),
        absorption: material.absorption.clone(),
        unit: material.absorption_unit,
        reference_path_mm: MATERIAL_DEFAULT_REFERENCE_PATH_MM,
        provenance: VariantProvenance::MaterialDefault,
    }
}

/// The variants of the built-in material named `material_name` (exact name, ASCII case ignored).
///
/// The Material Default comes first (built from the material's own tensor), then the data file's
/// entries in file order. The result is empty for an unknown name.
///
/// # Panics
///
/// Panics if the embedded `color_variants.toml` is invalid (a data error caught by the unit
/// tests).
#[must_use]
pub fn built_in_variants(material_name: &str) -> Vec<ColorVariant> {
    let Some(material) = GemMaterial::built_in()
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
    else {
        return Vec::new();
    };
    let mut variants = vec![material_default(material)];
    variants.extend(
        data::file_entries()
            .iter()
            .filter(|v| v.material == material.name)
            .cloned(),
    );
    variants
}

impl GemMaterial {
    /// Replaces ONLY the absorption tensor and its unit; every other field is kept bit-for-bit
    /// (dispersion, birefringence, axes, scattering, edge rounding, inclusions; the path scale
    /// follows the rule below). Drops `zoning`: a zoned material's color is its zones, and a zoned material
    /// left as `ModelUnit` would break the zoned tracer's per-millimetre rule. Never builds an
    /// isotropic tensor from a pleochroic one: the tensor is stored exactly as given.
    ///
    /// `absorption_path_scale` stays the host's own, with one exception: a material whose unit
    /// BECOMES [`AbsorptionUnit::PerMm`] gets the scale of a 7 mm stone,
    /// half of [`MATERIAL_DEFAULT_REFERENCE_PATH_MM`] (what `with_library_ray_bands` also uses).
    /// Per-millimetre numbers over model units are 3.5 times too light, so a consumer that traces
    /// without `render_setup` choosing the scale from the stone's size (or with no model width)
    /// still renders a sensible stone. `render_setup` replaces the default with the real scale
    /// whenever it knows the stone.
    #[must_use]
    pub fn with_color_variant_tensor(
        mut self,
        absorption: AbsorptionTensor,
        unit: AbsorptionUnit,
    ) -> Self {
        if unit == AbsorptionUnit::PerMm && self.absorption_unit != AbsorptionUnit::PerMm {
            self.absorption_path_scale = MATERIAL_DEFAULT_REFERENCE_PATH_MM / 2.0;
        }
        self.absorption = absorption;
        self.absorption_unit = unit;
        self.zoning = None;
        self
    }

    /// [`Self::with_color_variant_tensor`] with the tensor and unit of `variant`.
    ///
    /// Does not check the variant against this material; call [`ColorVariant::validate_for`]
    /// first when the variant did not come from [`built_in_variants`] of the same material.
    #[must_use]
    pub fn with_color_variant(self, variant: &ColorVariant) -> Self {
        self.with_color_variant_tensor(variant.absorption.clone(), variant.unit)
    }
}
