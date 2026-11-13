//! The autosave timer, which writes a recovery snapshot while the design is dirty.
//!
//! The snapshot is written whether or not the design currently solves.
//! [`find_leftover_autosave`] is the startup recovery check.

use super::{
    atomic_write::{temp_sibling, write_synced},
    design_paths::{autosave_file_name, is_autosave_file_name},
    save_helpers::{custom_material_snapshot_for_save, design_file_text},
};
use crate::{
    EditorModel, MainWindow,
    gui::{editor::state::EditorState, show_toast},
    settings::SettingsPersister,
};
use pleochroma_cut_core::native::DesignExtras;
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::warn;

/// Interval of the autosave timer. Two minutes bounds what a crash can lose while staying
/// cheap enough not to hitch a slow disk. A tick writes only when [`EditorState::is_dirty`],
/// and never touches the cutter's own save path (see [`autosave_path`] and
/// [`run_autosave_tick`]).
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);

thread_local! {
    /// Keeps the autosave `slint::Timer` alive: a `Timer` stops when its value is dropped.
    /// Thread-local because Slint's event loop is single-threaded.
    static AUTOSAVE_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };

    /// The recovery file the most recent autosave tick dispatched a write to, until a
    /// save takes it ([`take_last_autosave_path`]). A thread-local for the same reason
    /// [`AUTOSAVE_TIMER`] is: only UI-thread callbacks read or write it.
    static LAST_AUTOSAVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Stops and drops the autosave timer of a previous main window. Its callback holds the
/// window's `Arc<Mutex<Database>>`, which would otherwise keep the library open. See
/// `gui::window_release`.
pub(super) fn release_window_state() {
    let old = AUTOSAVE_TIMER.with(|cell| cell.borrow_mut().take());
    drop(old);
    LAST_AUTOSAVE_PATH.with(|cell| cell.borrow_mut().take());
}

/// Names this module's holder in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(super) fn note_holders(out: &mut Vec<&'static str>) {
    if AUTOSAVE_TIMER.with(|cell| cell.borrow().is_some()) {
        out.push("editor::native_io::autosave AUTOSAVE_TIMER (vault)");
    }
}

/// Takes the path of the recovery file the last autosave tick wrote, if any tick has
/// run since the previous take.
///
/// A landed save deletes this file (via the listener [`setup_autosave_timer`] registers).
/// It is named after the design's name at the time of the tick (`untitled.autosave.psdd`
/// for a design never saved), which can differ from the name the save gives the design,
/// so deleting only the file for the new name would leave it behind.
fn take_last_autosave_path() -> Option<PathBuf> {
    LAST_AUTOSAVE_PATH.with(|cell| cell.borrow_mut().take())
}

/// Starts the autosave timer. Called from [`setup_save_native_callback`], like the other
/// `setup_*` entry points here.
///
/// # Recovery
///
/// The autosave lands at [`autosave_path`]: `<name>.autosave.psdd` in this app's own
/// settings directory (the parent of `settings::store::default_settings_path`), never
/// beside the design's real file. After a crash the startup prompt offers it, or it can
/// be opened with File > Open like any other `.psdd` file.
///
/// The autosave is a complete design file (see [`design_file_text`]) with every tier in
/// full, so restoring it needs nothing else on disk.
pub(super) fn setup_autosave_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
) {
    let state = Rc::clone(state);
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("autosave_tick", || {
            run_autosave_tick(&ui, &state, &db);
        });
    });
    AUTOSAVE_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
    // A landed save makes the recovery file it stood in for obsolete, whatever name the
    // design was autosaved under -- see `take_last_autosave_path`.
    super::on_save_completed(|_ui, _state| {
        if let Some(stale) = take_last_autosave_path() {
            let _ = std::fs::remove_file(stale);
        }
    });
}

