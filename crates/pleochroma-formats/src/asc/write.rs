//! [`to_asc_string`]: serializes an [`AscSchedule`] back to `.asc` text, and
//! [`mark_reconstructed`]: marks a schedule as derived rather than authored.

use super::{schedule::AscSchedule, tier::AscTier};
use std::{borrow::Cow, fmt};

/// Everything that can go wrong serializing an [`AscSchedule`] to `.asc` text with
/// [`to_asc_string`].
///
/// A tier's name is never rejected here: an unsafe one is sanitised by
/// [`asc_safe_tier_name`] instead. Every variant is a value that would not survive the
/// round trip: a line break would split a single-line field, and a `NaN` or infinite
/// number is written as text that [`super::parse_asc`] rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AscWriteError {
    /// An `H` (header) line's text contains a newline or carriage return, which would
    /// split the header across physical lines.
    HeaderContainsNewline {
        /// 0-based position of the offending header in [`AscSchedule::headers`].
        header_index: usize,
        /// The offending header text.
        text: String,
    },
    /// An `F` (footnote) line's text contains a newline or carriage return; see
    /// [`Self::HeaderContainsNewline`].
    FootnoteContainsNewline {
        /// 0-based position of the offending footnote in [`AscSchedule::footnotes`].
        footnote_index: usize,
        /// The offending footnote text.
        text: String,
    },
    /// A tier's `G`-field notes contain a newline or carriage return, which would end
    /// the tier's `a` record line early and truncate the note.
    NotesContainNewline {
        /// 0-based position of the offending tier in [`AscSchedule::tiers`].
        tier_index: usize,
        /// The offending notes text.
        text: String,
    },
    /// A number is `NaN` or infinite, which [`super::parse_asc`] rejects.
    NonFiniteValue {
        /// 0-based position of the offending tier in [`AscSchedule::tiers`], or
        /// `None` for a header-level number.
        tier_index: Option<usize>,
        /// Which number: `"angle"`, `"mast"`, `"index"`, `"gear reference angle"` or
        /// `"refractive index"`.
        field: &'static str,
        /// The value as `Display` writes it (`NaN`, `inf` or `-inf`).
        value: String,
    },
}

impl fmt::Display for AscWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderContainsNewline { header_index, text } => write!(
                f,
                "header #{header_index} {text:?} contains a line break, which would split it \
                 across physical lines"
            ),
            Self::FootnoteContainsNewline {
                footnote_index,
                text,
            } => write!(
                f,
                "footnote #{footnote_index} {text:?} contains a line break, which would split \
                 it across physical lines"
            ),
            Self::NotesContainNewline { tier_index, text } => write!(
                f,
                "tier #{tier_index}: notes {text:?} contain a line break, which would truncate \
                 them on re-parse"
            ),
            Self::NonFiniteValue {
                tier_index: Some(tier_index),
                field,
                value,
            } => write!(
                f,
                "tier #{tier_index}: {field} is {value}, which cannot be read back"
            ),
            Self::NonFiniteValue {
                tier_index: None,
                field,
                value,
            } => write!(f, "{field} is {value}, which cannot be read back"),
        }
    }
}

impl std::error::Error for AscWriteError {}

/// `true` iff `name` would survive a plain `.asc` write/re-parse round trip as this
/// exact string -- equivalently, iff [`asc_safe_tier_name`] returns `name` unchanged
/// (borrowed, allocating nothing).
///
/// [`super::parse_asc`] splits every record on ASCII whitespace, so a name with an
/// embedded space, tab, or newline would read back as several tokens. The extra
/// tokens can be misread as `n`/`G` markers and corrupt the rest of the record.
/// An empty name is safe, and names joined with `/` are still one token.
///
/// [`to_asc_string`] sanitises an unsafe name via [`asc_safe_tier_name`] rather than
/// rejecting it; this predicate checks, without allocating, whether that happens.
#[must_use]
pub fn is_asc_safe_tier_name(name: &str) -> bool {
    !name.chars().any(char::is_whitespace)
}

/// Sanitises `name` into a value [`to_asc_string`] can safely write as a tier's
/// single ` n <name>` token.
///
/// Every run of Unicode whitespace (spaces, tabs, newlines, ...) becomes one `_`,
/// and leading/trailing whitespace is trimmed away entirely rather than becoming a
/// leading/trailing `_`. `"Crown Main"` becomes `"Crown_Main"`; `"  Star  "` becomes
/// `"Star"`; `""` stays `""`. Returns `name` itself, borrowed, whenever it is
/// already [`is_asc_safe_tier_name`] -- the common case allocates nothing.
///
/// The true name (e.g. `"Crown Main"`) is kept by the native design file; only a plain
/// `.asc` export carries this single-token form.
#[must_use]
pub fn asc_safe_tier_name(name: &str) -> Cow<'_, str> {
    if is_asc_safe_tier_name(name) {
        return Cow::Borrowed(name);
    }
    let mut out = String::with_capacity(name.len());
    let mut last_was_space = true; // leading whitespace is trimmed, not turned into `_`
    for ch in name.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push('_');
            }
            last_was_space = true;
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    if out.ends_with('_') {
        out.pop();
    }
    Cow::Owned(out)
}

