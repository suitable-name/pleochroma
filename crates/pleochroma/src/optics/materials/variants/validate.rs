//! Rank and sanity checks of a color variant against the material it is applied to.

use std::fmt;

use super::{ColorVariant, VariantProvenance};
use crate::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{GemMaterial, OpticalCharacter},
    },
    renderer::buffers::MAX_ABSORPTION_BANDS,
};

/// Most bands a variant may carry per ray: the GPU material buffer's capacity.
///
/// `renderer::buffers` is compiled without any feature, so this is the renderer's own constant,
/// not a copy that could drift.
pub const MAX_VARIANT_BANDS: usize = MAX_ABSORPTION_BANDS;

/// The absorption rank of a material or a tensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TensorRank {
    /// One band set for every direction (cubic and amorphous hosts).
    Isotropic,
    /// Ordinary and extraordinary band sets.
    Uniaxial,
    /// Alpha, beta and gamma band sets.
    Biaxial,
}

/// What is wrong with a variant for a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantProblem {
    /// A centre, width, peak or the reference path is NaN or infinite.
    NonFinite,
    /// A peak is negative, a centre or width is not above zero, or the reference path is not
    /// above zero.
    Negative,
    /// A ray carries more bands than the GPU holds ([`MAX_VARIANT_BANDS`]).
    TooManyBands {
        /// Which band set: `"o_ray"`, `"e_ray"` or `"beta_ray"`.
        ray: &'static str,
        /// How many bands that set has.
        count: usize,
    },
    /// The variant's rank is not one the host accepts.
    RankMismatch {
        /// The host's rank ([`host_rank`]).
        host: TensorRank,
        /// The variant's rank ([`tensor_rank`]).
        variant: TensorRank,
    },
    /// One band set is empty while another is not: a half-defined tensor. A tensor whose sets are
    /// all empty is a valid colorless variant.
    Empty,
}

impl fmt::Display for VariantProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite => f.write_str("a band value is not a finite number"),
            Self::Negative => {
                f.write_str("a peak is negative, or a centre, width or path is not above zero")
            }
            Self::TooManyBands { ray, count } => write!(
                f,
                "the {ray} has {count} bands, at most {MAX_VARIANT_BANDS} are allowed"
            ),
            Self::RankMismatch { host, variant } => {
                write!(f, "a {variant:?} variant does not fit a {host:?} material")
            }
            Self::Empty => f.write_str("one band set is empty while another is not"),
        }
    }
}

impl std::error::Error for VariantProblem {}

/// The host's absorption rank: [`TensorRank::Isotropic`] for [`OpticalCharacter::Isotropic`],
/// [`TensorRank::Uniaxial`] for the uniaxial characters, [`TensorRank::Biaxial`] for the biaxial
/// ones.
#[must_use]
pub const fn host_rank(host: &GemMaterial) -> TensorRank {
    match host.optical_character {
        OpticalCharacter::Isotropic => TensorRank::Isotropic,
        OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => {
            TensorRank::Uniaxial
        }
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => {
            TensorRank::Biaxial
        }
    }
}

/// The rank of a tensor: isotropic when it is not pleochroic, biaxial when it has a `beta_ray`,
/// uniaxial otherwise.
#[must_use]
pub const fn tensor_rank(tensor: &AbsorptionTensor) -> TensorRank {
    if !tensor.is_pleochroic {
        TensorRank::Isotropic
    } else if tensor.beta_ray.is_some() {
        TensorRank::Biaxial
    } else {
        TensorRank::Uniaxial
    }
}

