//! The repeating timer tick ([`setup_remote_rendering`]/[`poll_tick`]) of the orchestrator.
//!
//! It watches `RenderContext`'s camera/light pose, feeds `bridge::remote::handoff::HandoffMachine`,
//! and carries out the actions it returns ([`apply_actions`]) -- deferring the one action
//! that actually dispatches to a remote worker to `super::dispatch::start_remote_render`,
//! called right after by `poll_tick` itself. See this group's own `mod.rs` doc comment.

use super::{
    decisions::{
        LiveRemoteAllowedInputs, RedispatchInputs, drag_held_effective, live_remote_may_run,
        pose_settle_due, served_by_label, should_redispatch,
    },
    dispatch::{connection_is_stale, start_remote_render},
    state::{Orchestrator, current_drag_held, current_pose, lock},
    update::{continue_lane, redraw_from_epoch},
};
use crate::{
    MainWindow, RemoteWorkerModel,
    bridge::{
        export_thread::SceneSnapshot,
        frame_cache::guide_pass::GuideCache,
        remote::{
            LiveDispatch,
            handoff::{HandoffAction, HandoffEvent, HandoffMachine, HandoffState},
            live_lane::LiveLane,
            live_remote_dispatch,
            remote_render::RemoteConnectionHandle,
            zoned_scene_refusal,
        },
        render_thread::{RedrawGate, RenderContext},
    },
    gui::show_toast,
    settings::{LiveComputeTarget, RemoteEndpoint, SettingsPersister},
};
use slint::ComponentHandle;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

/// How long the scene must stay unchanged before `maybe_redispatch` starts a fresh
/// epoch (the edit-burst debounce). NOT the pose settle -- that is
/// [`POSE_SETTLE_DEBOUNCE`]; see `bridge::remote::handoff`'s module docs on what
/// "settled" means.
const SETTLE_DEBOUNCE: Duration = Duration::from_millis(600);
/// How long the pose must stay unchanged after the last change or button release before
/// the view settles (full resolution, remote handoff). A held button blocks the settle
/// outright (`decisions::pose_settle_due`), so this can be short. Counts from the release
/// edge ([`observe_drag_edge`]) or, for a wheel zoom, from the last tick.
const POSE_SETTLE_DEBOUNCE: Duration = Duration::from_millis(250);
/// A held-button flag with no pose change for this long is treated as released --
/// bounds the cost of a release event that never reached the viewport
/// (`decisions::drag_held_effective`).
const DRAG_HELD_WATCHDOG: Duration = Duration::from_secs(20);
/// How often the orchestrator timer below polls `RenderContext`'s camera/light pose
/// for a change. Independent of the debounces -- this is a poll granularity, not
/// a debounce itself.
const POLL_INTERVAL_MS: u64 = 100;

/// Handle to the orchestrator's live state, returned by [`setup_remote_rendering`] with its timer.
///
/// `gui::remote::worker_callbacks`' denoise toggle uses it to force a `RemoteOnly` redisplay
/// ([`Self::request_redisplay`]) without exposing `Orchestrator` itself.
#[derive(Clone)]
pub struct RemoteOrchestratorHandle(Arc<Mutex<Orchestrator>>);

impl RemoteOrchestratorHandle {
    /// Redraws the settled epoch's accumulated image. This is the `RemoteOnly` counterpart of
    /// `RenderContext::redisplay_requested`: `RemoteOnly` suspends the render thread's loop,
    /// so the redraw must reach the display directly. Safe to call in any mode; it does
    /// nothing while combining or with no live lane.
    pub fn request_redisplay(&self, ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
        let Some((width, height)) = lock(&self.0)
            .live_lane
            .as_ref()
            .map(|lane| lane.epoch().dimensions())
        else {
            return;
        };
        redraw_from_epoch(ui, render_ctx, &self.0, width, height);
    }
}

