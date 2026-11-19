//! The design-lifecycle callbacks: Solve, New Design, Load Selected, and the
//! Save/Discard/Cancel unsaved-changes guard they share with Open.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

use pleochroma_cut_core::History;
use pleochroma_editor::{EditorSession, guide::solving_events::SOLVE_REQUESTED};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;

use super::{misc::bump_form_reset_pulse, new_design::do_new_design_create, planned_stone};
use crate::{
    EditorModel, LibraryModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        editor::{
            callbacks::solve_actions::clear_analysis_results,
            landing, loading,
            material_lookup::{EditorMaterialLookup, nearest_built_in_material},
            native_io::{self, AfterSave, do_open_native},
            stall_guard::stall_guard,
            state::{
                ANGLE_NUDGE_COALESCE_WINDOW, DesignFileExtras, EditorState, MaterialComboCache,
                PendingUnsavedAction, PushedScratch,
            },
            view::{SolidLastSolved, push_has_design, refresh_all_now},
        },
        library::remote::fetch_remote_design_source,
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
        tutorial_events::raise,
    },
};

/// "Solve": the explicit re-solve action. It calls `refresh_all`, a real `Design::solve` that can
/// take seconds, rather than [`refresh_editor_panel_stale`].
///
/// Refuses to run while Deep Solve or Optimize is running, not only during another Solve. Solve,
/// Deep Solve and Optimize each guard only their own `*_running` flag, so any two could run at
/// once. The three flags are read directly; `EditorModel.busy_action` is derived from them.
pub(in crate::gui::editor) fn setup_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_solve(move || {
        stall_guard("on_solve", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<EditorModel>();
            if model.get_solve_running()
                || model.get_deep_solve_running()
                || model.get_optimize_running()
            {
                return;
            }
            refresh_all_now(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &state,
                false,
            );
            // Tells the tutorial guide that a solve has landed.
            raise(&ui, SOLVE_REQUESTED);
        });
    });
}

thread_local! {
    /// The live `EditorState`, for the remote branch of [`setup_load_selected_callback`]. The
    /// download's completion callback must be `Send`, and an `Rc` is not, so the callback reads
    /// the state from here on the UI thread instead of capturing it. Set once, when that
    /// callback is first registered; there is only ever one `EditorState`.
    static REMOTE_LOAD_TARGET: RefCell<Option<Rc<RefCell<EditorState>>>> = const { RefCell::new(None) };
}

/// The per-load payload of [`apply_loaded_design`], bundled to keep its argument count within
/// clippy's limit.
struct LoadedDesignOutcome<'a> {
    loaded: loading::LoadedDesign,
    printed_proportions: Option<pleochroma::geometry::stone_metrics::ExternalProportions>,
    /// The library entry's persisted material (`diagram_previews.preview_material`), tried
    /// by [`apply_loaded_design`] when the design itself names none; `None` for a remote
    /// load, which has none to read.
    preview_material: Option<String>,
    /// What the success toast calls the design -- see [`apply_loaded_design`]'s own
    /// doc comment.
    label: &'a str,
    /// The catalogue row this design was loaded from: `Some` for a local load, `None` for a
    /// remote one. A remote entry id is a separate id space and never lands here (see
    /// `EditorState::source_entry_id`).
    source_entry_id: Option<i64>,
    /// The catalogue entry's `url` for a local load, `None` for a remote one. A design whose file
    /// has no `[meta].id` takes the UUID derived from this `url`, so reopening the entry finds its
    /// saved variants. A remote design gets a fresh UUID: its `.asc` file name is not stable.
    catalogue_url: Option<&'a str>,
}

