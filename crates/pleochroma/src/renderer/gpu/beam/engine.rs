//! The GPU beam engine (beam guide §14.7): fragments on the GPU, and the residual traced by the
//! GPU reference pipeline with a convergence check that runs on the host or on the GPU.
//!
//! # What the engine does
//!
//! * [`BeamEngine::fragments`] shades the batch's packed leaves with the fragment kernels
//!   ([`GpuBackend::try_beam_fragments_compact`]) straight into the batch layout
//!   ([`BATCH_PIXEL_ORDER`](crate::renderer::beam::engines::BATCH_PIXEL_ORDER)), one slot per
//!   batch pixel ([`PixelSlots`]); no whole-frame buffer.
//! * [`BeamEngine::residual`] runs the batch's adaptive loop on the host: one [`AdaptiveRun`]
//!   restricted to the batch's tiles, every pass traced by the GPU reference pipeline with the
//!   batch's kept buffers
//!   ([`GpuBackend::try_accumulate_reference_live_uploaded_compact_cancellable`]), or the
//!   packed-kept entry when that upload declines. The convergence check is measured on the host,
//!   with the beam part inside the noise metric (§12.4).
//! * With [`GpuBeamEngine::with_gpu_convergence`] (the `--gpu-convergence` render option) the
//!   residual runs on the GPU-resident path: the frame's sums stay on the GPU
//!   ([`GpuBackend::try_adaptive_resident`]), restricted to the batch's tiles
//!   ([`GpuResidentRun::restricted_to`]) with the batch's kept buffers
//!   ([`GpuResidentRun::with_reference_kept`]). The convergence check runs on the GPU with the
//!   beam part uploaded once per batch, and only the band of rows the batch covers is read back.
//!   If the resident run cannot start or dies mid-run, the residual restarts on the host path from
//!   sample zero, since its sums lived on the GPU. The caller drops `NOTE_BEAMS_HOST_CONVERGENCE`
//!   when [`GpuBeamEngine::uses_gpu_convergence`] is true.
//!
//! # Declines (§12.5)
//!
//! Anything the GPU cannot do is an [`EngineOutcome::Declined`] with a reason for the log. The
//! caller hands the batch to the next engine, which redoes the whole request deterministically;
//! a declined residual leaves no partial result behind. The reasons: no adapter (the
//! backend entries decline), a lost device (checked before the request and between passes), a
//! retired engine ([`GpuBeamEngine::retire`]), the storage buffer count or a binding cap (the
//! backend entries halve or decline), and a batch that is not laid out on the adaptive tile grid.
//!
//! # Limits
//!
//! The leaf upload happens inside `try_beam_fragments_compact`, which has no separate upload
//! step, so the next batch's leaves cannot be uploaded while this batch's residual runs.
//!
//! The kept buffers are uploaded once per batch (`upload_reference_kept`) and shared by every
//! host-convergence pass of the batch; the packed entry is the fallback when that upload
//! declines.
//!
//! # Lifetimes
//!
//! The engine borrows the frame (through [`ResidualScene`]) and the backend, so it is
//! `GpuBeamEngine<'a>`; an [`EngineSet`](crate::renderer::beam::engines::EngineSet) that holds it
//! needs `Box<dyn BeamEngine + 'a>`.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use glam::Vec3;

use crate::renderer::{
    adaptive::{
        AdaptiveRun, AdaptiveSettings, LiveTiles, NoiseMetric, PendingCheck,
        metric::{tile_measure_with_offset, tile_noise},
    },
    beam::{
        BeamSettings,
        engines::{Batch, BatchFragments, BatchResidual, BeamEngine, EngineOutcome},
        pipeline::PACKED_KEPT_OVER_CAP,
        primary::TileRect,
        tile_residual::{ResidualScene, residual_share},
    },
    gpu::beam::kept::KeptBufferSet,
    gpu_backend::{
        BeamFragmentParams, GpuAccumulate, GpuBackend, GpuBeamFragments, GpuResidentRun,
        GpuSceneRef,
    },
};

// The one residual rule of both engines (the CPU engine calls the same function), so a batch's
// stop decisions do not depend on the engine.
pub use crate::renderer::beam::tile_residual::residual_settings;

/// The reason when a single tile's fragments do not fit the storage binding limit.
///
/// Defined here, not in `gpu_backend::backend::beam` (the entry that returns it), because that
/// module is private to `gpu_backend` and the entry already imports from this module.
pub const DECLINE_TILE_OVER_LIMIT: &str = "a tile exceeds the storage binding limit";

/// The reason when a piece's reduction does not fit the storage binding limit (see
/// [`DECLINE_TILE_OVER_LIMIT`] for where it is defined).
pub const DECLINE_REDUCTION_OVER_LIMIT: &str = "reduction exceeds the storage binding limit";

/// The reason when the device is lost or the engine is retired.
const DECLINE_LOST: &str = "GPU device lost";

