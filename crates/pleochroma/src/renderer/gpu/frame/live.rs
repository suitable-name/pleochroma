//! Adaptive (live-tile) passes of the megakernel: [`GpuFrameRenderer::accumulate_live_turn`].
//!
//! A pass adds the samples `sample_offset..sample_offset + spp` to every pixel of the live
//! tiles of a [`LiveTiles`], into the summed accumulator and into half-buffer B, which receives
//! only the samples with an odd absolute index (the contract in `renderer::adaptive`). Pixels
//! outside the live tiles are never read, written or traced.
//!
//! # How it works
//!
//! The live pixels become one flat list of real pixel indices ([`live_pixel_indices`]: live
//! tiles in ascending order, row-major inside a tile; clipped edge tiles hold only the pixels
//! inside the frame). The pass is the ordinary chunked accumulation over the "virtual" pixels
//! `0..n` of that list. The only kernel difference is the pixel lookup in
//! `transport_generate_ray` ([`super::live_source`]): a virtual pixel maps to its real pixel,
//! so every pixel draws exactly the stratified jitter, wavelengths and seeds it would in an
//! ordinary pass, and `LiveTiles::all` reproduces the ordinary accumulation. The chunk's tuple
//! sums are reduced by `reduce_xyz_live.wgsl` (the same ascending-order sum as
//! `reduce_xyz.wgsl`, plus the odd-sample sum), read back, and added to compact per-live-pixel
//! sums ([`LiveSums`]); the backend adds those into the caller's full-frame buffers at the real
//! pixel indices when the whole pass is done.
//!
//! Chunks are processed one at a time (dispatch, reduce, read back, wait), with the same chunk
//! sizing, turn budget, cancellation and poisoning rules as [`GpuFrameRenderer::accumulate_turn`].
//! Nothing here exists in an ordinary pass: with no adaptive request no buffer of this module is
//! allocated and no pipeline of it is compiled.
//!
//! The legacy [`GpuPipelineKind::Wavefront`] kernels are not supported by live passes; a
//! renderer set to it declines with [`GpuFrameError::LivePassUnavailable`].

use std::{collections::HashMap, sync::atomic::Ordering, time::Instant};

use glam::Vec3;
use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    optics::tables::OPTICS_TABLES_BINDING,
    renderer::{
        adaptive::LiveTiles,
        gpu::{GpuContext, MEGAKERNEL_STORAGE_BUFFERS, compute},
    },
};

use super::{
    CHUNK_TIMING_EMA_ALPHA, ChunkCursor, ChunkFrameState, ChunkSlotResources, ChunkTurnOutcome,
    GpuFrameError, GpuPipelineKind, TurnRequest, TurnSetup,
    dispatch::{build_chunk_params, count_u32, workgroups_for},
    live_source::{LIVE_PIXELS_BINDING, LIVE_REDUCE_ENTRY, LIVE_REDUCE_SHADER_SRC},
    renderer::GpuFrameRenderer,
    resident::{GpuReduceResidentParams, ResidentState},
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
    tray_shadow_source::megakernel_live_source_for,
};

/// Floats read back per live pixel: the summed XYZ, then the odd-sample XYZ.
const LIVE_FLOATS_PER_PIXEL: usize = 6;

/// `reduce_xyz_live.wgsl`'s uniform parameter struct (binding 0 there).
///
/// Four `u32`s, so the 16-byte uniform block needs no implicit padding beyond `_pad0`.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuReduceLiveParams {
    /// Live pixels in this chunk.
    pub(crate) num_pixels: u32,
    /// Samples per pixel in this chunk.
    pub(crate) num_samples: u32,
    /// Absolute index of the pass's first sample; sample `s` of a pixel has index
    /// `sample_offset + s`, and the odd ones feed half-buffer B.
    pub(crate) sample_offset: u32,
    /// Pads the struct to 16 bytes.
    pub(crate) _pad0: u32,
}

/// The real pixel index of every pixel of `live`: the live tiles in ascending order, row-major
/// inside a tile. A clipped edge tile contributes only its pixels inside the frame.
#[must_use]
pub fn live_pixel_indices(live: &LiveTiles) -> Vec<u32> {
    let width = live.width();
    let mut pixels = Vec::with_capacity(usize::try_from(live.pixel_count()).unwrap_or(0));
    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        for y in y0..y1 {
            let row = y * width;
            pixels.extend((x0..x1).map(|x| row + x));
        }
    }
    pixels
}

