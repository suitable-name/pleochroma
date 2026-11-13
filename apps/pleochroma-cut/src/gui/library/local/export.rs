//! Exports a local design to an `.asc` file, or experimentally to a Gem Cut Studio `.gcs`
//! file. Import and organize live in their own sibling modules.

use crate::{LibraryModel, MainWindow, bridge::library::source::LibrarySource, gui::show_toast};
use pleochroma_vault::{db::sqlite::Database, local};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// How many of `full`'s schedule rows are concave tiers (rows carrying a tool).
fn concave_row_count(full: &pleochroma_vault::model::entry::FullDiagramRecord) -> usize {
    full.angle_settings
        .iter()
        .filter(|row| row.tool.is_some())
        .count()
}

/// What exporting `full` as a tier list loses, as one dialog paragraph; `None` for a planar
/// design. The wording comes from the editor's `ManufacturabilityWarning`, so the two match.
fn concave_export_notice(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<String> {
    let count = concave_row_count(full);
    (count > 0).then(|| {
        pleochroma_cut_core::ManufacturabilityWarning::ConcaveTiersOmittedFromExport { count }
            .to_string()
    })
}

/// Splits `full`'s schedule for a reconstructed `.asc`. The flat rows form the tier list.
/// Each concave row becomes footnote lines, written by cut-core's own footnote writer. Without
/// the split, a concave row would be a flat tier with a placeholder mast, which is a facet the
/// stone does not have.
fn split_concave_rows(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> (
    Vec<pleochroma_vault::model::angle::AngleSetting>,
    Vec<String>,
) {
    use pleochroma_cut_core::design::{concave_facet_footnote, concave_tool_footnote};
    let mut flat = Vec::with_capacity(full.angle_settings.len());
    let mut footnotes = Vec::new();
    for row in &full.angle_settings {
        if row.tool.is_none() {
            flat.push(row.clone());
            continue;
        }
        footnotes.push(concave_facet_footnote(
            &row.facet, &row.angle, &row.index, &row.notes,
        ));
        if let Some(line) = &row.tool_line {
            footnotes.push(concave_tool_footnote(line));
        }
    }
    (flat, footnotes)
}

/// The sentence a `.asc` export of `full` shows when the design is frosted; `None` for a
/// polished design. `.asc` cannot hold frosting, which the library keeps in a side table, so
/// every `.asc` written from an entry is polished. The wording matches
/// `Design::asc_frosting_note`.
fn frosting_export_notice(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<String> {
    let count = full.frosting.len();
    (count > 0).then(|| {
        format!(
            "The .asc format has no notation for frosting, so the {count} frosted {} will be \
             written polished. A Gem Cut Studio (.gcs) export or the design file keeps it.",
            if count == 1 { "tier" } else { "tiers" }
        )
    })
}

/// The confirm dialog's title and paragraph for what an export loses: the concave tiers,
/// the frosting, or both. `None` when nothing is lost. The titles match the editor's.
fn loss_notice(
    concave: Option<String>,
    frosting: Option<String>,
) -> Option<(&'static str, String)> {
    match (concave, frosting) {
        (None, None) => None,
        (Some(concave), None) => Some(("Concave Tiers Are Not Part of This Format", concave)),
        (None, Some(frosting)) => Some(("Frosting Is Not Part of This Format", frosting)),
        (Some(concave), Some(frosting)) => Some((
            "Some Content Is Not Part of This Format",
            format!("{concave}\n{frosting}"),
        )),
    }
}

/// Runs `proceed` at once when nothing is lost. Otherwise it runs after the cutter accepts
/// the loss in `notice` (see [`loss_notice`]). This happens before any picker opens, so
/// declining writes nothing.
fn confirm_loss_then(
    ui: &MainWindow,
    notice: Option<(&'static str, String)>,
    proceed: impl FnOnce(&MainWindow) + 'static,
) {
    match notice {
        None => proceed(ui),
        Some((title, notice)) => crate::gui::editor::native_io::ask_write_confirm(
            ui,
            title,
            format!("{notice}\n\nExport anyway?"),
            "Export Anyway",
            None,
            proceed,
        ),
    }
}

/// Wires "Export .asc".
///
/// A design with an original `.asc` attachment exports the stored bytes unchanged, through the
/// same path as the Attachments tab. The stored bytes are the file as imported, so a
/// Windows-1252 file stays Windows-1252. Readers decode both the raw bytes and older UTF-8
/// copies with `pleochroma_formats::asc::decode_asc_bytes`. Otherwise the `.asc` is rebuilt
/// from the angle table, and the result is marked `RECONSTRUCTED`.
///
/// Refuses while a remote library is browsed. The selected id names a row on the remote
/// catalogue, so it must never be looked up in the local database.
///
/// Also wires the experimental "Export as Gem Cut Studio (.gcs)..." (see
/// [`setup_export_gcs_callback`]).
pub fn setup_export_asc_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_export = Arc::clone(db);
    let source_export = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if source_export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_remote()
        {
            show_toast(
                &ui,
                "Switch to the local library to export a reconstructed .asc (use the Attachments tab to download a remote design's own files).",
                "error",
            );
            return;
        }
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            show_toast(&ui, "No diagram selected for export.", "error");
            return;
        }

        // The record (attachment bytes included) is read on a worker; the dialogs
        // continue from the event loop once it lands.
        let db_for_start = Arc::clone(&db_export);
        read_full_record_then(&ui, &db_export, i64::from(entry_id), move |ui, full| {
            start_asc_export(ui, &db_for_start, entry_id, full);
        });
    });

    setup_export_gcs_callback(ui, db, source);
}

