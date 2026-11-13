//! Readers and writers for gemstone faceting design file formats.
//!
//! `pleochroma-formats` is an independent implementation of file formats originating
//! with `GemCAD` (Robert Strickland's faceting-design software) and Gem Cut Studio. Neither
//! produces or endorses it. Each format lives in its own module, so reading or writing a
//! design never pulls in a renderer, database or GUI toolkit:
//!
//! - [`asc`]: `GemCAD`'s `.asc` cutting-instructions text format. Read and write, verified
//!   against a corpus of 5,759 real files.
//! - [`native`]: the `.psdd` design file of `apps/pleochroma-cut`: the on-disk schema,
//!   TOML encode/decode and path rules. It also holds state `.asc` has no field for.
//!   Conversion to and from the in-memory editor design lives in `pleochroma-cut-core`.
//! - [`gcs`]: Gem Cut Studio's `.gcs` XML design format, per the Gem Cut Studio User's
//!   Manual v1.1.0 (pp. 58-61). Read (all 59 corpus files), conversion to `.asc` cutting
//!   instructions, and an experimental writer.
//! - [`gem`]: `GemCAD`'s native `.gem` binary save format. A structural decoder that frames
//!   all 254 corpus files byte-exactly, and conversion to `.asc` cutting instructions that
//!   reproduces exact `.asc` exports to 6 decimals.
//!
//! [`asc`], [`gcs`] and [`gem`] use no external crates; [`native`] uses `serde` and
//! `toml` for its encoding and `sha2` for attachment hashes. Code shared by more than one
//! format belongs at this crate root: the Windows-1252 decoding used by the `.asc`, `.gem`
//! and `.gcs` readers lives in the private `encoding` module.

/// `GemCAD`'s `.asc` cutting-instructions text format.
pub mod asc;
mod encoding;
/// Gem Cut Studio's `.gcs` XML design format.
pub mod gcs;
/// `GemCAD`'s native `.gem` binary save format.
pub mod gem;
/// The `.psdd` design file format of `apps/pleochroma-cut`.
pub mod native;
