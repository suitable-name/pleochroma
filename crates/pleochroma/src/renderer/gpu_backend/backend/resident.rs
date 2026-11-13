//! GPU-resident adaptive runs through [`GpuBackend`]: [`GpuBackend::try_adaptive_resident`].
//!
//! The ordinary adaptive pass ([`GpuBackend::try_accumulate_geom_surface_live_cancellable`])
//! reads the live pixels' summed XYZ and half-buffer B back after every pass (24 bytes per live
//! pixel) so the host can measure the tiles. A resident run keeps both accumulators on the GPU
//! for the whole run:
//!
//! - [`GpuResidentRun::pass`] traces a pass into the resident buffers and reads nothing back;
//! - [`GpuResidentRun::tile_errors`] runs the convergence check on the GPU and reads back one
//!   `f32` (4 bytes) per live tile;
//! - [`GpuResidentRun::finish`] reads the whole image back once, at the end.
//!
//! The decision (dilation, which tiles stop) stays on the host, in
//! `AdaptiveRun::finish_pass_with_tile_errors`, over the tile values; the host path and this one
//! share it.
//!
//! # Restricted to a beam batch
//!
//! [`GpuResidentRun::restricted_to`] narrows a run to one beam batch's tiles (the convergence
//! check and the live set cover only those tiles; the batch's beam part is uploaded once and
//! added inside the GPU metric), [`GpuResidentRun::with_reference_kept`] makes every pass the
//! beam residual (reference megakernel plus the batch's kept buffers), and
//! [`GpuResidentRun::finish_rows`] reads back only the rows the batch covers.
//!
//! # When it declines
//!
//! [`GpuBackend::try_adaptive_resident`] returns `None` wherever the live entry declines (too
//! many tools, `work_admits`, a lost device, no renderer, the legacy wavefront kernels), and
//! when the frame's accumulators do not fit one storage binding of the device or another run
//! already holds the resident frame. A run that fails MID-run (a device loss, a cancelled
//! pass, a refused pass) is dead: its sums lived on the GPU, so the caller restarts the render
//! on the host path from sample zero. A run is local to this one backend's GPU; remote and
//! hybrid passes (the tile split of the export thread) keep the host path.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use glam::Vec3;

use super::GpuBackend;
#[cfg(feature = "beam")]
use crate::renderer::{beam::beam::PackedKept, gpu::beam::kept::KeptBufferSet};
use crate::{
    geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive},
    optics::raytracer::surface::SurfaceScene,
    renderer::{
        adaptive::{LiveTiles, NoiseMetric, Pass, PendingCheck},
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameError, GpuFrameRenderer, GpuFrameScene,
            frame::{
                ChunkCursor, ChunkTurnOutcome, LivePass, ResidentCheck, TurnRequest,
                classify_material,
            },
        },
        gpu_backend::{GpuAccumulate, GpuPipelineKind, GpuSceneRef, turnstile::CHUNKS_PER_TURN},
    },
};

/// The id the next resident run takes; ids are never reused within a process.
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// One adaptive render whose sums live on the GPU; see the module documentation.
///
/// Dropping the run frees the resident buffers.
pub struct GpuResidentRun<'a> {
    backend: &'a GpuBackend,
    owner: u64,
    scene: GpuSceneRef<'a>,
    tools: &'a [ToolPrimitive],
    surface: GpuSurfaceScene,
    /// The batch this run is restricted to ([`Self::restricted_to`]); `None` for a whole-frame
    /// run.
    restriction: Option<Restriction>,
    /// The batch's kept regions, uploaded once by [`Self::with_reference_kept`], and the spectral
    /// node count: with them every pass runs the reference megakernel with the kept test.
    #[cfg(feature = "beam")]
    kept: Option<(KeptBufferSet, u32)>,
    /// Set by the first pass or check that did not complete; the run is unusable after it.
    failed: bool,
}

/// The start value the tile-error kernel reads as "this tile has no beam part".
const NO_BEAM_START: u32 = u32::MAX;

