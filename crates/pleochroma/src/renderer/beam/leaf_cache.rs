//! The leaf cache (beam guide §14.10).
//!
//! It retains a beam render's leaves and kept regions up to a byte cap, for instant re-light
//! (lighting, exposure, glare, head shadow) and, persisted, for re-renders of the same pose.
//!
//! # Re-light
//!
//! A leaf holds only lighting-independent data (§11.1), so a lighting, exposure, glare or
//! head-shadow change needs no new beam tree. [`LeafCache::relight`] first refines the leaves
//! whose recorded `τ_ω` is coarser than the new lighting needs (`retrace::refine_leaf`, which
//! re-evaluates their sample points through the stored `BranchPrefix`), then runs the engines'
//! fragment stage and a short residual (one pass of `residual_min_spp` samples) on the retained
//! kept index, and finally
//! the white balance and the end clamp. The tiles of batches that were not retained (over the cap,
//! or evicted by a refinement) are rendered fresh through the batch pipeline. The result is the
//! image a fresh beam render under the new lighting gives with the same short residual, when the
//! cache was built under the same or a coarser `τ_ω` (see [`LeafCache::relight`] for the other
//! direction).
//! [`LeafCache::relight_ray_traced`] is the Ray-Traced view's own, deterministic image.

use std::{
    borrow::Cow,
    sync::atomic::{AtomicBool, Ordering},
};

use glam::{DVec2, Vec3};

use super::{
    BeamProgress, BeamRun, BeamSettings, ResidualMode,
    beam::{Leaf, PackedKept, PackedLeaves},
    engines::{Batch, BatchResidual, BeamEngine, CpuBeamEngine, EngineOutcome, EngineSet, assign},
    frame::BeamFrame,
    leaf::{ReflectionKit, pack_leaves},
    lighting::tau_omega,
    pipeline::{HybridInput, combine_pixel, default_threads, render_hybrid_with_threads, run_jobs},
    primary::TileRect,
    residual::{KeptIndex, pack_kept},
    retrace::{RetraceKit, refine_leaf},
    stats::BeamStats,
    tile_residual::ResidualScene,
};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{build_plane_soa, surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::{
        adaptive::{AdaptiveSettings, DEFAULT_TILE, LiveTiles, MAX_TILE, MIN_TILE, NoiseMetric},
        frame_scene::FrameScene,
    },
};

/// The format version of persisted beam caches; part of their `CacheKey` (§14.10).
pub const BEAM_CACHE_FORMAT_VERSION: u32 = 1;

/// The noise target of the short residual: never reached, so the pass count alone decides.
const SHORT_RESIDUAL_TARGET_NOISE: f32 = 1.0e-6;

/// The log target of the beam renderer's debug lines.
const LOG_TARGET: &str = "pleochroma::beam";

/// Refined leaf groups of a cache, by `(batch, group)` position in ascending order: what
/// [`LeafCache::refinements`] computes without touching the cache.
#[derive(Clone, Debug, Default)]
pub struct Refinements {
    /// `((batch, group), refined leaves)`, ascending by position.
    groups: Vec<((usize, usize), Vec<Leaf>)>,
    /// The stale leaves that could not be retraced and were kept at their coarse tolerance.
    unrefined_leaves: usize,
}

impl Refinements {
    /// The number of stale leaves that could not be refined (kept unchanged, so the re-lit image
    /// can differ from a fresh render there); zero when every stale leaf was refined.
    #[must_use]
    pub const fn unrefined_leaves(&self) -> usize {
        self.unrefined_leaves
    }

    /// The refined leaves of the group at `position` (`(batch, group)`), if it was refined.
    #[must_use]
    pub fn get(&self, position: (usize, usize)) -> Option<&[Leaf]> {
        let at = self
            .groups
            .binary_search_by_key(&position, |(key, _)| *key)
            .ok()?;
        self.groups.get(at).map(|(_, leaves)| leaves.as_slice())
    }

    /// `true` when no group was refined.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// One retained batch: its tiles, leaves and kept index.
#[derive(Clone, Debug, Default)]
pub struct RetainedBatch {
    /// The batch's tiles, in tile order.
    pub tiles: Vec<TileRect>,
    /// The leaves per tile (`(tile index, leaves)`).
    pub leaves: Vec<(u32, Vec<Leaf>)>,
    /// The kept index per tile.
    pub kept: Vec<(TileRect, KeptIndex)>,
}

/// The retained leaves of one beam render.
#[derive(Clone, Debug, Default)]
pub struct LeafCache {
    /// The retention cap in bytes (`BeamSettings::leaf_cache_bytes`).
    cap_bytes: u64,
    /// The bytes retained so far.
    bytes: u64,
    /// The retained batches, in batch order.
    batches: Vec<RetainedBatch>,
}

impl LeafCache {
    /// An empty cache with a retention cap of `cap_bytes`.
    #[must_use]
    pub const fn new(cap_bytes: u64) -> Self {
        Self {
            cap_bytes,
            bytes: 0,
            batches: Vec::new(),
        }
    }

    /// Keeps `batch` when it fits under the cap; `false` when it does not (a re-light then
    /// retraces that batch).
    ///
    /// The batch's size is [`retained_bytes`]: its tiles, its leaves with their vertex vectors and
    /// its kept regions with their per-pixel tables. A batch that does not fit is not retained,
    /// but a later, smaller one may be.
    pub fn retain(&mut self, batch: RetainedBatch) -> bool {
        let size = retained_bytes(&batch);
        if self.bytes.saturating_add(size) > self.cap_bytes {
            tracing::debug!(
                target: LOG_TARGET,
                batch_bytes = size,
                retained_bytes = self.bytes,
                cap_bytes = self.cap_bytes,
                tiles = batch.tiles.len(),
                "beam leaf cache cap reached, the batch is not retained"
            );
            return false;
        }
        self.bytes += size;
        self.batches.push(batch);
        true
    }

    /// Re-lights the retained leaves under `scene`'s (new) lighting (§14.10): refine leaves whose
    /// recorded `τ_ω` is too coarse, re-run the fragment stage, run a short residual at
    /// `residual_min_spp` on the kept index. Returns the frame's final pixels (white-balanced and
    /// clamped like a fresh beam render), or `None` when cancelled or nothing is retained.
    ///
    /// The equality with a fresh render below holds when the cache was built under the same or a
    /// coarser `τ_ω` than the new lighting needs (only leaves coarser than the target are
    /// refined). Re-lighting a cache built under a finer `τ_ω` (an HDR map) to a coarser lighting
    /// (Studio) keeps the finer leaves, which a fresh render of the new lighting would not build:
    /// the image is unbiased and equal to the fresh one in expectation, but not bit for bit
    /// (summation order and the pieces dropped at `SPLIT_FLOOR_PX2` differ). A group that is
    /// refined also records the new `τ_ω` on its reflection leaves.
    ///
    /// The residual is one pass of `residual_min_spp` samples per pixel (sample indices from 0),
    /// so the result equals a fresh beam render under the new lighting whose adaptive settings
    /// are that single pass (`min_spp == max_spp == pass_spp == residual_min_spp`), bit for bit
    /// on the same engine. The tiles of batches that are not retained are rendered fresh through
    /// the pipeline on `engines` and retained now when they fit; refined leaves replace the old
    /// ones in the cache, and a refinement that grows the cache past its cap evicts the last
    /// batches.
    /// `scene` and `settings` must be those of the render the leaves came from, with the new
    /// lighting.
    #[must_use]
    pub fn relight(
        &mut self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        engines: &EngineSet<'_>,
        cancel: &AtomicBool,
    ) -> Option<Vec<Vec3>> {
        let plan = self.prepare(scene, settings, cancel, false)?;
        let width = plan.size.0 as usize;
        let mut out = vec![Vec3::ZERO; width * plan.size.1 as usize];
        let want_residual = settings.residual == ResidualMode::PathSamples;
        let batches = &self.batches;
        with_engines(scene, settings, &plan, engines, |ctx| {
            let mut failed = vec![false; ctx.engines.len()];
            for (ordinal, batch) in batches.iter().enumerate() {
                let shaded = shade_batch(
                    ctx,
                    &plan.frame,
                    (batch, to_u32(ordinal)),
                    (&mut failed, want_residual),
                    cancel,
                )?;
                scatter(&mut out, width, &batch.tiles, &shaded.pixels());
            }
            Some(())
        })?;
        // Fresh batches are admitted as they arrive. The pixels come from the requested tile set,
        // so a residual-only tile is not lost.
        let image = render_missing(
            scene,
            settings,
            engines,
            &plan,
            (cancel, want_residual),
            &mut |batch| {
                self.retain(batch);
            },
        )?;
        scatter_from(&mut out, &image, width, &missing_rects(&plan));
        (!cancel.load(Ordering::Relaxed)).then_some(out)
    }

    /// The Ray-Traced view's re-light (§14.10): a defined image of its own, not a path-traced one.
    ///
    /// Per pixel it is the beam part under `scene`'s lighting (the kept leaves: refined, shaded,
    /// white-balanced and clamped at zero) plus the Whitted engine's own shading of the pixel's
    /// uncovered share: `whitted_background` is the Whitted image of the frame (row-major,
    /// `width * height`), weighted by the share of the pixel's area no first-surface reflection
    /// leaf covers. The reflection leaves of a frame tile its silhouette, so inside the stone the
    /// share is zero and at the silhouette it is the part outside. The dropped share (pruned
    /// branches, slivers, depth caps, optic-axis regions) is left out, as the Whitted engine
    /// leaves out what falls below its own cutoff. There is no residual, so the image is
    /// deterministic and noise-free. Tiles of batches that are not retained are traced fresh
    /// without a residual.
    ///
    /// An empty cache is the first call of a pose: every tile is traced fresh (no residual) and
    /// retained, at the default adaptive tile edge, so that call is the build.
    ///
    /// `None` when cancelled, the frame is not beam traced, or `whitted_background` is not
    /// frame-sized.
    #[must_use]
    pub fn relight_ray_traced(
        &mut self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        whitted_background: &[Vec3],
        cancel: &AtomicBool,
    ) -> Option<Vec<Vec3>> {
        let width = scene.width as usize;
        let count = width * scene.height as usize;
        if whitted_background.len() != count {
            return None;
        }
        let plan = self.prepare(scene, settings, cancel, true)?;
        // No engine, but the plan's frame already prepared the material: the fresh tiles'
        // render reuses that preparation instead of building a second set of optics tables.
        let engines = EngineSet::with_prepared(Vec::new(), plan.frame.prepared());
        let mut beams = vec![Vec3::ZERO; count];
        let mut covered = vec![0.0_f64; count];
        let batches = &self.batches;
        with_engines(scene, settings, &plan, &engines, |ctx| {
            let mut failed = vec![false; ctx.engines.len()];
            for (ordinal, batch) in batches.iter().enumerate() {
                let shaded = shade_batch(
                    ctx,
                    &plan.frame,
                    (batch, to_u32(ordinal)),
                    (&mut failed, false),
                    cancel,
                )?;
                scatter(&mut beams, width, &batch.tiles, &shaded.pixels());
                add_coverage(&mut covered, width, batch);
            }
            Some(())
        })?;
        // The pixels come from the requested tile set (a residual-only tile has no retained
        // batch and falls back to the Whitted background); the coverage comes from the retained
        // reflection leaves only.
        let image = render_missing(
            scene,
            settings,
            &engines,
            &plan,
            (cancel, false),
            &mut |batch| {
                add_coverage(&mut covered, width, &batch);
                self.retain(batch);
            },
        )?;
        scatter_from(&mut beams, &image, width, &missing_rects(&plan));
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        Some(
            beams
                .iter()
                .zip(&covered)
                .zip(whitted_background)
                .map(|((&beam, &share), &whitted)| {
                    let uncovered = uncovered_share(share);
                    (beam + finite_or_zero(whitted) * uncovered).max(Vec3::ZERO)
                })
                .collect(),
        )
    }