/// Wires up the preview-then-handoff orchestrator.
///
/// A repeating `slint::Timer` polls
/// `render_ctx`'s camera/light pose, feeds `bridge::remote::handoff::HandoffMachine`, and
/// dispatches to `bridge::remote::remote_render` when the machine decides to hand off.
/// Returns the `slint::Timer`, which the caller (`gui::run_gui`) must keep alive for the
/// life of the window: a dropped `Timer` stops firing. Also returns a
/// [`RemoteOrchestratorHandle`] onto the same state, for `gui::remote::worker_callbacks`.
#[must_use]
pub fn setup_remote_rendering(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> (slint::Timer, RemoteOrchestratorHandle) {
    let state = Arc::new(Mutex::new(Orchestrator {
        handoff: HandoffMachine::new(),
        remote_handle: None,
        live_lane: None,
        lane_scene: None,
        current_request_id: None,
        next_request_id: 1,
        refusal_noted: false,
        worker_label: String::new(),
        scene_seen: (0, Instant::now()),
        gave_up_for: None,
        remote_connection: None,
        display_only_refused: false,
        remote_hdr: None,
        remote_zoning: None,
        last_pose: current_pose(render_ctx),
        last_change_at: Instant::now(),
        drag_was_held: false,
        guide_cache: GuideCache::new(),
        pending_guide_gen: None,
        pending_denoise_gen: None,
        last_denoised: None,
        redraw_gate: RedrawGate::new(),
        last_redraw_at: None,
        live_remote_was_allowed: true,
    }));
    let handle = RemoteOrchestratorHandle(state.clone());
    let timer = slint::Timer::default();
    let ui_weak = ui.as_weak();
    let render_ctx_poll = render_ctx.clone();
    let settings_store_poll = settings_store.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(POLL_INTERVAL_MS),
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            poll_tick(&ui, &render_ctx_poll, &settings_store_poll, &state);
        },
    );
    (timer, handle)
}

fn poll_tick(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    // Checked first, before the drag branch below can `return` early -- a Pause or an
    // export starting mid-drag must suspend the live remote lane right away rather than
    // wait for the drag to end. See `update_live_remote_suspension`'s own doc comment.
    super::progress::sync_availability(ui, render_ctx);
    let remote_allowed = update_live_remote_suspension(render_ctx, state);

    let pose = current_pose(render_ctx);
    let held_raw = observe_drag_edge(render_ctx, state);
    let changed = pose != lock(state).last_pose;

    if changed {
        lock(state).last_pose = pose;
        lock(state).last_change_at = Instant::now();
        let actions = lock(state).handoff.handle(HandoffEvent::OrientationChanged);
        apply_actions(&actions, render_ctx, state);
        sync_served_by_to_ui(ui, render_ctx, state);
        sync_camera_moving_to_ctx(render_ctx, state);
        return;
    }

    let (previewing, quiet_for) = {
        let s = lock(state);
        (
            matches!(s.handoff.state(), HandoffState::Previewing),
            s.last_change_at.elapsed(),
        )
    };
    // A held button keeps the machine in `Previewing` (so `camera_moving` and the reduced
    // preview resolution persist however long the pointer rests), unless the flag has
    // outlived `DRAG_HELD_WATCHDOG` with no pose change -- a lost release.
    let held = drag_held_effective(held_raw, quiet_for, DRAG_HELD_WATCHDOG);
    // Remote stays idle while `remote_allowed` is false: the image is not visible, live
    // rendering is paused, or an export/batch job runs (see `live_remote_allowed`). A
    // remote worker would otherwise spend GPU/cloud time on a frame nobody sees, as local
    // tracing is suspended for the same flags (`frame_helpers::SuspensionFlags`). The
    // handoff state is left untouched, so a settle is re-evaluated once allowed again.
    if pose_settle_due(previewing, held, quiet_for, POSE_SETTLE_DEBOUNCE) && remote_allowed {
        // `LocalOnly`, no configured worker, and a scene remote may not render (an HDR
        // environment -- see `bridge::remote::guard`) all count as "no worker" right
        // here, at the one place `HandoffMachine` ever learns whether a worker is
        // available; `bridge::remote::handoff` itself stays unaware these choices exist, seeing
        // only `worker_available: bool`.
        let endpoint = settle_endpoint(ui, render_ctx, settings_store, state);
        // Drop a cached connection whose identity does not match the freshly read
        // endpoint (address, cert dir, remote removed or compute switched off), so no
        // request is sent over a connection authenticated under stale certificates. A
        // remote render is never in flight here: `pose_settle_due` requires `Previewing`,
        // which only re-enters after the previous request's cleanup, so `remote_handle`
        // is `None`.
        drop_stale_connection(state, endpoint.as_ref());
        let actions = lock(state).handoff.handle(HandoffEvent::SettleElapsed {
            worker_available: endpoint.is_some(),
        });
        apply_actions(&actions, render_ctx, state);
        if let Some(endpoint) = endpoint {
            start_remote_render(ui, render_ctx, endpoint, state);
        }
    }
    // Synced every tick: `Previewing` can persist unchanged while a drag continues.
    sync_camera_moving_to_ctx(render_ctx, state);
    super::target::sync_target(render_ctx, state);
    reconcile_lane_with_ctx(render_ctx, state);
    if remote_allowed {
        resume_idle_lane(ui, render_ctx, state);
    }
    maybe_redispatch(ui, render_ctx, settings_store, state, held);
    reconcile_served_by_after_release(ui, render_ctx, state);
    // The "served by" indicator also depends on state no handoff event reports (the
    // live epoch's remote contribution in `Both`), so it is refreshed every tick.
    sync_served_by_to_ui(ui, render_ctx, state);
}