/// What [`GpuResidentRun::restricted_to`] fixes about a batch.
struct Restriction {
    /// `(tile index, position in pixels of the tile's first pixel in the beam part)`, ascending
    /// by tile index.
    tile_starts: Vec<(u32, u32)>,
    /// The pixel rows `rows.0..rows.1` the batch's tiles cover (what [`GpuResidentRun::finish_rows`]
    /// reads back).
    rows: (u32, u32),
    /// Whether a beam part was uploaded (an empty offset is "no beam part").
    has_offset: bool,
}

impl Restriction {
    /// The beam-part start of tile `index`, `None` when the batch does not hold the tile.
    fn start_of(&self, index: u32) -> Option<u32> {
        self.tile_starts
            .binary_search_by_key(&index, |&(tile, _)| tile)
            .ok()
            .map(|at| self.tile_starts[at].1)
    }

    /// Whether every tile of `live` belongs to the batch.
    fn covers(&self, live: &LiveTiles) -> bool {
        live.indices()
            .iter()
            .all(|&index| self.start_of(index).is_some())
    }
}

impl GpuBackend {
    /// Starts a GPU-resident adaptive run over `scene`, or `None` to take the host path.
    ///
    /// `pass_spp` is the samples per pass, checked against the work limits like every pass of
    /// the live entry. See the module documentation for what makes it decline.
    #[must_use]
    pub fn try_adaptive_resident<'a>(
        &'a self,
        scene: &GpuSceneRef<'a>,
        tools: &'a [ToolPrimitive],
        surface: &SurfaceScene<'_>,
        pass_spp: u32,
    ) -> Option<GpuResidentRun<'a>> {
        if tools.len() > GPU_MAX_TOOL_PRIMITIVES {
            tracing::debug!(tools = tools.len(), "GPU declined: too many concave tools");
            return None;
        }
        if scene.width == 0 || scene.height == 0 || !self.work_admits(scene, pass_spp) {
            return None;
        }
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return None;
        }
        let mutex = self.renderer.as_ref()?;
        let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        {
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);
            let mut renderer = self.lock_renderer(mutex)?;
            if renderer.pipeline_kind() != GpuPipelineKind::Megakernel {
                tracing::debug!("GPU declined a resident run: the legacy wavefront is selected");
                return None;
            }
            if let Err(error) = renderer.begin_resident(owner, scene.width, scene.height) {
                drop(renderer);
                self.note_resident_error(error);
                return None;
            }
        }
        Some(GpuResidentRun {
            backend: self,
            owner,
            scene: GpuSceneRef {
                camera: scene.camera,
                width: scene.width,
                height: scene.height,
                planes: scene.planes,
                facet_finishes: scene.facet_finishes,
                material: scene.material,
                max_bounces: scene.max_bounces,
                environment: scene.environment,
                estimator: scene.estimator,
            },
            tools,
            surface: GpuSurfaceScene::from_scene(surface),
            restriction: None,
            #[cfg(feature = "beam")]
            kept: None,
            failed: false,
        })
    }

    /// Logs why a resident run could not start or continue; a lost device marks the backend
    /// lost, anything else is an ordinary decline.
    fn note_resident_error(&self, error: GpuFrameError) {
        match error {
            GpuFrameError::DeviceLost(why) => {
                tracing::warn!(
                    "GPU device lost ({why}), disabling the GPU backend until a fresh \
                     device is acquired -- meanwhile every call falls back to the CPU \
                     tracer"
                );
                self.mark_lost(why);
            }
            other => {
                tracing::debug!("GPU declined this resident adaptive run: {other}");
            }
        }
    }
}

