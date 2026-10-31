//! The storage API of Color Patterns.
//!
//! Every function takes the vault [`Database`] (callers hold the `Mutex` guard) and returns an
//! `anyhow::Result`; "nothing stored" is `Ok(None)` / `Ok(false)` / an empty collection, never an
//! error, and a stored text that no longer parses is an `Err` with a sentence the UI can show.
//!
//! # What a save writes
//!
//! One transaction ([`Database::save_material_pattern_with_zoning`]): the material row (its
//! band column is the BASE zone's band rows, so a build that knows no zones shows the base color),
//! the `material_pattern` row, and the `material_zoning` row (the resolved zones, deleted for a
//! pattern without shaped zones). The zone geometry is stored as the pattern holds it: unit-wide
//! when the pattern is relative to the stone, millimeters otherwise.
//!
//! # Function index
//!
//! | area | functions |
//! |---|---|
//! | a material's pattern | [`save_pattern`], [`load_pattern`], [`remove_pattern`], [`copy_pattern`] |
//! | material dialog | [`write_back_pattern_base`] |
//! | presets | [`save_preset`], [`load_presets`] |

use anyhow::{Context, Result, bail};
use pleochroma_cut_core::{
    material::{
        absorption_bands_to_json,
        pattern::{
            ColorPattern, ColorRef, ColorSlot, PatternColor, PatternError, ZoneColor,
            bands_from_rows, decode_pattern, encode_pattern, rows_from_bands,
        },
    },
    rough_plan::zoned_plan::{ZONING_FORMAT_VERSION, encode_zoned, resolve_relative},
};
use pleochroma_vault::{
    db::sqlite::{CustomMaterialParams, Database},
    model::zoning::{ColorPatternPresetRow, MaterialPatternRow, MaterialZoningRow},
};
use tracing::warn;

/// The band rows of the pattern's base zone, as the material row stores them: the Single color,
/// or the Along (Alpha) color of a Two- or Three-Color base, or the `o_ray` set of a variant base.
/// Rows the material model cannot use are dropped (a stored column holding one would be ignored as
/// a whole).
fn base_rows(pattern: &ColorPattern) -> Vec<[f32; 3]> {
    let color = match &pattern.base.color {
        PatternColor::Single(color) => color,
        PatternColor::Dichroic { along, .. } => along,
        PatternColor::Trichroic { alpha, .. } => alpha,
        PatternColor::Variant { absorption, .. } => {
            let bands: Vec<_> = absorption
                .o_ray
                .iter()
                .map(pleochroma::optics::materials::variants::VariantBandRow::to_band)
                .collect();
            return rows_from_bands(&bands);
        }
    };
    rows_from_bands(&bands_from_rows(&color.bands))
}

/// Stores `pattern` as the pattern of the saved custom material `material_name`.
///
/// The material row is rebuilt from its CURRENT row (like `adopt_stone`) with the base zone's band
/// rows; a color recipe or legacy triple would take precedence over those bands, so both are
/// cleared. `stone_width_mm` is the width a relative pattern must resolve at (not positive: not
/// checked).
///
/// # Errors
///
/// A color is not solved yet, the pattern cannot become zones, the material is not saved in the
/// vault, a relative pattern does not resolve at `stone_width_mm`, or the vault write fails (the
/// transaction is then rolled back and nothing changed).
pub fn save_pattern(
    db: &Database,
    material_name: &str,
    pattern: &ColorPattern,
    stone_width_mm: f64,
) -> Result<()> {
    if let Some(at) = pattern.unsolved_colors().first() {
        return Err(PatternError::NeedsColor {
            zone: pattern
                .zone(at.zone)
                .map(|zone| zone.name.clone())
                .unwrap_or_default(),
        }
        .into());
    }
    let zoned = pattern.to_zoned()?;
    let Some(row) = db
        .get_custom_materials()?
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(material_name))
    else {
        bail!("Save the material '{material_name}' before giving it a color pattern");
    };
    let name = row.name.as_str();

    let zoning_row = match &zoned {
        Some(zoned) => {
            if pattern.relative_to_stone
                && stone_width_mm.is_finite()
                && stone_width_mm > 0.0
                && resolve_relative(zoned, stone_width_mm).is_none()
            {
                bail!(
                    "The pattern zones of '{name}' cannot be scaled to a stone width of {stone_width_mm} mm"
                );
            }
            Some(MaterialZoningRow {
                material_name: name.to_string(),
                zoned_json: encode_zoned(zoned)
                    .with_context(|| format!("The color zones of '{name}' could not be saved"))?,
                relative_to_stone: pattern.relative_to_stone,
                version: ZONING_FORMAT_VERSION,
            })
        }
        None => None,
    };
    let pattern_row = MaterialPatternRow {
        material_name: name.to_string(),
        pattern_json: encode_pattern(pattern)
            .with_context(|| format!("The color pattern of '{name}' could not be saved"))?,
        version: pattern.format_version(),
    };
    let bands_json = absorption_bands_to_json(&base_rows(pattern));
    db.save_material_pattern_with_zoning(
        &CustomMaterialParams {
            name,
            refractive_index: row.refractive_index,
            dispersion: row.dispersion,
            birefringence: row.birefringence,
            absorption_rgb: [0.0; 3],
            crystal_system: row.crystal_system.as_deref(),
            optical_character: row.optical_character.as_deref(),
            biaxial_delta_beta_alpha: row.biaxial_delta_beta_alpha,
            per_axis_dispersion_json: row.per_axis_dispersion_json.as_deref(),
            specific_gravity: row.specific_gravity,
            color_recipe_json: None,
            dispersion_model_json: row.dispersion_model_json.as_deref(),
            absorption_bands_json: bands_json.as_deref(),
            c_axis_tilt_deg: row.c_axis_tilt_deg,
            c_axis_azimuth_deg: row.c_axis_azimuth_deg,
            c_axis_second_azimuth_deg: row.c_axis_second_azimuth_deg,
            inclusion_families_json: row.inclusion_families_json.as_deref(),
            structure_json: row.structure_json.as_deref(),
        },
        &pattern_row,
        zoning_row.as_ref(),
    )
    .with_context(|| format!("The color pattern of '{name}' could not be saved"))
}

