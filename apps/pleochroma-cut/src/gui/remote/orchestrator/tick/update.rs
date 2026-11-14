//! Handling a `RemoteUpdate` from an in-flight chunk ([`handle_remote_update`]).
//!
//! Merges a finished chunk and requests the next one, keeps a failed chunk's prefix and hands
//! its remainder back, and finishes the epoch. For `LiveComputeTarget::RemoteOnly` it turns the
//! epoch's merged remote sum into a displayed, denoised image ([`redraw_from_epoch`]).

use super::{
    dispatch::dispatch_next_chunk,
    poll::{apply_actions, live_remote_allowed, sync_served_by_to_ui},
    state::{Orchestrator, lock},
};
use crate::{
    MainWindow, ViewportModel,
    bridge::{
        frame_cache::guide_pass::{GuideCache, GuideKey},
        remote::{
            handoff::HandoffEvent,
            live_lane::{ChunkVerdict, LiveLane},
            remote_render::RemoteUpdate,
        },
        render_thread::{RenderContext, tonemap_running_average},
    },
    gui::{remote::worker_callbacks::backend_label, show_toast},
    settings::LiveComputeTarget,
};
use pleochroma::geometry::tool::StoneGeometry;
use pleochroma_net::client::Accumulator;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use super::super::generation::{
    DenoiseGenerationJob, adopt_ready_denoise, adopt_ready_guides, spawn_denoise_generation,
};

/// Minimum real-time gap between two gated remote redraws (`Frame`, `Preview`, `DisplayFrame`).
///
/// About 30 FPS. A redraw is a plain tonemap, which costs tens of milliseconds at 4K. The
/// epoch's final redraw is never subject to this gap: the settled image must always be shown.
const REMOTE_REDRAW_MIN_INTERVAL: Duration = Duration::from_millis(33);

/// Whether another remote redraw is due: `min_interval` has passed since `last_redraw_at`.
///
/// `None` means no redraw yet. `now` is a parameter so the function stays pure and testable.
#[must_use]
fn redraw_is_due(last_redraw_at: Option<Instant>, min_interval: Duration, now: Instant) -> bool {
    last_redraw_at.is_none_or(|t| now.duration_since(t) >= min_interval)
}

/// Whether the effective live target is `Both`, where the render thread draws the combined image.
///
/// Callers then skip their remote-only redraw. A final-picture epoch never combines.
fn is_combining(render_ctx: &Arc<Mutex<RenderContext>>) -> bool {
    matches!(
        RenderContext::lock(render_ctx).effective_live_target(),
        LiveComputeTarget::Both
    )
}

/// The `on_update` callback of every chunk request: gates redraw-worthy events on the
/// connection thread, then carries the update out on the Slint event loop. `chunk` is
/// the epoch's dimensions plus the chunk's own accumulator, whose latest display frame
/// (final-picture live transfer) is what a `DisplayFrame` shows.
pub(super) fn handle_remote_update(
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    chunk: (u32, u32, &Arc<Mutex<Accumulator>>),
    update: RemoteUpdate,
) {
    let (width, height, chunk_accumulator) = chunk;
    let chunk_accumulator = Arc::clone(chunk_accumulator);
    let render_ctx = render_ctx.clone();
    let state = Arc::clone(state);

    // `Frame`/`Preview` events can arrive many times per second, each otherwise queuing a
    // UI-thread closure. They pass `Orchestrator::redraw_gate` (at most one pending closure)
    // and `REMOTE_REDRAW_MIN_INTERVAL` before anything is enqueued. Dropping one is safe: a
    // redraw re-reads the epoch's current sums when it runs. While combining, the render
    // thread owns redraws, and a `Frame` then only feeds the rate estimate.
    //
    // `Connected`/`Progress`/`Done`/`Failed` are never gated: each is a one-time state change
    // or a cheap bookkeeping update. `DisplayFrame` is gated the same way: its closure decodes
    // the accumulator's latest frame when it runs.
    let wants_gated_redraw = matches!(
        update,
        RemoteUpdate::Frame { .. }
            | RemoteUpdate::Preview { .. }
            | RemoteUpdate::DisplayFrame { .. }
    );
    if wants_gated_redraw {
        if is_combining(&render_ctx) {
            if let RemoteUpdate::Frame {
                request_id,
                samples_done,
            } = update
                && let Some(lane) = lock(&state).live_lane.as_mut()
            {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
            return;
        }
        let orch = lock(&state);
        let due = redraw_is_due(
            orch.last_redraw_at,
            REMOTE_REDRAW_MIN_INTERVAL,
            Instant::now(),
        );
        if !due || orch.redraw_gate.submit(()).is_none() {
            return;
        }
        drop(orch);
    }

    let ui_weak_for_next = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        // Release `redraw_gate`'s pending slot unconditionally, before anything else --
        // including the staleness check below, which can itself `return` early.
        if wants_gated_redraw {
            lock(&state).redraw_gate.take();
        }
        // By the time this queued closure actually runs, a later chunk or settle may
        // already be current. An update for anything else is stale and must never
        // drive this orchestrator's state.
        if lock(&state).current_request_id != Some(update.request_id()) {
            return;
        }
        apply_update(
            &UpdateCtx {
                ui: &ui,
                ui_weak: &ui_weak_for_next,
                render_ctx: &render_ctx,
                state: &state,
                chunk_accumulator: &chunk_accumulator,
                width,
                height,
            },
            update,
        );
    });
}