    /// The bytes retained.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The retention cap in bytes.
    #[must_use]
    pub const fn cap_bytes(&self) -> u64 {
        self.cap_bytes
    }

    /// The number of retained batches.
    #[must_use]
    pub const fn batch_count(&self) -> usize {
        self.batches.len()
    }

    /// The retained batches, in batch order (what `BeamRun::preloaded` reads).
    #[must_use]
    pub fn retained_batches(&self) -> &[RetainedBatch] {
        &self.batches
    }

    /// The number of retained leaves.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.batches
            .iter()
            .flat_map(|batch| &batch.leaves)
            .map(|(_, leaves)| leaves.len())
            .sum()
    }

    /// The share of every pixel (row-major, `size.0 * size.1`, 0 to 1) the retained first-surface
    /// reflection leaves cover: 1 inside the stone's silhouette, 0 outside, a fraction at its
    /// edge. This is the weight [`LeafCache::relight_ray_traced`] leaves to the beams.
    #[must_use]
    pub fn coverage(&self, size: (u32, u32)) -> Vec<f64> {
        let width = size.0 as usize;
        let mut covered = vec![0.0_f64; width * size.1 as usize];
        for batch in &self.batches {
            add_coverage(&mut covered, width, batch);
        }
        for share in &mut covered {
            *share = share.clamp(0.0, 1.0);
        }
        covered
    }

    /// `true` when nothing is retained.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    /// The common first steps of both re-lights: the beam frame of `scene`, the tile edge of the
    /// retained batches, the refinement of their stale leaves and the tiles nothing is retained
    /// for. With `build_when_empty` an empty cache plans every tile at [`build_tile`]'s edge (the
    /// Ray-Traced view's first call is its build); without it an empty cache gives `None`. `None`
    /// also when cancelled or the frame is not beam traced.
    fn prepare<'s, 'a>(
        &mut self,
        scene: &'s FrameScene<'a>,
        settings: &BeamSettings,
        cancel: &AtomicBool,
        build_when_empty: bool,
    ) -> Option<Plan<'s, 'a>> {
        let empty = self.batches.is_empty();
        if (empty && !build_when_empty) || cancel.load(Ordering::Relaxed) {
            return None;
        }
        let frame = BeamFrame::new(scene, settings).ok()?;
        let size = (scene.width, scene.height);
        let tile = if empty {
            build_tile(size)
        } else {
            infer_tile(&self.batches, size)?
        };
        self.refine(scene, settings, (&frame, tile), cancel)?;
        let missing = missing_tiles(&self.batches, size, tile);
        Some(Plan {
            frame,
            size,
            tile,
            missing,
        })
    }

    /// Refines every retained leaf whose recorded `τ_ω` is coarser than `scene`'s lighting needs
    /// (§11.2, §14.10) and rebuilds the kept index of the tiles that changed. `None` when
    /// cancelled; otherwise the number of stale leaves that could not be refined.
    fn refine(
        &mut self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        (frame, tile): (&BeamFrame<'_, '_>, u32),
        cancel: &AtomicBool,
    ) -> Option<usize> {
        let refinements = self.refinements(scene, settings, (frame, tile), cancel)?;
        if refinements.is_empty() {
            return Some(0);
        }
        let unrefined = refinements.unrefined_leaves;
        let size = (scene.width, scene.height);
        for ((b, g), leaves) in refinements.groups {
            let Some(batch) = self.batches.get_mut(b) else {
                continue;
            };
            let Some(group) = batch.leaves.get_mut(g) else {
                continue;
            };
            let rect = TileRect::of_index(size, tile, group.0);
            group.1 = leaves;
            if let Some(rect) = rect
                && let Some(entry) = batch.kept.iter_mut().find(|(r, _)| *r == rect)
            {
                entry.1 = KeptIndex::from_leaves(rect, &group.1);
            }
        }
        self.recount();
        Some(unrefined)
    }

    /// Refines the retained leaves for `scene`'s lighting in place (§11.2, §14.10): what
    /// [`Self::relight`] does first. Every leaf whose recorded `τ_ω` is coarser than the lighting
    /// needs is replaced by the leaves a fresh render under that lighting would hold, the kept
    /// index of the changed tiles is rebuilt, and a refinement that grows the cache past its cap
    /// evicts the last batches.
    ///
    /// `Some(())` when done (also when nothing was stale or nothing is retained); `None` when
    /// cancelled, or when `scene` is not a beam-traced frame or the retained batches fit no tile
    /// grid of it.
    pub fn refine_for(
        &mut self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        cancel: &AtomicBool,
    ) -> Option<()> {
        self.refine_for_counted(scene, settings, cancel).map(|_| ())
    }

    /// [`Self::refine_for`] that also returns the number of stale leaves that could not be
    /// refined (they stay at their coarse `τ_ω`; zero when every stale leaf was refined).
    pub fn refine_for_counted(
        &mut self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        cancel: &AtomicBool,
    ) -> Option<usize> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if self.batches.is_empty() {
            return Some(0);
        }
        let frame = BeamFrame::new(scene, settings).ok()?;
        let tile = infer_tile(&self.batches, (scene.width, scene.height))?;
        self.refine(scene, settings, (&frame, tile), cancel)
    }

    /// The refinement [`Self::refine_for`] would apply, without changing the cache: the stale
    /// groups whose rectangle is the `tile`-pixel grid rectangle of their index (the groups a
    /// preloaded render can load), refined for `scene`'s lighting. Empty when nothing is stale;
    /// `None` when cancelled.
    ///
    /// `frame` is the beam frame of `scene` and `tile` the beam tile edge.
    #[must_use]
    pub fn refinements(
        &self,
        scene: &FrameScene<'_>,
        settings: &BeamSettings,
        (frame, tile): (&BeamFrame<'_, '_>, u32),
        cancel: &AtomicBool,
    ) -> Option<Refinements> {
        let target = tau_omega(&scene.environment);
        let size = (scene.width, scene.height);
        let mut jobs = stale_groups(&self.batches, target);
        jobs.retain(|&(b, g)| self.group_on_grid((b, g), size, tile));
        if jobs.is_empty() {
            return Some(Refinements::default());
        }
        // The frame's own preparation (and its optics tables) serves the kit when the frame was
        // built from this very material; else it is prepared here.
        let reflection = if std::ptr::eq(frame.prepared().material, scene.material) {
            ReflectionKit::with_prepared(
                frame.prepared().clone(),
                scene.planes,
                scene.camera,
                size,
                (&scene.environment, frame.n_lambda()),
            )
        } else {
            ReflectionKit::new(
                scene.material,
                scene.planes,
                scene.camera,
                size,
                &scene.environment,
                frame.n_lambda(),
            )
        };
        let retrace = RetraceKit {
            camera: scene.camera,
            size,
            stone: frame.stone(),
            material: scene.material,
            n_lambda: frame.n_lambda(),
        };
        let batches = &self.batches;
        let refined = run_jobs(jobs.len(), default_threads(), cancel, |job| {
            let (b, g) = *jobs.get(job)?;
            let (index, leaves) = batches.get(b)?.leaves.get(g)?;
            let rect = TileRect::of_index(size, tile, *index)?;
            // The render's cancel token reaches the exit-leaf split too.
            let kit = reflection
                .interact_kit(retrace, settings, rect, scene.max_bounces)
                .with_cancel(cancel);
            Some(refine_group(leaves, &kit, target))
        })?;
        let unrefined_leaves: usize = refined.iter().map(|(_, count)| count).sum();
        Some(Refinements {
            groups: jobs
                .into_iter()
                .zip(refined.into_iter().map(|(leaves, _)| leaves))
                .collect(),
            unrefined_leaves,
        })
    }

    /// `true` when the group `(batch, group)` has a tile and a rectangle that is the `tile`-pixel
    /// grid rectangle of its index in a frame of `size`.
    fn group_on_grid(&self, (b, g): (usize, usize), size: (u32, u32), tile: u32) -> bool {
        self.batches.get(b).is_some_and(|batch| {
            batch
                .leaves
                .get(g)
                .zip(batch.tiles.get(g))
                .is_some_and(|((index, _), rect)| {
                    TileRect::of_index(size, tile, *index) == Some(*rect)
                })
        })
    }

    /// Recomputes the byte total and drops the last batches that no longer fit under the cap
    /// (their tiles are traced fresh by the next re-light).
    fn recount(&mut self) {
        let mut total = 0_u64;
        let mut keep = 0_usize;
        for (index, batch) in self.batches.iter().enumerate() {
            let size = retained_bytes(batch);
            if total.saturating_add(size) > self.cap_bytes {
                break;
            }
            total += size;
            keep = index + 1;
        }
        self.batches.truncate(keep);
        self.bytes = total;
    }
}

/// The bytes a retained batch holds in memory: its tiles, its leaves (the structs, their
/// polygons, directions and weights) and its kept indices (regions and per-pixel tables).
///
/// Buffers count at their capacity, which is what they occupy.
#[must_use]
pub fn retained_bytes(batch: &RetainedBatch) -> u64 {
    let mut total = size_of::<RetainedBatch>()
        + batch.tiles.capacity() * size_of::<TileRect>()
        + batch.leaves.capacity() * size_of::<(u32, Vec<Leaf>)>()
        + batch.kept.capacity() * size_of::<(TileRect, KeptIndex)>();
    for (_, leaves) in &batch.leaves {
        total += leaves.capacity() * size_of::<Leaf>();
        for leaf in leaves {
            total += leaf.dirs.capacity() * size_of::<Vec3>()
                + leaf.weights.capacity() * size_of::<f32>();
        }
    }
    let kept: u64 = batch
        .kept
        .iter()
        .fold(0, |sum, (_, index)| sum.saturating_add(index.heap_bytes()));
    u64::try_from(total)
        .unwrap_or(u64::MAX)
        .saturating_add(kept)
}

/// What the re-lights share once the leaves are refined.
struct Plan<'s, 'a> {
    /// The beam frame of the scene (white balance, stone, node count).
    frame: BeamFrame<'s, 'a>,
    /// The frame size in pixels.
    size: (u32, u32),
    /// The tile edge of the retained batches (the adaptive and the beam tile).
    tile: u32,
    /// The frame tile indices no retained batch covers, ascending.
    missing: Vec<u32>,
}

/// A count as `u32`, saturating.
fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `value` when every component is finite, else zero.
fn finite_or_zero(value: Vec3) -> Vec3 {
    if value.is_finite() { value } else { Vec3::ZERO }
}

/// The adaptive settings of the short residual: one pass of `residual_min_spp` samples on tiles of
/// `tile` pixels.
fn short_residual(settings: &BeamSettings, tile: u32) -> AdaptiveSettings {
    let samples = settings.residual_min_spp.max(1);
    AdaptiveSettings {
        target_noise: SHORT_RESIDUAL_TARGET_NOISE,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: samples,
        max_spp: samples,
        pass_spp: samples,
        tile,
    }
}

/// The fewest tiles an empty-cache build cuts a frame into, so its batches share the threads.
const MIN_BUILD_TILES: u32 = 16;

/// The tile edge of an empty-cache build of a `size` frame: the viewport's [`DEFAULT_TILE`],
/// halved down to at most [`MIN_TILE`] while the frame has fewer than [`MIN_BUILD_TILES`] tiles.
///
/// The pipeline works tile by tile, so a small frame in one tile runs on one thread. The edge
/// depends on the frame size alone, never on the thread count, so the image does not either.
const fn build_tile(size: (u32, u32)) -> u32 {
    let mut edge = DEFAULT_TILE;
    while edge > MIN_TILE {
        let (columns, rows) = TileRect::grid(size, edge);
        if columns.saturating_mul(rows) >= MIN_BUILD_TILES {
            break;
        }
        edge /= 2;
    }
    edge
}

