//! The host side of the GPU path cache: slabs of tiles, the partition of a pass, and the host
//! fallback of the residual completion.
//!
//! This file is compiled with and without the `gpu` feature, like [`super::GpuBackend`], so the
//! backend's cached entry point has one signature in both builds. It holds no GPU object. The GPU
//! half (the phase B kernel, the slab buffers, the streaming) is `renderer::gpu::path_cache`.
//!
//! # Slabs
//!
//! The cache of a frame is a list of tiles (`renderer::path_cache::TileCache`), built by phase A on
//! CPU worker threads. For the GPU the tiles are grouped into **slabs**: runs of consecutive tiles
//! whose words together fit a byte budget (one storage binding of the device; for the system-RAM
//! tier, a small budget so a slab uploads quickly). A slab is the unit of upload and of dispatch:
//!
//! - the video-memory tier uploads each slab once and keeps it resident across passes;
//! - the system-RAM tier uploads slab `s + 1` while slab `s` shades (double buffering), and drops
//!   slab `s` when it is done, so each cache byte crosses the bus once per pass.
//!
//! Slab boundaries depend only on the tile sizes and the budget, never on timing, so the
//! output of a pass is the same whenever an upload happens. The plan is extended lazily, one
//! slab at a time, as a pass reaches it; [`GpuPathCache::with_prefetch`] runs phase A on
//! worker threads ahead of the pass in the same ascending tile order.

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use glam::Vec3;

use crate::{
    optics::raytracer::pixel_rotations,
    renderer::{
        adaptive::LiveTiles,
        path_cache::{
            PathCacheFrame, Placement, TileCache, placement::MemoryBudget, shade::RESIDUAL_FLOOR,
        },
    },
};

/// Slab budget of the video-memory tier: 256 MiB, within the storage-binding limit that desktop
/// devices report.
///
/// A device with a smaller binding limit declines a pass whose slab does not fit it (see
/// `GpuBackend::path_cache_slab_limit` for the figure to build the cache with).
pub const VRAM_SLAB_BYTES: u64 = 256 * 1024 * 1024;

/// Slab budget of the system-RAM tier: 64 MiB, so a slab uploads quickly and the next one
/// stages while the current one shades.
pub const RAM_SLAB_BYTES: u64 = 64 * 1024 * 1024;

/// Fixed video memory reserved for everything else a render holds, on top of the per-pixel share.
const VRAM_RESERVED_FIXED_BYTES: u64 = 256 * 1024 * 1024;

/// Bytes of video memory per frame pixel the render itself needs (two resident accumulators of
/// 12 bytes, the pass scratch and the readback staging, rounded up).
const VRAM_RESERVED_PER_PIXEL: u64 = 48;

/// Where the slabs of a cache live between passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheTier {
    /// Every slab is uploaded once and stays in video memory.
    Vram,
    /// The cache stays in system RAM; slabs stream to the GPU and are dropped after use.
    SystemRam,
}

impl CacheTier {
    /// The tier a placement names, or `None` for [`Placement::Off`].
    #[must_use]
    pub const fn of(placement: &Placement) -> Option<Self> {
        match placement {
            Placement::Vram => Some(Self::Vram),
            Placement::SystemRam => Some(Self::SystemRam),
            Placement::Off(_) => None,
        }
    }

    /// The default slab size of the tier in bytes.
    #[must_use]
    pub const fn slab_budget_bytes(self) -> u64 {
        match self {
            Self::Vram => VRAM_SLAB_BYTES,
            Self::SystemRam => RAM_SLAB_BYTES,
        }
    }

    /// Whether uploaded slabs stay on the GPU after the pass that uploaded them.
    #[must_use]
    pub const fn keeps_slabs_resident(self) -> bool {
        matches!(self, Self::Vram)
    }
}

/// What the caller knows about free memory; `None` means unknown.
///
/// wgpu reports buffer-size limits but not free video memory, so the free figure has to come from
/// the operating system (the caller's DXGI, Vulkan memory budget or `nvidia-smi` style query).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryHints {
    /// Free video memory in bytes.
    pub vram_free: Option<u64>,
    /// Available physical RAM in bytes.
    pub ram_available: Option<u64>,
}

