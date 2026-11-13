//! The UI-thread-confined `Runtime` singleton auto-solve's other submodules share,
//! plus the handles [`init`] stashes into it for later retrieval.

use crate::{
    bridge::render_thread::RenderContext,
    gui::{
        editor::{activity::ActivityRegistry, state::EditorState, view::SolidLastSolved},
        solid_preview::preview_state::SolidPreviewState,
    },
};
use pleochroma_cut_core::Design;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64},
    },
    time::Duration,
};

/// This module's UI-thread-confined state -- see the parent module doc comment for why
/// this is a `thread_local!` rather than a new `EditorState` field.
pub(super) struct Runtime {
    /// Set by [`init`]. Lets a completed background solve push into the shared solid
    /// preview, as `view::refresh_viewport` does.
    pub(super) preview_state: Option<Arc<SolidPreviewState>>,
    pub(super) solid_last_solved: Option<SolidLastSolved>,
    /// Set by [`stash_render_ctx`].
    pub(super) render_ctx: Option<Arc<Mutex<RenderContext>>>,
    /// Set by [`stash_editor_state`]. A `Send` completion closure cannot capture the
    /// `Rc`, so it reads the state through [`editor_state`] on the UI thread.
    pub(super) editor_state: Option<Rc<RefCell<EditorState>>>,
    /// Set by [`init`]. Lets the solve dispatch and completion register the
    /// background-solve activity.
    pub(super) activity: Option<Rc<ActivityRegistry>>,
    /// The most recent REAL measured solve wall time (background or synchronous) for
    /// whichever design is currently loaded -- `None` until the first solve since the
    /// last New/Load. Drives `scheduling::should_schedule_auto_solve`.
    pub(super) last_solve: Option<Duration>,
    /// Bumped by every dispatch (explicit-Solve or auto-solve alike); see the parent
    /// module doc comment's "newer background solve" case.
    pub(super) current_seq: u64,
    /// The debounce timer auto-solve restarts on every eligible edit. Replacing this
    /// with a fresh `Timer` cancels whatever the previous one had pending -- Slint
    /// stops a `Timer`'s callback from firing once the `Timer` itself is dropped.
    pub(super) debounce: Option<slint::Timer>,
    /// The design/multi-select snapshot [`super::replan::stash_current_design`] last
    /// recorded, paired with the generation it was cloned at -- see that function's
    /// own doc comment. Read by [`super::replan::take_matching_design`] once a
    /// solid-preview frame lands claiming the SAME generation, so the tier table can
    /// be rebuilt from its already-solved masts instead of triggering a second,
    /// redundant `Design::solve()`.
    ///
    /// Shared as an `Arc<Design>`: `view::submit_preview_replan_for` makes one snapshot per
    /// edit-intent frame and stores a cheap `Arc::clone` here. See that function for the
    /// remaining deep clone, the solid-preview worker's `ReplanRequest::design`.
    pub(super) current_design: Option<(u64, Arc<Design>, BTreeSet<usize>)>,
    /// True from the moment `dispatch::dispatch_background_solve`
    /// actually spawns a worker thread until that worker's completion is
    /// observed by `dispatch::apply_background_solve_result` -- gates new dispatches
    /// against piling up multiple concurrent multi-second solve threads. See
    /// [`PendingDispatch`] for what happens to a dispatch request that arrives
    /// while this is `true`.
    pub(super) solve_in_flight: bool,
    /// The most recent dispatch request that arrived while [`Runtime::solve_in_flight`]
    /// was `true` -- captured instead of spawning a second concurrent worker.
    /// `dispatch::apply_background_solve_result` re-dispatches this on completion. A burst
    /// of edits therefore runs as the current solve followed by the latest pending one,
    /// never as a pile of worker threads. Only the latest request is kept.
    pub(super) pending_dispatch: Option<PendingDispatch>,
    /// Debounce timer for [`super::replan::schedule_idle_replan_if_stale`]. Replacing it
    /// cancels the pending timer, as with [`Runtime::debounce`].
    pub(super) idle_replan: Option<slint::Timer>,
    /// The cancel flag the CURRENTLY RUNNING worker's `SolveControl::with_cancel`
    /// observes -- `Some` only between the moment `dispatch::dispatch_background_solve`
    /// actually spawns a worker (not merely queues a [`PendingDispatch`]) and the
    /// moment its completion is observed by `dispatch::apply_background_solve_result`,
    /// which clears this back to `None` in the same preamble that frees
    /// [`Runtime::solve_in_flight`]. `dispatch::cancel_in_flight_solve` flips it, which
    /// is what makes "Abandon Solve" actually stop the worker rather than merely
    /// discard its eventual result -- see that function's own doc comment.
    pub(super) current_cancel: Option<Arc<AtomicBool>>,
    /// [`ActivityRegistry`] id of the running background solve, if any. Set and cleared
    /// together with [`Self::current_cancel`]. Lets a cancel finish the activity at once.
    pub(super) activity_id: Option<u64>,
    /// `true` while the design's over-`MAX_PLANES` warning toast is already shown.
    /// Stops a design stuck over the cap from toasting on every edit. Reset when a
    /// result comes back under the cap.
    pub(super) too_many_planes_toasted: bool,
    /// `true` from [`super::reset_for_new_design`] until the first plane claim of the new
    /// design has framed the stone. Later results of the same design do not re-frame.
    pub(super) frame_pending: bool,
}