/// The reason when a batch's tiles are not the adaptive tile grid in row-major order.
const DECLINE_GRID: &str = "batch tiles are not on the adaptive tile grid";

/// The reason when the offset slice does not have one entry per batch pixel.
const DECLINE_OFFSET: &str = "offset length differs from the batch";

/// The reason when the batch's tiles do not fit the frame.
const DECLINE_FRAME: &str = "batch tile outside the frame";

/// The reason when the adaptive settings are rejected.
const DECLINE_SETTINGS: &str = "adaptive settings are invalid";

/// The reason when a residual pass declines (the entry logs why).
const DECLINE_PASS: &str = "GPU residual pass declined";

/// A tile of a batch: its row-major index on the adaptive grid, its rectangle and where its first
/// pixel sits in the batch layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchTile {
    /// The tile's row-major index on the adaptive grid (`LiveTiles` numbering).
    pub index: u32,
    /// The tile's pixel rectangle.
    pub rect: TileRect,
    /// The batch-layout position of the tile's first pixel.
    pub start: usize,
}

/// The adaptive-grid layout of a batch's `tiles` in a `size` frame with tile edge `tile`.
///
/// `None` when a tile is not exactly a grid tile (origin on the grid, edge `tile` or clipped by the
/// frame) or the tiles are not in strictly ascending row-major order. The result keeps the batch
/// order, so [`BatchTile::index`] is ascending and a tile is found by binary search.
#[must_use]
pub fn batch_layout(tiles: &[TileRect], size: (u32, u32), tile: u32) -> Option<Vec<BatchTile>> {
    let tile = tile.max(1);
    let tiles_x = size.0.div_ceil(tile);
    let mut layout = Vec::with_capacity(tiles.len());
    let mut start = 0_usize;
    let mut previous: Option<u32> = None;
    for &rect in tiles {
        let aligned = rect.x0 < rect.x1
            && rect.y0 < rect.y1
            && rect.x0.is_multiple_of(tile)
            && rect.y0.is_multiple_of(tile)
            && rect.x1 == rect.x0.saturating_add(tile).min(size.0)
            && rect.y1 == rect.y0.saturating_add(tile).min(size.1);
        if !aligned {
            return None;
        }
        let index = (rect.y0 / tile) * tiles_x + rect.x0 / tile;
        if previous.is_some_and(|before| before >= index) {
            return None;
        }
        previous = Some(index);
        layout.push(BatchTile { index, rect, start });
        start += rect.width() as usize * rect.height() as usize;
    }
    Some(layout)
}

/// The pixel counts a batch's scratch needs, by the three ways of laying it out.
///
/// The batch's own tiles are the compact slots; the band is the rows from the first tile's top to
/// the last tile's bottom (what the resident path reads back); the frame is what the whole-image
/// backend entries (`try_beam_fragments`, `try_accumulate_reference_live_cancellable`) take. The
/// host-convergence engine sizes its scratch by `compact`. A two-tile batch at opposite corners of
/// an 8K frame needs `2 * tile^2` compact slots, almost the whole frame as a band, and the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScratchPlan {
    /// The pixels of the batch's tiles: one slot per batch pixel, tile after tile.
    pub compact: u64,
    /// The pixels of the full-width rows the batch's tiles span.
    pub band: u64,
    /// The pixels of the whole frame.
    pub frame: u64,
}

/// The [`ScratchPlan`] of `layout` in a frame of `size`.
#[must_use]
pub fn scratch_plan(layout: &[BatchTile], size: (u32, u32)) -> ScratchPlan {
    let compact = layout
        .iter()
        .map(|tile| u64::from(tile.rect.width()) * u64::from(tile.rect.height()))
        .sum();
    let first = layout.iter().map(|tile| tile.rect.y0).min();
    let last = layout.iter().map(|tile| tile.rect.y1).max();
    let rows = first.zip(last).map_or(0, |(top, bottom)| bottom - top);
    ScratchPlan {
        compact,
        band: u64::from(rows) * u64::from(size.0),
        frame: u64::from(size.0) * u64::from(size.1),
    }
}

/// One row of one batch tile in a [`PixelSlots`]: the image row `y`, the columns `x0..x1` and the
/// slot of the row's first pixel.
///
/// Rows sort by `(y, x0, x1, base)`; [`PixelSlots::slot`] searches them by `(y, x0)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SlotRow {
    y: u32,
    x0: u32,
    x1: u32,
    base: usize,
}

/// The compact slots of a batch's pixels: the tiles in order, each tile row-major, which is the
/// batch layout ([`BatchTile::start`]) the engine reports in.
///
/// The compact backend entries (`try_beam_fragments_compact`,
/// `try_accumulate_reference_live_compact_cancellable`) write into slices of [`Self::len`] pixels
/// instead of whole frames; this maps a frame pixel to its slot. The table holds one entry per
/// tile row, so a lookup is a binary search and the memory is `tile_pixels / tile width` entries,
/// never the frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PixelSlots {
    rows: Vec<SlotRow>,
    len: usize,
}

