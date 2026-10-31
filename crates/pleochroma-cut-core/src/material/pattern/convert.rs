//! Pattern to zones and back: [`ColorPattern::to_zoned`] and [`ColorPattern::from_zoned`].

use super::{
    ColorPattern, MAX_PATTERN_ZONES, PATTERN_BASE_FORMAT_VERSION, PatternAxis, PatternColor,
    PatternError, PatternTensorRows, PatternZone, ZoneColor,
};
use pleochroma::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
    zoning::{Zone, ZoneAbsorption, ZonedAbsorption},
};

/// The width above which a row is read as an energy band (a legacy encoding, see
/// [`bands_from_rows`]).
const ENERGY_WIDTH_THRESHOLD: f32 = 500.0;

/// The key of the [`PatternColor::Variant`] that keeps a color the row form cannot carry.
///
/// See `pattern_color`. It is not a variant of any material's list, so the color picker shows it with
/// the "kept in pattern" note.
pub const EXACT_COLOR_KEY: &str = "zone.exact";

/// The label of that variant.
pub const EXACT_COLOR_LABEL: &str = "Current Color";

/// A row is usable when every number is finite and its centre, width and amplitude are positive
/// (the rule of `GemMaterial::with_body_color_bands` and `absorption_bands_from_json`; a centre
/// of zero would divide by zero in an energy band's `1e7 / centre`).
fn usable(row: &[f32; 3]) -> bool {
    row.iter().all(|v| v.is_finite()) && row[0] > 0.0 && row[1] > 0.0 && row[2] > 0.0
}

/// The absorption bands of body-color rows (`[centre_nm, width_nm, amplitude_per_mm]` each).
///
/// The same rule as `GemMaterial::with_body_color_bands`: a row with a width above 500 is an
/// energy band (the width is the wavenumber sigma, restored exactly), any other row is a
/// Gaussian in wavelength; non-positive or non-finite amplitude, width OR CENTRE rows are
/// dropped. A test keeps the two in lock-step.
///
/// The width threshold is a LEGACY ENCODING: the row has no shape tag, so an energy band whose
/// sigma is at most 500 cm^-1 reads back as a wavelength Gaussian of that many nm, and a
/// wavelength band whose sigma exceeds 500 nm reads back as an energy band. A versioned band
/// representation is pending (owner decision D5); until then [`rows_from_bands`] refuses to write
/// a band that would change shape.
#[must_use]
pub fn bands_from_rows(rows: &[[f32; 3]]) -> Vec<AbsorptionBand> {
    rows.iter()
        .filter(|row| usable(row))
        .map(|row| {
            if row[1] > ENERGY_WIDTH_THRESHOLD {
                AbsorptionBand {
                    center_nm: row[0],
                    width_nm: row[1],
                    peak: row[2],
                    shape: BandShape::GaussianEnergy,
                }
            } else {
                AbsorptionBand::new(row[0], row[1], row[2])
            }
        })
        .collect()
}

/// Whether the row of `band` decodes to a band of the same shape ([`bands_from_rows`]).
fn keeps_shape(band: &AbsorptionBand) -> bool {
    match band.shape {
        BandShape::GaussianEnergy => band.width_nm > ENERGY_WIDTH_THRESHOLD,
        BandShape::GaussianWavelength => band.width_nm <= ENERGY_WIDTH_THRESHOLD,
    }
}

/// Whether the row form carries every band of `bands` that has an effect (a peak above zero)
/// unchanged: `bands_from_rows(rows_from_bands(bands))` gives them back.
#[must_use]
pub fn rows_carry_bands(bands: &[AbsorptionBand]) -> bool {
    bands
        .iter()
        .filter(|band| band.peak != 0.0)
        .all(|band| usable(&[band.center_nm, band.width_nm, band.peak]) && keeps_shape(band))
}

/// Whether the row form carries every band set of `tensor` ([`rows_carry_bands`]).
///
/// A tensor it does not carry (a measured energy band narrower than 500 cm^-1 sigma, say) becomes a
/// [`PatternColor::Variant`] of exact rows when a pattern is made of it, never a picked color
/// with bands left out.
#[must_use]
pub fn rows_carry_tensor(tensor: &AbsorptionTensor) -> bool {
    [&tensor.o_ray, &tensor.e_ray]
        .into_iter()
        .chain(tensor.beta_ray.as_ref())
        .all(|bands| rows_carry_bands(bands))
}

