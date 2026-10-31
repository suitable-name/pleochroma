//! Remote-only progress shares the local image's UI and ETA estimator.
use super::state::{Orchestrator, lock};
use crate::{
    MainWindow,
    bridge::render_thread::{RenderContext, progress},
    settings::LiveComputeTarget,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Runs before early returns in the poll tick, including for local-only rendering.
pub(super) fn sync_availability(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let (generation, suspension, visible, has_geometry) = {
        let mut ctx = RenderContext::lock(render_ctx);
        let suspension = if ctx.effective_paused() {
            Some("Paused")
        } else if ctx.export_active() {
            Some("Paused for export")
        } else if !ctx.tab_visible {
            Some("Suspended")
        } else {
            None
        };
        (
            ctx.scene_generation(),
            suspension,
            ctx.tab_visible,
            !ctx.active_planes.is_empty(),
        )
    };
    progress::sync_availability(ui, generation, suspension, visible, has_geometry);
}

/// Announce setup before network/device preparation can take time. Both mode's
/// combined progress is published by the local display path instead.
pub(super) fn preparing(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    target: u32,
) {
    if RenderContext::lock(render_ctx).effective_live_target() == LiveComputeTarget::RemoteOnly {
        progress::update_progress(
            ui,
            "Preparing remote trace",
            generation,
            0,
            target,
            Duration::ZERO,
        );
    }
}

/// Report only full-size committed frames or full-size tracing progress. PREVIEW
/// snapshots never call this helper and never advance the final sample budget.
pub(super) fn samples(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    reported: u32,
) {
    let epoch = lock(state)
        .live_lane
        .as_ref()
        .map(|lane| (Arc::clone(lane.epoch()), lane.is_display_only()));
    let Some((epoch, display_only)) = epoch else {
        return;
    };
    let valid = {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.effective_live_target() == LiveComputeTarget::RemoteOnly
            && ctx.scene_generation() == epoch.scene_generation()
            && ctx
                .live_epoch
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &epoch))
    };
    if !valid {
        return;
    }
    // Full-data counts come from one consistent epoch read, including transmitted
    // in-flight samples. Heartbeats never double-count a prefix racing a FRAME.
    // A display-only epoch owns no radiance sum, so its header reports the count.
    let completed = if display_only {
        reported
    } else {
        epoch.remote_done()
    }
    .min(epoch.target_samples());
    progress::update_progress(
        ui,
        "Remote rendering",
        epoch.scene_generation(),
        completed,
        epoch.target_samples(),
        Duration::ZERO,
    );
}

/// Read the actual last frame at DONE, since its `DisplayFrame` callback may have
/// been coalesced away. A completion without an image never fabricates samples.
pub(super) fn finished(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    accumulator: &Mutex<pleochroma_net::client::Accumulator>,
) {
    let samples_done = {
        let accumulator = accumulator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        accumulator
            .last_display_frame()
            .map_or_else(|| accumulator.samples_done(), |frame| frame.samples_done)
    };
    samples(ui, render_ctx, state, samples_done);
}
