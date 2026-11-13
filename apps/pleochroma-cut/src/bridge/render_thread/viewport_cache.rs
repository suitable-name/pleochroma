//! The path cache of the live viewport (Preferences > Rendering > "Speed Up Still Views With a
//! Path Cache"): memory only, built for a pose that has stood still.
//!
//! # When it lives
//!
//! While the stone moves there is no cache. When a still view (the final phase, the path-traced
//! engine) has refined for [`VIEWPORT_CACHE_AFTER_SPP`] progressive samples, [`ViewportCache`]
//! builds the cache of that pose in system RAM (phase A on every core the viewport may use), and
//! the rest of the refinement shades the stone pixels from it (phase B), with or without the
//! adaptive viewport. Any reset of the progressive accumulation (a pose, design, material,
//! lighting, resolution or engine change, a preview phase, the preference itself) drops it at once,
//! and a stone the cache cannot serve (see `PathCacheOff`) or one that would not fit
//! [`VIEWPORT_CACHE_CAP_BYTES`] is refused for the rest of that accumulation, with the ordinary
//! refinement carrying on. Nothing is ever written to disk.
//!
//! # Tiles, not a frame
//!
//! A `PathCacheFrame` borrows the scene of the frame it was built for, and the viewport rebuilds
//! its frame description every pass, so what lives between passes is the built tiles
//! ([`ViewportCache::with_pass_frame`] hands them to a fresh frame of the pass and takes them
//! back). The frame is cheap to make: it builds no tile.
//!
//! # One engine per pass
//!
//! A pass runs on the GPU's cached live entry (the cache streams to it from system RAM, a slab at
//! a time) when the compute choice allows and the backend takes it, and on the CPU otherwise.

use super::{
    engine::frame_scene,
    gpu_backend::{BackendFrame, ViewportGpu},
    local_preview::RenderPhase,
};
use crate::{
    bridge::export_thread::path_cache_memory::memory_hints, settings::model::LocalComputeTarget,
};
use glam::Vec3;
use pleochroma::{
    optics::raytracer::{
        Estimator, build_plane_soa, default_estimator, transport::PreparedMaterial,
    },
    renderer::{
        adaptive::LiveTiles,
        cpu_frame::{scatter_interleaved, trace_pixels_interleaved_surface_cached},
        gpu_backend::{GpuAccumulate, memory_budget},
        path_cache::{
            BuildSettings, CacheOptions, PathCacheFrame, TileCache, estimate_size, placement::plan,
        },
        tonemap::live_cpu_budget,
    },
};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
};

/// Progressive samples a still pose needs before its cache is built.
pub const VIEWPORT_CACHE_AFTER_SPP: u32 = 64;

/// The most memory the viewport's cache may take, in bytes (4 GiB).
pub const VIEWPORT_CACHE_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Whether the viewport may use a path cache this frame: the preference is on, the engine is the
/// path tracer, the view is still (the final phase) and the process estimator is branching
/// (see [`applies_with`]).
#[must_use]
pub(super) fn applies(enabled: bool, ray_traced: bool, phase: RenderPhase) -> bool {
    applies_with(enabled, ray_traced, phase, default_estimator())
}

/// [`applies`] for an explicit `estimator`: exit collection never uses the path cache, so it is
/// refused under it whatever the preference says.
#[must_use]
pub(super) const fn applies_with(
    enabled: bool,
    ray_traced: bool,
    phase: RenderPhase,
    estimator: Estimator,
) -> bool {
    enabled
        && !ray_traced
        && matches!(phase, RenderPhase::Final)
        && matches!(estimator, Estimator::Branching)
}

/// What [`ViewportCache::sync`] needs to know about this frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct CacheWanted {
    /// [`applies`] for this frame.
    pub(super) applies: bool,
    /// The progressive accumulation was reset this frame (a scene change or a new size).
    pub(super) reset: bool,
    /// The samples per pixel the ordinary accumulation holds.
    pub(super) samples: u32,
    /// The frame size.
    pub(super) size: (u32, u32),
}