/// Serializes an [`AscSchedule`] back to `.asc` text.
///
/// Not byte-identical to hand-authored `GemCAD` output (whitespace and numbers are
/// normalized, a wrapped tier is written on one line, and a culet is always written as
/// `a -0 -<mast> <indices>`, see [`AscTier::angle_deg`]), but it round-trips
/// semantically. The one exception: a culet with no index comes back with the gear
/// tooth count as its index, which is the same azimuth-independent plane.
///
/// Names are written at the positions in [`AscTier::index_names`]. Every line ends with
/// [`AscSchedule::line_ending`]. Numbers use `Display`, which emits the shortest decimal
/// that reads back to the same `f64`, so no precision is lost.
///
/// A name is sanitised via [`asc_safe_tier_name`] before it becomes an `n` token. A
/// name that sanitises to nothing would leave the marker bare, so it is written as an
/// automatic label: the block letter (`C` crown, `G` girdle, `P` pavilion or culet)
/// and the tier's 1-based ordinal within that block, e.g. `C3`. A table is `Table`.
///
/// # Errors
///
/// [`AscWriteError`] when a header, footnote, or tier notes string contains a line
/// break, or when a tier angle, mast or index, the gear reference angle, or the
/// refractive index is `NaN` or infinite.
pub fn to_asc_string(schedule: &AscSchedule) -> Result<String, AscWriteError> {
    use fmt::Write as _;

    validate_writable(schedule)?;

    let eol = schedule.line_ending.as_str();
    let mut out = String::new();

    // The parser keeps only the text after the `GemCad` keyword; strip a keyword the
    // version string still carries, or the header would read "GemCad GemCad 5.0".
    let version = schedule.gemcad_version.trim();
    let version = ["GemCad ", "GemCAD ", "Gemcad "]
        .iter()
        .find_map(|keyword| version.strip_prefix(keyword))
        .unwrap_or(version);
    let _ = write!(out, "GemCad {version}{eol}");
    let _ = write!(
        out,
        "g {} {}{eol}",
        schedule.gear_teeth, schedule.gear_reference_angle
    );
    let _ = write!(
        out,
        "y {} {}{eol}",
        schedule.symmetry_order,
        if schedule.mirror { "y" } else { "n" }
    );
    let _ = write!(out, "I {}{eol}", schedule.refractive_index);
    for header in &schedule.headers {
        let _ = write!(out, "H {header}{eol}");
    }

    let mut g_count = 0usize;
    let mut c_count = 0usize;
    let mut p_count = 0usize;
    for tier in &schedule.tiers {
        let block_ordinal = if is_girdle(tier) {
            g_count += 1;
            g_count
        } else if tier.angle_deg > 0.0 {
            c_count += 1;
            c_count
        } else if tier.angle_deg < 0.0 || tier.is_culet() {
            p_count += 1;
            p_count
        } else {
            1
        };
        write_tier(&mut out, tier, block_ordinal, schedule.gear_teeth_abs());
        out.push_str(eol);
    }

    for footnote in &schedule.footnotes {
        let _ = write!(out, "F {footnote}{eol}");
    }

    Ok(out)
}

/// `true` when `text` holds a newline or a carriage return. The reader turns either
/// into a line break, so neither may sit inside a single-line field.
fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

/// An [`AscWriteError::NonFiniteValue`] unless `value` is finite.
fn require_finite(
    tier_index: Option<usize>,
    field: &'static str,
    value: f64,
) -> Result<(), AscWriteError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(AscWriteError::NonFiniteValue {
            tier_index,
            field,
            value: value.to_string(),
        })
    }
}