/// Whether a pattern can hold `tensor` without changing it.
///
/// As a picked color ([`rows_carry_tensor`]), else as a color variant of exact, shape-tagged rows (what
/// [`PatternColor::Variant`] stores; the rows must pass [`PatternTensorRows::is_usable`]).
#[must_use]
pub fn pattern_holds_tensor(tensor: &AbsorptionTensor) -> bool {
    rows_carry_tensor(tensor) || PatternTensorRows::from_tensor(tensor).is_usable()
}

/// The body-color rows of absorption bands: the inverse of [`bands_from_rows`].
///
/// Energy and wavelength bands alike give `[center_nm, width_nm, peak]`, the width telling them
/// apart. Unusable bands are dropped, and so is a band whose row would decode to the other shape
/// (an energy band needs a width above 500 cm^-1, a wavelength band one of at most 500 nm; see
/// [`bands_from_rows`]), with a warning: writing it would silently change the band's spectrum.
/// A caller that must not lose a band asks [`rows_carry_bands`] first.
#[must_use]
pub fn rows_from_bands(bands: &[AbsorptionBand]) -> Vec<[f32; 3]> {
    bands
        .iter()
        .filter(|band| usable(&[band.center_nm, band.width_nm, band.peak]))
        .filter(|band| {
            let kept = keeps_shape(band);
            if !kept {
                tracing::warn!(
                    center_nm = band.center_nm,
                    width_nm = band.width_nm,
                    "band row would not round-trip its shape; dropped"
                );
            }
            kept
        })
        .map(|band| [band.center_nm, band.width_nm, band.peak])
        .collect()
}

/// The per-millimeter tensor of a zone's color, or the zone's name when a color is unsolved.
pub(super) fn zone_absorption(zone: &PatternZone) -> Result<ZoneAbsorption, PatternError> {
    let needs_color = || PatternError::NeedsColor {
        zone: zone.name.clone(),
    };
    let tensor = match &zone.color {
        PatternColor::Single(color) => {
            if !color.is_solved() {
                return Err(needs_color());
            }
            AbsorptionTensor::isotropic(bands_from_rows(&color.bands))
        }
        PatternColor::Dichroic { along, across } => {
            if !along.is_solved() || !across.is_solved() {
                return Err(needs_color());
            }
            AbsorptionTensor::uniaxial(
                bands_from_rows(&along.bands),
                bands_from_rows(&across.bands),
            )
        }
        PatternColor::Trichroic { alpha, beta, gamma } => {
            if !alpha.is_solved() || !beta.is_solved() || !gamma.is_solved() {
                return Err(needs_color());
            }
            AbsorptionTensor::biaxial(
                bands_from_rows(&alpha.bands),
                bands_from_rows(&beta.bands),
                bands_from_rows(&gamma.bands),
            )
        }
        // Per millimeter by construction (`PatternColor::from_variant`), stored exactly.
        PatternColor::Variant { absorption, .. } => absorption.to_tensor(),
    };
    Ok(ZoneAbsorption::per_mm(tensor))
}

/// The pattern color of a zone tensor: the `o_ray` rows, and for a pleochroic zone the `e_ray`
/// rows as the Across color; a pleochroic tensor with a third (`beta_ray`) set becomes a
/// [`PatternColor::Trichroic`] color (alpha = `o_ray`, beta = `beta_ray`, gamma = `e_ray`).
/// `lch_of` seeds the sliders from band rows.
///
/// A tensor the row form cannot carry ([`rows_carry_tensor`]) is kept exactly instead, as a
/// [`PatternColor::Variant`] of its shape-tagged rows (key [`EXACT_COLOR_KEY`]): the rows of a
/// picked color would leave its narrow energy bands out and change the stone's color.
pub(super) fn pattern_color(
    absorption: &ZoneAbsorption,
    lch_of: &impl Fn(&[[f32; 3]]) -> [f64; 3],
) -> PatternColor {
    let color = |bands: &[AbsorptionBand]| {
        let rows = rows_from_bands(bands);
        ZoneColor::solved(lch_of(&rows), rows)
    };
    let tensor = &absorption.tensor;
    if !rows_carry_tensor(tensor) {
        let exact = PatternTensorRows::from_tensor(tensor);
        if exact.is_usable() {
            return PatternColor::Variant {
                key: EXACT_COLOR_KEY.to_owned(),
                label: EXACT_COLOR_LABEL.to_owned(),
                absorption: exact,
            };
        }
    }
    if let (true, Some(beta)) = (tensor.is_pleochroic, &tensor.beta_ray) {
        PatternColor::Trichroic {
            alpha: color(&tensor.o_ray),
            beta: color(beta),
            gamma: color(&tensor.e_ray),
        }
    } else if tensor.is_pleochroic {
        PatternColor::Dichroic {
            along: color(&tensor.o_ray),
            across: color(&tensor.e_ray),
        }
    } else {
        PatternColor::Single(color(&tensor.o_ray))
    }
}

