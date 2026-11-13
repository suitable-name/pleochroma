//! Imports `.asc`, `.gem` and `.gcs` files into the local database, optionally including
//! subfolders. The work runs off the UI thread, and each file is panic-isolated.
//!
//! Nothing here touches the network or parses HTML or PDF. It works only on files the user
//! hands the app (`import_path`) and on rows already in the local catalogue. The parsing
//! and reconstruction live in `pleochroma_vault::local`.
//!
//! [`scan`] finds candidate files, [`foreign`] converts `.gem`/`.gcs` designs to `.asc`,
//! [`measure`] derives proportions and shape, [`pipeline`] runs the per-file steps and the
//! batch loop, [`confirm`] handles filename collisions, and [`folder_memory`] remembers the
//! import folder. [`wiring`] connects all of them to the UI.

mod confirm;
mod folder_memory;
mod foreign;
mod measure;
mod pipeline;
mod scan;
#[cfg(test)]
mod tests;
mod wiring;

#[cfg(test)]
pub use foreign::test_gem;
pub use foreign::{ForeignFormat, convert_foreign_design, converted_asc_file_name};
pub use measure::{apply_measured_metadata, merge_reimport_metadata};
pub use pipeline::catch_file_panic;
pub use wiring::setup_import_callback;