/// Everything [`apply_update`] needs on the Slint event loop, bundled.
struct UpdateCtx<'a> {
    ui: &'a MainWindow,
    ui_weak: &'a Weak<MainWindow>,
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    state: &'a Arc<Mutex<Orchestrator>>,
    chunk_accumulator: &'a Mutex<Accumulator>,
    width: u32,
    height: u32,
}

/// Carries one current (not stale) update out on the Slint event loop.
fn apply_update(c: &UpdateCtx<'_>, update: RemoteUpdate) {
    let (ui, render_ctx, state) = (c.ui, c.render_ctx, c.state);
    if let RemoteUpdate::Frame { samples_done, .. }
    | RemoteUpdate::Progress { samples_done, .. }
    | RemoteUpdate::DisplayFrame { samples_done, .. } = &update
    {
        super::progress::samples(ui, render_ctx, state, *samples_done);
    }

    match update {
        RemoteUpdate::Connected { info, .. } => {
            // A no-op for every chunk after the epoch's first (the machine is
            // already `RemoteRendering`).
            let actions = lock(state)
                .handoff
                .handle(HandoffEvent::RemoteStreamStarted);
            apply_actions(&actions, render_ctx, state);
            {
                let mut s = lock(state);
                s.worker_label = backend_label(info.render.as_ref());
                s.remote_hdr = Some(info.render.as_ref().is_some_and(|r| r.hdr));
                {
                    s.remote_zoning = Some(info.zoning);
                }
            }
            sync_served_by_to_ui(ui, render_ctx, state);
        }
        RemoteUpdate::Frame {
            request_id,
            samples_done,
        } => {
            tracing::trace!("remote chunk {request_id}: {samples_done} samples done");
            if let Some(lane) = lock(state).live_lane.as_mut() {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
            redraw_from_epoch(ui, render_ctx, state, c.width, c.height);
        }
        RemoteUpdate::Preview { .. } => {
            redraw_from_epoch(ui, render_ctx, state, c.width, c.height);
        }
        RemoteUpdate::Progress {
            request_id,
            samples_done,
        } => {
            if let Some(lane) = lock(state).live_lane.as_mut() {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
        }
        RemoteUpdate::Done {
            request_id,
            cancelled,
        } => {
            if cancelled {
                // A drag or scene-change cancel already abandoned the lane, so `chunk_paused`
                // finds no matching chunk. A suspend cancel (`poll::suspend_live_remote`) keeps
                // the lane and waits for this confirmation to merge the chunk's valid prefix.
                on_chunk_paused(state, request_id);
            } else {
                super::progress::finished(ui, render_ctx, state, c.chunk_accumulator);
                // The final display frame may have been coalesced away by the gate.
                if lane_is_display_only(state) {
                    show_display_frame(ui, state, c.chunk_accumulator, c.width, c.height);
                }
                on_chunk_done(ui, c.ui_weak, render_ctx, state, request_id);
            }
        }
        RemoteUpdate::Failed {
            request_id,
            message,
        } => on_chunk_failed(ui, c.ui_weak, render_ctx, state, request_id, &message),
        RemoteUpdate::Unsupported {
            request_id,
            message,
        } => {
            if lane_is_display_only(state) {
                on_display_only_refused(ui, render_ctx, state, &message);
            } else {
                on_chunk_failed(ui, c.ui_weak, render_ctx, state, request_id, &message);
            }
        }
        RemoteUpdate::DisplayFrame {
            request_id,
            samples_done,
        } => {
            tracing::trace!("remote display frame {request_id}: {samples_done} samples");
            show_display_frame(ui, state, c.chunk_accumulator, c.width, c.height);
        }
        RemoteUpdate::CapabilityChanged { render, .. } => {
            // A coordinator gained or lost joined workers: keep "served by" true, and
            // its HDR-map support current for the next settle.
            {
                let mut s = lock(state);
                s.worker_label = backend_label(render.as_ref());
                s.remote_hdr = Some(render.as_ref().is_some_and(|r| r.hdr));
                // Only a coordinator re-advertises capacity, and a coordinator never
                // advertises the zoning capability.
                {
                    s.remote_zoning = Some(false);
                }
            }
            sync_served_by_to_ui(ui, render_ctx, state);
        }
        // Only a `FinalImageRequest` produces one, and the live view never sends it.
        RemoteUpdate::FinalImage { .. } => {}
    }
}

fn lane_is_display_only(state: &Arc<Mutex<Orchestrator>>) -> bool {
    lock(state)
        .live_lane
        .as_ref()
        .is_some_and(LiveLane::is_display_only)
}

/// The decoded RGBA8 of `accumulator`'s latest display frame (final-picture live
/// transfer), `None` before the first one arrived. Pure: no Slint, no lock.
///
/// # Errors
///
/// A message when the frame's size is not the epoch's `width x height` or it does not
/// decode (`pleochroma_net::display::decode_rgba8`'s bounded checks).
fn display_frame_rgba(
    accumulator: &Accumulator,
    width: u32,
    height: u32,
) -> Result<Option<Vec<u8>>, String> {
    let Some(frame) = accumulator.last_display_frame() else {
        return Ok(None);
    };
    if (frame.width, frame.height) != (width, height) {
        return Err(format!(
            "a {}x{} display frame for a {width}x{height} view",
            frame.width, frame.height
        ));
    }
    pleochroma_net::display::decode_rgba8(frame.encoding, width, height, &frame.bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Shows the chunk's latest display frame as the settled image -- already tone-mapped
/// and denoised by the remote, so no local denoise/tonemap touches it.
fn show_display_frame(
    ui: &MainWindow,
    state: &Arc<Mutex<Orchestrator>>,
    chunk_accumulator: &Mutex<Accumulator>,
    width: u32,
    height: u32,
) {
    let decoded = display_frame_rgba(
        &chunk_accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        width,
        height,
    );
    match decoded {
        Ok(Some(rgba)) => {
            lock(state).last_redraw_at = Some(Instant::now());
            let Some(generation) = lock(state)
                .live_lane
                .as_ref()
                .map(|lane| lane.epoch().scene_generation())
            else {
                return;
            };
            push_image(ui, generation, width, height, &rgba);
        }
        Ok(None) => {}
        Err(message) => tracing::warn!("dropping a remote display frame: {message}"),
    }
}

/// The remote refused a display-only request (`UNSUPPORTED_REQUEST`: a plain worker).
/// Remember it for this connection, drop the epoch, and let the next poll tick
/// re-dispatch the settled view with full data, with one note.
fn on_display_only_refused(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    message: &str,
) {
    {
        let mut s = lock(state);
        s.display_only_refused = true;
        s.remote_handle = None;
        if let Some(mut lane) = s.live_lane.take() {
            lane.abandon();
        }
        s.lane_scene = None;
    }
    RenderContext::lock(render_ctx).release_remote();
    let actions = lock(state).handoff.handle(HandoffEvent::RemoteFailed);
    apply_actions(&actions, render_ctx, state);
    show_toast(
        ui,
        &format!(
            "The remote cannot send finished live frames ({message}); using full data \
             instead."
        ),
        "info",
    );
}

/// Requests the epoch's next chunk if [`live_remote_allowed`] (a hidden viewport, a
/// Pause, or a running export just pauses the lane between chunks; `poll::
/// resume_idle_lane` picks it up again once allowed), and finishes the epoch once the
/// lane has nothing left to claim.
pub(super) fn continue_lane(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    if live_remote_allowed(render_ctx) && dispatch_next_chunk(ui_weak, render_ctx, state) {
        return;
    }
    let finished = lock(state)
        .live_lane
        .as_ref()
        .is_some_and(LiveLane::is_finished);
    if finished {
        finish_epoch(ui, render_ctx, state);
    }
}

/// A chunk's `DONE` (not cancelled): merge it into the epoch, then continue the lane.
fn on_chunk_done(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    request_id: u32,
) {
    let merged = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_done(request_id, Instant::now()));
    if merged.is_none() {
        return;
    }
    lock(state).remote_handle = None;
    continue_lane(ui, ui_weak, render_ctx, state);
}

/// Confirms the cancellation of the in-flight chunk `request_id` after a suspend.
///
/// The cancellation is `RemoteUpdate::Done{cancelled: true}`, requested by
/// `poll::suspend_live_remote`. Merges the chunk's valid
/// prefix via `LiveLane::chunk_paused`, counting neither a failure nor a rate sample: the
/// worker was cut off, not slow or broken. Does not call [`continue_lane`]: the lane stays
/// idle until `poll::resume_idle_lane` sees `live_remote_allowed` again. A no-op when
/// `request_id` is not the lane's in-flight chunk.
fn on_chunk_paused(state: &Arc<Mutex<Orchestrator>>, request_id: u32) {
    let merged = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_paused(request_id));
    if merged.is_none() {
        return;
    }
    lock(state).remote_handle = None;
}

/// Handles a failed chunk: worker error, transport error or liveness timeout.
///
/// The lane keeps its valid prefix and hands the remainder back. After too many failures in a
/// row the lane gives up for this epoch and shows one status note. Under `Both` the image
/// finishes locally (the merged remote prefix stays in it); under `RemoteOnly` the epoch is
/// released and local rendering restarts.
fn on_chunk_failed(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    request_id: u32,
    message: &str,
) {
    let verdict = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_failed(request_id));
    lock(state).remote_handle = None;
    match verdict {
        None => {}
        Some(ChunkVerdict::Continue) => {
            tracing::warn!("remote chunk {request_id} failed ({message}); retrying");
            continue_lane(ui, ui_weak, render_ctx, state);
        }
        Some(ChunkVerdict::GaveUp) => {
            // Not re-dispatched for this scene and mode (see `decisions::should_redispatch`);
            // a scene change or mode switch starts an epoch that may try the worker again.
            let mode = RenderContext::lock(render_ctx).live_compute_target;
            {
                let mut s = lock(state);
                s.gave_up_for = s
                    .live_lane
                    .as_ref()
                    .map(|lane| (lane.epoch().scene_generation(), mode));
            }
            let actions = lock(state).handoff.handle(HandoffEvent::RemoteFailed);
            apply_actions(&actions, render_ctx, state);
            if is_combining(render_ctx) {
                show_toast(
                    ui,
                    &format!(
                        "Remote worker failed twice in a row ({message}); finishing this \
                         image locally."
                    ),
                    "info",
                );
            } else {
                {
                    let mut s = lock(state);
                    s.live_lane = None;
                    s.lane_scene = None;
                }
                RenderContext::lock(render_ctx).release_remote();
                show_toast(ui, &format!("Remote render failed: {message}"), "error");
            }
        }
    }
}

/// The lane claimed and finished everything it could: show the final remote image
/// (`RemoteOnly`) and tell the handoff machine the remote render is done.
/// `ctx.remote_active` is deliberately NOT cleared -- the settled image keeps
/// combining (and, in `RemoteOnly`, local stays paused) until a real invalidation.
fn finish_epoch(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let Some((width, height)) = lock(state)
        .live_lane
        .as_ref()
        .map(|lane| lane.epoch().dimensions())
    else {
        return;
    };
    redraw_from_epoch(ui, render_ctx, state, width, height);
    let actions = lock(state).handoff.handle(HandoffEvent::RemoteDone);
    apply_actions(&actions, render_ctx, state);
    sync_served_by_to_ui(ui, render_ctx, state);
    lock(state).remote_handle = None;
}

/// Shows the epoch's merged remote sum for `LiveComputeTarget::RemoteOnly`.
///
/// The sum covers the finished chunks plus the in-flight one. Reads it with its matching sample
/// count, then picks a cached denoised frame, a plain tonemap, or a fresh denoise. A no-op while
/// combining (the render thread owns the image then) or when no epoch is live.
///
/// The color is `remote_sum / remote_count`, with the count read under the same locks as the
/// sum (`LiveEpoch::remote_snapshot`). The background denoise gets that same count, so tone
/// mapping and denoising divide by the count matching the radiance.
///
/// Runs on the UI thread for every `Frame`/`Preview` and at the epoch's end, which can be
/// many times per second. The multi-second denoise pass therefore never runs here: this
/// function does only the plain tonemap synchronously and hands the denoise to
/// `spawn_denoise_generation`. Its result is swapped in on a later redraw, once
/// `adopt_ready_denoise` finds it valid for the pose on screen.
///
/// `state`'s lock is taken briefly and several times, never held across the tonemap:
/// `handle_remote_update`'s rate-limit check takes the same lock on the connection thread.
pub(super) fn redraw_from_epoch(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    width: u32,
    height: u32,
) {
    if is_combining(render_ctx) {
        return;
    }
    // A final-picture epoch sums no radiance: its image is the remote's display frames
    // (`show_display_frame`), never a tonemap of the (empty) epoch sums.
    let Some(epoch) = lock(state)
        .live_lane
        .as_ref()
        .filter(|lane| !lane.is_display_only())
        .map(|lane| Arc::clone(lane.epoch()))
    else {
        return;
    };
    if epoch.dimensions() != (width, height) {
        return;
    }
    let (buffer, remote_count) = epoch.remote_snapshot();
    let samples_done = remote_count.max(1);
    let (yaw, pitch, distance, camera_roll, camera_pan, planes, tools, denoise_enabled, n_d) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.yaw,
            ctx.pitch,
            ctx.distance,
            ctx.camera_roll,
            ctx.camera_pan,
            ctx.active_planes.clone(),
            ctx.active_tools.clone(),
            ctx.denoise_enabled && ctx.engine.denoises(),
            ctx.active_n_d(),
        )
    };
    let stone = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    let camera = pleochroma::optics::raytracer::Camera::new(
        yaw,
        pitch,
        distance,
        pleochroma::optics::raytracer::DEFAULT_FOV_DEG,
    )
    .with_roll(camera_roll)
    .with_pan(camera_pan);
    let desired_key = GuideCache::key_for_camera(width, height, &camera, stone, n_d);

    // Brief lock: adopt a finished background denoise for this pose and read a valid cached one.
    let cached_denoised = cached_denoised_frame(state, &desired_key, denoise_enabled);

    // The plain tonemap runs with no lock held: the rate-limit check in
    // `handle_remote_update` takes this `state` lock on the connection thread.
    let bytes = cached_denoised
        .unwrap_or_else(|| tonemap_running_average(width, height, samples_done, &buffer));

    // Second (brief) lock: dispatch a fresh background denoise generation if warranted,
    // and stamp the redraw timestamp the rate limit reads.
    {
        let mut orch = lock(state);

        // One background denoise generation in flight per pose. A fresh one is dispatched when
        // denoising is on and `pending_denoise_gen` does not match `desired_key` (this covers
        // "nothing dispatched yet", "just adopted above" and "pose changed"). It needs this
        // pose's guides ready; otherwise a later redraw dispatches once the guide prepass lands.
        if denoise_enabled {
            let pending_matches = orch
                .pending_denoise_gen
                .as_ref()
                .is_some_and(|p| p.key == desired_key);
            if !pending_matches {
                let Orchestrator {
                    guide_cache,
                    pending_guide_gen,
                    ..
                } = &mut *orch;
                if adopt_ready_guides(&desired_key, guide_cache, pending_guide_gen.as_ref()) {
                    let guides = guide_cache
                        .ensure_geom_cancellable(
                            desired_key.clone(),
                            stone,
                            &std::sync::atomic::AtomicBool::new(false),
                        )
                        .expect("the adopted guide cache is ready and cancellation is disabled")
                        .clone();
                    orch.pending_denoise_gen = Some(spawn_denoise_generation(
                        desired_key,
                        DenoiseGenerationJob {
                            width,
                            height,
                            samples_done,
                            buffer,
                            guides,
                            yaw,
                            pitch,
                            distance,
                            camera_roll,
                            camera_pan,
                            // An owned copy for the background job; the `Arc` clones above
                            // kept the read cheap.
                            planes: planes.as_ref().clone(),
                            tools: tools.as_ref().clone(),
                            n_d,
                        },
                    ));
                }
            }
        }

        // Stamps this redraw for the Frame/Preview rate limit in `handle_remote_update`.
        orch.last_redraw_at = Some(Instant::now());
    }

    push_image(ui, epoch.scene_generation(), width, height, &bytes);
}

