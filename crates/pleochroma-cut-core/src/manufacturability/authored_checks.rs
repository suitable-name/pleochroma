//! Checks 3, 4 and 5: [`check_gear_quantization`], [`check_cut_order`] and
//! [`check_meet_name_asc_safety`], the three that need no solved mast at all --
//! see the parent module's doc comment. All three read `design`'s raw authored
//! state directly, so they run even on a design that has never been solved.

use super::warning::ManufacturabilityWarning;
use crate::design::{Design, meet_name_is_asc_safe};
use pleochroma::geometry::meet_solver::{MeetConstraint, MeetNameResolver};

/// An index within this absolute distance of an integer counts as landing on a real gear
/// tooth. `.asc` index tokens are plain decimal literals (`0`, `24`, `47.5`), so this only
/// absorbs text and float round-trip noise. A genuine half-tooth position such as `47.5`
/// is far above it.
const GEAR_QUANTIZATION_EPS: f64 = 1e-9;

/// Check 3: an authored index-wheel position does not land on a real gear
/// tooth -- see the module docs.
///
/// Needs no mast at all, so this runs off `design`'s raw authored `indices`
/// regardless of whether the design has ever been solved.
///
/// Reports nothing when `design.meta.gear_teeth_abs() == 0`. With no real gear stated
/// there is nothing to quantize against, and a `.max(1)` fallback would invent a
/// one-tooth wheel and a meaningless `azimuth_error_deg`.
#[must_use]
pub fn check_gear_quantization(design: &Design) -> Vec<ManufacturabilityWarning> {
    let gear_teeth_abs = design.meta.gear_teeth_abs();
    if gear_teeth_abs == 0 {
        return Vec::new();
    }
    let gear_teeth = f64::from(gear_teeth_abs);
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        for &requested in &tier.indices {
            let achievable = requested.round();
            let delta = requested - achievable;
            if delta.abs() > GEAR_QUANTIZATION_EPS {
                warnings.push(ManufacturabilityWarning::FractionalIndex {
                    tier_index,
                    tier_id: design.tier_id_at_or_synthetic(tier_index),
                    tier_name: tier.name.clone(),
                    requested,
                    achievable,
                    azimuth_error_deg: 360.0 * delta / gear_teeth,
                });
            }
        }
    }
    warnings
}

/// Check 4: a tier's stated meet target comes at or after the tier itself in
/// the schedule -- see the module docs.
///
/// Needs no mast either. It resolves `design`'s authored [`MeetConstraint::MeetNamed`]
/// references with the same [`MeetNameResolver`] the solver uses, so the two cannot
/// disagree about which tier a name means. It reports a resolved target whose index is
/// `>=` the tier's own index. A self-reference counts too, since a tier cannot meet a
/// facet it has not cut yet.
///
/// An unresolved name, or one the schedule states as [`MeetConstraint::MeetExisting`]/
/// [`MeetConstraint::ScaleReference`], is out of scope here -- this check is
/// specifically about a reference that *did* resolve to a real tier, just the
/// wrong-in-time one.
#[must_use]
pub fn check_cut_order(design: &Design) -> Vec<ManufacturabilityWarning> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let MeetConstraint::MeetNamed(names) = &tier.constraint else {
            continue;
        };
        let resolution = resolver.resolve_names(names);
        for &target in &resolution.refs {
            if target >= tier_index {
                warnings.push(ManufacturabilityWarning::OutOfOrderMeet {
                    tier_index,
                    tier_id: design.tier_id_at_or_synthetic(tier_index),
                    tier_name: tier.name.clone(),
                    target_tier_index: target,
                    target_tier_name: design.tiers[target].name.clone(),
                });
            }
        }
    }
    warnings
}

/// Check 5: a tier's [`MeetConstraint::MeetNamed`] names a target that would not
/// survive a plain `.asc` export/re-import.
///
/// See [`meet_name_is_asc_safe`]'s own doc comment for exactly which names fail
/// and why (`crate::design::export`'s own `"Meet <names>"` text is the export
/// this guards). Needs no mast either, same as [`check_cut_order`]: purely a
/// property of the tier's own authored name list.
#[must_use]
pub fn check_meet_name_asc_safety(design: &Design) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let MeetConstraint::MeetNamed(names) = &tier.constraint else {
            continue;
        };
        let unsafe_names: Vec<String> = names
            .iter()
            .filter(|name| !meet_name_is_asc_safe(name.as_str()))
            .cloned()
            .collect();
        if !unsafe_names.is_empty() {
            warnings.push(ManufacturabilityWarning::MeetNameNotAscSafe {
                tier_index,
                tier_id: design.tier_id_at_or_synthetic(tier_index),
                tier_name: tier.name.clone(),
                unsafe_names,
            });
        }
    }
    warnings
}
