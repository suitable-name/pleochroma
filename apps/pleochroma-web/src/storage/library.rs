//! The browser design library: the async `IndexedDB` store behind it.
//!
//! The pure record shapes, keys, error classes and the import plan live in
//! [`super::library_record`] (native-testable); this module moves them in and out of the
//! stores that [`super::idb`] creates.
//!
//! # Layout
//!
//! | store | key | value |
//! |---|---|---|
//! | `entries` | design id | `{ design_id, url, updated_ms, summary: <LibraryEntry JSON> }` |
//! | `files` | design id | `{ attachments: [{ name, url, content: Uint8Array }] }` |
//! | `thumbs` | design id, or a variant key | `{ png: Uint8Array, stamp: string }` |
//! | `side` | `<design id>/variant/<id>`, `/cut`, `/lighting` | `{ design_id, kind, json }` |
//! | `meta` | counter name | the counter as a decimal string |
//!
//! A listing reads `entries` only: attachment bytes and thumbnails are separate records so
//! a library of hundreds of designs lists without loading a byte of design text. Side data is
//! keyed by the design id whether or not the design has an entry, like the desktop vault's
//! side tables (a design opened from disk can have variants).
//!
//! # Async shape
//!
//! Every function is `async`, opens the database for the call and closes it again, and
//! resolves when its transaction has completed. All of them return `Result<_, StoreError>`;
//! none shows anything to the user, and none touches the autosave record. A read-modify-write
//! (a counter, a tag edit, a mark) runs inside one `readwrite` transaction; the chained
//! requests rely on the transaction staying active across the microtask that resumes the
//! `.await`, which every current browser guarantees.

use super::{
    idb::{
        INDEX_ENTRIES_URL, STORE_ENTRIES, STORE_FILES, STORE_META, STORE_SIDE, STORE_THUMBS,
        js_text, open_db, request_result, store, transaction, transaction_done,
    },
    library_record::{
        LibraryEntry, RECORD_VERSION, SideKind, StorageEstimate, StoreError, StoreErrorKind,
        StoredFiles, Thumbnail, VariantRecord, apply_mark, check_thumbnail, design_prefix_bounds,
        remove_mark, require_design_id, side_key, variant_prefix_bounds, variant_summaries,
        variant_thumbnail_key,
    },
};
use js_sys::{Array, Date, Object, Reflect, Uint8Array};
use pleochroma_vault::model::{
    cut_progress::CutProgressMark,
    design_lighting::DesignLighting,
    design_variant::{DesignVariant, NewVariant, VariantSummary},
    file::AttachedFile,
};
use serde::{Serialize, de::DeserializeOwned};
use std::future::Future;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbKeyRange, IdbObjectStore, IdbTransactionMode};

/// The counter that numbers entries (`LibraryEntry::item.id`).
const COUNTER_ENTRIES: &str = "entries";
/// The counter that numbers variants.
const COUNTER_VARIANTS: &str = "variants";

/// The result of a store call.
pub type StoreResult<T> = Result<T, StoreError>;

/// What [`list_entries`] read.
#[derive(Debug, Clone)]
pub struct EntryList {
    /// Every readable entry, in key (design id) order; sort them for display.
    pub entries: Vec<LibraryEntry>,
    /// Records that could not be read back in this build's shape (skipped, not deleted).
    pub unreadable: usize,
}

/// A design's entry with its attachments, read together.
#[derive(Debug, Clone)]
pub struct StoredDesignRecord {
    /// The summary record.
    pub entry: LibraryEntry,
    /// The attachments (the design file among them).
    pub files: StoredFiles,
}

// ---------------------------------------------------------------------------------------
// JS plumbing
// ---------------------------------------------------------------------------------------

/// A JS error as text (for `map_err`).
#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` function pointer, which hands over the error by value"
)]
fn js_err(error: JsValue) -> String {
    js_text(&error)
}

/// Opens the database, runs `work`, closes the database.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn with_store<T, F, Fut>(work: F) -> StoreResult<T>
where
    F: FnOnce(IdbDatabase) -> Fut,
    Fut: Future<Output = StoreResult<T>>,
{
    let db = open_db().await?;
    let outcome = work(db.clone()).await;
    db.close();
    outcome
}

/// A field that is present (not `undefined`, not `null`).
fn field(record: &JsValue, key: &str) -> Option<JsValue> {
    Reflect::get(record, &JsValue::from_str(key))
        .ok()
        .filter(|value| !is_absent(value))
}

