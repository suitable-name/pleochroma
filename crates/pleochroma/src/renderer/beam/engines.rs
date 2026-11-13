//! Beam engines (beam guide §12.4, §14.5–14.8): who shades a batch's leaves and runs its residual.
//!
//! The CPU builds the beam trees of a batch; an engine (the CPU, the GPU, a remote worker) then
//! shades the fragments and runs the residual of that batch's tiles. Tiles are assigned to engines
//! stickily and deterministically: the assignment is a pure function of the tile index, the engine
//! list and the failure history, so the image does not depend on timing.
//!
//! This file holds the trait, the assignment and the CPU engine ([`CpuBeamEngine`]). The GPU
//! engine is another implementation of the trait.

use std::{
    borrow::Cow,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use glam::Vec3;

use super::{
    BeamSettings,
    beam::{Leaf, PackedKept, PackedLeaves},
    lighting::LightingScene,
    pipeline::{default_threads, run_jobs},
    primary::TileRect,
    raster::shade_tile_cancellable,
    residual::KeptIndex,
    stats::BeamStats,
    tile_residual::{DEFAULT_METRIC_SCALE, ResidualScene, run_batch_residual_with},
};
use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, transport::PreparedMaterial},
        studio_rig::StudioRig,
    },
    renderer::adaptive::AdaptiveSettings,
};

/// One batch of tiles with its beam results, as the engines see it (§12.4).
#[derive(Clone, Copy)]
pub struct Batch<'b> {
    /// The batch's ordinal in the frame (row-major tile order, `batch_tiles` per batch).
    pub index: u32,
    /// The batch's tiles, in tile order; their pixels are disjoint.
    pub tiles: &'b [TileRect],
    /// The leaves per tile (`(tile index, leaves)`, canonical order). A batch traced for this
    /// render owns its leaves ([`Cow::Owned`]); a batch lent by the preloaded leaf cache borrows them
    /// ([`Cow::Borrowed`]), so no leaf is copied.
    pub leaves: &'b [(u32, Cow<'b, [Leaf]>)],
    /// The kept index per tile, for the CPU `kept` callback.
    pub kept: &'b [(TileRect, KeptIndex)],
    /// The leaves in the GPU layout (`leaf::pack_leaves`).
    pub packed_leaves: &'b PackedLeaves,
    /// The kept regions in the GPU layout (`residual::pack_kept`).
    pub packed_kept: &'b PackedKept,
}

impl Batch<'_> {
    /// The number of pixels of the batch (all tiles).
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        self.tiles
            .iter()
            .map(|t| (t.x1 - t.x0) as usize * (t.y1 - t.y0) as usize)
            .sum()
    }
}

/// The batch layout every per-pixel slice uses: the tiles in order, each tile row-major.
///
/// `offset`, [`BatchFragments::xyz`] and [`BatchResidual`]'s vectors all have
/// [`Batch::pixel_count`] entries in this order.
pub const BATCH_PIXEL_ORDER: &str = "tiles in batch order, each tile row-major";

/// The beam part of a batch: per pixel the fragment sum (XYZ, before white balance).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchFragments {
    /// One entry per batch pixel (`BATCH_PIXEL_ORDER`).
    pub xyz: Vec<Vec3>,
}

/// The residual of a batch: the adaptive loop's sums over the batch's tiles (§12.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchResidual {
    /// Summed residual samples per batch pixel (`BATCH_PIXEL_ORDER`).
    pub sum: Vec<Vec3>,
    /// Summed odd-index samples per batch pixel.
    pub half_b: Vec<Vec3>,
    /// Samples each batch pixel received.
    pub counts: Vec<u32>,
    /// Samples traced over the batch.
    pub samples_traced: u64,
}

/// What an engine did with one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineOutcome<T> {
    /// The work is done.
    Done(T),
    /// The engine cannot do it (no device, a lost device, a cap, a gone worker); the text is the
    /// reason for the log. The batch goes to the next engine deterministically.
    Declined(&'static str),
    /// The cancel token was observed set.
    Cancelled,
}

/// One engine: shades a batch's fragments and runs its residual (§14.6 table).
pub trait BeamEngine: Send + Sync {
    /// The engine's name for the per-batch log line ("cpu", "gpu", "remote 10.0.0.7").
    fn name(&self) -> &str;

    /// The batch's beam part: rasterise and light every leaf of the batch's tiles.
    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments>;

    /// The batch's residual: one adaptive loop over the batch's tiles with the reference tracer
    /// and the kept test, the beam part `offset` added inside the noise metric (§12.4).
    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual>;

    /// The counters (fragments, pieces, sun-disc fragments, fragment-cap drops) of the most recent
    /// [`BeamEngine::fragments`] call, for the per-batch log and the render's `BeamStats`. An
    /// engine that does not count them reports all zero.
    ///
    /// The pipeline calls it right after `fragments` returned `Done`, from the one thread that
    /// drives the engines, so "the most recent call" is the batch's.
    fn fragment_stats(&self) -> BeamStats {
        BeamStats::default()
    }