/// Reads `entry_id`'s full record, with every attachment's bytes, on a worker over a
/// read-only connection. Then it runs `then` on the UI thread, or toasts "not found".
fn read_full_record_then(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    entry_id: i64,
    then: impl FnOnce(&MainWindow, pleochroma_vault::model::entry::FullDiagramRecord) + Send + 'static,
) {
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let full =
            crate::gui::library::read_access::with_read_db(&db, |db| db.get_diagram_full(entry_id));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| match full {
            Ok(Some(full)) => then(&ui, full),
            _ => show_toast(&ui, "Diagram detail not found.", "error"),
        });
    });
}

/// The body of "Export .asc" once the record is in hand: picks the default file name,
/// asks about concave tiers if the reconstruction would lose them, then opens the
/// Save As dialog.
fn start_asc_export(
    ui: &MainWindow,
    db_export: &Arc<Mutex<Database>>,
    entry_id: i32,
    full: pleochroma_vault::model::entry::FullDiagramRecord,
) {
    // Every branch writes to a user-chosen destination. One Save As dialog serves them all,
    // and it opens before any work starts, so cancelling writes nothing.
    let existing_name = full
        .attached_files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with(".asc"))
        .map(|f| f.name.clone());
    let default_file_name = existing_name.clone().unwrap_or_else(|| {
        // A `.gem`/`.gcs`-only design exports its converted cutting
        // instructions under `<stem>.asc` (see `finish_export_asc`).
        local::design_attachment_position(full.attached_files.iter().map(|f| f.name.as_str()))
            .map_or_else(
                || format!("{}.asc", sanitize_filename(&full.title)),
                |(position, _)| local::converted_asc_file_name(&full.attached_files[position].name),
            )
    });

    // The picker runs off the UI thread. The export itself runs in its continuation.
    let default_dir = std::path::Path::new("exports");
    let db_for_pick = Arc::clone(db_export);
    // Only the reconstruction loses concave tiers. A stored `.asc` already holds them as
    // footnotes, and a `.gem` or `.gcs` cannot hold one.
    let concave = (existing_name.is_none()
        && local::design_attachment_position(full.attached_files.iter().map(|f| f.name.as_str()))
            .is_none())
    .then(|| concave_export_notice(&full))
    .flatten();
    // The frosting is lost on every path: no `.asc` holds it, a stored one included.
    let notice = loss_notice(concave, frosting_export_notice(&full));
    confirm_loss_then(ui, notice, move |ui| {
        crate::gui::pickers::pick(
            ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::SaveFile,
                title: None,
                filters: vec![crate::gui::pickers::PickerFilter {
                    label: ".asc Design".to_string(),
                    extensions: vec!["asc".to_string()],
                }],
                default_file_name: Some(default_file_name),
                starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
            },
            move |ui, dest_path| {
                finish_export_asc(ui, &db_for_pick, entry_id, &full, existing_name, dest_path);
            },
        );
    });
}

/// "Export as Gem Cut Studio (.gcs)..." (experimental) for the selected design. It is
/// registered by [`setup_export_asc_callback`] and refuses a remote library. The file comes
/// from [`gcs_export_for_record`]. A design that cannot be built is reported before any
/// picker opens.
fn setup_export_gcs_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_export = Arc::clone(db);
    let source_export = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_export_gcs(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if source_export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_remote()
        {
            show_toast(
                &ui,
                "Switch to the local library to export a .gcs file.",
                "error",
            );
            return;
        }
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            show_toast(&ui, "No diagram selected for export.", "error");
            return;
        }
        // The record is read on a worker; the dialogs continue from the event loop.
        read_full_record_then(&ui, &db_export, i64::from(entry_id), |ui, full| {
            start_gcs_export(ui, &full);
        });
    });
}

