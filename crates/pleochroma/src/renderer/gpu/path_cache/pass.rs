//! Phase B passes of the path cache: slab upload, the chunk loop and the fired-sample completion.
//!
//! This file lives in `renderer/gpu/path_cache/` but is mounted as a child module of
//! `renderer::gpu::frame` (`#[path]` in `frame/mod.rs`): it needs the renderer's private state
//! (device, scene buffers, chunk sizing, poisoning) exactly as `frame::live` does, and it is the
//! cached twin of that module.
//!
//! # One slab, one turn sequence
//!
//! A pass with a cache runs one slab at a time
//! ([`GpuFrameRenderer::accumulate_cached_slab_turn`]). The slab's cached live pixels become a
//! flat list of real pixel indices (the virtual pixels), and the slab is processed in chunks
//! exactly like a live pass: `pcb_phase_b_main` writes one XYZ triple per (pixel, sample) tuple
//! into the transport output buffer, then (when the GPU completes the fired samples) the
//! completion pass traces each fired sample and adds its completion into the sample's own tuple
//! slot, `reduce_xyz_live_main` sums each pixel's samples (and the odd absolute samples into
//! half-buffer B), and the sums are read back and added to the caller's compact sums. Chunk
//! sizing, the turn budget, cancellation and poisoning are the live pass's.
//!
//! A renderer that cannot build or bind the completion pass (see [`Completion`]) skips it, reads
//! the fired list back and leaves the completion to the host (`GpuPathCache::complete_residuals`).
//!
//! # Double buffering
//!
//! After the first chunk of a slab is submitted (the GPU is busy), and before the host waits for
//! it, the next slab of the pass is staged: its words are written into a fresh buffer with
//! `Queue::write_buffer` (wgpu's staging buffer; the copy is ordered before every later
//! submission) while the GPU shades the current slab. The staged slab goes into the resident map,
//! so the next turn sequence finds it uploaded. The system-RAM tier drops a slab when its sequence
//! is done; the video-memory tier keeps it for the next pass.
//!
//! Which slab holds which tiles, and which pixels each chunk covers, are decided before any
//! upload, from tile sizes and the live set only; an upload's timing cannot change a result.

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{OnceLock, atomic::Ordering},
    time::Instant,
};

use glam::Vec3;
use wgpu::BufferUsages;

use super::{
    ChunkCursor, ChunkFrameState, ChunkSlotResources, ChunkTurnOutcome, GpuFrameError,
    GpuPipelineKind, SHADER_SRC, TurnRequest, TurnSetup,
    dispatch::{build_chunk_params, count_u32, workgroups_for},
    live::{GpuReduceLiveParams, LiveSums},
    live_source::{LIVE_REDUCE_ENTRY, LIVE_REDUCE_SHADER_SRC},
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
    tables_source::{megakernel_tables_source, megakernel_uses_tables},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::tables::OPTICS_TABLES_BINDING,
    renderer::{
        gpu::{
            compute,
            path_cache::{
                COMPLETE_ENTRY, COMPLETE_STORAGE_BUFFERS, COMPLETE_UNIFORM_BUFFERS, PHASE_B_ENTRY,
                completion_variant, phase_b_source,
            },
        },
        gpu_backend::HostSlab,
    },
};

/// Binding of the slab words in `phase_b.wgsl`.
pub const BIND_WORDS: u32 = 180;
/// Binding of the tile word offsets.
pub const BIND_TILE_OFFSETS: u32 = 181;
/// Binding of the pass's real pixel indices.
pub const BIND_PIXELS: u32 = 182;
/// Binding of the pass parameters ([`GpuPhaseBParams`]).
pub const BIND_PARAMS: u32 = 183;
/// Binding of the fired-sample list.
pub const BIND_FIRED: u32 = 184;

/// Storage buffers phase B binds at once: material, output, HDR texels, slab words, tile offsets,
/// pixel list and fired list.
const PHASE_B_STORAGE_BUFFERS: u32 = 7;

/// Floats the reduce kernel writes per pixel: the summed XYZ, then the odd-sample XYZ.
const FLOATS_PER_PIXEL: usize = 6;

/// The most fired-sample entries one chunk may need while the host completes them. A chunk's
/// tuples are capped to it while the residual is on, so the list never overflows and the readback
/// and the CPU work per chunk stay bounded. A GPU-completed chunk is capped by the storage
/// binding instead ([`GpuFrameRenderer::fired_entry_limit`]).
const FIRED_CAPACITY_MAX: usize = 1 << 20;

/// Bytes of the fired list's count header (`PcbFired::count`, padded to 16-byte alignment).
const FIRED_HEADER_BYTES: u64 = 16;

/// Bytes of one fired-list entry (`vec4<u32>`).
const FIRED_ENTRY_BYTES: u64 = 16;

