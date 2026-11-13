//! Wires up the detail header's metadata editor modal to the local database.

use super::{local_load::load_diagram_detail, shared::non_empty};
use crate::{
    EditorModel, LibraryModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{library::local::refresh_after_library_change, show_toast},
};
use pleochroma_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Wires the detail header's metadata editor (`metadata_editor_dialog.slint`) to
/// `save_metadata`.
///
/// Local designs only. A remote `selected_entry_id` names a row on the remote server, not
/// in this database, and the library-sync protocol is read-only. The header also hides the
/// editor for remote designs, so this guard is a backstop.
///
/// Title and metadata save in one transaction through
/// [`Database::rename_and_update_metadata`]. The title lives in `diagram_entries`. The
/// metadata `UPDATE` touches only the twelve fields given, unlike `save_diagram_detail`.
///
/// On success the design is reloaded from the database rather than patched in place. The
/// reload picks up SQLite's numeric normalisation (`"1.760"` reads back `"1.76"`) and the
/// 3-decimal display formatting, so the header matches a fresh open. The planes are also
/// reconstructed, because `shape` feeds `reconstruct_planes`.
pub fn setup_save_metadata_callback(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let db_meta = Arc::clone(db_mutex);
    let source_meta = Arc::clone(source);
    let render_ctx_meta = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_save_metadata(
        move |title: SharedString,
              designer: SharedString,
              shape: SharedString,
              refractive_index: SharedString,
              index_gear: SharedString,
              facets_count: SharedString,
              symmetry_order: SharedString,
              mirror_symmetry: bool,
              lw_ratio: SharedString,
              hw_ratio: SharedString,
              cw_ratio: SharedString,
              pw_ratio: SharedString,
              volume: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_meta
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to edit a design's metadata.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let entry_id = i64::from(entry_id);

            let update = MetadataUpdate {
                designer_info: non_empty(&designer),
                shape: non_empty(&shape),
                refractive_index: non_empty(&refractive_index),
                index_gear: non_empty(&index_gear),
                facets_count: non_empty(&facets_count),
                symmetry_order: non_empty(&symmetry_order),
                mirror_symmetry: Some(mirror_symmetry),
                lw_ratio: non_empty(&lw_ratio),
                hw_ratio: non_empty(&hw_ratio),
                cw_ratio: non_empty(&cw_ratio),
                pw_ratio: non_empty(&pw_ratio),
                volume: non_empty(&volume),
            };
            // Title and metadata in one transaction: a rejected numeric field must not
            // leave the new title applied while the toast reports a failure.
            //
            // The write and the cache deletes run on a worker, as in
            // `refresh_after_library_change`. The UI work returns through the event loop.
            let db = Arc::clone(&db_meta);
            let source = Arc::clone(&source_meta);
            let render_ctx = Arc::clone(&render_ctx_meta);
            let ui_weak = ui.as_weak();
            let title = title.to_string();
            std::thread::spawn(move || {
                let result = {
                    let guard = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let result = guard.rename_and_update_metadata(entry_id, &title, &update);
                    if result.is_ok() {
                        invalidate_derived_caches(&guard, entry_id);
                    }
                    drop(guard);
                    result.map_err(|e| e.to_string())
                };
                // The queued closure holds the vault weakly, so a window closed meanwhile
                // does not keep the database file open until the event loop runs it.
                let db_weak = Arc::downgrade(&db);
                drop(db);
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    if let Some(db) = db_weak.upgrade() {
                        finish_save_metadata(&ui, &db, &source, &render_ctx, entry_id, result);
                    }
                });
            });
        },
    );
}

/// The UI half of the metadata save, run on the event loop after the write finishes.
///
/// Shows the toast, sets the editor's dirty flag, reloads the detail and refreshes the list.
/// The refresh keeps the narrowed range filters, since a metadata edit should not clear them.
fn finish_save_metadata(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => {
            show_toast(ui, "Metadata updated.", "success");
            // When this is the design open in the editor, its file now lags the
            // row: mark it unsaved so the next Save writes the edit.
            if crate::gui::editor::native_io::mark_design_row_edited(entry_id) {
                ui.global::<EditorModel>().set_is_dirty(true);
            }
            // The selection may have moved while the worker wrote; reloading the old
            // row then would put its header over the newly selected design.
            if i64::from(ui.global::<LibraryModel>().get_selected_entry_id()) == entry_id {
                load_diagram_detail(ui, db, render_ctx, entry_id);
            }
            refresh_after_library_change(ui, db, source);
        }
        Err(e) => show_toast(ui, &format!("Metadata update failed: {e}"), "error"),
    }
}

/// Deletes `entry_id`'s cached previews, tilt curves and solid extents after a metadata edit.
///
/// The edited fields (shape, index, gear) feed those renders and sweeps. Errors are logged,
/// not propagated, since the edit itself succeeded.
fn invalidate_derived_caches(db: &Database, entry_id: i64) {
    if let Err(e) = db.delete_preview_images(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached previews for #{entry_id}: {e}");
    }
    if let Err(e) = db.delete_tilt_curves(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached tilt curves for #{entry_id}: {e}");
    }
    if let Err(e) = db.delete_solid_extents(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached solid extents for #{entry_id}: {e}");
    }
}
