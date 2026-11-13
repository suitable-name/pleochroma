//! The mm-per-unit anchor ([`mm_per_unit`]) and the figures downstream of it or of geometry alone
//! ([`volume_mm3`], [`volumetric_yield`], [`carat_weight`]).
//!
//! The parent module explains why masts need a unit and which of these figures are exact.

/// A model-unit measurement too small to trust as a real linear extent -- guards
/// [`mm_per_unit`]/[`super::fit::exceeds_preform`] against dividing by (or comparing
/// against) a collapsing/degenerate solid's near-zero width. Mast units are of order 1
/// (see `crate::preform`'s own module doc comment), so this is many orders of
/// magnitude below anything a real design would ever measure.
pub(super) const MIN_TRUSTED_WIDTH: f64 = 1e-9;

/// Millimetres per model ("mast") unit, anchored on the girdle diameter.
///
/// See this module's doc comment. `width_axis` is the FINISHED, solved design's own
/// measured
/// [`width_axis`](pleochroma::geometry::stone_metrics::SolidMetrics::width_axis) (not the
/// preform's) -- the model-unit girdle width `girdle_diameter_mm` is a real-world
/// measurement of.
///
/// `None` when there is nothing sane to compute from: a non-finite or non-positive
/// `girdle_diameter_mm`, or a `width_axis` too small to trust ([`MIN_TRUSTED_WIDTH`] --
/// a collapsing/degenerate solid has no meaningful width to anchor against).
#[must_use]
pub fn mm_per_unit(girdle_diameter_mm: f64, width_axis: f64) -> Option<f64> {
    if girdle_diameter_mm.is_finite() && girdle_diameter_mm > 0.0 && width_axis > MIN_TRUSTED_WIDTH
    {
        Some(girdle_diameter_mm / width_axis)
    } else {
        None
    }
}

/// Converts a mast-unit volume to mm^3 given a `mm_per_unit` scale factor.
///
/// Volume scales as the **cube** of a linear factor, so this is `volume_units *
/// mm_per_unit.powi(3)`, never `volume_units * mm_per_unit`. The exponent is pinned by
/// `gate_3_changing_girdle_diameter_scales_volume_by_the_cube_of_the_ratio`.
#[must_use]
pub fn volume_mm3(volume_units: f64, mm_per_unit: f64) -> f64 {
    volume_units * mm_per_unit.powi(3)
}

/// Volumetric yield: finished solid volume over preform volume.
///
/// **Exact**, and independent of any mm scale or material data. Both volumes are in
/// the same model-unit system, so converting them to mm^3 multiplies numerator and
/// denominator by the same `mm_per_unit^3`, which cancels. The arguments may be in
/// any one consistent unit, and the function never touches [`mm_per_unit`] or specific
/// gravity. `gate_2_volumetric_yield_is_independent_of_specific_gravity` pins this.
///
/// `None` when `preform_volume` is not positive (nothing to divide by).
#[must_use]
pub fn volumetric_yield(finished_volume: f64, preform_volume: f64) -> Option<f64> {
    (preform_volume > 0.0).then_some(finished_volume / preform_volume)
}

/// Estimated carat weight from a real mm^3 volume and a specific gravity (g/cm^3).
///
/// 1 carat = 0.2 g exactly, and SG is grams per **cm^3**, not mm^3 (1 cm^3 = 1000
/// mm^3), so `mass_g = (volume_mm3 / 1000) * specific_gravity` and `carat_weight =
/// mass_g / 0.2`, i.e. `volume_mm3 * specific_gravity / 200`.
///
/// **An estimate, never more precise than `specific_gravity` itself** (see
/// `crate::material` for how wide a species' SG range can be). It takes a plain
/// `specific_gravity: f64`, not a [`crate::material::MaterialSelection`], so it stays
/// independent of which material supplied the number. [`volumetric_yield`] never calls it.
#[must_use]
pub fn carat_weight(volume_mm3: f64, specific_gravity: f64) -> f64 {
    volume_mm3 * specific_gravity / 200.0
}