impl PixelSlots {
    /// The slots of `tiles` in the given order; `None` when two tiles overlap (a pixel would have
    /// two slots). Empty tiles hold no slot.
    #[must_use]
    pub fn new(tiles: &[TileRect]) -> Option<Self> {
        let mut rows = Vec::new();
        let mut len = 0_usize;
        for tile in tiles.iter().filter(|tile| !tile.is_empty()) {
            for y in tile.y0..tile.y1 {
                rows.push(SlotRow {
                    y,
                    x0: tile.x0,
                    x1: tile.x1,
                    base: len,
                });
                len += tile.width() as usize;
            }
        }
        rows.sort_unstable();
        let overlap = rows
            .windows(2)
            .any(|pair| pair[0].y == pair[1].y && pair[1].x0 < pair[0].x1);
        (!overlap).then_some(Self { rows, len })
    }

    /// The number of slots: the pixels of the batch's tiles.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// `true` when the batch has no pixel.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The slot of pixel `(x, y)`, or `None` when no tile holds it.
    #[must_use]
    pub fn slot(&self, x: u32, y: u32) -> Option<usize> {
        let after = self.rows.partition_point(|row| (row.y, row.x0) <= (y, x));
        let row = self.rows.get(after.checked_sub(1)?)?;
        (row.y == y && x < row.x1).then(|| row.base + (x - row.x0) as usize)
    }

    /// The slot of the pixel at row-major `index` of an image `width` pixels wide.
    #[must_use]
    pub fn slot_of_index(&self, width: u32, index: usize) -> Option<usize> {
        let width = width as usize;
        if width == 0 {
            return None;
        }
        self.slot(
            u32::try_from(index % width).ok()?,
            u32::try_from(index / width).ok()?,
        )
    }
}

/// The residual sums of a batch in compact slots: `sum` and `half_b` hold one entry per slot of
/// `slots`.
pub struct SlotSums<'a> {
    /// Where each pixel's entry sits.
    pub slots: &'a PixelSlots,
    /// The sample sums per slot.
    pub sum: &'a mut [Vec3],
    /// The odd-sample sums per slot.
    pub half_b: &'a mut [Vec3],
}

/// The pixels of the whole-image `image` (`width` pixels per row) in the batch layout: `tiles` in
/// order, each tile row-major. `None` when a tile does not lie inside `image`.
#[must_use]
pub fn batch_order(image: &[Vec3], width: u32, tiles: &[TileRect]) -> Option<Vec<Vec3>> {
    let width = width as usize;
    let mut out = Vec::new();
    for tile in tiles {
        if tile.x1 as usize > width {
            return None;
        }
        for y in tile.y0..tile.y1 {
            let row = y as usize * width;
            out.extend_from_slice(image.get(row + tile.x0 as usize..row + tile.x1 as usize)?);
        }
    }
    Some(out)
}

/// Sets the pixels inside each `[x0, y0, x1, y1]` rectangle (end-exclusive) of the whole-image
/// `image` to zero; parts outside the image are skipped.
pub fn clear_rects(image: &mut [Vec3], width: u32, rects: impl IntoIterator<Item = [u32; 4]>) {
    let width = width as usize;
    for [x0, y0, x1, y1] in rects {
        for y in y0..y1 {
            let row = y as usize * width;
            if let Some(span) = image.get_mut(row + x0 as usize..row + x1 as usize) {
                span.fill(Vec3::ZERO);
            }
        }
    }
}

/// The noise of every live tile of `check`, in the order of `check.live`.
///
/// The beam part `offset` (batch layout) added inside the metric: `offset + mean_a` against
/// `offset + mean_b`, tone-mapped as the render is (§12.4).
///
/// `sums` is `(sum, half_b)`, the whole-frame residual buffers of width `width`; `display` is the
/// display multiplier and the metric. An empty `offset` means no beam part. A live tile that is
/// not in `layout`, or that `offset` does not cover, reads as unmeasured (infinite), so it never
/// converges by accident. The per-tile measure is `tile_measure_with_offset`, the function the CPU
/// batch residual uses, so the two engines judge a tile alike.
#[must_use]
pub fn batch_tile_noises(
    check: &PendingCheck,
    layout: &[BatchTile],
    offset: &[Vec3],
    sums: (&[Vec3], &[Vec3]),
    width: u32,
    display: (f32, NoiseMetric),
) -> Vec<f32> {
    tile_noises_with(
        check,
        layout,
        offset,
        display,
        |tile, local_sum, local_half| {
            for y in tile.rect.y0..tile.rect.y1 {
                let row = y as usize * width as usize;
                for x in tile.rect.x0..tile.rect.x1 {
                    let pixel = row + x as usize;
                    local_sum.push(sums.0.get(pixel).copied().unwrap_or(Vec3::ZERO));
                    local_half.push(sums.1.get(pixel).copied().unwrap_or(Vec3::ZERO));
                }
            }
        },
    )
}