/// Reads `RenderContext::camera_drag_held` and tracks its held-to-released edge: on that
/// edge `last_change_at` restarts, so the settle debounce
/// ([`POSE_SETTLE_DEBOUNCE`]) counts from the release rather than from the last pose
/// change made while the button was down. Returns the raw flag (the watchdog is applied
/// by the caller via `decisions::drag_held_effective`).
fn observe_drag_edge(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) -> bool {
    let held_raw = current_drag_held(render_ctx);
    let was_held = std::mem::replace(&mut lock(state).drag_was_held, held_raw);
    if was_held && !held_raw {
        lock(state).last_change_at = Instant::now();
    }
    held_raw
}

/// Whether the live remote lane may transmit anything right now -- the lock-reading
/// wrapper around the pure `decisions::live_remote_may_run`. Read fresh every poll
/// tick and by `update::continue_lane` (gating the next chunk), so a Pause, an export
/// starting/finishing, or a tab-visibility change always applies on the very next
/// check, never stale by more than one [`POLL_INTERVAL_MS`].
#[must_use]
pub(super) fn live_remote_allowed(render_ctx: &Arc<Mutex<RenderContext>>) -> bool {
    let ctx = RenderContext::lock(render_ctx);
    crate::bridge::render_thread::remote_allowed(ctx.engine)
        && live_remote_may_run(LiveRemoteAllowedInputs {
            tab_visible: ctx.tab_visible,
            paused: ctx.effective_paused(),
            export_active: ctx.export_active(),
        })
}

/// Tracks the `true -> false` edge of [`live_remote_allowed`] and suspends the live
/// remote lane exactly on that edge (see [`suspend_live_remote`]) -- called once at the
/// very top of every tick, before [`poll_tick`]'s drag/settle branches, so a Pause or an
/// export starting mid-drag is still caught immediately rather than only once the drag
/// ends. Returns the freshly computed value, so callers needn't lock `render_ctx` again
/// for the same tick.
///
/// The reverse edge (`false -> true`) needs no action here: [`resume_idle_lane`] and
/// [`maybe_redispatch`] already re-evaluate on every tick and pick things back up on
/// their own once this is true again.
#[must_use]
fn update_live_remote_suspension(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) -> bool {
    let allowed = live_remote_allowed(render_ctx);
    let was_allowed = std::mem::replace(&mut lock(state).live_remote_was_allowed, allowed);
    if was_allowed && !allowed {
        suspend_live_remote(render_ctx, state);
    }
    allowed
}

/// Suspends the live remote lane the instant [`live_remote_allowed`] turns `false` (a
/// Pause, an export/batch job starting, or the tab becoming invisible):
///
/// - a display-only (final-picture) lane is abandoned: its single request covers the whole
///   budget and cannot resume mid-stream. `maybe_redispatch` starts a fresh one once allowed.
/// - an accumulating lane only has its in-flight chunk cancelled on the wire. The lane and
///   epoch stay in place. The confirmed cancellation (`update::on_chunk_paused`) merges the
///   chunk's valid prefix through `LiveLane::chunk_paused` without counting a failure, and
///   the lane waits idle for [`resume_idle_lane`].
///
/// A no-op when no lane is live (nothing dispatched yet, or the previous release
/// already cleared it).
fn suspend_live_remote(render_ctx: &Arc<Mutex<RenderContext>>, state: &Arc<Mutex<Orchestrator>>) {
    let Some(display_only) = lock(state)
        .live_lane
        .as_ref()
        .map(LiveLane::is_display_only)
    else {
        return;
    };
    if display_only {
        apply_actions(
            &[
                HandoffAction::SendCancelToWorker,
                HandoffAction::DiscardRemotePartial,
            ],
            render_ctx,
            state,
        );
        if matches!(
            lock(state).handoff.state(),
            HandoffState::Settling | HandoffState::RemoteRendering
        ) {
            let actions = lock(state).handoff.handle(HandoffEvent::RemoteFailed);
            apply_actions(&actions, render_ctx, state);
        }
    } else {
        apply_actions(&[HandoffAction::SendCancelToWorker], render_ctx, state);
    }
}

