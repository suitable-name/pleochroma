//! The disk limit of the path cache and its least-recently-used eviction (guide section 8.9).
//!
//! Only folders named like a [`CacheKey`] (32 lowercase hex digits) are ever looked at or deleted,
//! so a root that holds other files is safe. The eviction order is the `meta.json` last-used time
//! (oldest first), ties broken by the key hex, so it is the same on every run.

use std::{fs, io, path::Path};

use super::{
    key::CacheKey,
    persist::{KeyFolder, folder_size, read_meta},
};

/// The default disk limit: 50 GB (decimal, as the `path_cache_disk_limit_gb` setting is read).
pub const DEFAULT_DISK_LIMIT_BYTES: u64 = 50_000_000_000;

/// One cache folder, as the eviction sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    /// The key as 32 lowercase hex digits (the folder name).
    pub key: String,
    /// Bytes on disk.
    pub size_bytes: u64,
    /// Last use in Unix seconds; 0 for a folder without readable metadata.
    pub last_used_unix: u64,
}

/// Lists the cache folders under `root`, sorted by key; a missing root is empty.
///
/// # Errors
///
/// Returns the I/O error when the root exists but cannot be listed.
pub fn scan(root: &Path) -> io::Result<Vec<CacheEntry>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if CacheKey::from_hex(&name).is_none() || !entry.path().is_dir() {
            continue;
        }
        let folder = entry.path();
        let (size_bytes, last_used_unix) = read_meta(&folder).map_or_else(
            || (folder_size(&folder), 0),
            |meta| {
                let size = if meta.complete && meta.size_bytes > 0 {
                    meta.size_bytes
                } else {
                    folder_size(&folder)
                };
                (size, meta.last_used_unix)
            },
        );
        out.push(CacheEntry {
            key: name,
            size_bytes,
            last_used_unix,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// The total bytes of the cache folders under `root`.
///
/// # Errors
///
/// Returns the I/O error when the root cannot be listed.
pub fn total_size(root: &Path) -> io::Result<u64> {
    Ok(scan(root)?.iter().map(|entry| entry.size_bytes).sum())
}

/// The keys to delete so the total fits `limit_bytes`, in the order to delete them.
///
/// Oldest last-used time first, ties by key hex. The `protect` key (the cache just written) is
/// never chosen, so the result may still exceed the limit when that one cache alone does.
#[must_use]
pub fn plan_eviction(
    entries: &[CacheEntry],
    limit_bytes: u64,
    protect: Option<&str>,
) -> Vec<String> {
    let mut total: u64 = entries.iter().map(|entry| entry.size_bytes).sum();
    let mut order: Vec<&CacheEntry> = entries.iter().collect();
    order.sort_by(|a, b| {
        a.last_used_unix
            .cmp(&b.last_used_unix)
            .then_with(|| a.key.cmp(&b.key))
    });
    let mut evict = Vec::new();
    for entry in order {
        if total <= limit_bytes {
            break;
        }
        if protect == Some(entry.key.as_str()) {
            continue;
        }
        total = total.saturating_sub(entry.size_bytes);
        evict.push(entry.key.clone());
    }
    evict
}

/// Deletes least-recently-used caches under `root` until the total fits `limit_bytes`.
///
/// Returns the evicted keys in deletion order. A folder that cannot be deleted is logged and
/// skipped.
///
/// # Errors
///
/// Returns the I/O error when the root cannot be listed.
pub fn enforce_limit(
    root: &Path,
    limit_bytes: u64,
    protect: Option<&CacheKey>,
) -> io::Result<Vec<String>> {
    let entries = scan(root)?;
    let protect = protect.map(ToString::to_string);
    let mut evicted = Vec::new();
    for key in plan_eviction(&entries, limit_bytes, protect.as_deref()) {
        let Some(parsed) = CacheKey::from_hex(&key) else {
            continue;
        };
        let dir = KeyFolder::folder_path(root, &parsed);
        match fs::remove_dir_all(&dir) {
            Ok(()) => {
                tracing::info!(key = %key, "path cache: evicted to stay under the disk limit");
                evicted.push(key);
            }
            Err(error) => {
                tracing::warn!(key = %key, %error, "path cache: could not evict a cache");
            }
        }
    }
    Ok(evicted)
}

/// Deletes everything inside `root` (the "Clear path cache" button); returns the entries deleted.
///
/// The root folder itself stays. A missing root is already clear.
///
/// # Errors
///
/// Returns the first I/O error; entries deleted before it stay deleted.
pub fn clear(root: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    let mut removed = 0;
    for path in paths {
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        removed += 1;
    }
    Ok(removed)
}
