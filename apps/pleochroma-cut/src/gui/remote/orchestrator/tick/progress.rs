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

/// Reports only full-size committed frames or full-size tracing progress. Preview snapshots
/// never call this helper, so they never advance the final sample budget.
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

/// The Live Render stage text while a remote computer builds its shader cache:
/// "Preparing <name>'s graphics card… 12 of 40", without the counts when `total == 0`.
#[must_use]
pub(super) fn preparing_stage_text(remote_name: &str, done: u32, total: u32) -> String {
    let owner = if remote_name.is_empty() {
        "the remote computer's".to_owned()
    } else {
        format!("{remote_name}'s")
    };
    if total == 0 {
        format!("Preparing {owner} graphics card\u{2026}")
    } else {
        format!(
            "Preparing {owner} graphics card\u{2026} {} of {total}",
            done.min(total)
        )
    }
}

/// Shows [`preparing_stage_text`] as the Live Render stage while the remote has traced
/// nothing yet. The bar stays indeterminate (no samples). Stale or foreign epochs are ignored
/// exactly as in [`samples`].
pub(super) fn remote_preparing(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    done: u32,
    total: u32,
) {
    let (epoch, name) = {
        let s = lock(state);
        let Some(lane) = s.live_lane.as_ref() else {
            return;
        };
        (
            Arc::clone(lane.epoch()),
            s.remote_connection
                .as_ref()
                .map(|h| h.worker().name.clone())
                .unwrap_or_default(),
        )
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
    // Samples that already arrived mean the remote is tracing: the normal stage applies.
    if !valid || epoch.remote_done() > 0 {
        return;
    }
    progress::update_progress(
        ui,
        &preparing_stage_text(&name, done, total),
        epoch.scene_generation(),
        0,
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

#[cfg(test)]
mod tests {
    use super::preparing_stage_text;

    #[test]
    fn preparing_text_names_the_computer_and_counts() {
        assert_eq!(
            preparing_stage_text("Studio PC", 12, 40),
            "Preparing Studio PC's graphics card\u{2026} 12 of 40"
        );
    }

    #[test]
    fn preparing_text_drops_unknown_counts_and_empty_names() {
        assert_eq!(
            preparing_stage_text("Studio PC", 3, 0),
            "Preparing Studio PC's graphics card\u{2026}"
        );
        assert_eq!(
            preparing_stage_text("", 0, 0),
            "Preparing the remote computer's graphics card\u{2026}"
        );
    }
}
