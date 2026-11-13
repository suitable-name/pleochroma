//! The batch pipeline: the CPU tree of batch n + 1 overlaps the engine work on batch n (beam guide
//! §12.4, §14.5).
//!
//! # Shape
//!
//! The tiles to render (all of the frame's adaptive tiles, or the ones a remote worker was asked
//! for) are cut, row-major, into batches of `BeamSettings::batch_tiles`. A producer thread traces
//! the beam trees of the batches one after the other (parallel over a batch's tiles, every tile on
//! one thread in a fixed order, the results taken in tile order) and hands each over a rendezvous
//! channel. The calling thread receives a batch, builds its kept index, and lets an engine shade
//! its fragments and run its residual. The channel holds nothing, so at most two batches exist at
//! a time: the one being shaded and the one just traced and waiting. A finished batch is dropped,
//! or handed to the `retain` hook (the leaf cache).
//!
//! # Engines
//!
//! A batch goes to `engines::assign(batch ordinal, failed)`. An engine that answers `Declined` is
//! marked failed for the rest of the render and the batch is run again, both stages, on the engine
//! `assign` names next; with every engine failed (or none given) the batch goes to the pipeline's
//! own CPU engine, which does not decline. A decline for a size cap
//! ([`BeamEngine::decline_is_per_batch`]: the next batch may fit) skips the engine for this batch
//! only. So the engine of a batch is a pure function of the batch ordinal and of the failure
//! history, and the failure history is a pure function of the failure point: the image does not
//! depend on timing. `Cancelled` stops the render.
//!
//! # The decline rule
//!
//! When the residual of the batches holding kept regions read so far needs more than
//! [`BEAM_RESIDUAL_SHARE_LIMIT`] of the plain render's samples on its stone pixels, the render
//! stops with [`BeamOff::ResidualTooLarge`] and the caller renders without beams. The rule reads
//! sample counts only, so it is deterministic.
//!
//! # The tree rule
//!
//! A render armed for the decline rule, which keeps no leaves and loads none, gives every tile a
//! work budget that follows the cost of the plain samples it replaces ([`tile_budget`]); a tile
//! that spends it hands its pending beams to the residual. Before the producer thread and the
//! engines start, a probe stage (`probe_trees`) traces the first [`BEAM_TREE_PROBE_TILES`] tiles
//! holding a primary beam, in tile order. When at least half of them spent their budget, the
//! render stops with [`BeamOff::TreeTooLarge`] and the caller renders without beams. The probe
//! stops as soon as the verdict is certain (`probe_verdict`), so the verdict does not depend on
//! timing or on the thread count. A declined render has traced the probe tiles and nothing else:
//! no batch, no engine call, no producer thread. When no engine of the set works on this
//! computer's CPU, the probe also has a time bound (`probe_time_bound`); a probe still undecided
//! then declines as well. The probe polls the cancel token every few milliseconds and reports no
//! progress. A render that passes traces the probe tiles again in its batches.
//!
//! # Memory
//!
//! At most two batches live at a time (the one being shaded and the one being traced), so each
//! gets half of `BeamSettings::memory_cap_bytes` ([`batch_share`]). The producer builds the trees
//! tile by tile and hands the batch over early when the running estimate ([`tile_cost`]: the
//! leaves, their kept regions, the kept index's memberships, counts and insertion cursors) would
//! pass that share, before the next tile joins it. The cut depends on the estimates in tile order
//! alone, never on timing or on the thread count. The parts of one chunk keep the chunk's ordinal,
//! so a part uses the engine of its chunk. A single tile whose estimate alone passes the share
//! becomes residual-only: its beams are dropped (the residual covers their energy) before its kept
//! index is built, it is a batch of its own, and it is not retained for the leaf cache. Nothing
//! fails. A part is a batch of its own for the residual's adaptive loop, so with adaptive stopping
//! the dilation across a part border reads only the part's own errors (the documented beam-mode
//! difference, guide §12.4). With a fixed sample count the image does not depend on the cut,
//! except for a residual-only tile.
//!
//! # Threads
//!
//! The producer and the consumer run at the same time, so a quota of `threads` is shared: the
//! producer takes half, rounded up ([`producer_threads`]), the consumer's CPU engine the rest.
//! When no engine of the set runs on this computer's CPU ([`BeamEngine::shares_cpu_threads`]), the
//! producer takes all but one thread ([`gpu_producer_threads`]).
//!
//! # Preloaded beams
//!
//! With `BeamRun::preloaded` a batch whose tiles are all in the cache skips its tree: the stored
//! leaves of those tiles become the batch (cut, kept index, fragments and the full adaptive
//! residual as for a fresh one), so the image equals a fresh render's bit for bit on the same
//! engine. A batch with a tile the cache lacks is traced whole. Batches taken from the cache are
//! not handed to `retain` again. Leaves recorded for a coarser exit-direction tolerance
//! (`tau_omega`) than the render's lighting needs are refined first, as `LeafCache::relight` does
//! (`LeafCache::refinements`), so loaded leaves match a fresh render under that lighting.
//!
//! # Cancel
//!
//! The token is polled by every loop that can run for a few milliseconds: per beam in the tree,
//! per job in [`run_jobs`], per tile in the kept-index build, per tile and residual sample in the
//! CPU engine, between pieces and batches, and every [`POLL_INTERVAL`] while the calling thread
//! waits for a tree. A cancelled render returns what it finished (the callers check the token) and
//! logs one debug line with the seconds the producer thread took to stop after the calling thread
//! did.
//!
//! # Progress
//!
//! `done = traced_fraction · w_tree + finished_fraction · (1 − w_tree)` with `w_tree` the tree's
//! share of the first batch's time, measured once and then fixed; `finished_fraction` counts the
//! batch in progress by the share of its residual budget its engine has traced so far (reported
//! after every adaptive pass, [`BeamEngine::residual_with_progress`]), so a long batch moves
//! the bar.
//! The reported value never decreases, and the last report is exactly 1. The ETA is
//! `elapsed · (1 − done) / done` once `done` passes a few percent.

use std::{
    borrow::Cow,
    collections::VecDeque,
    num::NonZeroUsize,
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    time::{Duration, Instant},
};

use glam::{DVec3, Vec3};

use super::{
    BatchHook, BeamOff, BeamPhase, BeamProgress, BeamRun, BeamSettings, ResidualMode,
    beam::{KeptRegion, Leaf, PackedKept, PackedLeaves},
    engines::{Batch, BatchResidual, BeamEngine, CpuBeamEngine, EngineOutcome, EngineSet, assign},
    frame::BeamFrame,
    leaf::pack_leaves_counted,
    leaf_cache::{LeafCache, Refinements, RetainedBatch},
    primary::TileRect,
    residual::{KeptIndex, kept_footprint, kept_memberships, tile_kept_index, try_pack_kept},
    stats::{BeamStats, TILE_ALLOWANCE},
    tile_residual::{ResidualScene, residual_share},
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::{
        accel_beam::{
            BEAM_DECLINE_AFTER_STONE_BATCHES, BEAM_DECLINE_MIN_CAP_RATIO, BEAM_MIN_TILE_BUDGET,
            BEAM_RESIDUAL_SHARE_LIMIT, BEAM_SAMPLES_PER_WORK_UNIT, BEAM_TREE_PROBE_MILLIS,
            BEAM_TREE_PROBE_TILES,
        },
        adaptive::{AdaptiveImage, AdaptiveSettings, LiveTiles},
        frame_scene::FrameScene,
        path_cache::eligibility::{StoneTraits, check_eligibility},
    },
    simd::PlanesSoA32,
};

/// How often the receiving side wakes while it waits for the next batch's trees, to poll the
/// cancel token and report the tracing progress.
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The tree's share of the work before the first batch has measured it.
pub const DEFAULT_TREE_WEIGHT: f64 = 0.5;

/// The smallest tree share the measurement may give (a batch with a free tree still has a tree).
pub const MIN_TREE_WEIGHT: f64 = 0.05;

/// The largest tree share the measurement may give (a batch with a free residual still has one).
pub const MAX_TREE_WEIGHT: f64 = 0.95;

/// The target of the per-batch log line.
const LOG_TARGET: &str = "pleochroma::beam";

/// Why a batch skips an engine whose packed kept index would pass the memory cap. It is a property
/// of the batch, so the engine keeps the following ones ([`BeamEngine::decline_is_per_batch`]).
pub const PACKED_KEPT_OVER_CAP: &str = "packed kept index over the memory cap";

thread_local! {
    /// The cap [`with_thread_limit`] puts on [`default_threads`] for the calling thread.
    static THREAD_LIMIT: std::cell::Cell<Option<NonZeroUsize>> = const { std::cell::Cell::new(None) };
}

/// The worker threads to use when the caller gives none: all available cores, capped by an
/// enclosing [`with_thread_limit`] on this thread.
#[must_use]
pub fn default_threads() -> usize {
    let all = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    THREAD_LIMIT
        .with(std::cell::Cell::get)
        .map_or(all, |limit| limit.get().min(all))
}

/// Runs `work` with [`default_threads`] capped at `threads` (at least 1) on the calling thread.
///
/// Every beam entry and leaf-cache operation started from it stays within one CPU quota. The
/// previous cap is restored afterwards, also when `work` panics.
pub fn with_thread_limit<R>(threads: usize, work: impl FnOnce() -> R) -> R {
    /// Restores the previous cap when dropped.
    struct Restore(Option<NonZeroUsize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD_LIMIT.with(|limit| limit.set(self.0));
        }
    }
    let limit = NonZeroUsize::new(threads.max(1));
    let _restore = Restore(THREAD_LIMIT.with(|cell| cell.replace(limit)));
    work()
}

/// Runs `work` for the indices `0..count` on `threads` workers taking indices from one counter and
/// returns the results in index order; `None` when `cancel` was set or a job gave nothing.
///
/// The jobs must not depend on which worker ran them, so the result is the same for any `threads`.
pub fn run_jobs<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: impl Fn(usize) -> Option<T> + Sync,
) -> Option<Vec<T>> {
    let threads = threads.clamp(1, count.max(1));
    let mut results: Vec<Option<T>> = Vec::new();
    results.resize_with(count, || None);
    if threads == 1 {
        for (index, slot) in results.iter_mut().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            *slot = work(index);
        }
    } else {
        let next = AtomicUsize::new(0);
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let done: Vec<Vec<(usize, Option<T>)>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        // A worker logs to the subscriber of the thread that started the jobs.
                        let _logging = tracing::dispatcher::set_default(&dispatch);
                        let mut mine = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            if index >= count || cancel.load(Ordering::Relaxed) {
                                break;
                            }
                            mine.push((index, work(index)));
                        }
                        mine
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| match worker.join() {
                    Ok(mine) => mine,
                    Err(payload) => std::panic::resume_unwind(payload),
                })
                .collect()
        });
        for (index, result) in done.into_iter().flatten() {
            if let Some(slot) = results.get_mut(index) {
                *slot = result;
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    results.into_iter().collect()
}

/// A sample count as `f32` (counts are far below 2^24, where `f32` is exact).
#[expect(
    clippy::cast_precision_loss,
    reason = "sample counts are far below 2^24, where f32 is exact"
)]
const fn count_f32(count: u32) -> f32 {
    count as f32
}