/// How a cached pass ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CacheOutcome {
    /// The samples are in the accumulator.
    Done,
    /// The frame's cancellation fired; the accumulator is untouched.
    Cancelled,
    /// There is no cache to shade from; the ordinary pass runs.
    Unavailable,
}

/// The built tiles of one pose.
struct Built {
    tiles: Vec<TileCache>,
    size: (u32, u32),
    /// The bytes of the tiles, for the tests ([`ViewportCache::built_bytes`]).
    #[cfg(test)]
    bytes: u64,
}

/// Where the cache of the current accumulation stands.
enum Stage {
    /// Not built (yet).
    Idle,
    /// Built and ready to shade.
    Built(Built),
    /// Refused for this accumulation: ineligible stone, too large, or no room.
    Refused,
}

/// The viewport's path cache.
pub(super) struct ViewportCache {
    stage: Stage,
    /// The GPU declined a cached pass: the rest of this cache's life is the CPU's.
    gpu_declined: bool,
}

impl Default for ViewportCache {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            gpu_declined: false,
        }
    }
}

impl ViewportCache {
    /// Brings the cache in step with this frame and says whether it should be built or used: the
    /// path cache applies, the still pose has refined for [`VIEWPORT_CACHE_AFTER_SPP`] samples and
    /// the stone was not refused.
    ///
    /// A reset of the accumulation, a frame the cache does not apply to or a new size drops
    /// whatever was built, and the refusal with it.
    pub(super) fn sync(&mut self, want: CacheWanted) -> bool {
        let size_changed = matches!(&self.stage, Stage::Built(built) if built.size != want.size);
        if !want.applies || want.reset || size_changed {
            self.stage = Stage::Idle;
            self.gpu_declined = false;
        }
        want.applies
            && want.samples >= VIEWPORT_CACHE_AFTER_SPP
            && !matches!(self.stage, Stage::Refused)
    }

    /// Whether a cache is built.
    pub(super) const fn is_built(&self) -> bool {
        matches!(self.stage, Stage::Built(_))
    }

    /// Whether the stone was refused for this accumulation.
    #[cfg(test)]
    pub(super) const fn is_refused(&self) -> bool {
        matches!(self.stage, Stage::Refused)
    }

    /// The bytes of the built tiles; zero without a cache.
    #[cfg(test)]
    pub(super) const fn built_bytes(&self) -> u64 {
        match &self.stage {
            Stage::Built(built) => built.bytes,
            _ => 0,
        }
    }