    /// `true` when the engine reads [`Batch::packed_leaves`] and [`Batch::packed_kept`] (the GPU
    /// layouts). The pipeline packs a batch only for an engine that says so; the default is `true`
    /// so an engine that does not know the question is always given the packed form.
    fn needs_packed(&self) -> bool {
        true
    }

    /// `true` when the engine runs its stages on this computer's CPU threads, so the tree producer
    /// leaves it its share of the quota ([`super::pipeline::consumer_threads`]); a GPU or remote
    /// engine occupies one driving thread and the producer takes the rest. The default is `false`.
    fn shares_cpu_threads(&self) -> bool {
        false
    }

    /// [`BeamEngine::residual`] reporting the share of the batch's residual done so far (0 to 1,
    /// never decreasing) after every adaptive pass.
    ///
    /// The default ignores `progress`; the CPU and GPU engines report, so a batch of many passes
    /// moves the render's progress bar.
    fn residual_with_progress(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> EngineOutcome<BatchResidual> {
        let _ = progress;
        self.residual(batch, offset, cancel)
    }

    /// Whether a decline with `reason` is a property of this batch (a size cap: the next batch may
    /// fit) rather than of the engine (a lost device).
    ///
    /// The pipeline then gives only this batch to the next engine and keeps the engine for the
    /// following ones. The default is `false`.
    fn decline_is_per_batch(&self, reason: &'static str) -> bool {
        let _ = reason;
        false
    }
}

/// The engines of a render, in priority order. Engines may borrow the frame's scene for `'e`.
pub struct EngineSet<'e> {
    /// The engines; index 0 is the first choice.
    pub engines: Vec<Box<dyn BeamEngine + 'e>>,
    /// The material the engines were built over, prepared once for the whole render, `None` when
    /// the set does not carry it.
    prepared: Option<PreparedMaterial<'e>>,
}

impl<'e> EngineSet<'e> {
    /// The engine set of `engines`.
    #[must_use]
    pub fn new(engines: Vec<Box<dyn BeamEngine + 'e>>) -> Self {
        Self {
            engines,
            prepared: None,
        }
    }

    /// The set of `engines` that were all built over `prepared` (the `prepared` of their
    /// [`ResidualScene`]).
    ///
    /// The renderer then reuses that preparation, and the optics tables inside it, for its own
    /// frame and fallback engine instead of preparing the same material again. A clone shares the
    /// tables.
    #[must_use]
    pub fn with_prepared(
        engines: Vec<Box<dyn BeamEngine + 'e>>,
        prepared: &PreparedMaterial<'e>,
    ) -> Self {
        Self {
            engines,
            prepared: Some(prepared.clone()),
        }
    }

    /// The preparation of `material` this set carries, when it carries one built from that very
    /// material (the same object, not an equal one); `None` otherwise.
    #[must_use]
    pub fn prepared_for<'m>(&self, material: &'m GemMaterial) -> Option<PreparedMaterial<'m>>
    where
        'e: 'm,
    {
        self.prepared
            .as_ref()
            .filter(|prepared| std::ptr::eq(prepared.material, material))
            .cloned()
    }

    /// The number of engines.
    #[must_use]
    pub fn len(&self) -> usize {
        self.engines.len()
    }

    /// `true` when there is no engine.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.engines.is_empty()
    }
}

/// The engine of `tile` (sticky assignment, §14.5).
///
/// Tiles are dealt round-robin over the live engines in tile order, so the assignment is a pure
/// function of the tile index and of which engines have failed. `failed[e]` marks engine `e`
/// out; `None` when every engine failed.
#[must_use]
pub fn assign(tile: u32, failed: &[bool]) -> Option<usize> {
    let live: Vec<usize> = (0..failed.len()).filter(|&e| !failed[e]).collect();
    if live.is_empty() {
        return None;
    }
    Some(live[tile as usize % live.len()])
}

/// The CPU engine: fragments by `raster::shade_tile` over the batch's leaves, the residual by
/// `tile_residual::run_batch_residual_with` (beam guide §12.4, §14.6 table).
///
/// It never declines a well-formed batch, so it is also the pipeline's last resort when every other
/// engine has failed. Both stages run on `threads` workers with a fixed work split, so the result is
/// bit-identical for any thread count.
pub struct CpuBeamEngine<'e> {
    /// The frame, the reference tracer's inputs and the residual settings.
    scene: ResidualScene<'e>,
    /// The lighting of the fragments.
    lighting: LightingScene<'e>,
    /// The adaptive settings of the residual (their `tile` is the batch tile).
    adaptive: AdaptiveSettings,
    /// The worker threads of both stages.
    threads: usize,
    /// The display multiplier of the residual's noise check.
    display_scale: f32,
    /// The counters of the last `fragments` call, merged in tile order.
    shaded: Mutex<BeamStats>,
}