/// `part / whole` as `f64`; 1 for an empty whole (nothing to do is all done).
#[expect(
    clippy::cast_precision_loss,
    reason = "tile and batch counts are far below 2^52, where f64 is exact"
)]
fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        1.0
    } else {
        part as f64 / whole as f64
    }
}

/// `value` as `f32` for a progress report; the loss of precision is the point.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a progress fraction in 0..=1 loses nothing that matters in f32"
)]
const fn progress_f32(value: f64) -> f32 {
    value as f32
}

/// The number of samples with an odd absolute index among `count` samples starting at `first`.
const fn odd_samples(first: u32, count: u32) -> u32 {
    if first.is_multiple_of(2) {
        count / 2
    } else {
        count.div_ceil(2)
    }
}

/// `value` when every component is finite, else zero.
fn finite_or_zero(value: Vec3) -> Vec3 {
    if value.is_finite() { value } else { Vec3::ZERO }
}

/// One pixel of the hybrid image: the beam part plus the residual's sums.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HybridPixel {
    /// Every sample's sum with the beam part counted once per sample: `sum / count` is the
    /// unclamped total mean.
    pub sum: Vec3,
    /// The same for the odd-index samples (`half_b / odd` is the second half's total mean).
    pub half_b: Vec3,
    /// The final pixel: the beam part plus the residual mean, clamped at zero once (guide §5.3).
    pub mean: Vec3,
}

/// Combines a pixel's balanced beam part with its residual sums (`residual` is `(sum, half_b)`)
/// and the samples it received (`count`, of which `odd` had an odd absolute index).
///
/// A non-finite beam part counts as zero. A pixel without samples is just its beam part.
#[must_use]
pub fn combine_pixel(beam: Vec3, residual: (Vec3, Vec3), (count, odd): (u32, u32)) -> HybridPixel {
    let beam = finite_or_zero(beam);
    let (residual_sum, residual_half) = residual;
    let samples = count_f32(count);
    let mean = if count == 0 {
        beam
    } else {
        beam + residual_sum / samples
    };
    HybridPixel {
        sum: residual_sum + beam * samples,
        half_b: residual_half + beam * count_f32(odd),
        mean: mean.max(Vec3::ZERO),
    }
}

/// The bytes of one `u32` of the kept index.
const U32_BYTES: u64 = 4;

/// The `u32` tables the kept index holds per tile pixel while it is built: the offsets, the counts
/// and the insertion cursors.
const TABLES_PER_PIXEL: u64 = 3;

/// The share of the memory cap one batch may use: half of it, because at most two batches live at
/// a time (the one being shaded and the one being traced), and at least one byte.
#[must_use]
pub const fn batch_share(cap: u64) -> u64 {
    let half = cap / 2;
    if half == 0 { 1 } else { half }
}

/// `true` when a tile costing `cost` can join a batch that already costs `held` within `share`.
#[must_use]
pub const fn fits_share(held: u64, cost: u64, share: u64) -> bool {
    held.saturating_add(cost) <= share
}

/// The tile ranges the producer's rule cuts `costs` into: consecutive tiles join a batch while the
/// running estimate stays within `share`, and a tile that does not fit starts the next batch.
///
/// A tile that alone passes `share` is a batch of its own here (the producer makes it
/// residual-only first). The ranges cover `0..costs.len()` exactly, in order.
#[must_use]
pub fn runs_for_share(costs: &[u64], share: u64) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut held = 0_u64;
    for (index, &cost) in costs.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if fits_share(held, cost, share) => {
                run.end = index + 1;
                held = held.saturating_add(cost);
            }
            _ => {
                runs.push(index..index + 1);
                held = cost;
            }
        }
    }
    runs
}

/// The bytes a tile's leaves and kept index hold while the index is built.
///
/// That is the leaves with their vertex data, one kept region per leaf, the index's per-pixel
/// tables (offsets, counts, insertion cursors) and its pixel-to-region memberships, which grow
/// with the sum of the leaves' pixel-rectangle areas.
#[must_use]
pub fn tile_cost(rect: TileRect, leaves: &[Leaf]) -> u64 {
    let pixels = u64::try_from(rect.pixel_count()).unwrap_or(u64::MAX);
    let mut bytes = pixels.saturating_mul(TABLES_PER_PIXEL * U32_BYTES);
    for leaf in leaves {
        let own = size_of::<Leaf>()
            + leaf.dirs.len() * size_of::<Vec3>()
            + leaf.weights.len() * size_of::<f32>()
            + leaf.image.len() * size_of::<DVec3>()
            + size_of::<KeptRegion>();
        bytes = bytes.saturating_add(u64::try_from(own).unwrap_or(u64::MAX));
    }
    bytes.saturating_add(kept_memberships(rect, leaves).saturating_mul(U32_BYTES))
}

/// The threads of the tree producer out of a quota of `threads`: half, rounded up, at least one.
/// The consumer's engine takes [`consumer_threads`] of the same quota.
#[must_use]
pub const fn producer_threads(threads: usize) -> usize {
    let threads = if threads == 0 { 1 } else { threads };
    threads.div_ceil(2)
}

/// The threads of the tree producer when no engine of the set runs on this computer's CPU.
///
/// All but the one that drives the GPU or remote engine, at least one.
/// The pipeline's own CPU engine keeps [`consumer_threads`] for the batches it takes over after a
/// decline; such a batch may oversubscribe the cores for its duration, the price of the fallback.
#[must_use]
pub const fn gpu_producer_threads(threads: usize) -> usize {
    let threads = if threads == 0 { 1 } else { threads };
    if threads <= 2 { threads } else { threads - 1 }
}

/// The threads of the consumer's CPU engine out of a quota of `threads`: what the producer leaves,
/// at least one (a quota of one cannot be split, so both sides then run one thread each).
#[must_use]
pub const fn consumer_threads(threads: usize) -> usize {
    let rest = threads.saturating_sub(producer_threads(threads));
    if rest == 0 { 1 } else { rest }
}

/// Whether a quota of `threads` makes the tree side and the shading side take turns: a quota of
/// one cannot be split between them, so they must not run at the same time.
#[must_use]
pub const fn runs_in_turns(threads: usize) -> bool {
    threads <= 1
}

/// Cuts the tiles with `costs` into consecutive ranges that each fit `cap`, halving by tiles
/// (beam guide §14.5, §14.7). A range of one tile is kept whatever it costs: nothing fails.
///
/// Returns the ranges in order, covering `0..costs.len()` exactly, and the number of halvings.
#[must_use]
pub fn split_for_cap(costs: &[u64], cap: u64) -> (Vec<Range<usize>>, u32) {
    let mut ranges = Vec::new();
    let mut halvings = 0_u32;
    let mut stack: Vec<Range<usize>> = Vec::new();
    stack.push(0..costs.len());
    while let Some(range) = stack.pop() {
        if range.is_empty() {
            continue;
        }
        let total = costs.get(range.clone()).map_or(0, |part| {
            part.iter().fold(0_u64, |a, &c| a.saturating_add(c))
        });
        if total > cap && range.len() > 1 {
            let middle = range.start + range.len() / 2;
            halvings += 1;
            stack.push(middle..range.end);
            stack.push(range.start..middle);
        } else {
            ranges.push(range);
        }
    }
    (ranges, halvings)
}

/// The frame tile indices to render, ascending: every tile of the `tile`-pixel grid of a frame of
/// `size`, or the members of `restrict` when it is given.
///
/// A `restrict` built for another frame size or tile edge gives no tile (the rule of
/// `AdaptiveRun::restricted_to`).
#[must_use]
pub fn tile_list(size: (u32, u32), tile: u32, restrict: Option<&LiveTiles>) -> Vec<u32> {
    let (columns, rows) = TileRect::grid(size, tile);
    let count = columns.saturating_mul(rows);
    match restrict {
        None => (0..count).collect(),
        Some(live)
            if live.width() == size.0 && live.height() == size.1 && live.tile() == tile.max(1) =>
        {
            live.indices()
                .iter()
                .copied()
                .filter(|&index| index < count)
                .collect()
        }
        Some(_) => Vec::new(),
    }
}

/// The beam trees of one batch, as the tree side hands them over.
///
/// `'l` is the lifetime of the preloaded cache that lends the leaves of a loaded batch; a traced
/// batch owns its leaves and is valid for any `'l`.
pub struct TracedBatch<'l> {
    /// The batch's ordinal.
    pub index: u32,
    /// The batch's tiles, in tile order.
    pub tiles: Vec<TileRect>,
    /// `(tile index, leaves)` per tile, in the order of `tiles`: owned when traced, borrowed from
    /// the preloaded cache when loaded (nothing is copied).
    pub leaves: Vec<(u32, Cow<'l, [Leaf]>)>,
    /// The memory estimate of each tile of the batch ([`tile_cost`] of what the batch holds).
    pub costs: Vec<u64>,
    /// The tiles' counters, merged in tile order.
    pub stats: BeamStats,
    /// The seconds the trees took (the measurement of the tree share).
    pub seconds: f64,
    /// `true` when the leaves came from the preloaded cache (no tree was traced; the batch is
    /// not retained again).
    pub loaded: bool,
    /// `true` for the last part of the batch's chunk (a chunk is handed over in several parts when
    /// the memory estimate passes the batch's share).
    pub last: bool,
    /// `true` when the batch is one tile whose beams were dropped to the residual because the tile
    /// alone passed the batch's share; such a batch is not retained.
    pub residual_only: bool,
}

/// One traced tile on its way into a batch.
pub(super) struct TracedTile {
    /// The frame tile index.
    pub(super) index: u32,
    /// The tile's pixel rectangle.
    pub(super) rect: TileRect,
    /// The tile's leaves (none for a residual-only tile).
    pub(super) leaves: Vec<Leaf>,
    /// The tree's counters.
    pub(super) stats: BeamStats,
    /// The memory estimate of the tile as it is.
    pub(super) cost: u64,
    /// The tile's beams were dropped to the residual (see [`TracedBatch::residual_only`]).
    pub(super) residual_only: bool,
}

/// The batch the producer is filling.
#[derive(Default)]
struct OpenBatch {
    /// The ordinal of the chunk the batch belongs to.
    ordinal: u32,
    /// The tiles so far.
    tiles: Vec<TileRect>,
    /// `(tile index, leaves)` per tile; always owned.
    leaves: Vec<(u32, Cow<'static, [Leaf]>)>,
    /// The estimate of each tile.
    costs: Vec<u64>,
    /// The sum of `costs`.
    cost: u64,
    /// The tiles' counters, merged in tile order.
    stats: BeamStats,
    /// The seconds spent tracing for it.
    seconds: f64,
    /// A residual-only tile is in it (and so it stays alone).
    residual_only: bool,
}

impl OpenBatch {
    /// Adds `tile` to the batch.
    fn push(&mut self, tile: TracedTile) {
        self.stats.merge(&tile.stats);
        self.cost = self.cost.saturating_add(tile.cost);
        self.residual_only |= tile.residual_only;
        self.tiles.push(tile.rect);
        self.leaves.push((tile.index, Cow::Owned(tile.leaves)));
        self.costs.push(tile.cost);
    }

