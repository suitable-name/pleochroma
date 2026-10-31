//! Color variants and per-axis colors as zone colors: the stored rows of a variant color
//! ([`PatternTensorRows`]), the per-millimeter rule ([`per_mm_rows`]) and how a zone color fits the
//! host's rank ([`zone_rank_fit`]).
//!
//! # Per millimeter only
//!
//! A zone is rendered per millimeter (`ZoneAbsorption::per_mm`), so a [`PatternColor::Variant`]
//! holds per-millimeter rows. A variant whose unit is `ModelUnit` (every built-in Material Default
//! and the Pink seed) is converted when it becomes a zone color ([`PatternColor::from_variant`]).
//! The rule is the render's own: a `ModelUnit` material gets the scale
//! `(W / 7) / K * (R / model_width)` and a `PerMm` one `W / model_width` (`render_setup::
//! absorption_path_scale_for`; `K` = `MODEL_UNIT_FACE_UP_PATH`, `R` = `REFERENCE_MODEL_WIDTH`,
//! `W` the stone width), so the same stone shows the same color when the per-millimeter numbers
//! are the model-unit ones times `R / (7 * K)`. The stone width and the design's model width cancel
//! out, which is why [`per_mm_factor`] takes no size.
//!
//! # The wire form
//!
//! The pattern also crosses the web Worker boundary as positional postcard, which cannot skip a
//! field. [`PatternTensorRows`] therefore always writes every field (unlike the vault's
//! `VariantTensorRows`, whose absent `beta_ray` is skipped in JSON).

use super::{
    PatternColor, VARIANT_SEED_LCH, ZoneColor,
    convert::{pattern_color, rows_carry_tensor},
};
use pleochroma::{
    optics::{
        absorption::AbsorptionTensor,
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{
                ColorVariant, MAX_VARIANT_BANDS, TensorRank, VariantBandRow, VariantProblem,
                VariantProvenance, VariantTensorRows, host_rank,
            },
        },
        zoning::ZoneAbsorption,
    },
    render_setup::{
        MODEL_UNIT_FACE_UP_PATH, PHYSICS_DEFAULT_STONE_WIDTH_MM, REFERENCE_MODEL_WIDTH,
    },
};
use serde::{Deserialize, Serialize};

/// The reference path the converted rows of a variant are checked at (the built-ins' own 7 mm).
const CHECK_REFERENCE_PATH_MM: f32 = 7.0;

/// A per-axis tensor as stored rows, per millimeter: the content of a
/// [`VariantTensorRows`], with every field always written (see the module notes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternTensorRows {
    /// Bands of the ordinary ray (uniaxial) / the alpha direction (biaxial).
    pub o_ray: Vec<VariantBandRow>,
    /// Bands of the extraordinary ray (uniaxial) / the gamma direction (biaxial).
    pub e_ray: Vec<VariantBandRow>,
    /// Bands of the beta direction of a biaxial (trichroic) tensor; `None` otherwise.
    #[serde(default)]
    pub beta_ray: Option<Vec<VariantBandRow>>,
    /// Whether the eigenmodes use different band sets (`AbsorptionTensor::is_pleochroic`).
    pub pleochroic: bool,
}

impl PatternTensorRows {
    /// The rows of the vault form, field for field.
    #[must_use]
    pub fn from_variant_rows(rows: &VariantTensorRows) -> Self {
        Self {
            o_ray: rows.o_ray.clone(),
            e_ray: rows.e_ray.clone(),
            beta_ray: rows.beta_ray.clone(),
            pleochroic: rows.pleochroic,
        }
    }

    /// The vault form of these rows, field for field.
    #[must_use]
    pub fn to_variant_rows(&self) -> VariantTensorRows {
        VariantTensorRows {
            o_ray: self.o_ray.clone(),
            e_ray: self.e_ray.clone(),
            beta_ray: self.beta_ray.clone(),
            pleochroic: self.pleochroic,
        }
    }

    /// The rows of a renderer tensor (exact: see [`VariantTensorRows::from_tensor`]).
    #[must_use]
    pub fn from_tensor(tensor: &AbsorptionTensor) -> Self {
        Self::from_variant_rows(&VariantTensorRows::from_tensor(tensor))
    }

    /// The renderer tensor of these rows (exact).
    #[must_use]
    pub fn to_tensor(&self) -> AbsorptionTensor {
        self.to_variant_rows().to_tensor()
    }

    /// The rank of the tensor: isotropic when it is not pleochroic, biaxial when it has a beta
    /// set, uniaxial otherwise (the rule of `tensor_rank`).
    #[must_use]
    pub const fn rank(&self) -> TensorRank {
        if !self.pleochroic {
            TensorRank::Isotropic
        } else if self.beta_ray.is_some() {
            TensorRank::Biaxial
        } else {
            TensorRank::Uniaxial
        }
    }

