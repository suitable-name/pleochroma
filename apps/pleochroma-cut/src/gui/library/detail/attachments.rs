//! Exporting a design's attached files, from either the local database or a
//! remote worker.

use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::{self as library_source, LibrarySource},
    gui::library::read_access::with_read_db,
    settings::WorkerSettings,
};
use pleochroma_net::library::{LibraryRequest, LibraryResponse};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Exports one attached file from the LOCAL database, by exact name match, to
/// `dest_path`. Returns a human-readable status line on success.
///
/// Reads the attachment list as metadata, then only the matching attachment's bytes (by
/// id), over a read-only connection so the shared lock is held briefly. Blocking file and
/// database I/O: call it from a worker thread.
pub fn export_diagram_file(
    db_mutex: &Arc<Mutex<Database>>,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) -> Result<String, String> {
    let attachment = with_read_db(db_mutex, |db| {
        let meta = db.get_diagram_full_meta(entry_id).ok().flatten()?;
        let id = meta
            .attached_files
            .iter()
            .find(|af| af.name == file_name)?
            .id;
        db.get_attachment_content(id).ok().flatten()
    });

    if let Some((name, content)) = attachment {
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(dest_path, &content).is_ok() {
            return Ok(format!("Saved '{name}' to {}", dest_path.display()));
        }
    }
    Err(format!("Failed to export file '{file_name}'."))
}

/// Exports an attachment from the active library: a local worker or a remote fetch.
///
/// Reports through `ui` (status line and toast) rather than a return value, because the
/// remote path is asynchronous. The Save As dialog runs first, so one prompt serves both
/// branches and cancelling writes nothing. The dialog is seeded with `file_name` in
/// `./exports/` when that directory exists.
///
/// The picker runs off the UI thread via `gui::pickers::pick`. The single caller is
/// `gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`.
pub fn export_diagram_file_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    entry_id: i64,
    file_name: &str,
) {
    let default_dir = std::path::Path::new("exports");
    let extension_filter = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_string);
    let db_mutex = Arc::clone(db_mutex);
    let source = Arc::clone(source);
    let entry_id_owned = entry_id;
    let file_name_owned = file_name.to_string();
    crate::gui::pickers::pick(
        ui,
        crate::gui::pickers::PickerRequest {
            kind: crate::gui::pickers::PickerKind::SaveFile,
            title: None,
            filters: extension_filter
                .map(|ext| vec![crate::gui::pickers::PickerFilter::single(ext)])
                .unwrap_or_default(),
            default_file_name: Some(file_name_owned.clone()),
            starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
        },
        move |ui, dest_path| {
            let Some(dest_path) = dest_path else {
                let msg = "Export cancelled.".to_string();
                ui.global::<LibraryModel>()
                    .set_status_message(msg.clone().into());
                crate::gui::show_toast(ui, &msg, "info");
                return;
            };

            let current = source
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            match current {
                LibrarySource::Local => {
                    // The read and the file write run on a worker; the report comes
                    // back through the event loop.
                    let db_mutex = Arc::clone(&db_mutex);
                    let file_name = file_name_owned.clone();
                    let ui_weak = ui.as_weak();
                    std::thread::spawn(move || {
                        let result =
                            export_diagram_file(&db_mutex, entry_id_owned, &file_name, &dest_path);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                            Ok(msg) => {
                                ui.global::<LibraryModel>()
                                    .set_status_message(msg.clone().into());
                                crate::gui::show_toast(&ui, &msg, "success");
                            }
                            Err(err) => {
                                ui.global::<LibraryModel>()
                                    .set_status_message(err.clone().into());
                                crate::gui::show_toast(&ui, &err, "error");
                            }
                        });
                    });
                }
                LibrarySource::Remote(worker) => {
                    export_diagram_file_remote(
                        ui,
                        worker,
                        entry_id_owned,
                        &file_name_owned,
                        &dest_path,
                    );
                }
            }
        },
    );
}

/// The remote counterpart of [`export_diagram_file`].
///
/// `FetchAttachment` takes an id, while the attachments tab only knows the name, so this
/// first fetches the design to resolve the name, then fetches the bytes. Two round trips
/// are acceptable for a user-initiated export. `dest_path` is already chosen by
/// [`export_diagram_file_via_source`], so this only writes.
fn export_diagram_file_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) {
    let file_name = file_name.to_string();
    let dest_path = dest_path.to_path_buf();
    let worker_for_attachment = worker.clone();
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| {
            let attachment_id = match result {
                Ok(LibraryResponse::Design(record)) => record
                    .attachments
                    .iter()
                    .find(|f| f.name == file_name)
                    .map(|f| f.id),
                _ => None,
            };
            let Some(attachment_id) = attachment_id else {
                report_export_failure(ui, &file_name);
                return;
            };
            let file_name_for_failure = file_name.clone();
            let dest_path_for_attachment = dest_path.clone();
            library_source::spawn_library_request(
                ui.as_weak(),
                worker_for_attachment,
                LibraryRequest::FetchAttachment { attachment_id },
                move |ui, result| match result {
                    Ok(LibraryResponse::Attachment { name, content }) => {
                        if let Some(parent) = dest_path_for_attachment.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::write(&dest_path_for_attachment, &content).is_ok() {
                            let msg =
                                format!("Saved '{name}' to {}", dest_path_for_attachment.display());
                            ui.global::<LibraryModel>()
                                .set_status_message(msg.clone().into());
                            crate::gui::show_toast(ui, &msg, "success");
                        } else {
                            report_export_failure(ui, &file_name_for_failure);
                        }
                    }
                    _ => report_export_failure(ui, &file_name_for_failure),
                },
            );
        },
    );
}

fn report_export_failure(ui: &MainWindow, file_name: &str) {
    let msg = format!("Failed to export file '{file_name}'.");
    ui.global::<LibraryModel>()
        .set_status_message(msg.clone().into());
    crate::gui::show_toast(ui, &msg, "error");
}