/// One autosave timer tick: writes a recovery snapshot iff [`EditorState::is_dirty`].
///
/// The snapshot is a self-contained design file ([`design_file_text`]) holding each tier's
/// `angle_deg` and `indices`. It needs no cached solve, so every dirty tick produces a
/// recoverable file. It carries the custom-material snapshot and the history like a real
/// Save, so a recovered design keeps its material.
///
/// The tick is skipped while a background solve (`EditorModel.solve_running`) runs, so the
/// two never contend for the same locks. The write ([`write_autosave`]) and the encoding of
/// the attachments run on a background thread.
fn run_autosave_tick(ui: &MainWindow, state: &Rc<RefCell<EditorState>>, db: &Arc<Mutex<Database>>) {
    if ui.global::<EditorModel>().get_solve_running() {
        return;
    }
    let (design, asc_filename, printed_proportions, history_entries, file_extras) = {
        let st = state.borrow();
        if !st.is_dirty() {
            return;
        }
        (
            st.design.clone(),
            st.asc_filename.clone(),
            st.printed_proportions,
            st.history.description_log().to_vec(),
            st.file_extras.clone(),
        )
    };
    let custom_material = custom_material_snapshot_for_save(&design, db);
    let path = autosave_path(asc_filename.as_deref());
    // Remembered so the save that eventually lands can delete THIS file even though the
    // design may have a different name by then -- see `take_last_autosave_path`.
    LAST_AUTOSAVE_PATH.with(|cell| *cell.borrow_mut() = Some(path.clone()));
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        // Serialised here, off the UI thread: the design's attachments (PDFs, images)
        // are encoded and hashed, which is the one expensive part of the snapshot.
        let extras = DesignExtras {
            custom_material: custom_material.as_ref(),
            history_entries: &history_entries,
            metadata: Some(&file_extras.metadata),
            attachments: &file_extras.attachments,
        };
        // Serialization is unreachable in practice for a design that was read from a
        // valid file (see `design_file_text`'s own doc comment) and there is nothing
        // actionable to toast for a silent background autosave.
        let Ok(native_toml) =
            design_file_text(&design, printed_proportions.as_ref(), &extras, false)
        else {
            return;
        };
        let result = write_autosave(&path, &native_toml);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Err(e) = result {
                show_toast(
                    &ui,
                    &format!("Autosave to {} failed: {e}", path.display()),
                    "error",
                );
            }
        });
    });
}

/// The settings directory autosaves live in, falling back to the OS temp directory on
/// the rare platform where even that can't be resolved -- either way, never a
/// location the cutter chose or the design's own files live at.
fn autosave_dir() -> PathBuf {
    crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
}

/// Where [`run_autosave_tick`] writes: `<name>.autosave.psdd` inside this app's
/// own settings directory.
pub(super) fn autosave_path(asc_filename: Option<&str>) -> PathBuf {
    autosave_dir().join(autosave_file_name(asc_filename))
}

/// Removes the recovery file for the design recorded as `asc_filename`
/// (`<name>.autosave.psdd`). Errors are ignored: a missing file is the ordinary case.
pub(super) fn delete_autosaves_for(asc_filename: Option<&str>) {
    let _ = std::fs::remove_file(autosave_dir().join(autosave_file_name(asc_filename)));
}

/// The startup check for a leftover autosave, called by the editor's startup sequence
/// before [`EditorState::fresh`] replaces the previous session's state.
///
/// Returns the most recently modified `*.autosave.psdd` in [`autosave_path`]'s directory.
/// It opens and deletes nothing: a leftover only shows that some earlier session was dirty
/// when it stopped, not that its design is still wanted. The caller offers it and removes
/// it afterward.
#[must_use]
pub(in crate::gui::editor) fn find_leftover_autosave() -> Option<PathBuf> {
    // Every leftover is listed so the newest can be picked here, while the prompt's
    // "Delete" action (`delete_leftover_autosave`) removes only the one file offered.
    find_all_leftover_autosaves()
        .into_iter()
        .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Every autosave file in [`find_leftover_autosave`]'s directory, before the newest is
/// picked. Older leftovers are never opened or removed here: they stay until their design
/// is saved again (which deletes its autosave) or the cutter removes them by hand.
fn find_all_leftover_autosaves() -> Vec<PathBuf> {
    std::fs::read_dir(autosave_dir())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_autosave_file_name)
        })
        .collect()
}

