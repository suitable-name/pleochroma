//! The filename-collision confirmation flow.
//!
//! It pre-scans what an import would replace, and shows a Yes/No prompt when it would replace
//! anything.

use super::scan::collect_import_candidates;
use crate::MainWindow;
use pleochroma_vault::db::sqlite::Database;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// Pre-scans, by file name only, what `super::pipeline::import_path` would import from `path`.
///
/// Returns how many candidates already share a name with a catalogue row, then the candidate
/// total. It uses the same `local://<file name>` test as the save step, so no file is read.
///
/// `(0, 0)` for an unreadable or empty folder. The import reports that failure itself.
pub(super) fn count_pending_collisions(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
) -> (usize, usize) {
    let candidates = collect_import_candidates(path, recurse).unwrap_or_default();
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let collisions = candidates
        .iter()
        .filter(|p| {
            let file_name = p.file_name().map_or_else(
                || "unknown.asc".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            // A failed lookup counts as a collision: the safe side is to ask.
            db.has_detail_for_entry_url(&format!("local://{file_name}"))
                .unwrap_or_else(|e| {
                    tracing::warn!("Import: collision check for '{file_name}' failed: {e}");
                    true
                })
        })
        .count();
    (collisions, candidates.len())
}

/// When `collisions` is non-zero, asks the cutter to confirm the replacement of existing
/// catalogue rows, then runs `proceed` on acceptance. Declining runs nothing, so the
/// catalogue stays untouched. With no collisions, `proceed` runs at once.
///
/// This uses the editor's write-confirm dialog (`native_io::ask_write_confirm`), which is
/// driven by the `EditorModel.write_confirm_*` properties, so no new Slint callback is needed.
pub(super) fn confirm_collisions_then(
    ui: &MainWindow,
    collisions: usize,
    total: usize,
    proceed: impl FnOnce(&MainWindow) + 'static,
) {
    if !collisions_need_confirmation(collisions) {
        proceed(ui);
        return;
    }
    crate::gui::editor::native_io::ask_write_confirm(
        ui,
        "Replace Existing Design(s)?",
        format!(
            "{collisions} of {total} file(s) have the same name as a design already in \
             the catalogue and will REPLACE it (filename-only match, not a content \
             comparison). Continue?"
        ),
        "Continue",
        // Not suppressible: a cutter must not stop being warned before a catalogue row is
        // overwritten.
        None,
        proceed,
    );
}

/// The pure decision in [`confirm_collisions_then`]: any collision needs the dialog. It is
/// separate so it can be tested. The dialog itself needs a live `MainWindow`, which the test
/// environment cannot start.
#[must_use]
pub(super) const fn collisions_need_confirmation(collisions: usize) -> bool {
    collisions > 0
}