/// [`batch_tile_noises`] for sums in compact slots: `sums` is `(sum, half_b)` with a tile's pixels
/// at [`BatchTile::start`] onward, tile-local row-major.
///
/// Slots past the end of a short slice read as zero, as pixels past a whole-frame slice do.
#[must_use]
pub fn compact_tile_noises(
    check: &PendingCheck,
    layout: &[BatchTile],
    offset: &[Vec3],
    sums: (&[Vec3], &[Vec3]),
    display: (f32, NoiseMetric),
) -> Vec<f32> {
    tile_noises_with(
        check,
        layout,
        offset,
        display,
        |tile, local_sum, local_half| {
            let area = tile.rect.pixel_count();
            for slot in tile.start..tile.start + area {
                local_sum.push(sums.0.get(slot).copied().unwrap_or(Vec3::ZERO));
                local_half.push(sums.1.get(slot).copied().unwrap_or(Vec3::ZERO));
            }
        },
    )
}

/// The noise of every live tile of `check`, with `gather` filling a tile's local sums (row-major
/// over the tile's rectangle); the rest is [`batch_tile_noises`].
fn tile_noises_with(
    check: &PendingCheck,
    layout: &[BatchTile],
    offset: &[Vec3],
    display: (f32, NoiseMetric),
    gather: impl Fn(&BatchTile, &mut Vec<Vec3>, &mut Vec<Vec3>),
) -> Vec<f32> {
    let (scale, metric) = display;
    let counts = (check.samples, check.odd_samples);
    let mut local_sum: Vec<Vec3> = Vec::new();
    let mut local_half: Vec<Vec3> = Vec::new();
    check
        .live
        .indices()
        .iter()
        .map(|&index| {
            let Ok(at) = layout.binary_search_by_key(&index, |tile| tile.index) else {
                return f32::INFINITY;
            };
            let tile = layout[at];
            local_sum.clear();
            local_half.clear();
            gather(&tile, &mut local_sum, &mut local_half);
            // The tile's beam part starts at `tile.start` and is laid out like the local buffers.
            let beam = (!offset.is_empty()).then(|| offset.get(tile.start..).unwrap_or(&[]));
            let measure = tile_measure_with_offset(
                &local_sum,
                &local_half,
                beam,
                tile.rect.width(),
                (0, 0, tile.rect.width(), tile.rect.height()),
                counts,
                scale,
            );
            tile_noise(&measure, metric)
        })
        .collect()
}

/// A pooled scratch buffer of the batch's compact slots, all zero between uses.
type Pool = Mutex<Vec<Vec3>>;

/// Takes the buffer out of `pool`, all zero and `len` long.
fn take_zeroed(pool: &Pool, len: usize) -> Vec<Vec3> {
    let mut buffer = std::mem::take(&mut *pool.lock().unwrap_or_else(PoisonError::into_inner));
    if buffer.len() != len {
        buffer.clear();
        buffer.resize(len, Vec3::ZERO);
    }
    buffer
}

/// Hands a buffer that is all zero again back to `pool`.
fn give_back(pool: &Pool, buffer: Vec<Vec3>) {
    *pool.lock().unwrap_or_else(PoisonError::into_inner) = buffer;
}

/// What one residual run needs besides the engine.
struct ResidualJob<'j, 'b> {
    batch: &'j Batch<'b>,
    layout: &'j [BatchTile],
    offset: &'j [Vec3],
    cancel: &'j AtomicBool,
}

/// The GPU beam engine: fragments and the residual of a batch on the GPU. The convergence check
/// runs on the host unless [`GpuBeamEngine::with_gpu_convergence`] is set.
pub struct GpuBeamEngine<'a> {
    backend: &'a GpuBackend,
    scene: ResidualScene<'a>,
    adaptive: AdaptiveSettings,
    fragment_params: BeamFragmentParams,
    display_scale: f32,
    gpu_convergence: bool,
    retired: AtomicBool,
    /// Batches whose residual completed on the GPU-resident path.
    resident_completions: AtomicU32,
    /// Batches that asked for the resident path but ran on the host path instead.
    resident_fallbacks: AtomicU32,
    /// Batches this engine was asked to shade.
    batches_seen: AtomicU32,
    /// Fragment or residual requests this engine declined, whatever the reason.
    declined_batches: AtomicU32,
    /// The reason of the most recent decline.
    last_decline: Mutex<Option<&'static str>>,
    fragment_image: Pool,
    residual_sum: Pool,
    residual_half: Pool,
}