/// The [`MemoryBudget`] of a render of `width` x `height` pixels on this GPU.
///
/// It holds the hints, with the video memory the render itself needs
/// ([`VRAM_RESERVED_PER_PIXEL`] bytes per pixel and a fixed 256 MiB) already counted as reserved.
#[must_use]
pub fn memory_budget(hints: MemoryHints, width: u32, height: u32) -> MemoryBudget {
    let pixels = u64::from(width) * u64::from(height);
    MemoryBudget {
        vram_free: hints.vram_free,
        vram_reserved: pixels
            .saturating_mul(VRAM_RESERVED_PER_PIXEL)
            .saturating_add(VRAM_RESERVED_FIXED_BYTES),
        ram_available: hints.ram_available,
    }
}

/// A run of consecutive tiles uploaded and dispatched together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlabSpan {
    /// Row-major index of the first tile.
    pub first_tile: u32,
    /// Number of tiles.
    pub tile_count: u32,
    /// Bytes of the tiles' words together.
    pub bytes: u64,
}

impl SlabSpan {
    /// One past the last tile index.
    #[must_use]
    pub const fn end_tile(&self) -> u32 {
        self.first_tile + self.tile_count
    }
}

/// The slab that starts at `first_tile` and takes tiles of the given `tile_bytes` (in order) while
/// they fit `budget` bytes. A slab always holds at least one tile, however large.
///
/// The iterator is pulled lazily, so a caller that builds tiles on request builds only the
/// tiles of the slab and the one that did not fit.
#[must_use]
pub fn plan_span(
    first_tile: u32,
    tile_bytes: impl IntoIterator<Item = u64>,
    budget: u64,
) -> SlabSpan {
    let mut bytes = 0_u64;
    let mut tile_count = 0_u32;
    for size in tile_bytes {
        if tile_count > 0 && bytes.saturating_add(size) > budget {
            break;
        }
        bytes = bytes.saturating_add(size);
        tile_count += 1;
    }
    SlabSpan {
        first_tile,
        tile_count,
        bytes,
    }
}

/// The slabs planned so far.
#[derive(Default)]
struct SlabPlan {
    spans: Vec<SlabSpan>,
    planned_tiles: u32,
    /// Per slab, once known: whether any of its positions is above the residual floor.
    has_residual: Vec<Option<bool>>,
}

/// Whether any position of `tile` has a residual the completion would trace.
fn tile_has_residual(tile: &TileCache) -> bool {
    (0..tile.header().position_count as usize)
        .any(|position| tile.position_residual(position) >= RESIDUAL_FLOOR)
}

/// One slab prepared for a pass: its tiles, where each starts, and the pass's pixels in it.
pub struct HostSlab<'f> {
    /// The slab's index in the cache's plan.
    pub index: usize,
    /// The tiles the slab covers.
    pub span: SlabSpan,
    /// Every tile of the span, in order (the GPU holds all of them).
    pub tiles: Vec<&'f TileCache>,
    /// The word offset of every tile of the span in the slab buffer.
    pub tile_offsets: Vec<u32>,
    /// Real indices of the live pixels in the slab that have cached positions, ascending by tile
    /// and then row-major: the pixels phase B shades.
    pub pixels: Vec<u32>,
    /// Real indices of the live pixels in the slab without cached positions: the path tracer's.
    pub rest: Vec<u32>,
    /// Whether any position of the slab has a residual above the completion floor, so phase B has
    /// samples to report (the kernel skips the fired list otherwise).
    pub has_residual: bool,
}

impl HostSlab<'_> {
    /// Words of the slab buffer.
    #[must_use]
    pub fn words(&self) -> usize {
        self.tiles.iter().map(|tile| tile.as_words().len()).sum()
    }

    /// Bytes of the slab buffer.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        u64::try_from(self.words())
            .unwrap_or(u64::MAX)
            .saturating_mul(4)
    }
}

/// Source of cache ids; an id names the GPU slabs of one cache.
static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);

/// A path cache as the GPU backend uses it: the CPU-built tiles of a frame, the memory tier
/// and the slab plan.
pub struct GpuPathCache<'f, 's, 'a> {
    frame: &'f PathCacheFrame<'s, 'a>,
    id: u64,
    tier: CacheTier,
    slab_budget_bytes: u64,
    stage_delay: Duration,
    cpu_completion: bool,
    slabs: Mutex<SlabPlan>,
}

