//! `localStorage` for small string keys: settings, guide state, cutting marks, GPU
//! admission records.
//!
//! The store is shared by every tab of the origin and kept across restarts.
//!
//! Failures (blocked storage, private mode, quota) are swallowed: a read returns `None`,
//! a write returns `false`. Callers decide whether to fall back (see [`super::small_get`])
//! or to log.

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Whether `localStorage` can be reached at all.
#[must_use]
pub fn available() -> bool {
    storage().is_some()
}

/// Reads `key`, or `None` when absent or storage is unavailable.
#[must_use]
pub fn get(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

/// Writes `value` under `key`; `false` when storage is unavailable or full.
pub fn set(key: &str, value: &str) -> bool {
    storage().is_some_and(|s| s.set_item(key, value).is_ok())
}

/// Removes `key` (a missing key or unavailable storage is not an error).
pub fn remove(key: &str) {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(key);
    }
}