    /// Builds the cache of `frame`'s pose when none is built: eligibility, the size estimate and the
    /// placement rules (cap [`VIEWPORT_CACHE_CAP_BYTES`]), then phase A of every tile on the
    /// viewport's tracing threads. Returns whether a cache is built afterwards.
    ///
    /// A raised `cancel` ends the build and discards it (the next frame starts it again); a stone
    /// that cannot use the cache, or does not fit, is refused until the accumulation resets.
    pub(super) fn ensure_built(&mut self, frame: &BackendFrame<'_>, cancel: &AtomicBool) -> bool {
        if frame.environment.tray_shadow().is_some() {
            // Not a refusal: the cache is built once the shadow is off. `PathCacheFrame::new`
            // refuses a shadow map on its own, pass by pass.
            tracing::debug!(
                reason = "the stone shadow needs exit positions the cache does not keep",
                "viewport path cache: not built"
            );
            return false;
        }
        if frame.environment.tray_mirror().is_some() {
            // Not a refusal, as for the stone shadow: the tray reflection needs paths the cache
            // does not keep.
            tracing::debug!(
                reason = "the tray reflection needs paths the cache does not keep",
                "viewport path cache: not built"
            );
            return false;
        }
        match self.stage {
            Stage::Built(_) => return true,
            Stage::Refused => return false,
            Stage::Idle => {}
        }
        let scene = frame_scene(frame);
        let soa = build_plane_soa(frame.planes);
        let settings = BuildSettings::default();
        let options = CacheOptions {
            settings,
            depth_of_field: false,
            budget_bytes: Some(VIEWPORT_CACHE_CAP_BYTES),
        };
        let extras = (frame.tools, frame.fluorescence, frame.surface);
        let cache = match PathCacheFrame::new(&scene, &soa, extras, options) {
            Ok(cache) => cache,
            Err(off) => {
                tracing::debug!(reason = off.message(), "viewport path cache: refused");
                self.stage = Stage::Refused;
                return false;
            }
        };
        let prepared = PreparedMaterial::new(scene.material);
        let estimate = estimate_size(&scene, &prepared, &soa, &settings);
        let budget = memory_budget(memory_hints(), frame.width, frame.height);
        let (placement, line) = plan(&estimate, &budget);
        if estimate.raw_bytes > VIEWPORT_CACHE_CAP_BYTES || !placement.is_cached() {
            tracing::debug!(%line, "viewport path cache: no room, sampling as usual");
            self.stage = Stage::Refused;
            return false;
        }

        let _lease = live_cpu_budget::single_core_lease();
        let _trace_lease = live_cpu_budget::trace_lease();
        let count = cache.tile_count();
        let threads = live_cpu_budget::partition().0.min(count).max(1);
        let next = AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    while !cancel.load(Ordering::Relaxed) {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= count {
                            break;
                        }
                        let _ = cache.tile(u32::try_from(index).unwrap_or(u32::MAX));
                    }
                });
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let bytes = u64::try_from(cache.built_bytes()).unwrap_or(u64::MAX);
        if bytes > VIEWPORT_CACHE_CAP_BYTES {
            tracing::debug!(bytes, "viewport path cache: larger than its cap, dropped");
            self.stage = Stage::Refused;
            return false;
        }
        self.stage = Stage::Built(Built {
            tiles: cache.into_tiles(),
            size: (frame.width, frame.height),
            #[cfg(test)]
            bytes,
        });
        true
    }

    /// Runs `run` with a path-cache frame of this pass (the built tiles moved into it and back
    /// out), or with `None` when no cache is built.
    pub(super) fn with_pass_frame<R>(
        &mut self,
        frame: &BackendFrame<'_>,
        run: impl FnOnce(Option<&PathCacheFrame<'_, '_>>) -> R,
    ) -> R {
        let Stage::Built(built) = &mut self.stage else {
            return run(None);
        };
        let scene = frame_scene(frame);
        let soa = build_plane_soa(frame.planes);
        let options = CacheOptions {
            settings: BuildSettings::default(),
            depth_of_field: false,
            budget_bytes: Some(VIEWPORT_CACHE_CAP_BYTES),
        };
        let extras = (frame.tools, frame.fluorescence, frame.surface);
        let Ok(cache) = PathCacheFrame::new(&scene, &soa, extras, options) else {
            // The pass no longer qualifies (it did when the tiles were built): drop them.
            self.stage = Stage::Refused;
            return run(None);
        };
        // Every tile came out of a frame of this same pose and size, so all of them are taken.
        let _ = cache.preload(std::mem::take(&mut built.tiles));
        let result = run(Some(&cache));
        built.tiles = cache.into_tiles();
        result
    }

    /// [`Self::accumulate`] with the engine the compute choice allows: an explicit GPU choice waits
    /// for the shader, the combined choice never holds up a pass for it and uses the CPU until the
    /// GPU is ready. `buffers` is `(accum, scratch)`.
    pub(super) fn accumulate_on(
        &mut self,
        backend: &mut ViewportGpu,
        compute: LocalComputeTarget,
        frame: &BackendFrame<'_>,
        buffers: (&mut [Vec3], &mut Vec<Vec3>),
        cancel: &AtomicBool,
    ) -> CacheOutcome {
        let gpu = match compute {
            LocalComputeTarget::Cpu => None,
            LocalComputeTarget::Gpu => Some((backend, true)),
            LocalComputeTarget::CpuGpu => Some((backend, false)),
        };
        let (accum, scratch) = buffers;
        self.accumulate(gpu, frame, accum, scratch, cancel)
    }

    /// Adds the samples `frame.sample_offset..frame.sample_offset + frame.spp` of every pixel to
    /// `accum`, shading the stone from the cache: on the GPU when `gpu` is given (with whether to
    /// wait for its preparation) and it takes the pass, else on the CPU. `scratch` is half-buffer
    /// B of the GPU entry, which the viewport has no use for.
    pub(super) fn accumulate(
        &mut self,
        gpu: Option<(&mut ViewportGpu, bool)>,
        frame: &BackendFrame<'_>,
        accum: &mut [Vec3],
        scratch: &mut Vec<Vec3>,
        cancel: &AtomicBool,
    ) -> CacheOutcome {
        let gpu = gpu.filter(|_| !self.gpu_declined);
        let mut declined = false;
        let outcome = self.with_pass_frame(frame, |cache| {
            let Some(cache) = cache else {
                return CacheOutcome::Unavailable;
            };
            if let Some((backend, wait)) = gpu {
                if scratch.len() != accum.len() {
                    *scratch = vec![Vec3::ZERO; accum.len()];
                }
                let live = LiveTiles::all(frame.width, frame.height, cache.settings().tile);
                match backend.try_accumulate_cached_live_cancellable(
                    frame,
                    (frame.sample_offset, frame.spp),
                    &live,
                    cache,
                    (&mut *accum, scratch.as_mut_slice()),
                    wait,
                    cancel,
                ) {
                    Some(GpuAccumulate::Done) => return CacheOutcome::Done,
                    Some(GpuAccumulate::Cancelled) => return CacheOutcome::Cancelled,
                    Some(GpuAccumulate::Declined) => declined = true,
                    // The combined choice's GPU is still preparing: this pass is the CPU's.
                    None => {}
                }
            }
            accumulate_cpu(frame, cache, accum, cancel)
        });
        self.gpu_declined |= declined;
        outcome
    }
}

