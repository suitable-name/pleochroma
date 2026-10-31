//! A minimal `IndexedDB` wrapper: the autosaved current design and the schema of the
//! design library.
//!
//! One database (`pleochroma`, version 3). Store `designs` holds one record under the key
//! `"current"`: a JS object `{ bytes: Uint8Array, meta: string }` where `bytes` is the
//! `.psdd` design file text (attachments included) and `meta` a JSON [`DesignMeta`].
//! `IndexedDB` has no `sessionStorage`-style size ceiling, so the whole file goes in. The
//! library stores (`entries`, `files`, `thumbs`, `side`, `meta`; see
//! [`super::library`]) are created beside it by the version 2 upgrade and never touch it. The
//! version 3 upgrade adds `color_variants` (the cutter's saved color variants, see
//! [`super::color_variants`]) the same way.
//!
//! Everything is async (`wasm-bindgen-futures`) and every failure -- no `IndexedDB`
//! (private mode in some browsers), a blocked open, quota, an aborted transaction --
//! comes back as `Err(String)` for the caller to log, worded `"<Name>: <message>"` when
//! the browser gave a `DOMException` (the library classifies on that name); nothing here
//! shows the user anything. A write resolves when its transaction has completed, not merely
//! when the request was queued.
//!
//! # The autosave is never the library's casualty
//!
//! The autosave functions open the same database but only ever use their own store in their
//! own transactions. A library write that runs out of quota aborts its own transaction and
//! leaves `designs/current` as it was; the upgrade creates each library store behind its own
//! guard, so a failure there cannot stop the `designs` store from existing.

use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use std::future::Future;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    IdbDatabase, IdbObjectStore, IdbOpenDbRequest, IdbRequest, IdbTransaction, IdbTransactionMode,
};

/// The database name.
pub const DB_NAME: &str = "pleochroma";
/// The schema version (bump only with an `onupgradeneeded` migration).
///
/// 1: the `designs` store. 2: the library stores. 3: the `color_variants` store.
pub const DB_VERSION: u32 = 3;
/// The object store holding the autosaved design.
pub const STORE_DESIGNS: &str = "designs";
/// The library's summary records, keyed by design id.
pub const STORE_ENTRIES: &str = "entries";
/// The library's attachment bytes, keyed by design id.
pub const STORE_FILES: &str = "files";
/// The library's thumbnails (PNG bytes), keyed by design id or variant key.
pub const STORE_THUMBS: &str = "thumbs";
/// The library's side data (variants, cut progress, lighting), keyed `<design id>/<kind>...`.
pub const STORE_SIDE: &str = "side";
/// The library's counters.
pub const STORE_META: &str = "meta";
/// The cutter's saved color variants, keyed by variant key (`user.<uuid>`); see
/// [`super::color_variants`].
pub const STORE_COLOR_VARIANTS: &str = "color_variants";
/// The index of [`STORE_ENTRIES`] on the entry's `url`.
pub const INDEX_ENTRIES_URL: &str = "by_url";
/// The index of [`STORE_ENTRIES`] on `updated_ms`.
pub const INDEX_ENTRIES_UPDATED: &str = "by_updated";
/// The index of [`STORE_SIDE`] on `design_id`.
pub const INDEX_SIDE_DESIGN: &str = "by_design";
/// The key of the autosaved current design.
pub const CURRENT_KEY: &str = "current";

/// The small record stored beside the design bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignMeta {
    /// The design's title (`[meta] title`, else its display name).
    pub title: String,
    /// When it was written, milliseconds since the Unix epoch (`Date.now()`).
    pub saved_at_ms: f64,
    /// `EditorSession::current_generation` at write time.
    pub generation: u64,
    /// The design's recorded `.asc` name, if any.
    #[serde(default)]
    pub design_name: Option<String>,
    /// Whether the design had changes no save had captured.
    #[serde(default)]
    pub unsaved: bool,
}

/// What [`load_design`] reads back.
#[derive(Debug, Clone)]
pub struct StoredDesign {
    /// The `.psdd` file bytes (UTF-8 text).
    pub bytes: Vec<u8>,
    /// The meta record (default-filled when the stored one was unreadable).
    pub meta: Option<DesignMeta>,
}

/// A JS error value as text: `"<Name>: <message>"` for a `DOMException`.
pub(super) fn js_text(value: &JsValue) -> String {
    if let Some(text) = value.as_string() {
        return text;
    }
    let field = |key: &str| {
        Reflect::get(value, &JsValue::from_str(key))
            .ok()
            .and_then(|v| v.as_string())
    };
    match (field("name"), field("message")) {
        (Some(name), Some(message)) if !message.is_empty() => format!("{name}: {message}"),
        (_, Some(message)) if !message.is_empty() => message,
        (Some(name), _) => name,
        _ => "unknown IndexedDB error".to_string(),
    }
}

/// A `DOMException` as `"<Name>: <message>"`.
fn dom_text(error: &web_sys::DomException) -> String {
    format!("{}: {}", error.name(), error.message())
}

