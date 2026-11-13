//! Live passes with a path cache through [`GpuBackend`]: the entry point beside
//! `try_accumulate_geom_surface_live_cancellable`.
//!
//! A pass adds the samples `sample_offset..sample_offset + spp` to every pixel of the live tiles of
//! a [`LiveTiles`], into the summed accumulator and into half-buffer B (the odd absolute samples
//! only). The cached stone pixels of the live tiles shade in the phase B kernel, one slab of the
//! cache at a time; every other live pixel goes to the ordinary live path-tracer pass with an
//! explicit pixel list, so each live pixel is traced exactly once.
//!
//! Like the other entry points, the pass runs turn by turn under the turnstile into private
//! scratch sums, and the caller's buffers change only when the whole pass is done. Between turns
//! the calling thread prepares the next slab (phase A on worker threads runs ahead of it). The
//! fired samples of a slab are completed on the GPU inside each chunk; only a renderer that cannot
//! run the completion pass hands them back, and the calling thread completes them after the slab's
//! turns (`GpuPathCache::complete_residuals`).

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use glam::Vec3;

use super::GpuBackend;
use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::surface::SurfaceScene,
    renderer::{
        adaptive::LiveTiles,
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameError, GpuFrameRenderer, GpuFrameScene,
            frame::{
                CachedSlabJob, CachedSlabProgress, ChunkCursor, ChunkTurnOutcome, LiveSums,
                SlabGeometry, TurnRequest, classify_material,
            },
        },
        gpu_backend::{
            GpuAccumulate, GpuPathCache, GpuSceneRef, HostSlab, turnstile::CHUNKS_PER_TURN,
        },
    },
};

/// The summed samples of the cached pixels of one slab, one entry per `pixels` entry.
struct SlabSums {
    pixels: Vec<u32>,
    sum: Vec<Vec3>,
    half_b: Vec<Vec3>,
}

/// Everything a whole cached pass summed, before it reaches the caller's buffers.
struct PassSums {
    slabs: Vec<SlabSums>,
    /// The live pixels the cache does not shade, and their sums from the path tracer.
    rest: SlabSums,
}

