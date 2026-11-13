//! Remembers where the cutter last imported/saved to, and quietly checks that
//! folder for new `.asc`/`.gem`/`.gcs` files once per session.

use super::confirm::count_pending_collisions;
use crate::{LibraryModel, MainWindow, gui::show_toast, settings::SettingsPersister};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

/// The folder of the most recent native save or open, the first entry of
/// `AppSettings::recent_native_files`. `None` when there is no such entry, or its path has
/// no parent directory.
pub(super) fn last_save_folder(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    settings_store
        .snapshot()
        .settings
        .recent_native_files
        .first()
        .map(PathBuf::from)
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Checks the last save folder once per session for design files not yet in the catalogue.
///
/// If it finds any, it toasts a pointer to the "Choose folder..." picker. It never imports by
/// itself, because filename collisions must never be resolved silently (see
/// `super::confirm::confirm_collisions_then`). It runs off the UI thread, so startup never waits
/// on catalogue I/O.
pub(super) fn spawn_save_folder_rescan(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    settings_store: Arc<SettingsPersister>,
) {
    thread::spawn(move || {
        let Some(folder) = last_save_folder(&settings_store) else {
            return;
        };
        let (collisions, total) = count_pending_collisions(&db, &folder, false);
        let new_files = total - collisions;
        if new_files == 0 {
            return;
        }
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            show_toast(
                &ui,
                &format!(
                    "{new_files} new design file(s) found in your last save folder -- open \
                     Import > Choose Folder to bring {} in.",
                    if new_files == 1 { "it" } else { "them" }
                ),
                "info",
            );
        });
    });
}

/// The folder of the last import, as a picker's starting directory.
///
/// `None` when nothing was imported yet or the folder no longer exists. The existence check is
/// made here, since the file dialog handles a missing directory inconsistently across platforms.
pub(super) fn last_import_directory(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    let remembered = settings_store.snapshot().settings.last_import_directory;
    if remembered.is_empty() {
        return None;
    }
    let path = PathBuf::from(remembered);
    path.is_dir().then_some(path)
}

/// Records where the cutter just imported from, so the next picker opens there.
/// A `None` directory (a path with no parent, which a picked file should never
/// have) leaves the previous value alone rather than clearing it.
pub(super) fn remember_import_directory(
    settings_store: &Arc<SettingsPersister>,
    directory: Option<&Path>,
) {
    let Some(directory) = directory else {
        return;
    };
    let as_string = directory.to_string_lossy().into_owned();
    settings_store.update(|s| s.settings.last_import_directory.clone_from(&as_string));
}

/// Clears the progress readout before a new import, so the last import's finished state
/// does not show until the first progress callback arrives.
pub(super) fn reset_import_progress(ui: &MainWindow) {
    ui.global::<LibraryModel>().set_import_done(0);
    ui.global::<LibraryModel>().set_import_total(0);
    ui.global::<LibraryModel>().set_import_progress(0.0);
}