/// The tile edge the retained batches were cut on, found from their tile rectangles and indices
/// (the cache does not store it): the smallest edge in `MIN_TILE..=MAX_TILE` for which every
/// group's index names the rectangle of its position. `None` when none does.
fn infer_tile(batches: &[RetainedBatch], size: (u32, u32)) -> Option<u32> {
    let mut edge = MIN_TILE;
    while edge <= MAX_TILE {
        let fits = batches.iter().all(|batch| {
            batch.leaves.len() == batch.tiles.len()
                && batch
                    .leaves
                    .iter()
                    .zip(&batch.tiles)
                    .all(|((index, _), rect)| TileRect::of_index(size, edge, *index) == Some(*rect))
        });
        if fits {
            return Some(edge);
        }
        edge *= 2;
    }
    None
}

/// The frame tile indices no retained batch has a group for, ascending.
fn missing_tiles(batches: &[RetainedBatch], size: (u32, u32), tile: u32) -> Vec<u32> {
    let (columns, rows) = TileRect::grid(size, tile);
    let count = columns.saturating_mul(rows) as usize;
    let mut have = vec![false; count];
    for (index, _) in batches.iter().flat_map(|batch| &batch.leaves) {
        if let Some(slot) = have.get_mut(*index as usize) {
            *slot = true;
        }
    }
    have.iter()
        .enumerate()
        .filter(|(_, covered)| !**covered)
        .map(|(index, _)| to_u32(index))
        .collect()
}

/// `true` when `leaf` was built with a coarser exit-direction tolerance than `target` and
/// interpolates its directions approximately (a reflection leaf is exact).
fn is_stale(leaf: &Leaf, target: f32) -> bool {
    !leaf.reflection && leaf.tau_omega > target
}

/// The `(batch, group)` positions that hold a stale leaf, in order.
fn stale_groups(batches: &[RetainedBatch], target: f32) -> Vec<(usize, usize)> {
    let mut jobs = Vec::new();
    for (b, batch) in batches.iter().enumerate() {
        for (g, (_, leaves)) in batch.leaves.iter().enumerate() {
            if leaves.iter().any(|leaf| is_stale(leaf, target)) {
                jobs.push((b, g));
            }
        }
    }
    jobs
}

/// The leaves of one tile with the stale ones refined in place (a leaf that cannot be refined is
/// kept as it is), in the order the tile's tree produced them, and the number of stale leaves that
/// could not be refined.
///
/// A reflection leaf of the group records `target` too, so a refined group holds one `τ_ω` for its
/// reflection leaves (they are lighting independent; only the recorded value changes).
///
/// The refinement budget differs from the fresh tree's in two ways: the split depth restarts at
/// zero for every leaf (see [`refine_leaf`]), and the stats, and so the tile's
/// `TILE_ALLOWANCE`, are shared by every leaf of the group, while a fresh tree spends the
/// allowance on its whole tree. If either limit trips, the refinement drops the rest as a
/// depth-cap drop where the fresh render would not.
fn refine_group(
    leaves: &[Leaf],
    kit: &super::interact::InteractKit<'_, '_>,
    target: f32,
) -> (Vec<Leaf>, usize) {
    let mut stats = BeamStats::default();
    let mut unrefined = 0_usize;
    let mut out = Vec::with_capacity(leaves.len());
    for leaf in leaves {
        if !is_stale(leaf, target) {
            let mut kept = leaf.clone();
            if kept.reflection {
                kept.tau_omega = target;
            }
            out.push(kept);
            continue;
        }
        let Some(parts) = refine_leaf(leaf, kit, &mut stats) else {
            unrefined += 1;
            out.push(leaf.clone());
            continue;
        };
        out.extend(parts);
    }
    if unrefined > 0 {
        tracing::debug!(
            target: LOG_TARGET,
            unrefined_leaves = unrefined,
            leaves = leaves.len(),
            "beam leaf cache: stale leaves that cannot be retraced stay at their coarse tolerance"
        );
    }
    (out, unrefined)
}

/// The engines of a re-light.
struct Ctx<'c, 'e> {
    /// The caller's engines, in priority order.
    engines: &'c [Box<dyn BeamEngine + 'e>],
    /// The CPU engine for when the set is empty or every engine has declined.
    fallback: &'c dyn BeamEngine,
}

/// Runs `run` with the engines of `engines` and a CPU fallback engine of `scene` whose residual is
/// the short one.
fn with_engines<R>(
    scene: &FrameScene<'_>,
    settings: &BeamSettings,
    plan: &Plan<'_, '_>,
    engines: &EngineSet<'_>,
    run: impl FnOnce(&Ctx<'_, '_>) -> R,
) -> R {
    // The plan's frame already prepared this material; its preparation (tables shared) serves the
    // fallback engine too.
    let prepared = if std::ptr::eq(plan.frame.prepared().material, scene.material) {
        plan.frame.prepared().clone()
    } else {
        PreparedMaterial::new(scene.material)
    };
    let soa = build_plane_soa(scene.planes);
    let adaptive = short_residual(settings, plan.tile);
    let residual = ResidualScene {
        frame: scene,
        extras: (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        plane_soa: &soa,
        prepared: &prepared,
        n_lambda: plan.frame.n_lambda(),
        sample_offset: 0,
        residual_min_spp: settings.residual_min_spp,
    };
    let fallback =
        CpuBeamEngine::new(&residual, settings, &adaptive).with_threads(default_threads());
    run(&Ctx {
        engines: engines.engines.as_slice(),
        fallback: &fallback,
    })
}

/// What the engine side of one batch produced.
struct Shaded {
    /// The white-balanced, unclamped beam part per batch pixel.
    beam: Vec<Vec3>,
    /// The short residual, when it was asked for and ran.
    residual: Option<BatchResidual>,
}

impl Shaded {
    /// The final pixel per batch pixel: the beam part plus the residual mean, clamped at zero
    /// once; without a residual the clamped beam part.
    fn pixels(&self) -> Vec<Vec3> {
        let Some(residual) = &self.residual else {
            return self.beam.iter().map(|&b| b.max(Vec3::ZERO)).collect();
        };
        self.beam
            .iter()
            .enumerate()
            .map(|(at, &beam)| {
                combine_pixel(
                    beam,
                    (
                        residual.sum.get(at).copied().unwrap_or(Vec3::ZERO),
                        residual.half_b.get(at).copied().unwrap_or(Vec3::ZERO),
                    ),
                    (residual.counts.get(at).copied().unwrap_or(0), 0),
                )
                .mean
            })
            .collect()
    }
}

/// How one engine fared with a batch.
enum Attempt {
    /// Both stages are done.
    Done(Shaded),
    /// The engine declined or returned a malformed result.
    Declined,
    /// The cancel token was observed set.
    Cancelled,
}

/// Runs the fragment stage and, when `want_residual`, the residual of `batch` on `engine`.
fn try_engine(
    engine: &dyn BeamEngine,
    frame: &BeamFrame<'_, '_>,
    batch: &Batch<'_>,
    want_residual: bool,
    cancel: &AtomicBool,
) -> Attempt {
    let pixels = batch.pixel_count();
    let fragments = match engine.fragments(batch, cancel) {
        EngineOutcome::Done(done) => done,
        EngineOutcome::Declined(_) => return Attempt::Declined,
        EngineOutcome::Cancelled => return Attempt::Cancelled,
    };
    if fragments.xyz.len() != pixels {
        return Attempt::Declined;
    }
    let beam: Vec<Vec3> = fragments
        .xyz
        .iter()
        .map(|&xyz| frame.balanced(finite_or_zero(xyz)))
        .collect();
    if !want_residual {
        return Attempt::Done(Shaded {
            beam,
            residual: None,
        });
    }
    let residual = match engine.residual(batch, &beam, cancel) {
        EngineOutcome::Done(done) => done,
        EngineOutcome::Declined(_) => return Attempt::Declined,
        EngineOutcome::Cancelled => return Attempt::Cancelled,
    };
    if residual.sum.len() != pixels
        || residual.half_b.len() != pixels
        || residual.counts.len() != pixels
    {
        return Attempt::Declined;
    }
    Attempt::Done(Shaded {
        beam,
        residual: Some(residual),
    })
}

/// Shades a retained batch on the engine `assign` names for its ordinal, failing over to the next
/// on a decline and to the CPU fallback when none is left (the pipeline's rule). `None` when
/// cancelled.
fn shade_batch(
    ctx: &Ctx<'_, '_>,
    frame: &BeamFrame<'_, '_>,
    (batch, ordinal): (&RetainedBatch, u32),
    (failed, want_residual): (&mut [bool], bool),
    cancel: &AtomicBool,
) -> Option<Shaded> {
    loop {
        let slot = assign(ordinal, failed);
        let engine: &dyn BeamEngine = slot
            .and_then(|e| ctx.engines.get(e))
            .map_or(ctx.fallback, |boxed| &**boxed);
        let (packed_leaves, packed_kept) = if engine.needs_packed() {
            (pack_leaves(&batch.leaves), pack_kept(&batch.kept))
        } else {
            (PackedLeaves::default(), PackedKept::default())
        };
        // The view lends the retained leaves (pointers only, no leaf is copied).
        let lent: Vec<(u32, Cow<'_, [Leaf]>)> = batch
            .leaves
            .iter()
            .map(|(tile, leaves)| (*tile, Cow::Borrowed(leaves.as_slice())))
            .collect();
        let view = Batch {
            index: ordinal,
            tiles: &batch.tiles,
            leaves: &lent,
            kept: &batch.kept,
            packed_leaves: &packed_leaves,
            packed_kept: &packed_kept,
        };
        match try_engine(engine, frame, &view, want_residual, cancel) {
            Attempt::Done(done) => return Some(done),
            Attempt::Cancelled => return None,
            Attempt::Declined => match slot.and_then(|e| failed.get_mut(e)) {
                Some(mark) => *mark = true,
                None => {
                    return Some(Shaded {
                        beam: vec![Vec3::ZERO; view.pixel_count()],
                        residual: None,
                    });
                }
            },
        }
    }
}

/// The rectangles of the tiles of `plan.missing`: the requested set whose pixels a fresh render
/// supplies, whether or not a batch of it was retained.
fn missing_rects(plan: &Plan<'_, '_>) -> Vec<TileRect> {
    plan.missing
        .iter()
        .filter_map(|&index| TileRect::of_index(plan.size, plan.tile, index))
        .collect()
}

/// Renders the tiles of `plan.missing` fresh through the batch pipeline and returns the frame
/// image (zero elsewhere). Nothing to do gives an empty image.
///
/// Every finished batch is handed to `admit` as it arrives, so the caller's cap applies batch by
/// batch and nothing accumulates; a residual-only tile has no batch.
/// Nothing is handed over once `cancel` is set. `(cancel, want_residual)`; `None` when the render
/// fails or is cancelled.
fn render_missing(
    scene: &FrameScene<'_>,
    settings: &BeamSettings,
    engines: &EngineSet<'_>,
    plan: &Plan<'_, '_>,
    (cancel, want_residual): (&AtomicBool, bool),
    admit: &mut dyn FnMut(RetainedBatch),
) -> Option<Vec<Vec3>> {
    if plan.missing.is_empty() {
        return Some(Vec::new());
    }
    let soa = build_plane_soa(scene.planes);
    let adaptive = short_residual(settings, plan.tile);
    let mut beam = *settings;
    if !want_residual {
        beam.residual = ResidualMode::Off;
    }
    let live = LiveTiles::from_indices(plan.size.0, plan.size.1, plan.tile, plan.missing.clone());
    let input = HybridInput {
        scene,
        extras: (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        plane_soa: &soa,
        beam: &beam,
        adaptive: &adaptive,
        engines,
        sample_offset: 0,
        scale: 1.0,
    };
    let mut progress = |_: BeamProgress| {};
    let mut keep = |batch: RetainedBatch| {
        if !cancel.load(Ordering::Relaxed) {
            admit(batch);
        }
    };
    let run = BeamRun {
        tiles: Some(&live),
        progress: &mut progress,
        on_batch: None,
        retain: Some(&mut keep),
        preloaded: None,
    };
    let (image, _) = render_hybrid_with_threads(&input, default_threads(), cancel, run).ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(image.pixels)
}

/// Writes `pixels` (the pixels of `tiles` in batch order) into the row-major `out`.
fn scatter(out: &mut [Vec3], width: usize, tiles: &[TileRect], pixels: &[Vec3]) {
    let mut at = 0_usize;
    for rect in tiles {
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let target = y as usize * width + x as usize;
                if let (Some(slot), Some(&value)) = (out.get_mut(target), pixels.get(at)) {
                    *slot = value;
                }
                at += 1;
            }
        }
    }
}

/// Copies the pixels of `tiles` from the frame image `image` into `out`.
fn scatter_from(out: &mut [Vec3], image: &[Vec3], width: usize, tiles: &[TileRect]) {
    for rect in tiles {
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let at = y as usize * width + x as usize;
                if let (Some(slot), Some(&value)) = (out.get_mut(at), image.get(at)) {
                    *slot = value;
                }
            }
        }
    }
}

