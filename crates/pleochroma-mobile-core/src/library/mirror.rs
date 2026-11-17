//! Copying the paired computer's designs into this library: a sync.
//!
//! The algorithm is `pleochroma_library_mirror`'s (shared with Cut): it lists the computer's
//! catalogue, skips what has not changed since the last sync, and writes each new or changed
//! design in one transaction. This module sets it up for a phone and words the result:
//!
//! - **What is kept:** [`phone_mirror_options`]. Only a design's rows and its design file are
//!   copied (not the diagram image, PDFs or pictures: they stay on the computer, which keeps the
//!   phone's copy small). A design whose design file is over the 16 MiB a phone opens is skipped
//!   and counted, so a copy that could not be opened is never made; the next sync checks it
//!   again.
//! - **Where it goes:** the phone's own library database, under the computer's `url` and the
//!   `source_id` of [`RemoteLibrary::source_id`]. The designs read as *synced* (see the module
//!   docs of `library`), and a design the user deletes stays deleted.
//! - **Who is written:** only rows the sync wrote. A design of the user's own with the same
//!   `local://` url wins and is counted as kept.
//! - **How it stops:** by [`MirrorHandle::cancel`] (a clean prefix of finished designs stays), or
//!   by itself after [`FAILURE_STREAK_LIMIT`] designs in a row fail to copy, which usually means
//!   the connection or the disk is gone.

use std::collections::HashSet;

use pleochroma_library_mirror::{LibraryTransport, MirrorOutcome, run_mirror_sync};
pub use pleochroma_library_mirror::{
    MirrorContent, MirrorCounts, MirrorHandle, MirrorOptions, MirrorProgress,
};
use pleochroma_web_core::input::MAX_DESIGN_FILE_BYTES;

use super::{EntryId, LibraryHandle, search::is_live_mirror};
use crate::remote::library::{METERED_SYNC_REFUSAL, RemoteLibrary, RemoteLibraryError};

/// The library revision moves after this many designs were written, so the list on screen
/// refreshes while a long sync runs.
const WRITES_PER_REVISION_BUMP: usize = 25;

/// A sync stops as interrupted when this many designs in a row failed to copy.
const FAILURE_STREAK_LIMIT: usize = 10;

/// What a sync on the phone keeps: design files only, at most the 16 MiB a design file may have
/// here, and a design over that is skipped and counted instead of saved without its file.
#[must_use]
pub const fn phone_mirror_options() -> MirrorOptions {
    MirrorOptions {
        max_attachment_bytes: MAX_DESIGN_FILE_BYTES,
        content: MirrorContent::DesignFilesOnly,
        skip_designs_over_cap: true,
    }
}

/// How a sync ended.
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Every design of the computer was examined.
    Completed(MirrorCounts),
    /// Stopped on request. Every design finished before that stays.
    Cancelled(MirrorCounts),
    /// Stopped by itself after [`FAILURE_STREAK_LIMIT`] failed designs in a row. Carries the
    /// reason the connection gave.
    Interrupted {
        /// What was done before it stopped.
        counts: MirrorCounts,
        /// Why the last request failed.
        error: RemoteLibraryError,
    },
    /// The computer's catalogue could not be listed (or the sync was refused before it began).
    /// Nothing was written.
    Failed(RemoteLibraryError),
}

impl SyncOutcome {
    /// One plain sentence or two for the result panel and the toast; `computer` is the
    /// computer's name.
    #[must_use]
    pub fn summary(&self, computer: &str) -> String {
        match self {
            Self::Completed(counts) => completed_summary(counts, computer),
            Self::Cancelled(counts) => {
                format!(
                    "Sync stopped. {}; sync again to continue.",
                    designs_copied(counts)
                )
            }
            Self::Interrupted { counts, error } => match error {
                RemoteLibraryError::Refused(_) => format!(
                    "{computer} refused this phone. {}. Pair with it again, then sync to continue.",
                    designs_copied(counts)
                ),
                RemoteLibraryError::NoLibrary => format!(
                    "{computer} no longer shares a design library. {}.",
                    designs_copied(counts)
                ),
                _ => format!(
                    "{computer} stopped answering. {}; sync again to continue.",
                    designs_copied(counts)
                ),
            },
            Self::Failed(error) => failed_summary(error, computer),
        }
    }