/// The failure text of a request that fired `error`.
fn request_error_text(request: &IdbRequest) -> String {
    request
        .error()
        .ok()
        .flatten()
        .map_or_else(|| "IndexedDB request failed".to_string(), |e| dom_text(&e))
}

/// Resolves with the request's result once it succeeds.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn request_result(request: &IdbRequest) -> Result<JsValue, String> {
    let promise = Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let ok_request = request.clone();
        let err_request = request.clone();
        let on_success = Closure::once_into_js(move || {
            let value = ok_request.result().unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::NULL, &value);
        });
        let on_error = Closure::once_into_js(move || {
            let text = JsValue::from_str(&request_error_text(&err_request));
            let _ = reject.call1(&JsValue::NULL, &text);
        });
        request.set_onsuccess(Some(on_success.unchecked_ref()));
        request.set_onerror(Some(on_error.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|e| js_text(&e))
}

/// Resolves once the transaction has completed (data durable), rejects when it aborts.
///
/// A failed request aborts its transaction (nothing here calls `preventDefault`), so the
/// abort carries the cause (`QuotaExceededError`, `ConstraintError`, ...) in
/// `transaction.error`.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn transaction_done(tx: &IdbTransaction) -> Result<(), String> {
    let promise = Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let on_complete = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let abort_tx = tx.clone();
        let on_abort = Closure::once_into_js(move || {
            let text = abort_tx.error().map_or_else(
                || "IndexedDB transaction aborted".to_string(),
                |e| dom_text(&e),
            );
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&text));
        });
        tx.set_oncomplete(Some(on_complete.unchecked_ref()));
        tx.set_onabort(Some(on_abort.unchecked_ref()));
    });
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|e| js_text(&e))
}

/// Creates every store and index of the current schema that does not exist yet.
///
/// Runs inside the version-change transaction. Version 1 had only `designs`; a database
/// upgraded from it keeps that store (and its `current` record) because the repeated create
/// fails and is ignored. Each library store is created behind its own guard so one failure
/// cannot stop the others, and `designs` is created first. A later version adds a step here
/// keyed on the old version (`IdbVersionChangeEvent::old_version`).
///
/// Version 3 added `color_variants` as one more `for` entry: every store is created whatever the
/// old version was, so a database upgraded from 2 (or 1) keeps all of its stores and records and
/// only gains the new one.
fn create_schema(db: &IdbDatabase) {
    let _ = db.create_object_store(STORE_DESIGNS);
    for name in [STORE_FILES, STORE_THUMBS, STORE_META, STORE_COLOR_VARIANTS] {
        let _ = db.create_object_store(name);
    }
    if let Ok(entries) = db.create_object_store(STORE_ENTRIES) {
        let _ = entries.create_index_with_str(INDEX_ENTRIES_URL, "url");
        let _ = entries.create_index_with_str(INDEX_ENTRIES_UPDATED, "updated_ms");
    }
    if let Ok(side) = db.create_object_store(STORE_SIDE) {
        let _ = side.create_index_with_str(INDEX_SIDE_DESIGN, "design_id");
    }
}

/// The failure text of an open that another tab's older connection holds up. The word
/// "blocked" is what `library_record::classify_error_text` maps to `Unavailable`.
const OPEN_BLOCKED_TEXT: &str = "blocked: another tab holds an older version of the library";

/// [`request_result`] for the open request, which can also be *blocked*: an upgrade waits for
/// every other connection to close and fires `blocked` (and nothing else) while one does not.
/// That rejects the open, so the library screen and the autosave report an unavailable store
/// instead of waiting forever. If the other tab lets go later and the open then succeeds, the
/// connection is closed again at once (nobody is waiting for it).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn open_request_result(open: &IdbOpenDbRequest) -> Result<JsValue, String> {
    let promise = Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let ok_request = open.clone();
        let err_request = open.clone();
        let late_request = open.clone();
        let reject_blocked = reject.clone();
        let on_success = Closure::once_into_js(move || {
            let value = ok_request.result().unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::NULL, &value);
        });
        let on_error = Closure::once_into_js(move || {
            let text = JsValue::from_str(&request_error_text(&err_request));
            let _ = reject.call1(&JsValue::NULL, &text);
        });
        let on_blocked = Closure::once_into_js(move || {
            let _ = reject_blocked.call1(&JsValue::NULL, &JsValue::from_str(OPEN_BLOCKED_TEXT));
            // Replace the success handler: a connection that arrives after the refusal is closed.
            let close_late = Closure::once_into_js({
                let late = late_request.clone();
                move || {
                    if let Ok(db) = late
                        .result()
                        .and_then(wasm_bindgen::JsCast::dyn_into::<IdbDatabase>)
                    {
                        db.close();
                    }
                }
            });
            late_request.set_onsuccess(Some(close_late.unchecked_ref()));
        });
        open.set_onsuccess(Some(on_success.unchecked_ref()));
        open.set_onerror(Some(on_error.unchecked_ref()));
        open.set_onblocked(Some(on_blocked.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|e| js_text(&e))
}