impl<'a> GpuBeamEngine<'a> {
    /// The engine for `scene` on `backend`.
    ///
    /// The fragment stage takes `max_fragment_levels` and `lighting_tolerance` from `settings`
    /// and the node count, sample offset and `residual_min_spp` from `scene`; the residual loop
    /// uses `adaptive` for the metric, the pass size, the cap and the tile edge (the beam tile is
    /// the adaptive tile, §12.4). The display scale of the noise metric starts at 1; see
    /// [`Self::with_display_scale`].
    #[must_use]
    pub const fn new(
        backend: &'a GpuBackend,
        scene: &ResidualScene<'a>,
        settings: &BeamSettings,
        adaptive: &AdaptiveSettings,
    ) -> Self {
        Self {
            backend,
            scene: *scene,
            adaptive: *adaptive,
            fragment_params: BeamFragmentParams {
                max_levels: settings.max_fragment_levels,
                n_lambda: scene.n_lambda,
                tolerance: settings.lighting_tolerance,
            },
            display_scale: 1.0,
            gpu_convergence: false,
            retired: AtomicBool::new(false),
            resident_completions: AtomicU32::new(0),
            resident_fallbacks: AtomicU32::new(0),
            batches_seen: AtomicU32::new(0),
            declined_batches: AtomicU32::new(0),
            last_decline: Mutex::new(None),
            fragment_image: Mutex::new(Vec::new()),
            residual_sum: Mutex::new(Vec::new()),
            residual_half: Mutex::new(Vec::new()),
        }
    }

    /// The multiplier the noise metric applies to the means before the display tone-map (the
    /// `scale` of `AdaptiveRun::finish_pass`).
    #[must_use]
    pub const fn with_display_scale(mut self, scale: f32) -> Self {
        self.display_scale = scale;
        self
    }

    /// Whether the residual runs on the GPU-resident path (the `--gpu-convergence` render option);
    /// off by default. The batch falls back to the host path whenever the resident run cannot be
    /// used (see the module documentation).
    #[must_use]
    pub const fn with_gpu_convergence(mut self, on: bool) -> Self {
        self.gpu_convergence = on;
        self
    }

    /// Whether the residual is asked to take the GPU-resident path; when true the caller leaves
    /// out `NOTE_BEAMS_HOST_CONVERGENCE`.
    #[must_use]
    pub const fn uses_gpu_convergence(&self) -> bool {
        self.gpu_convergence
    }

    /// How many batch residuals completed on the GPU-resident path.
    ///
    /// A test of resident execution requires this to be positive, rather than trusting that the
    /// silent host fallback did not run.
    #[must_use]
    pub fn resident_completions(&self) -> u32 {
        self.resident_completions.load(Ordering::Relaxed)
    }

    /// How many batch residuals asked for the resident path but ran on the host path.
    #[must_use]
    pub fn resident_fallbacks(&self) -> u32 {
        self.resident_fallbacks.load(Ordering::Relaxed)
    }

    /// How many batches this engine was asked to shade.
    #[must_use]
    pub fn batches_seen(&self) -> u32 {
        self.batches_seen.load(Ordering::Relaxed)
    }

    /// How many requests (fragments or residual) this engine declined; a decline hands the batch
    /// to the next engine.
    #[must_use]
    pub fn declined_batches(&self) -> u32 {
        self.declined_batches.load(Ordering::Relaxed)
    }

    /// The reason of the most recent decline, `None` while there was none.
    #[must_use]
    pub fn last_decline_reason(&self) -> Option<&'static str> {
        *self
            .last_decline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts a decline in `outcome` and passes the outcome on unchanged.
    fn counted<T>(&self, outcome: EngineOutcome<T>) -> EngineOutcome<T> {
        if let EngineOutcome::Declined(reason) = &outcome {
            self.declined_batches.fetch_add(1, Ordering::Relaxed);
            *self
                .last_decline
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(*reason);
        }
        outcome
    }