    /// Whether the sync wrote a design (so the library on screen changed).
    #[must_use]
    pub const fn changed(&self) -> bool {
        match self {
            Self::Completed(counts)
            | Self::Cancelled(counts)
            | Self::Interrupted { counts, .. } => counts.new_count + counts.updated_count > 0,
            Self::Failed(_) => false,
        }
    }
}

/// "37 designs were copied", "1 design was copied" or "No designs were copied".
fn designs_copied(counts: &MirrorCounts) -> String {
    match counts.new_count + counts.updated_count {
        0 => "No designs were copied".to_owned(),
        1 => "1 design was copied".to_owned(),
        copied => format!("{copied} designs were copied"),
    }
}

/// The result of a sync that went through every design.
fn completed_summary(counts: &MirrorCounts, computer: &str) -> String {
    let written = counts.new_count + counts.updated_count;
    let nothing_left_over = counts.failed == 0
        && counts.designs_skipped_too_large == 0
        && counts.local_conflicts_skipped == 0;
    let head = if written == 0 && nothing_left_over {
        format!("Everything from {computer} is already on this phone.")
    } else {
        format!(
            "Synced with {computer}: {} new, {} updated, {} unchanged.",
            counts.new_count, counts.updated_count, counts.skipped_unchanged
        )
    };
    let mut parts = vec![head];
    if counts.failed > 0 {
        parts.push(format!(
            "{} could not be copied; sync again to retry.",
            counts.failed
        ));
    }
    match counts.designs_skipped_too_large {
        0 => {}
        1 => parts.push("1 was too large to copy.".to_owned()),
        many => parts.push(format!("{many} were too large to copy.")),
    }
    match counts.local_conflicts_skipped {
        0 => {}
        1 => parts.push("1 kept as it is on this phone (same file name).".to_owned()),
        many => parts.push(format!(
            "{many} kept as they are on this phone (same file name)."
        )),
    }
    match counts.skipped_deleted {
        0 => {}
        1 => parts.push("1 you deleted was not copied again.".to_owned()),
        many => parts.push(format!("{many} you deleted were not copied again.")),
    }
    parts.join(" ")
}

/// The result of a sync that wrote nothing.
fn failed_summary(error: &RemoteLibraryError, computer: &str) -> String {
    match error {
        RemoteLibraryError::Unreachable(_) => format!(
            "Couldn't reach {computer}. Nothing was changed. Check that it is on and on the same Wi-Fi."
        ),
        RemoteLibraryError::NoLibrary => {
            format!("{computer} no longer shares a design library.")
        }
        RemoteLibraryError::Refused(message) if message == METERED_SYNC_REFUSAL => message.clone(),
        RemoteLibraryError::Refused(message) => {
            format!("{computer} refused the connection: {message}")
        }
        RemoteLibraryError::Interrupted(_) => {
            format!("The connection to {computer} was lost. Nothing was changed.")
        }
        RemoteLibraryError::Failed(message) => {
            format!("Couldn't read the design list from {computer}: {message}")
        }
        RemoteLibraryError::Cancelled => "Sync stopped. Nothing was changed.".to_owned(),
    }
}

/// The line under the progress bar: "Copying 120 of 500: Round Brilliant".
#[must_use]
pub fn progress_text(progress: &MirrorProgress, computer: &str) -> String {
    let total = progress.counts.total_found;
    if progress.current_title.trim().is_empty() {
        format!("Copying {} of {total} from {computer}", progress.processed)
    } else {
        format!(
            "Copying {} of {total}: {}",
            progress.processed,
            progress.current_title.trim()
        )
    }
}

/// What a finished run of the algorithm means for the user.
///
/// `stopped` says the failure streak (not the user) ended it; `last_error` is the error of the
/// last request that failed, which is the reason when nothing could be listed.
pub(super) fn settle(
    outcome: MirrorOutcome,
    stopped: bool,
    last_error: Option<RemoteLibraryError>,
) -> SyncOutcome {
    match outcome {
        MirrorOutcome::Completed(counts) => SyncOutcome::Completed(counts),
        MirrorOutcome::Cancelled(counts) if stopped => SyncOutcome::Interrupted {
            counts,
            error: last_error.unwrap_or_else(|| {
                RemoteLibraryError::Failed(
                    "Several designs in a row could not be copied.".to_owned(),
                )
            }),
        },
        MirrorOutcome::Cancelled(counts) => SyncOutcome::Cancelled(counts),
        MirrorOutcome::Failed(message) => match last_error {
            // Cancelled while the catalogue was listed: nothing was written, nothing failed.
            Some(RemoteLibraryError::Cancelled) => SyncOutcome::Cancelled(MirrorCounts::default()),
            Some(error) => SyncOutcome::Failed(error),
            None => SyncOutcome::Failed(RemoteLibraryError::Failed(message)),
        },
    }
}

