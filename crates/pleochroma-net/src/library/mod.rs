//! The read-only design-library sync protocol.
//!
//! A client (the viewer, or a mobile client without a renderer) mirrors designs from a
//! `pleochroma-worker`'s catalogue.
//!
//! [`LibraryRequest`]/[`LibraryResponse`] are their own message family, tagged into the
//! same [`crate::messages::ClientMessage`] envelope the render protocol uses
//! (`ClientMessage::Library`). This module never depends on `pleochroma` and is always
//! compiled in regardless of this crate's `render` feature, so a mobile client can speak
//! it with no renderer.
//!
//! # Pull-mirror, read-only
//!
//! The server is authoritative; a client mirrors from it. There is no `Put`/`Delete`/merge
//! request, and `pleochroma-worker` never writes to its catalogue.
//!
//! [`LibraryRequest::Search`] is a single round trip capped at the vault's
//! `SEARCH_RESULT_CAP` (1000 rows) -- fine for an interactive search box, not for listing a
//! catalogue with more matching rows than that. [`LibraryRequest::SearchPage`]/
//! [`LibraryResponse::SearchResultsPage`] is the keyset-cursor form a full-catalogue walk
//! (a mirror sync) uses instead: same filters plus a cursor, walked page by page until a
//! short page signals the end.
//!
//! New operations are added as variants appended at the tail of each enum, so existing
//! postcard indices stay put. [`DesignSummary::version`] and [`DesignRecord::version`]
//! serve read-side staleness detection.
//!
//! # Staleness: a content hash and a revision token
//!
//! [`DesignSummary::version`] is a SHA-256 content hash over the fields the summary
//! carries (excluding the hash field itself and `entry_id`, which is a per-database row
//! id and differs between databases holding the same design). `pleochroma-worker`
//! computes it at serve time; this crate only carries the resulting 32 bytes.
//!
//! [`DesignRecord::version`] and [`DesignSummary::design_version`] are not content
//! hashes: they are an O(1) revision token derived from the design's `url` and its
//! `diagram_entries.updated_at`, so the summary can name the full record's revision
//! without reading the detail rows or image bytes. Equal tokens mean the same stored
//! revision.
//!
//! [`DesignSummary::version`] is the cheap first filter (up to 1000 rows per search);
//! compare [`DesignSummary::design_version`] against the mirrored token to decide whether
//! a `FetchDesign` is needed, and [`DesignRecord::version`] is the same token on the
//! fetched record.
//!
//! # Attachments: fetched separately, one at a time
//!
//! [`DesignRecord`] carries attachment metadata only ([`AttachedFileMeta`]), never
//! content: attachments can hold multi-megabyte PDFs and several designs often share the
//! same one, so inlining content into every `FetchDesign` reply would re-send those
//! bytes per referencing design. A client fetches an attachment's bytes lazily, once per
//! id, via [`LibraryRequest::FetchAttachment`].
//!
//! Per-request memory is bounded to one attachment's bytes at a time, not a whole
//! design's attachment set or the search result set.
//!
//! # Loading a remote design into the local editor
//!
//! [`LibraryRequest::FetchDesignSource`]/[`LibraryResponse::DesignSource`] is a
//! narrower, purpose-built sibling of the general `FetchAttachment` path above: it
//! names the design by `entry_id` rather than an attachment id, and it specifically
//! finds and returns that design's own `.asc` cutting-instructions attachment's TEXT (not
//! an arbitrary attachment's raw bytes), decoded as UTF-8. This is what lets
//! `apps/pleochroma-cut`'s cutting-design editor load a remote design exactly the way
//! it already loads a local one -- see [`LibraryResponse::DesignSourceNotAvailable`]
//! for what a design with no `.asc` attachment gets instead.
//!
//! Split across submodules by topic:
//!
//! - [`filter`]: wire counterparts of `pleochroma_vault`'s search/filter types.
//! - [`record`]: the design data types a [`LibraryResponse`] carries.
//! - [`protocol`]: the request/response envelope, [`LibraryRequest`]/[`LibraryResponse`].
//!
//! Every item is re-exported here at its original flat `library::` path.

mod filter;
mod protocol;
mod record;
#[cfg(test)]
mod tests;

// Lets `tests` (via `use super::*;`) name `ErrorMsg` without its own import.
#[cfg(test)]
use crate::messages::ErrorMsg;

pub use filter::{
    AttributeRangesWire, PerformanceAggregateWire, PerformanceBoundWire, PerformanceFilterWire,
    PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
pub use protocol::{LibraryRequest, LibraryResponse};
pub use record::{
    AngleSettingWire, AttachedFileMeta, DesignRecord, DesignSummary, FrostedTierWire,
};
