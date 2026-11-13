//! File-based import and export for the Edit tab: the edited `.asc` export and the
//! `.psdd` design file save and open.
//!
//! # Module split
//!
//! Shared infrastructure used by every write, export, autosave and open path:
//! [`solve`] (Group 1: a matching cached solve, else a background solve, never inline;
//! `resolve_solve_at` for generation-tagged writes with a typed failure,
//! `resolve_solved_then` for the plain one), [`confirm`] (Group 2: the in-window
//! write-confirm dialog, no blocking OS dialogs) and [`picker`] (Group 3: file pickers
//! off the UI thread). [`save_helpers`] and [`catalogue`] hold the save-side helpers
//! (custom-material snapshot, degenerate-marker header, design-file text, catalogue
//! write-back), and [`design_paths`] the `.psdd` file-name rules.
//!
//! Each write path has its own module: [`export`] ("Export Edited .asc", the cutting
//! sheet and the diagram), [`save`], [`save_finish`] and [`atomic_write`] (the Save
//! dialog, its success tail and the stage-then-rename disk write), [`autosave`]
//! (Group 4: the two-minute recovery snapshot), and [`open_native`], [`open_picker`]
//! and [`open_commit`] (the Open entry points, file reading and parsing, and committing
//! a loaded design into `EditorState`). Tests live in [`tests`] and the `*_tests`
//! modules.

mod atomic_write;
mod autosave;
mod catalogue;
mod confirm;
mod design_meta;
#[cfg(test)]
mod design_meta_tests;
mod design_paths;
#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod dispersion_model_tests;
mod export;
mod open_commit;
mod open_native;
mod open_picker;
#[cfg(test)]
mod physics_color_tests;
mod picker;
mod save;
mod save_finish;
mod save_helpers;
mod solve;
#[cfg(test)]
mod tests;

use crate::{MainWindow, gui::editor::state::EditorState};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

// Re-exported at the same `pub(in crate::gui)` visibility `AfterSave` has at its definition
// (`state::core`): `gui::window_close` is a sibling of `gui::editor` and cannot name the
// private `state` module itself.
pub(in crate::gui) use crate::gui::editor::state::AfterSave;

thread_local! {
    /// The `.psdd` design file this `EditorState` was last loaded from or saved to, if any.
    /// A quick Save writes there (see [`save::setup_save_native_callback`]; never to a
    /// legacy file, see `design_paths::path_belongs_to_schedule_name`), and
    /// [`atomic_write::write_file_atomically`] keeps a `.bak` of the replaced copy. `None`
    /// for a design from anywhere else (a catalogue row, a bare `.asc`, a recovered
    /// autosave): its first Save asks for a file name. A plain `RefCell`, not an
    /// `EditorState` field, because it names a location on disk and not design data, so
    /// `EditorState::replace_wholesale` must not reset it.
    static CURRENT_NATIVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };

    /// The folder the Save dialog opens in for a design without a design file yet: the
    /// folder a legacy file was opened from, so the new `.psdd` lands beside it. `None`
    /// falls back to `./exports` when that folder exists.
    static SUGGESTED_SAVE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };

    /// The live `EditorState`, stashed once by [`save::setup_save_native_callback`] so
    /// [`request_save_then_close`] can reach it from `gui::window_close`. That module is
    /// a sibling of `gui::editor`, and a plain `Rc` cannot reach it without a new
    /// parameter on the call site outside this module.
    static LIVE_EDITOR_STATE: RefCell<Option<Rc<RefCell<EditorState>>>> = const { RefCell::new(None) };

    /// Listeners called by [`notify_save_completed`] after each successful save, in
    /// registration order.
    static SAVE_COMPLETED_LISTENERS: RefCell<Vec<SaveCompletedListener>> =
        RefCell::new(Vec::new());
}

/// One [`on_save_completed`] listener, boxed so [`SAVE_COMPLETED_LISTENERS`] can hold
/// a heterogeneous list of them.
type SaveCompletedListener = Box<dyn Fn(&MainWindow, &Rc<RefCell<EditorState>>)>;

/// Stashes `state` for [`request_save_then_close`] to reach later.
pub(super) fn remember_editor_state(state: &Rc<RefCell<EditorState>>) {
    LIVE_EDITOR_STATE.with(|cell| *cell.borrow_mut() = Some(Rc::clone(state)));
}

/// Marks [`EditorState::after_save`] as [`AfterSave::CloseWindow`] before `gui::window_close`'s
/// Save handler invokes `EditorModel.save_native`. The save is asynchronous, so the close
/// resumes from the save-completed listener. A no-op until [`remember_editor_state`] has run.
pub(in crate::gui) fn request_save_then_close() {
    LIVE_EDITOR_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            state.borrow_mut().after_save = Some(AfterSave::CloseWindow);
        }
    });
}

/// A copy of the open design, for the Color Pattern window's Current Design preview. `None` when
/// no editor is live, or the editor state is borrowed (the caller shows something else).
pub(in crate::gui) fn live_design() -> Option<pleochroma_cut_core::Design> {
    LIVE_EDITOR_STATE.with(|cell| {
        let cell = cell.borrow();
        let state = cell.as_ref()?;
        let st = state.try_borrow().ok()?;
        Some(st.design.clone())
    })
}

