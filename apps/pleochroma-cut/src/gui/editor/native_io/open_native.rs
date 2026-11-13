//! The "Open" command: loads a `.psdd` design file, a bare `.asc` or a `.gem`/`.gcs`.
//!
//! [`setup_open_native_callback`] and [`open_recent_native_path`] are the entry points.
//! [`do_open_native`] hands a picked file to the replace-state paths in [`super::open_commit`].

use super::{
    open_commit::{
        DesignFileLoad, ReplaceGuard, open_converted_design, open_design_file, open_plain_asc,
        register_vault,
    },
    open_picker::{PickedNative, pick_design_then, read_picked_path_then},
};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::state::{EditorState, PendingUnsavedAction},
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires the "Open" command and File > Open Recent.
///
/// Open replaces the whole editor state the way "New" and "Load Selected" do: a fresh
/// `History`, and no printed proportions unless the file carries them, since a locally
/// opened file has no catalogue row to verify Deep Solve against.
///
/// The dirty check runs before the file picker opens, as in New and Load Selected: a dirty
/// design stashes [`PendingUnsavedAction::OpenNative`] and shows the guard dialog.
/// `setup_unsaved_guard_dispatch` (in `gui::editor::callbacks::tier_actions::lifecycle`)
/// resumes by calling [`do_open_native`] once Save or Discard is chosen. The picker and the
/// file read are asynchronous and the editor stays interactive, so [`do_open_native`]
/// re-checks for edits made meanwhile ([`ReplaceGuard`]) before anything is replaced.
pub(in crate::gui::editor) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
) {
    // The vault a design file's color pattern is restored into.
    register_vault(db);
    let state_open = Rc::clone(state);
    let render_ctx_open = Arc::clone(render_ctx);
    let preview_state_open = Arc::clone(preview_state);
    let solid_last_solved_open = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_open_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("open_native", || {
            if state_open.borrow().is_dirty() {
                state_open.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::OpenNative);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Opening a file will discard the current design's unsaved changes.".into(),
                );
                ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                return;
            }
            do_open_native(
                &ui,
                &state_open,
                &render_ctx_open,
                &preview_state_open,
                &solid_last_solved_open,
            );
        });
    });

    // File > Open Recent (`MainWindow.open_recent_native_file` in `ui/app.slint`). It is a
    // root-component callback because the recent list lives on `MainWindow`. Opening an
    // entry moves it to the front of the list (see `open_design_file`).
    let state_recent = Rc::clone(state);
    let render_ctx_recent = Arc::clone(render_ctx);
    let preview_state_recent = Arc::clone(preview_state);
    let solid_last_solved_recent = Arc::clone(solid_last_solved);
    let ui_weak_recent = ui.as_weak();
    ui.on_open_recent_native_file(move |path| {
        let Some(ui) = ui_weak_recent.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("open_recent_native_file", || {
            open_recent_native_path(
                &ui,
                &state_recent,
                &render_ctx_recent,
                &preview_state_recent,
                &solid_last_solved_recent,
                PathBuf::from(path.as_str()),
            );
        });
    });
}

/// File > Open Recent's click handler, and the startup path for "reopen last design", the
/// leftover-autosave offer and a design given on the command line. Loads `native_path`
/// without a file picker, through the same read as [`do_open_native`] (a `.psdd`, `.asc`
/// or `.gem`/`.gcs`).
///
/// A dirty design is refused with a toast. Unlike [`setup_open_native_callback`], this does
/// not resume through the Save/Discard dialog: [`PendingUnsavedAction`] has no variant that
/// carries a path to resume at. Refusing loses nothing.
pub(in crate::gui::editor) fn open_recent_native_path(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    native_path: PathBuf,
) {
    if state.borrow().is_dirty() {
        show_toast(
            ui,
            "Save or discard the current design's unsaved changes before opening a recent file.",
            "error",
        );
        return;
    }
    let guard = ReplaceGuard::capture(&state.borrow(), false);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    read_picked_path_then(ui, native_path, move |ui, picked| {
        // The read is asynchronous: re-check for edits made since the dirty check
        // above before anything is replaced.
        dispatch_picked(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            guard,
            picked,
        );
    });
}

/// Commits whatever the Open picker (or Open Recent, or the command line) read: re-checks
/// for edits made since the unsaved-changes decision ([`ReplaceGuard`]), then hands
/// the picked design to the replace-state path for its kind. `None` (a cancelled
/// picker or a read that already toasted its failure) does nothing.
fn dispatch_picked(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    guard: ReplaceGuard,
    picked: Option<PickedNative>,
) {
    match picked {
        Some(_) if !guard.allows_replace(ui, state) => {}
        Some(PickedNative::Design {
            native_path,
            loaded,
        }) => open_design_file(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            DesignFileLoad {
                native_path,
                loaded: *loaded,
            },
        ),
        Some(PickedNative::AscOnly { asc_path, asc_text }) => open_plain_asc(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            &asc_path,
            &asc_text,
        ),
        Some(PickedNative::Converted(converted)) => open_converted_design(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            converted,
        ),
        None => {}
    }
}

/// Shows the Open picker and dispatches the file the cutter picks (see [`dispatch_picked`]) to
/// [`open_design_file`], [`open_plain_asc`] or [`open_converted_design`]. The unsaved-changes
/// guard runs before this, not inside it (see [`setup_open_native_callback`]).
pub(in crate::gui::editor) fn do_open_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
) {
    // Captured now, after the dirty check (or the Save/Discard answer that resumed this call).
    // The picker is asynchronous and the editor stays interactive while it is open.
    let guard = ReplaceGuard::capture(&state.borrow(), true);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    pick_design_then(ui, move |ui, picked| {
        dispatch_picked(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            guard,
            picked,
        );
    });
}
