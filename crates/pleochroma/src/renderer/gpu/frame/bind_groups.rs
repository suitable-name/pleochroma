//! Pipeline compilation and bind-group construction.
//!
//! This covers the megakernel's own bind group
//! ([`build_bind_group`]/[`build_chunk_bind_group`]) plus the five wavefront pipelines
//! ([`WavefrontPipelines`]) and their four per-round bind groups
//! ([`WavefrontRoundBindGroups`]).

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    optics::tables::OPTICS_TABLES_BINDING,
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial, GpuTransportParams},
        env_map_gpu::HdrEnvGpuData,
        gpu::{GpuContext, compute},
    },
};

use super::{
    WAVEFRONT_SHADER_SRC,
    scene_buffers::{FrameSceneBuffers, TransportOutputs, WavefrontRayBuffers},
};

/// The material, scene and output inputs shared by [`build_bind_group`] and its dispatch callers
/// ([`super::encode_and_dispatch`], and the wasm32 async dispatch).
///
/// A struct only to stay under clippy's `too_many_arguments` limit. `total_tuples` stays a separate
/// parameter of the dispatch functions: it sets the workgroup count and can be smaller than
/// `outputs.capacity` for a partial chunk.
pub struct TransportDispatchArgs<'a> {
    pub(crate) ctx: &'a GpuContext,
    pub(crate) pipeline: &'a wgpu::ComputePipeline,
    pub(crate) camera_params: &'a GpuCameraParams,
    pub(crate) params: &'a GpuTransportParams,
    pub(crate) material: &'a GpuGemMaterial,
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) facet_finishes: &'a [u32],
    pub(crate) outputs: &'a TransportOutputs,
    /// Bindings 10-14 (`texels`, `dims`, `dist_func`, `dist_cdf`, `dist_dims`) are always
    /// bound, even for a non-HDR dispatch (pass [`HdrEnvGpuData::dummy`]), because the
    /// megakernel's bind group layout always includes them.
    pub(crate) hdr_env: &'a HdrEnvGpuData,
    /// Optional tray buffer (the spectral tray map and the mirror words), present only for the
    /// tray variant.
    pub(crate) tray_shadow: Option<&'a wgpu::Buffer>,
}

/// Uploads the scene inputs and binds every buffer, WITHOUT dispatching.
///
/// Used by the self-test path ([`super::encode_and_dispatch`]) and by the wasm32 async
/// dispatch. Native's pipelined production path (`GpuFrameRenderer::dispatch_chunk`) does not
/// call this, since it would re-upload the scene every chunk; [`FrameSceneBuffers`] and
/// [`build_chunk_bind_group`] avoid that. The local upload buffers may be dropped on return:
/// `wgpu` keeps a resource alive while any unfinished submission references it.
pub fn build_bind_group(args: &TransportDispatchArgs<'_>) -> wgpu::BindGroup {
    let camera_buf = compute::upload(
        &args.ctx.device,
        "transport camera",
        std::slice::from_ref(args.camera_params),
        BufferUsages::UNIFORM,
    );
    let params_buf = compute::upload(
        &args.ctx.device,
        "transport params",
        std::slice::from_ref(args.params),
        BufferUsages::UNIFORM,
    );
    let material_buf = compute::upload(
        &args.ctx.device,
        "transport material",
        std::slice::from_ref(args.material),
        BufferUsages::STORAGE,
    );
    let planes_buf = compute::upload(
        &args.ctx.device,
        "transport planes",
        args.planes,
        BufferUsages::STORAGE,
    );
    // A SEPARATE storage buffer, parallel to `planes_buf`, never merged into
    // `GpuFacetPlane` itself -- see `renderer::buffers::facet_finish`'s module doc
    // comment for why.
    let facet_finishes_buf = compute::upload(
        &args.ctx.device,
        "transport facet finishes",
        args.facet_finishes,
        BufferUsages::STORAGE,
    );

    let mut entries = vec![
        (0, &camera_buf),
        (1, &params_buf),
        (2, &material_buf),
        (3, &planes_buf),
        (4, args.outputs.xyz()),
        (5, args.outputs.radiance()),
        (6, args.outputs.lambdas()),
        (7, args.outputs.path_pdf()),
        (8, &facet_finishes_buf),
        (9, args.outputs.compat()),
        (10, &args.hdr_env.texels),
        (11, &args.hdr_env.dims),
        (12, &args.hdr_env.dist_func),
        (13, &args.hdr_env.dist_cdf),
        (14, &args.hdr_env.dist_dims),
    ];
    if let Some(buffer) = args.tray_shadow {
        entries.push((
            crate::optics::raytracer::environment::tray_shadow::TRAY_SHADOW_BINDING,
            buffer,
        ));
    }
    compute::bind_buffers(
        &args.ctx.device,
        "transport bind group",
        args.pipeline,
        &entries,
    )
}