impl LibraryHandle {
    /// Copies the computer's designs into this library, see the module docs.
    ///
    /// Blocks for minutes on a large library, so it needs a thread of its own; `handle` (another
    /// clone of which cancels) is the only way to stop it, and the connection stops within about
    /// 200 ms of that. The database is locked per call, so searches and opening designs go on
    /// meanwhile. `on_progress` runs after each design the sync examines. The library revision
    /// moves every 25 written designs and at the end.
    ///
    /// A sync that must not start (a metered network the user did not allow) or cannot start
    /// (no credentials) ends as [`SyncOutcome::Failed`] without a request.
    pub fn sync_from(
        &self,
        remote: &RemoteLibrary,
        handle: &MirrorHandle,
        on_progress: impl FnMut(&MirrorProgress),
    ) -> SyncOutcome {
        if let Some(refusal) = remote.sync_refusal() {
            return SyncOutcome::Failed(RemoteLibraryError::Refused(refusal));
        }
        let source_id = match remote.source_id() {
            Ok(source_id) => source_id,
            Err(error) => return SyncOutcome::Failed(error),
        };
        let cancelled = || handle.is_cancelled();
        let session = remote.session(&cancelled);
        let (outcome, stopped) = self.sync_with(
            &session,
            &source_id,
            phone_mirror_options(),
            handle,
            on_progress,
        );
        settle(outcome, stopped, session.last_error())
    }

    /// The algorithm over any transport, with the revision bumps and the failure streak. The
    /// flag says the streak ended the run: the run then reads as cancelled, and `handle` is
    /// cancelled too.
    pub(super) fn sync_with<T: LibraryTransport>(
        &self,
        transport: &T,
        source_id: &str,
        options: MirrorOptions,
        handle: &MirrorHandle,
        mut on_progress: impl FnMut(&MirrorProgress),
    ) -> (MirrorOutcome, bool) {
        let mut written_at_bump = 0_usize;
        let mut previous_failed = 0_usize;
        let mut streak = 0_usize;
        let mut stopped = false;
        let outcome = run_mirror_sync(
            &self.db,
            transport,
            source_id,
            options,
            handle.flag(),
            |progress: MirrorProgress| {
                let counts = progress.counts;
                let written = counts.new_count + counts.updated_count;
                if written >= written_at_bump + WRITES_PER_REVISION_BUMP {
                    self.bump();
                    written_at_bump = written;
                }
                if counts.failed > previous_failed {
                    streak += 1;
                } else {
                    streak = 0;
                }
                previous_failed = counts.failed;
                if streak >= FAILURE_STREAK_LIMIT && !stopped {
                    stopped = true;
                    handle.cancel();
                }
                on_progress(&progress);
            },
        );
        let written = match &outcome {
            MirrorOutcome::Completed(counts) | MirrorOutcome::Cancelled(counts) => {
                counts.new_count + counts.updated_count
            }
            MirrorOutcome::Failed(_) => 0,
        };
        if written > 0 {
            self.bump();
        }
        (outcome, stopped)
    }

    /// The urls among `urls` a sync copied here and the user has not deleted: what the
    /// computer's list marks "On This Phone".
    #[must_use]
    pub fn mirrored_urls(&self, urls: &[&str]) -> HashSet<String> {
        let database = self.lock();
        urls.iter()
            .filter(|url| is_live_mirror(&database, url))
            .map(|url| (*url).to_owned())
            .collect()
    }

    /// Whether entry `id` is a synced design (see the module docs of `library`). `false` for an
    /// entry that is gone or whose row cannot be read.
    pub(super) fn is_synced(&self, id: EntryId) -> bool {
        let database = self.lock();
        database
            .get_diagram_full_meta_lite(id)
            .ok()
            .flatten()
            .is_some_and(|(meta, _has_image)| is_live_mirror(&database, &meta.url))
    }
}