impl Runtime {
    pub(super) const fn new() -> Self {
        Self {
            preview_state: None,
            solid_last_solved: None,
            render_ctx: None,
            editor_state: None,
            activity: None,
            last_solve: None,
            current_seq: 0,
            debounce: None,
            current_design: None,
            solve_in_flight: false,
            pending_dispatch: None,
            idle_replan: None,
            current_cancel: None,
            activity_id: None,
            too_many_planes_toasted: false,
            frame_pending: false,
        }
    }
}

/// A `dispatch::dispatch_background_solve` call suppressed by
/// [`Runtime::solve_in_flight`] -- see that field's own doc comment.
pub(super) struct PendingDispatch {
    pub(super) design: Design,
    /// The `generation` value when this dispatch was queued. Kept rather than re-read at
    /// replay, so edits made while queued are detected as stale by
    /// `apply::apply_background_solve_result`.
    pub(super) started_generation: u64,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) multi_selected: BTreeSet<usize>,
}

thread_local! {
    pub(super) static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::new()) };
}

/// Stashes the shared handles this module needs. Called once from
/// `setup_editor_callbacks`, before any dispatch can happen.
///
/// All other fields are reset. The state is per thread, so a window set up again on the
/// same thread would otherwise inherit a stuck [`Runtime::solve_in_flight`]. A worker
/// still running from the old window is told to stop, and `current_seq` keeps counting
/// up, so its result cannot pass `dispatch::is_current`.
pub(in crate::gui::editor) fn init(
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    activity: &Rc<ActivityRegistry>,
) {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if let Some(cancel) = rt.current_cancel.as_ref() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let next_seq = rt.current_seq + 1;
        *rt = Runtime::new();
        rt.current_seq = next_seq;
        rt.preview_state = Some(Arc::clone(preview_state));
        rt.solid_last_solved = Some(Arc::clone(solid_last_solved));
        rt.activity = Some(Rc::clone(activity));
    });
}

/// Returns the shared solid-preview handle that [`init`] stashed, if any. Lets the tier
/// callbacks resubmit the facet overlay without an extra parameter.
pub(in crate::gui::editor) fn preview_state() -> Option<Arc<SolidPreviewState>> {
    RUNTIME.with(|cell| cell.borrow().preview_state.clone())
}

/// Returns the shared "last solved" cache [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`], for the callbacks that need to build a
/// `facet_map::FacetMap` against the last rendered frame's masts without a new
/// parameter on their own fixed call sites.
pub(in crate::gui::editor) fn solid_last_solved() -> Option<SolidLastSolved> {
    RUNTIME.with(|cell| cell.borrow().solid_last_solved.clone())
}

/// Returns the shared [`ActivityRegistry`] [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`]/[`solid_last_solved`], so
/// `callbacks::solve_actions`'s Deep Solve/Optimize dispatch and completion
/// handlers can register/finish their own activities without a new parameter on
/// their own fixed call sites. `None` only if this is called before
/// [`setup_editor_callbacks`](super::super::setup_editor_callbacks) has run `init`,
/// which cannot happen from any real Slint callback.
#[must_use]
pub(in crate::gui::editor) fn activity() -> Option<Rc<ActivityRegistry>> {
    RUNTIME.with(|cell| cell.borrow().activity.clone())
}

/// Stashes the editor state handle for [`editor_state`] below -- called from
/// `callbacks::solve_actions::setup_deep_solve_callback`, which already holds the
/// `Rc` and runs once on the UI thread before the event loop starts.
pub(in crate::gui::editor) fn stash_editor_state(state: &Rc<RefCell<EditorState>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().editor_state = Some(Rc::clone(state));
    });
}

/// Returns the editor state handle [`stash_editor_state`] stashed, if it has run
/// yet. Used by completion closures that must be `Send` and therefore cannot
/// capture the `Rc` themselves; they run on the UI thread, where this is valid.
#[must_use]
pub(in crate::gui::editor) fn editor_state() -> Option<Rc<RefCell<EditorState>>> {
    RUNTIME.with(|cell| cell.borrow().editor_state.clone())
}

/// Stashes the shared [`RenderContext`] handle for [`render_ctx`]. Called from
/// `callbacks::tier_actions::setup_toggle_detach_callback` during setup, before the
/// event loop starts.
pub(in crate::gui::editor) fn stash_render_ctx(render_ctx: &Arc<Mutex<RenderContext>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().render_ctx = Some(Arc::clone(render_ctx));
    });
}

/// Returns the shared [`RenderContext`] handle [`stash_render_ctx`] stashed, if
/// it has run yet -- lets `callbacks::tier_actions`'s Solid-view hover/click
/// callbacks read the configured render resolution without
/// a new parameter on their own fixed call sites in
/// `gui::editor::mod`, which this module does not own.
pub(in crate::gui::editor) fn render_ctx() -> Option<Arc<Mutex<RenderContext>>> {
    RUNTIME.with(|cell| cell.borrow().render_ctx.clone())
}
