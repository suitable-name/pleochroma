//! The native design file: the self-contained `.psdd` file ([`design`], Pleochroma Suite
//! Design Document) that carries a whole design.
//!
//! # Why it exists
//!
//! `.asc` can express only what `GemCAD` (and this crate's [`asc`](crate::asc) module)
//! can: angles, mast distances, index-wheel positions and free-text notes. An editor
//! also needs data that has no place in that format:
//!
//! - A rough blank's own shape and size, independent of any cut facet.
//! - A tier's meet constraint as intent. `.asc`'s `G` field holds only free-text meet
//!   notes, and a constraint that pins a tier's mast to a scale reference has no field.
//!   Reimporting `.asc` re-derives every constraint from geometry and silently drops a
//!   deliberate "meet an existing facet" or "meet a named facet" choice.
//! - Which of a tier's symmetry-generated facets are exempt from group propagation.
//! - A real-world millimeter binding for the design's girdle diameter -- `.asc` is
//!   unitless.
//! - A material preset and specific-gravity/refractive-index override.
//!
//! # Format: TOML
//!
//! TOML over JSON for a file real people open, diff, and hand-edit: it supports
//! comments (`# this crown was copied from RBC-445`), and its `[[tiers]]`
//! array-of-tables keeps a one-tier edit to a one-line diff. Like JSON it is
//! self-describing (field names travel with the value, unlike `postcard`), so an
//! unrecognized key survives inspection rather than being silently misread by
//! position -- see the `unknown_keys_survive_at_every_level` test in [`design`].
//!
//! [`design::DesignFile`], [`PreformTable`], [`MaterialTable`], and [`TierTable`] each
//! carry a `#[serde(flatten)]` `unknown: toml::Table` field: keys a newer build wrote
//! that this build doesn't claim land there and are written back unchanged rather than
//! dropped. `toml::Table` is an ordered map, never a `HashMap`, so re-serializing a
//! value this build only reads reproduces its exact key order -- what keeps a
//! save-without-editing round trip byte-identical (see the determinism test).
//!
//! # The self-contained design file
//!
//! [`design`] holds the single-file `.psdd` format that carries a whole design with no
//! paired `.asc`. [`design::detect_kind`] tells a design file from anything else.
//!
//! # Module layout
//!
//! [`fingerprint`] is the SHA-256 helper; [`schema`] holds the shared tables the design
//! file is made of. Building those tables from -- or applying them back onto -- an
//! actual editor design is `pleochroma-cut-core`'s own `native` module's job, not this
//! crate's: this module only owns the on-disk document shape, never the in-memory editor
//! types it mirrors.

mod custom_material;
pub mod design;
mod fingerprint;
mod schema;
#[cfg(test)]
mod tests;

pub use custom_material::{
    ColorRecipeDto, CustomGemDataTable, CustomMaterialSnapshot, DispersionModelDto,
    GEM_TOUGHNESS_WORDS, RayBandsTable, gem_toughness_rank,
};
pub use fingerprint::sha256_hex;
pub use schema::{
    COLOR_VARIANT_SHAPE_ENERGY, COLOR_VARIANT_SHAPE_WAVELENGTH, COLOR_VARIANT_UNIT_MODEL_UNIT,
    COLOR_VARIANT_UNIT_PER_MM, ColorVariantBandTable, ColorVariantTable, ConcaveTierTable,
    HistoryTable, MaterialTable, MeetPickFacetTable, NativeMeetConstraint, NativePreformShape,
    NativeTierTarget, PreformTable, SourceTable, SurfaceColumns, SurfaceFacetTable, TierTable,
};