impl<'e> CpuBeamEngine<'e> {
    /// The CPU engine of `scene`'s frame.
    ///
    /// `scene` is the residual scene of the render (it carries the frame, the tools and surface
    /// extras, the prepared material, the spectral node count, the first sample index and
    /// `residual_min_spp`); `settings` supplies the lighting tolerance and the deepest fragment
    /// subdivision; `adaptive` the residual's sampling (its `tile` must be the tile the batches are
    /// cut on). Runs on all available cores and judges the noise at the display multiplier 1; see
    /// [`CpuBeamEngine::with_threads`] and [`CpuBeamEngine::with_display_scale`].
    #[must_use]
    pub fn new(
        scene: &ResidualScene<'e>,
        settings: &BeamSettings,
        adaptive: &AdaptiveSettings,
    ) -> Self {
        let frame = scene.frame;
        let size = (frame.width, frame.height);
        let rig = match frame.environment {
            EnvironmentSource::Studio {
                light_yaw,
                light_pitch,
                ..
            } => Some(StudioRig::new(light_yaw, light_pitch)),
            EnvironmentSource::HdrMap { .. } => None,
        };
        Self {
            scene: *scene,
            lighting: LightingScene {
                environment: frame.environment,
                rig,
                camera: frame.camera,
                size,
                tolerance: settings.lighting_tolerance,
                max_levels: settings.max_fragment_levels,
                n_lambda: scene.n_lambda,
            },
            adaptive: *adaptive,
            threads: default_threads(),
            display_scale: DEFAULT_METRIC_SCALE,
            shaded: Mutex::new(BeamStats::default()),
        }
    }

    /// The engine on `threads` workers (at least one); the result does not depend on it.
    #[must_use]
    pub const fn with_threads(mut self, threads: usize) -> Self {
        self.threads = if threads == 0 { 1 } else { threads };
        self
    }

    /// The engine with the noise check at the display multiplier `scale` (the `scale` of
    /// `AdaptiveRun::finish_pass`).
    #[must_use]
    pub const fn with_display_scale(mut self, scale: f32) -> Self {
        self.display_scale = scale;
        self
    }

    /// The leaves of the batch tile at `position`, whose frame tile index is `index`.
    fn leaves_of<'b>(batch: &Batch<'b>, position: usize, index: u32) -> &'b [Leaf] {
        batch
            .leaves
            .get(position)
            .filter(|(tile, _)| *tile == index)
            .or_else(|| batch.leaves.iter().find(|(tile, _)| *tile == index))
            .map_or(&[], |(_, leaves)| &**leaves)
    }
}

impl BeamEngine for CpuBeamEngine<'_> {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        let tile = self.adaptive.tile.max(1);
        let columns = self.scene.frame.width.div_ceil(tile);
        let shaded = run_jobs(batch.tiles.len(), self.threads, cancel, |position| {
            let rect = *batch.tiles.get(position)?;
            let index = (rect.y0 / tile) * columns + rect.x0 / tile;
            let leaves = Self::leaves_of(batch, position, index);
            let mut stats = BeamStats::default();
            // Polled per leaf: a cancel inside a tile of many leaves ends the batch at once.
            let buffer = shade_tile_cancellable(leaves, rect, &self.lighting, &mut stats, cancel)?;
            let xyz: Vec<Vec3> = buffer
                .xyz
                .iter()
                .copied()
                .map(glam::DVec3::as_vec3)
                .collect();
            Some((xyz, stats))
        });
        let Some(tiles) = shaded else {
            return if cancel.load(Ordering::Relaxed) {
                EngineOutcome::Cancelled
            } else {
                EngineOutcome::Declined("a batch tile could not be shaded")
            };
        };
        let mut xyz = Vec::with_capacity(batch.pixel_count());
        let mut counted = BeamStats::default();
        for (tile_xyz, tile_stats) in tiles {
            xyz.extend(tile_xyz);
            counted.merge(&tile_stats);
        }
        *self.shaded.lock().unwrap_or_else(PoisonError::into_inner) = counted;
        EngineOutcome::Done(BatchFragments { xyz })
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
        if cancel.load(Ordering::Relaxed) {
            return EngineOutcome::Cancelled;
        }
        let result = run_batch_residual_with(
            &self.scene,
            batch,
            offset,
            &self.adaptive,
            (self.threads, self.display_scale),
            cancel,
            progress,
        );
        match result {
            Some(residual) => EngineOutcome::Done(residual),
            None if cancel.load(Ordering::Relaxed) => EngineOutcome::Cancelled,
            None => EngineOutcome::Declined("the batch is not on the adaptive grid"),
        }
    }

    fn fragment_stats(&self) -> BeamStats {
        self.shaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn needs_packed(&self) -> bool {
        false
    }

    fn shares_cpu_threads(&self) -> bool {
        true
    }
}