/// A library metadata edit rewrote the row `entry_id`. When that row is the open
/// design's own, the design file's `[meta]` table is now behind the row, so the design
/// is marked as having unsaved changes: its next Save writes the edit into the file
/// (see `design_meta`). Returns `true` when it marked the open design, so the caller
/// can refresh the dirty indicator. A silent no-op for any other row.
///
/// Uses the generation bookkeeping without touching the generation itself: a metadata
/// edit changes no geometry, so it must not make a Deep Solve or Optimize result stale.
pub(in crate::gui) fn mark_design_row_edited(entry_id: i64) -> bool {
    LIVE_EDITOR_STATE.with(|cell| {
        let cell = cell.borrow();
        let Some(state) = cell.as_ref() else {
            return false;
        };
        let Ok(mut st) = state.try_borrow_mut() else {
            return false;
        };
        if !st.has_design || st.source_entry_id != Some(entry_id) {
            return false;
        }
        let behind = st.current_generation().wrapping_sub(1);
        st.saved_generation = behind;
        true
    })
}

/// Registers a listener [`notify_save_completed`] calls after every successful save (draft
/// or real: both write a file).
///
/// Used for [`AfterSave`]: the window-close guard and the unsaved-changes guard each run
/// something once an asynchronous Save lands. Each registers its own listener, closing over
/// the handles it already has, so the write path does not need their parameters.
///
/// A listener that does not recognize the `after_save` value (it belongs to the other guard,
/// or none is pending) must leave it as found. Only one listener may consume a value, so a
/// plain `Option::take` would let the first listener steal the other's value.
pub(in crate::gui) fn on_save_completed(
    listener: impl Fn(&MainWindow, &Rc<RefCell<EditorState>>) + 'static,
) {
    SAVE_COMPLETED_LISTENERS.with(|cell| cell.borrow_mut().push(Box::new(listener)));
}

/// Calls every [`on_save_completed`] listener; called by
/// [`save_finish::finish_save_native_success`].
pub(super) fn notify_save_completed(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    SAVE_COMPLETED_LISTENERS.with(|cell| {
        for listener in &*cell.borrow() {
            listener(ui, state);
        }
    });
}

/// Forgets what a previous main window left in this module's thread-locals: the save-completed
/// listeners (each closes over the old window's handles and would resume a pending action with
/// them) and the remembered design location. Called once at the start of every
/// `setup_editor_callbacks`, before any listener of the new window is registered.
pub(in crate::gui::editor) fn reset_for_new_window() {
    SAVE_COMPLETED_LISTENERS.with(|cell| cell.borrow_mut().clear());
    remember_design_location(None, None);
}

/// [`reset_for_new_window`], and the previous window's editor state is let go too. See
/// `gui::window_release`.
pub(in crate::gui::editor) fn release_window_state() {
    reset_for_new_window();
    autosave::release_window_state();
    confirm::release_window_state();
    open_picker::release_window_state();
    save::release_window_state();
    solve::release_window_state();
    let old = LIVE_EDITOR_STATE.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Names the holders of this module group that are still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(in crate::gui::editor) fn note_holders(out: &mut Vec<&'static str>) {
    autosave::note_holders(out);
    save::note_holders(out);
    solve::note_holders(out);
    if LIVE_EDITOR_STATE.with(|cell| cell.borrow().is_some()) {
        out.push("editor::native_io LIVE_EDITOR_STATE");
    }
    if SAVE_COMPLETED_LISTENERS.with(|cell| !cell.borrow().is_empty()) {
        out.push("editor::native_io SAVE_COMPLETED_LISTENERS (vault)");
    }
}

/// Records where a Save should go next: `design_path` is the design file the
/// design now lives in (`None` when it has none), and `suggested_dir` the folder the
/// Save dialog should open in when there is no such file yet.
pub(super) fn remember_design_location(
    design_path: Option<PathBuf>,
    suggested_dir: Option<PathBuf>,
) {
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = design_path);
    SUGGESTED_SAVE_DIR.with(|cell| *cell.borrow_mut() = suggested_dir);
}

/// The design file this design was last loaded from or saved to, if any; "Edit as Text"
/// compares its text with this file. `None` for a design without a design file yet (see
/// [`CURRENT_NATIVE_PATH`]).
#[must_use]
pub(in crate::gui::editor) fn current_design_file() -> Option<PathBuf> {
    CURRENT_NATIVE_PATH.with(|cell| cell.borrow().clone())
}

// Each re-exported item is declared at this visibility at its definition site:
// `pub(in crate::gui::editor)`, or `pub(in crate::gui)` for `ask_write_confirm`.
pub(in crate::gui::editor) use autosave::{
    delete_leftover_autosave, find_leftover_autosave, forget_recent_native_file,
};
pub(in crate::gui) use confirm::ask_write_confirm;
pub(in crate::gui::editor) use export::{
    setup_export_asc_callback, setup_export_cutting_sheet_callback, setup_export_diagram_callback,
};
pub(in crate::gui::editor) use open_native::{
    do_open_native, open_recent_native_path, setup_open_native_callback,
};
pub(in crate::gui::editor) use save::setup_save_native_callback;
pub(in crate::gui::editor) use solve::{SolveFailure, resolve_solve_at, resolve_solved_then};
