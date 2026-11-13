//! Mirror-sync types: options, counts, progress, outcome, handle and the transport trait.
//!
//! [`MirrorOptions`]/[`MirrorCounts`]/[`MirrorProgress`]/[`MirrorOutcome`]/
//! [`MirrorHandle`], and the [`LibraryTransport`] abstraction [`super::sync`]'s
//! algorithm runs generically over. See this group's own `mod.rs` doc comment.

use crate::{
    bridge::library::client::{self as library_client, LibrarySession},
    settings::WorkerSettings,
};
use pleochroma_net::library::{LibraryRequest, LibraryResponse};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The `source_id` a mirror sync attributes every design it saves/updates to -- distinct
/// per configured worker (its address).
#[must_use]
pub fn mirror_source_id(worker: &WorkerSettings) -> String {
    format!("remote-library:{}", worker.address)
}

/// Tuning knobs for one mirror sync. `Default` suits a UI trigger without advanced options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorOptions {
    /// An attachment whose advertised [`pleochroma_net::library::AttachedFileMeta::size`]
    /// exceeds this is skipped; the design is still saved without it. `50 MiB` is far above
    /// a typical PDF or diagram image, so it only catches anomalous files.
    pub max_attachment_bytes: u64,
}

impl Default for MirrorOptions {
    fn default() -> Self {
        Self {
            max_attachment_bytes: 50 * 1024 * 1024,
        }
    }
}

/// Running/final tallies for one mirror sync.
///
/// Both [`MirrorProgress::counts`] (a live snapshot, after each design) and
/// [`MirrorOutcome`]'s completed/cancelled payload (the final snapshot) use this same shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorCounts {
    /// Total designs the remote catalogue reported, across every `SearchPage` page.
    pub total_found: usize,
    /// Designs added to the local library by this sync.
    pub new_count: usize,
    /// Existing designs updated by this sync.
    pub updated_count: usize,
    /// Skipped without a `FetchDesign` at all -- neither the summary hash nor the design
    /// revision token changed since the last sync. See the module doc comment's
    /// "Identity and staleness" section.
    pub skipped_unchanged: usize,
    /// Skipped because the user deleted the mirrored design locally (tombstone); never fetched.
    pub skipped_deleted: usize,
    /// A design whose fetch or local save failed for any reason (network error,
    /// database error, or it vanished server-side between `SearchPage` and `FetchDesign`).
    /// Left as it was locally (if it existed at all), never marked as synced, so the next sync
    /// retries it.
    pub failed: usize,
    /// Attachment files downloaded by this sync.
    pub attachments_fetched: usize,
    /// An attachment skipped for exceeding [`MirrorOptions::max_attachment_bytes`] --
    /// its design was still saved, just without this one file's bytes.
    pub attachments_skipped_too_large: usize,
    /// Total bytes of attachments downloaded by this sync.
    pub attachment_bytes_fetched: u64,
    /// A remote design whose `url` names a local row this sync never mirrored (no
    /// `library_mirror_state` entry). The local row is left untouched; see the mirror module's
    /// safety rule. This is not an error, so it is counted apart from [`Self::failed`], and a
    /// retry would skip it again.
    pub local_conflicts_skipped: usize,
    /// How many `library_mirror_state` rows have no local design left
    /// (`Database::count_mirror_states_without_entry`): designs mirrored earlier and then
    /// deleted locally. Set once at the end of a sync, whether completed or cancelled, not a
    /// running tally. See the mirror module's "Local delete wins" section.
    pub orphaned_mirror_states: u64,
}

/// One progress update, delivered after each design this sync examines (whether it was
/// skipped, saved, or failed).
#[derive(Debug, Clone)]
pub struct MirrorProgress {
    /// How many of `counts.total_found` designs have been examined so far, including
    /// this one.
    pub processed: usize,
    /// Running totals of the sync so far.
    pub counts: MirrorCounts,
    /// Title of the design currently being synced.
    pub current_title: String,
}

/// How [`super::sync::run_mirror_sync`] ended.
#[derive(Debug, Clone)]
pub enum MirrorOutcome {
    Completed(MirrorCounts),
    /// Stopped early via [`MirrorHandle::cancel`] -- `MirrorCounts` reflects every
    /// design fully processed before cancellation was observed (always a clean prefix,
    /// never a partial design; see the module doc comment's "Cancellation" section).
    Cancelled(MirrorCounts),
    /// Could not even enumerate the remote catalogue (a `SearchPage` request failed,
    /// on the first page or any later one) -- nothing was written locally at all.
    Failed(String),
}

/// Handle returned by [`super::sync::spawn_mirror_sync`]. Cancelling is cooperative: the sync
/// stops at its next per-design check.
pub struct MirrorHandle {
    pub(super) cancel: Arc<AtomicBool>,
}

impl MirrorHandle {
    /// Requests cancellation; the running job stops at its next check.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A source of [`LibraryResponse`]s for exactly one [`LibraryRequest`] at a time.
///
/// Lets the sync run against a scripted in-memory fake (`sync::tests::fixtures::FakeTransport`)
/// with no socket or TLS handshake. Production uses [`LibrarySession`], which holds one
/// connection for the whole sync and reconnects once per request if it drops. The
/// [`WorkerSettings`] impl makes a new connection per call; the sync does not use it, but the
/// interactive remote browse does, through the same one-shot `request`.
pub trait LibraryTransport {
    /// # Errors
    ///
    /// Whatever the underlying transport failed with -- see
    /// `bridge::library::client::LibraryClientError`'s variants.
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError>;
}

impl LibraryTransport for WorkerSettings {
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError> {
        library_client::request(self, req)
    }
}

impl LibraryTransport for LibrarySession {
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError> {
        // Resolves to `LibrarySession`'s own inherent `request` (inherent methods take
        // priority over a trait method of the same name) -- see that type's doc comment.
        self.request(req)
    }
}