    /// Takes the engine out of service: every later request declines with a lost-device reason,
    /// so the caller fails over to the next engine. The pipeline calls this when it has decided
    /// the device is not to be trusted.
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }

    /// Whether [`Self::retire`] was called.
    #[must_use]
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    /// The decline reason when the device cannot be used right now.
    fn unavailable(&self) -> Option<&'static str> {
        (self.is_retired() || self.backend.is_lost()).then_some(DECLINE_LOST)
    }

    /// The frame as the GPU entries take it.
    const fn gpu_scene(&self) -> GpuSceneRef<'a> {
        let frame = self.scene.frame;
        GpuSceneRef {
            camera: frame.camera,
            width: frame.width,
            height: frame.height,
            planes: frame.planes,
            facet_finishes: frame.facet_finishes,
            material: frame.material,
            max_bounces: frame.max_bounces,
            environment: frame.environment,
            estimator: Some(crate::optics::raytracer::Estimator::Branching),
        }
    }

    /// The frame size in pixels.
    const fn frame_size(&self) -> (u32, u32) {
        (self.scene.frame.width, self.scene.frame.height)
    }

    /// The fragment stage into a clean buffer of the batch's own pixels (compact slots, which are
    /// the batch layout), never a whole frame.
    fn shade(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        let scene = self.gpu_scene();
        let (width, height) = self.frame_size();
        let inside = batch
            .tiles
            .iter()
            .all(|tile| tile.x1 <= width && tile.y1 <= height);
        let slots = PixelSlots::new(batch.tiles).filter(|_| inside);
        let Some(slots) = slots else {
            return EngineOutcome::Declined(DECLINE_FRAME);
        };
        let mut image = take_zeroed(&self.fragment_image, slots.len());
        let outcome = self.backend.try_beam_fragments_compact(
            &scene,
            batch.packed_leaves,
            self.fragment_params,
            (&slots, &mut image),
            cancel,
        );
        let result = match outcome {
            GpuBeamFragments::Done => EngineOutcome::Done(BatchFragments { xyz: image.clone() }),
            GpuBeamFragments::Declined(reason) => EngineOutcome::Declined(reason),
            GpuBeamFragments::Cancelled => EngineOutcome::Cancelled,
        };
        // Whatever the outcome, the buffer goes back to zero.
        image.fill(Vec3::ZERO);
        give_back(&self.fragment_image, image);
        result
    }

    /// The residual passes of `run`, each traced by the GPU reference pipeline, the check on the
    /// host. `sums` is the batch's compact `(sum, half_b)` pair, `slots` its pixel map.
    fn run_passes(
        &self,
        job: &ResidualJob<'_, '_>,
        mut run: AdaptiveRun,
        (slots, sums): (&PixelSlots, (&mut [Vec3], &mut [Vec3])),
        progress: &mut dyn FnMut(f32),
    ) -> EngineOutcome<BatchResidual> {
        let scene = self.gpu_scene();
        let (sum, half_b) = sums;
        let pixels = job.batch.pixel_count();
        // The kept regions are uploaded once, by the first pass, and shared by every later pass of
        // the batch; `Some(None)` records a declined upload, and the passes then use the packed
        // entry, which makes the same decline decision itself.
        let mut kept: Option<Option<KeptBufferSet>> = None;
        while let Some(pass) = run.next_pass() {
            if job.cancel.load(Ordering::Relaxed) {
                return EngineOutcome::Cancelled;
            }
            if let Some(reason) = self.unavailable() {
                return EngineOutcome::Declined(reason);
            }
            let uploaded = kept.get_or_insert_with(|| {
                self.backend
                    .upload_reference_kept(job.batch.packed_kept, self.scene.n_lambda)
            });
            let compact = SlotSums {
                slots,
                sum: &mut *sum,
                half_b: &mut *half_b,
            };
            let outcome = if let Some(set) = uploaded {
                self.backend
                    .try_accumulate_reference_live_uploaded_compact_cancellable(
                        &scene,
                        self.scene.extras.0,
                        &self.scene.extras.2,
                        pass.first_sample,
                        pass.spp,
                        &pass.live,
                        set,
                        self.scene.n_lambda,
                        compact,
                        job.cancel,
                    )
            } else {
                self.backend
                    .try_accumulate_reference_live_compact_cancellable(
                        &scene,
                        self.scene.extras.0,
                        &self.scene.extras.2,
                        pass.first_sample,
                        pass.spp,
                        &pass.live,
                        job.batch.packed_kept,
                        self.scene.n_lambda,
                        compact,
                        job.cancel,
                    )
            };
            match outcome {
                GpuAccumulate::Done => {}
                GpuAccumulate::Declined => return EngineOutcome::Declined(DECLINE_PASS),
                GpuAccumulate::Cancelled => return EngineOutcome::Cancelled,
            }
            match run.pending_check() {
                Some(check) => {
                    let noises = compact_tile_noises(
                        &check,
                        job.layout,
                        job.offset,
                        (sum, half_b),
                        (self.display_scale, self.adaptive.metric),
                    );
                    run.finish_pass_with_tile_errors(&noises);
                }
                None => run.finish_pass_with_tile_errors(&[]),
            }
            progress(residual_share(
                run.samples_traced(),
                pixels,
                self.adaptive.max_spp,
            ));
        }
        EngineOutcome::Done(compact_residual_result(&run, job.layout, (sum, half_b)))
    }

    /// The residual of the batch on the GPU-resident path, or why the host path must redo it.
    ///
    /// Starts a resident run restricted to the batch's tiles with the batch's kept buffers and
    /// beam part, then drives `run` with GPU passes and GPU convergence checks and reads back the
    /// band of rows the batch covers.
    fn resident_residual(
        &self,
        job: &ResidualJob<'_, '_>,
        run: AdaptiveRun,
        (pass_spp, progress): (u32, &mut dyn FnMut(f32)),
    ) -> ResidentOutcome {
        let Some(tile_starts) = job
            .layout
            .iter()
            .map(|tile| {
                u32::try_from(tile.start)
                    .ok()
                    .map(|start| (tile.index, start))
            })
            .collect::<Option<Vec<(u32, u32)>>>()
        else {
            return ResidentOutcome::Unavailable;
        };
        let Some(first_row) = job.layout.first().map(|tile| tile.rect.y0) else {
            return ResidentOutcome::Unavailable;
        };
        let last_row = job.layout.iter().map(|tile| tile.rect.y1).max();
        let Some(last_row) = last_row else {
            return ResidentOutcome::Unavailable;
        };
        let scene = self.gpu_scene();
        let started = self.backend.try_adaptive_resident(
            &scene,
            self.scene.extras.0,
            &self.scene.extras.2,
            pass_spp,
        );
        let Some(resident) = started
            .and_then(|resident| {
                resident.restricted_to(&tile_starts, job.offset, (first_row, last_row))
            })
            .map(|resident| {
                resident.with_reference_kept(job.batch.packed_kept, self.scene.n_lambda)
            })
        else {
            return ResidentOutcome::Unavailable;
        };
        self.drive_resident(job, run, resident, (first_row, progress))
    }

    /// The passes and checks of `run` on the resident run `resident`; `first_row` is the first
    /// pixel row of the band it reads back, `progress` takes the share done after every pass.
    fn drive_resident(
        &self,
        job: &ResidualJob<'_, '_>,
        mut run: AdaptiveRun,
        mut resident: GpuResidentRun<'_>,
        (first_row, progress): (u32, &mut dyn FnMut(f32)),
    ) -> ResidentOutcome {
        let pixels = job.batch.pixel_count();
        while let Some(pass) = run.next_pass() {
            if job.cancel.load(Ordering::Relaxed) {
                return ResidentOutcome::Cancelled;
            }
            if self.unavailable().is_some() {
                return ResidentOutcome::Unavailable;
            }
            match resident.pass(&pass, job.cancel) {
                GpuAccumulate::Done => {}
                GpuAccumulate::Declined => return ResidentOutcome::Unavailable,
                GpuAccumulate::Cancelled => return ResidentOutcome::Cancelled,
            }
            match run.pending_check() {
                Some(check) => {
                    let measured =
                        resident.tile_errors(&check, self.display_scale, self.adaptive.metric);
                    let Some(noises) = measured else {
                        return ResidentOutcome::Unavailable;
                    };
                    run.finish_pass_with_tile_errors(&noises);
                }
                None => run.finish_pass_with_tile_errors(&[]),
            }
            progress(residual_share(
                run.samples_traced(),
                pixels,
                self.adaptive.max_spp,
            ));
        }
        let Some((sum, half_b)) = resident.finish_rows() else {
            return ResidentOutcome::Unavailable;
        };
        ResidentOutcome::Done(residual_result(
            &run,
            job.layout,
            (&sum, &half_b),
            (self.scene.frame.width, first_row),
        ))
    }
}