/// The pattern stored for custom material `material_name`, or `None` when it has none.
///
/// # Errors
///
/// The vault read fails, or the stored text was written by a newer build or does not parse.
pub fn load_pattern(db: &Database, material_name: &str) -> Result<Option<ColorPattern>> {
    let Some(row) = db.load_material_pattern(material_name)? else {
        return Ok(None);
    };
    decode_pattern(&row.pattern_json)
        .map(Some)
        .with_context(|| format!("The color pattern of '{material_name}' cannot be used"))
}

/// Removes the pattern AND the zones of `material_name`; the material row keeps its base color.
/// Nothing stored is a no-op that succeeds.
///
/// # Errors
///
/// The vault write fails.
pub fn remove_pattern(db: &Database, material_name: &str) -> Result<()> {
    db.delete_material_pattern_and_zoning(material_name)
}

/// Copies the pattern of `from` to `to` (a save under a new name), unless `to` has a pattern of
/// its own or `from` has none. Returns whether a pattern was copied. The zones are copied by the
/// material save itself.
///
/// # Errors
///
/// The vault read or write fails.
pub fn copy_pattern(db: &Database, from: &str, to: &str) -> Result<bool> {
    if from.eq_ignore_ascii_case(to) || db.load_material_pattern(to)?.is_some() {
        return Ok(false);
    }
    let Some(row) = db.load_material_pattern(from)? else {
        return Ok(false);
    };
    db.save_material_pattern(&MaterialPatternRow {
        material_name: to.to_string(),
        ..row
    })?;
    Ok(true)
}

/// Writes an edited body color into the stored pattern's base zone: the band rows `bands`, and
/// `lch` (the sliders the dialog used) when it has them, else the old sliders stay.
///
/// Returns whether the stored pattern changed (`false`: no pattern, a Two-Color base, which is
/// edited in the pattern editor only, or the same color).
///
/// # Errors
///
/// A band is not a number, the stored pattern cannot be read, or the vault write fails.
pub fn write_back_pattern_base(
    db: &Database,
    name: &str,
    bands: &[[f32; 3]],
    lch: Option<[f64; 3]>,
) -> Result<bool> {
    let Some(mut pattern) = load_pattern(db, name)? else {
        return Ok(false);
    };
    if bands.iter().any(|row| row.iter().any(|v| !v.is_finite())) {
        bail!("The body color of '{name}' has a band that is not a number");
    }
    let base = ColorRef {
        zone: 0,
        slot: ColorSlot::Single,
    };
    let Some(current) = pattern.color(base) else {
        return Ok(false);
    };
    let rows = rows_from_bands(&bands_from_rows(bands));
    let lch = lch.unwrap_or(current.lch);
    if current.is_solved() && current.bands == rows && current.lch == lch {
        return Ok(false);
    }
    pattern.set_color(base, ZoneColor::solved(lch, rows));
    db.save_material_pattern(&MaterialPatternRow {
        material_name: name.to_string(),
        pattern_json: encode_pattern(&pattern)
            .with_context(|| format!("The color pattern of '{name}' could not be saved"))?,
        version: pattern.format_version(),
    })?;
    Ok(true)
}

// ---- presets ---------------------------------------------------------------------------------

/// Stores (replaces) the preset called `name` (trimmed; ASCII case is ignored).
///
/// # Errors
///
/// The name is blank, the pattern cannot be stored, or the vault write fails.
pub fn save_preset(db: &Database, name: &str, pattern: &ColorPattern) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Give the preset a name");
    }
    db.save_color_pattern_preset(&ColorPatternPresetRow {
        name: name.to_string(),
        pattern_json: encode_pattern(pattern)
            .with_context(|| format!("The preset '{name}' could not be saved"))?,
        version: pattern.format_version(),
    })
}

/// Every saved preset, by name. A preset that can no longer be read (a newer build wrote it, or
/// its text is damaged) is skipped with a log line.
///
/// # Errors
///
/// The vault read fails.
pub fn load_presets(db: &Database) -> Result<Vec<(String, ColorPattern)>> {
    let mut presets = Vec::new();
    for row in db.load_color_pattern_presets()? {
        match decode_pattern(&row.pattern_json) {
            Ok(pattern) => presets.push((row.name, pattern)),
            Err(error) => warn!("Color pattern preset '{}' was skipped: {error}", row.name),
        }
    }
    Ok(presets)
}