/// Like [`build_bind_group`], for [`super::GpuFrameRenderer::dispatch_chunk`]'s pipelined path.
///
/// The four scene buffers and `params_buf` are persistent (see [`FrameSceneBuffers`] and
/// [`super::GpuFrameRenderer::params_buffers`]) and already written by the caller, so this
/// function only builds the bind group.
///
/// `planes_len` is the current scene's plane count, which can be smaller than the allocated
/// capacity of `frame_buffers.planes`. `planes` and `facet_finishes` therefore bind an explicit
/// byte range via [`compute::bind_buffers_sized`]. Otherwise `arrayLength(&planes)` in the
/// shader would see the stale larger size.
///
/// `tables` is the optics lookup table buffer, bound at `OPTICS_TABLES_BINDING` for a pipeline
/// built from the table variant (`None` for the shipped kernel, whose bind group is then exactly
/// the fifteen scene and output bindings). `shadow` is the Stone Shadow lookup buffer, bound at
/// `TRAY_SHADOW_BINDING` for a pipeline built from the tray variant (`None` otherwise). A Tray
/// Reflection without a shadow binds the same buffer, header words only. Both optional buffers
/// travel as one `(tables, shadow)` tuple.
pub fn build_chunk_bind_group(
    ctx: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
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
    ];
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
        "transport bind group (pipelined production)",
        pipeline,
        &entries,
    )
}

/// The five `wavefront_transport.wgsl` entry points. See
/// [`super::GpuFrameRenderer::dispatch_chunk_wavefront`] for how they are driven.
pub struct WavefrontPipelines {
    pub(super) generate: wgpu::ComputePipeline,
    pub(super) bounce: wgpu::ComputePipeline,
    pub(super) compact_scan: wgpu::ComputePipeline,
    pub(super) compact_scatter: wgpu::ComputePipeline,
    pub(super) finalize_survivors: wgpu::ComputePipeline,
}

impl WavefrontPipelines {
    pub(crate) fn new(device: &wgpu::Device, class: u32) -> Self {
        let constants = super::material_class::pipeline_constants(class);
        let pipeline = |entry| {
            compute::create_compute_pipeline_with_constants(
                device,
                entry,
                WAVEFRONT_SHADER_SRC,
                entry,
                &constants,
            )
        };
        Self {
            generate: pipeline("wavefront_generate"),
            bounce: pipeline("wavefront_bounce"),
            compact_scan: pipeline("wavefront_compact_scan"),
            compact_scatter: pipeline("wavefront_compact_scatter"),
            finalize_survivors: pipeline("wavefront_finalize_survivors"),
        }
    }
}

/// Bytes per (pixel, sample) tuple of [`WavefrontRayBuffers::stokes`], the widest per-ray field:
/// `tuples * 8` elements of `[f32; 4]`, an 8-channel Stokes vector per ray.
///
/// [`super::GpuFrameRenderer::cap_byte_budget_for_wavefront`] caps a chunk's tuple count with it so
/// that `stokes` stays within `max_storage_buffer_binding_size`. That limit applies per binding, so
/// tracking the widest field is enough. Fields of 16 bytes per tuple, such as
/// `pending_light_mis_dir` (binding 34) and `nee_xyz` (binding 35), stay well under this constant.
pub const WAVEFRONT_STOKES_BYTES_PER_TUPLE: usize = 8 * 16;

