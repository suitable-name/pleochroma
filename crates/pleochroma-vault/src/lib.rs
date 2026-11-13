//! `pleochroma-vault`: plain data models, a SQLite-backed store, and local import/export
//! for the user's own faceting-design library.
//!
//! The storage layer and nothing more: no network client, no HTML/SVG parsing, no OCR.
//! The dependency direction is strictly one way -- anything that acquires designs
//! depends on this crate's [`model`] and `db`, never the reverse.
//!
//! The default `sqlite` feature adds `db` (and with it `rusqlite`); without it the crate
//! is pure Rust (`model` and `local`) and builds for `wasm32-unknown-unknown`.
//!
//! - `db` (feature `sqlite`): SQLite storage and schema migrations. The schema is wider
//!   than this crate fills: columns such as `page_url` and `pdf_file` are plain columns,
//!   and an existing `facet_diagrams.sqlite` keeps opening with every value it holds.
//! - [`model`]: `FacetingDiagramDetail`, `AngleSetting`, `AttachedFile`, search/range
//!   filters.
//! - [`local`]: import/export of the user's own design files (`.asc`, `.psdd`, and converted
//!   `.gem`/`.gcs`), independent of any online source.

/// SQLite storage and schema migrations -- see [`db::sqlite::Database`].
///
/// Only with the default `sqlite` feature; a build without it keeps [`model`] and
/// [`local`], which are pure Rust.
#[cfg(feature = "sqlite")]
pub mod db;
/// Import/export of the user's own design files, independent of any online source.
///
/// See [`local::import_asc`]/[`local::reconstruct_asc_schedule`].
pub mod local;
/// Plain data models: `FacetingDiagramDetail`, `AngleSetting`, `AttachedFile`, search and
/// range filters.
pub mod model;