/// The startup prompt's "Delete" action: removes only the leftover file that was offered,
/// never another one from [`find_all_leftover_autosaves`]. Errors are ignored: a failed
/// delete only means the file is offered again on the next launch.
pub(in crate::gui::editor) fn delete_leftover_autosave(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Writes `native_toml` to `path` with the stage-then-rename steps of
/// [`write_file_atomically`], so a crash mid-write leaves no partial recovery file. The
/// staged file is flushed before the rename, so a power loss cannot leave an empty file.
///
/// # Errors
///
/// A ready-to-toast message.
pub(super) fn write_autosave(path: &Path, native_toml: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create recovery directory {} for {}: {e}",
                parent.display(),
                path.display()
            )
        })?;
    }
    let tmp = temp_sibling(path);
    if let Err(error) = write_synced(&tmp, native_toml) {
        let cleanup = std::fs::remove_file(&tmp)
            .err()
            .filter(|cleanup_error| cleanup_error.kind() != std::io::ErrorKind::NotFound);
        let cleanup_context = cleanup
            .map(|cleanup_error| format!("; also failed to remove staged file: {cleanup_error}"))
            .unwrap_or_default();
        return Err(format!(
            "Failed to write and sync staged recovery file {} for {}: {error}{cleanup_context}",
            tmp.display(),
            path.display()
        ));
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let cleanup = std::fs::remove_file(&tmp)
            .err()
            .filter(|cleanup_error| cleanup_error.kind() != std::io::ErrorKind::NotFound);
        let cleanup_context = cleanup
            .map(|cleanup_error| format!("; also failed to remove staged file: {cleanup_error}"))
            .unwrap_or_default();
        return Err(format!(
            "Failed to publish staged recovery file {} as {}: {error}{cleanup_context}",
            tmp.display(),
            path.display()
        ));
    }
    Ok(())
}

/// Records `native_path_display` as the most recently used design file and refreshes
/// File > Open Recent. Called after every successful Save and Open (through
/// [`open_design_file`]), never for [`open_plain_asc`]'s bare `.asc` path.
///
/// The entry goes through the application's [`SettingsPersister`], not the settings file:
/// every close path flushes the persister's snapshot over the file, so a direct write
/// would be erased on exit. With no persister installed nothing is recorded.
pub(super) fn record_recent_native_file(ui: &MainWindow, native_path_display: &str) {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!(
            "No settings persister is installed; not recording {native_path_display} as a recent file"
        );
        return;
    };
    persister.update(|file| {
        file.settings
            .record_recent_native_file(native_path_display.to_string());
    });
    show_recent_native_files(ui, &persister);
}

/// The startup offer's Discard: removes `native_path_display` from the recent design files
/// ([`crate::settings::model::AppSettings::forget_recent_native_file`]), through the same
/// [`SettingsPersister`] as [`record_recent_native_file`], and refreshes File > Open Recent. The
/// file itself is not touched.
pub(in crate::gui::editor) fn forget_recent_native_file(
    ui: &MainWindow,
    native_path_display: &str,
) {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!(
            "No settings persister is installed; not removing {native_path_display} from the recent files"
        );
        return;
    };
    persister.update(|file| {
        file.settings.forget_recent_native_file(native_path_display);
    });
    show_recent_native_files(ui, &persister);
}

/// Puts the persister's recent design files into `MainWindow.recent_native_files` (File > Open
/// Recent).
fn show_recent_native_files(ui: &MainWindow, persister: &SettingsPersister) {
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        persister
            .snapshot()
            .settings
            .recent_native_files
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}
