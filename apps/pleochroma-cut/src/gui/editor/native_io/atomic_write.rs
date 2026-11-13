//! Writes a design file to disk as one atomic-as-possible operation: stage it under a
//! same-directory temp name (unique per writer, flushed to stable storage), back up
//! whatever already sits at the destination, then rename the temp file into place.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Distinguishes the temp names [`temp_sibling`] hands out within this process, so
/// two writers aimed at the same target never stage into the same file.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How long a writer may hold the [`WriteGate`] before a later save stops waiting for
/// it: a writer thread that panicked never reports back, and the gate must not stay
/// shut for the rest of the session.
const WRITER_STALL_LIMIT: Duration = Duration::from_secs(120);

/// Admits one disk writer at a time. A request that arrives while a writer is in
/// flight is parked (the newest one wins -- an older parked request describes a
/// design the newer one already supersedes) and handed back by [`Self::release`] when
/// the running writer reports in, so two saves never race on the same files.
///
/// Pure bookkeeping over an opaque request type `T`: the caller owns the thread that
/// does the writing and calls [`Self::release`] from the UI thread when it finishes.
#[derive(Debug)]
pub(super) struct WriteGate<T> {
    /// When the writer now in flight was admitted, or `None` while idle.
    in_flight_since: Option<Instant>,
    /// The newest request parked behind the writer in flight.
    queued: Option<T>,
}

impl<T> WriteGate<T> {
    /// An idle gate.
    pub(super) const fn new() -> Self {
        Self {
            in_flight_since: None,
            queued: None,
        }
    }

    /// `Some(request)` when the writer may start now (the gate is marked in flight at
    /// `now`); `None` when a writer is already running and `request` was parked
    /// instead, replacing any older parked request.
    pub(super) fn admit(&mut self, request: T, now: Instant) -> Option<T> {
        let busy = self
            .in_flight_since
            .is_some_and(|since| now.saturating_duration_since(since) < WRITER_STALL_LIMIT);
        if busy {
            self.queued = Some(request);
            return None;
        }
        self.in_flight_since = Some(now);
        Some(request)
    }

    /// The running writer finished: hands back the parked request to start next (the
    /// gate stays in flight, marked at `now`), or `None` and an idle gate.
    pub(super) fn release(&mut self, now: Instant) -> Option<T> {
        let next = self.queued.take();
        self.in_flight_since = next.as_ref().map(|_| now);
        next
    }
}

/// Writes `text` to `path` as one atomic-as-possible operation, so a failed write (full
/// disk, locked file) never leaves a truncated design file behind.
///
/// The text is staged in a same-directory temp sibling ([`temp_sibling`]): the rename that
/// follows then stays on one volume and cannot fall back to copy+delete. The staged file is
/// flushed ([`write_synced`]) before the rename, so a power loss cannot leave a zero-length
/// file under the real name. A file already at `path` is copied to a `.bak` sibling
/// ([`backup_existing`]) first, so the previous version is never lost.
///
/// # Errors
///
/// A ready-to-toast message naming exactly what is on disk afterward (always: nothing
/// was saved).
pub(super) fn write_file_atomically(path: &Path, text: &str) -> Result<(), String> {
    let staged = temp_sibling(path);
    if let Err(e) = write_synced(&staged, text) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            path.display()
        ));
    }
    if let Err(message) = backup_existing(path) {
        let _ = std::fs::remove_file(&staged);
        return Err(message);
    }
    if let Err(e) = std::fs::rename(&staged, path) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Failed to finalize {}: {e}. Nothing was saved.",
            path.display()
        ));
    }
    Ok(())
}

/// A same-directory temp sibling of `path` for staging a write (see
/// [`write_file_atomically`]). Never in `std::env::temp_dir()`, which may be another
/// filesystem.
///
/// Unique per call (`<file>.<pid>.<counter>.tmp`), so concurrent writers to one target
/// never share a staging file. A crash can leave such a file behind; it is never reused.
pub(super) fn temp_sibling(path: &Path) -> PathBuf {
    let suffix = format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from(format!("save{suffix}")),
        |n| {
            let mut s = n.to_os_string();
            s.push(&suffix);
            s
        },
    );
    path.with_file_name(file_name)
}

/// Creates (or truncates) `path`, writes `contents` and flushes the file to stable
/// storage (`File::sync_all`) before returning, so the rename that follows publishes
/// bytes that are actually on disk.
///
/// # Errors
///
/// Any error from creating, writing or syncing the file.
pub(super) fn write_synced(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// `path`'s `.bak` sibling, e.g. `design.psdd` -> `design.psdd.bak`. Only one generation is
/// kept: each save overwrites the previous `.bak`.
fn backup_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.bak"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".bak");
            s
        },
    );
    path.with_file_name(file_name)
}

/// Copies `path` to its [`backup_sibling`] before [`write_file_atomically`] renames the
/// staged file over it. A no-op when `path` does not exist yet. The file is copied, not
/// renamed, so a failed backup aborts the save and leaves `path` untouched.
///
/// # Errors
///
/// A ready-to-toast message naming the file that could not be backed up.
fn backup_existing(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = backup_sibling(path);
    std::fs::copy(path, &backup).map_err(|e| {
        format!(
            "Failed to back up {} to {} before overwriting it: {e}. Nothing was saved.",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}