impl<'f, 's, 'a> GpuPathCache<'f, 's, 'a> {
    /// A GPU cache of `frame` in memory tier `tier`, with the tier's default slab size.
    #[must_use]
    pub fn new(frame: &'f PathCacheFrame<'s, 'a>, tier: CacheTier) -> Self {
        Self {
            frame,
            id: NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed),
            tier,
            slab_budget_bytes: tier.slab_budget_bytes(),
            stage_delay: Duration::ZERO,
            cpu_completion: false,
            slabs: Mutex::new(SlabPlan::default()),
        }
    }

    /// A GPU cache for the tier `placement` names, or `None` when the placement is off (the render
    /// then goes on without a cache).
    #[must_use]
    pub fn from_placement(
        frame: &'f PathCacheFrame<'s, 'a>,
        placement: &Placement,
    ) -> Option<Self> {
        CacheTier::of(placement).map(|tier| Self::new(frame, tier))
    }

    /// The same cache with slabs of at most `bytes` bytes (a slab holds at least one tile). Set it
    /// before the first pass; the plan, once made, keeps its boundaries.
    #[must_use]
    pub fn with_slab_budget_bytes(mut self, bytes: u64) -> Self {
        self.slab_budget_bytes = bytes.max(1);
        self
    }

    /// The same cache sleeping `delay` before every slab upload: a test and diagnostics hook that
    /// proves the output does not depend on when an upload happens.
    #[must_use]
    pub const fn with_stage_delay(mut self, delay: Duration) -> Self {
        self.stage_delay = delay;
        self
    }

    /// The same cache completing the fired samples on the host even where the GPU could.
    ///
    /// The host completion is the CPU path tracer and cover test of
    /// `PathCacheFrame::complete_residual`, the GPU's fallback. A test and diagnostics hook; renders
    /// leave it off.
    #[must_use]
    pub const fn with_cpu_completion(mut self) -> Self {
        self.cpu_completion = true;
        self
    }

    /// Whether the fired samples complete on the host ([`Self::with_cpu_completion`]).
    #[must_use]
    pub const fn cpu_completion(&self) -> bool {
        self.cpu_completion
    }

    /// The CPU cache the slabs come from.
    #[must_use]
    pub const fn frame(&self) -> &'f PathCacheFrame<'s, 'a> {
        self.frame
    }

    /// The id that names this cache's slabs on the GPU.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// The memory tier.
    #[must_use]
    pub const fn tier(&self) -> CacheTier {
        self.tier
    }

    /// The most bytes a slab holds (unless one tile alone is larger).
    #[must_use]
    pub const fn slab_budget_bytes(&self) -> u64 {
        self.slab_budget_bytes
    }

    /// The artificial upload delay (zero outside tests).
    #[must_use]
    pub const fn stage_delay(&self) -> Duration {
        self.stage_delay
    }

    /// Slabs planned so far.
    #[must_use]
    pub fn planned_slabs(&self) -> usize {
        self.lock_plan().spans.len()
    }

    fn lock_plan(&self) -> std::sync::MutexGuard<'_, SlabPlan> {
        self.slabs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Slab `index` of the plan, planning (and so building the tiles of) every slab up to it.
    /// `None` past the last tile.
    #[must_use]
    pub fn slab(&self, index: usize) -> Option<SlabSpan> {
        let total = u32::try_from(self.frame.tile_count()).unwrap_or(u32::MAX);
        let mut plan = self.lock_plan();
        while plan.spans.len() <= index {
            if plan.planned_tiles >= total {
                return None;
            }
            let first = plan.planned_tiles;
            let sizes = (first..total).map(|tile| {
                self.frame.tile(tile).map_or(0, |cache| {
                    u64::try_from(cache.byte_len()).unwrap_or(u64::MAX)
                })
            });
            let span = plan_span(first, sizes, self.slab_budget_bytes);
            plan.planned_tiles = span.end_tile();
            plan.spans.push(span);
        }
        plan.spans.get(index).copied()
    }

    /// Prepares slab `index` for a pass over `live`: its tiles, their offsets, and the live pixels
    /// of the slab split into those phase B shades and those the path tracer keeps.
    ///
    /// `None` past the last tile.
    #[must_use]
    pub fn prepare_slab(&self, index: usize, live: &LiveTiles) -> Option<HostSlab<'f>> {
        let span = self.slab(index)?;
        let frame: &'f PathCacheFrame<'s, 'a> = self.frame;
        let mut tiles = Vec::with_capacity(span.tile_count as usize);
        let mut tile_offsets = Vec::with_capacity(span.tile_count as usize);
        let mut offset = 0_usize;
        for tile_index in span.first_tile..span.end_tile() {
            let tile = frame.tile(tile_index)?;
            tile_offsets.push(u32::try_from(offset).ok()?);
            offset += tile.as_words().len();
            tiles.push(tile);
        }
        let indices = live.indices();
        let low = indices.partition_point(|&tile| tile < span.first_tile);
        let high = indices.partition_point(|&tile| tile < span.end_tile());
        let mut pixels = Vec::new();
        let mut rest = Vec::new();
        for &tile_index in &indices[low..high] {
            let tile = tiles[(tile_index - span.first_tile) as usize];
            let in_tile = tile.header().pixel_count as usize;
            let (x0, y0, x1, y1) = live.tile_rect(tile_index);
            let mut local = 0_usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    let pixel = y * live.width() + x;
                    if local < in_tile && !tile.pixel_positions(local).is_empty() {
                        pixels.push(pixel);
                    } else {
                        rest.push(pixel);
                    }
                    local += 1;
                }
            }
        }
        let has_residual = self.slab_has_residual(index, &tiles);
        Some(HostSlab {
            index,
            span,
            tiles,
            tile_offsets,
            pixels,
            rest,
            has_residual,
        })
    }

    /// Whether slab `index` (made of `tiles`) has a position above the residual floor, scanned
    /// once and remembered.
    fn slab_has_residual(&self, index: usize, tiles: &[&TileCache]) -> bool {
        let known = self.lock_plan().has_residual.get(index).copied().flatten();
        if let Some(known) = known {
            return known;
        }
        let found = tiles.iter().any(|tile| tile_has_residual(tile));
        let mut plan = self.lock_plan();
        if plan.has_residual.len() <= index {
            plan.has_residual.resize(index + 1, None);
        }
        plan.has_residual[index] = Some(found);
        found
    }

    /// Runs `body` while worker threads build the tiles up to `last_tile` that the plan has not
    /// reached yet, in ascending order: phase A ahead of the pass, overlapping the GPU's phase B of
    /// the slabs before them. The workers stop when `body` returns.
    ///
    /// With every tile up to `last_tile` already planned (every pass after the first), no thread
    /// starts.
    pub fn with_prefetch<R>(&self, last_tile: u32, body: impl FnOnce() -> R) -> R {
        let first = self.lock_plan().planned_tiles;
        if first > last_tile {
            return body();
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .saturating_sub(1)
            .max(1);
        let next = AtomicU32::new(first);
        let stop = AtomicBool::new(false);
        let frame: &PathCacheFrame<'s, 'a> = self.frame;
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        let tile = next.fetch_add(1, Ordering::Relaxed);
                        if tile > last_tile {
                            break;
                        }
                        // The result is kept by the frame; a tile built here is found built later.
                        let _ = frame.tile(tile);
                    }
                });
            }
            let result = body();
            stop.store(true, Ordering::Relaxed);
            result
        })
    }

    /// The host completion of the samples phase B reported for `slab`.
    ///
    /// It is the fallback of the GPU completion, for a renderer that cannot run
    /// `shaders/path_cache/complete.wgsl` or a cache built [`Self::with_cpu_completion`].
    /// `fired` holds `[virtual pixel, absolute sample index, position index, tuple index]`
    /// entries as the kernel appended them, in GPU arrival order. They are sorted first, so the
    /// result does not depend on that order. Returns `(virtual pixel, sample index, XYZ)` in
    /// sorted order, computed on all available threads.
    #[must_use]
    pub fn complete_residuals(
        &self,
        slab: &HostSlab<'_>,
        width: u32,
        fired: &mut Vec<[u32; 4]>,
    ) -> Vec<(usize, u32, Vec3)> {
        fired.sort_unstable();
        fired.dedup();
        if fired.is_empty() {
            return Vec::new();
        }
        let frame: &PathCacheFrame<'s, 'a> = self.frame;
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(fired.len());
        let per_thread = fired.len().div_ceil(threads);
        let one = |entry: &[u32; 4]| -> (usize, u32, Vec3) {
            let virtual_pixel = entry[0] as usize;
            let Some(&pixel) = slab.pixels.get(virtual_pixel) else {
                return (virtual_pixel, entry[1], Vec3::ZERO);
            };
            let rot = pixel_rotations(pixel);
            let value = frame.complete_residual(
                (pixel % width, pixel / width),
                (entry[1], entry[2] as usize),
                &rot,
            );
            (virtual_pixel, entry[1], value)
        };
        let mut results = Vec::with_capacity(fired.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = fired
                .chunks(per_thread)
                .map(|part| scope.spawn(move || part.iter().map(one).collect::<Vec<_>>()))
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(part) => results.extend(part),
                    // A panicked worker must not silently drop samples of the image.
                    Err(payload) => std::panic::resume_unwind(payload),
                }
            }
        });
        results
    }
}
