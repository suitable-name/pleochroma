//! The editor group's smaller `setup_*` wirings: the solve-cancel button, the startup
//! restore offer and the tier-cutoff slider's replan queue. Each registers its Slint
//! callbacks on the `EditorModel`/`SolidPreviewModel` globals and is called once from
//! [`super::setup_editor_callbacks`].

use super::{
    auto_solve, edit_intent, landing::Landing, native_io, stall_guard, state::EditorState, view,
};
use crate::{
    MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        solid_preview::{cut_slider, preview_state::SolidPreviewState},
        tutorial_events::raise,
    },
};
use pleochroma_editor::{
    edit_intent::DRAIN_INTERVAL,
    guide::viewing_events::{DIAGRAM_PANEL_ENLARGED, cut_slider_event},
};
use slint::{ComponentHandle as _, Model as _};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires the Solve cancel button (`EditorModel.solve_cancel`).
///
/// `auto_solve::cancel_in_flight_solve` drops the in-flight result and any queued dispatch,
/// and sets the worker's cancel flag. The editor is returned on the same click. The design is
/// unchanged, so it reads as "stale" and still needs a solve.
///
/// The worker stops within a sweep or pipeline run, because `dispatch_background_solve`
/// passes a `SolveControl::with_cancel` into the solve. The design really stops solving, and
/// does not just discard a result still running in the background.
pub(super) fn setup_solve_cancel_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>().on_solve_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        auto_solve::cancel_in_flight_solve();
        let model = ui.global::<crate::EditorModel>();
        model.set_solve_running(false);
        model.set_solve_state("stale".into());
        model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
        model.set_status_is_problem(true);
    });
}

/// Offers the leftover autosave, or else the most recent recent file, at startup. Nothing
/// opens by itself.
///
/// An offer, not a silent reopen: silently reopening would hide a crash-recovery file inside
/// an ordinary-looking session, and a cutter could overwrite unsaved work unaware.
///
/// A leftover autosave outranks the recent-files list. It is on disk only because the last run
/// did not shut down cleanly, and it holds work never saved. The next successful save deletes it.
///
/// The recent file offered is the most recent one still on disk, since a missing file could
/// only fail to reopen. Discard removes it from the recent list only, and never deletes the file.
///
/// A design named on the command line (`pleochroma-cut <file>`) outranks both and opens at
/// once. A leftover autosave stays on disk and is offered at the next start.
///
/// Accepting the offer shows the Edit view once the design has landed. A design from the
/// command line leaves the window on Live Render, the page the app starts on.
pub(super) fn setup_startup_restore(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    if let Some(path) = crate::gui::startup_file::take_requested_open() {
        open_requested_design(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            path,
        );
        return;
    }
    let model = ui.global::<crate::EditorModel>();
    let autosave = native_io::find_leftover_autosave();
    let offer = autosave.clone().or_else(|| {
        ui.get_recent_native_files()
            .iter()
            .map(|path| std::path::PathBuf::from(path.as_str()))
            .find(|path| path.is_file())
    });
    let Some(offer) = offer else {
        return;
    };
    model.set_startup_restore_is_autosave(autosave.is_some());
    model.set_startup_restore_path(offer.display().to_string().into());

    let state_accept = Rc::clone(state);
    let render_ctx_accept = Arc::clone(render_ctx);
    let preview_accept = Arc::clone(preview_state);
    let solved_accept = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_accept(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Cleared FIRST: the open below can itself toast or open the
            // fingerprint-mismatch dialog, and this prompt must be gone by then
            // rather than stacked underneath it.
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            native_io::open_recent_native_path(
                &ui,
                &state_accept,
                &render_ctx_accept,
                &preview_accept,
                &solved_accept,
                std::path::PathBuf::from(path.as_str()),
                // The cutter chose to go on with this design, so it shows the Edit view when
                // it lands, although the app starts on Live Render.
                Landing::ShowEditView,
            );
        });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_dismiss(move || {
            let Some(ui) = ui_weak_dismiss.upgrade() else {
                return;
            };
            // The file is left exactly where it is -- declining the offer is not a
            // decision to throw work away. A leftover autosave is removed only by
            // the next successful save (see `finish_save_native_success`).
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
        });

    // "Delete": only shown for a leftover autosave (`EditorModel.startup_restore_is_autosave`,
    // wired in `global_dialogs.slint`'s `tertiary_label`). It removes exactly the offered file,
    // never another leftover that `find_leftover_autosave` did not pick (see
    // `native_io::delete_leftover_autosave`).
    let ui_weak_delete = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_delete(move || {
            let Some(ui) = ui_weak_delete.upgrade() else {
                return;
            };
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            native_io::delete_leftover_autosave(std::path::Path::new(path.as_str()));
        });

    // "Discard": only ever shown for an ordinary recent file, the counterpart of the
    // autosave's Delete. The file is the cutter's own save, so it stays on disk; only the
    // offer goes, by removing the file from the recent list it was taken from.
    let ui_weak_forget = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_forget(move || {
            let Some(ui) = ui_weak_forget.upgrade() else {
                return;
            };
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            if !path.is_empty() {
                native_io::forget_recent_native_file(&ui, path.as_str());
            }
        });
}

/// Opens the design named on the command line. Deferred to the first turns of the
/// event loop: the editor is still being wired up when this is called, and opening
/// pushes the design into panels and the viewport that are not all connected yet.
fn open_requested_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    path: std::path::PathBuf,
) {
    let ui_weak = ui.as_weak();
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        native_io::open_recent_native_path(
            &ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            path,
            // Start-up stays on Live Render: the design shows there, and nobody asked for
            // the editor.
            Landing::StayOnPage,
        );
    });
}