/// The share of a pixel no leaf covers, from the covered area in square pixels.
fn uncovered_share(covered: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a share in 0..=1 loses nothing that matters in f32"
    )]
    let share = (1.0 - covered).clamp(0.0, 1.0) as f32;
    share
}

/// `floor(v)` as `u32`, clamped to `0..=hi`.
fn floor_to(v: f64, hi: u32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to a valid u32 range just before the cast"
    )]
    let index = v.floor().clamp(0.0, f64::from(hi)) as u32;
    index
}

/// The polygon `ring` clipped to the half-plane `p[axis] >= bound` (`keep_greater`) or
/// `p[axis] <= bound`.
fn clip_axis(ring: &[DVec2], axis: usize, bound: f64, keep_greater: bool) -> Vec<DVec2> {
    let inside = |p: DVec2| {
        if keep_greater {
            p[axis] >= bound
        } else {
            p[axis] <= bound
        }
    };
    let mut out = Vec::with_capacity(ring.len() + 2);
    for (i, &a) in ring.iter().enumerate() {
        let b = ring[(i + 1) % ring.len()];
        let (a_in, b_in) = (inside(a), inside(b));
        if a_in {
            out.push(a);
        }
        if a_in != b_in {
            let t = (bound - a[axis]) / (b[axis] - a[axis]);
            out.push(a + (b - a) * t);
        }
    }
    out
}

/// The area of the convex `ring` inside the pixel cell centred on `(x, y)`.
fn cell_area(ring: &[DVec2], (x, y): (u32, u32)) -> f64 {
    let (cx, cy) = (f64::from(x), f64::from(y));
    let mut clipped = ring.to_vec();
    for (axis, bound, greater) in [
        (0, cx - 0.5, true),
        (0, cx + 0.5, false),
        (1, cy - 0.5, true),
        (1, cy + 0.5, false),
    ] {
        if clipped.len() < 3 {
            return 0.0;
        }
        clipped = clip_axis(&clipped, axis, bound, greater);
    }
    if clipped.len() < 3 {
        return 0.0;
    }
    let twice: f64 = (0..clipped.len())
        .map(|i| clipped[i].perp_dot(clipped[(i + 1) % clipped.len()]))
        .sum();
    twice.abs() * 0.5
}

/// Adds, per pixel, the area (square pixels) the first-surface reflection leaves of `batch` cover.
///
/// The reflection leaves of a frame tile the stone's silhouette without overlap, so their summed
/// area is the share of a pixel the beams cover.
fn add_coverage(covered: &mut [f64], width: usize, batch: &RetainedBatch) {
    let height = covered.len().checked_div(width).unwrap_or(0);
    let (Some(last_x), Some(last_y)) = (width.checked_sub(1), height.checked_sub(1)) else {
        return;
    };
    let (last_x, last_y) = (to_u32(last_x), to_u32(last_y));
    for leaf in batch
        .leaves
        .iter()
        .flat_map(|(_, leaves)| leaves)
        .filter(|leaf| leaf.reflection)
    {
        let (points, count) = leaf.image.points_2d();
        let ring = &points[..count];
        let low = ring
            .iter()
            .fold(DVec2::splat(f64::INFINITY), |m, &p| m.min(p));
        let high = ring
            .iter()
            .fold(DVec2::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
        if !(low.is_finite() && high.is_finite()) {
            continue;
        }
        let (x0, x1) = (
            floor_to(low.x + 0.5, last_x),
            floor_to(high.x + 0.5, last_x),
        );
        let (y0, y1) = (
            floor_to(low.y + 0.5, last_y),
            floor_to(high.y + 0.5, last_y),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(slot) = covered.get_mut(y as usize * width + x as usize) {
                    *slot += cell_area(ring, (x, y));
                }
            }
        }
    }
}

/// The persisted part: the word serialisation of a retained batch and the helpers of
/// `LeafCache::store` and `LeafCache::load`.
///
/// A batch becomes one chunk of the path cache store (`KeyFolder::write_beam_chunk`): little
/// endian words, byte shuffled, compressed, with a CRC32 in the chunk header. The words are
///
/// ```text
/// tile_count, then x0 y0 x1 y1 per tile
/// group_count, then per group: tile index, leaf_count, leaves
/// kept_count, then per kept entry: x0 y0 x1 y1, the index of the leaf group it was built from
/// ```
///
/// and a leaf is its tile, its polygon frame (12 `f64`), its vertices (3 `f64` each), its
/// directions (3 `f32` each), its weights, the spectral interval, the signature, the used prefix
/// steps, the flags and `τ_ω`, then the pre-clip polygon laid out like the leaf polygon, present
/// only when flag bit 2 is set. A kept index is not stored: it is a pure function of its leaves
/// (`KeptIndex::from_leaves`) and is rebuilt on load.
#[cfg(feature = "path-cache-disk")]
mod disk {
    use std::io;

    use glam::{DVec3, Vec3};

    use super::{BEAM_CACHE_FORMAT_VERSION, LeafCache, RetainedBatch};
    use crate::renderer::{
        beam::{
            beam::{BranchPrefix, KeptRegion, Leaf, MAX_PREFIX, SpectralInterval},
            geom::{frame::Frame2, polygon::ConvexPolygon},
            primary::TileRect,
            residual::{KeptIndex, kept_memberships},
        },
        path_cache::{ChunkError, KeyFolder, PersistError, key::CacheKey, persist::BeamChunk},
    };

    /// A chunk error for a batch that cannot be written or read back.
    fn layout(message: &str) -> ChunkError {
        ChunkError::Layout(message.to_owned())
    }

    /// `len` as a word.
    fn to_u32(len: usize) -> Result<u32, ChunkError> {
        u32::try_from(len).map_err(|_| ChunkError::BadLength)
    }

    /// A growing word buffer.
    #[derive(Default)]
    pub struct WordWriter(Vec<u32>);

    impl WordWriter {
        fn put(&mut self, value: u32) {
            self.0.push(value);
        }

        fn put_len(&mut self, len: usize) -> Result<(), ChunkError> {
            self.0.push(to_u32(len)?);
            Ok(())
        }

        fn put_f32(&mut self, value: f32) {
            self.0.push(value.to_bits());
        }

        fn put_f64(&mut self, value: f64) {
            let b = value.to_bits().to_le_bytes();
            self.0.push(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            self.0.push(u32::from_le_bytes([b[4], b[5], b[6], b[7]]));
        }

        fn put_dvec3(&mut self, value: DVec3) {
            self.put_f64(value.x);
            self.put_f64(value.y);
            self.put_f64(value.z);
        }

        fn put_vec3(&mut self, value: Vec3) {
            self.put_f32(value.x);
            self.put_f32(value.y);
            self.put_f32(value.z);
        }

        fn put_rect(&mut self, rect: TileRect) {
            self.put(rect.x0);
            self.put(rect.y0);
            self.put(rect.x1);
            self.put(rect.y1);
        }
    }

    /// A cursor over the words of a chunk; every read is checked.
    pub struct WordReader<'a> {
        words: &'a [u32],
        at: usize,
    }

    impl<'a> WordReader<'a> {
        const fn new(words: &'a [u32]) -> Self {
            Self { words, at: 0 }
        }

        const fn remaining(&self) -> usize {
            self.words.len() - self.at
        }

        fn get(&mut self) -> Result<u32, ChunkError> {
            let word = *self.words.get(self.at).ok_or(ChunkError::Truncated)?;
            self.at += 1;
            Ok(word)
        }

        fn get_f32(&mut self) -> Result<f32, ChunkError> {
            Ok(f32::from_bits(self.get()?))
        }

        fn get_f64(&mut self) -> Result<f64, ChunkError> {
            let lo = u64::from(self.get()?);
            let hi = u64::from(self.get()?);
            Ok(f64::from_bits(lo | (hi << 32)))
        }

        fn get_dvec3(&mut self) -> Result<DVec3, ChunkError> {
            Ok(DVec3::new(
                self.get_f64()?,
                self.get_f64()?,
                self.get_f64()?,
            ))
        }

        fn get_vec3(&mut self) -> Result<Vec3, ChunkError> {
            Ok(Vec3::new(self.get_f32()?, self.get_f32()?, self.get_f32()?))
        }

        fn get_rect(&mut self) -> Result<TileRect, ChunkError> {
            Ok(TileRect {
                x0: self.get()?,
                y0: self.get()?,
                x1: self.get()?,
                y1: self.get()?,
            })
        }

        /// A count of items of at least `per_item` words each; it must fit in what is left, so a
        /// corrupt count never allocates more than the chunk holds.
        fn get_count(&mut self, per_item: usize) -> Result<usize, ChunkError> {
            let count = self.get()? as usize;
            if count
                .checked_mul(per_item)
                .is_none_or(|words| words > self.remaining())
            {
                return Err(layout("a count is larger than the chunk"));
            }
            Ok(count)
        }
    }

    fn put_frame(w: &mut WordWriter, frame: &Frame2) {
        w.put_dvec3(frame.origin);
        w.put_dvec3(frame.u);
        w.put_dvec3(frame.v);
        w.put_dvec3(frame.normal);
    }

    fn get_frame(r: &mut WordReader<'_>) -> Result<Frame2, ChunkError> {
        Ok(Frame2 {
            origin: r.get_dvec3()?,
            u: r.get_dvec3()?,
            v: r.get_dvec3()?,
            normal: r.get_dvec3()?,
        })
    }

    /// A polygon as its frame, its vertex count and its vertices.
    fn put_polygon(w: &mut WordWriter, polygon: &ConvexPolygon) -> Result<(), ChunkError> {
        put_frame(w, polygon.frame());
        w.put_len(polygon.len())?;
        for &vertex in polygon.vertices() {
            w.put_dvec3(vertex);
        }
        Ok(())
    }

    /// The polygon [`put_polygon`] wrote.
    fn get_polygon(r: &mut WordReader<'_>) -> Result<ConvexPolygon, ChunkError> {
        let frame = get_frame(r)?;
        let vertex_count = r.get_count(6)?;
        let vertices = (0..vertex_count)
            .map(|_| r.get_dvec3())
            .collect::<Result<Vec<_>, _>>()?;
        ConvexPolygon::with_frame(&vertices, frame)
            .ok_or_else(|| layout("a leaf polygon is not valid"))
    }

    fn put_leaf(w: &mut WordWriter, leaf: &Leaf) -> Result<(), ChunkError> {
        if usize::from(leaf.prefix.len) > MAX_PREFIX {
            return Err(layout("a leaf prefix is longer than the maximum"));
        }
        w.put(leaf.tile);
        put_polygon(w, &leaf.image)?;
        w.put_len(leaf.dirs.len())?;
        for &dir in &leaf.dirs {
            w.put_vec3(dir);
        }
        w.put_len(leaf.weights.len())?;
        for &weight in &leaf.weights {
            w.put_f32(weight);
        }
        w.put(u32::from(leaf.spectrum.first) | (u32::from(leaf.spectrum.last) << 16));
        w.put(leaf.sig[0]);
        w.put(leaf.sig[1]);
        w.put(u32::from(leaf.prefix.len));
        for &step in leaf.prefix.steps() {
            w.put(step);
        }
        w.put(
            u32::from(leaf.glare)
                | (u32::from(leaf.reflection) << 1)
                | (u32::from(leaf.pre_clip.is_some()) << 2),
        );
        w.put_f32(leaf.tau_omega);
        if let Some(pre_clip) = &leaf.pre_clip {
            put_polygon(w, pre_clip)?;
        }
        Ok(())
    }