impl GpuResidentRun<'_> {
    /// Restricts the run to the tiles of one beam batch (beam guide section 12.4).
    ///
    /// The frame's sums stay on the GPU. From here on every pass and every convergence check
    /// must cover only `tile_starts`' tiles (anything else fails the run), and
    /// [`Self::finish_rows`] reads back only the band `rows.0..rows.1`.
    ///
    /// `tile_starts` is `(tile index, position in pixels of the tile's first pixel in offset)`,
    /// strictly ascending by tile index; `offset` is the batch's beam part, one XYZ per pixel in
    /// the batch layout (the tiles in order, each row-major), uploaded once here and added inside
    /// the GPU metric as the host metric does (`offset + mean_a` against `offset + mean_b`). An
    /// empty `offset` means no beam part: the shipped metric.
    ///
    /// `None` (the run is released) when the tiles are not ascending, `rows` is empty or leaves
    /// the frame, or the GPU could not hold the offset.
    #[must_use]
    pub fn restricted_to(
        mut self,
        tile_starts: &[(u32, u32)],
        offset: &[Vec3],
        rows: (u32, u32),
    ) -> Option<Self> {
        let ascending = tile_starts.windows(2).all(|pair| pair[0].0 < pair[1].0);
        if !ascending || rows.0 >= rows.1 || rows.1 > self.scene.height {
            tracing::debug!("GPU declined a restricted resident run: malformed batch");
            return None;
        }
        if !offset.is_empty() {
            let backend = self.backend;
            let mutex = backend.renderer.as_ref()?;
            let ticket = backend.turnstile.take_ticket();
            let _turn = backend.turnstile.wait_for_turn(ticket);
            if backend.lost.load(Ordering::Acquire) {
                return None;
            }
            let mut renderer = backend.lock_renderer(mutex)?;
            if let Err(error) =
                renderer.upload_resident_offset(self.owner, offset, tile_starts.len())
            {
                drop(renderer);
                backend.note_resident_error(error);
                return None;
            }
        }
        self.restriction = Some(Restriction {
            tile_starts: tile_starts.to_vec(),
            rows,
            has_offset: !offset.is_empty(),
        });
        Some(self)
    }

    /// Makes every pass of the run the beam residual: the reference megakernel with the kept test
    /// of `kept` (`n_lambda` spectral nodes), as
    /// [`GpuBackend::try_accumulate_reference_live_cancellable`] runs it, but adding into the
    /// resident sums. A pass declines (and the run ends) where that entry declines: too few
    /// storage buffers per stage, kept buffers that do not fit a binding, a drifted anchor.
    ///
    /// The kept regions are uploaded here, once for the batch (the run is one batch's), not at
    /// every pass. If the upload is declined the run is marked failed, so its first pass answers
    /// [`GpuAccumulate::Declined`] and the caller takes the host path.
    #[cfg(feature = "beam")]
    #[must_use]
    pub fn with_reference_kept(mut self, kept: &PackedKept, n_lambda: u32) -> Self {
        match self.backend.upload_reference_kept(kept, n_lambda) {
            Some(set) => self.kept = Some((set, n_lambda)),
            None => self.failed = true,
        }
        self
    }
}