impl GpuBackend {
    /// [`Self::try_accumulate_geom_surface_live_cancellable`] with a path cache: the cached stone
    /// pixels of the live tiles shade in the GPU phase B kernel, every other live pixel on the
    /// live path tracer.
    ///
    /// Adds the samples `sample_offset..sample_offset + spp` (absolute indices) to `accum` at
    /// every live pixel, and the samples with an ODD absolute index only to `half_b`. Pixels
    /// outside the live tiles are left untouched in both. The cache stands for the path tracer's
    /// mean (see `renderer::path_cache`), so the result agrees with the uncached live pass in
    /// mean, not sample for sample.
    ///
    /// Declines (`accum` and `half_b` untouched, [`GpuAccumulate::Declined`]) exactly where the
    /// live entry does, and also when the stone has concave tools (the cache is off for them),
    /// when `live` or `cache` describe another frame size or tile size, or when a slab does not
    /// fit a storage binding of the device. [`GpuAccumulate::Cancelled`] likewise leaves both
    /// untouched.
    ///
    /// # Panics
    ///
    /// Panics if `accum` or `half_b` is not `scene.width * scene.height` long.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry point's parameter list plus the cache"
    )]
    pub fn try_accumulate_cached_live_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        cache: &GpuPathCache<'_, '_, '_>,
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
        if !tools.is_empty() {
            tracing::debug!("GPU declined: the path cache is off for concave stones");
            return GpuAccumulate::Declined;
        }
        if scene.environment.tray_shadow().is_some() {
            tracing::debug!("GPU declined: the cached live pass has no Stone Shadow phase");
            return GpuAccumulate::Declined;
        }
        if scene.environment.tray_mirror().is_some() {
            tracing::debug!("GPU declined: the cached live pass has no Tray Reflection phase");
            return GpuAccumulate::Declined;
        }
        if !cache.frame().matches(scene.width, scene.height)
            || cache.frame().settings().tile != live.tile()
        {
            tracing::debug!("GPU declined: the path cache describes another frame or tile size");
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
        let Some(&last_live) = live.indices().last() else {
            return GpuAccumulate::Done;
        };
        if spp == 0 {
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

        let started = Instant::now();
        let built_before = cache.frame().built_tiles();
        let result = cache.with_prefetch(last_live, || {
            self.run_cached_pass(mutex, &request, live, cache, last_live)
        });
        let sums = match result {
            Ok(sums) => sums,
            Err(outcome) => return outcome,
        };
        tracing::debug!(
            sample_offset,
            spp,
            live_tiles = live.indices().len(),
            slabs = sums.slabs.len(),
            tiles_built = cache.frame().built_tiles().saturating_sub(built_before),
            seconds = started.elapsed().as_secs_f64(),
            "path cache: cached pass"
        );
        for slab in &sums.slabs {
            add_sums(slab, accum, half_b);
        }
        add_sums(&sums.rest, accum, half_b);
        GpuAccumulate::Done
    }

    /// Drops the slabs of `cache` from the GPU (the video-memory tier keeps them between passes).
    pub fn release_path_cache(&self, cache: &GpuPathCache<'_, '_, '_>) {
        let Some(mutex) = &self.renderer else {
            return;
        };
        if let Some(mut renderer) = self.lock_renderer(mutex) {
            renderer.release_cached_slabs(cache.id());
        }
    }

    /// The largest slab, in bytes, this device can bind: the figure to build a
    /// [`GpuPathCache`] with (`with_slab_budget_bytes`). `None` without a usable GPU.
    #[must_use]
    pub fn path_cache_slab_limit(&self) -> Option<u64> {
        let mutex = self.renderer.as_ref()?;
        let renderer = self.lock_renderer(mutex)?;
        Some(renderer.storage_binding_limit())
    }

    /// The whole pass into private sums: every slab with cached pixels, then the live pixels the
    /// cache does not shade. `Err` carries the outcome that ended the pass early.
    fn run_cached_pass(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        live: &LiveTiles,
        cache: &GpuPathCache<'_, '_, '_>,
        last_live: u32,
    ) -> Result<PassSums, GpuAccumulate> {
        let geometry = SlabGeometry {
            tile: live.tile(),
            tiles_x: live.tiles_x(),
            width: live.width(),
        };
        let mut rest_pixels: Vec<u32> = Vec::new();
        let mut slabs = Vec::new();
        let mut index = 0_usize;
        let mut current = next_slab(cache, live, last_live, &mut index, &mut rest_pixels);
        while let Some(host) = current {
            let ahead = next_slab(cache, live, last_live, &mut index, &mut rest_pixels);
            let sums = self.run_slab(mutex, request, cache, (&host, ahead.as_ref()), geometry)?;
            slabs.push(sums);
            current = ahead;
        }

        let rest = self.run_rest(mutex, request, rest_pixels)?;
        Ok(PassSums { slabs, rest })
    }

    /// One slab: its turns (phase B, the completion of its fired samples, the reduce), then the host
    /// completion of any samples read back (only where the GPU cannot complete them).
    fn run_slab(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        cache: &GpuPathCache<'_, '_, '_>,
        (host, next): (&HostSlab<'_>, Option<&HostSlab<'_>>),
        geometry: SlabGeometry,
    ) -> Result<SlabSums, GpuAccumulate> {
        let count = host.pixels.len();
        let mut sum = vec![Vec3::ZERO; count];
        let mut half_b = vec![Vec3::ZERO; count];
        let mut progress = CachedSlabProgress::default();
        let mut cursor = ChunkCursor::default();
        let job = CachedSlabJob {
            cache_id: cache.id(),
            resident: cache.tier().keeps_slabs_resident(),
            slab: host,
            next,
            geometry,
            fire: host.has_residual,
            stage_delay: cache.stage_delay(),
            cpu_completion: cache.cpu_completion(),
        };
        let outcome = self.run_step_turns(mutex, |renderer| {
            renderer.accumulate_cached_slab_turn(
                request,
                &job,
                &mut progress,
                &mut LiveSums {
                    sum: &mut sum,
                    half_b: &mut half_b,
                },
                &mut cursor,
            )
        });
        if outcome != GpuAccumulate::Done {
            return Err(outcome);
        }
        let mut fired = std::mem::take(&mut progress.fired);
        for (virtual_pixel, sample, value) in
            cache.complete_residuals(host, geometry.width, &mut fired)
        {
            if !value.is_finite() {
                continue;
            }
            sum[virtual_pixel] += value;
            if !sample.is_multiple_of(2) {
                half_b[virtual_pixel] += value;
            }
        }
        Ok(SlabSums {
            pixels: host.pixels.clone(),
            sum,
            half_b,
        })
    }

    /// The live pixels the cache does not shade, on the ordinary live path tracer.
    fn run_rest(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        pixels: Vec<u32>,
    ) -> Result<SlabSums, GpuAccumulate> {
        if pixels.is_empty() {
            return Ok(SlabSums {
                pixels,
                sum: Vec::new(),
                half_b: Vec::new(),
            });
        }
        let Some(pass) = self.begin_pixel_pass(mutex, pixels.clone()) else {
            return Err(GpuAccumulate::Declined);
        };
        let mut sum = vec![Vec3::ZERO; pixels.len()];
        let mut half_b = vec![Vec3::ZERO; pixels.len()];
        let outcome = self.run_live_turns(
            mutex,
            request,
            &pass,
            &mut LiveSums {
                sum: &mut sum,
                half_b: &mut half_b,
            },
        );
        if outcome != GpuAccumulate::Done {
            return Err(outcome);
        }
        Ok(SlabSums {
            pixels,
            sum,
            half_b,
        })
    }

    /// Runs `turn` once per fairness turn until it completes, is cancelled or declines: the turn
    /// loop of `run_live_turns` around any step of the renderer.
    fn run_step_turns(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        mut turn: impl FnMut(&mut GpuFrameRenderer) -> Result<ChunkTurnOutcome, GpuFrameError>,
    ) -> GpuAccumulate {
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
            let outcome = turn(&mut renderer);
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
                    tracing::debug!("GPU declined this cached pass, using the CPU tracer: {error}");
                    return GpuAccumulate::Declined;
                }
            }
        }
    }
}

/// The next slab at or after `*index` that has cached live pixels, with `*index` moved past it.
/// The live pixels of the slabs on the way that the cache does not cover go to `rest`.
fn next_slab<'f>(
    cache: &GpuPathCache<'f, '_, '_>,
    live: &LiveTiles,
    last_live: u32,
    index: &mut usize,
    rest: &mut Vec<u32>,
) -> Option<HostSlab<'f>> {
    loop {
        if cache.slab(*index)?.first_tile > last_live {
            return None;
        }
        let mut host = cache.prepare_slab(*index, live)?;
        *index += 1;
        rest.append(&mut host.rest);
        if !host.pixels.is_empty() {
            return Some(host);
        }
    }
}

/// Adds compact sums into the caller's full-frame buffers at their real pixel indices.
fn add_sums(sums: &SlabSums, accum: &mut [Vec3], half_b: &mut [Vec3]) {
    for (virtual_pixel, &pixel) in sums.pixels.iter().enumerate() {
        let pixel = pixel as usize;
        accum[pixel] += sums.sum[virtual_pixel];
        half_b[pixel] += sums.half_b[virtual_pixel];
    }
}