    fn get_prefix(r: &mut WordReader<'_>) -> Result<BranchPrefix, ChunkError> {
        let len = r.get()? as usize;
        if len > MAX_PREFIX {
            return Err(layout("a leaf prefix is longer than the maximum"));
        }
        let mut steps = [0_u32; MAX_PREFIX];
        for slot in &mut steps[..len] {
            *slot = r.get()?;
        }
        let len = u8::try_from(len).map_err(|_| ChunkError::BadLength)?;
        Ok(BranchPrefix { steps, len })
    }

    fn get_leaf(r: &mut WordReader<'_>) -> Result<Leaf, ChunkError> {
        let tile = r.get()?;
        let image = get_polygon(r)?;
        let dir_count = r.get_count(3)?;
        let dirs = (0..dir_count)
            .map(|_| r.get_vec3())
            .collect::<Result<Vec<_>, _>>()?;
        let weight_count = r.get_count(1)?;
        let weights = (0..weight_count)
            .map(|_| r.get_f32())
            .collect::<Result<Vec<_>, _>>()?;
        let packed = r.get()?;
        let first = u16::try_from(packed & 0xffff).map_err(|_| ChunkError::BadLength)?;
        let last = u16::try_from(packed >> 16).map_err(|_| ChunkError::BadLength)?;
        if first > last {
            return Err(layout("a leaf spectral interval is empty"));
        }
        let spectrum = SpectralInterval { first, last };
        // One direction and one weight per polygon vertex and interval node: a short vector
        // would shade black and panic a refinement.
        let needed = image.len() * spectrum.len();
        if dirs.len() != needed || weights.len() != needed {
            return Err(layout(
                "a leaf's vertex data do not match its polygon and interval",
            ));
        }
        let sig = [r.get()?, r.get()?];
        let prefix = get_prefix(r)?;
        let flags = r.get()?;
        if flags >> 3 != 0 {
            return Err(layout("a leaf has unknown flags"));
        }
        let tau_omega = r.get_f32()?;
        if !(tau_omega.is_finite() && tau_omega > 0.0) {
            return Err(layout("a leaf's direction tolerance is not positive"));
        }
        let pre_clip = if flags & 4 != 0 {
            Some(get_polygon(r)?)
        } else {
            None
        };
        Ok(Leaf {
            tile,
            image,
            dirs,
            weights,
            spectrum,
            sig,
            prefix,
            glare: flags & 1 != 0,
            reflection: flags & 2 != 0,
            tau_omega,
            pre_clip,
        })
    }

    /// The leaf group `index` (a kept entry at `position`) was built from: the group at the same
    /// position when it matches, else the first group that does.
    fn kept_group(
        batch: &RetainedBatch,
        position: usize,
        rect: TileRect,
        index: &KeptIndex,
    ) -> Option<usize> {
        let matches = |group: usize| {
            batch.leaves.get(group).is_some_and(|(_, leaves)| {
                leaves.len() == index.len() && KeptIndex::from_leaves(rect, leaves) == *index
            })
        };
        if matches(position) {
            return Some(position);
        }
        (0..batch.leaves.len()).find(|&group| group != position && matches(group))
    }

    /// The words of a retained batch.
    ///
    /// # Errors
    ///
    /// Returns [`ChunkError::Layout`] when a kept index is not the one its leaves give (the
    /// index cannot be rebuilt on load) and [`ChunkError::BadLength`] for a count past `u32`.
    pub fn encode_batch(batch: &RetainedBatch) -> Result<Vec<u32>, ChunkError> {
        let mut w = WordWriter::default();
        w.put_len(batch.tiles.len())?;
        for &tile in &batch.tiles {
            w.put_rect(tile);
        }
        w.put_len(batch.leaves.len())?;
        for (tile, leaves) in &batch.leaves {
            w.put(*tile);
            w.put_len(leaves.len())?;
            for leaf in leaves {
                put_leaf(&mut w, leaf)?;
            }
        }
        w.put_len(batch.kept.len())?;
        for (position, (rect, index)) in batch.kept.iter().enumerate() {
            let group = kept_group(batch, position, *rect, index)
                .ok_or_else(|| layout("a kept index does not derive from the batch leaves"))?;
            w.put_rect(*rect);
            w.put_len(group)?;
        }
        Ok(w.0)
    }

    /// The retained batch of `words`.
    ///
    /// # Errors
    ///
    /// Returns the first thing wrong with the words: truncation, a count past the chunk, an
    /// invalid polygon, interval or flag set, a kept entry naming no leaf group, or words left
    /// over.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    pub fn decode_batch(words: &[u32]) -> Result<RetainedBatch, ChunkError> {
        decode_batch_within(words, u64::MAX, None)?
            .ok_or_else(|| layout("the batch exceeds the byte limit"))
    }

    /// The frame size and tile edge a persisted batch must lie on.
    pub type Grid = ((u32, u32), u32);

    /// The bytes the kept index of `rect` with `regions` regions allocates before its per-pixel
    /// lists are known: the regions, the offsets and the counting pass. `None` on overflow.
    ///
    /// Computed from the decoded rectangle before anything is allocated.
    #[must_use]
    pub fn index_plan_bytes(rect: TileRect, regions: usize) -> Option<u64> {
        let width = u64::from(rect.x1.saturating_sub(rect.x0));
        let height = u64::from(rect.y1.saturating_sub(rect.y0));
        let pixels = width.checked_mul(height)?;
        let words = pixels.checked_mul(2)?.checked_add(1)?;
        let regions = u64::try_from(regions)
            .ok()?
            .checked_mul(u64::try_from(size_of::<KeptRegion>()).ok()?)?;
        words.checked_mul(4)?.checked_add(regions)
    }

    /// The bytes the finished kept index of `rect` over `leaves` retains: the regions, the
    /// per-pixel offsets and the region lists with every pixel-to-region membership, the same
    /// accounting as live retention (`KeptIndex::heap_bytes`). `None` on overflow.
    ///
    /// Charged before the index is built, so a batch whose memberships alone pass the limit never
    /// allocates them.
    #[must_use]
    pub fn index_footprint_bytes(rect: TileRect, leaves: &[Leaf]) -> Option<u64> {
        let width = u64::from(rect.x1.saturating_sub(rect.x0));
        let height = u64::from(rect.y1.saturating_sub(rect.y0));
        let word = u64::try_from(size_of::<u32>()).ok()?;
        let offsets = width
            .checked_mul(height)?
            .checked_add(1)?
            .checked_mul(word)?;
        let regions = u64::try_from(leaves.len())
            .ok()?
            .checked_mul(u64::try_from(size_of::<KeptRegion>()).ok()?)?;
        let lists = kept_memberships(rect, leaves).checked_mul(word)?;
        offsets.checked_add(regions)?.checked_add(lists)
    }

    /// `true` when a kept entry's rectangle is one of the batch's tile rectangles and, given the
    /// frame `grid`, the rectangle its group's tile index gives.
    fn kept_rect_valid(
        rect: TileRect,
        tiles: &[TileRect],
        group_tile: u32,
        grid: Option<Grid>,
    ) -> bool {
        tiles.contains(&rect)
            && grid
                .is_none_or(|(size, tile)| TileRect::of_index(size, tile, group_tile) == Some(rect))
    }

    /// Shrinks every buffer of `batch` to its length, so the byte count of a loaded batch is the
    /// same function of its content as a live one.
    pub fn shrink(batch: &mut RetainedBatch) {
        batch.tiles.shrink_to_fit();
        for (_, leaves) in &mut batch.leaves {
            for leaf in leaves.iter_mut() {
                leaf.dirs.shrink_to_fit();
                leaf.weights.shrink_to_fit();
            }
            leaves.shrink_to_fit();
        }
        batch.leaves.shrink_to_fit();
        batch.kept.shrink_to_fit();
    }

    /// [`decode_batch`] that checks each kept entry before building its index.
    ///
    /// A kept rectangle must be one of the batch's tile rectangles and, with a `grid`, the
    /// rectangle its group's tile index gives; anything else is refused. The index allocation is
    /// planned with checked arithmetic against `limit` bytes first: a batch that cannot fit is
    /// parsed in full, then returned as `Ok(None)` without allocating its indices.
    ///
    /// # Errors
    ///
    /// As [`decode_batch`], plus a kept rectangle off the tile list or the grid.
    pub fn decode_batch_within(
        words: &[u32],
        limit: u64,
        grid: Option<Grid>,
    ) -> Result<Option<RetainedBatch>, ChunkError> {
        let mut r = WordReader::new(words);
        let tile_count = r.get_count(4)?;
        let tiles = (0..tile_count)
            .map(|_| r.get_rect())
            .collect::<Result<Vec<_>, _>>()?;
        let group_count = r.get_count(2)?;
        let mut leaves = Vec::new();
        for _ in 0..group_count {
            let tile = r.get()?;
            let leaf_count = r.get_count(1)?;
            let mut group = Vec::new();
            for _ in 0..leaf_count {
                group.push(get_leaf(&mut r)?);
            }
            leaves.push((tile, group));
        }
        let kept_count = r.get_count(5)?;
        let mut kept = Vec::new();
        let mut planned = 0_u64;
        let mut fits = true;
        for _ in 0..kept_count {
            let rect = r.get_rect()?;
            let group = r.get()? as usize;
            let (group_tile, group_leaves) = leaves
                .get(group)
                .ok_or_else(|| layout("a kept entry names a missing leaf group"))?;
            if !kept_rect_valid(rect, &tiles, *group_tile, grid) {
                return Err(layout("a kept rectangle is not a tile of the frame"));
            }
            // The cheap plan first (a huge rectangle is refused without looking at the leaves),
            // then the complete footprint with the memberships.
            let bytes = index_plan_bytes(rect, group_leaves.len())
                .filter(|&plan| plan <= limit)
                .and_then(|_| index_footprint_bytes(rect, group_leaves))
                .and_then(|bytes| planned.checked_add(bytes));
            match bytes {
                Some(total) if total <= limit && fits => planned = total,
                _ => fits = false,
            }
            if fits {
                kept.push((rect, KeptIndex::from_leaves(rect, group_leaves)));
            }
        }
        if r.remaining() != 0 {
            return Err(layout("the chunk has words left over"));
        }
        if !fits {
            return Ok(None);
        }
        let mut batch = RetainedBatch {
            tiles,
            leaves,
            kept,
        };
        shrink(&mut batch);
        Ok(Some(batch))
    }

    /// Reads chunk `index`. The outer error is a real I/O failure; the inner one is the reason the
    /// cache has to be rebuilt (a missing chunk, a failed check).
    fn read_chunk(
        folder: &KeyFolder,
        index: u32,
    ) -> Result<Result<BeamChunk, String>, PersistError> {
        match folder.read_beam_chunk(index, BEAM_CACHE_FORMAT_VERSION) {
            Ok(chunk) => Ok(Ok(chunk)),
            Err(PersistError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                Ok(Err(format!("chunk {index} is missing")))
            }
            Err(error @ PersistError::Io(_)) => Err(error),
            Err(error) => Ok(Err(error.to_string())),
        }
    }

    /// Reads every chunk of `folder` into a cache that retains what fits under `cap_bytes`.
    ///
    /// The outer error is a real I/O failure; the inner one is the reason the cache is corrupt.
    ///
    /// With a `grid` every kept rectangle must lie on that frame's tile grid. The retained size
    /// of a loaded batch is [`super::retained_bytes`], the live accounting.
    pub fn load_all(
        folder: &KeyFolder,
        cap_bytes: u64,
        grid: Option<Grid>,
    ) -> Result<Result<LeafCache, String>, PersistError> {
        let mut chunk = match read_chunk(folder, 0)? {
            Ok(chunk) => chunk,
            Err(reason) => return Ok(Err(reason)),
        };
        let count = chunk.batch_count;
        if count == 0 {
            return Ok(Err("the cache holds no batch".to_owned()));
        }
        let mut cache = LeafCache::new(cap_bytes);
        for index in 0..count {
            if index > 0 {
                chunk = match read_chunk(folder, index)? {
                    Ok(chunk) => chunk,
                    Err(reason) => return Ok(Err(reason)),
                };
            }
            if chunk.batch_count != count {
                return Ok(Err(format!(
                    "chunk {index} counts {} batches, chunk 0 counts {count}",
                    chunk.batch_count
                )));
            }
            let room = cap_bytes.saturating_sub(cache.bytes);
            let batch = match decode_batch_within(&chunk.words, room, grid) {
                Ok(Some(batch)) => batch,
                Ok(None) => continue,
                Err(error) => return Ok(Err(format!("chunk {index}: {error}"))),
            };
            let size = super::retained_bytes(&batch);
            if cache.bytes.saturating_add(size) <= cap_bytes {
                cache.bytes += size;
                cache.batches.push(batch);
            }
        }
        if folder.tile_path(count).exists() {
            return Ok(Err(format!("chunk {count} exists beyond the batch count")));
        }
        Ok(Ok(cache))
    }

    /// Deletes the key folder of a cache that failed verification and logs why.
    pub fn discard(folder: &KeyFolder, reason: &str) {
        tracing::warn!(
            folder = %folder.dir().display(),
            %reason,
            "beam cache: discarding the cache, it will be rebuilt"
        );
        folder.clone().delete();
    }

    /// Stamps the cache as used now from the metadata on disk (the handle passed in may predate
    /// the `finish` of `store`), so the shared LRU sees the use.
    pub fn stamp_used(folder: &KeyFolder) {
        let Some(root) = folder.dir().parent() else {
            return;
        };
        let Some(key) = CacheKey::from_hex(&folder.meta().key) else {
            return;
        };
        if let Ok(mut fresh) = KeyFolder::open(root, &key)
            && let Err(error) = fresh.mark_used()
        {
            tracing::warn!(%error, "beam cache: could not stamp the use time");
        }
    }
}