/// Starts a fresh epoch for a settled view whose previous one was released by
/// something other than a drag -- a scene or settings change, a live-compute-target
/// change, or the render loop's scene-identity check -- once the scene has been stable
/// for [`SETTLE_DEBOUNCE`] (see `decisions::should_redispatch`), without waiting for
/// the next drag and settle. The same dispatch decision as a settle applies afterwards
/// (compute target, configured worker, HDR guard); an epoch whose lane gave up is
/// never retried for the same scene and mode. Blocked while `drag_held` (a mouse button
/// is down for an orbit or light drag): the next move would cancel the new epoch.
fn maybe_redispatch(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    state: &Arc<Mutex<Orchestrator>>,
    drag_held: bool,
) {
    let (generation, tab_visible, suspended, epoch_in_ctx, mode) = {
        let mut ctx = RenderContext::lock(render_ctx);
        (
            ctx.scene_generation(),
            ctx.tab_visible,
            ctx.effective_paused() || ctx.export_active(),
            ctx.live_epoch.is_some(),
            ctx.live_compute_target,
        )
    };
    if matches!(mode, LiveComputeTarget::LocalOnly) {
        return;
    }
    let now = Instant::now();
    let inputs = {
        let mut s = lock(state);
        if s.scene_seen.0 != generation {
            s.scene_seen = (generation, now);
        }
        RedispatchInputs {
            handoff_idle: matches!(s.handoff.state(), HandoffState::Idle),
            tab_visible,
            epoch_live: epoch_in_ctx || s.live_lane.is_some(),
            scene_stable: now.saturating_duration_since(s.scene_seen.1) >= SETTLE_DEBOUNCE,
            gave_up_for_this_scene: s.gave_up_for == Some((generation, mode)),
            suspended,
            drag_held,
        }
    };
    if !should_redispatch(inputs) {
        return;
    }
    let Some(endpoint) = settle_endpoint(ui, render_ctx, settings_store, state) else {
        return;
    };
    // Same stale-certificate guard as the settle path above.
    drop_stale_connection(state, Some(&endpoint));
    let actions = lock(state).handoff.handle(HandoffEvent::Redispatch);
    apply_actions(&actions, render_ctx, state);
    start_remote_render(ui, render_ctx, endpoint, state);
}

/// Drops the cached persistent connection when its identity no longer matches the
/// configured endpoint (see `connection_is_stale`), and with it the connection's
/// remembered "final picture refused" -- a new connection may reach an upgraded server.
fn drop_stale_connection(state: &Arc<Mutex<Orchestrator>>, endpoint: Option<&RemoteEndpoint>) {
    let mut s = lock(state);
    if connection_is_stale(
        s.remote_connection
            .as_ref()
            .map(RemoteConnectionHandle::worker),
        endpoint.map(|e| &e.connection),
    ) {
        s.remote_connection = None;
        s.display_only_refused = false;
        s.remote_hdr = None;
        {
            s.remote_zoning = None;
        }
    }
}

/// The remote endpoint this settle dispatches to, or `None` to render locally: the live
/// dispatch decision (`bridge::remote::live_remote_dispatch`: compute target, remote
/// configured, HDR guard) applied to the configured endpoint. A refusal shows its status
/// note once per refusal period, reset when remote is allowed again.
fn settle_endpoint(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    state: &Arc<Mutex<Orchestrator>>,
) -> Option<RemoteEndpoint> {
    let endpoint = settings_store.snapshot().settings.remote;
    let remote_hdr = lock(state).remote_hdr;
    let decision = {
        let ctx = RenderContext::lock(render_ctx);
        live_remote_dispatch(
            ctx.live_compute_target,
            endpoint.is_some(),
            ctx.env_map.as_ref(),
            remote_hdr,
        )
    };
    let decision = keep_zoned_stone_local(decision, render_ctx, state);
    match decision {
        LiveDispatch::Remote => {
            lock(state).refusal_noted = false;
            endpoint
        }
        LiveDispatch::Local => None,
        LiveDispatch::Refused(refusal) => {
            let first_time = !std::mem::replace(&mut lock(state).refusal_noted, true);
            if first_time {
                show_toast(ui, refusal.live_note(), "info");
            }
            None
        }
    }
}