/// The shared tail of [`setup_load_selected_callback`]'s local and remote branches:
/// replaces `state` wholesale with `loaded`'s design (fresh `History`, every other
/// per-design transient field cleared -- the same wholesale replacement
/// `setup_new_design_create_callback` does for "New"), pushes a real solve into the
/// panel/viewport via `refresh_all`, shows the "loaded"/"placeholder" toast, and
/// offers the catalogue material-suggestion banner built from `loaded.design`'s own
/// schedule RI.
///
/// `loaded_label` names what the success toast calls the design: the local branch's
/// own catalogue title, or the remote branch's bare `.asc` file name -- a remote
/// fetch (`gui::library::remote::RemoteDesignSource`) never carries the catalogue
/// title alongside its `.asc` text, only the attachment's own name.
#[expect(
    clippy::too_many_lines,
    reason = "a single ordered state transition; steps share many locals"
)]
fn apply_loaded_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    outcome: LoadedDesignOutcome<'_>,
) {
    let LoadedDesignOutcome {
        mut loaded,
        printed_proportions,
        preview_material,
        label: loaded_label,
        source_entry_id,
        catalogue_url,
    } = outcome;
    let schedule_ri = loaded.design.meta.refractive_index;
    // The design carries the material the library shows for its entry, so the Stone tab
    // is only needed to CHANGE it. Set on the design BEFORE the session is built: no undo
    // step, and the fresh session is clean (not an unsaved change). A design file that
    // names a material keeps its own (`apply_library_material` leaves it alone).
    let applied_library_material = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pleochroma_editor::loading::apply_library_material(
            &mut loaded.design,
            preview_material.as_deref(),
            &EditorMaterialLookup::new(&ctx.custom_materials),
        )
    };
    let design_names_material = loaded.design.material.name.is_some();
    // A color pattern this build cannot apply renders as the plain material; the load toast says so.
    let (pattern_note, pattern_problem) =
        pleochroma_editor::material_lookup::with_pattern_notice("", &loaded.design);
    let mut st = state.borrow_mut();
    // `replace_wholesale` keeps the state's `generation` counter (and bumps it), so a
    // background solve started against the replaced design still sees the change.
    st.replace_wholesale(EditorState {
        used_placeholder: loaded.used_placeholder,
        // The design file's `[meta]` and attachments (empty for a bare `.asc`), kept so
        // the next Save writes them back unchanged.
        file_extras: DesignFileExtras::new(loaded.metadata, loaded.attachments)
            .with_design_uuid_assigned(catalogue_url),
        source_entry_id,
        // Same coalescing window as `EditorState::fresh`.
        session: EditorSession::with_history(
            loaded.design,
            History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        ),
        printed_proportions,
        design_epoch: Arc::new(AtomicU64::new(0)),
        pending_unsaved_action: None,
        after_save: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        deep_solve_result_generation: None,
        asc_filename: loaded.asc_filename,
        original_asc_text: loaded.original_asc_text,
        pending_gear_remap: None,
        pending_retarget: None,
        // A replaced state has pushed nothing to the panel yet.
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        // Likewise, no material combo options are cached yet.
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        applied_library_material,
        has_design: true,
    });
    // Analysis results of the replaced design no longer describe the screen; see
    // `clear_analysis_results`.
    clear_analysis_results(ui);
    // Captured before dropping the borrow below -- `refresh_all_now` needs to
    // borrow `state` itself, so `st` cannot still be held across that call.
    let loaded_asc_filename = st.asc_filename.clone();
    drop(st);
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
    // The `EditorState` above was replaced, so a selected tier index names at best an
    // unrelated row. Both halves of the selection reset: the property (for the overlay tint)
    // and the pulse (so `EditorView`'s form blanks even when the index was already `-1`).
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    // A freshly replaced state is clean by construction.
    ui.global::<EditorModel>().set_is_dirty(false);
    // A loaded design is a real design: the empty-state card grid gives way to it.
    push_has_design(ui, &state.borrow());
    // The window title names whichever design is open -- `native_io` sets this on
    // every Save/Open, and this is the matching Load Selected path.
    ui.set_loaded_design_name(loaded_asc_filename.unwrap_or_default().into());
    if loaded.used_placeholder {
        show_toast(
            ui,
            &format!(
                "Loaded a reconstructed schedule -- mast distances are \
                 placeholders (no attached .asc file was found); adjust \
                 masts before exporting.{pattern_note}"
            ),
            // Every mast on this design is a fabricated 0.0. A cutter who
            // misses that can export a file that looks like a real cut
            // instruction and is not, so this must not auto-dismiss the way an
            // "info" toast does.
            "warning",
        );
    } else {
        show_toast(
            ui,
            &format!("Loaded '{loaded_label}' into the editor.{pattern_note}"),
            if pattern_problem {
                "warning"
            } else {
                "success"
            },
        );
    }
    // Suggests the built-in material whose n_D is nearest the schedule RI (within
    // 0.01) for a design that still names none after the load (the library gave it
    // nothing to apply): only offered as a banner the user can dismiss or accept. A design
    // that carries a material has nothing left to suggest.
    if let Some((name, ri)) =
        nearest_built_in_material(schedule_ri, 0.01).filter(|_| !design_names_material)
    {
        // Built before `name` moves into `set_material_suggestion_name` below.
        let suggestion_text = format!("Set material to {name} (RI {ri:.4})?");
        ui.global::<EditorModel>()
            .set_material_suggestion_name(name.into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text(suggestion_text.into());
    } else {
        ui.global::<EditorModel>()
            .set_material_suggestion_name("".into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text("".into());
    }
    // The design that landed is worked on in the Edit view, so the cutter is taken there from
    // whichever page the load was started on, also when the load took a while and the page
    // changed meanwhile. A design a guide launched leaves the page to the guide. The view
    // changes after the refresh above, which may wait one turn of the event loop (see
    // `landing`). Noted before a planned stone is applied below, which is an edit of its own.
    let generation = state.borrow().current_generation();
    landing::design_landed(
        ui,
        generation,
        landing::Landing::ShowEditView,
        landing::Refresh::MayBeDeferred,
    );
    // A stone the Rough Planner asked to open (`planned_stone`) is applied as one undoable edit
    // when the design it was computed for is the one that landed. Any other load drops it.
    if let Some(request) = planned_stone::take_for(source_entry_id) {
        planned_stone::apply_pending(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            &request,
        );
    }
}

/// "Load Selected": replaces the editor state with the currently selected catalogue
/// design. Local library: reads the full record straight from `db` and prefers a
/// real attached `.asc` (see `loading::design_from_full_record`). Remote library:
/// fetches that entry's own `.asc` text off the UI thread
/// (`gui::library::remote::fetch_remote_design_source`) and builds the identical
/// `Design` from it via `loading::design_from_asc_text` -- there is no printed-
/// proportions/placeholder-reconstruction data available over that wire, so a
/// remote load never offers Deep Solve's external verification and never falls back
/// to the angle-table placeholder (a remote entry with no attached `.asc` reports
/// `DesignSourceNotAvailable`, surfaced as a toast instead).
///
/// Checks `entry_id` first, then [`EditorState::is_dirty`]. A dirty design stashes
/// [`PendingUnsavedAction::LoadSelected`] and opens the Save/Discard/Cancel guard instead of
/// replacing anything. [`setup_unsaved_guard_dispatch`], registered here, resumes the load once
/// "Save" or "Discard" is chosen. Everything past that point is [`do_load_selected`], run either
/// directly or as that resume.
pub(in crate::gui::editor) fn setup_load_selected_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let state = Rc::clone(state);
    REMOTE_LOAD_TARGET.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&state)));
    setup_unsaved_guard_dispatch(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
        db,
        source,
    );
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let db = Arc::clone(db);
    let source = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_load_selected(move || {
        stall_guard("on_load_selected", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // A load the person starts outlives no stone the planner left behind.
            planned_stone::load_selected_invoked();
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                show_toast(&ui, "No diagram selected to load.", "error");
                return;
            }

            if state.borrow().is_dirty() {
                state.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::LoadSelected);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Loading another design will discard the current one's unsaved changes.".into(),
                );
                ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                return;
            }
            do_load_selected(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &db,
                &source,
            );
        });
    });
}

