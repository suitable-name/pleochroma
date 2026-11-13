//! Adaptive (live-tile) passes through [`GpuBackend`]: the entry point beside the
//! `try_accumulate_geom*` family.
//!
//! A pass adds samples `sample_offset..sample_offset + spp` to every pixel of the live tiles of
//! a [`LiveTiles`], into the caller's summed accumulator and into half-buffer B (the samples
//! with an odd absolute index only); pixels outside the live tiles are left untouched in both.
//! It runs under the same fairness rules as every other request: turn by turn through the
//! turnstile, into private scratch sums that reach the caller only when the whole pass is
//! done, so a device loss or a cancellation on a later turn never leaves half a pass behind.
//!
//! The scratch sums are compact (one entry per live pixel), so memory scales with the live
//! pixels, not with the frame size.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use glam::Vec3;

use super::GpuBackend;
use crate::{
    geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive},
    optics::raytracer::surface::SurfaceScene,
    renderer::{
        adaptive::LiveTiles,
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameError, GpuFrameRenderer, GpuFrameScene,
            frame::{
                ChunkCursor, ChunkTurnOutcome, LivePass, LiveSums, TurnRequest, classify_material,
            },
        },
        gpu_backend::{GpuAccumulate, GpuSceneRef, turnstile::CHUNKS_PER_TURN},
    },
};

