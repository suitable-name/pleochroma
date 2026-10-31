//! The cutter's saved color variants in `IndexedDB`: the browser's twin of the desktop vault's
//! `material_color_variants` table.
//!
//! One object store (`color_variants`, created by the version 3 upgrade in [`super::idb`]),
//! keyed by the variant key (`user.<uuid>`), one record per saved variant:
//!
//! | field | value |
//! |---|---|
//! | `key` | the variant key (also the store key) |
//! | `material_name` | the material it belongs to |
//! | `label` | the display label |
//! | `version` | `RECORD_VERSION` |
//! | `variant_json` | `{"unit","reference_path_mm","absorption"}`, the desktop vault's JSON |
//! | `created_ms`, `updated_ms` | `Date.now()` of the first and the latest write |
//!
//! The record shape, the JSON and the checks are `pleochroma_web_core::design_color`
//! (native-tested); this module only moves them in and out of the store. Every function opens the
//! database for the call and closes it again, resolves when its transaction has completed, and
//! reports a failure as `Err(String)` for the caller to log -- nothing here shows the user
//! anything, and nothing touches the autosave or the library stores.

use super::idb::{
    STORE_COLOR_VARIANTS, js_text, request_result, store, transaction, transaction_done, with_db,
};
use js_sys::{Array, Date, Object, Reflect};
use pleochroma::optics::materials::variants::ColorVariant;
use pleochroma_web_core::design_color::{
    ColorVariantRecord, record_for, user_key, variant_from_record,
};
use wasm_bindgen::JsValue;
use web_sys::IdbTransactionMode;

/// What [`list`] read.
#[derive(Debug, Clone, Default)]
pub struct SavedColors {
    /// Every readable saved variant, in key order.
    pub variants: Vec<ColorVariant>,
    /// The sentences for records this build cannot read (a newer version, damaged JSON): they are
    /// skipped, not deleted.
    pub skipped: Vec<String>,
}

/// 16 random bytes: `crypto.getRandomValues` where the page has it, `Math.random` otherwise (a
/// key needs to be unlikely to collide, not secret).
fn random_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let filled = web_sys::window()
        .and_then(|window| window.crypto().ok())
        .is_some_and(|crypto| crypto.get_random_values_with_u8_array(&mut bytes).is_ok());
    if !filled {
        for byte in &mut bytes {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "random() is in [0, 1), so the product is in [0, 256) and its floor fits a u8"
            )]
            let value = (js_sys::Math::random() * 256.0) as u8;
            *byte = value;
        }
    }
    bytes
}

/// A fresh key for a variant that has none yet (`user.<uuid v4>`).
#[must_use]
pub fn fresh_key() -> String {
    user_key(random_bytes())
}

/// A string field of a stored record.
fn field_string(record: &JsValue, name: &str) -> Option<String> {
    Reflect::get(record, &JsValue::from_str(name))
        .ok()?
        .as_string()
}

/// A number field of a stored record.
fn field_number(record: &JsValue, name: &str) -> Option<f64> {
    Reflect::get(record, &JsValue::from_str(name))
        .ok()?
        .as_f64()
}

/// The stored record as a JS object (`created_ms` kept from an earlier write by the caller).
fn record_value(
    record: &ColorVariantRecord,
    created_ms: f64,
    updated_ms: f64,
) -> Result<JsValue, String> {
    let object = Object::new();
    let set = |name: &str, value: &JsValue| {
        Reflect::set(&object, &JsValue::from_str(name), value)
            .map(|_| ())
            .map_err(|e| js_text(&e))
    };
    set("key", &JsValue::from_str(&record.key))?;
    set("material_name", &JsValue::from_str(&record.material_name))?;
    set("label", &JsValue::from_str(&record.label))?;
    set("version", &JsValue::from_f64(f64::from(record.version)))?;
    set("variant_json", &JsValue::from_str(&record.variant_json))?;
    set("created_ms", &JsValue::from_f64(created_ms))?;
    set("updated_ms", &JsValue::from_f64(updated_ms))?;
    Ok(object.into())
}

/// The record in a stored JS object; `Err` names what is missing.
fn record_from_value(value: &JsValue) -> Result<ColorVariantRecord, String> {
    let text = |name: &str| {
        field_string(value, name)
            .ok_or_else(|| format!("a saved color record has no '{name}' field"))
    };
    let version = field_number(value, "version")
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= f64::from(u32::MAX))
        .ok_or_else(|| "a saved color record has no version".to_owned())?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked to be a whole-range u32 value just above"
    )]
    let version = version as u32;
    Ok(ColorVariantRecord {
        key: text("key")?,
        material_name: text("material_name")?,
        label: text("label")?,
        version,
        variant_json: text("variant_json")?,
    })
}

/// Every saved variant of the browser, readable ones first-class, unreadable ones as sentences.
///
/// # Errors
///
/// `IndexedDB` is unavailable or the read failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn list() -> Result<SavedColors, String> {
    let values = with_db(|db| async move {
        let tx = transaction(&db, &[STORE_COLOR_VARIANTS], IdbTransactionMode::Readonly)?;
        let request = store(&tx, STORE_COLOR_VARIANTS)?
            .get_all()
            .map_err(|e| js_text(&e))?;
        request_result(&request).await
    })
    .await?;
    let mut saved = SavedColors::default();
    for value in Array::from(&values).iter() {
        match record_from_value(&value).and_then(|record| variant_from_record(&record)) {
            Ok(variant) => saved.variants.push(variant),
            Err(sentence) => saved.skipped.push(sentence),
        }
    }
    Ok(saved)
}

/// Writes `variant` (replacing the record under its key; the first write's `created_ms` is kept).
///
/// # Errors
///
/// The variant cannot be stored (`design_color::record_for`: a blank name, a key without the
/// `user.` prefix, a number that is not finite), `IndexedDB` is unavailable, or the write failed
/// (quota included).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn save(variant: &ColorVariant) -> Result<(), String> {
    let record = record_for(variant)?;
    with_db(|db| async move {
        let tx = transaction(&db, &[STORE_COLOR_VARIANTS], IdbTransactionMode::Readwrite)?;
        let colors = store(&tx, STORE_COLOR_VARIANTS)?;
        let key = JsValue::from_str(&record.key);
        let now = Date::now();
        let earlier = request_result(&colors.get(&key).map_err(|e| js_text(&e))?).await?;
        let created = if earlier.is_undefined() || earlier.is_null() {
            now
        } else {
            field_number(&earlier, "created_ms").unwrap_or(now)
        };
        colors
            .put_with_key(&record_value(&record, created, now)?, &key)
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    })
    .await
}

/// Deletes the variant stored under `key` (nothing happens when there is none).
///
/// # Errors
///
/// `IndexedDB` is unavailable or the transaction failed.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub async fn delete(key: &str) -> Result<(), String> {
    let key = key.to_owned();
    with_db(|db| async move {
        let tx = transaction(&db, &[STORE_COLOR_VARIANTS], IdbTransactionMode::Readwrite)?;
        store(&tx, STORE_COLOR_VARIANTS)?
            .delete(&JsValue::from_str(&key))
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    })
    .await
}
