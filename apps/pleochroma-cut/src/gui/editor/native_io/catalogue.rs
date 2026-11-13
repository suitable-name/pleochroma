//! The catalogue write-back after a save's `.psdd` file is on disk.
//!
//! It builds the same [`pleochroma_vault::local::ImportedAsc`] a re-import of the design's files
//! would, then updates the design's row or inserts a new one. See [`write_back_to_catalogue`] for
//! what is overwritten and what is kept.

use pleochroma_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// The outcome of [`write_back_to_catalogue`], used for the toast of
/// [`finish_save_native_success`]. A plain update gets no extra toast; the other cases
/// are called out.
pub(super) enum CatalogueWriteBack {
    /// The known `source_entry_id` still named a real row, which was updated in place.
    /// `stale_cache_invalidation_failed` is true when its cached previews, tilt curves or
    /// solid extents could not be cleared. The cutter is told on screen, since stale cached
    /// geometry would otherwise go unnoticed.
    UpdatedExisting {
        stale_cache_invalidation_failed: bool,
    },
    /// This design had no known source row -- a brand-new one was inserted.
    NewRow,
    /// This design's known `source_entry_id` no longer named a real row (deleted
    /// while the design was open) -- a brand-new row was inserted instead of an
    /// update, same as [`Self::NewRow`], but worth telling the cutter about.
    SourceRowGoneNewRowCreated,
    /// The design's catalogue url (`local://<file name>`) already belongs to a different
    /// catalogue design, so nothing was written. The upsert is keyed by url and would have
    /// renamed that design and replaced its angle table and attachments. `title` is that
    /// other design's title. The id returned with this outcome belongs to the other design,
    /// so a caller must not adopt it as this design's source row.
    UrlCollision { existing_id: i64, title: String },
}

/// Writes this design's catalogue row after the save's file is on disk.
///
/// Called by [`finish_save_native_success`] only after [`write_file_atomically`] succeeded:
/// the file is the design of record, so a failure here never means the save failed.
///
/// The row is built by the same parse-and-measure path as an Import of the design's files
/// ([`pleochroma_vault::local::import_asc`] and
/// [`crate::gui::library::local::apply_measured_metadata`]), so it describes the design
/// exactly as a re-import would.
///
/// # What gets overwritten versus preserved
///
/// - `source_entry_id: Some(id)` and `id` still names a row: the angle table, the attached
///   files and every geometry-derived column (indices, facet count, symmetry, the measured
///   ratios, shape) are replaced with this save's values. Other fields such as
///   `designer_info`, a hand-corrected shape and the competition or citation columns are
///   merged from the existing row ([`crate::gui::library::local::merge_reimport_metadata`]),
///   as on a re-import. The title is never changed. Previews, tilt curves and solid extents
///   are deleted, since they describe the geometry from before this save.
/// - `source_entry_id: Some(id)` but the row is gone (deleted while the design was open):
///   handled as `None`, so the save still lands in a new row.
/// - `source_entry_id: None`: a new row is inserted with its entry and detail in one
///   transaction (`Database::save_design`).
/// - In either case, if the url (`local://<file name>`) belongs to a different row, nothing
///   is written and the outcome is [`CatalogueWriteBack::UrlCollision`]. Otherwise the
///   upsert would rename that design and replace its data. An explicit Import, which asks
///   for confirmation, is the way to overwrite it.
///
/// Returns the row id, which the caller stores as [`EditorState::source_entry_id`], so
/// later saves update the same row. For [`CatalogueWriteBack::UrlCollision`] the id belongs
/// to the other design and must not be stored.
///
/// # Errors
///
/// A ready-to-toast message. A failure here never rolls back the files
/// [`write_file_atomically`] already wrote -- the design is safely on disk either
/// way; this only affects whether the library list reflects it yet.
pub(super) fn write_back_to_catalogue(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    asc_filename: &str,
    asc_text: &str,
    design_filename: &str,
    design_text: &str,
) -> Result<(i64, CatalogueWriteBack), String> {
    // Parse and measure run inside `catch_file_panic`, as in a `.asc` import, because the
    // measurement reaches into `pleochroma`'s geometry code. This runs on Save's background
    // thread, which has no `catch_unwind`: an uncaught panic would kill that thread and the
    // completion would never run. An `Err` is reported by `spawn_native_save_write` instead.
    // The files on disk are already safe at this point.
    let mut parsed = match crate::gui::library::local::catch_file_panic(
        std::panic::AssertUnwindSafe(|| {
            pleochroma_vault::local::import_asc(
                asc_filename,
                asc_text,
                Some((design_filename, design_text.as_bytes())),
            )
            .and_then(|mut parsed| {
                // The design file just written carries the descriptive metadata and
                // the attachments (PDF, `.gem`, diagram image, ...): the row gets them
                // the way a re-import of the file would give them.
                let file = pleochroma_formats::native::design::parse(design_text)
                    .map_err(|e| e.to_string())?;
                pleochroma_vault::local::apply_design_file_meta(&mut parsed, &file)?;
                crate::gui::library::local::apply_measured_metadata(&mut parsed.detail);
                Ok(parsed)
            })
        }),
    ) {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(e)) => {
            return Err(format!(
                "Saved to disk, but could not update the catalogue: {e}"
            ));
        }
        Err(panic_msg) => {
            warn!(
                "Save: catalogue write-back panicked while measuring '{asc_filename}': \
                 {panic_msg}"
            );
            return Err(format!(
                "Saved to disk, but updating the catalogue failed unexpectedly ({panic_msg}) -- \
                 the file itself is safe; try Save again to retry the catalogue update."
            ));
        }
    };

    // The lock covers the database work and nothing else: the caller goes on to
    // toast and refresh the library list, and holding the catalogue mutex across
    // that would serialise it against every other reader.
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_back_locked(&db, source_entry_id, &mut parsed)
}

