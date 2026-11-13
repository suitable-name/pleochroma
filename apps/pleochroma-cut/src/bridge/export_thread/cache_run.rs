//! The path cache of one export render: the decision, the cache a render shades from, and its
//! copy on disk.
//!
//! # The decision
//!
//! [`decide`] runs once per render, before any sample, when the render asks for a cache
//! (`RenderTaps::path_cache`) with the path-traced engine:
//!
//! 1. The stone must be eligible (`PathCacheFrame::new`). If not, the render is traced without a
//!    cache and the reason (`PathCacheOff::message`) is one progress note.
//! 2. A render that does not keep its cache (`PathCacheSettings::persist` off) builds none and
//!    notes `placement::ONE_SHOT_NOTE`. A kept cache gets a pre-pass that measures the stone, and
//!    the placement rules choose a memory tier or refuse with a reason.
//! 3. With `persist`, the cache key names a folder under the cache directory. A complete folder
//!    is loaded. A miss, or a corrupt folder that `load_cache` already deleted, creates a new one
//!    that [`write_cache`] fills on a background thread.
//!
//! # Rendering
//!
//! A [`CacheRun`] hangs on the render's `ExportCtx`. Fixed-spp batches go to the GPU's cached
//! live entry with every tile live, or to the CPU's cached tracer when the GPU declines or is
//! not in use. Adaptive passes (the `adaptive-sampling` feature) follow the same order. One
//! engine runs a whole batch, so the hybrid CPU and GPU split is not used with a cache.
//!
//! # Memory
//!
//! The free-memory figures come from [`memory_hints`].

use super::{
    RenderTaps, batch::ExportCtx, path_cache_memory::memory_hints, scene_snapshot::SceneSnapshot,
};
use crate::settings::{
    model::clamp_path_cache_disk_limit_gb,
    store::{default_render_cache_dir, peek_path_cache_disk_limit_gb},
};
use glam::{Mat4, Vec3};
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::Pass;
use pleochroma::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::{
        accel::PathCacheSettings,
        adaptive::LiveTiles,
        cpu_frame::{scatter_interleaved, trace_pixels_interleaved_surface_cached},
        frame_scene::FrameScene,
        gpu_backend::{CacheTier, GpuAccumulate, GpuBackend, GpuPathCache, memory_budget},
        path_cache::{
            BuildSettings, CacheKey, CacheKeyInputs, CacheMeta, CacheOptions, KeyFolder,
            PathCacheFrame, Placement, enforce_limit, estimate_size, load_cache,
            persist::unix_now,
            placement::{ONE_SHOT_NOTE, plan},
        },
    },
    simd::PlanesSoA32,
};
use std::{
    path::PathBuf,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

/// The disk limit saved in the settings file, in bytes (decimal gigabytes), or the default when
/// the file has none.
pub(super) fn saved_limit_bytes() -> u64 {
    let gb = clamp_path_cache_disk_limit_gb(peek_path_cache_disk_limit_gb());
    u64::from(gb) * 1_000_000_000
}

/// A cache folder being written for a render that keeps its cache.
pub(super) struct Persist {
    root: PathBuf,
    key: CacheKey,
    folder: KeyFolder,
    limit: u64,
}

/// The path cache a render chose to use.
pub(super) struct Chosen<'a> {
    /// The tiles, built on first use (or loaded).
    pub(super) frame: PathCacheFrame<'a, 'a>,
    /// Where the cache lives.
    pub(super) placement: Placement,
    /// The status line for the first progress note.
    pub(super) note: String,
    /// The folder to fill, for a render that keeps its cache and found none.
    pub(super) persist: Option<Persist>,
}

/// What [`decide`] settled.
pub(super) enum Decision<'a> {
    /// Render as today; the text, if any, is the one progress note that says why.
    Off(Option<String>),
    /// Render with the cache.
    On(Box<Chosen<'a>>),
}

/// Build settings of a render's cache: the adaptive tile and noise target when adaptive.
#[cfg(feature = "adaptive-sampling")]
pub(super) fn build_settings(taps: &RenderTaps) -> BuildSettings {
    taps.adaptive
        .as_ref()
        .map_or_else(BuildSettings::default, BuildSettings::from_adaptive)
}

