//! Resolves a stone's pattern without changing the shared material preset.

use super::{
    ColorPattern, PATTERN_BASE_FORMAT_VERSION, PATTERN_FORMAT_VERSION, PatternAxis, PatternColor,
    PatternError, ZoneProblem, convert::zone_absorption, follow_noted_pattern,
};
use crate::rough_plan::zoned_plan::resolve_relative;
use pleochroma::optics::{
    materials::{AbsorptionUnit, GemMaterial},
    zoning::ZonedAbsorption,
};

/// Applies a solved color pattern at the stone's physical width in millimetres.
///
/// An unset or invalid width uses the render's 7 mm default. Uniform patterns replace
/// the base absorption and remove zoning; a nonuniform pattern replaces both. No
/// other optical property is changed, and the material's own preset is never mutated.
///
/// A pattern of any format version from the first release up to [`PATTERN_FORMAT_VERSION`] is
/// accepted, so stored version 1 patterns keep loading. A zone color's rank is not checked
/// against `material` (a stored one-color zone on a pleochroic host keeps rendering as saved; the
/// editor labels it): the zone tensors are applied as stored.
///
/// # Errors
///
/// Returns the pattern's validation error when its colors or geometry cannot render, and
/// [`PatternError::UnsupportedVersion`] for a version above [`PATTERN_FORMAT_VERSION`].
pub fn apply_color_pattern(
    mut material: GemMaterial,
    pattern: &ColorPattern,
    stone_width_mm: f64,
) -> Result<GemMaterial, PatternError> {
    // Every version up to the newest this build reads: a stored version 1 pattern keeps loading.
    if pattern.version < PATTERN_BASE_FORMAT_VERSION || pattern.version > PATTERN_FORMAT_VERSION {
        return Err(PatternError::UnsupportedVersion(pattern.version));
    }
    if let PatternAxis::Fixed { tilt_rad, turn_rad } = pattern.axis
        && !(tilt_rad.is_finite() && turn_rad.is_finite())
    {
        return Err(PatternError::BadZone {
            zone: None,
            problem: ZoneProblem::BadAxis,
        });
    }
    for zone in std::iter::once(&pattern.base).chain(&pattern.zones) {
        let bad_variant = matches!(
            &zone.color,
            PatternColor::Variant { absorption, .. } if !absorption.is_usable()
        );
        if bad_variant
            || zone.color.zone_colors().into_iter().any(|color| {
                !color.lch.iter().all(|value| value.is_finite())
                    || color.bands.iter().any(|row| {
                        !row.iter().all(|value| value.is_finite()) || row[1] <= 0.0 || row[2] < 0.0
                    })
            })
        {
            return Err(PatternError::BadZone {
                zone: Some(zone.name.clone()),
                problem: ZoneProblem::BadBands,
            });
        }
    }
    let base = zone_absorption(&pattern.base)?;
    // `to_zoned` deliberately returns None for Uniform, so validate its base separately.
    let mut base_validation = ZonedAbsorption::new(base.clone());
    base_validation.frame = pattern.axis.frame();
    base_validation.boundary_softness_mm = pattern.softness;
    base_validation
        .validate()
        .map_err(|error| PatternError::from_zoning(error, &pattern.base, &pattern.zones))?;
    let width = if stone_width_mm.is_finite() && stone_width_mm > 0.0 {
        stone_width_mm
    } else {
        7.0
    };
    let zoning = pattern
        .to_zoned()?
        .map(|zones| {
            if pattern.relative_to_stone {
                resolve_relative(&zones, width).ok_or(PatternError::BadZone {
                    zone: None,
                    problem: ZoneProblem::NotFinite,
                })
            } else {
                Ok(zones)
            }
        })
        .transpose()?;
    material.absorption = base.tensor;
    material.absorption_unit = AbsorptionUnit::PerMm;
    // Render setup replaces this with physical width / actual model width. Keeping
    // the normal unit-width preview scale also makes direct material consumers sane.
    material.absorption_path_scale = 3.5;
    material.zoning = zoning;
    follow_noted_pattern(&mut material);
    Ok(material)
}