    /// Closes the batch (`last` marks the end of its chunk) and starts an empty one of the same
    /// chunk.
    fn close(&mut self, last: bool) -> TracedBatch<'static> {
        let done = std::mem::take(self);
        self.ordinal = done.ordinal;
        TracedBatch {
            index: done.ordinal,
            tiles: done.tiles,
            leaves: done.leaves,
            costs: done.costs,
            stats: done.stats,
            seconds: done.seconds,
            loaded: false,
            last,
            residual_only: done.residual_only,
        }
    }
}

/// Where the tiles of the preloaded cache wait, by frame tile index (a pure lookup table, so the
/// order of use never depends on a hash).
struct Loaded<'p> {
    /// The preloaded cache.
    cache: &'p LeafCache,
    /// Per frame tile: the `(batch, group)` position in the cache, when the cache holds a group
    /// whose rectangle is that tile of the render's grid.
    slots: Vec<Option<(usize, usize)>>,
    /// The groups whose leaves were traced for a coarser exit-direction tolerance than this
    /// render's lighting needs, refined here as `LeafCache::relight` does; a loaded group reads
    /// these leaves instead of the cache's.
    refined: Refinements,
}

impl<'p> Loaded<'p> {
    /// The lookup of `cache` for the `tile`-pixel grid of a frame of `size`, with the groups that
    /// the render's lighting needs finer (`refined`). A group whose rectangle is not the grid's
    /// rectangle of its index (another tile edge, another frame size) is left out, so a foreign
    /// cache simply loads nothing.
    fn new(cache: &'p LeafCache, refined: Refinements, size: (u32, u32), tile: u32) -> Self {
        let (columns, rows) = TileRect::grid(size, tile);
        let count = columns.saturating_mul(rows) as usize;
        let mut slots: Vec<Option<(usize, usize)>> = vec![None; count];
        for (b, batch) in cache.retained_batches().iter().enumerate() {
            if batch.leaves.len() != batch.tiles.len() {
                continue;
            }
            for (g, ((index, _), rect)) in batch.leaves.iter().zip(&batch.tiles).enumerate() {
                if TileRect::of_index(size, tile, *index) != Some(*rect) {
                    continue;
                }
                if let Some(slot) = slots.get_mut(*index as usize)
                    && slot.is_none()
                {
                    *slot = Some((b, g));
                }
            }
        }
        Self {
            cache,
            slots,
            refined,
        }
    }

    /// The cached groups of `chunk` as borrowed leaves with their estimates, or `None` when any
    /// tile of the chunk is not in the cache (the chunk is then traced as a whole).
    ///
    /// Nothing is copied here or later: the cache lends its leaves, and [`Self::part`] hands them
    /// on as [`Cow::Borrowed`].
    fn locate(&self, chunk: &[u32]) -> Option<Vec<Located<'_>>> {
        let batches = self.cache.retained_batches();
        let mut found = Vec::with_capacity(chunk.len());
        for &index in chunk {
            let (b, g) = (*self.slots.get(index as usize)?)?;
            let batch = batches.get(b)?;
            let rect = *batch.tiles.get(g)?;
            let (_, stored) = batch.leaves.get(g)?;
            let leaves = self.refined.get((b, g)).unwrap_or(stored.as_slice());
            found.push(Located {
                index,
                rect,
                leaves,
                cost: tile_cost(rect, leaves),
            });
        }
        Some(found)
    }

    /// The batch `ordinal` made of the located tiles `part`, lending their leaves (no leaf is
    /// copied); `last` marks the end of the chunk.
    fn part<'l>(ordinal: u32, part: &[Located<'l>], last: bool) -> TracedBatch<'l> {
        let mut stats = BeamStats::default();
        for tile in part {
            stats.leaves += u64::try_from(tile.leaves.len()).unwrap_or(u64::MAX);
        }
        TracedBatch {
            index: ordinal,
            tiles: part.iter().map(|tile| tile.rect).collect(),
            leaves: part
                .iter()
                .map(|tile| (tile.index, Cow::Borrowed(tile.leaves)))
                .collect(),
            costs: part.iter().map(|tile| tile.cost).collect(),
            stats,
            seconds: 0.0,
            loaded: true,
            last,
            residual_only: false,
        }
    }
}

/// A tile of the preloaded cache, located but not copied.
struct Located<'c> {
    /// The frame tile index.
    index: u32,
    /// The tile's pixel rectangle.
    rect: TileRect,
    /// The cache's leaves of the tile (refined where the render's lighting needs it).
    leaves: &'c [Leaf],
    /// The memory estimate of the tile.
    cost: u64,
}

/// What the producer thread reads, fixed for the render.
struct Producer<'a, 'f, 'g, 'l> {
    /// The beam frame.
    frame: &'a BeamFrame<'f, 'g>,
    /// The beam tile edge.
    tile: u32,
    /// The tree workers (a wave traces this many tiles at a time).
    threads: usize,
    /// The memory a batch may use ([`batch_share`] of the cap).
    share: u64,
    /// Where the batches go.
    sender: &'a SyncSender<TracedBatch<'l>>,
    /// With a thread quota of one, the consumer's "batch finished" signals: the producer waits for
    /// one after every hand-over, so tracing and shading take turns and the quota holds. `None`
    /// when the quota lets them overlap.
    turns: Option<&'a Receiver<()>>,
    /// The preloaded cache, when there is one; it lends its leaves to the batches for `'l`.
    loaded: Option<&'l Loaded<'l>>,
    /// The tiles traced so far (for the progress).
    traced_tiles: &'a AtomicUsize,
    /// The cancel token.
    cancel: &'a AtomicBool,
}

/// How often the probe stage wakes to poll the cancel token and its time bound.
const PROBE_POLL: Duration = Duration::from_millis(5);

/// How many tiles the probe stage tests for a primary beam per parallel block.
const PROBE_SCAN_BLOCK: usize = 256;

/// What the probe stage decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProbeOutcome {
    /// The trees are affordable, or the rule does not apply: the render goes on with beams.
    Go,
    /// At least half of the probe tiles spent their work budget.
    Declined,
    /// The time bound passed before the verdict was certain.
    TimedOut,
    /// The cancel token was observed set.
    Cancelled,
}

/// The tree rule's verdict (`true`: decline) as soon as it is certain, from the probe tiles read
/// so far: `spent` of them spent their budget, `unspent` finished within it, and `stone_tiles` is
/// how many tiles the probe reads.
///
/// The rule declines when at least half of [`BEAM_TREE_PROBE_TILES`] stone tiles spent their
/// budget, and never with fewer stone tiles. `None` while the tiles still to read can change the
/// answer. Once it is `Some`, more tiles never change it, so the verdict does not depend on the
/// order in which the tiles finish.
#[must_use]
pub(super) const fn probe_verdict(spent: u32, unspent: u32, stone_tiles: u32) -> Option<bool> {
    if stone_tiles < BEAM_TREE_PROBE_TILES {
        return Some(false);
    }
    if spent.saturating_mul(2) >= stone_tiles {
        return Some(true);
    }
    // Even if every tile not read yet spent its budget, fewer than half would have.
    if stone_tiles.saturating_sub(unspent).saturating_mul(2) < stone_tiles {
        return Some(false);
    }
    None
}

/// The time bound of the tree probe for a tile budget of `allowance` on `threads` workers:
/// [`BEAM_TREE_PROBE_MILLIS`] per wave of probe tiles, scaled with the budget over
/// [`BEAM_MIN_TILE_BUDGET`] (a larger budget pays for a larger plain render).
#[must_use]
pub(super) fn probe_time_bound(allowance: u64, threads: usize) -> Duration {
    let workers = u64::try_from(threads.max(1)).unwrap_or(u64::MAX);
    let waves = u64::from(BEAM_TREE_PROBE_TILES).div_ceil(workers);
    let per_wave = BEAM_TREE_PROBE_MILLIS.saturating_mul(allowance.max(BEAM_MIN_TILE_BUDGET))
        / BEAM_MIN_TILE_BUDGET;
    Duration::from_millis(per_wave.saturating_mul(waves))
}

