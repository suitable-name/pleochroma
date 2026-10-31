//! Cancels obsolete live work when a context mutation releases its lock.

use super::RenderContext;
use crate::settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale};
use pleochroma::renderer::engine::EngineSpec;
use std::{
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

/// A context lock that invalidates render/display work after relevant edits.
///
/// The token is captured when the lock is acquired. A snapshot can therefore replace
/// it without cancelling its newly installed token when this guard is dropped.
pub struct RenderContextGuard<'a> {
    guard: MutexGuard<'a, RenderContext>,
    cancel: Arc<AtomicBool>,
    before: Activity,
}

impl Deref for RenderContextGuard<'_> {
    type Target = RenderContext;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for RenderContextGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for RenderContextGuard<'_> {
    fn drop(&mut self) {
        self.guard.discard_stale_view_color_pattern();
        let after = Activity::capture(&self.guard);
        if self.before.scene_changed(&after) {
            self.guard.dirty = true;
        }
        if self.guard.dirty || self.before != after {
            self.cancel.store(true, Ordering::Release);
            // A pause, resume or compute-policy edit may not invalidate accumulated
            // radiance, but any discarded display request still needs a replacement.
            self.guard.redisplay_requested = true;
        }
    }
}

impl RenderContext {
    /// Locks the live context and cancels obsolete work when edits are committed.
    ///
    /// A poisoned mutex is recovered consistently with the rest of the UI: the
    /// context contains plain state, and a later edit replaces any partial update.
    pub fn lock(ctx: &Mutex<Self>) -> super::RenderContextGuard<'_> {
        let guard = ctx.lock().unwrap_or_else(PoisonError::into_inner);
        let before = Activity::capture(&guard);
        let cancel = Arc::clone(&guard.frame_cancel);
        RenderContextGuard {
            guard,
            cancel,
            before,
        }
    }

    /// Reuses the current generation's token, replacing only invalidated work.
    pub(super) fn frame_cancellation(&mut self) -> Arc<AtomicBool> {
        if self.dirty || self.frame_cancel.load(Ordering::Acquire) {
            self.frame_cancel.store(true, Ordering::Release);
            self.frame_cancel = Arc::new(AtomicBool::new(false));
        }
        Arc::clone(&self.frame_cancel)
    }
}

/// Cheap state that can change without a scene's usual `dirty` marker.
///
/// Material and geometry edits already set `dirty`. Pointer identities catch a
/// replaced scene buffer without hashing its contents on every GUI lock.
#[derive(PartialEq)]
struct Activity {
    dimensions: (u32, u32),
    pose: [u32; 6],
    quality: (u32, u32),
    engine: EngineSpec,
    compute: (LiveComputeTarget, LocalComputeTarget),
    preview: LocalPreviewScale,
    switches: u16,
    buffers: [usize; 5],
}

impl Activity {
    fn scene_changed(&self, after: &Self) -> bool {
        self.dimensions != after.dimensions
            || self.pose != after.pose
            || self.quality.1 != after.quality.1
            || self.engine != after.engine
            || self.buffers[..4] != after.buffers[..4]
    }