/// Whether `host` accepts a COLORED variant of rank `variant`.
///
/// An isotropic host accepts only isotropic variants, a uniaxial host uniaxial ones, a biaxial
/// host biaxial ones, and uniaxial ones only while its own tensor has no `beta_ray`. The one
/// exception is the host's own Material Default (`material_default`, which the caller has
/// already checked belongs to `host`): it carries the material's stored tensor, which may be
/// simpler than its optics, and always validates against its own material. Every other variant
/// must match the host's optics, so an anisotropic host never receives an isotropic color from
/// a variant, whatever its own stored tensor is.
///
/// A tensor with no bands at all has no pleochroism to lose and is accepted by
/// [`ColorVariant::validate_for`] before this rule is asked.
fn rank_accepted(host: &GemMaterial, variant: TensorRank, material_default: bool) -> bool {
    if material_default && variant == tensor_rank(&host.absorption) {
        return true;
    }
    match (host_rank(host), variant) {
        (TensorRank::Isotropic, TensorRank::Isotropic)
        | (TensorRank::Uniaxial, TensorRank::Uniaxial)
        | (TensorRank::Biaxial, TensorRank::Biaxial) => true,
        (TensorRank::Biaxial, TensorRank::Uniaxial) => host.absorption.beta_ray.is_none(),
        _ => false,
    }
}

/// The band sets of a tensor with their names, in o / e / beta order.
fn named_sets(tensor: &AbsorptionTensor) -> Vec<(&'static str, &[AbsorptionBand])> {
    let mut sets: Vec<(&'static str, &[AbsorptionBand])> = vec![
        ("o_ray", tensor.o_ray.as_slice()),
        ("e_ray", tensor.e_ray.as_slice()),
    ];
    if let Some(beta) = &tensor.beta_ray {
        sets.push(("beta_ray", beta.as_slice()));
    }
    sets
}

/// Checks the numbers and the band counts of a tensor (everything but the rank).
fn validate_tensor(tensor: &AbsorptionTensor) -> Result<(), VariantProblem> {
    let sets = named_sets(tensor);
    let all = || sets.iter().flat_map(|(_, bands)| bands.iter());
    if all().any(|b| !b.center_nm.is_finite() || !b.width_nm.is_finite() || !b.peak.is_finite()) {
        return Err(VariantProblem::NonFinite);
    }
    if all().any(|b| b.peak < 0.0 || b.center_nm <= 0.0 || b.width_nm <= 0.0) {
        return Err(VariantProblem::Negative);
    }
    for (ray, bands) in &sets {
        if bands.len() > MAX_VARIANT_BANDS {
            return Err(VariantProblem::TooManyBands {
                ray,
                count: bands.len(),
            });
        }
    }
    let any_empty = sets.iter().any(|(_, bands)| bands.is_empty());
    let any_filled = sets.iter().any(|(_, bands)| !bands.is_empty());
    if any_empty && any_filled {
        return Err(VariantProblem::Empty);
    }
    Ok(())
}

impl ColorVariant {
    /// Checks this variant for `host`: finite numbers, nonnegative peaks, positive centres,
    /// widths and reference path, at most [`MAX_VARIANT_BANDS`] bands per ray, no half-empty
    /// tensor, and a rank the host accepts (see [`VariantProblem`] and the rank rule in the
    /// module's `rank_accepted`).
    ///
    /// An all-empty tensor (no band in any ray) is a valid colorless variant for every host,
    /// whatever its `pleochroic` flag says: there is no pleochroism to lose, so a data entry
    /// needs no `pleochroic = true` trick to be colorless on a pleochroic host. The Material
    /// Default exception of the rank rule applies only to the Material Default OF `host`
    /// (`material` equal to the host's name, ASCII case ignored).
    ///
    /// # Errors
    ///
    /// The first [`VariantProblem`] found, in the order `NonFinite`, `Negative`, `TooManyBands`,
    /// `Empty`, `RankMismatch`.
    pub fn validate_for(&self, host: &GemMaterial) -> Result<(), VariantProblem> {
        validate_tensor(&self.absorption)?;
        if !self.reference_path_mm.is_finite() {
            return Err(VariantProblem::NonFinite);
        }
        if self.reference_path_mm <= 0.0 {
            return Err(VariantProblem::Negative);
        }
        let colorless = named_sets(&self.absorption)
            .iter()
            .all(|(_, bands)| bands.is_empty());
        let variant = tensor_rank(&self.absorption);
        let material_default = matches!(self.provenance, VariantProvenance::MaterialDefault)
            && self.material.eq_ignore_ascii_case(&host.name);
        if colorless || rank_accepted(host, variant, material_default) {
            Ok(())
        } else {
            Err(VariantProblem::RankMismatch {
                host: host_rank(host),
                variant,
            })
        }
    }
}