/// The body of "Export as Gem Cut Studio (.gcs)..." once the record is in hand: builds
/// the file, asks about concave tiers if the rebuild would lose them, then opens the
/// Save As dialog.
fn start_gcs_export(ui: &MainWindow, full: &pleochroma_vault::model::entry::FullDiagramRecord) {
    let export = match gcs_export_for_record(full) {
        Ok(export) => export,
        Err(message) => {
            show_toast(ui, &message, "error");
            return;
        }
    };
    let default_dir = std::path::Path::new("exports");
    // An original `.gcs` is written byte for byte. A rebuilt one has no place for concave
    // tiers. A `.gcs` keeps the frosting, so only the concave tiers can be lost.
    let notice = loss_notice(
        (!export.is_original)
            .then(|| concave_export_notice(full))
            .flatten(),
        None,
    );
    confirm_loss_then(ui, notice, move |ui| {
        crate::gui::pickers::pick(
            ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::SaveFile,
                title: None,
                filters: vec![crate::gui::pickers::PickerFilter {
                    label: "Gem Cut Studio Design (.gcs)".to_string(),
                    extensions: vec!["gcs".to_string()],
                }],
                default_file_name: Some(export.file_name.clone()),
                starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
            },
            move |ui, dest_path| finish_export_gcs(ui, &export, dest_path),
        );
    });
}

/// What the catalogue's `.gcs` export writes for one design.
#[derive(Debug)]
struct GcsExport {
    /// The file name the save dialog proposes.
    file_name: String,
    /// The file's bytes.
    bytes: Vec<u8>,
    /// `true` when `bytes` is the design's own original `.gcs` attachment,
    /// `false` when it was written from the attached `.asc`.
    is_original: bool,
}