/// Checks everything [`to_asc_string`] cannot write faithfully: a line break in a
/// header, footnote or tier note, and a `NaN` or infinite number anywhere.
fn validate_writable(schedule: &AscSchedule) -> Result<(), AscWriteError> {
    for (header_index, text) in schedule.headers.iter().enumerate() {
        if has_line_break(text) {
            return Err(AscWriteError::HeaderContainsNewline {
                header_index,
                text: text.clone(),
            });
        }
    }
    for (footnote_index, text) in schedule.footnotes.iter().enumerate() {
        if has_line_break(text) {
            return Err(AscWriteError::FootnoteContainsNewline {
                footnote_index,
                text: text.clone(),
            });
        }
    }
    require_finite(None, "gear reference angle", schedule.gear_reference_angle)?;
    require_finite(None, "refractive index", schedule.refractive_index)?;
    for (tier_index, tier) in schedule.tiers.iter().enumerate() {
        if has_line_break(&tier.notes) {
            return Err(AscWriteError::NotesContainNewline {
                tier_index,
                text: tier.notes.clone(),
            });
        }
        let at = Some(tier_index);
        require_finite(at, "angle", tier.angle_deg)?;
        require_finite(at, "mast", tier.mast)?;
        for &index in &tier.indices {
            require_finite(at, "index", index)?;
        }
    }
    Ok(())
}

/// Whether `tier` is a girdle tier (`|angle| = 90` exactly, as the format writes it).
#[expect(
    clippy::float_cmp,
    reason = "the format marks a girdle by the literal angle 90; the angle is parsed or stored exactly"
)]
fn is_girdle(tier: &AscTier) -> bool {
    tier.angle_deg.abs() == 90.0
}

/// The label written for a facet name that sanitises to nothing: the tier's block
/// letter and `block_ordinal`, or `Table` for the table.
fn auto_tier_name(tier: &AscTier, block_ordinal: usize) -> String {
    let letter = if is_girdle(tier) {
        'G'
    } else if tier.angle_deg > 0.0 {
        'C'
    } else if tier.angle_deg < 0.0 || tier.is_culet() {
        'P'
    } else {
        'T'
    };
    if letter == 'T' {
        "Table".to_string()
    } else {
        format!("{letter}{block_ordinal}")
    }
}

/// Writes one `a` record (without its line terminator).
///
/// A culet ([`AscTier::is_culet`]) is written as `a -0 -<mast> <indices>`, the form
/// `GemCAD` documents. It always has at least one index: when the tier has none, the
/// gear tooth count is used, since a culet is azimuth-independent and readers need an
/// index to create a facet.
///
/// Names go where [`AscTier::index_names`] recorded them. A tier with a name but no
/// recorded positions puts it after the first index. A name that sanitises to nothing
/// is written as [`auto_tier_name`] of `ordinal`, so the `n` marker is never bare.
fn write_tier(out: &mut String, tier: &AscTier, ordinal: usize, gear_teeth_abs: u32) {
    use fmt::Write as _;

    let culet = tier.is_culet();
    if culet {
        let _ = write!(out, "a -0 {}", -tier.mast.abs());
    } else {
        let _ = write!(out, "a {} {}", tier.angle_deg, tier.mast);
    }
    let gear_index = [f64::from(gear_teeth_abs)];
    let indices: &[f64] = if culet && tier.indices.is_empty() {
        &gear_index
    } else {
        &tier.indices
    };
    let folded_name;
    let names: &[(usize, String)] = if tier.index_names.is_empty() && !tier.name.is_empty() {
        folded_name = [(0, tier.name.clone())];
        &folded_name
    } else {
        &tier.index_names
    };
    let write_name = |out: &mut String, name: &str| {
        let safe = asc_safe_tier_name(name);
        if safe.is_empty() {
            let _ = write!(out, " n {}", auto_tier_name(tier, ordinal));
        } else {
            let _ = write!(out, " n {safe}");
        }
    };
    if indices.is_empty() {
        for (_, name) in names {
            write_name(out, name);
        }
    } else {
        let last = indices.len() - 1;
        for (position, idx) in indices.iter().enumerate() {
            let _ = write!(out, " {idx}");
            // A position past the end (indices removed after recording) stays on the
            // last index rather than being dropped.
            for (_, name) in names
                .iter()
                .filter(|(at, _)| *at == position || (position == last && *at > last))
            {
                write_name(out, name);
            }
        }
    }
    if !tier.notes.is_empty() {
        let _ = write!(out, " G {}", tier.notes);
    }
}

/// Prepends a `RECONSTRUCTED` marker to `schedule`'s headers, so a schedule whose masts were solved
/// rather than measured is not mistaken for authored cutting instructions.
///
/// Does nothing if the first header already is the marker.
///
/// `note` says how the schedule was derived; it follows the marker on the same line.
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str) {
    let already_marked = schedule
        .headers
        .first()
        .is_some_and(|h| h.starts_with("RECONSTRUCTED"));
    if already_marked {
        return;
    }
    schedule.headers.insert(
        0,
        format!("RECONSTRUCTED -- mast distances are solved, not original -- {note}"),
    );
}