/// Whether a read found nothing.
fn is_absent(value: &JsValue) -> bool {
    value.is_undefined() || value.is_null()
}

/// A string field.
fn field_string(record: &JsValue, key: &str) -> Option<String> {
    field(record, key)?.as_string()
}

/// A byte-array field.
fn field_bytes(record: &JsValue, key: &str) -> Option<Vec<u8>> {
    field(record, key)?
        .dyn_into::<Uint8Array>()
        .ok()
        .map(|array| array.to_vec())
}

/// Sets `key` on a fresh JS object.
fn set_field(object: &Object, key: &str, value: &JsValue) -> Result<(), String> {
    Reflect::set(object, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(js_err)
}

/// The key as a JS string.
fn key_value(key: &str) -> JsValue {
    JsValue::from_str(key)
}

/// A key range over `[low, high)`.
fn range(bounds: &(String, String)) -> Result<IdbKeyRange, String> {
    IdbKeyRange::bound_with_lower_open_and_upper_open(
        &key_value(&bounds.0),
        &key_value(&bounds.1),
        false,
        true,
    )
    .map_err(js_err)
}

/// The next number of a counter, written in the same transaction.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn next_counter(meta: &IdbObjectStore, name: &str) -> StoreResult<i64> {
    let request = meta.get(&key_value(name)).map_err(js_err)?;
    let current = request_result(&request)
        .await?
        .as_string()
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or(0);
    let next = current.saturating_add(1);
    meta.put_with_key(&JsValue::from_str(&next.to_string()), &key_value(name))
        .map_err(js_err)?;
    Ok(next)
}

// ---------------------------------------------------------------------------------------
// Record <-> JS value
// ---------------------------------------------------------------------------------------

/// The `entries` record of `entry`.
fn entry_value(entry: &LibraryEntry) -> StoreResult<JsValue> {
    let json = serde_json::to_string(entry).map_err(|e| StoreError::invalid(e.to_string()))?;
    let object = Object::new();
    set_field(&object, "design_id", &key_value(&entry.design_id))?;
    set_field(&object, "url", &key_value(&entry.item.url))?;
    set_field(&object, "updated_ms", &JsValue::from_f64(entry.updated_ms))?;
    set_field(&object, "summary", &JsValue::from_str(&json))?;
    Ok(object.into())
}

/// The entry in an `entries` record.
fn entry_from_value(value: &JsValue) -> StoreResult<LibraryEntry> {
    let json = field_string(value, "summary")
        .ok_or_else(|| StoreError::corrupt("an entry record has no summary"))?;
    serde_json::from_str(&json)
        .map_err(|e| StoreError::corrupt(format!("an entry record is unreadable: {e}")))
}

/// The `files` record of `files`.
fn files_value(files: &[AttachedFile]) -> StoreResult<JsValue> {
    let attachments = Array::new();
    for file in files {
        let object = Object::new();
        set_field(&object, "name", &key_value(&file.name))?;
        set_field(&object, "url", &key_value(&file.url))?;
        set_field(
            &object,
            "content",
            &Uint8Array::from(file.content.as_slice()),
        )?;
        attachments.push(&object);
    }
    let record = Object::new();
    set_field(&record, "attachments", &attachments)?;
    Ok(record.into())
}

/// The attachments in a `files` record.
fn files_from_value(value: &JsValue) -> StoreResult<StoredFiles> {
    let attachments = field(value, "attachments")
        .ok_or_else(|| StoreError::corrupt("a files record has no attachments"))?;
    Array::from(&attachments)
        .iter()
        .map(|item| {
            Ok(AttachedFile {
                name: field_string(&item, "name")
                    .ok_or_else(|| StoreError::corrupt("an attachment has no name"))?,
                url: field_string(&item, "url").unwrap_or_default(),
                content: field_bytes(&item, "content")
                    .ok_or_else(|| StoreError::corrupt("an attachment has no bytes"))?,
            })
        })
        .collect()
}

/// A thumbnail record.
fn thumbnail_value(png: &[u8], stamp: &str) -> StoreResult<JsValue> {
    let object = Object::new();
    set_field(&object, "png", &Uint8Array::from(png))?;
    set_field(&object, "stamp", &key_value(stamp))?;
    Ok(object.into())
}

/// The thumbnail in a record.
fn thumbnail_from_value(value: &JsValue) -> StoreResult<Thumbnail> {
    Ok(Thumbnail {
        png: field_bytes(value, "png")
            .ok_or_else(|| StoreError::corrupt("a thumbnail record has no bytes"))?,
        stamp: field_string(value, "stamp").unwrap_or_default(),
    })
}