/// `PcbParams` of `phase_b.wgsl` (binding [`BIND_PARAMS`]): 32 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuPhaseBParams {
    /// Tile edge in pixels.
    pub(crate) tile: u32,
    /// Tiles per frame row.
    pub(crate) tiles_x: u32,
    /// Frame width in pixels.
    pub(crate) width: u32,
    /// Row-major index of the slab's first tile.
    pub(crate) first_tile: u32,
    /// 1 when samples may be appended to the fired list.
    pub(crate) fire_enabled: u32,
    /// Entries the fired list can hold.
    pub(crate) fired_capacity: u32,
    /// Pads the struct to 32 bytes.
    pub(crate) _pad0: u32,
    /// Pads the struct to 32 bytes.
    pub(crate) _pad1: u32,
}

/// How the cache's tiles tile the frame, as phase B reads them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlabGeometry {
    /// Tile edge in pixels.
    pub tile: u32,
    /// Tiles per frame row.
    pub tiles_x: u32,
    /// Frame width in pixels.
    pub width: u32,
}

/// One slab of a pass: what to shade, what to stage next, and how to treat the slab afterwards.
pub struct CachedSlabJob<'a, 'f> {
    /// The id of the cache the slab belongs to.
    pub cache_id: u64,
    /// Keep the slab on the GPU after its turns (the video-memory tier).
    pub resident: bool,
    /// The slab to shade.
    pub slab: &'a HostSlab<'f>,
    /// The slab to stage while this one shades, if any.
    pub next: Option<&'a HostSlab<'f>>,
    /// The tile geometry of the cache.
    pub geometry: SlabGeometry,
    /// Whether the kernel appends the samples whose residual is completed (on the GPU, or on the
    /// host where the GPU cannot).
    pub fire: bool,
    /// Sleep before an upload (a test hook: the output must not depend on upload timing).
    pub stage_delay: std::time::Duration,
    /// Complete the fired samples on the host even where the GPU could
    /// (`GpuPathCache::with_cpu_completion`, a test and diagnostics hook).
    pub cpu_completion: bool,
}

/// What a slab's turns carry from one turn to the next.
#[derive(Default)]
pub struct CachedSlabProgress {
    /// The slab's pixel list on the GPU, uploaded on the first turn.
    list: Option<wgpu::Buffer>,
    /// Whether the next slab has been staged.
    next_staged: bool,
    /// The fired samples read back for the host completion: `[virtual pixel, absolute sample,
    /// position, tuple]`; empty when the GPU completes them.
    pub(crate) fired: Vec<[u32; 4]>,
    /// Whether the GPU completes this slab's fired samples, decided on the slab's first turn.
    completed_on_gpu: bool,
}

/// A slab on the GPU.
struct GpuSlab {
    words: wgpu::Buffer,
    tile_offsets: wgpu::Buffer,
}

/// How the fired samples of one material class complete.
enum Completion {
    /// On the GPU, by the completion variant of the class's megakernel.
    Gpu(wgpu::ComputePipeline),
    /// On the host (`GpuPathCache::complete_residuals`): the variant does not build, or the device
    /// cannot bind it.
    Host,
}

impl Completion {
    /// Whether the GPU completes.
    const fn on_gpu(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }
}

/// The completion unit `class` compiles, built once per megakernel base.
///
/// The completion variant of the shipped unit, or of its optics-table variant when the class
/// uses the tables. `None` when an anchor of that base drifted.
fn completion_source_for(class: u32) -> Option<&'static str> {
    static SOURCES: [OnceLock<Option<String>>; 2] = [const { OnceLock::new() }; 2];
    let tables = megakernel_uses_tables(class);
    SOURCES[usize::from(tables)]
        .get_or_init(|| {
            let base = if tables {
                megakernel_tables_source()?
            } else {
                SHADER_SRC
            };
            completion_variant(base)
        })
        .as_deref()
}

/// What the completion pass binds besides the scene buffers.
struct CompletionBindings<'b> {
    /// The packed planes the planes binding holds (bound at the byte size the live pass uses).
    planes_len: usize,
    /// The chunk's transport parameters (binding 1).
    params: &'b wgpu::Buffer,
    /// The chunk's outputs. `out_xyz` holds phase B's values and takes the completions. The
    /// chunk's parameters turn off the debug-buffer writes of `transport_finalize_ray`, so the
    /// other four buffers (production placeholders) are bound but never written.
    outputs: &'b TransportOutputs,
    /// The slab (bindings 180 and 181).
    slab: &'b GpuSlab,
    /// The slab's real pixel indices (binding 182).
    pixels: &'b wgpu::Buffer,
    /// Phase B's pass parameters (binding 183).
    phase_params: &'b wgpu::Buffer,
    /// The fired list (binding 184).
    fired: &'b wgpu::Buffer,
    /// The optics table buffer of a table class (binding 170).
    tables: Option<&'b wgpu::Buffer>,
}