impl ColorPattern {
    /// The zones this pattern stands for: base and zones as per-millimeter tensors (a
    /// [`PatternColor::Dichroic`] color as a uniaxial tensor, `o_ray` = Along, `e_ray` = Across,
    /// `is_pleochroic = true`; a [`PatternColor::Trichroic`] one as a biaxial tensor, alpha,
    /// beta and gamma as `o_ray`, `beta_ray` and `e_ray`; a [`PatternColor::Variant`] as the
    /// tensor of its stored rows), the frame from [`Self::axis`], the geometry as stored (for a
    /// unit-wide stone when [`Self::relative_to_stone`]; resolving it to millimeters is
    /// `rough_plan::zoned_plan::resolve_relative`) and [`Self::softness`] as the boundary
    /// softness.
    ///
    /// `Ok(None)` when [`Self::zones`] is empty: a Uniform pattern stores no zones, so it is the
    /// plain material.
    ///
    /// # Errors
    ///
    /// [`PatternError::NeedsColor`] when a color has no band rows yet,
    /// [`PatternError::TooManyZones`] for more than [`MAX_PATTERN_ZONES`] zones, and
    /// [`PatternError::BadZone`] when `ZonedAbsorption::validate` refuses the result.
    pub fn to_zoned(&self) -> Result<Option<ZonedAbsorption>, PatternError> {
        if self.zones.is_empty() {
            return Ok(None);
        }
        if self.zones.len() > MAX_PATTERN_ZONES {
            return Err(PatternError::TooManyZones);
        }
        let mut zoned = ZonedAbsorption::new(zone_absorption(&self.base)?);
        zoned.frame = self.axis.frame();
        zoned.boundary_softness_mm = self.softness;
        zoned.follows_axis = self.axis.follows_crystal_axis();
        for zone in &self.zones {
            zoned.zones.push(Zone {
                shape: zone.shape.clone(),
                absorption: zone_absorption(zone)?,
            });
        }
        zoned
            .validate()
            .map_err(|error| PatternError::from_zoning(error, &self.base, &self.zones))?;
        Ok(Some(zoned))
    }

    /// A pattern that renders exactly like `zoned` (the Rough Color wizard's zones): each
    /// tensor's `o_ray` rows are the zone's band rows (the color fit uses the body-color basis,
    /// so they import exactly); a pleochroic tensor imports as a two-color zone, `o_ray` as the
    /// Along color and `e_ray` as the Across color; a tensor with a third (`beta_ray`) set as a
    /// three-color zone ([`PatternColor::Trichroic`]). The frame is kept bit for bit
    /// ([`PatternAxis::from_frame`]). `relative` says whether the geometry is for a unit-wide
    /// stone. `lch_of` seeds each color's sliders from its band rows.
    ///
    /// The zones are named "Zone 1", "Zone 2" and so on, the base "Base Zone"; the pattern has
    /// no template. Its version is [`PATTERN_BASE_FORMAT_VERSION`], or the newer one when a zone
    /// has three colors ([`ColorPattern::required_format_version`]).
    ///
    /// A pattern seeded from a biaxial material (a tensor with a `beta_ray` set) is therefore a
    /// version 2 pattern: a build older than the three-color zones refuses that design's pattern
    /// with its "newer version" sentence, where it used to open it with the beta set dropped. This
    /// is by design; say so in the release note. A uniaxial or isotropic seed stays version 1.
    #[must_use]
    pub fn from_zoned(
        zoned: &ZonedAbsorption,
        relative: bool,
        lch_of: impl Fn(&[[f32; 3]]) -> [f64; 3],
    ) -> Self {
        Self {
            version: PATTERN_BASE_FORMAT_VERSION,
            template: None,
            base: PatternZone::base("Base Zone", pattern_color(&zoned.base, &lch_of)),
            zones: zoned
                .zones
                .iter()
                .enumerate()
                .map(|(i, zone)| PatternZone {
                    name: format!("Zone {}", i + 1),
                    shape: zone.shape.clone(),
                    color: pattern_color(&zone.absorption, &lch_of),
                })
                .collect(),
            softness: zoned.boundary_softness_mm,
            axis: if zoned.follows_axis {
                PatternAxis::FollowsCrystalAxis
            } else {
                PatternAxis::from_frame(&zoned.frame)
            },
            relative_to_stone: relative,
        }
        .with_required_version()
    }
}