/// Whether replacing the design now would throw away edits made since `captured`, the
/// generation the cutter last decided about: the generation moved AND the design is
/// unsaved. A generation that did not move means the cutter already answered for
/// exactly this design (a "Discard" that resumed the load leaves it dirty by choice).
const fn discards_edits_since(captured: u64, current: u64, is_dirty: bool) -> bool {
    current != captured && is_dirty
}

/// [`discards_edits_since`] against the live state.
fn edited_since(state: &Rc<RefCell<EditorState>>, captured: u64) -> bool {
    let st = state.borrow();
    discards_edits_since(captured, st.current_generation(), st.is_dirty())
}

/// Puts the Save/Discard/Cancel guard back up for a Load Selected whose remote
/// download finished after the design gained unsaved edits: nothing has been
/// replaced, and "Save"/"Discard" resume the load (a fresh download) exactly like
/// the first dirty check does.
fn ask_again_before_load(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    state.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::LoadSelected);
    let model = ui.global::<EditorModel>();
    model.set_unsaved_dialog_message(
        "The design was edited while the remote design was downloading. Loading it will \
         discard those unsaved changes."
            .into(),
    );
    model.set_unsaved_dialog_open(true);
}

/// The actual "Load Selected" work. The dirty check runs before this is called (see
/// [`setup_load_selected_callback`]). `entry_id` is read from
/// [`LibraryModel::get_selected_entry_id`] rather than passed in: the guard dialog blocks other
/// interaction while it is open, so the selection cannot have changed.
fn do_load_selected(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if entry_id < 0 {
        show_toast(ui, "No diagram selected to load.", "error");
        return;
    }

    let current_source = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(worker) = current_source.worker().cloned() {
        // Remote: fetch this entry's `.asc` text off the UI thread. The completion closure must
        // be `Send + 'static`, so it captures only `Arc` handles and reads the state through
        // `REMOTE_LOAD_TARGET`.
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        // The download is asynchronous and the editor stays interactive meanwhile:
        // the generation as of the dirty check (or the Save/Discard answer) is
        // compared again when the design arrives, before anything is replaced.
        let requested_generation = state.borrow().current_generation();
        fetch_remote_design_source(
            ui.as_weak(),
            worker,
            i64::from(entry_id),
            move |ui, result| match result {
                Ok(remote) => {
                    // `.asc` has no notation for frosting: the reply carries it beside the
                    // text, and it is applied to the design built from that text.
                    let design =
                        loading::design_from_asc_text(&remote.file_name, &remote.asc_text, None)
                            .map(|mut loaded| {
                                loaded.design.apply_imported_frosting(&remote.frosting);
                                loaded
                            });
                    match design {
                        Ok(loaded) => REMOTE_LOAD_TARGET.with(|cell| {
                            let target = cell.borrow().clone();
                            if let Some(state) = target {
                                if edited_since(&state, requested_generation) {
                                    ask_again_before_load(ui, &state);
                                    return;
                                }
                                apply_loaded_design(
                                    ui,
                                    &state,
                                    &render_ctx,
                                    &preview_state,
                                    &solid_last_solved,
                                    LoadedDesignOutcome {
                                        loaded,
                                        printed_proportions: None,
                                        // The remote reply carries no persisted material:
                                        // the refractive index names it.
                                        preview_material: None,
                                        label: &remote.file_name,
                                        // A remote entry id and a local row id occupy
                                        // independent id spaces (`EditorState::
                                        // source_entry_id`'s own doc comment) -- never
                                        // stored here.
                                        source_entry_id: None,
                                        catalogue_url: None,
                                    },
                                );
                            }
                        }),
                        Err(e) => show_toast(
                            ui,
                            &format!(
                                "'{}' failed to parse as a .asc cutting instructions: {e}",
                                remote.file_name
                            ),
                            "error",
                        ),
                    }
                }
                Err(e) => show_toast(ui, &e, "error"),
            },
        );
        return;
    }

    load_local_entry(
        ui,
        state,
        &RenderPreviewHandles {
            render_ctx,
            preview_state,
            solid_last_solved,
        },
        db,
        entry_id,
    );
}

