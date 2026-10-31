//! Change a stopping target while retaining the current full-data remote epoch.

use super::state::{Orchestrator, lock};
use crate::bridge::render_thread::RenderContext;
use std::sync::{Arc, Mutex};

pub(super) fn sync_target(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let target = {
        let ctx = RenderContext::lock(render_ctx);
        ctx.engine.target_samples(ctx.target_samples)
    };
    let restart = lock(state).live_lane.as_mut().and_then(|lane| {
        lane.set_target_samples(target)
            .then(|| Arc::clone(lane.epoch()))
    });
    if let Some(epoch) = restart {
        let mut ctx = RenderContext::lock(render_ctx);
        if ctx
            .live_epoch
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &epoch))
        {
            // Final-picture transfer has no radiance to merge. Keep the visible old
            // image until a fresh whole-budget request delivers its replacement.
            ctx.live_epoch = None;
            ctx.remote_active = false;
            ctx.dirty = true;
        }
    }
}
