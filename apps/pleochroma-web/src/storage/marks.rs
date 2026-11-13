//! Per-stone cutting marks (Cutting Mode): one JSON blob per stone key under
//! `pleochroma.marks.<stone>.v1`, stored through the small-key policy
//! ([`super::small_set`]).
//!
//! This module moves text, or any `serde` type as JSON. Cutting Mode's blob is
//! [`keys::StoredMarks`] (step key to fingerprint), read and written through
//! [`load_marks`] and [`save_marks`]. Its stone key comes from [`keys::mark_keys`]: the
//! design id when saved, else a hash of the design's text. The marks stay in this browser.

use super::{keys, small_get, small_remove, small_set};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

/// The stored marks text of `stone`, or `None` when absent.
#[must_use]
pub fn get(stone: &str) -> Option<String> {
    small_get(&keys::marks_key(stone))
}

/// Stores `json` as the marks of `stone`; `false` when no storage took it.
pub fn set(stone: &str, json: &str) -> bool {
    small_set(&keys::marks_key(stone), json)
}

/// Forgets the marks of `stone`.
pub fn remove(stone: &str) {
    small_remove(&keys::marks_key(stone));
}

/// The marks of `stone` parsed as `T`; `None` when absent or unparsable. An unparsable
/// entry is left in place, so the caller may overwrite it.
#[must_use]
pub fn get_json<T: DeserializeOwned>(stone: &str) -> Option<T> {
    serde_json::from_str(&get(stone)?).ok()
}

/// Stores `value` as JSON; `false` when it does not serialize or no storage took it.
pub fn set_json<T: Serialize>(stone: &str, value: &T) -> bool {
    serde_json::to_string(value).is_ok_and(|json| set(stone, &json))
}

/// The cutting marks stored under `stone`: step key to the fingerprint it was marked at.
/// `None` when nothing is stored or the stored text does not parse.
#[must_use]
pub fn load_marks(stone: &str) -> Option<BTreeMap<String, String>> {
    get_json::<keys::StoredMarks>(stone).map(|stored| stored.marks)
}

/// Stores `marks` under `stone`. An empty set removes the entry instead. `false` when no
/// storage took the write.
pub fn save_marks(stone: &str, marks: &BTreeMap<String, String>) -> bool {
    if marks.is_empty() {
        remove(stone);
        return true;
    }
    set_json(stone, &keys::StoredMarks::new(marks.clone()))
}
