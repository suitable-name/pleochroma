//! The JSON text a pattern is stored as (the vault's `material_pattern` and
//! `color_pattern_presets` rows, the design file's `[material.custom.pattern]` table).
//!
//! Everything read back is hostile input (a hand-edited file, a newer build's row), so
//! [`decode_pattern`] refuses a newer version and checks the structure it parsed. Colors may be
//! unsolved in a stored pattern (a draft); geometry is validated when the pattern becomes zones
//! ([`ColorPattern::to_zoned`]).

use super::{ColorPattern, MAX_PATTERN_ZONES, PATTERN_FORMAT_VERSION, PatternColor, ZoneColor};
use std::fmt;

/// The longest pattern text a reader accepts, in bytes (a pattern is a few kilobytes; this is
/// only a guard against garbage).
pub const MAX_PATTERN_TEXT_BYTES: usize = 1024 * 1024;

/// Why a stored pattern text could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternCodecError {
    /// The text is not the expected JSON.
    Json(String),
    /// The text parsed but describes a pattern that cannot be used.
    Invalid(String),
    /// The pattern was written by a newer build.
    UnsupportedVersion(u32),
    /// The text is longer than [`MAX_PATTERN_TEXT_BYTES`].
    TooLarge(usize),
}

impl fmt::Display for PatternCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "The stored pattern is not readable: {message}"),
            Self::Invalid(message) => write!(f, "The stored pattern cannot be used: {message}"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "The stored pattern was written by a newer version (format {version}). \
                 This version reads formats up to {PATTERN_FORMAT_VERSION}."
            ),
            Self::TooLarge(bytes) => {
                write!(f, "The stored pattern text is too large ({bytes} bytes).")
            }
        }
    }
}

impl std::error::Error for PatternCodecError {}

fn color_is_sane(color: &ZoneColor) -> bool {
    color.lch.iter().all(|v| v.is_finite())
        && color
            .bands
            .iter()
            .all(|row| row.iter().all(|v| v.is_finite()))
}

/// Checks what serde cannot: the zone count, the softness and the finiteness of every color.
fn check_structure(pattern: &ColorPattern) -> Result<(), PatternCodecError> {
    if pattern.zones.len() > MAX_PATTERN_ZONES {
        return Err(PatternCodecError::Invalid(
            "it holds more zones than a pattern may".to_owned(),
        ));
    }
    if !(pattern.softness.is_finite() && pattern.softness >= 0.0) {
        return Err(PatternCodecError::Invalid(
            "its edge softness is not a number of zero or more".to_owned(),
        ));
    }
    let sane = std::iter::once(&pattern.base)
        .chain(&pattern.zones)
        .all(|zone| match &zone.color {
            PatternColor::Variant { absorption, .. } => absorption.is_usable(),
            color => color.zone_colors().into_iter().all(color_is_sane),
        });
    if sane {
        Ok(())
    } else {
        Err(PatternCodecError::Invalid(
            "a color holds a value that is not a number".to_owned(),
        ))
    }
}

/// The pattern as JSON text, with [`ColorPattern::format_version`] as its version.
///
/// # Errors
///
/// [`PatternCodecError::Invalid`] for a pattern [`decode_pattern`] would refuse,
/// [`PatternCodecError::Json`] if serialisation fails.
pub fn encode_pattern(pattern: &ColorPattern) -> Result<String, PatternCodecError> {
    check_structure(pattern)?;
    // A pattern is written with the lowest version that holds it, never below its own: only a
    // pattern with a variant or a three-color zone is a version 2 file, so an older build still
    // reads every other pattern.
    let version = pattern.format_version();
    let text = if version == pattern.version {
        serde_json::to_string(pattern)
    } else {
        serde_json::to_string(&ColorPattern {
            version,
            ..pattern.clone()
        })
    };
    text.map_err(|error| PatternCodecError::Json(error.to_string()))
}

/// The pattern of a stored text.
///
/// # Errors
///
/// [`PatternCodecError::TooLarge`], [`PatternCodecError::Json`] for text that does not parse,
/// [`PatternCodecError::UnsupportedVersion`] for a newer version and
/// [`PatternCodecError::Invalid`] for a pattern that fails the structure check.
pub fn decode_pattern(text: &str) -> Result<ColorPattern, PatternCodecError> {
    if text.len() > MAX_PATTERN_TEXT_BYTES {
        return Err(PatternCodecError::TooLarge(text.len()));
    }
    let json_error = |error: serde_json::Error| PatternCodecError::Json(error.to_string());
    let value: serde_json::Value = serde_json::from_str(text).map_err(json_error)?;
    // A newer build may have changed the layout, so the version is read before the rest.
    if let Some(version) = value.get("version").and_then(serde_json::Value::as_u64)
        && version > u64::from(PATTERN_FORMAT_VERSION)
    {
        return Err(PatternCodecError::UnsupportedVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }
    let pattern: ColorPattern = serde_json::from_value(value).map_err(json_error)?;
    check_structure(&pattern)?;
    Ok(pattern)
}