/// A side-data record holding `value` as JSON.
fn side_value<T: Serialize>(design_id: &str, kind: SideKind, value: &T) -> StoreResult<JsValue> {
    let json = serde_json::to_string(value).map_err(|e| StoreError::invalid(e.to_string()))?;
    let object = Object::new();
    set_field(&object, "design_id", &key_value(design_id))?;
    set_field(&object, "kind", &key_value(kind.name()))?;
    set_field(&object, "json", &JsValue::from_str(&json))?;
    Ok(object.into())
}

/// The value in a side-data record.
fn side_from_value<T: DeserializeOwned>(record: &JsValue) -> StoreResult<T> {
    let json = field_string(record, "json")
        .ok_or_else(|| StoreError::corrupt("a side record has no json"))?;
    serde_json::from_str(&json)
        .map_err(|e| StoreError::corrupt(format!("a side record is unreadable: {e}")))
}

/// Reads one side record; `None` when absent.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn side_get<T: DeserializeOwned>(side: &IdbObjectStore, key: &str) -> StoreResult<Option<T>> {
    let request = side.get(&key_value(key)).map_err(js_err)?;
    let record = request_result(&request).await?;
    if is_absent(&record) {
        return Ok(None);
    }
    side_from_value(&record).map(Some)
}

/// Queues a write of one side record.
fn side_put<T: Serialize>(
    side: &IdbObjectStore,
    design_id: &str,
    kind: SideKind,
    sub: Option<i64>,
    value: &T,
) -> StoreResult<()> {
    side.put_with_key(
        &side_value(design_id, kind, value)?,
        &key_value(&side_key(design_id, kind, sub)),
    )
    .map(|_| ())
    .map_err(|e| StoreError::from(js_err(e)))
}

// ---------------------------------------------------------------------------------------
// Availability, quota, persistence
// ---------------------------------------------------------------------------------------

/// Opens and closes the database once: the library's "is this browser able to keep one".
///
/// # Errors
///
/// `IndexedDB` is missing or blocked (the library screen then says so and hides itself),
/// or the upgrade is blocked by another tab.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn probe() -> StoreResult<()> {
    with_store(|_db| async { Ok(()) }).await
}

/// The browser's `StorageManager`.
fn storage_manager() -> StoreResult<web_sys::StorageManager> {
    let unavailable = || {
        StoreError::new(
            StoreErrorKind::Unavailable,
            "the browser has no StorageManager",
        )
    };
    let window = web_sys::window().ok_or_else(unavailable)?;
    let navigator = window.navigator();
    let manager = Reflect::get(&navigator, &JsValue::from_str("storage")).map_err(js_err)?;
    if is_absent(&manager) {
        return Err(unavailable());
    }
    Ok(manager.unchecked_into())
}

/// Asks the browser to keep this site's data (`navigator.storage.persist()`).
///
/// Returns whether the browser granted it. Call it when the first design is saved to the
/// library, not at page load: some browsers grant it only after user interaction.
///
/// # Errors
///
/// The browser has no `StorageManager` or the request failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn request_persistence() -> StoreResult<bool> {
    let promise = storage_manager()?.persist().map_err(js_err)?;
    let granted = JsFuture::from(promise).await.map_err(js_err)?;
    Ok(granted.as_bool().unwrap_or(false))
}

/// Whether the browser already promised to keep this site's data.
///
/// # Errors
///
/// The browser has no `StorageManager` or the request failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn is_persisted() -> StoreResult<bool> {
    let promise = storage_manager()?.persisted().map_err(js_err)?;
    let persisted = JsFuture::from(promise).await.map_err(js_err)?;
    Ok(persisted.as_bool().unwrap_or(false))
}

/// What the browser reports for this site's storage (`navigator.storage.estimate()`).
///
/// # Errors
///
/// The browser has no `StorageManager` or the request failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn storage_estimate() -> StoreResult<StorageEstimate> {
    let promise = storage_manager()?.estimate().map_err(js_err)?;
    let value = JsFuture::from(promise).await.map_err(js_err)?;
    let number = |key: &str| {
        Reflect::get(&value, &JsValue::from_str(key))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };
    Ok(StorageEstimate {
        usage_bytes: number("usage"),
        quota_bytes: number("quota"),
    })
}

// ---------------------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------------------