impl GpuBackend {
    /// [`Self::try_accumulate_geom_surface_cancellable`] restricted to the live tiles of `live`,
    /// with the odd-sample half-buffer: one adaptive sampling pass.
    ///
    /// Adds the samples `sample_offset..sample_offset + spp` (absolute indices) to `accum` at
    /// every pixel of every live tile, and the samples with an ODD absolute index only to
    /// `half_b` at the same pixels. Pixels outside the live tiles are left untouched in both.
    /// A pixel draws exactly the samples it would in an ordinary pass, so
    /// `LiveTiles::all` equals [`Self::try_accumulate_geom_surface`] over the same range, and
    /// `accum - half_b` is the even-index half.
    ///
    /// Declines (`accum` and `half_b` untouched, [`GpuAccumulate::Declined`]) wherever the
    /// ordinary surface entry does, and also when `live` does not describe this frame, when the
    /// renderer is set to the legacy wavefront kernels, or when the device cannot bind the live
    /// pixel list. [`GpuAccumulate::Cancelled`] likewise leaves both untouched.
    ///
    /// # Panics
    ///
    /// Panics if `accum` or `half_b` is not `scene.width * scene.height` long.
    #[expect(
        clippy::too_many_arguments,
        reason = "the cancellable surface entry point's parameter list plus the live tiles and the half-buffer"
    )]
    pub fn try_accumulate_geom_surface_live_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        accum: &mut [Vec3],
        half_b: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        assert_eq!(
            half_b.len(),
            num_pixels,
            "half-buffer B must have one entry per pixel"
        );
        if live.width() != scene.width || live.height() != scene.height {
            tracing::debug!("GPU declined: the live tiles describe another frame size");
            return GpuAccumulate::Declined;
        }
        if tools.len() > GPU_MAX_TOOL_PRIMITIVES {
            tracing::debug!(tools = tools.len(), "GPU declined: too many concave tools");
            return GpuAccumulate::Declined;
        }
        if !self.work_admits(scene, spp) {
            return GpuAccumulate::Declined;
        }
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &self.renderer else {
            return GpuAccumulate::Declined;
        };
        if spp == 0 || live.is_empty() {
            return GpuAccumulate::Done;
        }

        let gpu_scene = GpuFrameScene {
            camera: scene.camera,
            width: scene.width,
            height: scene.height,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            max_bounces: scene.max_bounces,
            environment: scene.environment,
        };
        let gpu_surface = GpuSurfaceScene::from_scene(surface);
        let request = TurnRequest {
            scene: &gpu_scene,
            tools,
            pipeline_class: classify_material(gpu_scene.material),
            sample_offset,
            spp,
            cancel: Some(cancel),
            max_chunks: self
                .configured_limits()
                .map_or(CHUNKS_PER_TURN, |limits| limits.chunks_per_turn),
            surface: Some(&gpu_surface),
            estimator: scene.estimator,
        };

        let Some(pass) = self.begin_live_pass(mutex, live) else {
            return GpuAccumulate::Declined;
        };
        let live_count = pass.pixels().len();
        let mut scratch_sum = self.take_scratch(live_count);
        let mut scratch_half = self.take_scratch(live_count);
        let outcome = self.run_live_turns(
            mutex,
            &request,
            &pass,
            &mut LiveSums {
                sum: &mut scratch_sum,
                half_b: &mut scratch_half,
            },
        );
        if outcome == GpuAccumulate::Done {
            for (virtual_pixel, &pixel) in pass.pixels().iter().enumerate() {
                let pixel = pixel as usize;
                accum[pixel] += scratch_sum[virtual_pixel];
                half_b[pixel] += scratch_half[virtual_pixel];
            }
        }
        self.return_scratch(scratch_sum);
        self.return_scratch(scratch_half);
        outcome
    }

    /// Uploads the live pixel list under the renderer lock; `None` (a decline) if the renderer
    /// is unusable or the list does not fit the device.
    pub(super) fn begin_live_pass(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        live: &LiveTiles,
    ) -> Option<LivePass> {
        let renderer = self.lock_renderer(mutex)?;
        match renderer.begin_live_pass(live) {
            Ok(pass) => Some(pass),
            Err(error) => {
                tracing::debug!("GPU declined this adaptive pass: {error}");
                None
            }
        }
    }

    /// [`Self::begin_live_pass`] for an explicit list of real pixel indices: the pixels a path
    /// cache does not shade. `None` (a decline) if the renderer is unusable or the list does not
    /// fit the device.
    pub(super) fn begin_pixel_pass(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        pixels: Vec<u32>,
    ) -> Option<LivePass> {
        let renderer = self.lock_renderer(mutex)?;
        match renderer.begin_pixel_pass(pixels) {
            Ok(pass) => Some(pass),
            Err(error) => {
                tracing::debug!("GPU declined this pixel-list pass: {error}");
                None
            }
        }
    }

    /// Runs `request` over `pass` turn by turn, summing into `sums`, until it completes, is
    /// cancelled or declines. `sums` hold partial sums after any outcome but `Done`; the caller
    /// discards them. The turn loop of `run_turns`, for a live pass.
    pub(super) fn run_live_turns(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        pass: &LivePass,
        sums: &mut LiveSums<'_>,
    ) -> GpuAccumulate {
        let mut cursor = ChunkCursor::default();
        loop {
            // FIFO admission across every caller sharing this backend, a ticket per turn.
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);
            // Re-checked after waiting: the turn ahead may have lost the device.
            if self.lost.load(Ordering::Acquire) {
                return GpuAccumulate::Declined;
            }
            let Some(mut renderer) = self.lock_renderer(mutex) else {
                return GpuAccumulate::Declined;
            };
            if let Some(limits) = self.configured_limits() {
                renderer.set_chunk_budget_bytes(limits.chunk_budget_bytes);
            }
            let outcome = renderer.accumulate_live_turn(request, pass, sums, &mut cursor);
            drop(renderer);

            match outcome {
                Ok(ChunkTurnOutcome::Done) => return GpuAccumulate::Done,
                Ok(ChunkTurnOutcome::Cancelled) => return GpuAccumulate::Cancelled,
                Ok(ChunkTurnOutcome::MoreWork) => {}
                Err(GpuFrameError::DeviceLost(why)) => {
                    tracing::warn!(
                        "GPU device lost ({why}), disabling the GPU backend until a fresh \
                         device is acquired -- meanwhile every call falls back to the CPU \
                         tracer"
                    );
                    self.mark_lost(why);
                    return GpuAccumulate::Declined;
                }
                Err(error) => {
                    tracing::debug!(
                        "GPU declined this adaptive pass, using the CPU tracer: {error}"
                    );
                    return GpuAccumulate::Declined;
                }
            }
        }
    }
}