/// Deletes `existing_id`'s cached previews, tilt curves and solid extents, which describe
/// the design this save just overwrote. Each failure is logged and not propagated. Returns
/// whether any deletion failed, for `CatalogueWriteBack::UpdatedExisting`.
fn invalidate_stale_caches(db: &Database, existing_id: i64) -> bool {
    let preview_failed = db
        .delete_preview_images(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale preview cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let tilt_curves_failed = db
        .delete_tilt_curves(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale tilt-curve cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let solid_extents_failed = db
        .delete_solid_extents(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale solid-extents cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    preview_failed || tilt_curves_failed || solid_extents_failed
}

/// Writes the tags, ignored mark and planner exclusion the saved design file carries
/// onto `entry_id`'s row (they only ever set, never clear). Logged, not propagated: the
/// design file and the row itself are already written.
fn restore_extras(db: &Database, entry_id: i64, extras: &pleochroma_vault::local::ImportedExtras) {
    if let Err(e) = pleochroma_vault::local::apply_imported_extras(db, entry_id, extras) {
        warn!("Save: could not restore the tags/marks of entry #{entry_id}: {e}");
    }
}

/// The catalogue row that owns `url` when that row is not `own_id`, already shaped as
/// the write-back outcome that declines to touch it. `None` when `url` is free or is
/// `own_id`'s own url.
fn url_owned_by_another(
    db: &Database,
    url: &str,
    own_id: Option<i64>,
) -> Result<Option<(i64, CatalogueWriteBack)>, String> {
    let owner = db.diagram_entry_for_url(url).map_err(|e| {
        format!(
            "Saved to disk, but could not check the catalogue for a design with the same \
             file name: {e}"
        )
    })?;
    Ok(owner
        .filter(|(id, _)| Some(*id) != own_id)
        .map(|(existing_id, title)| {
            (
                existing_id,
                CatalogueWriteBack::UrlCollision { existing_id, title },
            )
        }))
}

/// [`write_back_to_catalogue`]'s database half, with the lock already taken --
/// split out so the guard's scope is exactly this call rather than the remainder of
/// its caller.
///
/// Returns the row id written and which of the cases happened: the design's own row
/// updated in place, its row found missing and a fresh one inserted, a first-ever
/// insert for a design that had no row, or a declined write because another design
/// owns the url (see [`write_back_to_catalogue`]'s doc comment).
fn write_back_locked(
    db: &Database,
    source_entry_id: Option<i64>,
    parsed: &mut pleochroma_vault::local::ImportedAsc,
) -> Result<(i64, CatalogueWriteBack), String> {
    if let Some(existing_id) = source_entry_id {
        match db.get_diagram_full(existing_id) {
            Ok(Some(existing)) => {
                // "Save As" to a file name another design already owns would
                // move this row's url onto it; decline before changing anything.
                if let Some(collision) =
                    url_owned_by_another(db, &parsed.entry.url, Some(existing_id))?
                {
                    return Ok(collision);
                }
                crate::gui::library::local::merge_reimport_metadata(&mut parsed.detail, &existing);
                db.update_diagram_entry_url(existing_id, &parsed.entry.url)
                    .map_err(|e| e.to_string())?;
                db.save_diagram_detail(&parsed.detail, existing_id)
                    .map_err(|e| e.to_string())?;
                restore_extras(db, existing_id, &parsed.extras);
                let stale_cache_invalidation_failed = invalidate_stale_caches(db, existing_id);
                return Ok((
                    existing_id,
                    CatalogueWriteBack::UpdatedExisting {
                        stale_cache_invalidation_failed,
                    },
                ));
            }
            Ok(None) => {
                // Source row deleted while this design was open -- fall through to
                // insert a fresh row below, rather than updating nothing or erroring.
            }
            Err(e) => {
                return Err(format!(
                    "Saved to disk, but could not read this design's catalogue row to \
                     update it: {e}"
                ));
            }
        }
    }

    // This design owns no row (none known, or the known one is gone), so any row that
    // holds its url belongs to another design: `save_design` upserts by url and would
    // overwrite that design's title, angle table and attachments.
    if let Some(collision) = url_owned_by_another(db, &parsed.entry.url, None)? {
        return Ok(collision);
    }
    let new_id = db
        .save_design(
            &parsed.entry,
            &parsed.detail,
            pleochroma_vault::local::LOCAL_SOURCE_ID,
        )
        .map_err(|e| e.to_string())?;
    restore_extras(db, new_id, &parsed.extras);
    let outcome = if source_entry_id.is_some() {
        CatalogueWriteBack::SourceRowGoneNewRowCreated
    } else {
        CatalogueWriteBack::NewRow
    };
    Ok((new_id, outcome))
}