/// The first [`BEAM_TREE_PROBE_TILES`] tiles of `tiles` (frame tile indices, in order) that hold a
/// primary beam; fewer when the render has fewer. `None` when cancelled.
fn first_stone_tiles(
    frame: &BeamFrame<'_, '_>,
    tile: u32,
    tiles: &[u32],
    threads: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u32>> {
    let wanted = usize::try_from(BEAM_TREE_PROBE_TILES).unwrap_or(usize::MAX);
    let mut stone = Vec::with_capacity(wanted);
    for block in tiles.chunks(PROBE_SCAN_BLOCK) {
        let flags = run_jobs(block.len(), threads, cancel, |job| {
            let index = usize::try_from(*block.get(job)?).ok()?;
            Some(frame.tile_has_primary(index, tile))
        })?;
        for (&index, has_primary) in block.iter().zip(flags) {
            if has_primary {
                stone.push(index);
                if stone.len() == wanted {
                    return Some(stone);
                }
            }
        }
    }
    Some(stone)
}

/// What the traced probe tiles gave.
struct ProbeCount {
    /// Tiles that spent their work budget.
    spent: u32,
    /// Tiles that finished within it.
    unspent: u32,
    /// The time bound passed while the probe ran (or before it started).
    timed_out: bool,
}

/// Traces the probe tiles `stone` on `threads` workers against the frame's tile allowance until
/// [`probe_verdict`] is certain, the bound `deadline` (counted from `started`) has passed or
/// `cancel` is set, and counts the tiles that finished.
///
/// A bound that has passed already traces nothing. The traced leaves are dropped.
fn count_probe_tiles(
    frame: &BeamFrame<'_, '_>,
    tile: u32,
    stone: &[u32],
    threads: usize,
    (started, deadline): (Instant, Option<Duration>),
    cancel: &AtomicBool,
) -> ProbeCount {
    let stone_tiles = u32::try_from(stone.len()).unwrap_or(u32::MAX);
    let allowance = frame.tile_allowance();
    let spent = AtomicU32::new(0);
    let unspent = AtomicU32::new(0);
    let mut timed_out = deadline.is_some_and(|limit| started.elapsed() >= limit);
    // The probe's own stop flag: set at the verdict, at the time bound and for the user's token,
    // which is read here and never set.
    let stop = AtomicBool::new(timed_out);
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    std::thread::scope(|scope| {
        let tracer = scope.spawn(|| {
            // The tiles log on behalf of the render, to the subscriber of the calling thread.
            let _logging = tracing::dispatcher::set_default(&dispatch);
            // The verdict is read from the counters: a probe stopped early returns `None` here.
            let _ = run_jobs(stone.len(), threads, &stop, |job| {
                let index = usize::try_from(*stone.get(job)?).ok()?;
                let (_, stats, _) = frame.tile_leaves(index, tile, &stop)?;
                let counter = if stats.allowance_spent_at(allowance) {
                    &spent
                } else {
                    &unspent
                };
                counter.fetch_add(1, Ordering::Relaxed);
                let verdict = probe_verdict(
                    spent.load(Ordering::Relaxed),
                    unspent.load(Ordering::Relaxed),
                    stone_tiles,
                );
                if verdict.is_some() {
                    stop.store(true, Ordering::Relaxed);
                }
                Some(())
            });
        });
        while !tracer.is_finished() {
            timed_out |= deadline.is_some_and(|limit| started.elapsed() >= limit);
            if timed_out || cancel.load(Ordering::Relaxed) {
                stop.store(true, Ordering::Relaxed);
            }
            std::thread::sleep(PROBE_POLL);
        }
    });
    ProbeCount {
        spent: spent.load(Ordering::Relaxed),
        unspent: unspent.load(Ordering::Relaxed),
        timed_out,
    }
}

/// The probe stage of the tree rule: traces the first [`BEAM_TREE_PROBE_TILES`] stone tiles of
/// `tiles` on `threads` workers against the frame's tile allowance and stops as soon as
/// [`probe_verdict`] is certain, `deadline` has passed or `cancel` is set.
///
/// Nothing of the pipeline runs yet: a declined render has cost these trees and nothing else. A
/// render that goes on traces the tiles again in its batches.
pub(super) fn probe_trees(
    frame: &BeamFrame<'_, '_>,
    tile: u32,
    tiles: &[u32],
    threads: usize,
    deadline: Option<Duration>,
    cancel: &AtomicBool,
) -> ProbeOutcome {
    let started = Instant::now();
    let Some(stone) = first_stone_tiles(frame, tile, tiles, threads, cancel) else {
        return ProbeOutcome::Cancelled;
    };
    let stone_tiles = u32::try_from(stone.len()).unwrap_or(u32::MAX);
    if probe_verdict(0, 0, stone_tiles) == Some(false) {
        // Fewer stone tiles than the rule reads: nothing to trace.
        return ProbeOutcome::Go;
    }
    let count = count_probe_tiles(frame, tile, &stone, threads, (started, deadline), cancel);
    if cancel.load(Ordering::Relaxed) {
        return ProbeOutcome::Cancelled;
    }
    let (spent, unspent) = (count.spent, count.unspent);
    let allowance = frame.tile_allowance();
    let seconds = started.elapsed().as_secs_f64();
    match probe_verdict(spent, unspent, stone_tiles) {
        Some(true) => {
            tracing::info!(
                target: LOG_TARGET,
                spent,
                tiles = spent + unspent,
                allowance,
                seconds,
                "beam render abandoned: the first stone tiles spent their work budget, the \
                 picture renders without beams"
            );
            ProbeOutcome::Declined
        }
        None if count.timed_out => {
            tracing::info!(
                target: LOG_TARGET,
                spent,
                tiles = spent + unspent,
                allowance,
                seconds,
                "beam render abandoned: the tree probe ran out of time, the picture renders \
                 without beams"
            );
            ProbeOutcome::TimedOut
        }
        // `None` without a time-out cannot happen (every probe tile is a tile of the grid); the
        // render then goes on with beams, whose residual rule still holds.
        Some(false) | None => {
            tracing::debug!(
                target: LOG_TARGET,
                spent,
                unspent,
                allowance,
                seconds,
                "beam tree probe passed"
            );
            ProbeOutcome::Go
        }
    }
}

/// The work budget of one tile of a render capped at `max_spp` samples per pixel.
///
/// It is the attempted work whose cost is a fraction of the tile's plain render
/// ([`BEAM_SAMPLES_PER_WORK_UNIT`]), between [`BEAM_MIN_TILE_BUDGET`] and [`TILE_ALLOWANCE`]: a
/// function of the settings alone.
#[must_use]
pub fn tile_budget(tile: u32, max_spp: u32) -> u64 {
    let samples = u64::from(tile)
        .saturating_mul(u64::from(tile))
        .saturating_mul(u64::from(max_spp));
    (samples / BEAM_SAMPLES_PER_WORK_UNIT).clamp(BEAM_MIN_TILE_BUDGET, TILE_ALLOWANCE)
}

impl<'l> Producer<'_, '_, '_, 'l> {
    /// Traces the tiles `wave` (frame tile indices), in parallel and one thread each, and returns
    /// them in the order of `wave` with the seconds the wave took; `None` when cancelled.
    ///
    /// Each tile is priced and admitted on the thread that traced it, the moment its tree is built
    /// ([`admit_tile`]), so a tile over the share never waits unpriced with the rest of its wave.
    fn trace_wave(&self, wave: &[u32]) -> Option<(Vec<TracedTile>, f64)> {
        let (frame, tile, cancel, counter) =
            (self.frame, self.tile, self.cancel, self.traced_tiles);
        let share = self.share;
        let started = Instant::now();
        let tiles = run_jobs(wave.len(), self.threads, cancel, |job| {
            let index = *wave.get(job)?;
            let (leaves, stats, rect) =
                frame.tile_leaves(usize::try_from(index).ok()?, tile, cancel)?;
            counter.fetch_add(1, Ordering::Relaxed);
            let cost = tile_cost(rect, &leaves);
            Some(admit_tile(
                TracedTile {
                    index,
                    rect,
                    leaves,
                    stats,
                    cost,
                    residual_only: false,
                },
                share,
            ))
        })?;
        Some((tiles, started.elapsed().as_secs_f64()))
    }

    /// Hands the preloaded `chunk` of batch `ordinal` over in parts that each fit the share,
    /// copying the leaves of one part at a time (the cache lends them until then). `false` when
    /// the receiver is gone.
    ///
    /// The cut is the producer's rule on the estimates in tile order, [`runs_for_share`]; a tile
    /// over the share alone is a part of its own and the receiving side keeps it whole.
    fn produce_loaded(&self, ordinal: u32, located: &[Located<'l>]) -> bool {
        let costs: Vec<u64> = located.iter().map(|tile| tile.cost).collect();
        let runs = runs_for_share(&costs, self.share);
        let count = runs.len();
        for (at, run) in runs.into_iter().enumerate() {
            let Some(part) = located.get(run) else {
                return false;
            };
            if !self.hand_over(Loaded::part(ordinal, part, at + 1 == count)) {
                return false;
            }
        }
        true
    }

    /// Hands `batch` to the receiving side; `false` when it is gone.
    ///
    /// With a quota of one it then waits until the receiving side has finished the batch.
    fn hand_over(&self, batch: TracedBatch<'l>) -> bool {
        tracing::debug!(
            target: LOG_TARGET,
            batch = batch.index,
            tiles = batch.tiles.len(),
            beams = batch.stats.beams,
            leaves = batch.stats.leaves,
            seconds = batch.seconds,
            last = batch.last,
            "beam batch traced"
        );
        if self.sender.send(batch).is_err() {
            return false;
        }
        self.turns.is_none_or(|turns| turns.recv().is_ok())
    }

    /// Hands over the chunk `chunk` of batch `ordinal`: from the preloaded cache when it holds
    /// every tile, else traced wave by wave. A batch is closed early, before a tile that would
    /// pass the share joins it, and a residual-only tile is a batch of its own; the cut depends
    /// on the estimates in tile order alone. `false` when cancelled or the receiver is gone.
    fn produce_chunk(&self, ordinal: u32, chunk: &[u32]) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(located) = self.loaded.and_then(|loaded| loaded.locate(chunk)) {
            self.traced_tiles.fetch_add(chunk.len(), Ordering::Relaxed);
            return self.produce_loaded(ordinal, &located);
        }
        let mut open = OpenBatch {
            ordinal,
            ..OpenBatch::default()
        };
        let mut carry: VecDeque<TracedTile> = VecDeque::new();
        let mut waves = chunk.chunks(self.threads.max(1));
        loop {
            let Some(tile) = carry.pop_front() else {
                let Some(wave) = waves.next() else { break };
                let Some((tiles, seconds)) = self.trace_wave(wave) else {
                    return false;
                };
                open.seconds += seconds;
                carry.extend(tiles);
                continue;
            };
            let joins = !open.residual_only
                && !tile.residual_only
                && fits_share(open.cost, tile.cost, self.share);
            if !open.tiles.is_empty() && !joins {
                tracing::debug!(
                    target: LOG_TARGET,
                    batch = ordinal,
                    bytes = open.cost,
                    share = self.share,
                    "beam batch handed over early for the memory cap"
                );
                if !self.hand_over(open.close(false)) {
                    return false;
                }
            }
            open.push(tile);
        }
        self.hand_over(open.close(true))
    }
}

/// Makes `tile` residual-only when its estimate alone passes `share`: its beams are dropped,
/// and the residual covers its pixels. Called as each tree is built, so the leaves of an
/// over-share tile are freed before the rest of the wave finishes.
fn admit_tile(mut tile: TracedTile, share: u64) -> TracedTile {
    if tile.cost > share {
        tracing::debug!(
            target: LOG_TARGET,
            tile = tile.index,
            bytes = tile.cost,
            share,
            "beam tile over the memory cap, its beams go to the residual"
        );
        tile.leaves = Vec::new();
        tile.cost = tile_cost(tile.rect, &[]);
        tile.residual_only = true;
    }
    tile
}

/// The producer thread: traces the batches in order (or takes them from the preloaded cache) and
/// hands each over, stopping at the end, at a cancel, or when the receiving side is gone.
fn produce(producer: &Producer<'_, '_, '_, '_>, chunks: &[&[u32]]) {
    for (ordinal, chunk) in chunks.iter().enumerate() {
        let ordinal = u32::try_from(ordinal).unwrap_or(u32::MAX);
        if !producer.produce_chunk(ordinal, chunk) {
            return;
        }
    }
}

/// The two-phase progress of a render (guide §12.4).
struct Tracker<'h> {
    /// Where the reports go.
    sink: &'h mut dyn FnMut(BeamProgress),
    /// The batches of the render.
    batches: usize,
    /// The tiles of the render.
    tiles: usize,
    /// When the render started, for the ETA.
    started: Instant,
    /// The tree's share of the work, once the first batch has measured it.
    weight: Option<f64>,
    /// The last value reported, so the reports never decrease.
    last: f32,
    /// Every tile's tree is traced: the phase stays [`BeamPhase::Finishing`] from then on.
    finishing: bool,
    /// The share of the batch in progress that its engine has finished (0 to 1).
    partial: f64,
    /// The finished-batch count `partial` belongs to; a new count resets it.
    partial_of: usize,
}

/// The smallest finished fraction from which the ETA is extrapolated.
const MIN_ETA_FRACTION: f64 = 0.02;

impl<'h> Tracker<'h> {
    /// A tracker for `batches` batches over `tiles` tiles reporting to `sink`.
    fn new(sink: &'h mut dyn FnMut(BeamProgress), batches: usize, tiles: usize) -> Self {
        Self {
            sink,
            batches,
            tiles,
            started: Instant::now(),
            weight: None,
            last: 0.0,
            finishing: false,
            partial: 0.0,
            partial_of: 0,
        }
    }