/// The build settings of a render's cache in a build without the `adaptive-sampling` feature: the
/// default ones, whatever `taps.adaptive` says (the render is not adaptive there).
#[cfg(not(feature = "adaptive-sampling"))]
pub(super) fn build_settings(_taps: &RenderTaps) -> BuildSettings {
    BuildSettings::default()
}

/// Whether the render's pending path-cache note is `placement::ONE_SHOT_NOTE`: it asked for a
/// cache it does not keep. Call it before the first progress report takes the note.
pub(super) fn refused_one_shot(ctx: &ExportCtx<'_>) -> bool {
    ctx.cache_note
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_deref()
        .is_some_and(|note| note.contains(ONE_SHOT_NOTE))
}

/// Decides whether the render shades from a path cache; see the module doc.
pub(super) fn decide<'a>(
    frame_scene: &'a FrameScene<'a>,
    soa: &'a PlanesSoA32,
    snapshot: &'a SceneSnapshot,
    surface: SurfaceScene<'a>,
    taps: &RenderTaps,
) -> Decision<'a> {
    let Some(asked) = taps.path_cache else {
        return Decision::Off(None);
    };
    let build = build_settings(taps);
    let options = CacheOptions {
        settings: build,
        depth_of_field: false,
        budget_bytes: None,
    };
    let frame = match PathCacheFrame::new(
        frame_scene,
        soa,
        (&snapshot.tools, &snapshot.fluorescence, surface),
        options,
    ) {
        Ok(frame) => frame,
        Err(off) => return Decision::Off(Some(off.message().to_owned())),
    };
    // A cache that is not kept would cost more than it saves for one render.
    if !asked.persist {
        return Decision::Off(Some(ONE_SHOT_NOTE.to_owned()));
    }
    let prepared = PreparedMaterial::new(frame_scene.material);
    let started = Instant::now();
    let estimate = estimate_size(frame_scene, &prepared, soa, &build);
    tracing::info!(
        seconds = started.elapsed().as_secs_f64(),
        stone_pixels = estimate.stone_pixels,
        positions = estimate.positions,
        records_per_position = estimate.records_per_position,
        edge_share = estimate.edge_share,
        fire_share = estimate.fire_share,
        "path cache: pre-pass"
    );
    let budget = memory_budget(memory_hints(), frame_scene.width, frame_scene.height);
    let (placement, line) = plan(&estimate, &budget);
    if !placement.is_cached() {
        return Decision::Off(Some(line));
    }
    let (persist, loaded) = open_persist(&frame, frame_scene, &build, asked, taps);
    let note = if loaded {
        format!("{line}, loaded from disk")
    } else if persist.is_some() {
        format!("{line}, kept for re-renders")
    } else {
        // No folder could be created: the cache is used for this render only.
        line
    };
    Decision::On(Box::new(Chosen {
        frame,
        placement,
        note,
        persist,
    }))
}

/// [`decide`] for a render that may not have asked for a path cache (`asked` is `None` then).
///
/// Returns what the render keeps: the cache it chose, the one progress note (its size line, or
/// why it has none), and the folder to fill when it keeps its cache.
pub(super) fn choose<'a>(
    asked: Option<(&RenderTaps, &'a PlanesSoA32)>,
    frame_scene: &'a FrameScene<'a>,
    snapshot: &'a SceneSnapshot,
    surface: SurfaceScene<'a>,
) -> (Option<Box<Chosen<'a>>>, Option<String>, Option<Persist>) {
    let decision = asked.map(|(taps, soa)| decide(frame_scene, soa, snapshot, surface, taps));
    let (mut chosen, note) = match decision {
        Some(Decision::On(chosen)) => {
            let note = Some(chosen.note.clone());
            (Some(chosen), note)
        }
        Some(Decision::Off(note)) => (None, note),
        None => (None, None),
    };
    let persist = chosen.as_mut().and_then(|chosen| chosen.persist.take());
    (chosen, note, persist)
}