/// How a GPU-resident residual attempt ended.
enum ResidentOutcome {
    /// The batch's residual, from the GPU-resident run.
    Done(BatchResidual),
    /// The cancel token was observed set.
    Cancelled,
    /// The resident run could not start or died; the host path redoes the batch.
    Unavailable,
}

/// The finished run in the batch layout. `sums` holds the pixel rows from `first_row` on (the
/// whole frame for `first_row == 0`).
fn residual_result(
    run: &AdaptiveRun,
    layout: &[BatchTile],
    sums: (&[Vec3], &[Vec3]),
    (width, first_row): (u32, u32),
) -> BatchResidual {
    let mut result = BatchResidual::default();
    for tile in layout {
        let rect = tile.rect;
        for y in rect.y0..rect.y1 {
            let row = (y - first_row) as usize * width as usize;
            for x in rect.x0..rect.x1 {
                let pixel = row + x as usize;
                let count = run.pixel_spp(x, y);
                result
                    .sum
                    .push(sums.0.get(pixel).copied().unwrap_or(Vec3::ZERO));
                result
                    .half_b
                    .push(sums.1.get(pixel).copied().unwrap_or(Vec3::ZERO));
                result.counts.push(count);
                result.samples_traced += u64::from(count);
            }
        }
    }
    result
}

/// The finished run in the batch layout, from sums in compact slots (the batch layout itself, so
/// the sums are copied tile after tile, as laid out).
fn compact_residual_result(
    run: &AdaptiveRun,
    layout: &[BatchTile],
    sums: (&[Vec3], &[Vec3]),
) -> BatchResidual {
    let mut result = BatchResidual::default();
    for tile in layout {
        let rect = tile.rect;
        let mut slot = tile.start;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let count = run.pixel_spp(x, y);
                result
                    .sum
                    .push(sums.0.get(slot).copied().unwrap_or(Vec3::ZERO));
                result
                    .half_b
                    .push(sums.1.get(slot).copied().unwrap_or(Vec3::ZERO));
                result.counts.push(count);
                result.samples_traced += u64::from(count);
                slot += 1;
            }
        }
    }
    result
}