/// Every entry of the library.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the read failed. An unreadable record is not an error: it
/// is skipped and counted in [`EntryList::unreadable`].
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn list_entries() -> StoreResult<EntryList> {
    let values = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_ENTRIES], IdbTransactionMode::Readonly)?;
        let entries = store(&tx, STORE_ENTRIES)?;
        let request = entries.get_all().map_err(js_err)?;
        Ok(request_result(&request).await?)
    })
    .await?;
    let mut list = EntryList {
        entries: Vec::new(),
        unreadable: 0,
    };
    for value in Array::from(&values).iter() {
        match entry_from_value(&value) {
            Ok(entry) => list.entries.push(entry),
            Err(_) => list.unreadable += 1,
        }
    }
    Ok(list)
}

/// The entry of `design_id`, `Ok(None)` when there is none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn get_entry(design_id: &str) -> StoreResult<Option<LibraryEntry>> {
    let id = require_design_id(design_id)?;
    let value = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_ENTRIES], IdbTransactionMode::Readonly)?;
        let request = store(&tx, STORE_ENTRIES)?
            .get(&key_value(&id))
            .map_err(js_err)?;
        Ok(request_result(&request).await?)
    })
    .await?;
    if is_absent(&value) {
        return Ok(None);
    }
    entry_from_value(&value).map(Some)
}

/// The entry stored under `url` (the dedupe key), `Ok(None)` when there is none.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn find_by_url(url: &str) -> StoreResult<Option<LibraryEntry>> {
    let url = url.to_owned();
    let value = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_ENTRIES], IdbTransactionMode::Readonly)?;
        let request = store(&tx, STORE_ENTRIES)?
            .index(INDEX_ENTRIES_URL)
            .map_err(js_err)?
            .get(&key_value(&url))
            .map_err(js_err)?;
        Ok(request_result(&request).await?)
    })
    .await?;
    if is_absent(&value) {
        return Ok(None);
    }
    entry_from_value(&value).map(Some)
}

/// The attachments of `design_id`, `Ok(None)` when there are none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn get_files(design_id: &str) -> StoreResult<Option<StoredFiles>> {
    let id = require_design_id(design_id)?;
    let value = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_FILES], IdbTransactionMode::Readonly)?;
        let request = store(&tx, STORE_FILES)?
            .get(&key_value(&id))
            .map_err(js_err)?;
        Ok(request_result(&request).await?)
    })
    .await?;
    if is_absent(&value) {
        return Ok(None);
    }
    files_from_value(&value).map(Some)
}

/// The entry and its attachments of `design_id`, read in one transaction (what Open and
/// Export need); `Ok(None)` when there is no entry.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or a record is unreadable. An entry
/// without a files record yields empty attachments.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn get_design(design_id: &str) -> StoreResult<Option<StoredDesignRecord>> {
    let id = require_design_id(design_id)?;
    let (entry_record, files_record) = with_store(|db| async move {
        let tx = transaction(
            &db,
            &[STORE_ENTRIES, STORE_FILES],
            IdbTransactionMode::Readonly,
        )?;
        let key = key_value(&id);
        let entry_request = store(&tx, STORE_ENTRIES)?.get(&key).map_err(js_err)?;
        let files_request = store(&tx, STORE_FILES)?.get(&key).map_err(js_err)?;
        let entry_record = request_result(&entry_request).await?;
        let files_record = request_result(&files_request).await?;
        Ok((entry_record, files_record))
    })
    .await?;
    if is_absent(&entry_record) {
        return Ok(None);
    }
    let files = if is_absent(&files_record) {
        Vec::new()
    } else {
        files_from_value(&files_record)?
    };
    Ok(Some(StoredDesignRecord {
        entry: entry_from_value(&entry_record)?,
        files,
    }))
}