/// The persisted side of a render that keeps its cache: loads a finished folder into `frame`
/// (`true`), or creates the folder a background writer will fill.
fn open_persist<'a>(
    frame: &PathCacheFrame<'a, 'a>,
    frame_scene: &FrameScene<'a>,
    build: &BuildSettings,
    asked: PathCacheSettings,
    taps: &RenderTaps,
) -> (Option<Persist>, bool) {
    if !asked.persist {
        return (None, false);
    }
    let root = taps
        .cache_dir
        .clone()
        .unwrap_or_else(default_render_cache_dir);
    let limit = taps
        .cache_disk_limit_bytes
        .unwrap_or_else(saved_limit_bytes);
    let inputs = CacheKeyInputs::from_scene(frame_scene, Mat4::IDENTITY, build);
    let key = CacheKey::new(&inputs);
    match load_cache(&root, &key) {
        Ok((_, tiles)) => {
            let taken = frame.preload(tiles);
            tracing::info!(%key, tiles = taken, "path cache: loaded from disk");
            if let Err(error) = enforce_limit(&root, limit, Some(&key)) {
                tracing::warn!(%error, "path cache: could not enforce the disk limit");
            }
            return (None, true);
        }
        // Not an error: a first render has no folder, and a corrupt one is deleted and rebuilt.
        Err(reason) => tracing::debug!(%reason, "path cache: building a new cache"),
    }
    let meta = CacheMeta::describe(&inputs, &key, asked.compression, unix_now());
    match KeyFolder::create(&root, &key, meta) {
        Ok(folder) => (
            Some(Persist {
                root,
                key,
                folder,
                limit,
            }),
            false,
        ),
        Err(error) => {
            tracing::warn!(%error, "path cache: could not create the cache folder");
            (None, false)
        }
    }
}

/// Builds every tile of `frame` in tile order, writes each to the folder as it completes, then
/// marks the folder finished and enforces the disk limit.
///
/// Runs on a background thread beside the render. A raised `cancel` leaves the folder unfinished,
/// which the next `load_cache` deletes. A failed write deletes the folder. Errors are logged and
/// never fail the render.
pub(super) fn write_cache(frame: &PathCacheFrame<'_, '_>, persist: Persist, cancel: &AtomicBool) {
    let Persist {
        root,
        key,
        mut folder,
        limit,
    } = persist;
    let mut written = 0_u64;
    for index in 0..u32::try_from(frame.tile_count()).unwrap_or(u32::MAX) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Some(tile) = frame.tile(index) else {
            continue;
        };
        match folder.write_tile(tile) {
            Ok(bytes) => written += bytes,
            Err(error) => {
                tracing::warn!(%error, "path cache: could not write a tile, dropping the folder");
                folder.delete();
                return;
            }
        }
    }
    if let Err(error) = folder.finish(written) {
        tracing::warn!(%error, "path cache: could not finish the cache folder");
        return;
    }
    if let Err(error) = enforce_limit(&root, limit, Some(&key)) {
        tracing::warn!(%error, "path cache: could not enforce the disk limit");
    }
}

/// The GPU side of a chosen cache, or `None` when the placement is off. The slab size is capped
/// at what the device can bind.
pub(super) fn gpu_cache<'a>(
    gpu: &GpuBackend,
    frame: &'a PathCacheFrame<'a, 'a>,
    placement: &Placement,
) -> Option<GpuPathCache<'a, 'a, 'a>> {
    let tier = CacheTier::of(placement)?;
    let cache = GpuPathCache::new(frame, tier);
    Some(match gpu.path_cache_slab_limit() {
        Some(limit) => cache.with_slab_budget_bytes(tier.slab_budget_bytes().min(limit)),
        None => cache,
    })
}

/// The path cache of a render, as the batches use it.
pub(super) struct CacheRun<'a> {
    frame: &'a PathCacheFrame<'a, 'a>,
    gpu: Option<GpuPathCache<'a, 'a, 'a>>,
    scene: &'a FrameScene<'a>,
    soa: &'a PlanesSoA32,
    /// Every tile of the frame, for the fixed-spp batches.
    all_tiles: LiveTiles,
    /// Raised once the GPU declined a cached pass: the rest of the render stays on the CPU.
    gpu_declined: AtomicBool,
    /// Second buffer for the fixed-spp GPU batches; the render does not read it.
    scratch: Mutex<Vec<Vec3>>,
}

