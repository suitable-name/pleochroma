//! The Material Reference view model, shared by the desktop, the web app, the Android app and the
//! CLI.
//!
//! It holds the table of every material (rows, sort, filter, copy text), the gem facts of one
//! material as display text, the details page of one material, and the wording they share.
//!
//! - [`rows`]: [`ReferenceRow`], [`reference_rows`], [`sort_rows`], [`filter_rows`],
//!   [`filter_counts`] and [`table_text`].
//! - [`facts`]: [`GemFacts`] for a built-in and for a custom material, and [`CustomGemData`].
//! - [`text`]: [`family_line`], [`dispersion_chip_text`] and the window's phrases.
//! - [`detail`] and [`curves`]: [`MaterialDetail`], [`material_detail`] and the chart paths.
//!
//! No GUI toolkit types: every chart is a path string in a 0..100 viewbox that a front end only
//! draws.

pub mod curves;
pub mod detail;
pub mod facts;
pub mod rows;
pub mod text;

#[cfg(test)]
mod tests;

pub use curves::index_curve_path;
pub use detail::{DetailContext, MaterialDetail, material_detail, material_detail_for_variant};
pub use facts::{CustomGemData, GemFacts};
pub use rows::{
    Column, DispersionScale, Filter, Origin, ReferenceRow, SortKeys, filter_counts, filter_rows,
    reference_rows, sort_rows, table_text,
};
pub use text::{dispersion_chip_text, family_line};