    /// Forgets the share of the previous part: the next traced batch starts at zero.
    const fn start_part(&mut self) {
        self.partial = 0.0;
    }

    /// Starts a new `partial` when the finished-batch count moved on.
    const fn settle_partial(&mut self, finished: usize) {
        if finished != self.partial_of {
            self.partial_of = finished;
            self.partial = 0.0;
        }
    }

    /// Fixes the tree's share from the first batch: its tree seconds against its engine seconds.
    fn fix_weight(&mut self, tree_seconds: f64, engine_seconds: f64) {
        let total = tree_seconds + engine_seconds;
        let share = if total > 0.0 {
            tree_seconds / total
        } else {
            DEFAULT_TREE_WEIGHT
        };
        self.weight = Some(share.clamp(MIN_TREE_WEIGHT, MAX_TREE_WEIGHT));
    }

    /// The share done with `traced_tiles` tiles traced and `finished` batches finished.
    fn fraction(&self, traced_tiles: usize, finished: usize) -> f64 {
        let weight = self.weight.unwrap_or(DEFAULT_TREE_WEIGHT);
        let tree = ratio(traced_tiles, self.tiles);
        let batches = ratio_count(self.batches.max(1));
        let engines = ((ratio_count(finished) + self.partial) / batches).min(1.0);
        weight
            .mul_add(tree, (1.0 - weight) * engines)
            .clamp(0.0, 1.0)
    }

    /// The time left from the fraction done (`elapsed · (1 − f) / f`); `None` before a batch is
    /// finished and while the fraction is below [`MIN_ETA_FRACTION`].
    fn eta(&self, finished: usize, fraction: f64) -> Option<Duration> {
        if finished == 0 || fraction < MIN_ETA_FRACTION {
            return None;
        }
        let left = self.started.elapsed().as_secs_f64() * (1.0 - fraction) / fraction;
        Duration::try_from_secs_f64(left.max(0.0)).ok()
    }

    /// Reports the state: never below the last report, never 1 before [`Tracker::finish`].
    ///
    /// The phase is [`BeamPhase::Tracing`] until every tile's tree is traced and
    /// [`BeamPhase::Finishing`] from then on (beam guide §12.4: two phases, in that order). The
    /// pipeline finishes one batch while it traces the next, so the step it is waiting on does not
    /// name the phase: it would flip back and forth with every batch.
    fn report(&mut self, traced_tiles: usize, finished: usize) {
        self.settle_partial(finished);
        self.finishing |= traced_tiles >= self.tiles;
        let phase = if self.finishing {
            BeamPhase::Finishing
        } else {
            BeamPhase::Tracing
        };
        let fraction = self.fraction(traced_tiles, finished);
        let done = progress_f32(fraction);
        let done = done.max(self.last).min(1.0);
        self.last = done;
        let eta = self.eta(finished, f64::from(done));
        (self.sink)(BeamProgress { phase, done, eta });
    }

    /// [`Tracker::report`] with the engine's share of the batch in progress (0 to 1, the largest
    /// share seen for this batch counts).
    fn report_partial(&mut self, traced_tiles: usize, finished: usize, share: f32) {
        self.settle_partial(finished);
        self.partial = self.partial.max(f64::from(share).clamp(0.0, 1.0));
        self.report(traced_tiles, finished);
    }

    /// The last report: everything done.
    fn finish(&mut self) {
        self.last = 1.0;
        (self.sink)(BeamProgress {
            phase: BeamPhase::Finishing,
            done: 1.0,
            eta: Some(Duration::ZERO),
        });
    }
}

/// A count as `f64`.
#[expect(
    clippy::cast_precision_loss,
    reason = "batch counts are far below 2^52, where f64 is exact"
)]
const fn ratio_count(count: usize) -> f64 {
    count as f64
}

/// A sample or pixel total as `f64`.
#[expect(
    clippy::cast_precision_loss,
    reason = "sample totals are far below 2^52, where f64 is exact"
)]
const fn ratio_count_u64(count: u64) -> f64 {
    count as f64
}

/// Where the pixels of a tile go in an [`Accum`].
enum Layout {
    /// The whole frame, row-major.
    Frame {
        /// The frame width.
        width: usize,
    },
    /// Only the requested tiles, in request order, row-major inside a tile.
    Tiles {
        /// The frame tile indices, ascending.
        ids: Vec<u32>,
        /// The first pixel of each tile in the buffers.
        bases: Vec<usize>,
    },
}

/// The image the batches add up to: frame-sized and row-major, or only the requested tiles.
pub struct Accum {
    /// Where a tile's pixels go.
    layout: Layout,
    /// See [`AdaptiveImage::sum`] (the beam part counted once per sample).
    pub sum: Vec<Vec3>,
    /// See [`AdaptiveImage::half_b`] (the beam part counted once per odd sample).
    pub half_b: Vec<Vec3>,
    /// Residual samples per pixel.
    pub counts: Vec<u32>,
    /// The final pixels.
    pub pixels: Vec<Vec3>,
    /// Residual samples traced.
    pub samples_traced: u64,
    /// The counters, merged in tile order.
    pub stats: BeamStats,
}

impl Accum {
    /// A zero image of `size`.
    #[must_use]
    pub fn new(size: (u32, u32)) -> Self {
        let pixels = size.0 as usize * size.1 as usize;
        Self {
            layout: Layout::Frame {
                width: size.0 as usize,
            },
            sum: vec![Vec3::ZERO; pixels],
            half_b: vec![Vec3::ZERO; pixels],
            counts: vec![0; pixels],
            pixels: vec![Vec3::ZERO; pixels],
            samples_traced: 0,
            stats: BeamStats::default(),
        }
    }

    /// A zero image holding only the tiles `ids` (ascending frame tile indices of the `tile`-pixel
    /// grid of a frame of `size`), in that order and row-major inside a tile.
    #[must_use]
    pub fn tiled(size: (u32, u32), tile: u32, ids: &[u32]) -> Self {
        let mut bases = Vec::with_capacity(ids.len());
        let mut pixels = 0_usize;
        for &id in ids {
            bases.push(pixels);
            pixels += TileRect::of_index(size, tile, id).map_or(0, TileRect::pixel_count);
        }
        Self {
            layout: Layout::Tiles {
                ids: ids.to_vec(),
                bases,
            },
            sum: vec![Vec3::ZERO; pixels],
            half_b: vec![Vec3::ZERO; pixels],
            counts: vec![0; pixels],
            pixels: vec![Vec3::ZERO; pixels],
            samples_traced: 0,
            stats: BeamStats::default(),
        }
    }

    /// The buffer index of the first pixel of tile `id` (rectangle `rect`) and the stride between
    /// its rows; `None` when the image holds no such tile.
    fn place(&self, id: u32, rect: TileRect) -> Option<(usize, usize)> {
        match &self.layout {
            Layout::Frame { width } => Some((rect.y0 as usize * width + rect.x0 as usize, *width)),
            Layout::Tiles { ids, bases } => {
                let slot = ids.binary_search(&id).ok()?;
                Some((*bases.get(slot)?, rect.width() as usize))
            }
        }
    }

    /// Writes a finished piece (the tiles `ids`) into the image and returns its final pixels in
    /// batch order.
    fn store(
        &mut self,
        (tiles, ids): (&[TileRect], &[u32]),
        run: &EngineRun,
        sample_offset: u32,
    ) -> Vec<Vec3> {
        let mut means = Vec::with_capacity(run.offset.len());
        let mut at = 0_usize;
        for (rect, &id) in tiles.iter().zip(ids) {
            let place = self.place(id, *rect);
            for row in 0..rect.height() as usize {
                for column in 0..rect.width() as usize {
                    let count = run.residual.counts.get(at).copied().unwrap_or(0);
                    let pixel = combine_pixel(
                        run.offset.get(at).copied().unwrap_or(Vec3::ZERO),
                        (
                            run.residual.sum.get(at).copied().unwrap_or(Vec3::ZERO),
                            run.residual.half_b.get(at).copied().unwrap_or(Vec3::ZERO),
                        ),
                        (count, odd_samples(sample_offset, count)),
                    );
                    let target = place.map(|(first, stride)| first + row * stride + column);
                    if let Some((((sum, half_b), counts), mean)) = target.and_then(|t| {
                        self.sum
                            .get_mut(t)
                            .zip(self.half_b.get_mut(t))
                            .zip(self.counts.get_mut(t))
                            .zip(self.pixels.get_mut(t))
                    }) {
                        *sum = pixel.sum;
                        *half_b = pixel.half_b;
                        *counts = count;
                        *mean = pixel.mean;
                    }
                    means.push(pixel.mean);
                    at += 1;
                }
            }
        }
        self.samples_traced += run.residual.samples_traced;
        means
    }
}

/// What the engine side of a piece produced.
struct EngineRun {
    /// The balanced beam part per piece pixel (the residual's offset).
    offset: Vec<Vec3>,
    /// The residual.
    residual: BatchResidual,
    /// The counters of the fragment stage (fragments shaded, pieces, fragment-cap drops).
    fragments: BeamStats,
    /// The engine's name, for the log.
    engine: String,
}

impl EngineRun {
    /// A piece of `pixels` pixels with nothing in it: what a piece gets when even the CPU engine
    /// refused it (a batch tile off the adaptive grid; the pipeline cuts on the grid, so this is a
    /// guard, not a path).
    fn empty(pixels: usize) -> Self {
        Self {
            offset: vec![Vec3::ZERO; pixels],
            residual: BatchResidual {
                sum: vec![Vec3::ZERO; pixels],
                half_b: vec![Vec3::ZERO; pixels],
                counts: vec![0; pixels],
                samples_traced: 0,
            },
            fragments: BeamStats::default(),
            engine: String::from("none"),
        }
    }
}

/// How one engine fared with a piece.
enum Attempt {
    /// Both stages are done.
    Done(Box<EngineRun>),
    /// The engine declined; the text is the reason for the log.
    Declined(&'static str),
    /// The cancel token was observed set.
    Cancelled,
}

/// Why the engine side stopped before the last batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// The cancel token was observed set (or the producer failed).
    Cancelled,
    /// The residual-share rule abandoned the beam render ([`BeamOff::ResidualTooLarge`]).
    Declined,
}

/// A piece of a batch: tiles and their leaves, small enough for the memory cap.
struct Piece<'l> {
    /// The piece's tiles, in tile order.
    tiles: Vec<TileRect>,
    /// `(tile index, leaves)` per tile; borrowed from the preloaded cache when `loaded`.
    leaves: Vec<(u32, Cow<'l, [Leaf]>)>,
    /// The leaves came from the preloaded cache, which keeps them already.
    loaded: bool,
    /// The piece is a residual-only tile, which the leaf cache must not keep.
    residual_only: bool,
}