#[cfg(all(test, feature = "path-cache-disk"))]
#[cfg(all(feature = "beam", beam_tests))]
mod pre_clip_persist_tests {
    //! The pre-clip polygon of a leaf survives the persisted format, and a leaf without one costs
    //! no extra words.

    use glam::{DVec2, Vec3};

    use super::{
        RetainedBatch,
        disk::{decode_batch, encode_batch},
    };
    use crate::renderer::beam::{
        beam::{BranchPrefix, Leaf, SpectralInterval},
        geom::polygon::ConvexPolygon,
        primary::TileRect,
    };

    /// A one-leaf batch whose leaf has a pre-clip polygon when `with_pre_clip`.
    fn batch(with_pre_clip: bool) -> RetainedBatch {
        let corners = |side: f64| {
            ConvexPolygon::from_image_points(&[
                DVec2::new(0.0, 0.0),
                DVec2::new(side, 0.0),
                DVec2::new(side, side),
                DVec2::new(0.0, side),
            ])
            .expect("a square")
        };
        let image = corners(2.0);
        let leaf = Leaf {
            tile: 0,
            image,
            dirs: vec![Vec3::Z; image.len()],
            weights: vec![1.0; image.len()],
            spectrum: SpectralInterval { first: 0, last: 0 },
            sig: [1, 2],
            prefix: BranchPrefix::EMPTY,
            glare: false,
            reflection: false,
            tau_omega: 1.0e-4,
            pre_clip: with_pre_clip.then(|| corners(3.0)),
        };
        RetainedBatch {
            tiles: vec![TileRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 8,
            }],
            leaves: vec![(0, vec![leaf])],
            kept: Vec::new(),
        }
    }

    #[test]
    fn a_pre_clip_polygon_round_trips_and_costs_words_only_when_present() {
        let plain = batch(false);
        let clipped = batch(true);
        let plain_words = encode_batch(&plain).expect("encodes");
        let clipped_words = encode_batch(&clipped).expect("encodes");
        assert!(clipped_words.len() > plain_words.len());
        let back = decode_batch(&clipped_words).expect("decodes");
        assert_eq!(back.leaves[0].1[0], clipped.leaves[0].1[0]);
        assert!(back.leaves[0].1[0].pre_clip.is_some());
        let back = decode_batch(&plain_words).expect("decodes");
        assert_eq!(back.leaves[0].1[0].pre_clip, None);
    }

    /// A batch whose only leaf is changed by `damage` after the fixture built it.
    fn damaged(damage: impl FnOnce(&mut Leaf)) -> Vec<u32> {
        let mut broken = batch(false);
        damage(&mut broken.leaves[0].1[0]);
        encode_batch(&broken).expect("the encoder does not check the vectors")
    }

    #[test]
    fn a_leaf_whose_vectors_do_not_fit_its_polygon_and_interval_is_refused() {
        // A short or long vector must fail the decode, not shade black and panic a refinement
        // later.
        assert!(decode_batch(&damaged(|_| {})).is_ok());
        assert!(decode_batch(&damaged(|leaf| leaf.dirs.truncate(leaf.dirs.len() - 1))).is_err());
        assert!(decode_batch(&damaged(|leaf| leaf.weights.clear())).is_err());
        assert!(decode_batch(&damaged(|leaf| leaf.weights.push(1.0))).is_err());
        assert!(decode_batch(&damaged(|leaf| leaf.spectrum.last = 1)).is_err());
    }

    #[test]
    fn a_leaf_with_a_non_positive_or_non_finite_tolerance_is_refused() {
        for bad in [0.0, -1.0e-4, f32::NAN, f32::INFINITY] {
            assert!(decode_batch(&damaged(|leaf| leaf.tau_omega = bad)).is_err());
        }
    }
}

#[cfg(all(test, feature = "path-cache-disk"))]
#[cfg(all(feature = "beam", beam_tests))]
mod kept_rect_tests {
    //! A persisted kept rectangle is checked before any index is built.

    use super::disk::{decode_batch_within, encode_batch, index_plan_bytes};
    use crate::renderer::beam::{primary::TileRect, residual::KeptIndex};

    /// The words of a batch with one tile of `side` pixels, an empty leaf group and a kept entry.
    fn words_with_tile(side: u32) -> Vec<u32> {
        vec![
            1, 0, 0, side, side, // one tile
            1, 0, 0, // one group: tile index 0, no leaves
            1, 0, 0, side, side, 0, // one kept entry of group 0
        ]
    }

    #[test]
    fn an_oversized_kept_rectangle_is_planned_not_allocated() {
        let huge = TileRect {
            x0: 0,
            y0: 0,
            x1: 65_536,
            y1: 65_536,
        };
        assert_eq!(index_plan_bytes(huge, 0), Some((2 * (1_u64 << 32) + 1) * 4));
        let words = words_with_tile(65_536);
        let loaded = decode_batch_within(&words, 1 << 20, None).expect("well formed");
        assert!(loaded.is_none());
    }

    /// The footprint charged before a load is the retained accounting (offsets, regions, lists).
    #[test]
    fn the_footprint_is_the_retained_size_of_the_index() {
        let rect = TileRect {
            x0: 0,
            y0: 0,
            x1: 8,
            y1: 8,
        };
        let empty = KeptIndex::from_leaves(rect, &[]);
        assert_eq!(
            super::disk::index_footprint_bytes(rect, &[]),
            Some(empty.heap_bytes())
        );
    }

    #[test]
    fn a_kept_rectangle_off_the_tile_grid_is_refused() {
        let rect = TileRect {
            x0: 0,
            y0: 0,
            x1: 8,
            y1: 8,
        };
        let mut batch = super::RetainedBatch {
            tiles: vec![rect],
            leaves: vec![(0, Vec::new())],
            kept: vec![(rect, KeptIndex::from_leaves(rect, &[]))],
        };
        let words = encode_batch(&batch).expect("encodes");
        let on_grid = decode_batch_within(&words, u64::MAX, Some(((8, 8), 8)));
        assert_eq!(on_grid.expect("on the grid").map(|b| b.kept.len()), Some(1));
        assert!(decode_batch_within(&words, u64::MAX, Some(((16, 16), 4))).is_err());
        batch.tiles.clear();
        let words = encode_batch(&batch).expect("encodes");
        assert!(decode_batch_within(&words, u64::MAX, None).is_err());
    }
}