    fn capture(ctx: &RenderContext) -> Self {
        let switches = [
            ctx.running,
            ctx.effective_paused(),
            ctx.tab_visible,
            ctx.denoise_enabled,
            ctx.remote_active,
            ctx.export_active(),
            ctx.camera_moving,
            ctx.camera_drag_held,
            ctx.live_display_only,
        ]
        .into_iter()
        .enumerate()
        .fold(0, |bits, (shift, enabled)| {
            bits | (u16::from(enabled) << shift)
        });
        Self {
            dimensions: (ctx.width, ctx.height),
            pose: [
                ctx.yaw.to_bits(),
                ctx.pitch.to_bits(),
                ctx.camera_roll.to_bits(),
                ctx.distance.to_bits(),
                ctx.light_yaw.to_bits(),
                ctx.light_pitch.to_bits(),
            ],
            quality: (ctx.target_samples, ctx.max_bounces),
            engine: ctx.engine,
            compute: (ctx.live_compute_target, ctx.local_compute_target),
            preview: ctx.local_preview_scale,
            switches,
            buffers: [
                Arc::as_ptr(&ctx.active_planes) as usize,
                Arc::as_ptr(&ctx.active_tools) as usize,
                Arc::as_ptr(&ctx.active_frosted) as usize,
                ctx.env_map
                    .as_ref()
                    .map_or(0, |map| Arc::as_ptr(map) as usize),
                ctx.live_epoch
                    .as_ref()
                    .map_or(0, |epoch| Arc::as_ptr(epoch) as usize),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::context::frame::snapshot_frame_inputs;

    fn context() -> Arc<Mutex<RenderContext>> {
        Arc::new(Mutex::new(RenderContext::default()))
    }

    #[test]
    fn unchanged_snapshots_and_reads_keep_the_same_live_token() {
        let ctx = context();
        let first = snapshot_frame_inputs(&ctx).cancel;
        assert!(!first.load(Ordering::Acquire));
        let _width = RenderContext::lock(&ctx).width;
        let next = snapshot_frame_inputs(&ctx).cancel;
        assert!(Arc::ptr_eq(&first, &next));
        assert!(!next.load(Ordering::Acquire));
    }

    #[test]
    fn a_scene_edit_cancels_the_old_token_without_cancelling_its_replacement() {
        let ctx = context();
        let old = snapshot_frame_inputs(&ctx).cancel;
        {
            let mut guard = RenderContext::lock(&ctx);
            guard.material_name = "Rutile".into();
            guard.dirty = true;
        }
        assert!(old.load(Ordering::Acquire));
        let next = snapshot_frame_inputs(&ctx);
        assert!(next.dirty);
        assert!(!Arc::ptr_eq(&old, &next.cancel));
        assert!(!next.cancel.load(Ordering::Acquire));
        assert!(old.load(Ordering::Acquire));
    }

    #[test]
    fn pause_and_resume_cancel_pending_work_without_resetting_accumulation() {
        let ctx = context();
        let running = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).paused = true;
        assert!(running.load(Ordering::Acquire));
        let paused = snapshot_frame_inputs(&ctx);
        assert!(!paused.dirty);
        assert!(paused.redisplay_requested);
        RenderContext::lock(&ctx).paused = false;
        assert!(paused.cancel.load(Ordering::Acquire));
        let resumed = snapshot_frame_inputs(&ctx);
        assert!(!resumed.dirty);
        assert!(!resumed.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn motion_preview_and_compute_changes_cancel_without_needing_dirty() {
        let ctx = context();
        let moving = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).camera_drag_held = true;
        assert!(moving.load(Ordering::Acquire));
        let preview = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).local_preview_scale = LocalPreviewScale::Quarter;
        assert!(preview.load(Ordering::Acquire));
        let compute = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).local_compute_target = LocalComputeTarget::Gpu;
        assert!(compute.load(Ordering::Acquire));
        let hidden = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).tab_visible = false;
        assert!(hidden.load(Ordering::Acquire));
    }

    #[test]
    fn changing_pose_also_marks_old_accumulation_dirty() {
        let ctx = context();
        let first = snapshot_frame_inputs(&ctx).cancel;
        RenderContext::lock(&ctx).yaw += 0.2;
        assert!(first.load(Ordering::Acquire));
        let next = snapshot_frame_inputs(&ctx);
        assert!(next.dirty);
        assert!(!next.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn sample_target_edits_cancel_only_unfinished_work_and_keep_the_scene_identity() {
        let ctx = context();
        let first = snapshot_frame_inputs(&ctx);
        RenderContext::lock(&ctx).target_samples = 1024;
        assert!(first.cancel.load(Ordering::Acquire));
        let raised = snapshot_frame_inputs(&ctx);
        assert!(!raised.dirty);
        assert!(raised.redisplay_requested);
        assert_eq!(raised.scene_generation, first.scene_generation);
        assert_eq!(raised.target_samples, 1024);
        RenderContext::lock(&ctx).target_samples = 8;
        assert!(raised.cancel.load(Ordering::Acquire));
        let lowered = snapshot_frame_inputs(&ctx);
        assert!(!lowered.dirty);
        assert!(lowered.redisplay_requested);
        assert_eq!(lowered.scene_generation, first.scene_generation);
        assert!(!lowered.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn image_roll_invalidates_the_image_without_changing_orbit_or_zoom() {
        let ctx = context();
        let original = snapshot_frame_inputs(&ctx);
        RenderContext::lock(&ctx).camera_roll = std::f32::consts::FRAC_PI_2;
        assert!(original.cancel.load(Ordering::Acquire));
        let rolled = snapshot_frame_inputs(&ctx);
        assert!(rolled.dirty);
        assert_ne!(rolled.scene_generation, original.scene_generation);
        assert_eq!(
            (rolled.yaw, rolled.pitch, rolled.distance),
            (original.yaw, original.pitch, original.distance)
        );
    }
}