/// Writes an entry and its attachments (replacing both under the same design id).
///
/// An entry whose `item.id` is `0` gets the id of the entry it replaces, else the next
/// number of the store's counter. The record version is set; `updated_ms` is kept as the
/// caller set it. Returns the entry as stored. Deduping by url is the caller's job (use
/// [`find_by_url`] and `library_record::plan_import`).
///
/// # Errors
///
/// The design id is not a UUID, `IndexedDB` is unavailable, or the write failed (a
/// [`StoreErrorKind::Quota`] error leaves the library as it was and never touches the
/// autosave).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn put_design(
    mut entry: LibraryEntry,
    files: &[AttachedFile],
) -> StoreResult<LibraryEntry> {
    entry.design_id = require_design_id(&entry.design_id)?;
    entry.version = RECORD_VERSION;
    let files_record = files_value(files)?;
    with_store(|db| async move {
        let tx = transaction(
            &db,
            &[STORE_ENTRIES, STORE_FILES, STORE_META],
            IdbTransactionMode::Readwrite,
        )?;
        let entries = store(&tx, STORE_ENTRIES)?;
        let key = key_value(&entry.design_id);
        if entry.item.id == 0 {
            let existing = request_result(&entries.get(&key).map_err(js_err)?).await?;
            let known = (!is_absent(&existing))
                .then(|| entry_from_value(&existing).ok())
                .flatten()
                .map(|stored| stored.item.id)
                .filter(|id| *id != 0);
            entry.item.id = match known {
                Some(id) => id,
                None => next_counter(&store(&tx, STORE_META)?, COUNTER_ENTRIES).await?,
            };
        }
        entries
            .put_with_key(&entry_value(&entry)?, &key)
            .map_err(js_err)?;
        store(&tx, STORE_FILES)?
            .put_with_key(&files_record, &key)
            .map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(entry)
    })
    .await
}

/// Edits one entry in place: `edit` runs on the stored entry and `updated_ms` becomes now.
///
/// To change a field the listing shows (title, shape, ratios), edit `entry.detail` or
/// `entry.item.title` and call `entry.refresh_item()` inside `edit`. Returns the entry as
/// stored, `Ok(None)` when there is none (nothing is written).
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, the record is unreadable, or the write
/// failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn update_entry<F>(design_id: &str, edit: F) -> StoreResult<Option<LibraryEntry>>
where
    F: FnOnce(&mut LibraryEntry),
{
    let id = require_design_id(design_id)?;
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_ENTRIES], IdbTransactionMode::Readwrite)?;
        let entries = store(&tx, STORE_ENTRIES)?;
        let key = key_value(&id);
        let record = request_result(&entries.get(&key).map_err(js_err)?).await?;
        if is_absent(&record) {
            return Ok(None);
        }
        let mut entry = entry_from_value(&record)?;
        edit(&mut entry);
        entry.updated_ms = Date::now();
        entries
            .put_with_key(&entry_value(&entry)?, &key)
            .map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(Some(entry))
    })
    .await
}

/// Replaces the tags of an entry (trimmed, de-duplicated, sorted).
///
/// # Errors
///
/// As [`update_entry`].
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn set_tags(design_id: &str, tags: &[String]) -> StoreResult<Option<LibraryEntry>> {
    let tags = super::library_record::normalize_tags(tags);
    update_entry(design_id, move |entry| entry.tags = tags).await
}

/// Sets or clears the ignored mark of an entry.
///
/// # Errors
///
/// As [`update_entry`].
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn set_ignored(design_id: &str, ignored: bool) -> StoreResult<Option<LibraryEntry>> {
    update_entry(design_id, move |entry| entry.item.ignored = ignored).await
}

/// Deletes a design completely: entry, attachments, thumbnails, variants, cut progress and
/// lighting. Deleting what is not there succeeds.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn delete_design(design_id: &str) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    with_store(|db| async move {
        let tx = transaction(
            &db,
            &[STORE_ENTRIES, STORE_FILES, STORE_THUMBS, STORE_SIDE],
            IdbTransactionMode::Readwrite,
        )?;
        let key = key_value(&id);
        let bounds = design_prefix_bounds(&id);
        let fence = range(&bounds)?;
        let fence: &JsValue = &fence;
        store(&tx, STORE_ENTRIES)?.delete(&key).map_err(js_err)?;
        store(&tx, STORE_FILES)?.delete(&key).map_err(js_err)?;
        let thumbs = store(&tx, STORE_THUMBS)?;
        thumbs.delete(&key).map_err(js_err)?;
        thumbs.delete(fence).map_err(js_err)?;
        store(&tx, STORE_SIDE)?.delete(fence).map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