/// Opens (creating or upgrading on first use) the `pleochroma` database.
///
/// A tab that still holds an older connection blocks the upgrade; the open then fails with
/// [`OPEN_BLOCKED_TEXT`] rather than never settling.
///
/// The connection closes itself when another tab asks for a newer version, so a tab
/// holding an old connection never blocks an upgrade.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn open_db() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .ok_or_else(|| "no window".to_string())?
        .indexed_db()
        .map_err(|e| js_text(&e))?
        .ok_or_else(|| "IndexedDB is unavailable".to_string())?;
    let open = factory
        .open_with_u32(DB_NAME, DB_VERSION)
        .map_err(|e| js_text(&e))?;
    let upgrade_request = open.clone();
    let on_upgrade = Closure::once_into_js(move || {
        if let Ok(result) = upgrade_request.result() {
            create_schema(&result.unchecked_into::<IdbDatabase>());
        }
    });
    open.set_onupgradeneeded(Some(on_upgrade.unchecked_ref()));
    let result = open_request_result(&open).await?;
    let db = result
        .dyn_into::<IdbDatabase>()
        .map_err(|_| "IndexedDB open returned no database".to_string())?;
    let closing = db.clone();
    let on_version_change = Closure::once_into_js(move || closing.close());
    db.set_onversionchange(Some(on_version_change.unchecked_ref()));
    Ok(db)
}

/// Opens the database, runs `work` with it and closes it again, whatever `work` returned.
///
/// # Errors
///
/// The open failed, or `work` did.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn with_db<T, F, Fut>(work: F) -> Result<T, String>
where
    F: FnOnce(IdbDatabase) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let db = open_db().await?;
    let outcome = work(db.clone()).await;
    db.close();
    outcome
}

/// A transaction over `stores` (one name or several).
pub(super) fn transaction(
    db: &IdbDatabase,
    stores: &[&str],
    mode: IdbTransactionMode,
) -> Result<IdbTransaction, String> {
    let opened = if let [only] = stores {
        db.transaction_with_str_and_mode(only, mode)
    } else {
        let names = Array::new();
        for name in stores {
            names.push(&JsValue::from_str(name));
        }
        db.transaction_with_str_sequence_and_mode(&names, mode)
    };
    opened.map_err(|e| js_text(&e))
}

/// One store of a transaction.
pub(super) fn store(tx: &IdbTransaction, name: &str) -> Result<IdbObjectStore, String> {
    tx.object_store(name).map_err(|e| js_text(&e))
}

/// Writes the autosaved design (replacing the previous one).
///
/// # Errors
///
/// `IndexedDB` is unavailable, the open was blocked, the value could not be built, or the
/// transaction failed (quota included).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn save_design(bytes: &[u8], meta: &DesignMeta) -> Result<(), String> {
    let meta_json = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    let value = Object::new();
    Reflect::set(&value, &"bytes".into(), &Uint8Array::from(bytes)).map_err(|e| js_text(&e))?;
    Reflect::set(&value, &"meta".into(), &JsValue::from_str(&meta_json))
        .map_err(|e| js_text(&e))?;

    with_db(|db| async move {
        let tx = transaction(&db, &[STORE_DESIGNS], IdbTransactionMode::Readwrite)?;
        let designs = store(&tx, STORE_DESIGNS)?;
        designs
            .put_with_key(&value, &JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    })
    .await
}

/// Reads the autosaved design, `Ok(None)` when there is none.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the read failed. A record that is present but not shaped
/// like ours is an `Err` too (the caller treats it as "no usable design").
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn load_design() -> Result<Option<StoredDesign>, String> {
    let record = with_db(|db| async move {
        let tx = transaction(&db, &[STORE_DESIGNS], IdbTransactionMode::Readonly)?;
        let designs = store(&tx, STORE_DESIGNS)?;
        let request = designs
            .get(&JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        request_result(&request).await
    })
    .await?;
    if record.is_undefined() || record.is_null() {
        return Ok(None);
    }
    let bytes = Reflect::get(&record, &"bytes".into())
        .map_err(|e| js_text(&e))?
        .dyn_into::<Uint8Array>()
        .map_err(|_| "stored design has no byte array".to_string())?
        .to_vec();
    let meta = Reflect::get(&record, &"meta".into())
        .ok()
        .and_then(|m| m.as_string())
        .and_then(|text| serde_json::from_str::<DesignMeta>(&text).ok());
    Ok(Some(StoredDesign { bytes, meta }))
}

/// Deletes the autosaved design.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn clear_design() -> Result<(), String> {
    with_db(|db| async move {
        let tx = transaction(&db, &[STORE_DESIGNS], IdbTransactionMode::Readwrite)?;
        let designs = store(&tx, STORE_DESIGNS)?;
        designs
            .delete(&JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    })
    .await
}
