//! Reader and writer for `GemCAD`-style `.asc` cutting-instructions files.
//!
//! `GemCAD` is Robert Strickland's faceting-design software; this module is not
//! produced, endorsed, or affiliated with `GemCAD` or its author (see the crate README
//! for the affiliation note). The `.asc` text format is a de facto standard in the
//! faceting community, so this module reads and writes it without any renderer,
//! database or GUI dependency.
//!
//! The `a` records are the only place a schedule's "mast" distance (how far a facet
//! plane sits from the stone's center) is stored.
//!
//! # Format
//!
//! ```text
//! GemCad 5.0
//! g 96 0.0                                       <- gear teeth, reference angle
//! y 6 y                                           <- symmetry order, mirror flag (y/n)
//! I 1.72                                          <- refractive index
//! H PC 45.149  Round Trichecker-12                <- header/title lines (repeatable)
//! H by Fred W. Van Sant, X 51, Extra Designs 2000
//! a -41.000000 0.64991234 92 n 1 84 76 68 60 ...  <- tier: angle, mast, indices/name
//! F "For small stones"                            <- footnote (repeatable)
//! ```
//!
//! Each `a` record is: a signed `angle` (degrees from the girdle plane; `GemCAD`'s
//! convention is negative = pavilion, positive = crown), the `mast` distance (the
//! field this crate exists to extract reliably), then a mix of index-wheel positions
//! (usually integers, occasionally fractional) and `n <name>` markers, and finally an
//! optional `G <notes...>` free-text tail. A record can list indices in more than one
//! `n <name>` group (e.g. `92 n c ... 94 n d ...`). They are folded into one
//! [`AscTier`]'s `indices`, since they share an angle and mast; only the azimuth differs.
//!
//! **Table and culet.** A zero angle is a flat facet. The file states the culet as
//! angle `0` with a NEGATIVE distance (`a 0.00 -0.368 0`; the manual: "positive
//! unless the facet is a culet (0° pavilion) facet") or as a `-0.000000` angle
//! token; any other zero angle is the table. [`parse_asc`] stores a culet as a
//! sign-negative zero [`AscTier::angle_deg`] with a positive `mast`, so the side is
//! decided by the angle's sign alone, never by file order (`GemCAD`'s own
//! pavilion-first and table-first sort orders both occur). [`to_asc_string`] writes
//! a culet back as `a -0 -<mast> <indices>`, adding the gear tooth count as its
//! index when it has none.
//!
//! **Names.** A name after `n` belongs to the index written just before it, and
//! `GemCAD`'s diagram labels that facet. [`AscTier::name`] folds a tier's names into
//! one label (`c/d`); [`AscTier::index_names`] keeps each name's position, and the
//! writer puts every name back there (or after the first index for a tier built by
//! hand).
//!
//! **Encoding and line endings.** `GemCAD` for Windows writes Windows-1252 (a legacy
//! `°` is byte `0xB0`); [`decode_asc_bytes`] strips a UTF-8 byte-order mark, reads
//! valid UTF-8 as is and anything else as Windows-1252, and turns a lone CR into LF.
//! Read files through it (or [`parse_asc_bytes`]), never `read_to_string`. The
//! parser accepts LF and CRLF and records which one the file used in
//! [`AscSchedule::line_ending`]; the writer reproduces it (a schedule built by hand
//! defaults to CRLF, as `GemCAD` writes).
//!
//! Long index lists wrap onto continuation lines that do not start with `a`: they start
//! with a bare number, `n` or `G`. [`parse_asc`] treats any line that does not start with
//! a known record keyword (`GemCad`, `g`, `y`, `I`, `H`, `F`, `a`) as a continuation of
//! the open `a` record.
//!
//! [`parse_asc`] is also lenient about: continuation lines with no `a` prefix, facet
//! names that are plain numbers (told apart from indices only by position right after an
//! `n` marker), fractional index positions, negative gear-teeth counts (a handedness
//! convention), a bare `g` line without its keyword, and keywords glued to their values
//! (`g96 0.0`).
//!
//! # Writing schedules
//!
//! [`to_asc_string`] serializes an [`AscSchedule`] back to `.asc` text. Its output is not
//! byte-identical to `GemCAD`'s: numbers are normalized, wrapped tiers become one line,
//! and a culet is always written as `a -0 -<mast>`. It round-trips semantically, so
//! parsing the output reproduces an equal [`AscSchedule`] (see `asc/tests/round_trip.rs`).
//!
//! Writing is fallible. [`to_asc_string`] returns [`AscWriteError`] for a header,
//! footnote or tier notes string containing a newline or carriage return, since that
//! would split the field across lines, and for a non-finite number
//! ([`AscWriteError::NonFiniteValue`]).
//!
//! A tier name is never rejected. One with whitespace would split into extra tokens on
//! re-parse, so [`asc_safe_tier_name`] sanitises it (`"Crown Main"` is written as
//! `Crown_Main`). The native `.psdd` design file keeps the true name. A name that
//! sanitises to nothing is written as an automatic label (block letter plus position:
//! `C1`, `P3`, ...). See [`is_asc_safe_tier_name`].
//!
//! Reading is bounded the same way: [`parse_asc`] rejects a gear with more than
//! [`AscParseError::MAX_GEAR_TEETH`] (720) teeth with
//! [`AscParseError::GearTeethTooLarge`], so no per-tooth loop downstream can be made
//! to run for billions of steps by a hostile file.

mod decode;
mod error;
mod meet_instruction;
mod parse;
mod schedule;
mod tier;
mod write;

#[cfg(test)]
mod tests;

pub use decode::{decode_asc_bytes, parse_asc_bytes};
pub use error::AscParseError;
pub use meet_instruction::MeetInstruction;
pub use parse::{parse_asc, parse_asc_with_gear_line};
pub use schedule::{AscLineEnding, AscSchedule};
pub use tier::AscTier;
pub use write::{
    AscWriteError, asc_safe_tier_name, is_asc_safe_tier_name, mark_reconstructed, to_asc_string,
};