/// The pure arithmetic behind [`super::GpuFrameRenderer::cap_byte_budget_for_wavefront`].
///
/// Takes the storage binding limit as a plain `usize`, so the adapter-free tests in
/// `renderer::gpu::frame` can pass a fake limit. Caps `byte_budget_pixels` to the pixels whose
/// `spp` samples fit [`WavefrontRayBuffers::stokes`] (at [`WAVEFRONT_STOKES_BYTES_PER_TUPLE`]
/// bytes per tuple) within one binding. The `.max(1)` steps keep a tiny limit or `spp == 0`
/// from returning 0, which would make a later chunk-sizing division panic.
#[must_use]
pub fn wavefront_pixel_cap(max_binding_bytes: usize, spp: u32, byte_budget_pixels: usize) -> usize {
    let tuple_cap = (max_binding_bytes / WAVEFRONT_STOKES_BYTES_PER_TUPLE).max(1);
    let spp_usize = (spp as usize).max(1);
    let pixel_cap = (tuple_cap / spp_usize).max(1);
    byte_budget_pixels.min(pixel_cap)
}

/// Deterministic exclusive prefix sum over `counts` (one entry per compaction
/// workgroup), plus the total. A trivial sequential CPU loop -- see
/// `wavefront_compact_scan`'s own doc comment (`wavefront_transport.wgsl`) for why this
/// stays on the host rather than a GPU-side atomic counter.
pub fn exclusive_prefix_sum(counts: &[u32]) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(counts.len());
    let mut running = 0u32;
    for &c in counts {
        offsets.push(running);
        running += c;
    }
    (offsets, running)
}

/// `wavefront_generate`'s binding list -- see `wavefront_transport.wgsl`'s own module
/// doc comment for why this entry point reaches `camera`/`params` (binding 0/1, camera
/// ray generation) but none of the other shared scene bindings.
pub const fn wavefront_generate_bindings<'a>(
    camera: &'a wgpu::Buffer,
    params_buf: &'a wgpu::Buffer,
    wf_params_buf: &'a wgpu::Buffer,
    rays: &'a WavefrontRayBuffers,
) -> [(u32, &'a wgpu::Buffer); 19] {
    [
        (0, camera),
        (1, params_buf),
        (15, wf_params_buf),
        (16, &rays.origin),
        (17, &rays.dir),
        (18, &rays.k),
        (19, &rays.prev_plane_normal),
        (20, &rays.flags),
        (21, &rays.stokes),
        (22, &rays.radiance),
        (23, &rays.path_pdf),
        (24, &rays.split_radiance),
        (25, &rays.compat),
        (26, &rays.pending_light_mis),
        (27, &rays.lambdas),
        (28, &rays.seed),
        (29, &rays.active_in),
        (34, &rays.pending_light_mis_dir),
        (35, &rays.nee_xyz),
    ]
}

/// The four bind groups every bounce round (plus the final survivors pass) dispatches
/// through, built ONCE per chunk since the buffers they reference never change across
/// rounds -- see [`super::GpuFrameRenderer::run_wavefront_bounce_rounds`]'s own comment on
/// why `active_ray_indices`/`active_ray_indices_next` are copied between rounds instead of
/// the bind groups being rebuilt.
pub struct WavefrontRoundBindGroups {
    pub(super) bounce: wgpu::BindGroup,
    pub(super) compact_scan: wgpu::BindGroup,
    pub(super) compact_scatter: wgpu::BindGroup,
    pub(super) finalize: wgpu::BindGroup,
}

/// Builds [`WavefrontRoundBindGroups`] for one chunk.
///
/// Split out of [`super::GpuFrameRenderer::dispatch_chunk_wavefront`] to keep that function
/// under clippy's function-length limit, and split again into one function per bind group.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per distinct buffer source this chunk's four bind groups draw \
              from (device, pipelines, scene buffers, params, geometry length, transport \
              outputs, wavefront params, ray state) -- bundling them into a context struct \
              here would just re-introduce this exact parameter list one level removed, the \
              same reasoning ChunkFrameState/TransportDispatchArgs/TurnSetup already \
              document elsewhere in this module tree"
)]
pub fn build_wavefront_round_bind_groups(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> WavefrontRoundBindGroups {
    WavefrontRoundBindGroups {
        bounce: wavefront_bounce_bind_group(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            wf_params_buf,
            rays,
        ),
        compact_scan: wavefront_compact_scan_bind_group(device, pipelines, wf_params_buf, rays),
        compact_scatter: wavefront_compact_scatter_bind_group(
            device,
            pipelines,
            wf_params_buf,
            rays,
        ),
        finalize: wavefront_finalize_bind_group(
            device,
            pipelines,
            params_buf,
            outputs,
            wf_params_buf,
            rays,
        ),
    }
}

/// `wavefront_bounce`'s bind group: everything `transport_bounce_step` and
/// `transport_finalize_ray` can reach, plus this pipeline's per-round state.
///
/// Includes `camera` (binding 0). `wavefront_bounce` recomputes each ray's observer direction
/// through `transport_generate_ray`, which reads `camera` and `params` to rebuild the ray's
/// pixel and sample indices. The auto-derived pipeline layout requires every binding the entry
/// point's call graph reaches, so omitting `camera` fails wgpu validation on every dispatch.
#[expect(
    clippy::too_many_arguments,
    reason = "see build_wavefront_round_bind_groups' own #[expect] just above"
)]
fn wavefront_bounce_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    compute::bind_buffers_sized(
        device,
        "wavefront bounce bind group",
        &pipelines.bounce,
        &[
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
            (15, wf_params_buf, None),
            (16, &rays.origin, None),
            (17, &rays.dir, None),
            (18, &rays.k, None),
            (19, &rays.prev_plane_normal, None),
            (20, &rays.flags, None),
            (21, &rays.stokes, None),
            (22, &rays.radiance, None),
            (23, &rays.path_pdf, None),
            (24, &rays.split_radiance, None),
            (25, &rays.compat, None),
            (26, &rays.pending_light_mis, None),
            (27, &rays.lambdas, None),
            (28, &rays.seed, None),
            (29, &rays.active_in, None),
            (34, &rays.pending_light_mis_dir, None),
            (35, &rays.nee_xyz, None),
        ],
    )
}