/// The live pixels of one adaptive pass, uploaded for the GPU. Built once per pass by
/// [`GpuFrameRenderer::begin_live_pass`] and shared by every turn of that pass.
pub struct LivePass {
    /// Real pixel index of each virtual pixel (see [`live_pixel_indices`]).
    pixels: Vec<u32>,
    /// `pixels` as the shader's `live_pixels` storage array.
    pub(super) list: wgpu::Buffer,
    /// The kept regions of a beam residual pass, uploaded; `Some` makes the pass run the
    /// reference megakernel with the kept test instead of the shipped one.
    #[cfg(feature = "beam")]
    pub(super) kept: Option<crate::renderer::gpu::beam::kept::KeptBufferSet>,
}

impl LivePass {
    /// Whether this pass runs the reference megakernel with the kept test.
    #[cfg(feature = "beam")]
    const fn is_reference(&self) -> bool {
        self.kept.is_some()
    }

    /// Whether this pass runs the reference megakernel with the kept test (never, without the
    /// `beam` feature).
    #[cfg(not(feature = "beam"))]
    #[expect(
        clippy::unused_self,
        reason = "the signature of the `beam` build's method, so the callers need no `cfg` of their own"
    )]
    const fn is_reference(&self) -> bool {
        false
    }

    /// The `(binding, buffer)` pairs the pass binds beyond the live list: the kept buffers of a
    /// reference pass, none otherwise.
    #[cfg(feature = "beam")]
    fn extra_bindings(&self) -> Vec<(u32, &wgpu::Buffer)> {
        self.kept
            .as_ref()
            .map_or_else(Vec::new, |kept| kept.bindings().to_vec())
    }

    /// The `(binding, buffer)` pairs the pass binds beyond the live list (none without the `beam`
    /// feature).
    #[cfg(not(feature = "beam"))]
    #[expect(
        clippy::unused_self,
        reason = "the signature of the `beam` build's method, so the callers need no `cfg` of their own"
    )]
    const fn extra_bindings(&self) -> Vec<(u32, &wgpu::Buffer)> {
        Vec::new()
    }

    /// The real pixel index of each virtual pixel, in virtual order.
    #[must_use]
    pub(crate) fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// [`build_chunk_params`] of the chunk `first..first + pixels_this_chunk` of this pass.
    ///
    /// A beam residual pass runs the reference megakernel, whose kept test knows nothing of
    /// collected leaves: it always branches.
    fn chunk_params(
        &self,
        state: &ChunkFrameState<'_>,
        first: usize,
        pixels_this_chunk: usize,
    ) -> crate::renderer::buffers::GpuTransportParams {
        let params = build_chunk_params(state, first, pixels_this_chunk);
        if self.is_reference() {
            params.with_estimator(crate::optics::raytracer::Estimator::Branching, false)
        } else {
            params
        }
    }
}

/// The two accumulators a live pass adds into: the summed samples and half-buffer B (the odd
/// absolute sample indices only), COMPACT -- one entry per live pixel, in the virtual order of
/// [`LivePass::pixels`] (entry `v` belongs to the real pixel `pass.pixels()[v]`). The caller
/// adds them into its full-frame buffers once the pass is done.
pub struct LiveSums<'a> {
    /// The summed accumulator.
    pub(crate) sum: &'a mut [Vec3],
    /// Half-buffer B.
    pub(crate) half_b: &'a mut [Vec3],
}

/// Where the sums of a live chunk go.
pub(super) enum LiveSink<'a, 'b> {
    /// Read back after every chunk and added to compact host sums (the ordinary live pass).
    Host(&'a mut LiveSums<'b>),
    /// Added in place into the GPU-resident frame of [`super::resident`]; nothing is read back.
    Resident,
}

/// One chunk's reduced output: [`LIVE_FLOATS_PER_PIXEL`] floats per pixel, `capacity` pixels.
struct LiveOut {
    buffer: wgpu::Buffer,
    capacity: usize,
}

/// What a [`GpuFrameRenderer`] holds for live passes. All empty until the first live pass.
#[derive(Default)]
pub(super) struct LiveState {
    /// The GPU-resident adaptive run's frame, pipelines and check buffers.
    pub(super) resident: ResidentState,
    /// The path cache's phase B pipelines, buffers and uploaded slabs.
    pub(super) cached: super::cached_pass::CachedState,
    /// The live megakernel pipelines by folded class value
    /// ([`super::material_class::pipeline_key`]), built lazily. Looked up, never iterated.
    pipelines: HashMap<u32, wgpu::ComputePipeline>,
    /// The live reference megakernel pipelines (beam residual passes) by class value, built
    /// lazily. Looked up, never iterated.
    #[cfg(feature = "beam")]
    reference_pipelines: HashMap<u32, wgpu::ComputePipeline>,
    /// `reduce_xyz_live_main`.
    reduce_pipeline: Option<wgpu::ComputePipeline>,
    /// The persistent [`GpuReduceLiveParams`] uniform, rewritten before each chunk's reduce.
    reduce_params: Option<wgpu::Buffer>,
    /// The reduced output of the chunk in flight.
    out: Option<LiveOut>,
}