impl BeamEngine for GpuBeamEngine<'_> {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        self.batches_seen.fetch_add(1, Ordering::Relaxed);
        let outcome = self.fragments_uncounted(batch, cancel);
        self.counted(outcome)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.residual_with_progress(batch, offset, cancel, &mut |_| {})
    }

    fn residual_with_progress(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> EngineOutcome<BatchResidual> {
        let outcome = self.residual_uncounted(batch, offset, (cancel, progress));
        self.counted(outcome)
    }

    fn decline_is_per_batch(&self, reason: &'static str) -> bool {
        // The size caps: the next batch may fit. A lost device, a missing adapter or a failed pass
        // retire the engine.
        reason == DECLINE_TILE_OVER_LIMIT
            || reason == DECLINE_REDUCTION_OVER_LIMIT
            || reason == PACKED_KEPT_OVER_CAP
    }
}

impl GpuBeamEngine<'_> {
    /// [`BeamEngine::fragments`] without the decline count.
    fn fragments_uncounted(
        &self,
        batch: &Batch<'_>,
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchFragments> {
        if cancel.load(Ordering::Relaxed) {
            return EngineOutcome::Cancelled;
        }
        if batch.packed_leaves.headers.is_empty() {
            return EngineOutcome::Done(BatchFragments {
                xyz: vec![Vec3::ZERO; batch.pixel_count()],
            });
        }
        if let Some(reason) = self.unavailable() {
            return EngineOutcome::Declined(reason);
        }
        self.shade(batch, cancel)
    }

    /// [`BeamEngine::residual`] without the decline count.
    fn residual_uncounted(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        (cancel, progress): (&AtomicBool, &mut dyn FnMut(f32)),
    ) -> EngineOutcome<BatchResidual> {
        if cancel.load(Ordering::Relaxed) {
            return EngineOutcome::Cancelled;
        }
        let (width, height) = self.frame_size();
        let Some(layout) = batch_layout(batch.tiles, (width, height), self.adaptive.tile) else {
            return EngineOutcome::Declined(DECLINE_GRID);
        };
        if !offset.is_empty() && offset.len() != batch.pixel_count() {
            return EngineOutcome::Declined(DECLINE_OFFSET);
        }
        if layout.is_empty() {
            return EngineOutcome::Done(BatchResidual::default());
        }
        if let Some(reason) = self.unavailable() {
            return EngineOutcome::Declined(reason);
        }
        let settings = residual_settings(&self.adaptive, self.scene.residual_min_spp);
        let new_run = || {
            let live = LiveTiles::from_indices(
                width,
                height,
                settings.tile,
                layout.iter().map(|tile| tile.index).collect(),
            );
            // Sparse restricted state: no full-frame tile map per batch.
            AdaptiveRun::new_restricted(settings, width, height, &live)
                .ok()
                .map(|run| run.with_sample_offset(self.scene.sample_offset))
        };
        let job = ResidualJob {
            batch,
            layout: &layout,
            offset,
            cancel,
        };
        let Some(first_run) = new_run() else {
            return EngineOutcome::Declined(DECLINE_SETTINGS);
        };
        let run = if self.gpu_convergence {
            match self.resident_residual(&job, first_run, (settings.pass_spp, &mut *progress)) {
                ResidentOutcome::Done(result) => {
                    self.resident_completions.fetch_add(1, Ordering::Relaxed);
                    return EngineOutcome::Done(result);
                }
                ResidentOutcome::Cancelled => return EngineOutcome::Cancelled,
                ResidentOutcome::Unavailable => {
                    self.resident_fallbacks.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(
                        "GPU-resident beam residual unavailable, the batch's residual runs with \
                         the host convergence check"
                    );
                    // The resident run is dead and its sums lived on the GPU: start over.
                    new_run()
                }
            }
        } else {
            Some(first_run)
        };
        let Some(run) = run else {
            return EngineOutcome::Declined(DECLINE_SETTINGS);
        };
        self.host_residual(&job, run, progress)
    }
}

impl GpuBeamEngine<'_> {
    /// The residual of the batch with the host convergence check, on pooled sums sized by the
    /// batch's own pixels (`scratch_plan(..).compact`), not the frame.
    fn host_residual(
        &self,
        job: &ResidualJob<'_, '_>,
        run: AdaptiveRun,
        progress: &mut dyn FnMut(f32),
    ) -> EngineOutcome<BatchResidual> {
        let plan = scratch_plan(job.layout, self.frame_size());
        let rects: Vec<TileRect> = job.layout.iter().map(|tile| tile.rect).collect();
        let Some(slots) = PixelSlots::new(&rects) else {
            return EngineOutcome::Declined(DECLINE_GRID);
        };
        tracing::debug!(
            compact_pixels = plan.compact,
            band_pixels = plan.band,
            frame_pixels = plan.frame,
            "GPU host-convergence residual: compact sums"
        );
        let mut sum = take_zeroed(&self.residual_sum, slots.len());
        let mut half_b = take_zeroed(&self.residual_half, slots.len());
        let result = self.run_passes(job, run, (&slots, (&mut sum, &mut half_b)), progress);
        // The passes touched only the batch's slots: zero them and keep the buffers.
        sum.fill(Vec3::ZERO);
        half_b.fill(Vec3::ZERO);
        give_back(&self.residual_sum, sum);
        give_back(&self.residual_half, half_b);
        result
    }
}