#[cfg(feature = "path-cache-disk")]
impl LeafCache {
    /// Stores the retained leaves under `folder` (`KeyFolder`, codec, CRC and LRU, §14.10).
    ///
    /// One chunk per retained batch is written and the folder is marked complete (through a copy
    /// of the handle: `folder` itself is not updated, open it again to read it). Nothing is
    /// written for an empty cache. Disk-limit eviction is the caller's: see
    /// [`LeafCache::store_with_limit`].
    ///
    /// # Errors
    ///
    /// The persistence error of the write, or [`PersistError::Chunk`] when a kept index is not the
    /// one its leaves give.
    pub fn store(
        &self,
        folder: &crate::renderer::path_cache::KeyFolder,
    ) -> Result<(), crate::renderer::path_cache::PersistError> {
        use crate::renderer::path_cache::PersistError;

        let count = u32::try_from(self.batches.len())
            .map_err(|_| PersistError::Chunk(crate::renderer::path_cache::ChunkError::BadLength))?;
        if count == 0 {
            return Ok(());
        }
        let mut total = 0_u64;
        for (index, batch) in (0..count).zip(&self.batches) {
            let words = disk::encode_batch(batch)?;
            total += folder.write_beam_chunk(index, count, BEAM_CACHE_FORMAT_VERSION, &words)?;
        }
        // A longer cache stored here earlier must not leave its next chunk behind.
        match std::fs::remove_file(folder.tile_path(count)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut finished = folder.clone();
        finished.finish(total)?;
        Ok(())
    }

    /// [`LeafCache::store`], then evicts the least recently used caches under the same root until
    /// the root fits `limit_bytes` (the shared LRU: path-cache and beam folders alike). The
    /// folder just written is never evicted. Returns the evicted keys.
    ///
    /// # Errors
    ///
    /// The persistence error of the write, or the I/O error when the root cannot be listed.
    pub fn store_with_limit(
        &self,
        folder: &crate::renderer::path_cache::KeyFolder,
        limit_bytes: u64,
    ) -> Result<Vec<String>, crate::renderer::path_cache::PersistError> {
        use crate::renderer::path_cache::{key::CacheKey, lru::enforce_limit};

        self.store(folder)?;
        let Some(root) = folder.dir().parent() else {
            return Ok(Vec::new());
        };
        let key = CacheKey::from_hex(&folder.meta().key);
        Ok(enforce_limit(root, limit_bytes, key.as_ref())?)
    }

    /// Loads the leaves stored under `folder`; `Ok(None)` when there are none; a corrupt chunk
    /// deletes the key and returns `Ok(None)` (the render rebuilds silently).
    ///
    /// Batches beyond `cap_bytes` are not retained (a re-light retraces them). A chunk of another
    /// `BEAM_CACHE_FORMAT_VERSION` counts as corrupt. The kept indices are rebuilt from the
    /// leaves. `folder` should come from `KeyFolder::open`.
    ///
    /// # Errors
    ///
    /// The persistence error of the read, other than corruption.
    pub fn load(
        folder: &crate::renderer::path_cache::KeyFolder,
        cap_bytes: u64,
    ) -> Result<Option<Self>, crate::renderer::path_cache::PersistError> {
        Self::load_checked(folder, cap_bytes, None)
    }

    /// [`LeafCache::load`] that also refuses a chunk whose kept rectangles are not the tile-grid
    /// rectangles of a frame of `size` cut into `tile`-pixel tiles.
    ///
    /// # Errors
    ///
    /// As [`LeafCache::load`].
    pub fn load_for_frame(
        folder: &crate::renderer::path_cache::KeyFolder,
        cap_bytes: u64,
        size: (u32, u32),
        tile: u32,
    ) -> Result<Option<Self>, crate::renderer::path_cache::PersistError> {
        Self::load_checked(folder, cap_bytes, Some((size, tile)))
    }

    /// The shared body of the two loaders.
    fn load_checked(
        folder: &crate::renderer::path_cache::KeyFolder,
        cap_bytes: u64,
        grid: Option<disk::Grid>,
    ) -> Result<Option<Self>, crate::renderer::path_cache::PersistError> {
        if !folder.tile_path(0).is_file() {
            return Ok(None);
        }
        match disk::load_all(folder, cap_bytes, grid)? {
            Ok(cache) => {
                disk::stamp_used(folder);
                Ok(Some(cache))
            }
            Err(reason) => {
                disk::discard(folder, &reason);
                Ok(None)
            }
        }
    }

    /// A cache holding exactly `batches` (tests only; it bypasses the retention cap).
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    #[must_use]
    pub fn persist_test_cache(mut batches: Vec<RetainedBatch>) -> Self {
        for batch in &mut batches {
            disk::shrink(batch);
        }
        Self {
            cap_bytes: u64::MAX,
            bytes: batches.iter().map(retained_bytes).sum(),
            batches,
        }
    }

    /// The retained batches (tests only).
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    #[must_use]
    pub fn persist_test_batches(&self) -> &[RetainedBatch] {
        &self.batches
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod dispersive_relight_tests {
    //! Tests of the re-light of a dispersive stone (beam guide §14.10).
    //!
    //! The retained leaves
    //! already went through the tree's bin-edge clipping, a refinement replays that rule on the
    //! pieces it makes, and a re-light under the same `τ_ω` equals a fresh render bit for bit.

    use std::{collections::HashMap, sync::atomic::AtomicBool};

    use glam::Vec3;

    use super::{BeamStats, Leaf, LeafCache, RetainedBatch};
    use crate::{
        optics::{
            fluorescence::Fluorescence,
            raytracer::{EnvironmentSource, LightingPreset, surface::SurfaceScene},
        },
        renderer::{
            adaptive::{AdaptiveSettings, NoiseMetric},
            beam::{
                BeamProgress, BeamRun, BeamSettings,
                engines::EngineSet,
                tests_support::{BeamRig, diamond, glass},
            },
            cpu_frame::render_beam_hybrid_surface,
            env_map::EnvironmentMap,
            frame_scene::FrameScene,
        },
    };

    /// The side of the square test frame in pixels.
    ///
    /// Every property here (bit identity of a re-light, the pre-clip rule, the retention cap) is
    /// exact at any size, so the smallest frame with several tiles serves.
    const FRAME: u32 = 16;

    /// The bounce limit of every render: enough for the refract-reflect-refract paths of the
    /// brilliant, far below the 12 of the statistical rows.
    const BOUNCES: u32 = 6;

    /// The spectral nodes of every render (the minimum multiple of 8 that still disperses).
    const NODES: u32 = 16;

    /// The adaptive tile and the beam tile in pixels.
    const TILE: u32 = 8;

    /// Two tiles per batch (two batches in the 16 pixel frame).
    const BATCH_TILES: u32 = 2;

    /// The residual samples per pixel of every render (one pass); the assertions here are about
    /// structure and bits, not statistics.
    const SPP: u32 = 4;

    /// The width of the HDR map in texels (`τ_ω` about a fifth of the Studio value).
    const HDR_WIDTH: usize = 8192;

    /// The height of the HDR map in texels.
    const HDR_HEIGHT: usize = 16;

    /// The settings of every render.
    fn beam_settings() -> BeamSettings {
        BeamSettings {
            residual_min_spp: SPP,
            spectral_nodes: NODES,
            batch_tiles: BATCH_TILES,
            ..BeamSettings::default()
        }
    }

    /// The adaptive settings of the fresh renders: the single pass the cache runs.
    const fn adaptive_settings() -> AdaptiveSettings {
        AdaptiveSettings {
            target_noise: 1.0e-6,
            metric: NoiseMetric::AbsoluteDeltaE,
            min_spp: SPP,
            max_spp: SPP,
            pass_spp: SPP,
            tile: TILE,
        }
    }

    /// A texel-sharp HDR map: a gradient with narrow bright bands.
    fn hdr_map() -> EnvironmentMap {
        let texels = (0..HDR_WIDTH * HDR_HEIGHT)
            .map(|i| {
                let x = i % HDR_WIDTH;
                let ramp = f32::from(u16::try_from(x % 1024).unwrap_or(0)) / 1024.0;
                let band = if (x / 48) % 7 == 3 { 5.0 } else { 0.0 };
                let value = 0.6_f32.mul_add(ramp, 0.2) + band;
                [value, 0.9 * value, 0.7 * value]
            })
            .collect();
        EnvironmentMap::from_rgb(HDR_WIDTH, HDR_HEIGHT, texels).expect("a consistent map")
    }

    /// The frame of `rig` lit by `environment`.
    fn scene_with<'a>(rig: &'a BeamRig, environment: EnvironmentSource<'a>) -> FrameScene<'a> {
        FrameScene {
            environment,
            ..rig.scene(FRAME, BOUNCES)
        }
    }

    /// A fresh beam render of `scene` on the pipeline's own CPU engine; its finished batches go to
    /// `cache` when one is given.
    fn fresh(rig: &BeamRig, scene: &FrameScene<'_>, cache: Option<&mut LeafCache>) -> Vec<Vec3> {
        fresh_with_stats(rig, scene, cache).0
    }

    /// [`fresh`] with the run's counters (drops and dropped energy by reason) next to the image.
    fn fresh_with_stats(
        rig: &BeamRig,
        scene: &FrameScene<'_>,
        cache: Option<&mut LeafCache>,
    ) -> (Vec<Vec3>, BeamStats) {
        let beam = beam_settings();
        let adaptive = adaptive_settings();
        let engines = EngineSet::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let mut progress = |_: BeamProgress| {};
        let mut sink = cache;
        let mut keep = |batch: RetainedBatch| {
            if let Some(cache) = sink.as_deref_mut() {
                cache.retain(batch);
            }
        };
        let (image, stats) = render_beam_hybrid_surface(
            scene,
            (&[], Fluorescence::none(), SurfaceScene::IDEAL),
            &rig.soa,
            &beam,
            &adaptive,
            &engines,
            (0, 1.0),
            &cancel,
            BeamRun {
                tiles: None,
                progress: &mut progress,
                on_batch: None,
                retain: Some(&mut keep),
                preloaded: None,
            },
        )
        .expect("an eligible stone");
        (image.pixels, stats)
    }

    /// The bit patterns of a vector image.
    fn bits(values: &[Vec3]) -> Vec<[u32; 3]> {
        values
            .iter()
            .map(|v| v.to_array().map(f32::to_bits))
            .collect()
    }

    /// Every retained leaf of `cache`, in batch and tile order.
    fn all_leaves(cache: &LeafCache) -> Vec<&Leaf> {
        cache
            .retained_batches()
            .iter()
            .flat_map(|batch| &batch.leaves)
            .flat_map(|(_, leaves)| leaves)
            .collect()
    }

    /// One entry per leaf of `leaves` (at most three): tile, nodes, image area, the reflection and
    /// pre-clip flags, the branch signature and the centroid.
    fn describe_leaves(leaves: &[&Leaf]) -> String {
        use std::fmt::Write as _;
        leaves.iter().take(3).fold(String::new(), |mut text, leaf| {
            let _ = write!(
                text,
                "[tile {} nodes {}..={} area {:e} reflection {} pre_clip {} sig {:08x}:{:08x} at {:?}]",
                leaf.tile,
                leaf.spectrum.first,
                leaf.spectrum.last,
                leaf.image.area(),
                leaf.reflection,
                leaf.pre_clip.is_some(),
                leaf.sig[0],
                leaf.sig[1],
                leaf.image.centroid().truncate()
            );
            text
        })
    }

    /// How many exit leaves of `fresh` lie under no leaf of `coarse` (same tile, same branch, the
    /// leaf's nodes inside the coarse leaf's): the regions a refinement of `coarse` cannot reach,
    /// because the coarse render kept no leaf whose pre-clip polygon holds them.
    fn uncarried(coarse: &LeafCache, fresh: &LeafCache) -> String {
        let carriers = all_leaves(coarse);
        let (mut count, mut area) = (0_usize, 0.0_f64);
        for leaf in all_leaves(fresh) {
            if leaf.reflection {
                continue;
            }
            let centre = leaf.image.centroid().truncate();
            let carried = carriers.iter().any(|carrier| {
                !carrier.reflection
                    && carrier.tile == leaf.tile
                    && carrier.sig == leaf.sig
                    && carrier.spectrum.first <= leaf.spectrum.first
                    && leaf.spectrum.last <= carrier.spectrum.last
                    && carrier
                        .pre_clip
                        .unwrap_or(carrier.image)
                        .contains_2d(centre, 1.0e-9)
            });
            if !carried {
                count += 1;
                area += leaf.image.area();
            }
        }
        format!("{count} fresh exit leaves ({area:e} px2) lie under no coarse leaf")
    }

    /// How the leaf sets of two caches differ: the leaves each holds that the other does not
    /// (equal tile, nodes, branch and bits), with their total image area and the first few.
    ///
    /// Used to tell, when a refined cache and a fresh render disagree, whether the fresh render
    /// has leaves the refinement cannot make (a leaf that left nothing in the retained set) or
    /// the refinement made leaves the fresh render does not hold.
    fn leaf_gap(refined: &LeafCache, fresh: &LeafCache) -> String {
        type Key = (u32, u16, [u32; 2]);
        let key = |leaf: &Leaf| -> Key { (leaf.tile, leaf.spectrum.first, leaf.sig) };
        // Two reflection leaves are the same leaf whatever `tau_omega` each records: they are
        // lighting independent and contribute identically.
        let same = |a: &Leaf, b: &Leaf| {
            if a.reflection && b.reflection {
                let mut twin = b.clone();
                twin.tau_omega = a.tau_omega;
                twin == *a
            } else {
                a == b
            }
        };
        let (refined_leaves, fresh_leaves) = (all_leaves(refined), all_leaves(fresh));
        let mut pool: HashMap<Key, Vec<(&Leaf, bool)>> = HashMap::new();
        for leaf in &fresh_leaves {
            pool.entry(key(leaf)).or_default().push((*leaf, false));
        }
        let mut only_refined = Vec::new();
        for leaf in &refined_leaves {
            let slot = pool.get_mut(&key(leaf)).and_then(|candidates| {
                candidates
                    .iter_mut()
                    .find(|(other, used)| !*used && same(other, leaf))
            });
            match slot {
                Some(entry) => entry.1 = true,
                None => only_refined.push(*leaf),
            }
        }
        let only_fresh: Vec<&Leaf> = pool
            .values()
            .flatten()
            .filter(|(_, used)| !*used)
            .map(|(leaf, _)| *leaf)
            .collect();
        let area = |leaves: &[&Leaf]| leaves.iter().map(|leaf| leaf.image.area()).sum::<f64>();
        let split = |leaves: &[&Leaf]| {
            let reflection = leaves.iter().filter(|leaf| leaf.reflection).count();
            format!(
                "{reflection} reflection + {} exit",
                leaves.len() - reflection
            )
        };
        // Count and image area per interval, in node order, so the failing node is named.
        let per_node = |leaves: &[&Leaf]| {
            let mut nodes: std::collections::BTreeMap<(u16, u16), (usize, f64)> =
                std::collections::BTreeMap::new();
            for leaf in leaves {
                let entry = nodes
                    .entry((leaf.spectrum.first, leaf.spectrum.last))
                    .or_default();
                entry.0 += 1;
                entry.1 += leaf.image.area();
            }
            nodes
                .iter()
                .map(|((first, last), (count, total))| {
                    format!("{first}..={last}: {count} leaves {total:.4e} px2")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "refined {} leaves, fresh {}; only refined {} ({}, area {:e}) {} by node [{}]; only fresh {} ({}, area {:e}) {} by node [{}]",
            refined_leaves.len(),
            fresh_leaves.len(),
            only_refined.len(),
            split(&only_refined),
            area(&only_refined),
            describe_leaves(&only_refined),
            per_node(&only_refined),
            only_fresh.len(),
            split(&only_fresh),
            area(&only_fresh),
            describe_leaves(&only_fresh),
            per_node(&only_fresh)
        )
    }

    /// Asserts that `got` equals `want` bit for bit; a mismatch reports how many pixels differ,
    /// the largest difference and the first differing pixel, so a targeted run shows the size of
    /// the gap (a few ulps point at summation order, a visible difference at a different leaf
    /// set).
    fn assert_same_image(label: &str, got: &[Vec3], want: &[Vec3]) {
        assert_eq!(got.len(), want.len(), "{label}: the image sizes differ");
        let differing: Vec<usize> = bits(got)
            .iter()
            .zip(bits(want))
            .enumerate()
            .filter(|(_, (a, b))| **a != *b)
            .map(|(at, _)| at)
            .collect();
        let worst = differing
            .iter()
            .map(|&at| (got[at] - want[at]).abs().max_element())
            .fold(0.0_f32, f32::max);
        let first = differing.first().map(|&at| (at, got[at], want[at]));
        assert!(
            differing.is_empty(),
            "{label}: {} of {} pixels differ, largest difference {worst:e}, first {first:?}",
            differing.len(),
            got.len()
        );
    }

    /// A render of `scene` that loads its leaves from `cache` (`BeamRun::preloaded`) and retains
    /// nothing.
    fn preloaded(rig: &BeamRig, scene: &FrameScene<'_>, cache: &LeafCache) -> Vec<Vec3> {
        let beam = beam_settings();
        let adaptive = adaptive_settings();
        let engines = EngineSet::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let mut progress = |_: BeamProgress| {};
        let (image, stats) = render_beam_hybrid_surface(
            scene,
            (&[], Fluorescence::none(), SurfaceScene::IDEAL),
            &rig.soa,
            &beam,
            &adaptive,
            &engines,
            (0, 1.0),
            &cancel,
            BeamRun {
                tiles: None,
                progress: &mut progress,
                on_batch: None,
                retain: None,
                preloaded: Some(cache),
            },
        )
        .expect("an eligible stone");
        assert_eq!(stats.beams, 0, "a preloaded render traces no beam");
        image.pixels
    }

    /// Kept leaves of a Studio render, loaded by a render under the HDR map, give the fresh HDR
    /// render bit for bit: the preloaded path refines the stale leaves like the re-light does.
    fn kept_leaves_under_an_hdr_map_equal_the_fresh_render(rig: &BeamRig) {
        let map = hdr_map();
        let studio = scene_with(rig, LightingPreset::RingLights.studio(1.0, 0.3, 0.9));
        let hdr = scene_with(rig, EnvironmentSource::hdr(&map));
        let mut cache = LeafCache::new(u64::MAX);
        let _ = fresh(rig, &studio, Some(&mut cache));
        assert!(!cache.is_empty(), "the render retained nothing");
        let (leaves, bytes) = (cache.leaf_count(), cache.bytes());
        // The fresh render keeps its leaves too, so a mismatch can say which leaves differ.
        let mut fresh_cache = LeafCache::new(u64::MAX);
        let reference = fresh(rig, &hdr, Some(&mut fresh_cache));
        let cancel = AtomicBool::new(false);
        let mut probe = cache.clone();
        let unrefined = probe
            .refine_for_counted(&hdr, &beam_settings(), &cancel)
            .expect("the refinement completes");
        let gap = leaf_gap(&probe, &fresh_cache);
        eprintln!("leaf sets after refining ({unrefined} unrefinable stale leaves): {gap}");

        let loaded = preloaded(rig, &hdr, &cache);
        eprintln!("preloaded render of {leaves} retained leaves, {bytes} bytes");
        assert_same_image(
            &format!("preloaded, unrefined cache ({gap})"),
            &loaded,
            &reference,
        );
        assert_eq!(
            (cache.leaf_count(), cache.bytes()),
            (leaves, bytes),
            "a preloaded render must not change the cache"
        );

        // Refining in place first gives the same image and the same leaves a re-light keeps.
        assert_eq!(cache.refine_for(&hdr, &beam_settings(), &cancel), Some(()));
        eprintln!(
            "refine_for: {leaves} leaves before, {} after",
            cache.leaf_count()
        );
        let refined = preloaded(rig, &hdr, &cache);
        assert_same_image(
            &format!("preloaded, refined cache ({gap})"),
            &refined,
            &reference,
        );
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn kept_glass_leaves_loaded_under_an_hdr_map_equal_the_fresh_render() {
        kept_leaves_under_an_hdr_map_equal_the_fresh_render(&BeamRig::brilliant(glass()));
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn kept_diamond_leaves_loaded_under_an_hdr_map_equal_the_fresh_render() {
        kept_leaves_under_an_hdr_map_equal_the_fresh_render(&BeamRig::brilliant(diamond()));
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn refining_for_the_same_lighting_changes_nothing_and_a_cancel_gives_none() {
        let rig = BeamRig::brilliant(diamond());
        let scene = scene_with(&rig, LightingPreset::RingLights.studio(1.0, 0.3, 0.9));
        let mut cache = LeafCache::new(u64::MAX);
        assert_eq!(
            cache.refine_for(&scene, &beam_settings(), &AtomicBool::new(false)),
            Some(()),
            "an empty cache has nothing to refine"
        );
        let _ = fresh(&rig, &scene, Some(&mut cache));
        let (leaves, bytes) = (cache.leaf_count(), cache.bytes());
        assert_eq!(
            cache.refine_for(&scene, &beam_settings(), &AtomicBool::new(false)),
            Some(())
        );
        assert_eq!((cache.leaf_count(), cache.bytes()), (leaves, bytes));
        assert_eq!(
            cache.refine_for(&scene, &beam_settings(), &AtomicBool::new(true)),
            None
        );
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn a_full_cache_refuses_a_batch_and_a_later_smaller_one_still_fits() {
        let rig = BeamRig::brilliant(diamond());
        let scene = scene_with(&rig, LightingPreset::RingLights.studio(1.0, 0.3, 0.9));
        let mut all = LeafCache::new(u64::MAX);
        let _ = fresh(&rig, &scene, Some(&mut all));
        // A clone has tight capacities, so its size is the same for every further clone.
        let first = all.retained_batches().first().expect("a batch").clone();
        let mut cache = LeafCache::new(super::retained_bytes(&first));
        assert!(cache.retain(first.clone()));
        assert!(!cache.retain(first), "the second copy exceeds the cap");
        assert_eq!(cache.batch_count(), 1);
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn a_pre_clip_polygon_is_kept_only_where_the_clipping_cut_the_leaf() {
        let rig = BeamRig::brilliant(diamond());
        let scene = scene_with(&rig, LightingPreset::RingLights.studio(1.0, 0.3, 0.9));
        let mut cache = LeafCache::new(u64::MAX);
        let _ = fresh(&rig, &scene, Some(&mut cache));
        let leaves: Vec<&super::Leaf> = cache
            .retained_batches()
            .iter()
            .flat_map(|batch| &batch.leaves)
            .flat_map(|(_, leaves)| leaves)
            .collect();
        let clipped = leaves.iter().filter(|leaf| leaf.pre_clip.is_some()).count();
        eprintln!(
            "diamond: {clipped} of {} leaves carry a pre-clip polygon",
            leaves.len()
        );
        for leaf in &leaves {
            if leaf.reflection {
                assert_eq!(leaf.pre_clip, None, "a reflection leaf is never clipped");
            }
            if let Some(pre) = &leaf.pre_clip {
                assert!(
                    pre.area() >= leaf.image.area() * (1.0 - 1.0e-9),
                    "the clipped image lies inside its pre-clip polygon"
                );
            }
        }
    }

    /// Renders `from` into a cache, re-lights it to `to` and returns the re-lit image, a fresh
    /// render of `to`, the leaf counts before and after and how the re-lit cache's leaves differ
    /// from the fresh render's (see `leaf_gap`).
    fn relit_and_fresh(
        rig: &BeamRig,
        from: EnvironmentSource<'_>,
        to: EnvironmentSource<'_>,
    ) -> (Vec<Vec3>, Vec<Vec3>, (usize, usize), String) {
        let scene_from = scene_with(rig, from);
        let scene_to = scene_with(rig, to);
        let mut cache = LeafCache::new(u64::MAX);
        let (_, coarse_stats) = fresh_with_stats(rig, &scene_from, Some(&mut cache));
        assert!(!cache.is_empty(), "the render retained nothing");
        let before = cache.leaf_count();
        let coarse = cache.clone();
        let engines = EngineSet::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let relit = cache
            .relight(&scene_to, &beam_settings(), &engines, &cancel)
            .expect("retained leaves");
        let after = cache.leaf_count();
        let mut fresh_cache = LeafCache::new(u64::MAX);
        let (reference, fresh_stats) = fresh_with_stats(rig, &scene_to, Some(&mut fresh_cache));
        // The drops of both renders by reason (`DropReason::slot`) and their energy are printed
        // for comparison.
        eprintln!(
            "drops by reason, coarse: {:?}, energy {:?}\ndrops by reason, fresh:  {:?}, energy {:?}",
            coarse_stats.drops,
            coarse_stats.dropped_energy,
            fresh_stats.drops,
            fresh_stats.dropped_energy
        );
        let gap = leaf_gap(&cache, &fresh_cache);
        eprintln!("coarse carriers: {}", uncarried(&coarse, &fresh_cache));
        (relit, reference, (before, after), gap)
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn a_diamond_relight_under_a_new_pose_equals_a_fresh_render_bit_for_bit() {
        let rig = BeamRig::brilliant(diamond());
        let (relit, reference, (before, after), _) = relit_and_fresh(
            &rig,
            LightingPreset::RingLights.studio(1.0, 0.3, 0.9),
            LightingPreset::RingLights.studio(1.0, 1.7, 0.5),
        );
        eprintln!("diamond pose re-light: {before} leaves before, {after} after");
        // The same `τ_ω`: nothing is refined, the retained (clipped) leaves are used as they are.
        assert_eq!(before, after);
        assert_eq!(bits(&relit), bits(&reference));
    }

    #[test]
    #[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
    fn a_diamond_relight_to_a_finer_map_refines_through_the_clipping_bit_for_bit() {
        let rig = BeamRig::brilliant(diamond());
        let map = hdr_map();
        let (relit, reference, (before, after), gap) = relit_and_fresh(
            &rig,
            LightingPreset::RingLights.studio(1.0, 0.3, 0.9),
            EnvironmentSource::hdr(&map),
        );
        eprintln!("diamond hdr re-light leaf sets: {gap}");
        let different = bits(&relit)
            .iter()
            .zip(bits(&reference))
            .filter(|(a, b)| **a != *b)
            .count();
        let worst = relit
            .iter()
            .zip(&reference)
            .map(|(a, b)| (*a - *b).abs().max_element())
            .fold(0.0_f32, f32::max);
        // A leaf keeps its pre-clip polygon (`Leaf::pre_clip`), so the refinement splits the same
        // polygon the fresh tree does and re-clips the pieces: the image is the fresh one.
        eprintln!(
            "diamond hdr re-light: {before} leaves before, {after} after; {different} of {} \
             pixels differ from the fresh render, largest difference {worst:e}",
            relit.len()
        );
        assert_eq!(relit.len(), reference.len());
        assert!(
            relit
                .iter()
                .all(|p| p.is_finite() && p.min_element() >= 0.0)
        );
        assert!(after > 0, "the refinement must keep leaves");
        assert_same_image(
            "the re-light must equal the fresh render",
            &relit,
            &reference,
        );
    }
}