/// Empties every library store (the autosave is not touched) and restarts the counters.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn clear_library() -> StoreResult<()> {
    with_store(|db| async move {
        let names = [
            STORE_ENTRIES,
            STORE_FILES,
            STORE_THUMBS,
            STORE_SIDE,
            STORE_META,
        ];
        let tx = transaction(&db, &names, IdbTransactionMode::Readwrite)?;
        for name in names {
            store(&tx, name)?.clear().map_err(js_err)?;
        }
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------
// Thumbnails
// ---------------------------------------------------------------------------------------

/// Caches the thumbnail of a design (replacing the previous one).
///
/// `stamp` is text the page chooses to tell whether the picture is still current (a hash of
/// the design text); read it back with [`get_thumbnail`].
///
/// # Errors
///
/// The id is not a UUID, the PNG is empty, not a PNG or over 256 KiB, `IndexedDB` is
/// unavailable, or the write failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn put_thumbnail(design_id: &str, png: &[u8], stamp: &str) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    check_thumbnail(png)?;
    write_thumbnail(id, thumbnail_value(png, stamp)?).await
}

/// Writes one thumbnail record under `key`.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn write_thumbnail(key: String, record: JsValue) -> StoreResult<()> {
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_THUMBS], IdbTransactionMode::Readwrite)?;
        store(&tx, STORE_THUMBS)?
            .put_with_key(&record, &key_value(&key))
            .map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

/// The cached thumbnail of a design, `Ok(None)` when there is none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn get_thumbnail(design_id: &str) -> StoreResult<Option<Thumbnail>> {
    let id = require_design_id(design_id)?;
    Ok(get_thumbnails(std::slice::from_ref(&id))
        .await?
        .into_iter()
        .next()
        .map(|(_, thumbnail)| thumbnail))
}

/// The cached thumbnails of several designs in one transaction; designs without one are
/// left out. The pairs come back in the order of `design_ids`.
///
/// # Errors
///
/// An id is not a UUID, `IndexedDB` is unavailable, or a record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn get_thumbnails(design_ids: &[String]) -> StoreResult<Vec<(String, Thumbnail)>> {
    let ids = design_ids
        .iter()
        .map(|id| require_design_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    read_thumbnails(ids).await
}

/// Reads the thumbnail records stored under `keys`.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn read_thumbnails(keys: Vec<String>) -> StoreResult<Vec<(String, Thumbnail)>> {
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_THUMBS], IdbTransactionMode::Readonly)?;
        let thumbs = store(&tx, STORE_THUMBS)?;
        let mut requests = Vec::with_capacity(keys.len());
        for key in &keys {
            requests.push(thumbs.get(&key_value(key)).map_err(js_err)?);
        }
        let mut found = Vec::new();
        for (key, request) in keys.into_iter().zip(requests) {
            let record = request_result(&request).await?;
            if !is_absent(&record) {
                found.push((key, thumbnail_from_value(&record)?));
            }
        }
        Ok(found)
    })
    .await
}

/// Removes the cached thumbnail of a design (a missing one is fine).
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn delete_thumbnail(design_id: &str) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    delete_key(STORE_THUMBS, id).await
}

/// Deletes one key of one store.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn delete_key(store_name: &'static str, key: String) -> StoreResult<()> {
    with_store(|db| async move {
        let tx = transaction(&db, &[store_name], IdbTransactionMode::Readwrite)?;
        store(&tx, store_name)?
            .delete(&key_value(&key))
            .map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------
// Variants
// ---------------------------------------------------------------------------------------

/// Saves a variant of a design and returns its list row.
///
/// The design need not have an entry. The parent, when given, must be a variant of the same
/// design. A thumbnail is cached beside it when `new.thumbnail_png` is non-empty.
///
/// # Errors
///
/// The id is not a UUID, the name or design text is blank, the parent does not exist, the
/// thumbnail is not an acceptable PNG, `IndexedDB` is unavailable, or the write failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn save_variant(design_id: &str, new: &NewVariant<'_>) -> StoreResult<VariantSummary> {
    let id = require_design_id(design_id)?;
    let thumbnail = new.thumbnail_png.filter(|png| !png.is_empty());
    if let Some(png) = thumbnail {
        check_thumbnail(png)?;
    }
    with_store(|db| async move {
        let tx = transaction(
            &db,
            &[STORE_SIDE, STORE_THUMBS, STORE_META],
            IdbTransactionMode::Readwrite,
        )?;
        let side = store(&tx, STORE_SIDE)?;
        if let Some(parent) = new.parent_variant_id {
            let parent_key = side_key(&id, SideKind::Variant, Some(parent));
            if side_get::<VariantRecord>(&side, &parent_key)
                .await?
                .is_none()
            {
                return Err(StoreError::invalid(
                    "the parent variant does not belong to this design",
                ));
            }
        }
        let variant_id = next_counter(&store(&tx, STORE_META)?, COUNTER_VARIANTS).await?;
        let record = VariantRecord::from_new(new, variant_id)?;
        side_put(&side, &id, SideKind::Variant, Some(variant_id), &record)?;
        if let Some(png) = thumbnail {
            store(&tx, STORE_THUMBS)?
                .put_with_key(
                    &thumbnail_value(png, "")?,
                    &key_value(&variant_thumbnail_key(&id, variant_id)),
                )
                .map_err(js_err)?;
        }
        transaction_done(&tx).await?;
        Ok(record.summary(&id))
    })
    .await
}

/// The variants of a design in id order (a parent that was deleted reads as `None`).
///
/// # Errors
///
/// The id is not a UUID or `IndexedDB` is unavailable. An unreadable record is skipped.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn list_variants(design_id: &str) -> StoreResult<Vec<VariantSummary>> {
    let id = require_design_id(design_id)?;
    let values = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readonly)?;
        let fence = range(&variant_prefix_bounds(&id))?;
        let request = store(&tx, STORE_SIDE)?
            .get_all_with_key(&fence)
            .map_err(js_err)?;
        Ok(request_result(&request).await?)
    })
    .await?;
    let id = require_design_id(design_id)?;
    let records = Array::from(&values)
        .iter()
        .filter_map(|value| side_from_value::<VariantRecord>(&value).ok())
        .collect();
    Ok(variant_summaries(&id, records))
}

