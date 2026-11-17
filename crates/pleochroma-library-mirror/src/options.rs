//! Mirror-sync types: options, counts, progress, outcome and the cancel handle.
//!
//! [`MirrorOptions`]/[`MirrorCounts`]/[`MirrorProgress`]/[`MirrorOutcome`]/[`MirrorHandle`] are
//! what [`crate::run_mirror_sync`] takes and returns. See the crate's own doc comment for the
//! design, and [`crate::LibraryTransport`] for the abstraction the algorithm runs over.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Which of a remote design's files a mirror sync stores locally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MirrorContent {
    /// The design's rows, every attachment (up to [`MirrorOptions::max_attachment_bytes`]) and
    /// its diagram image. What a desktop mirror keeps for offline use.
    #[default]
    Everything,
    /// The design's rows and only the attachments that are design files (a native `.psdd`, or
    /// a `.asc`, `.gem` or `.gcs`). The diagram image (`diagram_image_data` and
    /// `diagram_image_name`) and every other attachment (a PDF, a picture) are left on the
    /// remote, which keeps a phone's copy small. Counted in
    /// [`MirrorCounts::attachments_skipped_by_content`].
    DesignFilesOnly,
}

/// Tuning knobs for one mirror sync. `Default` suits a UI trigger without advanced options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorOptions {
    /// An attachment whose advertised [`pleochroma_net::library::AttachedFileMeta::size`]
    /// exceeds this is skipped; the design is still saved without it (unless
    /// [`Self::skip_designs_over_cap`] says otherwise). `50 MiB` is far above a typical PDF or
    /// diagram image, so it only catches anomalous files.
    pub max_attachment_bytes: u64,
    /// Which of a design's files are stored. Defaults to [`MirrorContent::Everything`].
    pub content: MirrorContent,
    /// When set, a design whose design file (the native `.psdd`, else the `.asc`, `.gem` or
    /// `.gcs` an editor would open) exceeds [`Self::max_attachment_bytes`] is not written at
    /// all and no mirror state is recorded for it, so a later sync checks it again. It is
    /// counted in [`MirrorCounts::designs_skipped_too_large`]. Off by default: then such a
    /// design is saved without that file.
    pub skip_designs_over_cap: bool,
}

impl Default for MirrorOptions {
    fn default() -> Self {
        Self {
            max_attachment_bytes: 50 * 1024 * 1024,
            content: MirrorContent::Everything,
            skip_designs_over_cap: false,
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
    /// revision token changed since the last sync. See the crate doc comment's
    /// "Identity and staleness" section.
    pub skipped_unchanged: usize,
    /// Skipped because the user deleted the mirrored design locally (tombstone); never fetched,
    /// or fetched but not written when the deletion happened during its fetch.
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
    /// `library_mirror_state` entry). The local row is left untouched; see the crate's
    /// safety rule. This is not an error, so it is counted apart from [`Self::failed`], and a
    /// retry would skip it again.
    pub local_conflicts_skipped: usize,
    /// How many `library_mirror_state` rows have no local design left
    /// (`Database::count_mirror_states_without_entry`): designs mirrored earlier and then
    /// deleted locally. Set once at the end of a sync, whether completed or cancelled, not a
    /// running tally. See the crate's "Local delete wins" section.
    pub orphaned_mirror_states: u64,
    /// An attachment left on the remote because [`MirrorContent::DesignFilesOnly`] keeps only
    /// design files.
    pub attachments_skipped_by_content: usize,
    /// A design not written at all because its design file exceeds
    /// [`MirrorOptions::max_attachment_bytes`] and [`MirrorOptions::skip_designs_over_cap`] is
    /// set. It has no mirror state, so the next sync looks at it again.
    pub designs_skipped_too_large: usize,
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

/// How [`crate::run_mirror_sync`] ended.
#[derive(Debug, Clone)]
pub enum MirrorOutcome {
    /// Every design was examined.
    Completed(MirrorCounts),
    /// Stopped early via [`MirrorHandle::cancel`] -- `MirrorCounts` reflects every
    /// design fully processed before cancellation was observed (always a clean prefix,
    /// never a partial design; see the crate doc comment's "Cancellation" section).
    Cancelled(MirrorCounts),
    /// Could not even enumerate the remote catalogue (a `SearchPage` request failed,
    /// on the first page or any later one) -- nothing was written locally at all.
    Failed(String),
}

/// The cancel flag of one mirror sync. Cancelling is cooperative: the sync stops at its next
/// per-design check. Clones share the flag, so one clone can run the sync while another cancels
/// it.
#[derive(Debug, Clone, Default)]
pub struct MirrorHandle {
    cancel: Arc<AtomicBool>,
}

impl MirrorHandle {
    /// A handle that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation; the running job stops at its next check.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether [`Self::cancel`] was called on this handle or any of its clones.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The shared flag, in the form [`crate::run_mirror_sync`] takes.
    #[must_use]
    pub fn flag(&self) -> &AtomicBool {
        &self.cancel
    }
}