/// Cuts a traced batch into pieces whose estimates (`costs`, one per tile) fit `share`; returns
/// them and the number of halvings. A batch the producer cut already is one piece; a batch taken
/// from the preloaded cache may be cut here. The last argument holds the batch's `share`, `loaded`
/// and `residual_only`.
fn cut_pieces<'l>(
    tiles: Vec<TileRect>,
    leaves: Vec<(u32, Cow<'l, [Leaf]>)>,
    costs: &[u64],
    (share, loaded, residual_only): (u64, bool, bool),
) -> (Vec<Piece<'l>>, u32) {
    let (ranges, halvings) = split_for_cap(costs, share);
    let mut tiles = tiles.into_iter();
    let mut leaves = leaves.into_iter();
    let pieces = ranges
        .into_iter()
        .map(|range| Piece {
            tiles: tiles.by_ref().take(range.len()).collect(),
            leaves: leaves.by_ref().take(range.len()).collect(),
            loaded,
            residual_only,
        })
        .collect();
    (pieces, halvings)
}

/// What the engine side reads, fixed for the render.
struct Ctx<'c, 'f, 'a, 'e> {
    /// The beam frame (stone, white balance).
    frame: &'c BeamFrame<'f, 'a>,
    /// The engines of the render, in priority order.
    engines: &'c [Box<dyn BeamEngine + 'e>],
    /// The pipeline's own CPU engine: the last resort.
    fallback: &'c dyn BeamEngine,
    /// The beam settings.
    settings: &'c BeamSettings,
    /// The frame size in pixels.
    size: (u32, u32),
    /// The adaptive tile (and beam tile) edge.
    tile: u32,
    /// The absolute index of the first residual sample.
    sample_offset: u32,
    /// The residual's sample cap per pixel (`AdaptiveSettings::max_spp`).
    max_spp: u32,
    /// The residual's minimum samples per pixel (`BeamSettings::residual_min_spp`).
    residual_min_spp: u32,
}

/// The hooks of the run.
struct Hooks<'h> {
    /// Each finished piece's tiles and final pixels.
    on_batch: Option<BatchHook<'h>>,
    /// Each finished piece's leaves and kept index, instead of freeing them.
    retain: Option<&'h mut dyn FnMut(RetainedBatch)>,
}

/// What the engine side changes.
struct State<'h> {
    /// The image so far.
    accum: Accum,
    /// Per engine: failed for the rest of the render.
    failed: Vec<bool>,
    /// The hooks.
    hooks: Hooks<'h>,
    /// Pixels of the finished tiles that hold kept regions (the decline rule's denominator).
    stone_pixels: u64,
    /// Residual samples those pixels received.
    stone_residual_samples: u64,
    /// Finished batches that held kept regions.
    stone_batches: usize,
}

/// Runs both stages of `batch` on `engine`; `partial` takes the share of the residual done so far.
fn attempt(
    ctx: &Ctx<'_, '_, '_, '_>,
    engine: &dyn BeamEngine,
    batch: &Batch<'_>,
    (cancel, partial): (&AtomicBool, &mut dyn FnMut(f32)),
) -> Attempt {
    let pixels = batch.pixel_count();
    let fragments = match engine.fragments(batch, cancel) {
        EngineOutcome::Done(done) => done,
        EngineOutcome::Declined(why) => return Attempt::Declined(why),
        EngineOutcome::Cancelled => return Attempt::Cancelled,
    };
    if fragments.xyz.len() != pixels {
        return Attempt::Declined("the engine returned a wrong number of fragment pixels");
    }
    let shaded = engine.fragment_stats();
    let offset: Vec<Vec3> = fragments
        .xyz
        .iter()
        .map(|&xyz| ctx.frame.balanced(finite_or_zero(xyz)))
        .collect();
    let residual = if ctx.settings.residual == ResidualMode::PathSamples {
        match engine.residual_with_progress(batch, &offset, cancel, partial) {
            EngineOutcome::Done(done) => done,
            EngineOutcome::Declined(why) => return Attempt::Declined(why),
            EngineOutcome::Cancelled => return Attempt::Cancelled,
        }
    } else {
        EngineRun::empty(pixels).residual
    };
    if residual.sum.len() != pixels
        || residual.half_b.len() != pixels
        || residual.counts.len() != pixels
    {
        return Attempt::Declined("the engine returned a wrong number of residual pixels");
    }
    Attempt::Done(Box::new(EngineRun {
        offset,
        residual,
        fragments: shaded,
        engine: engine.name().to_owned(),
    }))
}

/// Runs a piece on the engine `assign` names for batch `index`, failing over on a decline.
///
/// The GPU layouts are built lazily, once, for the first engine that wants them.
fn run_on_engines(
    ctx: &Ctx<'_, '_, '_, '_>,
    failed: &mut [bool],
    (index, piece, kept): (u32, &Piece<'_>, &[(TileRect, KeptIndex)]),
    stats: &mut BeamStats,
    (cancel, partial): (&AtomicBool, &mut dyn FnMut(f32)),
) -> Result<EngineRun, Stop> {
    let empty_leaves = PackedLeaves::default();
    let empty_kept = PackedKept::default();
    let mut packed: Option<(PackedLeaves, PackedKept)> = None;
    // Engines this batch cannot give its packed form to, or that declined it for a size cap (not
    // failed for the render).
    let mut skipped = vec![false; failed.len()];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let usable: Vec<bool> = failed
            .iter()
            .zip(&skipped)
            .map(|(&down, &skip)| down || skip)
            .collect();
        let slot = assign(index, &usable);
        let engine: &dyn BeamEngine = slot
            .and_then(|e| ctx.engines.get(e))
            .map_or(ctx.fallback, |boxed| &**boxed);
        if engine.needs_packed() && packed.is_none() {
            // The packed kept table spans the bounding rectangle of the batch's tiles: decline it
            // before it is allocated when it passes the memory cap.
            let Some(kept_form) = try_pack_kept(kept, ctx.settings.memory_cap_bytes) else {
                tracing::debug!(
                    target: LOG_TARGET,
                    batch = index,
                    engine = engine.name(),
                    bytes = kept_footprint(kept).bytes(),
                    cap_bytes = ctx.settings.memory_cap_bytes,
                    reason = PACKED_KEPT_OVER_CAP,
                    "packed kept index over the memory cap, the batch goes to the next engine"
                );
                let Some(mark) = slot.and_then(|e| skipped.get_mut(e)) else {
                    let pixels = piece.tiles.iter().map(|rect| rect.pixel_count()).sum();
                    return Ok(EngineRun::empty(pixels));
                };
                *mark = true;
                continue;
            };
            packed = Some((pack_leaves_counted(&piece.leaves, stats), kept_form));
        }
        let (packed_leaves, packed_kept) = packed
            .as_ref()
            .map_or((&empty_leaves, &empty_kept), |(l, k)| (l, k));
        let batch = Batch {
            index,
            tiles: &piece.tiles,
            leaves: &piece.leaves,
            kept,
            packed_leaves,
            packed_kept,
        };
        match attempt(ctx, engine, &batch, (cancel, &mut *partial)) {
            Attempt::Done(run) => return Ok(*run),
            Attempt::Cancelled => return Err(Stop::Cancelled),
            Attempt::Declined(reason) => {
                let per_batch = engine.decline_is_per_batch(reason);
                tracing::debug!(
                    target: LOG_TARGET,
                    batch = index,
                    engine = engine.name(),
                    reason,
                    per_batch,
                    "beam engine declined, the batch goes to the next engine"
                );
                // A size cap is this batch's property: only this batch skips the engine.
                let marks: &mut [bool] = if per_batch {
                    &mut skipped
                } else {
                    &mut *failed
                };
                match slot.and_then(|e| marks.get_mut(e)) {
                    Some(mark) => *mark = true,
                    None => return Ok(EngineRun::empty(batch.pixel_count())),
                }
            }
        }
    }
}

/// The kept index of every tile of `piece`, built one tile at a time so that `cancel` is polled
/// between tiles; it equals `batch_kept_indices` of the whole piece. `None` when cancelled.
fn kept_polled(
    piece: &Piece<'_>,
    (frame_width, tile_size): (u32, u32),
    cancel: &AtomicBool,
) -> Option<Vec<(TileRect, KeptIndex)>> {
    let mut kept = Vec::with_capacity(piece.tiles.len());
    for &rect in &piece.tiles {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        kept.push((
            rect,
            tile_kept_index(rect, &piece.leaves, (frame_width, tile_size)),
        ));
    }
    Some(kept)
}

/// Adds the pixels and residual samples of the tiles of `piece` that hold kept regions to
/// `state`'s decline-rule totals; `true` when the piece holds at least one such tile.
///
/// `counts` is the residual's samples per pixel in batch order (the tiles in order, each
/// row-major).
fn tally_stone(
    state: &mut State<'_>,
    piece: &Piece<'_>,
    kept: &[(TileRect, KeptIndex)],
    counts: &[u32],
) -> bool {
    let mut has_stone = false;
    let mut at = 0_usize;
    for (rect, (_, index)) in piece.tiles.iter().zip(kept) {
        let pixels = rect.pixel_count();
        if !index.is_empty() {
            has_stone = true;
            let samples: u64 = counts
                .get(at..at + pixels)
                .unwrap_or_default()
                .iter()
                .map(|&count| u64::from(count))
                .sum();
            state.stone_pixels += u64::try_from(pixels).unwrap_or(u64::MAX);
            state.stone_residual_samples += samples;
        }
        at += pixels;
    }
    has_stone
}

/// Finishes one piece: kept index, engines, image, hooks, log line. `true` when the piece holds
/// kept regions (the decline rule counts such batches). `partial` takes the share of the piece's
/// residual done so far.
fn finish_piece(
    ctx: &Ctx<'_, '_, '_, '_>,
    state: &mut State<'_>,
    (index, caps_hit): (u32, u32),
    piece: Piece<'_>,
    (cancel, partial): (&AtomicBool, &mut dyn FnMut(f32)),
) -> Result<bool, Stop> {
    let started = Instant::now();
    let kept = kept_polled(&piece, (ctx.size.0, ctx.tile), cancel).ok_or(Stop::Cancelled)?;
    let run = run_on_engines(
        ctx,
        &mut state.failed,
        (index, &piece, &kept),
        &mut state.accum.stats,
        (cancel, partial),
    )?;
    let ids: Vec<u32> = piece.leaves.iter().map(|(id, _)| *id).collect();
    let means = state
        .accum
        .store((&piece.tiles, &ids), &run, ctx.sample_offset);
    state.accum.stats.merge(&run.fragments);
    let has_stone = tally_stone(state, &piece, &kept, &run.residual.counts);
    let pixels = piece.tiles.iter().map(|rect| rect.pixel_count()).sum();
    tracing::debug!(
        target: LOG_TARGET,
        batch = index,
        tiles = piece.tiles.len(),
        leaves = piece.leaves.iter().map(|(_, leaves)| leaves.len()).sum::<usize>(),
        fragments = run.fragments.fragments,
        caps_hit,
        engine = run.engine.as_str(),
        residual_samples = run.residual.samples_traced,
        residual_share = f64::from(residual_share(run.residual.samples_traced, pixels, ctx.max_spp)),
        seconds = started.elapsed().as_secs_f64(),
        "beam batch finished"
    );
    if let Some(hook) = state.hooks.on_batch.as_deref_mut() {
        hook(&piece.tiles, &means);
    }
    if !piece.loaded
        && !piece.residual_only
        && let Some(hook) = state.hooks.retain.as_deref_mut()
    {
        hook(RetainedBatch {
            tiles: piece.tiles,
            // A retained piece was traced here, so its leaves are owned: this moves them.
            leaves: piece
                .leaves
                .into_iter()
                .map(|(tile, leaves)| (tile, leaves.into_owned()))
                .collect(),
            kept,
        });
    }
    Ok(has_stone)
}