/// The bind group of the completion pipeline: every megakernel binding as the live pass binds it
/// (without the live pixel list), the table buffer of a table class, and phase B's five bindings.
fn completion_bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    frame_buffers: &FrameSceneBuffers,
    bind: &CompletionBindings<'_>,
) -> wgpu::BindGroup {
    let planes_bytes = (bind.planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (bind.planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    let mut entries = vec![
        (0, &frame_buffers.camera, None),
        (1, bind.params, None),
        (2, &frame_buffers.material, None),
        (3, &frame_buffers.planes, Some(planes_bytes)),
        (4, bind.outputs.xyz(), None),
        (5, bind.outputs.radiance(), None),
        (6, bind.outputs.lambdas(), None),
        (7, bind.outputs.path_pdf(), None),
        (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
        (9, bind.outputs.compat(), None),
        (10, &frame_buffers.hdr_env.texels, None),
        (11, &frame_buffers.hdr_env.dims, None),
        (12, &frame_buffers.hdr_env.dist_func, None),
        (13, &frame_buffers.hdr_env.dist_cdf, None),
        (14, &frame_buffers.hdr_env.dist_dims, None),
        (BIND_WORDS, &bind.slab.words, None),
        (BIND_TILE_OFFSETS, &bind.slab.tile_offsets, None),
        (BIND_PIXELS, bind.pixels, None),
        (BIND_PARAMS, bind.phase_params, None),
        (BIND_FIRED, bind.fired, None),
    ];
    if let Some(tables) = bind.tables {
        entries.push((OPTICS_TABLES_BINDING, tables, None));
    }
    compute::bind_buffers_sized(
        device,
        "path cache completion bind group",
        pipeline,
        &entries,
    )
}

/// What a [`GpuFrameRenderer`] holds for cached passes. All empty until the first one.
#[derive(Default)]
pub(super) struct CachedState {
    /// The completion decision and pipeline by class value, made on the first fired slab of the
    /// class. Looked up, never iterated.
    completions: HashMap<u32, Completion>,
    /// The phase B pipelines by class value, built lazily. Looked up, never iterated.
    pipelines: HashMap<u32, wgpu::ComputePipeline>,
    /// `reduce_xyz_live_main`.
    reduce: Option<wgpu::ComputePipeline>,
    /// The persistent [`GpuReduceLiveParams`] uniform.
    reduce_params: Option<wgpu::Buffer>,
    /// The persistent [`GpuPhaseBParams`] uniform.
    phase_params: Option<wgpu::Buffer>,
    /// The reduced output of the chunk in flight and the pixels it holds.
    out: Option<(wgpu::Buffer, usize)>,
    /// The fired-sample list and the entries it holds.
    fired: Option<(wgpu::Buffer, usize)>,
    /// Uploaded slabs by (cache id, slab index). Looked up and filtered, never iterated for a
    /// decision.
    resident: HashMap<(u64, usize), GpuSlab>,
}

/// One chunk submitted and not yet read.
struct SubmittedChunk {
    staging: wgpu::Buffer,
    copy_index: wgpu::SubmissionIndex,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// Uploads a slab: its tiles back to back in one storage buffer, and the table of tile offsets.
fn upload_slab(device: &wgpu::Device, queue: &wgpu::Queue, host: &HostSlab<'_>) -> GpuSlab {
    let words = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("path cache slab words"),
        size: host.bytes().max(4),
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut offset = 0_u64;
    for tile in &host.tiles {
        let bytes: &[u8] = bytemuck::cast_slice(tile.as_words());
        queue.write_buffer(&words, offset, bytes);
        offset += bytes.len() as u64;
    }
    let tile_offsets = compute::upload(
        device,
        "path cache slab tile offsets",
        &host.tile_offsets,
        BufferUsages::STORAGE,
    );
    GpuSlab {
        words,
        tile_offsets,
    }
}

impl GpuFrameRenderer {
    /// Drops every slab of the cache `cache_id` from the GPU.
    pub(crate) fn release_cached_slabs(&mut self, cache_id: u64) {
        self.live
            .cached
            .resident
            .retain(|(id, _), _| *id != cache_id);
    }

    /// The largest slab, in bytes, this device can bind.
    #[must_use]
    pub(crate) fn storage_binding_limit(&self) -> u64 {
        let limits = self.ctx.device.limits();
        limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size)
    }

    /// Slabs of any cache currently held on the GPU.
    #[cfg(test)]
    pub(crate) fn cached_slab_count(&self) -> usize {
        self.live.cached.resident.len()
    }

    /// Runs one fairness turn of the slab `job`: at most `request.max_chunks` chunks starting at
    /// `cursor` (counted in virtual pixels of the slab), nothing left in flight when it returns,
    /// and the renderer poisoned on a device loss -- the cached twin of
    /// [`Self::accumulate_live_turn`].
    ///
    /// `sums` are compact: entry `v` belongs to the real pixel `job.slab.pixels[v]`.
    ///
    /// # Errors
    ///
    /// The conditions of [`Self::accumulate`], plus [`GpuFrameError::LivePassUnavailable`] for the
    /// legacy wavefront kernels, a slab the device cannot bind, or sums of the wrong length, and
    /// [`GpuFrameError::DeviceLimits`] for a device with too few storage buffers.
    pub(crate) fn accumulate_cached_slab_turn(
        &mut self,
        request: &TurnRequest<'_>,
        job: &CachedSlabJob<'_, '_>,
        progress: &mut CachedSlabProgress,
        sums: &mut LiveSums<'_>,
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
        self.turn_in_progress = true;
        let result = self.accumulate_cached_slab_body(request, job, progress, sums, cursor);
        self.turn_in_progress = false;
        if matches!(&result, Err(GpuFrameError::DeviceLost(_))) {
            self.abandon_in_flight();
        }
        result
    }

    /// The body [`Self::accumulate_cached_slab_turn`] wraps with the poisoning checks.
    fn accumulate_cached_slab_body(
        &mut self,
        request: &TurnRequest<'_>,
        job: &CachedSlabJob<'_, '_>,
        progress: &mut CachedSlabProgress,
        sums: &mut LiveSums<'_>,
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        let scene = request.scene;
        let count = job.slab.pixels.len();
        if sums.sum.len() != count || sums.half_b.len() != count {
            return Err(GpuFrameError::LivePassUnavailable(
                "the cached sums must have one entry per cached pixel of the slab".to_string(),
            ));
        }
        if request.spp == 0 || count == 0 {
            return Ok(ChunkTurnOutcome::Done);
        }
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }
        if self.pipeline_kind != GpuPipelineKind::Megakernel {
            return Err(GpuFrameError::LivePassUnavailable(
                "cached passes run on the megakernel, not the legacy wavefront".to_string(),
            ));
        }
        let limits = self.ctx.device.limits();
        if limits.max_storage_buffers_per_shader_stage < PHASE_B_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: PHASE_B_STORAGE_BUFFERS,
                available: limits.max_storage_buffers_per_shader_stage,
            });
        }
        let slab_bytes = job.slab.bytes();
        if slab_bytes > limits.max_storage_buffer_binding_size
            || slab_bytes > limits.max_buffer_size
        {
            return Err(GpuFrameError::LivePassUnavailable(format!(
                "a path cache slab of {slab_bytes} bytes does not fit one storage binding of this \
                 device"
            )));
        }

        let TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        } = self.prepare_turn(request)?;
        self.ensure_cached_pipelines(state.pipeline_class);
        progress.completed_on_gpu =
            job.fire && !job.cpu_completion && self.ensure_completion(state.pipeline_class);
        self.ensure_cached_buffers(byte_budget_pixels.min(count));
        self.ensure_slab_resident(job);
        if progress.list.is_none() {
            let upload: &[u32] = &job.slab.pixels;
            progress.list = Some(compute::upload(
                &self.ctx.device,
                "path cache slab pixel list",
                upload,
                BufferUsages::STORAGE,
            ));
        }

        let cancelled = self.run_cached_chunks(
            request,
            (&state, &frame_buffers, byte_budget_pixels),
            job,
            progress,
            (&mut *sums, &mut *cursor),
        )?;
        self.scene_buffers = Some(frame_buffers);
        self.check_uncaptured_error()?;

        if cancelled {
            return Ok(ChunkTurnOutcome::Cancelled);
        }
        if cursor.first_pixel >= count {
            if !job.resident {
                self.live
                    .cached
                    .resident
                    .remove(&(job.cache_id, job.slab.index));
            }
            return Ok(ChunkTurnOutcome::Done);
        }
        Ok(ChunkTurnOutcome::MoreWork)
    }

    /// Builds the phase B pipeline of `class` and the reduce pipeline, if not built yet.
    fn ensure_cached_pipelines(&mut self, class: u32) {
        let info = self.ctx.adapter.get_info();
        if self.live.cached.reduce.is_none() {
            self.live.cached.reduce = Some(compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                LIVE_REDUCE_ENTRY,
                LIVE_REDUCE_SHADER_SRC,
                LIVE_REDUCE_ENTRY,
                &[],
            ));
        }
        if !self.live.cached.pipelines.contains_key(&class) {
            let label = format!(
                "path cache phase B (MATERIAL_CLASS={}, features {:#x})",
                class & super::material_class::KIND_MASK,
                class & super::material_class::FEATURE_MASK
            );
            let pipeline = compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                &label,
                phase_b_source(),
                PHASE_B_ENTRY,
                &super::material_class::pipeline_constants(class),
            );
            self.live.cached.pipelines.insert(class, pipeline);
        }
    }

    /// Creates the uniforms and grows the reduced-output buffer to `pixels` pixels.
    fn ensure_cached_buffers(&mut self, pixels: usize) {
        let device = &self.ctx.device;
        let cached = &mut self.live.cached;
        if cached.reduce_params.is_none() {
            cached.reduce_params = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("path cache reduce params"),
                size: size_of::<GpuReduceLiveParams>() as wgpu::BufferAddress,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if cached.phase_params.is_none() {
            cached.phase_params = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("path cache phase B params"),
                size: size_of::<GpuPhaseBParams>() as wgpu::BufferAddress,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if cached.out.as_ref().is_none_or(|(_, cap)| *cap < pixels) {
            cached.out = Some((
                compute::zeroed_buffer::<f32>(
                    device,
                    "path cache reduced output",
                    pixels * FLOATS_PER_PIXEL,
                    BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                ),
                pixels,
            ));
        }
    }

    /// Grows the fired-sample list to `entries` entries (at least one: a storage binding cannot be
    /// empty).
    fn ensure_fired_capacity(&mut self, entries: usize) {
        let wanted = entries.max(1);
        if self
            .live
            .cached
            .fired
            .as_ref()
            .is_some_and(|(_, cap)| *cap >= wanted)
        {
            return;
        }
        let bytes = FIRED_HEADER_BYTES + FIRED_ENTRY_BYTES * wanted as wgpu::BufferAddress;
        self.live.cached.fired = Some((
            self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("path cache fired samples"),
                size: bytes,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            wanted,
        ));
    }

    /// Makes sure the slab of `job` is on the GPU, dropping the slabs of every other cache.
    fn ensure_slab_resident(&mut self, job: &CachedSlabJob<'_, '_>) {
        let cached = &mut self.live.cached;
        cached.resident.retain(|(id, _), _| *id == job.cache_id);
        let key = (job.cache_id, job.slab.index);
        if let Entry::Vacant(slot) = cached.resident.entry(key) {
            if !job.stage_delay.is_zero() {
                std::thread::sleep(job.stage_delay);
            }
            slot.insert(upload_slab(&self.ctx.device, &self.ctx.queue, job.slab));
        }
    }

    /// Stages the next slab of the pass while the GPU is busy with the current one.
    fn stage_next_slab(&mut self, job: &CachedSlabJob<'_, '_>, progress: &mut CachedSlabProgress) {
        if progress.next_staged {
            return;
        }
        progress.next_staged = true;
        let Some(next) = job.next else {
            return;
        };
        let key = (job.cache_id, next.index);
        if self.live.cached.resident.contains_key(&key) {
            return;
        }
        if !job.stage_delay.is_zero() {
            std::thread::sleep(job.stage_delay);
        }
        let slab = upload_slab(&self.ctx.device, &self.ctx.queue, next);
        self.live.cached.resident.insert(key, slab);
    }

    /// The chunk loop of one turn; returns whether `request.cancel` fired.
    fn run_cached_chunks(
        &mut self,
        request: &TurnRequest<'_>,
        (state, frame_buffers, byte_budget_pixels): (
            &ChunkFrameState<'_>,
            &FrameSceneBuffers,
            usize,
        ),
        job: &CachedSlabJob<'_, '_>,
        progress: &mut CachedSlabProgress,
        (sums, cursor): (&mut LiveSums<'_>, &mut ChunkCursor),
    ) -> Result<bool, GpuFrameError> {
        let count = job.slab.pixels.len();
        let mut first = cursor.first_pixel;
        let mut chunk_index = cursor.chunk_index;
        let mut chunks_this_turn = 0_usize;
        let mut cancelled = false;
        while first < count {
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
            let mut chunk_pixels = Self::next_chunk_pixels(
                self.ns_per_tuple_ema,
                self.growth_limited_pixels(state.spp, byte_budget_pixels),
                state.spp,
                count,
            );
            if job.fire {
                let entries = if progress.completed_on_gpu {
                    self.fired_entry_limit()
                } else {
                    FIRED_CAPACITY_MAX
                };
                chunk_pixels = chunk_pixels.min((entries / state.spp as usize).max(1));
            }
            let pixels_this_chunk = chunk_pixels.min(count - first);
            let started = Instant::now();
            let submitted = self.submit_cached_chunk(
                state,
                frame_buffers,
                job,
                progress,
                (first, pixels_this_chunk),
            )?;
            // The GPU is busy with this chunk: stage the next slab now.
            self.stage_next_slab(job, progress);
            self.finish_cached_chunk(submitted, job, progress, (first, pixels_this_chunk), sums)?;
            // A chunk cut short by the end of the slab says nothing about the next slab's size.
            if pixels_this_chunk == chunk_pixels {
                self.record_chunk_timing(started, pixels_this_chunk * state.spp as usize);
            }
            first += pixels_this_chunk;
            chunk_index += 1;
            chunks_this_turn += 1;
        }
        cursor.first_pixel = first;
        cursor.chunk_index = chunk_index;
        Ok(cancelled)
    }

    /// Writes phase B's pass parameters and resets the fired list's count for a chunk of `tuples`
    /// tuples.
    fn write_phase_params(
        queue: &wgpu::Queue,
        (phase_params, fired): (&wgpu::Buffer, &wgpu::Buffer),
        job: &CachedSlabJob<'_, '_>,
        (tuples, fired_capacity): (usize, usize),
    ) {
        queue.write_buffer(
            phase_params,
            0,
            bytemuck::bytes_of(&GpuPhaseBParams {
                tile: job.geometry.tile,
                tiles_x: job.geometry.tiles_x,
                width: job.geometry.width,
                first_tile: job.slab.span.first_tile,
                fire_enabled: u32::from(job.fire),
                fired_capacity: count_u32(tuples.min(fired_capacity)),
                _pad0: 0,
                _pad1: 0,
            }),
        );
        if job.fire {
            queue.write_buffer(fired, 0, bytemuck::bytes_of(&0_u32));
        }
    }

    /// The completion decision for `class`: `None` before a fired slab of the class asked.
    #[cfg(test)]
    pub(crate) fn completes_on_gpu(&self, class: u32) -> Option<bool> {
        self.live
            .cached
            .completions
            .get(&class)
            .map(Completion::on_gpu)
    }

    /// Whether the GPU completes the fired samples of `class`. The completion pipeline is built
    /// the first time the class asks.
    ///
    /// Decided once per renderer and class: the completion variant of the class's megakernel base
    /// must build ([`completion_source_for`]) and the device must bind its storage and uniform
    /// buffers. Otherwise the fired samples of the class complete on the host, logged once.
    fn ensure_completion(&mut self, class: u32) -> bool {
        if let Some(completion) = self.live.cached.completions.get(&class) {
            return completion.on_gpu();
        }
        let limits = self.ctx.device.limits();
        let storage = COMPLETE_STORAGE_BUFFERS + u32::from(megakernel_uses_tables(class));
        let bindings = storage + COMPLETE_UNIFORM_BUFFERS;
        let fits = limits.max_storage_buffers_per_shader_stage >= storage
            && limits.max_buffers_and_acceleration_structures_per_shader_stage >= bindings;
        let completion = match completion_source_for(class) {
            Some(source) if fits => {
                let label = format!(
                    "path cache completion (MATERIAL_CLASS={}, features {:#x})",
                    class & super::material_class::KIND_MASK,
                    class & super::material_class::FEATURE_MASK
                );
                Completion::Gpu(compute::create_persisted_compute_pipeline(
                    &self.ctx.device,
                    &self.ctx.adapter.get_info(),
                    &label,
                    source,
                    COMPLETE_ENTRY,
                    &super::material_class::pipeline_constants(class),
                ))
            }
            Some(_) => {
                tracing::info!(
                    storage,
                    available = limits.max_storage_buffers_per_shader_stage,
                    "path cache: this GPU cannot bind the completion pass, so the fired samples \
                     complete on the CPU"
                );
                Completion::Host
            }
            None => {
                tracing::warn!(
                    "path cache: the completion variant no longer matches the megakernel's \
                     anchors, so the fired samples complete on the CPU"
                );
                Completion::Host
            }
        };
        let on_gpu = completion.on_gpu();
        tracing::debug!(
            class,
            on_gpu,
            "path cache: where the fired samples complete"
        );
        self.live.cached.completions.insert(class, completion);
        on_gpu
    }

    /// The most fired-list entries one storage binding of this device holds.
    fn fired_entry_limit(&self) -> usize {
        usize::try_from(
            self.storage_binding_limit()
                .saturating_sub(FIRED_HEADER_BYTES)
                / FIRED_ENTRY_BYTES,
        )
        .unwrap_or(usize::MAX)
    }

    /// Dispatches the completion of the chunk's fired samples right behind phase B and before the
    /// reduce, without waiting.
    ///
    /// One invocation runs per fired entry; the dispatch covers `tuples`, an upper bound. Each
    /// invocation adds its sample's completion into the sample's tuple slot of `outputs.xyz()`.
    fn submit_completion(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        (job, progress): (&CachedSlabJob<'_, '_>, &CachedSlabProgress),
        (outputs, params_buf, tuples): (&TransportOutputs, &wgpu::Buffer, usize),
    ) -> Result<(), GpuFrameError> {
        let cached = &self.live.cached;
        let (
            Some(Completion::Gpu(pipeline)),
            Some(phase_params),
            Some((fired, _)),
            Some(slab),
            Some(pixels),
        ) = (
            cached.completions.get(&state.pipeline_class),
            cached.phase_params.as_ref(),
            cached.fired.as_ref(),
            cached.resident.get(&(job.cache_id, job.slab.index)),
            progress.list.as_ref(),
        )
        else {
            return Err(GpuFrameError::LivePassUnavailable(
                "the path cache completion was not prepared".to_string(),
            ));
        };
        let bind_group = completion_bind_group(
            &self.ctx.device,
            pipeline,
            frame_buffers,
            &CompletionBindings {
                planes_len: state.packed_len,
                params: params_buf,
                outputs,
                slab,
                pixels,
                phase_params,
                fired,
                tables: self.optics_tables_for(state.pipeline_class),
            },
        );
        let _ = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            pipeline,
            &bind_group,
            (workgroups_for(tuples), 1, 1),
        );
        self.check_device_error()
    }

    /// Submits phase B, the completion, the reduce and the readback copy of the virtual pixels
    /// `first..first + pixels_this_chunk`; nothing is waited for.
    fn submit_cached_chunk(
        &mut self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        job: &CachedSlabJob<'_, '_>,
        progress: &CachedSlabProgress,
        (first, pixels_this_chunk): (usize, usize),
    ) -> Result<SubmittedChunk, GpuFrameError> {
        self.check_device_error()?;
        let tuples = pixels_this_chunk * state.spp as usize;
        if job.fire {
            self.ensure_fired_capacity(tuples);
        } else {
            self.ensure_fired_capacity(1);
        }
        // The cache's records and its CPU twin (the attributed entry) are branching paths, so
        // phase B and the completion always run the branching estimator.
        let params = build_chunk_params(state, first, pixels_this_chunk)
            .with_estimator(crate::optics::raytracer::Estimator::Branching, false);
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let ChunkSlotResources {
            outputs,
            params_buf,
            ..
        } = self.resolve_chunk_slot_resources(0, tuples, pixels_this_chunk);
        let cached = &self.live.cached;
        let (
            Some(pipeline),
            Some(reduce),
            Some(reduce_params),
            Some(phase_params),
            Some((out, _)),
            Some((fired, fired_capacity)),
            Some(slab),
            Some(list),
        ) = (
            cached.pipelines.get(&state.pipeline_class),
            cached.reduce.as_ref(),
            cached.reduce_params.as_ref(),
            cached.phase_params.as_ref(),
            cached.out.as_ref(),
            cached.fired.as_ref(),
            cached.resident.get(&(job.cache_id, job.slab.index)),
            progress.list.as_ref(),
        )
        else {
            return Err(GpuFrameError::LivePassUnavailable(
                "the path cache pipelines or buffers were not prepared".to_string(),
            ));
        };

        queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
        Self::write_phase_params(queue, (phase_params, fired), job, (tuples, *fired_capacity));
        let bind_group = compute::bind_buffers_sized(
            device,
            "path cache phase B bind group",
            pipeline,
            &[
                (0, &frame_buffers.camera, None),
                (1, params_buf, None),
                (2, &frame_buffers.material, None),
                (4, outputs.xyz(), None),
                (10, &frame_buffers.hdr_env.texels, None),
                (11, &frame_buffers.hdr_env.dims, None),
                (BIND_WORDS, &slab.words, None),
                (BIND_TILE_OFFSETS, &slab.tile_offsets, None),
                (BIND_PIXELS, list, None),
                (BIND_PARAMS, phase_params, None),
                (BIND_FIRED, fired, None),
            ],
        );
        let _ = compute::dispatch(
            device,
            queue,
            pipeline,
            &bind_group,
            (workgroups_for(tuples), 1, 1),
        );
        self.check_device_error()?;

        if progress.completed_on_gpu {
            self.submit_completion(
                state,
                frame_buffers,
                (job, progress),
                (outputs, params_buf, tuples),
            )?;
        }

        self.submit_cached_reduce(
            state,
            (reduce, reduce_params),
            (outputs.xyz(), out),
            pixels_this_chunk,
        )
    }

    /// The second half of [`Self::submit_cached_chunk`]: submits the reduce of the phase B output
    /// `xyz` into the reduced output `out`, and the readback copy of `out`; nothing is waited for.
    fn submit_cached_reduce(
        &self,
        state: &ChunkFrameState<'_>,
        (reduce, reduce_params): (&wgpu::ComputePipeline, &wgpu::Buffer),
        (xyz, out): (&wgpu::Buffer, &wgpu::Buffer),
        pixels_this_chunk: usize,
    ) -> Result<SubmittedChunk, GpuFrameError> {
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
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
            "path cache reduce bind group",
            reduce,
            &[(0, reduce_params), (1, xyz), (2, out)],
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
            out,
            pixels_this_chunk * FLOATS_PER_PIXEL,
            "path cache readback staging",
        );
        let rx = compute::begin_map_read(&staging);
        Ok(SubmittedChunk {
            staging,
            copy_index,
            rx,
        })
    }

    /// Waits for a submitted chunk, adds its pixels to `sums`, and collects its fired samples.
    fn finish_cached_chunk(
        &self,
        submitted: SubmittedChunk,
        job: &CachedSlabJob<'_, '_>,
        progress: &mut CachedSlabProgress,
        (first, pixels_this_chunk): (usize, usize),
        sums: &mut LiveSums<'_>,
    ) -> Result<(), GpuFrameError> {
        let SubmittedChunk {
            staging,
            copy_index,
            rx,
        } = submitted;
        let device = &self.ctx.device;
        let lost = |e: compute::ComputeWaitError| {
            GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string()))
        };
        let data =
            compute::finish_map_read::<f32>(device, &staging, copy_index, &rx).map_err(lost)?;
        for (local, floats) in data
            .as_chunks::<FLOATS_PER_PIXEL>()
            .0
            .iter()
            .take(pixels_this_chunk)
            .enumerate()
        {
            let virtual_pixel = first + local;
            sums.sum[virtual_pixel] += Vec3::new(floats[0], floats[1], floats[2]);
            sums.half_b[virtual_pixel] += Vec3::new(floats[3], floats[4], floats[5]);
        }
        if job.fire && !progress.completed_on_gpu {
            self.read_fired(progress)?;
        }
        self.check_device_error()
    }

    /// Reads the fired list of the chunk just finished into `progress.fired`.
    fn read_fired(&self, progress: &mut CachedSlabProgress) -> Result<(), GpuFrameError> {
        let Some((fired, capacity)) = self.live.cached.fired.as_ref() else {
            return Ok(());
        };
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let lost = |e: compute::ComputeWaitError| {
            GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string()))
        };
        let (staging, index) =
            compute::copy_to_staging::<u32>(device, queue, fired, 1, "path cache fired count");
        let rx = compute::begin_map_read(&staging);
        let counts = compute::finish_map_read::<u32>(device, &staging, index, &rx).map_err(lost)?;
        let fired_count = counts.first().copied().unwrap_or(0) as usize;
        if fired_count == 0 {
            return Ok(());
        }
        if fired_count > *capacity {
            return Err(GpuFrameError::LivePassUnavailable(format!(
                "{fired_count} fired samples overflow a list of {capacity}"
            )));
        }
        let (staging, index) = compute::copy_to_staging::<u32>(
            device,
            queue,
            fired,
            4 + 4 * fired_count,
            "path cache fired samples readback",
        );
        let rx = compute::begin_map_read(&staging);
        let words = compute::finish_map_read::<u32>(device, &staging, index, &rx).map_err(lost)?;
        let (entries, _) = words.get(4..).unwrap_or(&[]).as_chunks::<4>();
        progress.fired.extend_from_slice(entries);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    use super::{
        BIND_FIRED, BIND_PARAMS, BIND_PIXELS, BIND_TILE_OFFSETS, BIND_WORDS, GpuPhaseBParams,
        completion_source_for,
    };
    use crate::{
        optics::tables::OPTICS_TABLES_BINDING,
        renderer::gpu::path_cache::{
            COMPLETE_BINDINGS, COMPLETE_ENTRY, COMPLETE_STORAGE_BUFFERS, COMPLETE_UNIFORM_BUFFERS,
            PHASE_B_BINDINGS, completion_variant,
        },
    };

    /// The bindings an entry point reaches, sorted, and its storage and uniform global counts.
    fn reached(source: &str, entry: &str) -> (Vec<u32>, u32, u32) {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("unit does not parse: {}", e.emit_to_string(source)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("unit does not validate: {}", e.emit_to_string(source)));
        let index = module
            .entry_points
            .iter()
            .position(|e| e.name == entry)
            .expect("entry point exists");
        let uses = info.get_entry_point(index);
        let mut bindings = Vec::new();
        let (mut storage, mut uniform) = (0_u32, 0_u32);
        for (handle, var) in module.global_variables.iter() {
            if uses[handle].is_empty() {
                continue;
            }
            match var.space {
                naga::AddressSpace::Storage { .. } => storage += 1,
                naga::AddressSpace::Uniform => uniform += 1,
                _ => {}
            }
            if let Some(binding) = var.binding.as_ref() {
                bindings.push(binding.binding);
            }
        }
        bindings.sort_unstable();
        (bindings, storage, uniform)
    }

    #[test]
    fn the_completion_unit_of_a_table_class_validates_and_binds_the_tables() {
        let base = super::super::tables_source::megakernel_tables_source()
            .expect("the shipped unit has a table variant");
        let unit = completion_variant(base).expect("the table base builds");
        let (bindings, storage, uniform) = reached(&unit, COMPLETE_ENTRY);
        let mut expected = COMPLETE_BINDINGS.to_vec();
        expected.push(OPTICS_TABLES_BINDING);
        expected.sort_unstable();
        assert_eq!(bindings, expected);
        assert_eq!(storage, COMPLETE_STORAGE_BUFFERS + 1);
        assert_eq!(uniform, COMPLETE_UNIFORM_BUFFERS);
    }

    #[test]
    fn the_completion_source_of_a_class_follows_its_table_bit() {
        use super::super::material_class;
        let plain = completion_source_for(material_class::ISOTROPIC).expect("plain base builds");
        let tables =
            completion_source_for(material_class::ISOTROPIC | material_class::OPTICS_TABLES)
                .expect("table base builds");
        assert!(!plain.contains("fn ot_reach()"));
        assert!(tables.contains("fn ot_reach()"));
        assert!(plain.contains("fn pcc_finalize("));
        assert!(tables.contains("fn pcc_finalize("));
    }

    #[test]
    fn the_own_bindings_are_the_ones_the_kernel_declares() {
        for binding in [
            BIND_WORDS,
            BIND_TILE_OFFSETS,
            BIND_PIXELS,
            BIND_PARAMS,
            BIND_FIRED,
        ] {
            assert!(PHASE_B_BINDINGS.contains(&binding));
        }
        assert_eq!(
            [
                BIND_WORDS,
                BIND_TILE_OFFSETS,
                BIND_PIXELS,
                BIND_PARAMS,
                BIND_FIRED
            ],
            [180, 181, 182, 183, 184]
        );
    }

    #[test]
    fn the_pass_parameters_are_32_bytes() {
        assert_eq!(size_of::<GpuPhaseBParams>(), 32);
    }
}
