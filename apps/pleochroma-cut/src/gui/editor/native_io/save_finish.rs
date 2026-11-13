//! [`super::save`]'s success tail: records the just-written location, refreshes
//! `EditorState`'s saved-generation bookkeeping, toasts the outcome, and reports the
//! catalogue write-back's own result.

use super::{
    autosave::{delete_autosaves_for, record_recent_native_file},
    catalogue::CatalogueWriteBack,
    remember_design_location,
};
use crate::{
    EditorModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{
        editor::{state::EditorState, view::design_label},
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma_editor::guide::viewing_events as events;
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Records where a Save/Export just wrote, for the status strip's own persistent
/// "Saved: ..." segment and its click-to-reveal. The strip
/// gets the bare file name (all a 26px strip has room for) and the full path
/// separately, for the Details popup and for the reveal itself -- see
/// `EditorModel.last_saved_path`'s own doc comment. A path with no file-name
/// component cannot come out of a save dialog, but is handled anyway by falling
/// back to the full path rather than clearing the label.
pub(super) fn record_last_saved_path(ui: &MainWindow, path: &Path) {
    let full = path.display().to_string();
    let label = path
        .file_name()
        .map_or_else(|| full.clone(), |name| name.to_string_lossy().into_owned());
    let model = ui.global::<EditorModel>();
    model.set_last_saved_path(full.into());
    model.set_last_saved_label(label.into());
}

/// [`write_native_save`]'s background-thread result, after the file write and the catalogue
/// write-back: everything [`finish_save_native_success`] needs on the UI thread. `catalogue`
/// is its own `Result`: a catalogue failure does not mean the save failed (see
/// [`write_back_to_catalogue`]), so it is reported on its own.
pub(super) struct WriteNativeOutcome {
    /// The `.psdd` file that was written.
    pub(super) native_path: PathBuf,
    pub(super) asc_filename: String,
    /// The catalogue row's cutting instructions and the draft verdict.
    pub(super) asc: pleochroma_cut_core::native::CatalogueAsc,
    pub(super) catalogue: Result<(i64, CatalogueWriteBack), String>,
    /// `EditorState::generation` when `design` was cloned out of `state`, before the write
    /// started. The design is marked clean at this value, not at the live generation, which
    /// may already include edits this save never saw.
    pub(super) snapshot_generation: u64,
}

/// [`write_native_save`]'s success tail, run on the UI thread once its background thread
/// reports in. Split out to keep that function under clippy's line-count limit. It drops the
/// autosave (now older than the file on disk), records the file as the recent entry and the
/// design's location, updates the saved generation, toasts the outcome (a draft warning when
/// [`pleochroma_cut_core::native::CatalogueAsc::draft_reason`] is `Some`), and reports the
/// catalogue result.
pub(super) fn finish_save_native_success(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    outcome: WriteNativeOutcome,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let WriteNativeOutcome {
        native_path,
        asc_filename,
        asc,
        catalogue,
        snapshot_generation,
    } = outcome;
    // The autosave is older than the file just written, so it is dropped.
    delete_autosaves_for(Some(asc_filename.as_str()));
    record_recent_native_file(ui, &native_path.display().to_string());
    remember_design_location(Some(native_path.clone()), None);
    // The window title names the file just saved.
    let file_label = native_path.file_name().map_or_else(
        || asc_filename.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    ui.set_loaded_design_name(file_label.into());
    record_last_saved_path(ui, &native_path);
    let schedule_name = asc_filename.clone();

    {
        let mut st = state.borrow_mut();
        st.asc_filename = Some(asc_filename);
        st.original_asc_text = Some(asc.asc_text);
        // A draft is a successful save too, so this runs for drafts as well. Marks clean at
        // `snapshot_generation`, not the live counter: edits made while the save was in flight
        // are still unsaved, and the close guard must still prompt for them.
        st.saved_generation = snapshot_generation;
    }
    // The title names the saved file now, not only after the next edit or solve.
    ui.global::<EditorModel>()
        .set_design_label(design_label(&state.borrow()).into());
    ui.global::<EditorModel>()
        .set_is_dirty(state.borrow().is_dirty());

    // `draft_reason` is `Some` when the design does not solve (or has no tiers) and the `.asc`
    // holds placeholder masts. The file is on disk but is not a real cut instruction, so the
    // toast says so instead of reporting an error. `DraftReason`'s `Display` names the cause.
    if let Some(reason) = &asc.draft_reason {
        // A persistent warning, so the placeholder state is not missed.
        show_toast(
            ui,
            &format!(
                "Saved '{}' as a draft ({reason}). Add a scale-reference tier to \
                 finish it.",
                native_path.display()
            ),
            "warning",
        );
    } else {
        show_toast(
            ui,
            &format!("Saved '{}'.", native_path.display()),
            "success",
        );
    }
    // A tutorial step may wait for the design to be saved (a draft counts: the file is there).
    raise(ui, events::DESIGN_SAVED);

    // The catalogue row was written on a background thread by `write_native_save`
    // (see `write_back_to_catalogue`); this reports its outcome. Export Edited .asc never
    // writes a row.
    report_catalogue_write_back(ui, state, db, source, Path::new(&schedule_name), catalogue);

    // Runs last, so a listener that hides the window (the close-confirm guard) does so only
    // after the rest of a successful save.
    super::notify_save_completed(ui, state);
}

/// Reports [`write_back_to_catalogue`]'s result on the UI thread: adopts the written
/// row as the design's `source_entry_id`, refreshes the library list, and toasts the
/// outcomes the cutter should not miss. `dest_path` names the design's `.asc` entry in
/// the catalogue, for the collision wording.
fn report_catalogue_write_back(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    dest_path: &Path,
    catalogue: Result<(i64, CatalogueWriteBack), String>,
) {
    match catalogue {
        // Another design already owns this file name, so the catalogue is left untouched. The
        // id is that design's, and adopting it would make the next save overwrite it.
        Ok((_, CatalogueWriteBack::UrlCollision { existing_id, title })) => {
            let file = dest_path.file_name().map_or_else(
                || dest_path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            show_toast(
                ui,
                &format!(
                    "Saved to disk; the catalogue already holds a design named '{file}' \
                     (id {existing_id}, \"{title}\"), catalogue not updated. Import it \
                     explicitly to replace."
                ),
                "warning",
            );
        }
        Ok((id, catalogue_outcome)) => {
            // Later saves update this row and never insert another (see
            // `EditorState::source_entry_id`).
            state.borrow_mut().source_entry_id = Some(id);
            crate::gui::library::local::refresh_after_library_change(ui, db, source);
            match catalogue_outcome {
                CatalogueWriteBack::SourceRowGoneNewRowCreated => {
                    show_toast(
                        ui,
                        "This design's catalogue row no longer exists (it may have been \
                         deleted) -- saved as a new catalogue entry instead of updating it.",
                        "info",
                    );
                }
                // The design saved; only the library's cached previews and tilt curves
                // could not be cleared. The cutter is told, not only logged.
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: true,
                } => {
                    show_toast(
                        ui,
                        "Saved, but this design's cached preview image and tilt curves in \
                         the library could not be cleared automatically -- they may still \
                         show the geometry from before this save. Re-export or recompute \
                         them if they look wrong.",
                        "warning",
                    );
                }
                // `UrlCollision` is reported by the arm above and never reaches here.
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: false,
                }
                | CatalogueWriteBack::NewRow
                | CatalogueWriteBack::UrlCollision { .. } => {}
            }
        }
        Err(message) => show_toast(ui, &format!("Catalogue not updated: {message}"), "error"),
    }
}