/// `wavefront_compact_scan`'s bind group -- see `wavefront_transport.wgsl`'s own doc
/// comment for why it touches only `ray_flags`/`active_ray_indices` plus its own
/// per-workgroup scratch, none of the scene bindings.
fn wavefront_compact_scan_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scan bind group",
        &pipelines.compact_scan,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (31, &rays.local_offset),
            (32, &rays.block_alive_count),
        ],
    )
}

/// `wavefront_compact_scatter`'s bind group -- see `wavefront_compact_scan_bind_group`'s
/// own doc comment.
fn wavefront_compact_scatter_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scatter bind group",
        &pipelines.compact_scatter,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (30, &rays.active_out),
            (31, &rays.local_offset),
            (33, &rays.block_offset),
        ],
    )
}

/// `wavefront_finalize_survivors`'s bind group: the params and output buffers that
/// `transport_finalize_ray` writes, plus its per-ray scratch. It binds no scene buffers,
/// because a survivor's physics already ran in `wavefront_bounce`.
///
/// It binds `rays.nee_xyz` (35). This entry point reads a surviving ray's accumulated NEE
/// total, and the implicit pipeline layout `wgpu` derives needs every binding it reaches.
/// `pending_light_mis_dir` (34) is not bound because this entry point never reads it.
fn wavefront_finalize_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    params_buf: &wgpu::Buffer,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront finalize survivors bind group",
        &pipelines.finalize_survivors,
        &[
            (1, params_buf),
            (4, outputs.xyz()),
            (5, outputs.radiance()),
            (6, outputs.lambdas()),
            (7, outputs.path_pdf()),
            (9, outputs.compat()),
            (15, wf_params_buf),
            (20, &rays.flags),
            (22, &rays.radiance),
            (23, &rays.path_pdf),
            (24, &rays.split_radiance),
            (25, &rays.compat),
            (27, &rays.lambdas),
            (29, &rays.active_in),
            (35, &rays.nee_xyz),
        ],
    )
}
