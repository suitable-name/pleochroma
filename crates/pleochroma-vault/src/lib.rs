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
//! - `db` (feature `sqlite`): SQLite storage and schema migrations. The schema is deliberately wider
//!   than this crate itself fills -- columns like `page_url`/`pdf_file` are just
//!   columns, and an existing `facet_diagrams.sqlite`, however it was populated, keeps
//!   opening and keeps every value it already holds.
//! - [`model`]: `FacetingDiagramDetail`, `AngleSetting`, `AttachedFile`, search/range
//!   filters.
//! - [`local`]: import/export for the user's own `.asc` files (via `pleochroma_formats::asc`),
//!   independent of any online source.

/// SQLite storage and schema migrations -- see [`db::sqlite::Database`].
///
/// Only with the default `sqlite` feature; a build without it keeps [`model`] and
/// [`local`], which are pure Rust.
#[cfg(feature = "sqlite")]
pub mod db;
/// Import/export for the user's own `.asc` files, independent of any online source.
///
/// See [`local::import_asc`]/[`local::reconstruct_asc_schedule`].
pub mod local;
/// Plain data models: `FacetingDiagramDetail`, `AngleSetting`, `AttachedFile`, search/
/// range filters.
pub mod model;