/// The `.gcs` file for a design. If the design was imported from a `.gcs`, its original
/// bytes are used. Otherwise the file is written from the design-file attachment
/// ([`design_attachment_as_asc`]), with the frosting the library stored for it.
///
/// # Errors
///
/// A toast-ready message when the design has no design-file attachment (the
/// angle-table reconstruction has no mast distances, and a `.gcs` needs the
/// solid), when that attachment does not read or parse, or when the writer
/// refuses the schedule.
fn gcs_export_for_record(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Result<GcsExport, String> {
    if let Some(original) = full.attached_files.iter().find(|f| {
        local::DesignFileKind::from_file_name(&f.name) == Some(local::DesignFileKind::Gcs)
    }) {
        return Ok(GcsExport {
            file_name: original.name.clone(),
            bytes: original.content.clone(),
            is_original: true,
        });
    }
    let file = design_attachment_as_asc(full).unwrap_or_else(|| {
        Err(
            "This design has no attached .asc or .gem file, so its mast distances are unknown \
             and no .gcs can be built from it. Load it in the editor, solve it, and use Export \
             as Gem Cut Studio (.gcs) there."
                .to_string(),
        )
    })?;
    let schedule = pleochroma_formats::asc::parse_asc(&file.asc_text).map_err(|e| {
        format!(
            "The attached '{}' could not be read: {e}",
            file.asc_file_name
        )
    })?;
    // `.asc` cannot state frosting, so the stored rows supply it. An empty list writes the
    // same file as `to_gcs_string`.
    let frosting = pleochroma_vault::model::frosting::StoredTierFrosting::to_gcs(&full.frosting);
    let gcs = pleochroma_formats::gcs::to_gcs_string_with_frosting(&schedule, &frosting)
        .map_err(|e| format!("Cannot write this design as .gcs: {e}"))?;
    Ok(GcsExport {
        file_name: std::path::Path::new(&file.asc_file_name)
            .with_extension("gcs")
            .to_string_lossy()
            .into_owned(),
        bytes: gcs.into_bytes(),
        is_original: false,
    })
}

/// `full`'s design-file attachment, chosen by
/// `pleochroma_vault::local::design_attachment_position` (the first `.asc`, else `.gem`, else
/// `.gcs`). It is read as `.asc` text inside
/// [`super::catch_file_panic`].
///
/// `None` when the record has no design-file attachment; `Some(Err)` (a ready
/// message naming the attachment) when a `.gem`/`.gcs` does not read.
fn design_attachment_as_asc(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<Result<local::DesignFileText, String>> {
    let (position, kind) =
        local::design_attachment_position(full.attached_files.iter().map(|f| f.name.as_str()))?;
    let attached = &full.attached_files[position];
    let read = super::catch_file_panic(std::panic::AssertUnwindSafe(|| {
        local::design_file_to_asc_text(&attached.name, kind, &attached.content)
    }));
    Some(match read {
        Ok(result) => result.map_err(|e| format!("'{}': {e}", attached.name)),
        Err(panic_msg) => Err(format!("'{}': internal error: {panic_msg}", attached.name)),
    })
}

/// [`setup_export_gcs_callback`]'s picker continuation: writes `export` to
/// `dest_path` (`None` is a cancelled picker, reported like the `.asc` export's).
fn finish_export_gcs(ui: &MainWindow, export: &GcsExport, dest_path: Option<std::path::PathBuf>) {
    let Some(dest_path) = dest_path else {
        let msg = "Export cancelled.".to_string();
        ui.global::<LibraryModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "info");
        return;
    };
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dest_path, &export.bytes) {
        Ok(()) => {
            let msg = if export.is_original {
                format!(
                    "Exported the original .gcs file to {}.",
                    dest_path.display()
                )
            } else {
                format!(
                    "Exported {} (experimental Gem Cut Studio file -- check it in Gem Cut \
                     Studio before cutting from it).",
                    dest_path.display()
                )
            };
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// Writes a `.gem`/`.gcs` design's converted cutting instructions
/// ([`design_attachment_as_asc`]) to `dest_path` for "Export .asc", or reports
/// why the attachment could not be read.
fn write_converted_asc(
    ui: &MainWindow,
    converted: Result<local::DesignFileText, String>,
    dest_path: &std::path::Path,
) {
    let file = match converted {
        Ok(file) => file,
        Err(e) => {
            show_toast(
                ui,
                &format!("This diagram's design file could not be read: {e}"),
                "error",
            );
            return;
        }
    };
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(dest_path, file.asc_text) {
        Ok(()) => {
            let msg = format!(
                "Exported the cutting instructions converted from this design's .gem/.gcs \
                 file to {}.",
                dest_path.display()
            );
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// The picker continuation of [`setup_export_asc_callback`], split out to keep that function
/// under clippy's length lint. `dest_path` is `None` when the picker was cancelled.
fn finish_export_asc(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    entry_id: i32,
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
    existing_name: Option<String>,
    dest_path: Option<std::path::PathBuf>,
) {
    let Some(dest_path) = dest_path else {
        let msg = "Export cancelled.".to_string();
        ui.global::<LibraryModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "info");
        return;
    };

    if let Some(name) = existing_name {
        // Reading the attachment and writing the file run on a worker.
        let db = Arc::clone(db);
        let ui_weak = ui.as_weak();
        std::thread::spawn(move || {
            let result = crate::gui::library::detail::export_diagram_file(
                &db,
                i64::from(entry_id),
                &name,
                &dest_path,
            );
            let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                Ok(msg) => {
                    ui.global::<LibraryModel>()
                        .set_status_message(msg.clone().into());
                    show_toast(&ui, &msg, "success");
                }
                Err(e) => show_toast(&ui, &e, "error"),
            });
        });
        return;
    }
    // No `.asc` of its own, but a `.gem`/`.gcs`: its converted cutting
    // instructions carry the real masts the angle-table reconstruction lacks.
    if let Some(converted) = design_attachment_as_asc(full) {
        write_converted_asc(ui, converted, &dest_path);
        return;
    }

    let (flat_rows, concave_footnotes) = split_concave_rows(full);
    let schedule = match local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &flat_rows,
    ) {
        Ok(Some(mut schedule)) => {
            schedule.footnotes.extend(concave_footnotes);
            schedule
        }
        Ok(None) => {
            show_toast(
                ui,
                "No cutting-instructions data to export for this diagram.",
                "error",
            );
            return;
        }
        Err(e) => {
            show_toast(
                ui,
                &format!("This diagram's cutting-instructions data could not be read: {e}"),
                "error",
            );
            return;
        }
    };

    let text = match pleochroma_formats::asc::to_asc_string(&schedule) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Cannot write this schedule as .asc: {e}"),
                "error",
            );
            return;
        }
    };
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dest_path, text) {
        Ok(()) => {
            let msg = format!(
                "Exported reconstructed schedule to {} \
                 (mast distances are placeholders -- see the file's RECONSTRUCTED header).",
                dest_path.display()
            );
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// Replaces the characters that Windows disallows in file names, so any design title makes a
/// safe file name.
///
/// This is `pub` rather than `pub(crate)`, as `clippy::redundant_pub_crate` prefers inside a
/// private module. The editor's Export and Save dialogs use the same rule, so a title sanitizes
/// the same way in each.
pub fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "design".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests;
