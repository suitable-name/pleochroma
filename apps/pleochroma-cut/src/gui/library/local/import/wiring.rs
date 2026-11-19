//! Wires the "Import" popup's native pickers to the import pipeline, and runs that
//! pipeline on its own background thread with progress marshalled back to the UI.

use super::{
    confirm::{confirm_collisions_then, count_pending_collisions},
    folder_memory::{
        last_import_directory, remember_import_directory, reset_import_progress,
        spawn_save_folder_rescan,
    },
    pipeline::{ImportOutcome, import_path},
};
use crate::{
    EditorModel, LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{commands, show_toast, tutorial_events::raise},
    settings::SettingsPersister,
};
use pleochroma_editor::guide::viewing_events as events;
use pleochroma_formats::native::design::DESIGN_READ_EXTENSIONS;
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

/// Whether a finished import unfolds the library list, where the imported designs are: at
/// least one design arrived, and the cutter is not at work on a design in the Edit view.
/// Unfolding the list there would push the editor into its narrow layout in the middle of an
/// edit; the toast names what was imported, and the list is one click away.
const fn import_shows_library(imported: usize, editing_a_design: bool) -> bool {
    imported > 0 && !editing_a_design
}

/// Runs [`import_path`] on its own worker thread and marshals UI updates back through
/// `upgrade_in_event_loop`, as `bridge::export_thread::spawn_export` does.
///
/// A `BusyGuard` clears `is_busy` in its `Drop`, so the UI cannot stay busy if this thread
/// panics. A `Drop` guard is used rather than a reset on each return path, so no later
/// `continue`, early `return` or panic can skip it. `super::pipeline::catch_file_panic`
/// inside `import_path` keeps a bad file from panicking this thread at all.
fn spawn_import(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    source: Arc<Mutex<LibrarySource>>,
    path: PathBuf,
    recurse: bool,
) {
    thread::spawn(move || {
        struct BusyGuard(Weak<MainWindow>);
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                let ui_weak = self.0.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<LibraryModel>().set_is_busy(false);
                });
            }
        }
        let _busy_guard = BusyGuard(ui_weak.clone());

        let progress_ui_weak = ui_weak.clone();
        let ImportOutcome {
            summary: message,
            imported_ids,
            had_failures,
            had_collision,
            had_notes,
        } = import_path(&db, &path, recurse, move |done, total| {
            let _ = progress_ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<LibraryModel>()
                    .set_import_done(i32::try_from(done).unwrap_or(i32::MAX));
                ui.global::<LibraryModel>()
                    .set_import_total(i32::try_from(total).unwrap_or(i32::MAX));
                ui.global::<LibraryModel>()
                    .set_import_progress(done as f32 / total as f32);
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Importing {done} / {total}...").into());
            });
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<LibraryModel>()
                .set_import_result_text(message.clone().into());
            ui.global::<LibraryModel>()
                .set_status_message(message.clone().into());
            // The toast kind comes from the failure count, not the message text (see
            // `ImportOutcome::had_failures`).
            let toast_kind = if had_failures { "error" } else { "success" };
            show_toast(&ui, &message, toast_kind);
            // Keeps the popup open while there is something to read: a failure list, a
            // collision warning or a converter note. Auto-closing would turn that list
            // into a toast that disappears. Drives `import_dialog.slint`'s `changed importing`.
            ui.global::<LibraryModel>()
                .set_import_should_stay_open(had_failures || had_collision || had_notes);
            // With more than one imported id, `recent_import_filter` is set to exactly those
            // ids before `refresh_after_library_change`, so that refresh shows only the new
            // rows. `gui::library::search::read_id_filter` drops the filter on the cutter's
            // next real search or filter change. A single id is left to `invoke_select_diagram`.
            let imported_id_items: Vec<i32> = imported_ids
                .iter()
                .filter_map(|&id| i32::try_from(id).ok())
                .collect();
            ui.global::<LibraryModel>().set_id_filter_label("".into());
            if imported_id_items.len() > 1 {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(imported_id_items)));
            } else {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(Vec::<i32>::new())));
            }
            super::super::helpers::refresh_after_library_change(&ui, &db, &source);
            // The new rows are in the library list, which may be folded (a narrow window
            // folds it, and the empty state's "Import a Folder" card starts from there).
            let editing_a_design =
                commands::edit_view_showing(&ui) && ui.global::<EditorModel>().get_has_design();
            if import_shows_library(imported_ids.len(), editing_a_design) {
                commands::show_library_panel(&ui);
            }
            // A single imported design opens at once, so the cutter need not find its row in
            // a long list. `invoke_select_diagram` runs the same path as a click on the row
            // (`setup_diagram_selection_and_export_callbacks` in `diagram_list.rs`).
            if let [only_id] = imported_ids.as_slice()
                && let Ok(id) = i32::try_from(*only_id)
            {
                ui.global::<LibraryModel>().invoke_select_diagram(id);
            }
            // A tutorial step may wait for designs to be imported.
            if !imported_ids.is_empty() {
                raise(&ui, events::LIBRARY_IMPORTED);
            }
            // Offers preview generation for the new designs, with the same confirm dialog as
            // the missing-previews scan. Does nothing when nothing was imported.
            crate::gui::batch::preview::offer_import_previews(&ui, &imported_ids);
        });
        // `_busy_guard` drops at the end of this closure, queuing its `is_busy` reset behind
        // the completion closure above.
    });
}