/// What one Cut slider replan needs, shared by the queue's drain and its retries.
struct CutReplan {
    ui_weak: slint::Weak<MainWindow>,
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: view::SolidLastSolved,
}

/// Submits the replan a moved Cut slider (or a changed enlarged panel) asks for. The
/// editor state is only borrowed, never held across a Slint call that can re-enter; when
/// a writer holds it, this tries again on the queue's own 16 ms beat until
/// `retries_left` runs out.
fn run_cut_replan(replan: &Rc<CutReplan>, retries_left: u8) {
    let Some(ui) = replan.ui_weak.upgrade() else {
        return;
    };
    let Ok(st) = replan.state.try_borrow() else {
        if let Some(next) = cut_slider::retries_after_busy(retries_left) {
            let again = Rc::clone(replan);
            slint::Timer::single_shot(DRAIN_INTERVAL, move || run_cut_replan(&again, next));
        }
        return;
    };
    view::submit_preview_replan(
        &ui,
        &replan.render_ctx,
        &replan.preview_state,
        &replan.solid_last_solved,
        &st,
        std::collections::BTreeSet::new(),
        false,
    );
}

/// Redraws the Solid viewport when the high-contrast preference switches, so the diagram
/// takes the other palette at once instead of at the next edit. The plan worker reads the
/// flag itself, so this only asks for a replan of the committed design with an empty dirty
/// set (the solve is reused). The sink's first call happens while it registers and is not a
/// switch. It holds weak handles to the window and the editor state, and unregisters
/// (returns `false`) once either is gone.
fn register_theme_replan(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let ui_weak = ui.as_weak();
    let state_weak = Rc::downgrade(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let registered = std::cell::Cell::new(false);
    crate::gui::preferences::register_theme_sink(move |_| {
        let (Some(_), Some(state)) = (ui_weak.upgrade(), state_weak.upgrade()) else {
            return false;
        };
        if registered.replace(true) {
            let replan = Rc::new(CutReplan {
                ui_weak: ui_weak.clone(),
                state,
                render_ctx: Arc::clone(&render_ctx),
                preview_state: Arc::clone(&preview_state),
                solid_last_solved: Arc::clone(&solid_last_solved),
            });
            run_cut_replan(&replan, cut_slider::REPLAN_RETRIES);
        }
        true
    });
}

/// Redraws the preview when the tier-cutoff slider moves, and when the Diagram's enlarged
/// panel changes (`on_diagram_enlarged_panel_changed`, same queue).
///
/// Only the slider's own `changed(value)` callback is wired here. It fires on a drag, a
/// keyboard nudge or a click, not when a tier push moves the bound value. Changing how much of
/// the schedule is drawn leaves the design as it is, so the solve is reused and only the plane
/// arrangement is rebuilt. That is an empty `dirty` set with `force_full_solve: false`.
///
/// This handler uses `try_borrow`. A tick that finds the editor state borrowed is retried a few
/// ticks later ([`cut_slider::retries_after_busy`]). A writer that holds the state for longer
/// (a blocking file dialog) submits its own replan on the way out, reading the slider fresh.
///
/// A `changed(value)` tick fires on every pixel of a drag. Each one posts an
/// [`edit_intent::EditIntent::CutOff`] into a queue built once here, which coalesces them
/// (see [`edit_intent::EditIntentQueue`]). This avoids calling [`view::submit_preview_replan`]
/// per tick, which clones the whole `Design`, so a drag pays for at most one replan per tick.
pub(super) fn setup_tier_cutoff_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let replan = Rc::new(CutReplan {
        ui_weak: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
    });
    register_theme_replan(ui, state, render_ctx, preview_state, solid_last_solved);
    let intent_queue = edit_intent::EditIntentQueue::new(move |_intent| {
        run_cut_replan(&replan, cut_slider::REPLAN_RETRIES);
    });
    // The diagram's enlarged-panel choice (`SolidPreviewModel.diagram_enlarged_panel`)
    // is read by the plan worker just like the tier cutoff, and changing it alone
    // redraws nothing, so it shares this queue: the drain ignores the intent's
    // payload and `submit_preview_replan` reads every property fresh. Raised for a
    // pill click, a double-click and Escape alike (`changed diagram_enlarged_panel`
    // in `models/solid_preview.slint`).
    let enlarged_queue = Rc::clone(&intent_queue);
    let ui_weak_enlarged = ui.as_weak();
    ui.global::<crate::SolidPreviewModel>()
        .on_diagram_enlarged_panel_changed(move || {
            stall_guard::stall_guard("on_diagram_enlarged_panel_changed", || {
                let Some(ui) = ui_weak_enlarged.upgrade() else {
                    return;
                };
                let model = ui.global::<crate::SolidPreviewModel>();
                let count = model.get_tier_cutoff();
                enlarged_queue.post(edit_intent::EditIntent::CutOff { count });
                // A tutorial step may wait for a panel to be enlarged (not for it to go
                // back to all three).
                if model.get_diagram_enlarged_panel() >= 0 {
                    raise(&ui, DIAGRAM_PANEL_ENLARGED);
                }
            });
        });
    let ui_weak = ui.as_weak();
    ui.global::<crate::SolidPreviewModel>()
        .on_tier_cutoff_changed(move || {
            stall_guard::stall_guard("on_tier_cutoff_changed", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let count = ui.global::<crate::SolidPreviewModel>().get_tier_cutoff();
                intent_queue.post(edit_intent::EditIntent::CutOff { count });
                // A tutorial step may wait for the slider to reach the rough, a step in
                // between, or the finished stone.
                raise(&ui, cut_slider_event(count));
            });
        });
}
