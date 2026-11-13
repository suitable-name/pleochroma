//! GPU admission records, one per adapter.
//!
//! Each record lives under `pleochroma.admission.<adapter>.v1` and says whether the adapter
//! may be used or must not be probed again. It is stored through the small-key policy
//! ([`super::small_set`]).
//!
//! The typed record and the crash-marker scan are pure and live in
//! `pleochroma_web_core::gpu_admission` (native tests). This module reads and writes them
//! and scans both storages for keys left in the [`AdmissionState::Probing`] state, which
//! mark a probe that never finished (a browser or GPU process crash).

use serde::{Serialize, de::DeserializeOwned};

use pleochroma_web_core::gpu_admission::probing_keys;
pub use pleochroma_web_core::gpu_admission::{AdmissionRecord, AdmissionState};

use super::{keys, small_get, small_remove, small_set};

/// Normalizes `adapter_or_key` into a full storage key. A value that already has the
/// admission key prefix is kept; anything else is treated as an adapter name.
#[must_use]
pub fn resolve_key(adapter_or_key: &str) -> String {
    keys::resolve_admission_key(adapter_or_key)
}

/// The stored record text of `adapter_or_key`, or `None` when absent.
#[must_use]
pub fn get(adapter_or_key: &str) -> Option<String> {
    small_get(&resolve_key(adapter_or_key))
}

/// Stores `json` as the record of `adapter_or_key`; `false` when no storage took it.
pub fn set(adapter_or_key: &str, json: &str) -> bool {
    small_set(&resolve_key(adapter_or_key), json)
}

/// Forgets the record of `adapter_or_key` (lets it be probed again).
pub fn remove(adapter_or_key: &str) {
    small_remove(&resolve_key(adapter_or_key));
}

/// The record of `adapter_or_key` parsed as `T`; `None` when absent or unparsable.
#[must_use]
pub fn get_json<T: DeserializeOwned>(adapter_or_key: &str) -> Option<T> {
    serde_json::from_str(&get(adapter_or_key)?).ok()
}

/// Stores `value` as JSON; `false` when it does not serialize or no storage took it.
pub fn set_json<T: Serialize>(adapter_or_key: &str, value: &T) -> bool {
    serde_json::to_string(value).is_ok_and(|json| set(adapter_or_key, &json))
}

/// Loads the [`AdmissionRecord`] of `adapter_or_key`, or `None` when absent or unparsable.
#[must_use]
pub fn load(adapter_or_key: &str) -> Option<AdmissionRecord> {
    get_json(adapter_or_key)
}

/// Stores `record` as the admission record of `adapter_or_key`; `false` when storage failed.
pub fn save(adapter_or_key: &str, record: &AdmissionRecord) -> bool {
    set_json(adapter_or_key, record)
}

/// Forgets the admission record of `adapter_or_key` (lets it be probed again).
pub fn forget(adapter_or_key: &str) {
    remove(adapter_or_key);
}

/// Every admission key, in either storage, whose record is in the
/// [`AdmissionState::Probing`] state.
#[must_use]
pub fn all_probing() -> Vec<String> {
    let mut found = Vec::new();
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            found.extend(scan_storage_for_probing(&storage));
        }
        if let Ok(Some(storage)) = window.session_storage() {
            found.extend(scan_storage_for_probing(&storage));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The probing admission keys of one `web_sys::Storage`.
fn scan_storage_for_probing(storage: &web_sys::Storage) -> Vec<String> {
    let Ok(len) = storage.length() else {
        return Vec::new();
    };
    let mut entries: Vec<(String, String)> = Vec::new();
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i)
            && let Ok(Some(value)) = storage.get_item(&key)
        {
            entries.push((key, value));
        }
    }
    probing_keys(entries.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}