/// Turns a [`LiveDispatch::Remote`] decision into a refusal when the live remote is known
/// NOT to accept zoned materials ([`Orchestrator::remote_zoning`] is `Some(false)`: a
/// default-build worker or a coordinator) and the stone has colour zones, which it would
/// show only as its base zone. Before the connection first reported its capability (`None`)
/// the decision stands: the connection then refuses the first chunk with
/// `RemoteError::ZoningUnsupported`, which also records the answer. The scene is captured
/// only when the remote is known not to accept zones, so a zoning worker costs nothing here.
fn keep_zoned_stone_local(
    decision: LiveDispatch,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) -> LiveDispatch {
    if !matches!(decision, LiveDispatch::Remote) || lock(state).remote_zoning != Some(false) {
        return decision;
    }
    let zoned = SceneSnapshot::capture(render_ctx)
        .is_ok_and(|snapshot| !snapshot.engine.is_ray() && snapshot.material.zoning.is_some());
    match zoned_scene_refusal(zoned, false) {
        Ok(()) => decision,
        Err(refusal) => LiveDispatch::Refused(refusal),
    }
}

/// Drops the lane once `RenderContext::live_epoch` does not hold its epoch. The render
/// loop releases the epoch on a scene change or a scene-identity mismatch
/// (`resolve_remote_ownership`, `live_split::epoch_scene_mismatch`), and a live-compute-target
/// change does the same. The chunk still on the wire is cancelled, since its samples belong
/// to a dead epoch, and a remote attempt the handoff machine still thinks is running ends.
/// [`maybe_redispatch`] starts a fresh epoch once the scene is stable. A no-op otherwise.
fn reconcile_lane_with_ctx(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let ctx_epoch = RenderContext::lock(render_ctx).live_epoch.clone();
    let mut s = lock(state);
    let released = s.live_lane.as_ref().is_some_and(|lane| {
        ctx_epoch
            .as_ref()
            .is_none_or(|epoch| !Arc::ptr_eq(epoch, lane.epoch()))
    });
    if !released {
        return;
    }
    if let Some(handle) = s.remote_handle.take() {
        handle.cancel();
    }
    if let Some(mut lane) = s.live_lane.take() {
        lane.abandon();
    }
    s.lane_scene = None;
    if matches!(
        s.handoff.state(),
        HandoffState::Settling | HandoffState::RemoteRendering
    ) {
        s.handoff.handle(HandoffEvent::RemoteFailed);
    }
}

/// A hidden viewport, a Pause, or a running export pauses the lane between chunks
/// rather than discarding the epoch (the render thread keeps its own accumulation
/// while suspended, so nothing is lost); once [`live_remote_allowed`] again, the next
/// chunk is requested -- or the epoch finishes, if nothing is left to claim.
fn resume_idle_lane(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let idle = lock(state)
        .live_lane
        .as_ref()
        .is_some_and(LiveLane::is_idle);
    if idle {
        continue_lane(ui, &ui.as_weak(), render_ctx, state);
    }
}

/// The render loop can clear `ctx.remote_active` on its own (`resolve_remote_ownership`)
/// when a non-drag scene change (material, lighting, quality) arrives while a remote image
/// is shown. A drag reports that through a handoff event; a non-drag change does not. So
/// when `served_by()` is `Remote` but `remote_active` is cleared, this feeds
/// `HandoffEvent::SceneInvalidated`, so the "served by remote" indicator does not claim a
/// local image. A no-op on every other tick.
fn reconcile_served_by_after_release(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let still_remote_active = RenderContext::lock(render_ctx).remote_active;
    if still_remote_active {
        return;
    }
    let served_by_remote = matches!(
        lock(state).handoff.served_by(),
        crate::bridge::remote::handoff::ImageSource::Remote
    );
    if served_by_remote {
        lock(state).handoff.handle(HandoffEvent::SceneInvalidated);
        sync_served_by_to_ui(ui, render_ctx, state);
    }
}

/// Mirrors `HandoffMachine::state() == Previewing` onto `RenderContext::camera_moving`,
/// the single source of truth for "is the camera currently moving" shared by both the
/// remote handoff and the local preview-then-settle feature.
fn sync_camera_moving_to_ctx(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let moving = matches!(lock(state).handoff.state(), HandoffState::Previewing);
    RenderContext::lock(render_ctx).camera_moving = moving;
}