/// Finishes a traced batch: cuts it for the memory cap and finishes its pieces in order.
///
/// After the pieces, the decline rule reads the residual share of the stone tiles so far
/// ([`BEAM_RESIDUAL_SHARE_LIMIT`]) and ends the render with [`Stop::Declined`] when it is over.
fn finish_batch(
    ctx: &Ctx<'_, '_, '_, '_>,
    state: &mut State<'_>,
    traced: TracedBatch<'_>,
    (cancel, partial): (&AtomicBool, &mut dyn FnMut(f32)),
) -> Result<(), Stop> {
    let TracedBatch {
        index,
        tiles,
        leaves,
        costs,
        stats,
        loaded,
        residual_only,
        ..
    } = traced;
    state.accum.stats.merge(&stats);
    let share = batch_share(ctx.settings.memory_cap_bytes);
    let (pieces, caps_hit) = cut_pieces(tiles, leaves, &costs, (share, loaded, residual_only));
    if caps_hit > 0 {
        tracing::debug!(
            target: LOG_TARGET,
            batch = index,
            halvings = caps_hit,
            pieces = pieces.len(),
            cap_bytes = ctx.settings.memory_cap_bytes,
            "beam batch cut to fit the memory cap"
        );
    }
    let count = pieces.len();
    let mut has_stone = false;
    for (position, piece) in pieces.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        // The pieces of a batch share its slice of the progress bar.
        let mut piece_partial = |share: f32| {
            partial(progress_f32(
                (ratio_count(position) + f64::from(share)) / ratio_count(count),
            ));
        };
        has_stone |= finish_piece(
            ctx,
            state,
            (index, caps_hit),
            piece,
            (cancel, &mut piece_partial),
        )?;
    }
    if has_stone {
        state.stone_batches += 1;
    }
    decline_rule(ctx, state)
}

/// The decline rule: `Err(Stop::Declined)` when the stone tiles finished so far used more than
/// [`BEAM_RESIDUAL_SHARE_LIMIT`] of the plain render's samples.
///
/// Armed only for a sampled residual and a cap that leaves room to save
/// ([`BEAM_DECLINE_MIN_CAP_RATIO`]), and only after [`BEAM_DECLINE_AFTER_STONE_BATCHES`] batches
/// that held kept regions. A pure function of the sample counts, so it is deterministic.
fn decline_rule(ctx: &Ctx<'_, '_, '_, '_>, state: &State<'_>) -> Result<(), Stop> {
    let armed = ctx.settings.residual == ResidualMode::PathSamples
        && ctx.max_spp >= BEAM_DECLINE_MIN_CAP_RATIO.saturating_mul(ctx.residual_min_spp)
        && state.stone_batches >= BEAM_DECLINE_AFTER_STONE_BATCHES
        && state.stone_pixels > 0;
    if !armed {
        return Ok(());
    }
    let budget = ratio_count_u64(state.stone_pixels) * f64::from(ctx.max_spp);
    let share = ratio_count_u64(state.stone_residual_samples) / budget;
    if share > BEAM_RESIDUAL_SHARE_LIMIT {
        tracing::info!(
            target: LOG_TARGET,
            share,
            limit = BEAM_RESIDUAL_SHARE_LIMIT,
            stone_pixels = state.stone_pixels,
            "beam render abandoned: the residual of the first stone batches needs more than the \
             limit of the plain render's samples, the picture renders without beams"
        );
        return Err(Stop::Declined);
    }
    Ok(())
}

/// The receiving side: takes the traced batches in order and finishes them. Returns why it stopped
/// before the render was done, `None` when every batch is finished.
///
/// Whenever it stops early it sets `halt`, the token the producer polls (the user's `cancel` token
/// is read here and never set).
fn consume(
    ctx: &Ctx<'_, '_, '_, '_>,
    receiver: &mpsc::Receiver<TracedBatch<'_>>,
    (state, tracker): (&mut State<'_>, &mut Tracker<'_>),
    (batches, traced_tiles): (usize, &AtomicUsize),
    (cancel, halt, turns): (&AtomicBool, &AtomicBool, Option<&Sender<()>>),
) -> Option<Stop> {
    let stopped = |stop: Stop| {
        halt.store(true, Ordering::Relaxed);
        Some(stop)
    };
    let mut finished = 0_usize;
    let mut reported = usize::MAX;
    while finished < batches {
        if cancel.load(Ordering::Relaxed) {
            return stopped(Stop::Cancelled);
        }
        let traced = match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(traced) => traced,
            Err(RecvTimeoutError::Timeout) => {
                let now = traced_tiles.load(Ordering::Relaxed);
                if now != reported {
                    reported = now;
                    tracker.report(now, finished);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                // The producer ends without the token only when it failed; the caller must not
                // take that for a cancel.
                if !cancel.load(Ordering::Relaxed) {
                    tracing::warn!(
                        target: LOG_TARGET,
                        finished,
                        batches,
                        "the beam producer stopped before the last batch"
                    );
                }
                return stopped(Stop::Cancelled);
            }
        };
        let started = Instant::now();
        let (tree_seconds, from_cache, last) = (traced.seconds, traced.loaded, traced.last);
        tracker.start_part();
        tracker.report(traced_tiles.load(Ordering::Relaxed), finished);
        let outcome = {
            let mut on_partial = |share: f32| {
                tracker.report_partial(traced_tiles.load(Ordering::Relaxed), finished, share);
            };
            finish_batch(ctx, state, traced, (cancel, &mut on_partial))
        };
        if let Err(stop) = outcome {
            return stopped(stop);
        }
        // Tell a producer that waits for its turn (a quota of one) that the batch is done; a
        // producer that is gone needs no signal.
        if let Some(turns) = turns {
            let _ = turns.send(());
        }
        // A chunk comes in several parts when the memory estimate cut it; it is finished with
        // its last part.
        if last {
            finished += 1;
        }
        // The tree's share is measured once, on the first batch that really traced its trees.
        if tracker.weight.is_none() && !from_cache {
            tracker.fix_weight(tree_seconds, started.elapsed().as_secs_f64());
        }
        reported = usize::MAX;
        tracker.report(traced_tiles.load(Ordering::Relaxed), finished);
    }
    None
}