    /// Whether every number is usable: finite, a centre and width above zero, a peak of zero or
    /// more, and at most [`MAX_VARIANT_BANDS`] bands per ray.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        let sets = [Some(&self.o_ray), Some(&self.e_ray), self.beta_ray.as_ref()];
        sets.into_iter().flatten().all(|set| {
            set.len() <= MAX_VARIANT_BANDS
                && set.iter().all(|row| {
                    [row.center_nm, row.width_nm, row.peak]
                        .iter()
                        .all(|v| v.is_finite())
                        && row.center_nm > 0.0
                        && row.width_nm > 0.0
                        && row.peak >= 0.0
                })
        })
    }
}

/// The factor that turns the amplitudes of a `unit` tensor into per-millimeter ones.
///
/// It is `1` for [`AbsorptionUnit::PerMm`] and `REFERENCE_MODEL_WIDTH / (7 mm *
/// MODEL_UNIT_FACE_UP_PATH)` for [`AbsorptionUnit::ModelUnit`] (see the module notes: the
/// render's own conversion).
#[must_use]
pub fn per_mm_factor(unit: AbsorptionUnit) -> f64 {
    match unit {
        AbsorptionUnit::PerMm => 1.0,
        AbsorptionUnit::ModelUnit => {
            REFERENCE_MODEL_WIDTH
                / (f64::from(PHYSICS_DEFAULT_STONE_WIDTH_MM) * f64::from(MODEL_UNIT_FACE_UP_PATH))
        }
    }
}

/// The per-millimeter rows of `variant`: its tensor as stored when it is already per millimeter
/// (bit for bit), else every peak scaled by [`per_mm_factor`]. Centres, widths and shapes never
/// change.
#[must_use]
pub fn per_mm_rows(variant: &ColorVariant) -> PatternTensorRows {
    let mut rows = PatternTensorRows::from_tensor(&variant.absorption);
    if variant.unit == AbsorptionUnit::ModelUnit {
        let factor = per_mm_factor(AbsorptionUnit::ModelUnit);
        let scale = |set: &mut Vec<VariantBandRow>| {
            for row in set {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "f32 is the band precision; a scaled peak is in the normal float range"
                )]
                let peak = (f64::from(row.peak) * factor) as f32;
                row.peak = peak;
            }
        };
        scale(&mut rows.o_ray);
        scale(&mut rows.e_ray);
        if let Some(beta) = rows.beta_ray.as_mut() {
            scale(beta);
        }
    }
    rows
}

/// How a zone color relates to the rank of the host material it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneRankFit {
    /// The color has the rank the host asks for (or a rank the host accepts).
    Fits,
    /// A one-color zone on a pleochroic host: it renders as saved but has no pleochroism
    /// ("Legacy: No Pleochroism"; owner decision D2).
    Legacy,
    /// More axes than the host has (three colors on a uniaxial host, two on an isotropic one).
    TooRich,
    /// A variant color that fails `ColorVariant::validate_for` for the host.
    BadVariant(VariantProblem),
}

impl ZoneRankFit {
    /// Whether the color can be applied to the host as it is (everything but
    /// [`Self::TooRich`] and [`Self::BadVariant`]).
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        matches!(self, Self::Fits | Self::Legacy)
    }
}

/// The highest rank a zone color may have on `host`: the host's own rank, or uniaxial for a host
/// the material model calls isotropic while `two_colors_allowed` (a birefringent, non-cubic custom
/// material).
#[must_use]
pub const fn allowed_rank(host: &GemMaterial, two_colors_allowed: bool) -> TensorRank {
    match host_rank(host) {
        TensorRank::Isotropic if two_colors_allowed => TensorRank::Uniaxial,
        rank => rank,
    }
}

/// The rank order for comparing colors: isotropic below uniaxial below biaxial.
const fn order(rank: TensorRank) -> u8 {
    match rank {
        TensorRank::Isotropic => 0,
        TensorRank::Uniaxial => 1,
        TensorRank::Biaxial => 2,
    }
}