/// [`do_load_selected`]'s local branch. The full record (every angle row and file BLOB) and
/// its parse run on a worker over a read-only connection, like the remote branch. Only the
/// apply runs on the UI thread, gated by the solve policy in `apply_loaded_design`. The
/// edit-since check is the remote branch's, since the load is asynchronous.
fn load_local_entry(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    handles: &RenderPreviewHandles<'_>,
    db: &Arc<Mutex<Database>>,
    entry_id: i32,
) {
    let render_ctx = Arc::clone(handles.render_ctx);
    let preview_state = Arc::clone(handles.preview_state);
    let solid_last_solved = Arc::clone(handles.solid_last_solved);
    let requested_generation = state.borrow().current_generation();
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    let spawned = std::thread::Builder::new()
        .name("load-selected-read".to_owned())
        .spawn(move || {
            // The entry's persisted material rides along with the record: the library shows
            // it, so the design opens carrying it (`apply_library_material`).
            let (read, preview_material) =
                Database::with_read_connection(&crate::gui::db_path(), &db, |database| {
                    let id = i64::from(entry_id);
                    (
                        database.get_diagram_full(id),
                        database.get_preview_material(id).ok().flatten(),
                    )
                });
            let parsed = match read {
                Ok(Some(full)) => loading::design_from_full_record(&full).map(|loaded| {
                    let printed = loading::external_proportions_from_full_record(&full);
                    (full, loaded, printed)
                }),
                _ => Err("Diagram detail not found.".to_owned()),
            };
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                let (full, loaded, printed_proportions) = match parsed {
                    Ok(parts) => parts,
                    Err(e) => {
                        // No design lands, so a stone the planner asked for has nowhere to go.
                        planned_stone::clear();
                        return show_toast(&ui, &e, "error");
                    }
                };
                REMOTE_LOAD_TARGET.with(|cell| {
                    let target = cell.borrow().clone();
                    let Some(state) = target else {
                        return;
                    };
                    if edited_since(&state, requested_generation) {
                        ask_again_before_load(&ui, &state);
                        return;
                    }
                    apply_loaded_design(
                        &ui,
                        &state,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        LoadedDesignOutcome {
                            loaded,
                            printed_proportions,
                            preview_material,
                            label: &full.title,
                            source_entry_id: Some(full.entry_id),
                            catalogue_url: Some(&full.url),
                        },
                    );
                });
            });
        });
    if spawned.is_err() {
        planned_stone::clear();
        show_toast(ui, "Could not start loading the design.", "error");
    }
}