impl<'a> CacheRun<'a> {
    /// The cache of `frame`; `gpu` is its GPU side when the placement allows one.
    pub(super) fn new(
        frame: &'a PathCacheFrame<'a, 'a>,
        gpu: Option<GpuPathCache<'a, 'a, 'a>>,
        scene: &'a FrameScene<'a>,
        soa: &'a PlanesSoA32,
    ) -> Self {
        Self {
            frame,
            gpu,
            scene,
            soa,
            all_tiles: LiveTiles::all(scene.width, scene.height, frame.settings().tile),
            gpu_declined: AtomicBool::new(false),
            scratch: Mutex::new(Vec::new()),
        }
    }

    /// The cache shared by the CPU passes.
    #[cfg(feature = "adaptive-sampling")]
    pub(super) const fn frame(&self) -> &'a PathCacheFrame<'a, 'a> {
        self.frame
    }

    /// Whether the GPU may still be offered a cached pass.
    fn gpu_usable(&self, ctx: &ExportCtx<'_>) -> bool {
        self.gpu.is_some()
            && !self.gpu_declined.load(Ordering::Relaxed)
            && !ctx.gpu_retired.load(Ordering::Relaxed)
    }

    /// One adaptive pass on the GPU with the cache; `Declined` where the GPU cannot take it (the
    /// caller then traces the pass on the CPU, with the cache).
    #[cfg(feature = "adaptive-sampling")]
    pub(super) fn try_gpu_live(
        &self,
        ctx: &ExportCtx<'_>,
        pass: &Pass,
        sum: &mut [Vec3],
        half_b: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let Some(cache) = self.gpu.as_ref().filter(|_| self.gpu_usable(ctx)) else {
            return GpuAccumulate::Declined;
        };
        let outcome = ctx.gpu.try_accumulate_cached_live_cancellable(
            ctx.gpu_scene,
            &ctx.scene.tools,
            &ctx.surface,
            pass.first_sample,
            pass.spp,
            &pass.live,
            cache,
            sum,
            half_b,
            cancel,
        );
        if outcome == GpuAccumulate::Declined {
            self.gpu_declined.store(true, Ordering::Relaxed);
        }
        outcome
    }

    /// Adds samples `first_sample..first_sample + count` of every pixel to `accum`, shaded from
    /// the cache. Returns `true` when `cancel` ended it; nothing of the batch is then in `accum`.
    pub(super) fn accumulate(
        &self,
        ctx: &ExportCtx<'_>,
        first_sample: u32,
        count: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> bool {
        if let Some(cache) = self.gpu.as_ref().filter(|_| self.gpu_usable(ctx)) {
            let mut scratch = self.scratch.lock().unwrap_or_else(PoisonError::into_inner);
            if scratch.len() != accum.len() {
                *scratch = vec![Vec3::ZERO; accum.len()];
            }
            let outcome = ctx.gpu.try_accumulate_cached_live_cancellable(
                ctx.gpu_scene,
                &ctx.scene.tools,
                &ctx.surface,
                first_sample,
                count,
                &self.all_tiles,
                cache,
                accum,
                &mut scratch,
                cancel,
            );
            drop(scratch);
            match outcome {
                GpuAccumulate::Done => return false,
                GpuAccumulate::Cancelled => return true,
                GpuAccumulate::Declined => self.gpu_declined.store(true, Ordering::Relaxed),
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        self.accumulate_cpu(ctx, first_sample, count, accum);
        false
    }

    /// The CPU batch: every core takes every `stride`-th pixel, the sums are scattered back.
    fn accumulate_cpu(
        &self,
        ctx: &ExportCtx<'_>,
        first_sample: u32,
        count: u32,
        accum: &mut [Vec3],
    ) {
        let stride = thread::available_parallelism()
            .map_or(8, std::num::NonZero::get)
            .try_into()
            .unwrap_or(8_u32);
        let tools: &[_] = &ctx.scene.tools;
        let fluorescence: &Fluorescence = &ctx.scene.fluorescence;
        let extras = (tools, fluorescence, ctx.surface);
        let parts: Vec<(u32, Vec<Vec3>)> = thread::scope(|scope| {
            // Spawn all threads before joining any, so they run in parallel.
            let mut handles = Vec::new();
            for first in 0..stride {
                handles.push(scope.spawn(move || {
                    let sums = trace_pixels_interleaved_surface_cached(
                        self.scene,
                        extras,
                        self.soa,
                        (first, stride),
                        (first_sample, count),
                        Some(self.frame),
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
        for (first, sums) in parts {
            scatter_interleaved(accum, first, stride, &sums);
        }
    }
}