/// Everything a hybrid render takes (the arguments of `cpu_frame::render_beam_hybrid_surface`).
pub struct HybridInput<'a, 'e> {
    /// The frame.
    pub scene: &'a FrameScene<'a>,
    /// Tools, fluorescence and the surface scene, as the adaptive entries take them.
    pub extras: (&'a [ToolPrimitive], &'a Fluorescence, SurfaceScene<'a>),
    /// The intersection arena of `scene.planes`.
    pub plane_soa: &'a PlanesSoA32,
    /// The beam settings.
    pub beam: &'a BeamSettings,
    /// The adaptive settings of the residual; their `tile` is the batch tile.
    pub adaptive: &'a AdaptiveSettings,
    /// The engines, in priority order; empty means the pipeline's own CPU engine.
    pub engines: &'a EngineSet<'e>,
    /// The absolute index of the first residual sample.
    pub sample_offset: u32,
    /// The display multiplier of the residual's noise check.
    pub scale: f32,
}

/// The beam hybrid render of `input` on all available cores; see [`render_hybrid_with_threads`].
///
/// # Errors
///
/// The [`BeamOff`] reason the frame is not beam traced.
pub fn render_hybrid(
    input: &HybridInput<'_, '_>,
    cancel: &AtomicBool,
    run: BeamRun<'_>,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    render_hybrid_with_threads(input, default_threads(), cancel, run)
}

/// Checks that the stone and the extras are beam traced; the frame's own checks follow in
/// `BeamFrame::new`.
fn check_extras(input: &HybridInput<'_, '_>) -> Result<(), BeamOff> {
    let (tools, fluorescence, surface) = input.extras;
    if !tools.is_empty() {
        return Err(BeamOff::Concave);
    }
    check_eligibility(&StoneTraits {
        material: input.scene.material,
        facet_finishes: input.scene.facet_finishes,
        tools,
        fluorescence,
        surface,
        depth_of_field: false,
    })
    .map_err(BeamOff::NotEligible)
}

/// The beam hybrid render of `input` within a quota of `threads` (beam guide §12.4, §14.5).
///
/// The beam trees of the batches are traced on a producer thread while this thread shades and
/// finishes the previous batch on the engines; the quota is shared by the two sides
/// ([`producer_threads`], [`consumer_threads`]). Returns the image in the contract of
/// `render_adaptive_surface`: `pixels` are the final pixels (the white-balanced beam part plus the
/// residual mean, clamped at zero once), `counts` the residual samples per pixel, `sum` and
/// `half_b` the residual sums with the beam part added once per sample (so `sum / counts` is the
/// unclamped pixel and the half-split noise estimate is the residual's), `samples_traced` the
/// residual samples. Pixels of tiles that were not rendered stay zero with zero counts. The
/// counters are merged in tile order and include the fragments the engines shaded.
///
/// A cancelled render returns what was finished up to then; the caller checks the token.
///
/// # Errors
///
/// [`BeamOff::Concave`] for a stone with tools, [`BeamOff::NotEligible`] for an extra the path
/// cache refuses, the errors of `BeamFrame::new`, [`BeamOff::Unavailable`] when `adaptive` is
/// not valid (the caller then renders without beams, which reports the settings error itself), and
/// [`BeamOff::ResidualTooLarge`] when the decline rule abandons the render after the first stone
/// batch, and [`BeamOff::TreeTooLarge`] when the tree rule abandons it in its probe stage, before
/// any batch (the image so far is discarded; the caller renders without beams).
pub fn render_hybrid_with_threads(
    input: &HybridInput<'_, '_>,
    threads: usize,
    cancel: &AtomicBool,
    run: BeamRun<'_>,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    let (accum, _) = render_accum(input, threads, cancel, run, false)?;
    Ok((
        AdaptiveImage {
            width: input.scene.width,
            height: input.scene.height,
            sum: accum.sum,
            half_b: accum.half_b,
            counts: accum.counts,
            pixels: accum.pixels,
            samples_traced: accum.samples_traced,
        },
        accum.stats,
    ))
}

/// The image of a tile-ordered render, holding only the rendered tiles.
///
/// The buffers hold the pixels of the tiles `tiles` lists (their frame tile indices, ascending),
/// in that order, row-major inside a tile. The fields are those of [`AdaptiveImage`] with that
/// layout.
#[derive(Clone, Debug, PartialEq)]
pub struct TiledImage {
    /// The frame tile indices of the rendered tiles, ascending.
    pub tiles: Vec<u32>,
    /// See [`AdaptiveImage::sum`].
    pub sum: Vec<Vec3>,
    /// See [`AdaptiveImage::half_b`].
    pub half_b: Vec<Vec3>,
    /// Residual samples per pixel.
    pub counts: Vec<u32>,
    /// The final pixels.
    pub pixels: Vec<Vec3>,
    /// Residual samples traced.
    pub samples_traced: u64,
}

/// The beam hybrid render of `input` into tile-ordered buffers on all available cores; see
/// [`render_hybrid_tiles_with_threads`].
///
/// # Errors
///
/// The [`BeamOff`] reason the frame is not beam traced.
pub fn render_hybrid_tiles(
    input: &HybridInput<'_, '_>,
    cancel: &AtomicBool,
    run: BeamRun<'_>,
) -> Result<(TiledImage, BeamStats), BeamOff> {
    render_hybrid_tiles_with_threads(input, default_threads(), cancel, run)
}

/// The beam hybrid render of `input`, accumulating only the tiles of `run.tiles` in request order.
///
/// As [`render_hybrid_with_threads`] (all tiles of the frame without a restriction), but a remote
/// request for one tile holds the memory of one tile and not of the frame.
///
/// Every pixel is what the frame-sized render holds for it.
///
/// # Errors
///
/// The errors of [`render_hybrid_with_threads`].
pub fn render_hybrid_tiles_with_threads(
    input: &HybridInput<'_, '_>,
    threads: usize,
    cancel: &AtomicBool,
    run: BeamRun<'_>,
) -> Result<(TiledImage, BeamStats), BeamOff> {
    let (accum, tiles) = render_accum(input, threads, cancel, run, true)?;
    Ok((
        TiledImage {
            tiles,
            sum: accum.sum,
            half_b: accum.half_b,
            counts: accum.counts,
            pixels: accum.pixels,
            samples_traced: accum.samples_traced,
        },
        accum.stats,
    ))
}

/// The prepared material a hybrid render of `input` uses everywhere.
///
/// When the engine set carries a preparation built from this frame's very material
/// ([`EngineSet::with_prepared`]) it is reused, so its optics tables are shared by the frame, the
/// fallback engine and the engines; otherwise the material is prepared here, once. Either way the
/// prepared values equal those of a separate preparation, so the image does not change.
#[must_use]
pub fn shared_preparation<'a>(input: &HybridInput<'a, '_>) -> PreparedMaterial<'a> {
    input
        .engines
        .prepared_for(input.scene.material)
        .unwrap_or_else(|| PreparedMaterial::new(input.scene.material))
}

/// The render behind the entries: the accumulated image and the frame tile indices rendered.
///
/// The image is frame-sized, or tile-ordered when `tile_ordered`.
fn render_accum(
    input: &HybridInput<'_, '_>,
    threads: usize,
    cancel: &AtomicBool,
    run: BeamRun<'_>,
    tile_ordered: bool,
) -> Result<(Accum, Vec<u32>), BeamOff> {
    check_extras(input)?;
    let scene = input.scene;
    // One preparation of the material (and its optics tables) serves the frame's reflection kit,
    // the fallback engine and the residual: the engines' own when they carry it, else built here
    // once.
    let prepared = shared_preparation(input);
    let frame = BeamFrame::with_prepared(scene, input.beam, prepared.clone())?;
    if input.adaptive.validate().is_err() {
        return Err(BeamOff::Unavailable);
    }
    // The tree rule and the tile budget are armed like the residual rule, and only for a render
    // that neither keeps its leaves nor loads kept ones: a kept tree is paid once and used again,
    // and a stored tree must not depend on this render's sample count.
    let budgeted = input.beam.residual == ResidualMode::PathSamples
        && input.adaptive.max_spp
            >= BEAM_DECLINE_MIN_CAP_RATIO.saturating_mul(input.beam.residual_min_spp)
        && !input.beam.keep_leaves
        && run.retain.is_none()
        && run.preloaded.is_none();
    let frame = if budgeted {
        frame.with_tile_allowance(tile_budget(input.adaptive.tile, input.adaptive.max_spp))
    } else {
        frame
    };
    let size = (scene.width, scene.height);
    let tile = input.adaptive.tile;
    let residual = ResidualScene {
        frame: scene,
        extras: input.extras,
        plane_soa: input.plane_soa,
        prepared: &prepared,
        n_lambda: frame.n_lambda(),
        sample_offset: input.sample_offset,
        residual_min_spp: input.beam.residual_min_spp,
    };
    let fallback = CpuBeamEngine::new(&residual, input.beam, input.adaptive)
        .with_threads(consumer_threads(threads))
        .with_display_scale(input.scale);
    let ctx = Ctx {
        frame: &frame,
        engines: input.engines.engines.as_slice(),
        fallback: &fallback,
        settings: input.beam,
        size,
        tile,
        sample_offset: input.sample_offset,
        max_spp: input.adaptive.max_spp.max(1),
        residual_min_spp: input.beam.residual_min_spp,
    };
    let tiles = tile_list(size, tile, run.tiles);
    // Leaves traced for a coarser lighting than this render's are refined before they load, as
    // `LeafCache::relight` does. On a cancel the refinements are empty and the result is unused.
    let refined = run
        .preloaded
        .and_then(|cache| cache.refinements(scene, input.beam, (&frame, tile), cancel))
        .unwrap_or_default();
    let accum = if tile_ordered {
        Accum::tiled(size, tile, &tiles)
    } else {
        Accum::new(size)
    };
    let serial = runs_in_turns(threads);
    // An empty set runs every batch on the pipeline's own CPU engine, which shares the quota.
    let cpu_engines = input.engines.engines.is_empty()
        || input
            .engines
            .engines
            .iter()
            .any(|engine| engine.shares_cpu_threads());
    if budgeted {
        // The tree rule decides before the producer thread and the engines start, on the whole
        // quota: a declined render has traced the probe tiles and nothing else. Without an engine
        // on this computer's CPU the plain render is fast, so the probe is also time-bounded.
        let deadline = (!cpu_engines).then(|| probe_time_bound(frame.tile_allowance(), threads));
        match probe_trees(&frame, tile, &tiles, threads, deadline, cancel) {
            ProbeOutcome::Go => {}
            ProbeOutcome::Declined | ProbeOutcome::TimedOut => {
                return Err(BeamOff::TreeTooLarge);
            }
            // A cancelled render returns what it finished: nothing yet.
            ProbeOutcome::Cancelled => return Ok((accum, tiles)),
        }
    }
    let threads = if cpu_engines {
        producer_threads(threads)
    } else {
        gpu_producer_threads(threads)
    };
    let (accum, stop) = run_pipeline(
        &ctx,
        &tiles,
        (threads, serial, budgeted),
        (run, refined, accum),
        cancel,
    );
    if stop == Some(Stop::Declined) {
        return Err(BeamOff::ResidualTooLarge);
    }
    Ok((accum, tiles))
}

/// Runs the pipeline over `tiles` into `accum` with `threads` tree workers. Returns the image and
/// why it stopped before the last batch (`None` when it did not).
fn run_pipeline(
    ctx: &Ctx<'_, '_, '_, '_>,
    tiles: &[u32],
    (threads, serial, budgeted): (usize, bool, bool),
    (run, refined, accum): (BeamRun<'_>, Refinements, Accum),
    cancel: &AtomicBool,
) -> (Accum, Option<Stop>) {
    let BeamRun {
        progress,
        on_batch,
        retain,
        preloaded,
        ..
    } = run;
    let loaded = preloaded.map(|cache| Loaded::new(cache, refined, ctx.size, ctx.tile));
    let chunk = ctx.settings.batch_tiles.max(1) as usize;
    let chunks: Vec<&[u32]> = tiles.chunks(chunk).collect();
    tracing::debug!(
        target: LOG_TARGET,
        producer_threads = threads,
        serial,
        engines = ?ctx.engines.iter().map(|engine| engine.name()).collect::<Vec<_>>(),
        batch_tiles = chunk,
        tile_allowance = ctx.frame.tile_allowance(),
        budgeted,
        "beam pipeline started"
    );
    let traced_tiles = AtomicUsize::new(0);
    let mut tracker = Tracker::new(progress, chunks.len(), tiles.len());
    let mut state = State {
        accum,
        failed: vec![false; ctx.engines.len()],
        hooks: Hooks { on_batch, retain },
        stone_pixels: 0,
        stone_residual_samples: 0,
        stone_batches: 0,
    };
    let (frame, tile, share) = (
        ctx.frame,
        ctx.tile,
        batch_share(ctx.settings.memory_cap_bytes),
    );
    // The producer's own stop flag: the consumer sets it when it stops early, for the user's token
    // or for the decline rule, so the producer ends without the user's token being touched.
    let halt = AtomicBool::new(false);
    let halt_ref = &halt;
    let stopped = std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel::<TracedBatch<'_>>(0);
        // A quota of one cannot run the trees and the shading side by side: the producer then
        // waits for the consumer's signal after every batch.
        let (turn_tx, turn_rx) = mpsc::channel::<()>();
        let (turns_out, turns_in) = if serial {
            (Some(turn_rx), Some(&turn_tx))
        } else {
            (None, None)
        };
        let list = &chunks;
        let counter = &traced_tiles;
        let lookup = loaded.as_ref();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        scope.spawn(move || {
            // The producer logs on behalf of the render: it reports to the subscriber the calling
            // thread has set (a thread-scoped one does not follow a spawned thread by itself).
            let _logging = tracing::dispatcher::set_default(&dispatch);
            let producer = Producer {
                frame,
                tile,
                threads,
                share,
                sender: &sender,
                turns: turns_out.as_ref(),
                loaded: lookup,
                traced_tiles: counter,
                cancel: halt_ref,
            };
            produce(&producer, list);
        });
        let stop = consume(
            ctx,
            &receiver,
            (&mut state, &mut tracker),
            (chunks.len(), &traced_tiles),
            (cancel, halt_ref, turns_in),
        );
        (stop, Instant::now())
    });
    let (stop, consumer_stopped) = stopped;
    match stop {
        Some(Stop::Cancelled) => log_cancel(cancel, consumer_stopped),
        Some(Stop::Declined) => {}
        None => tracker.finish(),
    }
    (state.accum, stop)
}

/// Logs how a render that stopped with [`Stop::Cancelled`] ended: a failure when the user's token
/// is not set, else the seconds since the consuming side stopped.
fn log_cancel(cancel: &AtomicBool, consumer_stopped: Instant) {
    if cancel.load(Ordering::Relaxed) {
        // Where the time between the token and the return goes: the producer is joined after
        // the consuming side stopped, so a long join is a tree loop that polls too rarely.
        tracing::debug!(
            target: LOG_TARGET,
            "beam render stopped by the cancel token, producer joined after {:.3} s",
            consumer_stopped.elapsed().as_secs_f64()
        );
    } else {
        // A failure, not a cancel: the image is partial and the progress never reaches 1.
        tracing::error!(
            target: LOG_TARGET,
            "the beam render ended before its last batch without the cancel token; the image is incomplete"
        );
    }
}
