//! Why a pattern cannot be turned into zones, in sentences a cutter can read.

use super::PatternZone;
use pleochroma::optics::zoning::ZoningError;
use std::fmt;

/// What is wrong with one zone of a pattern (a [`ZoningError`] without the zone numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneProblem {
    /// A number is not finite.
    NotFinite,
    /// A direction does not have a length of one.
    NotUnit,
    /// The zone's color is not in the unit the renderer needs.
    BadUnit,
    /// A color has a band that cannot be used.
    BadBands,
    /// A layer's lower edge is not below its upper edge.
    BadLayer,
    /// A tube or prism with an outer radius that is not larger than the inner one.
    BadRadii,
    /// A prism with fewer than 3 or more than 12 sides.
    BadSideCount,
    /// A sector with an end angle that is not after the start or more than a turn later.
    BadAngles,
    /// A negative or non-finite edge softness.
    BadSoftness,
    /// The pattern axis is not a valid rotation.
    BadAxis,
    /// A freeform zone with a soft edge.
    SoftFreeform,
    /// A freeform zone that is not a closed shape.
    BadFreeform,
}

impl fmt::Display for ZoneProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "A zone has a value that is not a number.",
            Self::NotUnit => "A zone's direction must have a length of one.",
            Self::BadUnit => "A zone's color could not be used.",
            Self::BadBands => "A zone's color has a band that cannot be used.",
            Self::BadLayer => "A layer's lower edge must be below its upper edge.",
            Self::BadRadii => "A zone's outer radius must be larger than its inner radius.",
            Self::BadSideCount => "A prism zone needs between 3 and 12 sides.",
            Self::BadAngles => {
                "A sector's end angle must be after its start angle and at most one full turn later."
            }
            Self::BadSoftness => "The edge softness must be zero or larger.",
            Self::BadAxis => "The pattern axis could not be used.",
            Self::SoftFreeform => "A freeform zone needs sharp edges. Set the edge softness to zero.",
            Self::BadFreeform => "A freeform zone must be a closed shape.",
        })
    }
}

/// Why a [`ColorPattern`](super::ColorPattern) cannot become zones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    /// This build cannot interpret the authored pattern format.
    UnsupportedVersion(u32),
    /// More than [`MAX_PATTERN_ZONES`](super::MAX_PATTERN_ZONES) zones besides the base.
    TooManyZones,
    /// A color has no band rows yet (a template's colors are not solved until the application
    /// does it).
    NeedsColor {
        /// The name of the zone that still needs its color.
        zone: String,
    },
    /// A zone (or, with no name, the pattern as a whole) cannot be rendered.
    BadZone {
        /// The name of the zone, `None` for a problem of the whole pattern.
        zone: Option<String>,
        /// What is wrong.
        problem: ZoneProblem,
    },
}

impl PatternError {
    /// The name of the zone the problem is in, when it is in one.
    #[must_use]
    pub fn zone(&self) -> Option<&str> {
        match self {
            Self::UnsupportedVersion(_) | Self::TooManyZones => None,
            Self::NeedsColor { zone } => Some(zone),
            Self::BadZone { zone, .. } => zone.as_deref(),
        }
    }

    /// Maps the zoning validation's error, naming the zone from the pattern.
    pub(super) fn from_zoning(
        error: ZoningError,
        base: &PatternZone,
        zones: &[PatternZone],
    ) -> Self {
        // `None` is the base zone, `Some(i)` the 1-based shaped zone.
        let name = |zone: Option<usize>| {
            zone.map_or_else(
                || Some(base.name.clone()),
                |i| zones.get(i.wrapping_sub(1)).map(|z| z.name.clone()),
            )
        };
        let bad = |zone: Option<usize>, problem: ZoneProblem| Self::BadZone {
            zone: name(zone),
            problem,
        };
        let whole = |problem: ZoneProblem| Self::BadZone {
            zone: None,
            problem,
        };
        match error {
            ZoningError::TooManyZones { .. } => Self::TooManyZones,
            ZoningError::NotFinite { zone, .. } => bad(zone, ZoneProblem::NotFinite),
            ZoningError::NotUnit { zone, .. } => bad(Some(zone), ZoneProblem::NotUnit),
            ZoningError::NotPerMm { zone } => bad(zone, ZoneProblem::BadUnit),
            ZoningError::BadBands { zone } => bad(zone, ZoneProblem::BadBands),
            ZoningError::BadSlab { zone } => bad(Some(zone), ZoneProblem::BadLayer),
            ZoningError::BadRadii { zone } => bad(Some(zone), ZoneProblem::BadRadii),
            ZoningError::BadSideCount { zone, .. } => bad(Some(zone), ZoneProblem::BadSideCount),
            ZoningError::BadAngles { zone } => bad(Some(zone), ZoneProblem::BadAngles),
            ZoningError::BadSoftness => whole(ZoneProblem::BadSoftness),
            ZoningError::BadFrame => whole(ZoneProblem::BadAxis),
            ZoningError::SoftMeshShell => whole(ZoneProblem::SoftFreeform),
            ZoningError::BadMesh { zone, .. } => bad(Some(zone), ZoneProblem::BadFreeform),
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(
                f,
                "Pattern format {version} is not supported by this version."
            ),
            Self::TooManyZones => f.write_str("A pattern holds at most 4 zones besides the base."),
            Self::NeedsColor { .. } => f.write_str("Pick a color for every zone first."),
            Self::BadZone { problem, .. } => fmt::Display::fmt(problem, f),
        }
    }
}

impl std::error::Error for PatternError {}