impl GpuResidentRun<'_> {
    /// Traces `pass` into the resident sums: the samples `pass.first_sample..+pass.spp` go into
    /// every pixel of `pass.live` and the odd-index ones into half-buffer B. Reads nothing back.
    ///
    /// [`GpuAccumulate::Done`] means the pass is in the resident sums. Any other outcome ends
    /// the run (it answers [`GpuAccumulate::Declined`] from then on): the sums may hold part of
    /// the pass, so the caller either stops (cancelled) or renders again on the host.
    pub fn pass(&mut self, pass: &Pass, cancel: &AtomicBool) -> GpuAccumulate {
        if self.failed {
            return GpuAccumulate::Declined;
        }
        let outcome = self.run_pass(pass, cancel);
        if outcome != GpuAccumulate::Done {
            self.failed = true;
        }
        outcome
    }

    fn run_pass(&self, pass: &Pass, cancel: &AtomicBool) -> GpuAccumulate {
        let backend = self.backend;
        if pass.live.width() != self.scene.width || pass.live.height() != self.scene.height {
            tracing::debug!("GPU declined: the live tiles describe another frame size");
            return GpuAccumulate::Declined;
        }
        if !backend.work_admits(&self.scene, pass.spp) || backend.lost.load(Ordering::Acquire) {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &backend.renderer else {
            return GpuAccumulate::Declined;
        };
        if pass.spp == 0 || pass.live.is_empty() {
            return GpuAccumulate::Done;
        }
        let gpu_scene = GpuFrameScene {
            camera: self.scene.camera,
            width: self.scene.width,
            height: self.scene.height,
            planes: self.scene.planes,
            facet_finishes: self.scene.facet_finishes,
            material: self.scene.material,
            max_bounces: self.scene.max_bounces,
            environment: self.scene.environment,
        };
        let request = TurnRequest {
            scene: &gpu_scene,
            tools: self.tools,
            pipeline_class: classify_material(gpu_scene.material),
            sample_offset: pass.first_sample,
            spp: pass.spp,
            cancel: Some(cancel),
            max_chunks: backend
                .configured_limits()
                .map_or(CHUNKS_PER_TURN, |limits| limits.chunks_per_turn),
            surface: Some(&self.surface),
            estimator: self.scene.estimator,
        };
        if self
            .restriction
            .as_ref()
            .is_some_and(|restriction| !restriction.covers(&pass.live))
        {
            tracing::debug!("GPU declined: the pass has live tiles outside the run's batch");
            return GpuAccumulate::Declined;
        }
        let Some(live_pass) = self.begin_pass(mutex, &pass.live) else {
            return GpuAccumulate::Declined;
        };
        self.run_resident_turns(mutex, &request, &live_pass)
    }

    /// The uploaded live pixels of `live`: the reference pass with the kept buffers for a beam
    /// residual run, the ordinary live pass otherwise. `None` is a decline.
    fn begin_pass(&self, mutex: &Mutex<GpuFrameRenderer>, live: &LiveTiles) -> Option<LivePass> {
        #[cfg(feature = "beam")]
        if let Some((set, n_lambda)) = &self.kept {
            return self
                .backend
                .begin_reference_live_pass_uploaded(mutex, live, set, *n_lambda);
        }
        self.backend.begin_live_pass(mutex, live)
    }

    /// Runs `request` over `live_pass` turn by turn into the resident sums, until it completes,
    /// is cancelled or declines. The turn loop of the live entry's `run_live_turns`.
    fn run_resident_turns(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        live_pass: &LivePass,
    ) -> GpuAccumulate {
        let backend = self.backend;
        let mut cursor = ChunkCursor::default();
        loop {
            // FIFO admission across every caller sharing this backend, a ticket per turn.
            let ticket = backend.turnstile.take_ticket();
            let _turn = backend.turnstile.wait_for_turn(ticket);
            // Re-checked after waiting: the turn ahead may have lost the device.
            if backend.lost.load(Ordering::Acquire) {
                return GpuAccumulate::Declined;
            }
            let Some(mut renderer) = backend.lock_renderer(mutex) else {
                return GpuAccumulate::Declined;
            };
            if let Some(limits) = backend.configured_limits() {
                renderer.set_chunk_budget_bytes(limits.chunk_budget_bytes);
            }
            let outcome =
                renderer.accumulate_live_resident_turn(self.owner, request, live_pass, &mut cursor);
            drop(renderer);

            match outcome {
                Ok(ChunkTurnOutcome::Done) => return GpuAccumulate::Done,
                Ok(ChunkTurnOutcome::Cancelled) => return GpuAccumulate::Cancelled,
                Ok(ChunkTurnOutcome::MoreWork) => {}
                Err(error) => {
                    backend.note_resident_error(error);
                    return GpuAccumulate::Declined;
                }
            }
        }
    }

    /// The noise of every tile of `check.live` after the pass just traced, measured on the GPU.
    ///
    /// One `f32` per tile, in the order of `check.live.indices()`, in the unit of `metric`
    /// (`scale` multiplies the means before the tone-map, as in `AdaptiveRun::finish_pass`);
    /// feed it to `AdaptiveRun::finish_pass_with_tile_errors`. `None` (the run is then dead)
    /// when the GPU could not measure.
    pub fn tile_errors(
        &mut self,
        check: &PendingCheck,
        scale: f32,
        metric: NoiseMetric,
    ) -> Option<Vec<f32>> {
        if self.failed {
            return None;
        }
        let result = self.measure(check, scale, metric);
        if result.is_none() {
            self.failed = true;
        }
        result
    }

    fn measure(&self, check: &PendingCheck, scale: f32, metric: NoiseMetric) -> Option<Vec<f32>> {
        let backend = self.backend;
        let mutex = backend.renderer.as_ref()?;
        let ticket = backend.turnstile.take_ticket();
        let _turn = backend.turnstile.wait_for_turn(ticket);
        if backend.lost.load(Ordering::Acquire) {
            return None;
        }
        // A restricted run measures only its batch's tiles; with a beam part each live tile
        // names where its offset starts.
        let starts: Option<Vec<u32>> = match &self.restriction {
            Some(restriction) if !restriction.covers(&check.live) => return None,
            Some(restriction) if restriction.has_offset => Some(
                check
                    .live
                    .indices()
                    .iter()
                    .map(|&index| restriction.start_of(index).unwrap_or(NO_BEAM_START))
                    .collect(),
            ),
            None | Some(_) => None,
        };
        let mut renderer = backend.lock_renderer(mutex)?;
        let request = ResidentCheck {
            live: &check.live,
            samples: check.samples,
            odd_samples: check.odd_samples,
            scale,
            metric,
        };
        let measured = match &starts {
            Some(starts) => renderer.resident_tile_noise_with_offset(self.owner, &request, starts),
            None => renderer.resident_tile_noise(self.owner, &request),
        };
        match measured {
            Ok(noises) => Some(noises),
            Err(error) => {
                drop(renderer);
                backend.note_resident_error(error);
                None
            }
        }
    }

    /// Ends the run and returns the summed XYZ and half-buffer B of every pixel (full frame,
    /// row-major) in one readback; `None` when the GPU could not deliver them.
    #[must_use]
    pub fn finish(mut self) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
        if self.failed {
            return None;
        }
        let backend = self.backend;
        let mutex = backend.renderer.as_ref()?;
        let ticket = backend.turnstile.take_ticket();
        let _turn = backend.turnstile.wait_for_turn(ticket);
        if backend.lost.load(Ordering::Acquire) {
            return None;
        }
        let renderer = backend.lock_renderer(mutex)?;
        let result = renderer.read_resident(self.owner);
        drop(renderer);
        self.failed = true;
        match result {
            Ok(images) => Some(images),
            Err(error) => {
                backend.note_resident_error(error);
                None
            }
        }
    }
}