/// The render and preview handles that [`run_pending_unsaved_action`]'s three resumable actions
/// need. Bundled to keep that function within clippy's `too_many_arguments` limit.
#[derive(Clone, Copy)]
struct RenderPreviewHandles<'a> {
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    preview_state: &'a Arc<SolidPreviewState>,
    solid_last_solved: &'a SolidLastSolved,
}

/// Runs whichever [`PendingUnsavedAction`] `pending` holds, or nothing for `None`. It is called
/// from the "Discard" handler and from the [`AfterSave::Resume`] listener, the only two ways past
/// the Save/Discard/Cancel guard. `pending` is taken by value: each arm calls a `do_*` function
/// that borrows `state` itself, so the caller must not hold that borrow.
fn run_pending_unsaved_action(
    pending: Option<PendingUnsavedAction>,
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    handles: &RenderPreviewHandles<'_>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let RenderPreviewHandles {
        render_ctx,
        preview_state,
        solid_last_solved,
    } = *handles;
    match pending {
        Some(PendingUnsavedAction::New {
            spec,
            template_index,
        }) => {
            do_new_design_create(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                *spec,
                template_index,
            );
        }
        Some(PendingUnsavedAction::LoadSelected) => {
            do_load_selected(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                db,
                source,
            );
        }
        Some(PendingUnsavedAction::OpenNative) => {
            do_open_native(ui, state, render_ctx, preview_state, solid_last_solved);
        }
        None => {}
    }
}

