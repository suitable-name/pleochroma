//! Pull-mirror sync: copies a remote `pleochroma-worker`'s design library into the
//! local `pleochroma_vault` database, so it is available offline.
//!
//! This is machine-to-machine mirroring of the user's own library over the mutual-TLS
//! connection that `bridge::remote::enroll` set up. It fetches only from that worker: no public
//! URLs, no HTML parsing, no OCR.
//!
//! # The safety rule, and it is the whole point of this module
//!
//! **This sync is additive and update-only. It never deletes a local design, attachment or
//! custom material, and never overwrites a row it did not put there.**
//!
//! A design's identity is its `diagram_entries.url` (`UNIQUE`). A worker can serve its own
//! `local://` imports under the same synthetic URL scheme, so a remote design can name a URL that
//! a local row already owns. `sync::design::sync_one_design` checks for this before writing: a
//! local row for the `url` with no `library_mirror_state` entry is left untouched and counted in
//! [`options::MirrorCounts::local_conflicts_skipped`]. Nothing deletes local rows the sync did
//! not see: the sync pulls what the remote has and never makes local match remote.
//!
//! # Local delete wins
//!
//! Deleting a design locally keeps its `library_mirror_state` row (there is no
//! `ON DELETE CASCADE` to it, deliberately). `Database::delete_diagram_entry` marks that row
//! `deleted_locally`, a tombstone, in the same transaction as the delete. `sync::run_mirror_sync`
//! skips a tombstoned url before any fetch, whatever the remote versions say. Comparing versions
//! alone would not do: a changed remote design would be re-created. So local delete wins over
//! remote mirror, by design. [`options::MirrorCounts::skipped_deleted`] counts the tombstoned
//! designs a pass meets, and [`options::MirrorCounts::orphaned_mirror_states`] (from
//! `Database::count_mirror_states_without_entry`) counts the state rows that have no local design
//! left. A catalogue that predates the tombstone column has its already-orphaned rows tombstoned
//! by the migration.
//!
//! # Identity and staleness
//!
//! A remote design's `url` is the key `diagram_entries.url` is `UNIQUE` on, so no new identity
//! scheme is needed. `pleochroma_vault::model::mirror::MirrorState` records, per `url`, the last
//! summary hash and revision token seen. A sync lists the whole remote catalogue first, then skips
//! a design entirely (no `FetchDesign`, no attachment fetch, no local write) when its versions
//! have not moved. A no-op second sync therefore costs one `SearchPage` request per page plus one
//! `library_mirror_state` read per known design, and no `FetchDesign` or `FetchAttachment`.
//!
//! The summary hash covers only the search-result fields, so an edit to the angle table, notes,
//! attachments or ratios leaves it unchanged. The second tier is the revision token
//! (`DesignSummary::design_version`), which equals the `version` of the fetched record. It is a
//! version token, not a content hash: any remote edit moves it, and a summary that states none
//! (all zero bytes) counts as moved. The summary hash leaves the remote `entry_id` out on purpose,
//! so renumbering the remote database does not force a full re-fetch.
//!
//! # Pagination
//!
//! `sync::run_mirror_sync` walks a keyset cursor (`LibraryRequest::SearchPage`,
//! `id > cursor`) to exhaustion before syncing begins, rather than `OFFSET`-based
//! paging -- a keyset walk can't skip or duplicate a row when another design is
//! inserted server-side mid-walk.
//!
//! # Attachments: fetched eagerly, up to a size cap
//!
//! A mirror exists for offline use, so every attachment of a design is fetched immediately,
//! except one whose advertised size (`AttachedFileMeta::size`) exceeds
//! [`options::MirrorOptions::max_attachment_bytes`]. That cap is a safety net against one
//! anomalous file. A skipped attachment does not fail the design: that file is not stored.
//!
//! # Cancellation leaves the database consistent
//!
//! `cancel` is checked once per design, strictly before that design's network calls or
//! local write begin, never mid-design. Once a design starts, it always runs to
//! completion (attachments fetched first, then one transactional `save_design` write
//! of entry and detail together) before the next check.
//! **Invariant:** at any interruption point, the local database reflects some prefix of
//! fully-completed per-design syncs plus whatever was there before -- no design is ever
//! left mid-write. Enumeration itself is not cancellable mid-walk (it does no local
//! writes) and fails outright, nothing written, on any request error.
//!
//! # Module split
//!
//! [`options`] holds the tuning, progress and outcome types, plus the
//! [`options::LibraryTransport`] abstraction the algorithm runs over. [`sync`] holds the
//! algorithm, its worker-thread wrapper and its tests.

pub mod options;
pub mod sync;

pub use options::{MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress};
pub use sync::spawn_mirror_sync;