impl GpuResidentRun<'_> {
    /// Ends a run restricted by [`Self::restricted_to`] and returns the summed XYZ and
    /// half-buffer B of the pixel rows `rows.0..rows.1` it was restricted to (full width,
    /// row-major) in one readback; `None` when the run was not restricted or the GPU could not
    /// deliver them.
    #[must_use]
    pub fn finish_rows(mut self) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
        if self.failed {
            return None;
        }
        let rows = self.restriction.as_ref()?.rows;
        let backend = self.backend;
        let mutex = backend.renderer.as_ref()?;
        let ticket = backend.turnstile.take_ticket();
        let _turn = backend.turnstile.wait_for_turn(ticket);
        if backend.lost.load(Ordering::Acquire) {
            return None;
        }
        let renderer = backend.lock_renderer(mutex)?;
        let result = renderer.read_resident_rows(self.owner, rows);
        drop(renderer);
        self.failed = true;
        match result {
            Ok(images) => Some(images),
            Err(error) => {
                backend.note_resident_error(error);
                None
            }
        }
    }
}

impl Drop for GpuResidentRun<'_> {
    fn drop(&mut self) {
        let Some(mutex) = &self.backend.renderer else {
            return;
        };
        // Never recover a poisoned lock into a decision, only free what this run holds.
        let mut renderer = mutex.lock().unwrap_or_else(PoisonError::into_inner);
        renderer.release_resident(self.owner);
    }
}