/// The Save/Discard/Cancel unsaved-changes guard's resolution callbacks, shared by New, Load
/// Selected and Open (see [`PendingUnsavedAction`]). Only one of them can be pending at a time,
/// and Slint keeps only the last handler registered for a callback, so these are registered once,
/// from [`setup_load_selected_callback`], which also owns `db` and `source`.
///
/// "Save" invokes `EditorModel.save_native` and marks [`AfterSave::Resume`]. The pending action
/// then runs only once that save lands (see the listener below). Save is asynchronous, so an
/// `is_dirty` check right after the invoke would read the state from before the save. A failed
/// or cancelled save never fires the listener, and the pending action is dropped, since "Save"
/// always ends the guard.
fn setup_unsaved_guard_dispatch(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    // Runs when the save started by this guard's "Save" completes (see [`AfterSave::Resume`]).
    // A listener that finds another value in `after_save` leaves it alone, since only one
    // listener may consume a value (see `native_io::on_save_completed`).
    let render_ctx_listener = Arc::clone(render_ctx);
    let preview_state_listener = Arc::clone(preview_state);
    let solid_last_solved_listener = Arc::clone(solid_last_solved);
    let db_listener = Arc::clone(db);
    let source_listener = Arc::clone(source);
    native_io::on_save_completed(move |ui, state| {
        let pending = {
            let mut st = state.borrow_mut();
            match st.after_save.take() {
                Some(AfterSave::Resume(action)) => Some(action),
                other => {
                    st.after_save = other;
                    return;
                }
            }
        };
        run_pending_unsaved_action(
            pending,
            ui,
            state,
            &RenderPreviewHandles {
                render_ctx: &render_ctx_listener,
                preview_state: &preview_state_listener,
                solid_last_solved: &solid_last_solved_listener,
            },
            &db_listener,
            &source_listener,
        );
    });

    let state_save = Rc::clone(state);
    let ui_weak_save = ui.as_weak();
    ui.global::<EditorModel>().on_unsaved_dialog_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.global::<EditorModel>().set_unsaved_dialog_open(false);
        let Some(action) = state_save.borrow_mut().pending_unsaved_action.take() else {
            return;
        };
        // Marks `after_save` so the listener above resumes `action` once this
        // save really lands -- see this function's own doc comment.
        state_save.borrow_mut().after_save = Some(AfterSave::Resume(action));
        ui.global::<EditorModel>().invoke_save_native();
    });

    let state_discard = Rc::clone(state);
    let render_ctx_discard = Arc::clone(render_ctx);
    let preview_state_discard = Arc::clone(preview_state);
    let solid_last_solved_discard = Arc::clone(solid_last_solved);
    let db_discard = Arc::clone(db);
    let source_discard = Arc::clone(source);
    let ui_weak_discard = ui.as_weak();
    ui.global::<EditorModel>()
        .on_unsaved_dialog_discard(move || {
            let Some(ui) = ui_weak_discard.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_unsaved_dialog_open(false);
            let pending = state_discard.borrow_mut().pending_unsaved_action.take();
            run_pending_unsaved_action(
                pending,
                &ui,
                &state_discard,
                &RenderPreviewHandles {
                    render_ctx: &render_ctx_discard,
                    preview_state: &preview_state_discard,
                    solid_last_solved: &solid_last_solved_discard,
                },
                &db_discard,
                &source_discard,
            );
        });

    let state_cancel = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_unsaved_dialog_cancel(move || {
            state_cancel.borrow_mut().pending_unsaved_action = None;
            // A stone the Rough Planner asked to open waited on this answer: it is no.
            planned_stone::clear();
        });
}

#[cfg(test)]
mod tests {
    use super::discards_edits_since;

    #[test]
    fn edits_since_the_decision_are_not_discarded_silently() {
        assert!(
            discards_edits_since(4, 5, true),
            "the generation moved and the design is unsaved"
        );
    }

    #[test]
    fn an_unchanged_generation_is_the_design_the_cutter_already_answered_for() {
        assert!(
            !discards_edits_since(4, 4, true),
            "a Discard that resumed the load leaves the design dirty by choice"
        );
    }

    #[test]
    fn edits_undone_back_to_the_saved_state_need_no_question() {
        assert!(!discards_edits_since(4, 6, false));
    }
}