/// One variant with its design text, `Ok(None)` when there is none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn load_variant(design_id: &str, variant_id: i64) -> StoreResult<Option<DesignVariant>> {
    let id = require_design_id(design_id)?;
    let key = side_key(&id, SideKind::Variant, Some(variant_id));
    let record = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readonly)?;
        side_get::<VariantRecord>(&store(&tx, STORE_SIDE)?, &key).await
    })
    .await?;
    Ok(record.map(|stored| stored.into_variant(&id)))
}

/// The cached thumbnail PNG of a variant, `Ok(None)` when there is none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn variant_thumbnail(design_id: &str, variant_id: i64) -> StoreResult<Option<Vec<u8>>> {
    let id = require_design_id(design_id)?;
    let key = variant_thumbnail_key(&id, variant_id);
    Ok(read_thumbnails(vec![key])
        .await?
        .into_iter()
        .next()
        .map(|(_, thumbnail)| thumbnail.png))
}

/// Edits one stored variant record; `false` when there is none.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn edit_variant<F>(design_id: &str, variant_id: i64, edit: F) -> StoreResult<bool>
where
    F: FnOnce(&mut VariantRecord) -> StoreResult<()>,
{
    let id = require_design_id(design_id)?;
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readwrite)?;
        let side = store(&tx, STORE_SIDE)?;
        let key = side_key(&id, SideKind::Variant, Some(variant_id));
        let Some(mut record) = side_get::<VariantRecord>(&side, &key).await? else {
            return Ok(false);
        };
        edit(&mut record)?;
        side_put(&side, &id, SideKind::Variant, Some(variant_id), &record)?;
        transaction_done(&tx).await?;
        Ok(true)
    })
    .await
}

/// Renames a variant; `false` when there is none.
///
/// # Errors
///
/// The id is not a UUID, the trimmed name is blank, `IndexedDB` is unavailable, or the write
/// failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn rename_variant(design_id: &str, variant_id: i64, name: &str) -> StoreResult<bool> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(StoreError::invalid("a variant needs a name"));
    }
    edit_variant(design_id, variant_id, move |record| {
        record.name = name;
        Ok(())
    })
    .await
}

/// Sets or clears (blank) the note of a variant; `false` when there is none.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the write failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn set_variant_note(
    design_id: &str,
    variant_id: i64,
    note: Option<&str>,
) -> StoreResult<bool> {
    let note = super::library_record::clean_note(note);
    edit_variant(design_id, variant_id, move |record| {
        record.note = note;
        Ok(())
    })
    .await
}

/// Deletes a variant and its thumbnail; `false` when there was none. Variants made from it
/// stay and read as having no parent.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn delete_variant(design_id: &str, variant_id: i64) -> StoreResult<bool> {
    let id = require_design_id(design_id)?;
    with_store(|db| async move {
        let tx = transaction(
            &db,
            &[STORE_SIDE, STORE_THUMBS],
            IdbTransactionMode::Readwrite,
        )?;
        let side = store(&tx, STORE_SIDE)?;
        let key = key_value(&side_key(&id, SideKind::Variant, Some(variant_id)));
        let existed = !is_absent(&request_result(&side.get(&key).map_err(js_err)?).await?);
        side.delete(&key).map_err(js_err)?;
        store(&tx, STORE_THUMBS)?
            .delete(&key_value(&variant_thumbnail_key(&id, variant_id)))
            .map_err(js_err)?;
        transaction_done(&tx).await?;
        Ok(existed)
    })
    .await
}