impl LiveState {
    /// The live pipeline of `class`: the reference one when `reference`, else the shipped one.
    fn pipeline_for(&self, class: u32, reference: bool) -> Option<&wgpu::ComputePipeline> {
        #[cfg(feature = "beam")]
        if reference {
            return self.reference_pipelines.get(&class);
        }
        #[cfg(not(feature = "beam"))]
        let _ = reference;
        self.pipelines
            .get(&super::material_class::pipeline_key(class))
    }
}

/// The megakernel bind group of a live chunk: [`super::bind_groups::build_chunk_bind_group`]'s
/// bindings plus the live pixel list at [`LIVE_PIXELS_BINDING`], the `extra` bindings of a
/// reference pass (the kept buffers), and the optics lookup table buffer (`tables`) when the
/// pipeline is a table variant.
fn build_live_chunk_bind_group(
    ctx: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    (planes_len, outputs): (usize, &TransportOutputs),
    (live_list, extra): (&wgpu::Buffer, &[(u32, &wgpu::Buffer)]),
    (tables, shadow): (Option<&wgpu::Buffer>, Option<&wgpu::Buffer>),
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    let mut entries = vec![
        (0, &frame_buffers.camera, None),
        (1, params_buf, None),
        (2, &frame_buffers.material, None),
        (3, &frame_buffers.planes, Some(planes_bytes)),
        (4, outputs.xyz(), None),
        (5, outputs.radiance(), None),
        (6, outputs.lambdas(), None),
        (7, outputs.path_pdf(), None),
        (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
        (9, outputs.compat(), None),
        (10, &frame_buffers.hdr_env.texels, None),
        (11, &frame_buffers.hdr_env.dims, None),
        (12, &frame_buffers.hdr_env.dist_func, None),
        (13, &frame_buffers.hdr_env.dist_cdf, None),
        (14, &frame_buffers.hdr_env.dist_dims, None),
        (LIVE_PIXELS_BINDING, live_list, None),
    ];
    entries.extend(
        extra
            .iter()
            .map(|&(binding, buffer)| (binding, buffer, None)),
    );
    if let Some(table_buffer) = tables {
        entries.push((OPTICS_TABLES_BINDING, table_buffer, None));
    }
    if let Some(buffer) = shadow {
        entries.push((
            crate::optics::raytracer::environment::tray_shadow::TRAY_SHADOW_BINDING,
            buffer,
            None,
        ));
    }
    compute::bind_buffers_sized(
        &ctx.device,
        "adaptive live transport bind group",
        pipeline,
        &entries,
    )
}

impl GpuFrameRenderer {
    /// Uploads the pixel list of `live` for one adaptive pass.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] if the list does not fit one storage binding of
    /// this device (about 33 million pixels at the 128 MiB baseline limit): the caller then
    /// traces the pass elsewhere.
    pub(crate) fn begin_live_pass(&self, live: &LiveTiles) -> Result<LivePass, GpuFrameError> {
        self.begin_pixel_pass(live_pixel_indices(live))
    }

    /// Uploads an explicit list of real pixel indices as the pixels of one pass, such as the
    /// pixels a path cache does not shade. [`Self::begin_live_pass`] is this with the pixels of
    /// the live tiles.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_live_pass`].
    pub(crate) fn begin_pixel_pass(&self, pixels: Vec<u32>) -> Result<LivePass, GpuFrameError> {
        let limits = self.ctx.device.limits();
        let list_bytes = (pixels.len() * size_of::<u32>()) as u64;
        if list_bytes > limits.max_storage_buffer_binding_size
            || list_bytes > limits.max_buffer_size
        {
            return Err(GpuFrameError::LivePassUnavailable(format!(
                "{} live pixels do not fit one storage binding of this device",
                pixels.len()
            )));
        }
        // A storage binding cannot be empty; an empty pass never dispatches anyway.
        let upload: &[u32] = if pixels.is_empty() { &[0] } else { &pixels };
        let list = compute::upload(
            &self.ctx.device,
            "adaptive live pixel list",
            upload,
            BufferUsages::STORAGE,
        );
        Ok(LivePass {
            pixels,
            list,
            #[cfg(feature = "beam")]
            kept: None,
        })
    }

    /// [`Self::begin_live_pass`] for a beam residual pass: also uploads `kept`, so the pass runs
    /// the reference megakernel with the kept test of `n_lambda` spectral nodes.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_live_pass`], plus [`GpuFrameError::LivePassUnavailable`] when the kept
    /// buffers do not fit one storage binding (or the kept tables are malformed) and
    /// [`GpuFrameError::DeviceLimits`] when the adapter cannot bind the live list and the kept
    /// buffers beside the megakernel's own storage buffers.
    #[cfg(feature = "beam")]
    pub(crate) fn begin_reference_live_pass(
        &self,
        live: &LiveTiles,
        kept: &crate::renderer::beam::beam::PackedKept,
        n_lambda: u32,
    ) -> Result<LivePass, GpuFrameError> {
        use crate::renderer::gpu::beam::{kept::KeptBufferSet, reference_source};

        let available = self
            .ctx
            .device
            .limits()
            .max_storage_buffers_per_shader_stage;
        if !reference_source::reference_live_fits(available) {
            return Err(GpuFrameError::DeviceLimits {
                needed: reference_source::REFERENCE_LIVE_STORAGE_BUFFERS,
                available,
            });
        }
        let mut pass = self.begin_live_pass(live)?;
        pass.kept = Some(
            KeptBufferSet::upload(&self.ctx, kept, n_lambda).ok_or_else(|| {
                GpuFrameError::LivePassUnavailable(
                    "the kept regions do not fit one storage binding of this device".to_string(),
                )
            })?,
        );
        Ok(pass)
    }

    /// [`Self::begin_reference_live_pass`] with the kept regions already uploaded: `kept` came
    /// from `KeptBufferSet::upload` (one upload per batch), so only the live pixel list is
    /// uploaded here. A cheap clone of `kept` (the buffers are shared) rides in the pass.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_reference_live_pass`] except that no upload of the kept regions happens.
    #[cfg(feature = "beam")]
    pub(crate) fn begin_reference_live_pass_uploaded(
        &self,
        live: &LiveTiles,
        kept: crate::renderer::gpu::beam::kept::KeptBufferSet,
    ) -> Result<LivePass, GpuFrameError> {
        use crate::renderer::gpu::beam::reference_source;

        let available = self
            .ctx
            .device
            .limits()
            .max_storage_buffers_per_shader_stage;
        if !reference_source::reference_live_fits(available) {
            return Err(GpuFrameError::DeviceLimits {
                needed: reference_source::REFERENCE_LIVE_STORAGE_BUFFERS,
                available,
            });
        }
        let mut pass = self.begin_live_pass(live)?;
        pass.kept = Some(kept);
        Ok(pass)
    }

    /// Runs one fairness turn of the live pass `pass`, exactly like
    /// [`Self::accumulate_turn`] does for an ordinary request: at most `request.max_chunks`
    /// chunks starting at `cursor` (counted in virtual pixels), nothing left in flight when it
    /// returns, and the renderer poisoned on a device loss.
    ///
    /// `request.scene` is the full frame; the pass covers the pixels of `pass` with samples
    /// `request.sample_offset..+request.spp`, added into the compact `sums` (one entry per
    /// live pixel, see [`LiveSums`]).
    ///
    /// # Errors
    ///
    /// The conditions of [`Self::accumulate`], plus [`GpuFrameError::LivePassUnavailable`] for
    /// the legacy wavefront kernels and [`GpuFrameError::DeviceLimits`] for a device that cannot
    /// bind the live list beside the megakernel's own storage buffers.
    ///
    /// # Panics
    ///
    /// Panics if a slice of `sums` is not as long as `pass` has live pixels.
    pub(crate) fn accumulate_live_turn(
        &mut self,
        request: &TurnRequest<'_>,
        pass: &LivePass,
        sums: &mut LiveSums<'_>,
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        self.accumulate_live_turn_sink(request, pass, &mut LiveSink::Host(sums), cursor)
    }

    /// [`Self::accumulate_live_turn`] for the run `owner`, adding into its GPU-resident frame
    /// instead of host sums: no pixel is read back.
    ///
    /// # Errors
    ///
    /// As [`Self::accumulate_live_turn`], plus [`GpuFrameError::LivePassUnavailable`] when
    /// `owner` holds no resident frame of the request's size.
    pub(crate) fn accumulate_live_resident_turn(
        &mut self,
        owner: u64,
        request: &TurnRequest<'_>,
        pass: &LivePass,
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        if !self.holds_resident(owner, request.scene.width, request.scene.height) {
            return Err(GpuFrameError::LivePassUnavailable(
                "the GPU-resident frame is gone".to_string(),
            ));
        }
        self.accumulate_live_turn_sink(request, pass, &mut LiveSink::Resident, cursor)
    }

    /// The turn wrapper both entries share: the poisoning checks around one body.
    fn accumulate_live_turn_sink(
        &mut self,
        request: &TurnRequest<'_>,
        pass: &LivePass,
        sink: &mut LiveSink<'_, '_>,
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        if self.turn_in_progress {
            self.abandon_in_flight();
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned: a previous turn unwound without completing".to_string(),
            ));
        }
        self.check_uncaptured_error()?;
        self.note_sorted_live_pass();
        self.turn_in_progress = true;
        let result = self.accumulate_live_turn_body(request, pass, sink, cursor);
        self.turn_in_progress = false;
        if matches!(&result, Err(GpuFrameError::DeviceLost(_))) {
            self.abandon_in_flight();
        }
        result
    }

    /// The body [`Self::accumulate_live_turn`] wraps with the poisoning checks.
    fn accumulate_live_turn_body(
        &mut self,
        request: &TurnRequest<'_>,
        pass: &LivePass,
        sink: &mut LiveSink<'_, '_>,
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        let scene = request.scene;
        let live_count = pass.pixels.len();
        if let LiveSink::Host(sums) = &*sink {
            assert_eq!(
                sums.sum.len(),
                live_count,
                "the accumulation buffer must have one entry per live pixel"
            );
            assert_eq!(
                sums.half_b.len(),
                live_count,
                "half-buffer B must have one entry per live pixel"
            );
        }
        if request.spp == 0 || live_count == 0 {
            return Ok(ChunkTurnOutcome::Done);
        }
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }
        let shadow_bit = if scene.environment.tray_shadow().is_some() {
            super::material_class::TRAY_SHADOW
        } else {
            0
        };
        let mirror_bit = if scene.environment.tray_mirror().is_some() {
            super::material_class::TRAY_MIRROR
        } else {
            0
        };
        let tray_bits = shadow_bit | mirror_bit;
        if tray_bits != 0
            && (!self.supports_tray_shadow_class(
                self.effective_class(
                    request.pipeline_class
                        & !(super::material_class::TRAY_SHADOW
                            | super::material_class::TRAY_MIRROR),
                ) | tray_bits,
                true,
            ) || pass.is_reference())
        {
            return Err(GpuFrameError::TrayVariant);
        }
        if self.pipeline_kind != GpuPipelineKind::Megakernel {
            return Err(GpuFrameError::LivePassUnavailable(
                "adaptive passes run on the megakernel, not the legacy wavefront".to_string(),
            ));
        }
        let available = self
            .ctx
            .device
            .limits()
            .max_storage_buffers_per_shader_stage;
        if available <= MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS + 1,
                available,
            });
        }

        let TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        } = self.prepare_turn(request)?;
        if pass.is_reference() {
            // A reference pass adds into the resident frame like any other (the beam residual of
            // a batch with `--gpu-convergence`): the sink only changes where the reduce writes.
            #[cfg(feature = "beam")]
            self.ensure_reference_pipeline(state.pipeline_class)?;
        }
        self.ensure_live_pipelines(state.pipeline_class, pass.is_reference())?;
        if matches!(*sink, LiveSink::Host(_)) {
            self.ensure_live_buffers(byte_budget_pixels.min(live_count));
        }

        let cancelled = self.run_live_chunks(
            request,
            (&state, &frame_buffers, byte_budget_pixels),
            pass,
            sink,
            cursor,
        )?;
        // Persistent scene buffers go back for the next turn, this request's or another's.
        self.scene_buffers = Some(frame_buffers);
        self.check_uncaptured_error()?;

        if cancelled {
            return Ok(ChunkTurnOutcome::Cancelled);
        }
        if cursor.first_pixel >= live_count {
            return Ok(ChunkTurnOutcome::Done);
        }
        Ok(ChunkTurnOutcome::MoreWork)
    }

    /// Builds the live reference megakernel pipeline of `class` (beam residual passes), if not
    /// built yet: the reference text over the live list with the kept buffers, compiled with
    /// the class's specialisation constants. Cached per device (this renderer) and class.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] when an anchor of the reference variant or the
    /// pixel lookup line has drifted.
    #[cfg(feature = "beam")]
    fn ensure_reference_pipeline(&mut self, class: u32) -> Result<(), GpuFrameError> {
        if self.live.reference_pipelines.contains_key(&class) {
            return Ok(());
        }
        let source = super::live_source::reference_live_megakernel_source().ok_or_else(|| {
            GpuFrameError::LivePassUnavailable(
                "the reference megakernel variant no longer matches its anchors".to_string(),
            )
        })?;
        let label = format!(
            "transport_main live reference (MATERIAL_CLASS={}, features {:#x})",
            class & super::material_class::KIND_MASK,
            class & super::material_class::FEATURE_MASK
        );
        let info = self.ctx.adapter.get_info();
        let pipeline = compute::create_persisted_compute_pipeline(
            &self.ctx.device,
            &info,
            &label,
            source,
            "transport_main",
            &super::material_class::pipeline_constants(class),
        );
        self.live.reference_pipelines.insert(class, pipeline);
        Ok(())
    }

    /// Builds the live megakernel pipeline of `class` (unless `reference`: a beam residual pass
    /// builds its own) and the live reduce pipeline, if not built yet.
    fn ensure_live_pipelines(&mut self, class: u32, reference: bool) -> Result<(), GpuFrameError> {
        let info = self.ctx.adapter.get_info();
        if self.live.reduce_pipeline.is_none() {
            self.live.reduce_pipeline = Some(compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                LIVE_REDUCE_ENTRY,
                LIVE_REDUCE_SHADER_SRC,
                LIVE_REDUCE_ENTRY,
                &[],
            ));
        }
        // The shadow, the mirror and both share one pipeline: key and build by the folded class.
        let class = super::material_class::pipeline_key(class);
        if !reference && !self.live.pipelines.contains_key(&class) {
            let source = megakernel_live_source_for(class).ok_or_else(|| {
                GpuFrameError::LivePassUnavailable(
                    "the megakernel unit no longer holds the pixel lookup line".to_string(),
                )
            })?;
            let label = format!(
                "transport_main live (MATERIAL_CLASS={}, features {:#x})",
                class & super::material_class::KIND_MASK,
                class & super::material_class::FEATURE_MASK
            );
            let pipeline = compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                &label,
                source,
                "transport_main",
                &super::material_class::production_pipeline_constants(class),
            );
            self.live.pipelines.insert(class, pipeline);
        }
        Ok(())
    }

    /// Grows the reduced-output buffer to `pixels` pixels and creates the reduce uniform.
    fn ensure_live_buffers(&mut self, pixels: usize) {
        if self.live.reduce_params.is_none() {
            self.live.reduce_params =
                Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("adaptive live reduce params"),
                    size: size_of::<GpuReduceLiveParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let big_enough = self.live.out.as_ref().is_some_and(|o| o.capacity >= pixels);
        if !big_enough {
            self.live.out = Some(LiveOut {
                buffer: compute::zeroed_buffer::<f32>(
                    &self.ctx.device,
                    "adaptive live reduced output",
                    pixels * LIVE_FLOATS_PER_PIXEL,
                    BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                ),
                capacity: pixels,
            });
        }
    }

    /// The chunk loop of one turn; returns whether `request.cancel` fired.
    fn run_live_chunks(
        &mut self,
        request: &TurnRequest<'_>,
        (state, frame_buffers, byte_budget_pixels): (
            &ChunkFrameState<'_>,
            &FrameSceneBuffers,
            usize,
        ),
        pass: &LivePass,
        sink: &mut LiveSink<'_, '_>,
        cursor: &mut ChunkCursor,
    ) -> Result<bool, GpuFrameError> {
        let live_count = pass.pixels.len();
        let mut first = cursor.first_pixel;
        let mut chunk_index = cursor.chunk_index;
        let mut chunks_this_turn = 0usize;
        let mut cancelled = false;
        while first < live_count {
            self.check_uncaptured_error()?;
            if request
                .cancel
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                cancelled = true;
                break;
            }
            if chunks_this_turn >= request.max_chunks {
                break;
            }
            let chunk_pixels = Self::next_chunk_pixels(
                self.ns_per_tuple_ema,
                self.growth_limited_pixels(state.spp, byte_budget_pixels),
                state.spp,
                live_count,
            );
            let pixels_this_chunk = chunk_pixels.min(live_count - first);
            self.run_live_chunk(state, frame_buffers, pass, (first, pixels_this_chunk), sink)?;
            first += pixels_this_chunk;
            chunk_index += 1;
            chunks_this_turn += 1;
        }
        cursor.first_pixel = first;
        cursor.chunk_index = chunk_index;
        Ok(cancelled)
    }

    /// Traces, reduces and reads back the virtual pixels `first..first + pixels_this_chunk` of
    /// `pass`, waits for the result, and adds it to `sums` at the real pixel indices.
    fn run_live_chunk(
        &mut self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        pass: &LivePass,
        (first, pixels_this_chunk): (usize, usize),
        sink: &mut LiveSink<'_, '_>,
    ) -> Result<(), GpuFrameError> {
        let LiveSink::Host(sums) = sink else {
            return self.run_resident_chunk(state, frame_buffers, pass, (first, pixels_this_chunk));
        };
        self.check_device_error()?;
        let started = Instant::now();
        let tuples = pixels_this_chunk * state.spp as usize;
        let params = pass.chunk_params(state, first, pixels_this_chunk);

        let data = {
            let device = &self.ctx.device;
            let queue = &self.ctx.queue;
            let ChunkSlotResources {
                outputs,
                params_buf,
                ..
            } = self.resolve_chunk_slot_resources(0, tuples, pixels_this_chunk);
            let reference = pass.is_reference();
            let (Some(pipeline), Some(reduce), Some(reduce_params), Some(out)) = (
                self.live.pipeline_for(state.pipeline_class, reference),
                self.live.reduce_pipeline.as_ref(),
                self.live.reduce_params.as_ref(),
                self.live.out.as_ref(),
            ) else {
                return Err(GpuFrameError::LivePassUnavailable(
                    "live pipelines were not prepared".to_string(),
                ));
            };

            queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
            // The reference megakernel has no table binding, so its bind group never carries
            // the optics table buffer.
            let tables = if reference {
                None
            } else {
                self.optics_tables_for(state.pipeline_class)
            };
            let bind_group = build_live_chunk_bind_group(
                &self.ctx,
                pipeline,
                frame_buffers,
                params_buf,
                (state.packed_len, outputs),
                (&pass.list, &pass.extra_bindings()),
                (tables, self.tray_shadow_for(state.pipeline_class)),
            );
            let _ = compute::dispatch(
                device,
                queue,
                pipeline,
                &bind_group,
                (workgroups_for(tuples), 1, 1),
            );
            self.check_device_error()?;

            queue.write_buffer(
                reduce_params,
                0,
                bytemuck::bytes_of(&GpuReduceLiveParams {
                    num_pixels: count_u32(pixels_this_chunk),
                    num_samples: state.spp,
                    sample_offset: state.sample_offset,
                    _pad0: 0,
                }),
            );
            let reduce_bind_group = compute::bind_buffers(
                device,
                "adaptive live reduce bind group",
                reduce,
                &[(0, reduce_params), (1, outputs.xyz()), (2, &out.buffer)],
            );
            let _ = compute::dispatch(
                device,
                queue,
                reduce,
                &reduce_bind_group,
                (workgroups_for(pixels_this_chunk), 1, 1),
            );
            self.check_device_error()?;

            let (staging, copy_index) = compute::copy_to_staging::<f32>(
                device,
                queue,
                &out.buffer,
                pixels_this_chunk * LIVE_FLOATS_PER_PIXEL,
                "adaptive live readback staging",
            );
            let rx = compute::begin_map_read(&staging);
            compute::finish_map_read::<f32>(device, &staging, copy_index, &rx).map_err(|e| {
                GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string()))
            })?
        };

        for (local, floats) in data
            .as_chunks::<LIVE_FLOATS_PER_PIXEL>()
            .0
            .iter()
            .take(pixels_this_chunk)
            .enumerate()
        {
            let virtual_pixel = first + local;
            sums.sum[virtual_pixel] += Vec3::new(floats[0], floats[1], floats[2]);
            sums.half_b[virtual_pixel] += Vec3::new(floats[3], floats[4], floats[5]);
        }

        self.record_chunk_timing(started, tuples);
        Ok(())
    }

    /// Feeds one finished chunk's wall time into the chunk-size estimate.
    pub(super) fn record_chunk_timing(&mut self, started: Instant, tuples: usize) {
        let measured_ns_per_tuple = started.elapsed().as_nanos() as f64 / tuples.max(1) as f64;
        self.measured_chunk_tuples = Some(tuples);
        self.ns_per_tuple_ema = Some(
            self.ns_per_tuple_ema
                .map_or(measured_ns_per_tuple, |previous| {
                    CHUNK_TIMING_EMA_ALPHA.mul_add(measured_ns_per_tuple - previous, previous)
                }),
        );
    }

    /// Traces the virtual pixels `first..first + pixels_this_chunk` of `pass` and adds their
    /// totals into the GPU-resident frame, waiting for the GPU to finish the chunk; nothing is
    /// read back. The chunk loop is [`Self::run_live_chunk`]'s, with the readback replaced by
    /// the in-place reduce of `reduce_xyz_resident.wgsl`.
    fn run_resident_chunk(
        &mut self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        pass: &LivePass,
        (first, pixels_this_chunk): (usize, usize),
    ) -> Result<(), GpuFrameError> {
        self.check_device_error()?;
        let started = Instant::now();
        let tuples = pixels_this_chunk * state.spp as usize;
        let params = pass.chunk_params(state, first, pixels_this_chunk);

        let submission = {
            let device = &self.ctx.device;
            let queue = &self.ctx.queue;
            let ChunkSlotResources {
                outputs,
                params_buf,
                ..
            } = self.resolve_chunk_slot_resources(0, tuples, pixels_this_chunk);
            let resident = &self.live.resident;
            let reference = pass.is_reference();
            let (Some(pipeline), Some(reduce), Some(reduce_params), Some(frame)) = (
                self.live.pipeline_for(state.pipeline_class, reference),
                resident.reduce.as_ref(),
                resident.reduce_params.as_ref(),
                resident.frame.as_ref(),
            ) else {
                return Err(GpuFrameError::LivePassUnavailable(
                    "resident live pipelines were not prepared".to_string(),
                ));
            };

            queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
            let bind_group = build_live_chunk_bind_group(
                &self.ctx,
                pipeline,
                frame_buffers,
                params_buf,
                (state.packed_len, outputs),
                (&pass.list, &pass.extra_bindings()),
                (
                    // The reference megakernel has no table binding (see `run_live_chunk`).
                    if reference {
                        None
                    } else {
                        self.optics_tables_for(state.pipeline_class)
                    },
                    self.tray_shadow_for(state.pipeline_class),
                ),
            );
            let _ = compute::dispatch(
                device,
                queue,
                pipeline,
                &bind_group,
                (workgroups_for(tuples), 1, 1),
            );
            self.check_device_error()?;

            queue.write_buffer(
                reduce_params,
                0,
                bytemuck::bytes_of(&GpuReduceResidentParams {
                    num_pixels: count_u32(pixels_this_chunk),
                    num_samples: state.spp,
                    sample_offset: state.sample_offset,
                    first_virtual: count_u32(first),
                }),
            );
            let reduce_bind_group = compute::bind_buffers(
                device,
                "adaptive resident reduce bind group",
                reduce,
                &[
                    (0, reduce_params),
                    (1, outputs.xyz()),
                    (2, &pass.list),
                    (3, &frame.sum),
                    (4, &frame.half_b),
                ],
            );
            compute::dispatch(
                device,
                queue,
                reduce,
                &reduce_bind_group,
                (workgroups_for(pixels_this_chunk), 1, 1),
            )
        };
        self.check_device_error()?;

        // The wait that keeps the chunk loop synchronous (the chunk-size estimate and the
        // watchdog bound rely on one chunk in flight); the host reads nothing.
        self.ctx
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(compute::GPU_WAIT_TIMEOUT),
            })
            .map_err(|e| {
                GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string()))
            })?;
        self.check_device_error()?;

        self.record_chunk_timing(started, tuples);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::live_pixel_indices;
    use crate::renderer::adaptive::LiveTiles;

    #[test]
    fn the_pixel_list_holds_each_live_pixel_once_in_tile_then_row_order() {
        // 10 x 6 frame, 4 x 4 tiles: 3 x 2 tiles, the right column 2 pixels wide and the
        // bottom row 2 pixels high.
        let live = LiveTiles::from_indices(10, 6, 4, vec![5, 1, 2]);
        assert_eq!(live.indices(), &[1, 2, 5]);
        let pixels = live_pixel_indices(&live);
        assert_eq!(u64::try_from(pixels.len()).ok(), Some(live.pixel_count()));

        // Tile 1: x 4..8, y 0..4 -- rows of 4, in row order.
        assert_eq!(&pixels[0..4], &[4, 5, 6, 7]);
        assert_eq!(&pixels[4..8], &[14, 15, 16, 17]);
        // Tile 2 is the clipped right edge tile: x 8..10, y 0..4 -- rows of 2.
        assert_eq!(&pixels[16..18], &[8, 9]);
        assert_eq!(&pixels[18..20], &[18, 19]);
        // Tile 5 is the clipped corner tile: x 8..10, y 4..6.
        let tail = &pixels[pixels.len() - 4..];
        assert_eq!(tail, &[48, 49, 58, 59]);

        let mut sorted = pixels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), pixels.len(), "no pixel appears twice");
        assert!(
            pixels.iter().all(|&p| p < 60),
            "every pixel is in the frame"
        );
    }

    #[test]
    fn all_tiles_list_every_pixel_of_the_frame() {
        let live = LiveTiles::all(13, 9, 8);
        let mut pixels = live_pixel_indices(&live);
        pixels.sort_unstable();
        let expected: Vec<u32> = (0..13 * 9).collect();
        assert_eq!(pixels, expected);
    }

    #[test]
    fn no_live_tile_lists_no_pixel() {
        let live = LiveTiles::from_indices(16, 16, 8, Vec::new());
        assert_eq!(live_pixel_indices(&live), Vec::<u32>::new());
    }
}
