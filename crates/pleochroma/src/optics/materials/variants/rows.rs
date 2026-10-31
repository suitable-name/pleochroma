//! Stored band rows: the plain, explicit form of an [`AbsorptionTensor`] that TOML, `.psdd` and
//! the vault keep.
//!
//! The band shape is a named field, never inferred from a width (the legacy `[centre, width,
//! amplitude]` rows of `GemMaterial::with_body_color_bands` read a width above 500 as an energy
//! band, an encoding that is ambiguous at the edges).

use crate::optics::absorption::{AbsorptionBand, AbsorptionTensor, BandShape};

/// How a stored band row is shaped (explicit, never inferred from the width).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariantBandShape {
    /// Gaussian in wavelength ([`BandShape::GaussianWavelength`]): the width is sigma in nm.
    Wavelength,
    /// Gaussian in wavenumber ([`BandShape::GaussianEnergy`]): the width is sigma in cm^-1.
    Energy,
}

impl VariantBandShape {
    /// The renderer's band shape this row shape stands for.
    #[must_use]
    pub const fn to_band_shape(self) -> BandShape {
        match self {
            Self::Wavelength => BandShape::GaussianWavelength,
            Self::Energy => BandShape::GaussianEnergy,
        }
    }

    /// The row shape of a renderer band shape. The match is exhaustive on purpose: a new
    /// [`BandShape`] must get its own row shape here before the crate compiles.
    #[must_use]
    pub const fn from_band_shape(shape: BandShape) -> Self {
        match shape {
            BandShape::GaussianWavelength => Self::Wavelength,
            BandShape::GaussianEnergy => Self::Energy,
        }
    }
}

/// One stored band: exactly an [`AbsorptionBand`] (`width_nm` is the band's own `width_nm`
/// field: sigma in nm for [`VariantBandShape::Wavelength`], sigma in cm^-1 for
/// [`VariantBandShape::Energy`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VariantBandRow {
    /// Band centre wavelength, nanometers.
    pub center_nm: f32,
    /// Gaussian width (standard deviation) in the unit the [`Self::shape`] names.
    pub width_nm: f32,
    /// Peak absorption coefficient at `center_nm` (per millimetre or per model unit, see the
    /// owning variant's unit).
    pub peak: f32,
    /// Which domain the Gaussian is in.
    pub shape: VariantBandShape,
}

impl VariantBandRow {
    /// The renderer band of this row, field for field.
    #[must_use]
    pub const fn to_band(&self) -> AbsorptionBand {
        AbsorptionBand {
            center_nm: self.center_nm,
            width_nm: self.width_nm,
            peak: self.peak,
            shape: self.shape.to_band_shape(),
        }
    }

    /// The row of a renderer band, field for field.
    #[must_use]
    pub const fn from_band(band: &AbsorptionBand) -> Self {
        Self {
            center_nm: band.center_nm,
            width_nm: band.width_nm,
            peak: band.peak,
            shape: VariantBandShape::from_band_shape(band.shape),
        }
    }
}

/// A full per-axis tensor as stored rows (TOML, `.psdd`, vault JSON).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VariantTensorRows {
    /// Bands of the ordinary ray (uniaxial) / the `n_alpha` direction (biaxial).
    pub o_ray: Vec<VariantBandRow>,
    /// Bands of the extraordinary ray (uniaxial) / the `n_gamma` direction (biaxial).
    pub e_ray: Vec<VariantBandRow>,
    /// Bands of the `n_beta` direction of a biaxial (trichroic) tensor; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta_ray: Option<Vec<VariantBandRow>>,
    /// Whether the eigenmodes use different band sets (`AbsorptionTensor::is_pleochroic`).
    /// Stored as given, never derived: a pleochroic tensor whose sets happen to be equal stays
    /// pleochroic.
    pub pleochroic: bool,
}

/// Converts stored rows to renderer bands.
fn bands_of(rows: &[VariantBandRow]) -> Vec<AbsorptionBand> {
    rows.iter().map(VariantBandRow::to_band).collect()
}

/// Converts renderer bands to stored rows.
fn rows_of(bands: &[AbsorptionBand]) -> Vec<VariantBandRow> {
    bands.iter().map(VariantBandRow::from_band).collect()
}

impl VariantTensorRows {
    /// The renderer tensor of these rows. Exact: every field is copied, so
    /// `from_tensor(t).to_tensor() == t` bit for bit (`is_pleochroic` stays the stored
    /// `pleochroic`, and `beta_ray` stays `Some` / `None`).
    #[must_use]
    pub fn to_tensor(&self) -> AbsorptionTensor {
        AbsorptionTensor {
            o_ray: bands_of(&self.o_ray),
            e_ray: bands_of(&self.e_ray),
            beta_ray: self.beta_ray.as_deref().map(bands_of),
            is_pleochroic: self.pleochroic,
        }
    }

    /// The rows of a renderer tensor. Exact inverse of [`Self::to_tensor`].
    #[must_use]
    pub fn from_tensor(tensor: &AbsorptionTensor) -> Self {
        Self {
            o_ray: rows_of(&tensor.o_ray),
            e_ray: rows_of(&tensor.e_ray),
            beta_ray: tensor.beta_ray.as_deref().map(rows_of),
            pleochroic: tensor.is_pleochroic,
        }
    }
}