/// Wires up the "Import" popup's native pickers (`import_dialog.slint`'s "Choose
/// file..." and "Choose folder..." buttons).
///
/// A pick first runs [`count_pending_collisions`]. With no filename collision,
/// [`begin_import`] starts at once, off the UI thread (see [`spawn_import`]). Otherwise
/// [`confirm_collisions_then`] asks a native Yes/No first, and declining leaves the
/// catalogue untouched. Cancelling the picker does nothing and the popup stays open. The
/// folder picker's `recurse` flag is the "Include subfolders" toggle, read when the button
/// is clicked.
///
/// Also calls [`spawn_save_folder_rescan`]. It runs once per session because this
/// function is only called once, from `gui::build_main_window`.
///
/// Always writes to the local database, whichever library is being browsed. `source` is
/// only passed to `super::super::helpers::refresh_after_library_change`, so the library on
/// screen stays the same.
pub fn setup_import_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let db_file = Arc::clone(db);
    let source_file = Arc::clone(source);
    let settings_file = Arc::clone(settings_store);
    let ui_weak_file = ui.as_weak();
    ui.global::<LibraryModel>().on_pick_asc_file(move || {
        let Some(ui) = ui_weak_file.upgrade() else {
            return;
        };
        // The picker runs off the UI thread via `gui::pickers::pick`; its continuation runs
        // the collision pre-scan, the confirm dialog and the import dispatch.
        let db_file = Arc::clone(&db_file);
        let source_file = Arc::clone(&source_file);
        let settings_file = Arc::clone(&settings_file);
        let ui_weak_file = ui_weak_file.clone();
        crate::gui::pickers::pick(
            &ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::OpenFile,
                title: None,
                // `.psdd` is listed first: the scan reads it stand-alone or beside a design.
                filters: vec![crate::gui::pickers::PickerFilter {
                    label: "Faceting Design (.psdd, .asc, .gem, .gcs)".to_string(),
                    extensions: DESIGN_READ_EXTENSIONS
                        .iter()
                        .chain(&["asc", "gem", "gcs"])
                        .map(ToString::to_string)
                        .collect(),
                }],
                default_file_name: None,
                starting_dir: last_import_directory(&settings_file),
            },
            move |ui, path| {
                let Some(path) = path else {
                    return;
                };
                remember_import_directory(&settings_file, path.parent());
                let (collisions, total) = count_pending_collisions(&db_file, &path, false);
                let db_file = Arc::clone(&db_file);
                let source_file = Arc::clone(&source_file);
                confirm_collisions_then(ui, collisions, total, move |ui| {
                    begin_import(ui, &db_file, &source_file, ui_weak_file, path, false);
                });
            },
        );
    });

    let db_folder = Arc::clone(db);
    let source_folder = Arc::clone(source);
    let settings_folder = Arc::clone(settings_store);
    let ui_weak_folder = ui.as_weak();
    // `recurse` is read when "Choose folder..." is clicked: the import may start in this
    // same callback, so the toggle must already be set.
    ui.global::<LibraryModel>()
        .on_pick_asc_folder(move |recurse: bool| {
            let Some(ui) = ui_weak_folder.upgrade() else {
                return;
            };
            // Same off-UI-thread picker as `on_pick_asc_file` above.
            let db_folder = Arc::clone(&db_folder);
            let source_folder = Arc::clone(&source_folder);
            let settings_folder = Arc::clone(&settings_folder);
            let ui_weak_folder = ui_weak_folder.clone();
            crate::gui::pickers::pick(
                &ui,
                crate::gui::pickers::PickerRequest {
                    kind: crate::gui::pickers::PickerKind::PickFolder,
                    title: None,
                    filters: Vec::new(),
                    default_file_name: None,
                    starting_dir: last_import_directory(&settings_folder),
                },
                move |ui, path| {
                    let Some(path) = path else {
                        return;
                    };
                    // Remembers the folder itself, not its parent: the next import is more
                    // likely to come from inside it.
                    remember_import_directory(&settings_folder, Some(path.as_path()));
                    let (collisions, total) = count_pending_collisions(&db_folder, &path, recurse);
                    let db_folder = Arc::clone(&db_folder);
                    let source_folder = Arc::clone(&source_folder);
                    confirm_collisions_then(ui, collisions, total, move |ui| {
                        begin_import(
                            ui,
                            &db_folder,
                            &source_folder,
                            ui_weak_folder,
                            path,
                            recurse,
                        );
                    });
                },
            );
        });

    spawn_save_folder_rescan(ui.as_weak(), Arc::clone(db), Arc::clone(settings_store));
}

/// The actual "start importing now" step -- [`setup_import_callback`]'s pickers call
/// this once the collision confirmation (if any) has cleared.
fn begin_import(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    ui_weak: Weak<MainWindow>,
    path: PathBuf,
    recurse: bool,
) {
    ui.global::<LibraryModel>().set_is_busy(true);
    ui.global::<LibraryModel>()
        .set_status_message("Importing...".into());
    reset_import_progress(ui);
    spawn_import(ui_weak, Arc::clone(db), Arc::clone(source), path, recurse);
}

#[cfg(test)]
mod tests {
    use super::import_shows_library;

    #[test]
    fn imported_designs_unfold_the_library_list() {
        assert!(import_shows_library(1, false));
        assert!(import_shows_library(40, false));
    }

    #[test]
    fn an_import_that_brought_nothing_leaves_the_list_folded() {
        assert!(!import_shows_library(0, false));
    }

    #[test]
    fn a_design_being_edited_keeps_its_layout() {
        assert!(!import_shows_library(3, true));
    }
}