/// How `color` fits `host` (the rule of the zone color picker: Single on an isotropic host,
/// Along and Across on a uniaxial one, Alpha, Beta and Gamma on a biaxial one).
///
/// `allowed` is the highest rank the host takes ([`allowed_rank`]). A variant color is checked
/// with `ColorVariant::validate_for` against the host (its own Material Default included).
#[must_use]
pub fn zone_rank_fit(color: &PatternColor, host: &GemMaterial, allowed: TensorRank) -> ZoneRankFit {
    if let PatternColor::Variant {
        key,
        label,
        absorption,
    } = color
    {
        let variant = ColorVariant {
            key: key.clone(),
            label: label.clone(),
            material: host.name.clone(),
            absorption: absorption.to_tensor(),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: CHECK_REFERENCE_PATH_MM,
            provenance: if key.ends_with(".default") {
                VariantProvenance::MaterialDefault
            } else {
                VariantProvenance::User
            },
        };
        return match variant.validate_for(host) {
            Ok(()) => ZoneRankFit::Fits,
            Err(problem @ VariantProblem::RankMismatch { .. }) => {
                match order(color.rank()).cmp(&order(allowed)) {
                    // A custom material the model calls isotropic but that may show two colors.
                    std::cmp::Ordering::Equal => ZoneRankFit::Fits,
                    // A simpler color than the host asks for: it renders, like a legacy
                    // one-color zone.
                    std::cmp::Ordering::Less => ZoneRankFit::Legacy,
                    std::cmp::Ordering::Greater => ZoneRankFit::BadVariant(problem),
                }
            }
            Err(problem) => ZoneRankFit::BadVariant(problem),
        };
    }
    let rank = color.rank();
    match order(rank).cmp(&order(allowed)) {
        std::cmp::Ordering::Greater => ZoneRankFit::TooRich,
        std::cmp::Ordering::Less if rank == TensorRank::Isotropic => ZoneRankFit::Legacy,
        // The rank the host asks for, or two colors on a biaxial host (the two-set
        // approximation, which the host accepts).
        std::cmp::Ordering::Equal | std::cmp::Ordering::Less => ZoneRankFit::Fits,
    }
}

impl PatternColor {
    /// The zone color of `variant`: a snapshot of its per-millimeter rows ([`per_mm_rows`]), so
    /// the pattern renders without the variant library, like a design's `VariantChoice`.
    #[must_use]
    pub fn from_variant(variant: &ColorVariant) -> Self {
        Self::Variant {
            key: variant.key.clone(),
            label: variant.label.clone(),
            absorption: per_mm_rows(variant),
        }
    }

    /// The absorption rank of the color: one color is isotropic, two uniaxial, three biaxial; a
    /// variant has the rank of its tensor.
    #[must_use]
    pub const fn rank(&self) -> TensorRank {
        match self {
            Self::Single(_) => TensorRank::Isotropic,
            Self::Dichroic { .. } => TensorRank::Uniaxial,
            Self::Trichroic { .. } => TensorRank::Biaxial,
            Self::Variant { absorption, .. } => absorption.rank(),
        }
    }

    /// Whether this is a picked color ([`Self::Single`], [`Self::Dichroic`] or
    /// [`Self::Trichroic`]) rather than a variant.
    #[must_use]
    pub const fn is_custom(&self) -> bool {
        !matches!(self, Self::Variant { .. })
    }

    /// The key of the variant this color is, if it is one.
    #[must_use]
    pub fn variant_key(&self) -> Option<&str> {
        match self {
            Self::Variant { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Every picked color the zone holds, in slot order (none for a variant).
    #[must_use]
    pub fn zone_colors(&self) -> Vec<&ZoneColor> {
        match self {
            Self::Single(color) => vec![color],
            Self::Dichroic { along, across } => vec![along, across],
            Self::Trichroic { alpha, beta, gamma } => vec![alpha, beta, gamma],
            Self::Variant { .. } => Vec::new(),
        }
    }

    /// This color as a picked one: itself for a custom color, and for a variant the picked color
    /// of the same rank whose band rows are the variant's. `lch_of` seeds the sliders from the
    /// rows.
    ///
    /// A variant the row form cannot carry (a band the row encoding would give the other shape,
    /// see `rows_from_bands`; most measured built-ins have one) never loses that band: its picked
    /// color has the `[L*, C*, h]` of each direction of the variant (`axes_of`, one entry per
    /// direction of its rank) and no rows yet, so the application solves bands for it like for
    /// any picked color.
    #[must_use]
    pub fn to_custom(
        &self,
        lch_of: impl Fn(&[[f32; 3]]) -> [f64; 3],
        axes_of: impl Fn(&AbsorptionTensor) -> Vec<[f64; 3]>,
    ) -> Self {
        let Self::Variant { absorption, .. } = self else {
            return self.clone();
        };
        let tensor = absorption.to_tensor();
        if rows_carry_tensor(&tensor) {
            return pattern_color(&ZoneAbsorption::per_mm(tensor), &lch_of);
        }
        let axes = axes_of(&tensor);
        let color_of_axis = |i: usize| {
            ZoneColor::unsolved(
                axes.get(i)
                    .or_else(|| axes.first())
                    .copied()
                    .unwrap_or(VARIANT_SEED_LCH),
            )
        };
        match absorption.rank() {
            TensorRank::Isotropic => Self::Single(color_of_axis(0)),
            TensorRank::Uniaxial => Self::Dichroic {
                along: color_of_axis(0),
                across: color_of_axis(1),
            },
            TensorRank::Biaxial => Self::Trichroic {
                alpha: color_of_axis(0),
                beta: color_of_axis(1),
                gamma: color_of_axis(2),
            },
        }
    }
}