/// Adopts a completed generation and snapshots the freshest valid denoised frame.
/// This brief lock ends before the caller performs its plain tonemap or dispatches work.
fn cached_denoised_frame(
    state: &Arc<Mutex<Orchestrator>>,
    desired_key: &GuideKey,
    denoise_enabled: bool,
) -> Option<Vec<u8>> {
    let mut orch = lock(state);
    if denoise_enabled {
        // Only clear a pending generation on adoption; otherwise it remains in flight.
        if let Some(fresh) = adopt_ready_denoise(desired_key, orch.pending_denoise_gen.as_ref()) {
            orch.pending_denoise_gen = None;
            orch.last_denoised = Some((desired_key.clone(), fresh));
        }
        // A few-samples-stale denoised image for this pose beats flickering back to noise
        // while a newer generation cooks. A different pose must use the plain tonemap.
        match orch.last_denoised.as_ref() {
            Some((key, bytes)) if key == desired_key => Some(bytes.clone()),
            _ => {
                orch.last_denoised = None;
                None
            }
        }
    } else {
        orch.pending_denoise_gen = None;
        orch.last_denoised = None;
        None
    }
}

/// Shows `rgba` (`width * height * 4` bytes) as the viewport's rendered image.
///
/// `generation` is the scene generation the frame was produced for. A frame is shown even
/// when the scene has moved on: during a drag the newest frame is usually already stale, and
/// dropping stale frames would freeze the viewport. The optical overlay gates itself on the
/// recorded stamp instead (`optical_overlay::displayed`).
fn push_image(ui: &MainWindow, generation: u64, width: u32, height: u32, rgba: &[u8]) {
    crate::bridge::optical_overlay::displayed(ui, generation, width, height);
    crate::gui::render::backdrop_color::push_backdrop(ui, rgba, width, height);
    let mut fb = crate::bridge::pixel_buffer::FramebufferTransfer::new(width, height);
    let image = fb.copy_from_gpu_slice(rgba);
    ui.global::<ViewportModel>()
        .set_render_image(crate::bridge::pixel_buffer::rgba_image(image));
    ui.global::<ViewportModel>().set_has_render(true);
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