/// The CPU pass: every tracing thread takes every `stride`-th pixel and the sums are scattered
/// back; a cancel raised during the pass discards it.
fn accumulate_cpu(
    frame: &BackendFrame<'_>,
    cache: &PathCacheFrame<'_, '_>,
    accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> CacheOutcome {
    if cancel.load(Ordering::Relaxed) {
        return CacheOutcome::Cancelled;
    }
    let _lease = live_cpu_budget::single_core_lease();
    let _trace_lease = live_cpu_budget::trace_lease();
    let stride = u32::try_from(live_cpu_budget::partition().0.max(1)).unwrap_or(1);
    let scene = frame_scene(frame);
    let soa = build_plane_soa(frame.planes);
    let extras = (frame.tools, frame.fluorescence, frame.surface);
    let samples = (frame.sample_offset, frame.spp);
    let (scene, soa) = (&scene, &soa);
    let parts: Vec<(u32, Vec<Vec3>)> = thread::scope(|scope| {
        // All threads must be running before the first join, so the spawns are collected eagerly.
        let mut handles = Vec::new();
        for first in 0..stride {
            handles.push(scope.spawn(move || {
                let sums = trace_pixels_interleaved_surface_cached(
                    scene,
                    extras,
                    soa,
                    (first, stride),
                    samples,
                    Some(cache),
                );
                (first, sums)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return CacheOutcome::Cancelled;
    }
    for (first, sums) in parts {
        scatter_interleaved(accum, first, stride, &sums);
    }
    CacheOutcome::Done
}

#[cfg(test)]
mod tests;