// ---------------------------------------------------------------------------------------
// Cut progress and lighting
// ---------------------------------------------------------------------------------------

/// The steps of a design marked done, in the order they were first marked.
///
/// These are the library's copy by design id; Cutting Mode's own marks (`marks.rs`, keyed per
/// the cutting-mode rule) are a separate store until a lane moves them here.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn cut_progress(design_id: &str) -> StoreResult<Vec<CutProgressMark>> {
    let id = require_design_id(design_id)?;
    let key = side_key(&id, SideKind::CutProgress, None);
    let marks = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readonly)?;
        side_get::<Vec<super::library_record::MarkRecord>>(&store(&tx, STORE_SIDE)?, &key).await
    })
    .await?;
    Ok(marks
        .unwrap_or_default()
        .into_iter()
        .map(CutProgressMark::from)
        .collect())
}

/// Marks a step done (replacing an earlier mark of the same step).
///
/// # Errors
///
/// The id is not a UUID, the step key is blank, `IndexedDB` is unavailable, or the write
/// failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn mark_step_done(design_id: &str, mark: &CutProgressMark) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    edit_marks(id, |marks| apply_mark(marks, mark).map(|()| true)).await
}

/// Removes the mark of a step; `false` when it was not marked.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the write failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn unmark_step(design_id: &str, step_key: &str) -> StoreResult<bool> {
    let id = require_design_id(design_id)?;
    let removed = std::cell::Cell::new(false);
    edit_marks(id, |marks| {
        removed.set(remove_mark(marks, step_key));
        Ok(removed.get())
    })
    .await?;
    Ok(removed.get())
}

/// Forgets every mark of a design.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn clear_cut_progress(design_id: &str) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    delete_key(STORE_SIDE, side_key(&id, SideKind::CutProgress, None)).await
}

/// Read-modify-write of a design's mark list; `edit` returns whether to write it back.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn edit_marks<F>(id: String, edit: F) -> StoreResult<()>
where
    F: FnOnce(&mut Vec<super::library_record::MarkRecord>) -> StoreResult<bool>,
{
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readwrite)?;
        let side = store(&tx, STORE_SIDE)?;
        let key = side_key(&id, SideKind::CutProgress, None);
        let mut marks = side_get::<Vec<super::library_record::MarkRecord>>(&side, &key)
            .await?
            .unwrap_or_default();
        if !edit(&mut marks)? {
            return Ok(());
        }
        if marks.is_empty() {
            side.delete(&key_value(&key)).map_err(js_err)?;
        } else {
            side_put(&side, &id, SideKind::CutProgress, None, &marks)?;
        }
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

/// The lighting a design was last shown under, `Ok(None)` when none was stored.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the record is unreadable.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn design_lighting(design_id: &str) -> StoreResult<Option<DesignLighting>> {
    let id = require_design_id(design_id)?;
    let key = side_key(&id, SideKind::Lighting, None);
    let record = with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readonly)?;
        side_get::<super::library_record::LightingRecord>(&store(&tx, STORE_SIDE)?, &key).await
    })
    .await?;
    Ok(record.map(DesignLighting::from))
}

/// Stores the lighting of a design (replacing the previous choice).
///
/// # Errors
///
/// The id is not a UUID, the preset name is blank, `IndexedDB` is unavailable, or the write
/// failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn set_design_lighting(design_id: &str, lighting: &DesignLighting) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    if lighting.preset_name.trim().is_empty() {
        return Err(StoreError::invalid("a lighting choice needs a preset name"));
    }
    let record = super::library_record::LightingRecord::from(lighting);
    with_store(|db| async move {
        let tx = transaction(&db, &[STORE_SIDE], IdbTransactionMode::Readwrite)?;
        side_put(
            &store(&tx, STORE_SIDE)?,
            &id,
            SideKind::Lighting,
            None,
            &record,
        )?;
        transaction_done(&tx).await?;
        Ok(())
    })
    .await
}

/// Forgets the lighting choice of a design.
///
/// # Errors
///
/// The id is not a UUID, `IndexedDB` is unavailable, or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn clear_design_lighting(design_id: &str) -> StoreResult<()> {
    let id = require_design_id(design_id)?;
    delete_key(STORE_SIDE, side_key(&id, SideKind::Lighting, None)).await
}
