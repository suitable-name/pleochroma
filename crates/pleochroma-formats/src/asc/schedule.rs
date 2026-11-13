//! [`AscSchedule`]: a fully parsed `GemCAD` `.asc` file's cutting instructions.

use super::tier::AscTier;
use std::fmt;

/// The line terminator an `.asc` file uses, recorded by [`super::parse_asc`] and
/// reproduced by [`super::to_asc_string`].
///
/// `GemCAD` itself writes CRLF, and Gem Cut Studio 1.1 reportedly misreads the last facet
/// name of an LF-only file, so a schedule built from scratch (the [`Default`])
/// writes CRLF. A parsed file keeps its own ending, since most catalogue files are LF and
/// re-exporting them must not rewrite every line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AscLineEnding {
    /// A bare line feed.
    Lf,
    /// Carriage return plus line feed, as `GemCAD` writes it.
    #[default]
    CrLf,
}

impl AscLineEnding {
    /// The terminator text itself: `"\n"` or `"\r\n"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    /// What `text` uses: [`Self::CrLf`] as soon as it contains one `"\r\n"` pair,
    /// [`Self::Lf`] otherwise (including a CR-only file after
    /// [`super::decode_asc_bytes`] has normalised it, and a one-line file with no
    /// terminator at all).
    #[must_use]
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::CrLf
        } else {
            Self::Lf
        }
    }
}

/// A fully parsed `GemCAD` `.asc` file's cutting instructions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscSchedule {
    /// The `GemCad` version string from the file's first line (e.g. "5.0", "4.51").
    pub gemcad_version: String,
    /// Index-wheel tooth count, as encoded in the file's `g` line. `GemCAD` sometimes
    /// encodes this as a negative number (an internal handedness/direction
    /// convention); use [`Self::gear_teeth_abs`] for the actual tooth count.
    pub gear_teeth: i32,
    /// The `g` line's second field: the index wheel's reference angle.
    pub gear_reference_angle: f64,
    /// Rotational symmetry order (the `y` line's first field).
    pub symmetry_order: u32,
    /// Mirror-symmetry flag (the `y` line's second field, `y`/`n`).
    pub mirror: bool,
    /// Refractive index (the `I` line).
    pub refractive_index: f64,
    /// Every `H` (header/title) line, in file order, with the leading `H` stripped.
    pub headers: Vec<String>,
    /// Every `F` (footnote) line, in file order, with the leading `F` stripped.
    pub footnotes: Vec<String>,
    /// Every facet tier, in file order.
    pub tiers: Vec<AscTier>,
    /// Lenient-parse diagnostics collected by [`super::parse_asc`] instead of failing:
    /// free text after the last facet tier, an unrecognised line, a stray token that is
    /// neither a number nor a marker, a keyword glued to its value (`g96`), a negative
    /// mast on a nonzero-angle tier, and a duplicate `g` line. Empty for a clean file.
    /// These describe the source file and are never written back by
    /// [`super::to_asc_string`]; a hand-built schedule leaves this empty.
    pub warnings: Vec<String>,
    /// The line terminator the source file used (see [`AscLineEnding`]);
    /// [`super::to_asc_string`] writes every line with it. [`Default`] is
    /// [`AscLineEnding::CrLf`], `GemCAD`'s own convention, for a schedule built by
    /// hand.
    pub line_ending: AscLineEnding,
}

impl AscSchedule {
    /// The index wheel's tooth count as an unsigned magnitude, for azimuth
    /// computations (`phi = 2*pi*index/gear_teeth_abs()`).
    #[must_use]
    pub const fn gear_teeth_abs(&self) -> u32 {
        self.gear_teeth.unsigned_abs()
    }

    /// Total number of individual facet planes this schedule describes (the sum of
    /// each tier's index count, or 1 for a tier with no explicit index).
    #[must_use]
    pub fn facet_plane_count(&self) -> usize {
        self.tiers.iter().map(|t| t.indices.len().max(1)).sum()
    }
}

impl fmt::Display for AscSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AscSchedule(gear={}, order={}, mirror={}, RI={}, tiers={})",
            self.gear_teeth,
            self.symmetry_order,
            self.mirror,
            self.refractive_index,
            self.tiers.len()
        )
    }
}
