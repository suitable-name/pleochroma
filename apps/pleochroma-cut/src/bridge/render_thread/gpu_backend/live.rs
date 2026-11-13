//! The GPU half of an adaptive viewport pass.
//!
//! It renders the samples of one `Pass` for the pixels of its live tiles, through the shared
//! backend's live entry points (the first is compiled only with the `adaptive-sampling`
//! feature), and the same with the viewport's path cache.

use super::{BackendFrame, GpuAccumulate, GpuSceneRef, ViewportGpu};
use glam::Vec3;
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::Pass;
use pleochroma::renderer::{
    adaptive::LiveTiles,
    gpu_backend::{CacheTier, GpuPathCache},
    path_cache::PathCacheFrame,
};
use std::sync::atomic::{AtomicBool, Ordering};

impl ViewportGpu {
    /// Adds the samples of `pass` for the pixels of its live tiles to `buffers.0` (the sum) and
    /// the odd-index ones to `buffers.1` (half-buffer B), on the GPU.
    ///
    /// `None` means the preparation worker has not finished and `wait` is `false`: the caller
    /// traces this pass on the CPU and asks again next pass. With `wait` the call holds until the
    /// preparation is done (or `cancel` is raised). `Declined` is final for this scene: a retired
    /// backend, a scene the GPU does not trace, no adapter, a lost device or a pass the live entry
    /// point does not take; the buffers are untouched then, as they are after `Cancelled`.
    #[cfg(feature = "adaptive-sampling")]
    pub(in crate::bridge::render_thread) fn try_accumulate_live_cancellable(
        &mut self,
        frame: &BackendFrame<'_>,
        pass: &Pass,
        buffers: (&mut [Vec3], &mut [Vec3]),
        wait: bool,
        cancel: &AtomicBool,
    ) -> Option<GpuAccumulate> {
        if cancel.load(Ordering::Relaxed) {
            return Some(GpuAccumulate::Cancelled);
        }
        if self.gpu_retired || !frame.routes_to_gpu() {
            return Some(GpuAccumulate::Declined);
        }
        if wait {
            if !self.wait_preparation(frame, cancel) {
                return Some(GpuAccumulate::Cancelled);
            }
        } else if !self.poll_preparation(frame) {
            return None;
        }
        let scene = GpuSceneRef {
            camera: frame.camera,
            width: frame.width,
            height: frame.height,
            planes: frame.planes,
            facet_finishes: frame.facet_finishes,
            material: frame.material,
            max_bounces: frame.max_bounces,
            environment: frame.environment,
            estimator: None,
        };
        let (sum, half_b) = buffers;
        let result = self.gpu.try_accumulate_geom_surface_live_cancellable(
            &scene,
            frame.tools,
            &frame.surface,
            pass.first_sample,
            pass.spp,
            &pass.live,
            sum,
            half_b,
            cancel,
        );
        match result {
            GpuAccumulate::Done => {
                self.last_gpu_ok = true;
                self.status_message = None;
            }
            GpuAccumulate::Declined => {
                self.last_gpu_ok = false;
                if self.gpu.is_lost() {
                    self.note_device_loss();
                }
            }
            GpuAccumulate::Cancelled => {}
        }
        Some(result)
    }

    /// The live entry point with the viewport's path cache: the cached stone pixels of the
    /// live tiles shade in the GPU's phase B kernel, every other live pixel on the live path
    /// tracer. `pass` is `(first_sample, spp)`.
    ///
    /// The cache streams from system RAM to the GPU one slab at a time on every call, so
    /// nothing stays resident between passes. Same answers as the uncached entry; the buffers
    /// are untouched on `Declined` and `Cancelled`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry's parameter list plus the cache and the live tiles"
    )]
    pub(in crate::bridge::render_thread) fn try_accumulate_cached_live_cancellable(
        &mut self,
        frame: &BackendFrame<'_>,
        pass: (u32, u32),
        live: &LiveTiles,
        cache: &PathCacheFrame<'_, '_>,
        buffers: (&mut [Vec3], &mut [Vec3]),
        wait: bool,
        cancel: &AtomicBool,
    ) -> Option<GpuAccumulate> {
        if cancel.load(Ordering::Relaxed) {
            return Some(GpuAccumulate::Cancelled);
        }
        if self.gpu_retired || !frame.routes_to_gpu() {
            return Some(GpuAccumulate::Declined);
        }
        if wait {
            if !self.wait_preparation(frame, cancel) {
                return Some(GpuAccumulate::Cancelled);
            }
        } else if !self.poll_preparation(frame) {
            return None;
        }
        let scene = GpuSceneRef {
            camera: frame.camera,
            width: frame.width,
            height: frame.height,
            planes: frame.planes,
            facet_finishes: frame.facet_finishes,
            material: frame.material,
            max_bounces: frame.max_bounces,
            environment: frame.environment,
            estimator: None,
        };
        let mut slab_cache = GpuPathCache::new(cache, CacheTier::SystemRam);
        if let Some(limit) = self.gpu.path_cache_slab_limit() {
            slab_cache = slab_cache
                .with_slab_budget_bytes(CacheTier::SystemRam.slab_budget_bytes().min(limit));
        }
        let (sum, half_b) = buffers;
        let result = self.gpu.try_accumulate_cached_live_cancellable(
            &scene,
            frame.tools,
            &frame.surface,
            pass.0,
            pass.1,
            live,
            &slab_cache,
            sum,
            half_b,
            cancel,
        );
        match result {
            GpuAccumulate::Done => {
                self.last_gpu_ok = true;
                self.status_message = None;
            }
            GpuAccumulate::Declined => {
                self.last_gpu_ok = false;
                if self.gpu.is_lost() {
                    self.note_device_loss();
                }
            }
            GpuAccumulate::Cancelled => {}
        }
        Some(result)
    }
}