/// Pushes "which backend served the current image" to the worker-list panel
/// (`RemoteWorkerModel.served_by_remote` + `served_by_worker_name`), as decided by
/// `decisions::served_by_label`: in `Both` a combined "Local + Remote (...)" label
/// whenever the live epoch holds remote samples (and it stays that way after the lane
/// finishes); in `RemoteOnly` the worker once `HandoffMachine::served_by` says so.
/// Called after every `HandoffMachine::handle` call that could change it and once per
/// poll tick; writes a property only when its value actually changes.
pub(super) fn sync_served_by_to_ui(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let (mode, epoch) = {
        let ctx = RenderContext::lock(render_ctx);
        let epoch = ctx.remote_active.then(|| ctx.live_epoch.clone()).flatten();
        // Effective: a final-picture `Both` epoch is labelled as remote-only.
        (ctx.effective_live_target(), epoch)
    };
    let combined_remote_samples = epoch.map_or(0, |epoch| epoch.remote_done());
    let (handoff_says_remote, worker_label) = {
        let s = lock(state);
        (
            matches!(
                s.handoff.served_by(),
                crate::bridge::remote::handoff::ImageSource::Remote
            ),
            s.worker_label.clone(),
        )
    };
    let label = served_by_label(
        mode,
        handoff_says_remote,
        combined_remote_samples,
        &worker_label,
    );
    let model = ui.global::<RemoteWorkerModel>();
    if model.get_served_by_remote() != label.is_some() {
        model.set_served_by_remote(label.is_some());
    }
    if let Some(label) = label
        && model.get_served_by_worker_name().as_str() != label
    {
        model.set_served_by_worker_name(label.into());
    }
}

/// Carries out the [`HandoffAction`]s a `HandoffMachine::handle` call returned, against
/// the real `RenderContext` and the orchestrator's own remote lane/epoch state.
/// Does not dispatch [`HandoffAction::SendRenderRequestToWorker`] itself (that needs the
/// worker config and UI handle, supplied by `start_remote_render` right after this
/// returns) -- only the discard/cancel side effects.
pub(super) fn apply_actions(
    actions: &[HandoffAction],
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    for action in actions {
        match action {
            // `DiscardLocalPreview` and `SendRenderRequestToWorker` always fire together
            // and are both handled by `start_remote_render`, which `poll_tick` calls right
            // after this. There `remote_active`, `dirty` and the fresh epoch are set in one
            // locked mutation with the discard and the dispatch.
            HandoffAction::DiscardLocalPreview | HandoffAction::SendRenderRequestToWorker => {}
            HandoffAction::SendCancelToWorker => {
                if let Some(handle) = &lock(state).remote_handle {
                    handle.cancel();
                }
            }
            HandoffAction::DiscardRemotePartial => {
                let mut s = lock(state);
                s.remote_handle = None;
                // Abandon the epoch's lane: its in-flight chunk is never merged, and
                // nothing of this epoch is ever summed into the next one.
                if let Some(mut lane) = s.live_lane.take() {
                    lane.abandon();
                }
                s.lane_scene = None;
                // The pose that made this render's guides valid is gone too -- abandon
                // whatever prepass was still running for it (see
                // `PendingGuideGeneration`'s doc comment).
                if let Some(pending) = s.pending_guide_gen.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                // Same idea for the background denoise pass -- no cooperative cancel
                // for it, so this just drops the orchestrator's reference; the thread
                // still runs to completion, but nothing here will adopt its result
                // once a fresh `RenderRequest` overwrites this pose's state.
                s.pending_denoise_gen = None;
                s.last_denoised = None;
                drop(s);
                // `remote_active`, the epoch (cursor, remote sums, in-flight chunk) and
                // a `dirty` restart, all in one locked mutation -- the cancelled epoch's
                // contribution must never keep being folded into the fresh local
                // preview that is about to start.
                RenderContext::lock(render_ctx).release_remote();
            }
        }
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;
    use pleochroma::renderer::engine::RenderEngine;

    #[test]
    fn remote_ray_ignores_path_pause_but_still_respects_visibility() {
        let ctx = Arc::new(Mutex::new(RenderContext {
            paused: true,
            tab_visible: true,
            ..RenderContext::default()
        }));
        assert!(!live_remote_allowed(&ctx));
        ctx.lock().unwrap().engine.engine = RenderEngine::RayTraced;
        assert!(live_remote_allowed(&ctx));
        ctx.lock().unwrap().tab_visible = false;
        assert!(!live_remote_allowed(&ctx));
        let mut state = ctx.lock().unwrap();
        state.tab_visible = true;
        state.engine.engine = RenderEngine::PathTraced;
        drop(state);
        assert!(!live_remote_allowed(&ctx));
    }
}
