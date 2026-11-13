//! The fragment and refine kernels (beam guide §14.7 item 2).
//!
//! One invocation per (leaf, covered pixel), lighting through the tracer's own WGSL environment
//! functions, refine pass up to six levels. Each fragment writes its own XYZ slot;
//! `reduce_beam_fragments.wgsl` sums the slots per pixel in fixed order.
//!
//! # How the kernel source is made
//!
//! `build.rs` assembles the megakernel text as prelude + zoned-patched bounce text + the
//! `transport_main` body. The fragment kernel takes that generated text up to `transport_main`
//! (so `GpuGemMaterial` keeps its zoned layout and the environment functions are the shipped
//! ones) and appends `shaders/beam/fragments.wgsl`; the refine kernel appends `refine.wgsl` too.
//! Anchors are counted: any other count declines (`None`).
//!
//! # Bindings
//!
//! Group 0 only (the pipelines use an inferred layout, so exactly the statically reached
//! resources are bound): the scene bindings `0` camera, `1` params, `10` `hdr_texels` and `11`
//! `hdr_env_dims` of the megakernel, and the beam range `200..=208`.

use std::sync::OnceLock;

use super::buffers::{FragmentPlan, LeafBufferSet};
use crate::renderer::{
    gpu::{context::GpuContext, frame::SHADER_SRC},
    gpu_backend::GpuSceneRef,
};

/// `shaders/beam/fragments.wgsl`.
pub const FRAGMENTS_WGSL: &str = include_str!("../../shaders/beam/fragments.wgsl");

/// `shaders/beam/refine.wgsl`.
pub const REFINE_WGSL: &str = include_str!("../../shaders/beam/refine.wgsl");

/// The entry point of the first-level kernel.
pub const FRAGMENTS_ENTRY: &str = "bm_fragments_main";

/// The entry point of the refine kernel.
pub const REFINE_ENTRY: &str = "bm_refine_main";

/// The beam bindings of both kernels (group 0), inside the beam range 200-219.
pub const BEAM_BINDINGS: [u32; 9] = [200, 201, 202, 203, 204, 205, 206, 207, 208];

/// The megakernel scene bindings both kernels reach: camera, params, `hdr_texels`, `hdr_env_dims`.
pub const SCENE_BINDINGS: [u32; 4] = [0, 1, 10, 11];

/// Storage buffers per shader stage both kernels need (eight beam buffers and `hdr_texels`).
pub const STORAGE_BUFFERS: u32 = 9;

/// Uniform buffers per shader stage both kernels need (camera, params, `hdr_env_dims`, own).
pub const UNIFORM_BUFFERS: u32 = 4;

/// Invocations per workgroup (matches `@workgroup_size` in the shaders).
pub const WORKGROUP_SIZE: u32 = 16;

/// Workgroup memory per workgroup in bytes (`16 * 40` `vec2<f32>` scratch slots).
pub const WORKGROUP_STORAGE_BYTES: u32 = 5120;

/// The spectral node count assumed when the caller does not say (`BeamSettings` default).
pub const DEFAULT_N_LAMBDA: u32 = 48;

/// The smoothness tolerance `τ_L` assumed when the caller does not say (guide §11.4).
pub const DEFAULT_TOLERANCE: f32 = 1e-3;

/// The deepest refine level (guide §11.4); larger requests are clamped to it.
pub const MAX_REFINE_LEVELS: u32 = 6;

/// The most workgroups along x of one dispatch (the shaders flatten the 2-D grid).
const MAX_GROUPS_X: u32 = 4096;

/// The most fragment slots one kernel submission covers; larger plans run as several chunks.
pub const FRAGMENT_CHUNK: u32 = 4096;

/// The bound on the wait for one fragment or refine chunk.
///
/// A chunk is a few thousand fragments, so 30 s is far above a healthy run and still surfaces a
/// wedged or hopelessly slow device as a decline.
pub const BEAM_CHUNK_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The decline reason when a chunk did not finish within [`BEAM_CHUNK_WAIT_TIMEOUT`].
pub const DECLINE_TIME_BOUND: &str = "fragment kernels exceeded the GPU time bound";

/// The decline reason when the device or the plan cannot run the fragment kernels.
pub const DECLINE_KERNEL: &str = "fragment kernel declined";

/// Why a cancellable fragment dispatch produced no slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentStop {
    /// The cancel token was observed set between chunks.
    Cancelled,
    /// The device declined; the text is the reason for the log and the render note.
    Declined(&'static str),
}

/// Single-line anchors that must occur exactly once in the megakernel text for the kernel to be
/// assembled: the functions and declarations the beam units call.
pub const REQUIRED_ANCHORS: [&str; 14] = [
    "fn sample_environment_with_rig(",
    "fn studio_dispatch(",
    "fn hdr_env_radiance_at(",
    "fn studio_spectral_power(",
    "fn daylight_sun_factor(",
    "fn cie_1931_cmf(",
    "fn studio_rig_key_dir(",
    "fn studio_rig_fill_dir(",
    "var<uniform> camera: GpuCameraParams;",
    "var<uniform> params: GpuTransportParams;",
    "var<storage, read> hdr_texels: array<vec4<f32>>;",
    "var<uniform> hdr_env_dims: HdrEnvDims;",
    "const SUN_DISC_COS: f32",
    "const SUN_ONE_MINUS_COS: f32",
];

/// The per-fragment output of [`dispatch_fragments`]: `plan.total` slots of `vec4<f32>` (XYZ and a
/// spare lane), in the plan's fragment order.
pub struct FragmentSlots {
    /// The slot buffer (storage and copy source, read by the reduction).
    pub slots: wgpu::Buffer,
    /// The slot count.
    pub count: u32,
}

/// The knobs of one fragment dispatch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FragmentOptions {
    /// The deepest fragment subdivision (clamped to [`MAX_REFINE_LEVELS`]).
    pub max_levels: u32,
    /// The spectral node count `N_λ` the leaves' node indices refer to.
    pub n_lambda: u32,
    /// The smoothness tolerance `τ_L`.
    pub tolerance: f32,
}

impl FragmentOptions {
    /// The defaults ([`DEFAULT_N_LAMBDA`], [`DEFAULT_TOLERANCE`]) with `max_levels`.
    #[must_use]
    pub const fn new(max_levels: u32) -> Self {
        Self {
            max_levels,
            n_lambda: DEFAULT_N_LAMBDA,
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// The generated megakernel text up to its entry point: prelude and zoned bounce text. `None`
/// when an anchor is missing or ambiguous.
fn kernel_prefix(source: &str) -> Option<&str> {
    for anchor in REQUIRED_ANCHORS {
        if source.matches(anchor).count() != 1 {
            return None;
        }
    }
    let entry = "fn transport_main(";
    if source.matches(entry).count() != 1 {
        return None;
    }
    let at = source.find(entry)?;
    let attribute = source[..at].rfind("@compute")?;
    if source[attribute..at].contains(['{', ';']) {
        return None;
    }
    Some(&source[..attribute])
}

/// The first-level fragment kernel: the generated text up to `transport_main` plus
/// `fragments.wgsl`; built once, `None` when an anchor drifted.
#[must_use]
pub fn fragment_kernel_source() -> Option<String> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            kernel_prefix(SHADER_SRC).map(|prefix| format!("{prefix}\n{FRAGMENTS_WGSL}"))
        })
        .clone()
}

/// The refine kernel: [`fragment_kernel_source`] plus `refine.wgsl`.
#[must_use]
pub fn refine_kernel_source() -> Option<String> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            kernel_prefix(SHADER_SRC)
                .map(|prefix| format!("{prefix}\n{FRAGMENTS_WGSL}\n{REFINE_WGSL}"))
        })
        .clone()
}

/// Runs the fragment kernel and its refine pass over the batch's leaves under the scene's
/// lighting; `None` when the device declines (limits, pipeline creation, a lost device).
///
/// Uses the default `N_λ` and `τ_L`; [`dispatch_fragments_with`] takes them explicitly.
#[must_use]
pub fn dispatch_fragments(
    ctx: &GpuContext,
    scene: &GpuSceneRef<'_>,
    leaves: &LeafBufferSet,
    plan: &FragmentPlan,
    max_levels: u32,
) -> Option<FragmentSlots> {
    dispatch_fragments_with(ctx, scene, leaves, plan, &FragmentOptions::new(max_levels))
}

/// Whether `plan` describes `leaves`.
fn plan_matches(plan: &FragmentPlan, leaves: &LeafBufferSet) -> bool {
    let count = leaves.leaf_count as usize;
    plan.first_fragment.len() == count + 1
        && plan.pixel_rects.len() == count
        && plan.first_fragment.last() == Some(&plan.total)
}

#[cfg(target_arch = "wasm32")]
/// Beam fragments are not offered in the browser.
#[must_use]
pub fn dispatch_fragments_with(
    ctx: &GpuContext,
    scene: &GpuSceneRef<'_>,
    leaves: &LeafBufferSet,
    plan: &FragmentPlan,
    options: &FragmentOptions,
) -> Option<FragmentSlots> {
    let _ = (ctx, scene, leaves, plan, options);
    None
}

#[cfg(target_arch = "wasm32")]
/// Beam fragments are not offered in the browser.
///
/// # Errors
///
/// Always [`FragmentStop::Declined`].
pub fn dispatch_fragments_cancellable(
    ctx: &GpuContext,
    scene: &GpuSceneRef<'_>,
    leaves: &LeafBufferSet,
    plan: &FragmentPlan,
    options: &FragmentOptions,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<FragmentSlots, FragmentStop> {
    let _ = (ctx, scene, leaves, plan, options, cancel);
    Err(FragmentStop::Declined(DECLINE_KERNEL))
}

#[cfg(target_arch = "wasm32")]
/// Beam fragments are not offered in the browser.
#[must_use]
pub const fn precompile_pipelines(ctx: &GpuContext) -> bool {
    let _ = ctx;
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{dispatch_fragments_cancellable, dispatch_fragments_with, precompile_pipelines};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{
            Arc, PoisonError,
            atomic::{AtomicBool, Ordering},
        },
    };

    use bytemuck::{Pod, Zeroable};

    use super::{
        BEAM_CHUNK_WAIT_TIMEOUT, DECLINE_KERNEL, DECLINE_TIME_BOUND, FRAGMENT_CHUNK,
        FRAGMENTS_ENTRY, FragmentOptions, FragmentSlots, FragmentStop, MAX_GROUPS_X,
        MAX_REFINE_LEVELS, REFINE_ENTRY, STORAGE_BUFFERS, UNIFORM_BUFFERS, WORKGROUP_SIZE,
        WORKGROUP_STORAGE_BYTES, fragment_kernel_source, plan_matches, refine_kernel_source,
    };
    use crate::{
        optics::raytracer::{EnvironmentSource, head_shadow_cosines},
        renderer::{
            beam::lighting::sigma_l,
            buffers::{GpuCameraParams, GpuTransportParams},
            env_map_gpu::HdrEnvGpuData,
            gpu::{compute, context::GpuContext, frame::environment_params},
            gpu_backend::GpuSceneRef,
        },
    };

    use super::super::{
        buffers::{FragmentPlan, LeafBufferSet, busy_reason, note_abandoned},
        device_caches::BeamFragmentPipelines,
    };

    /// `BmParams` of the shaders (32 bytes).
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Pod, Zeroable)]
    struct FragParams {
        total: u32,
        leaf_count: u32,
        n_lambda: u32,
        max_levels: u32,
        tolerance: f32,
        sigma: f32,
        grid_width: u32,
        /// The chunk's first slot; the shaders add it to the invocation's slot.
        slot_base: u32,
    }

    /// A hash of the map's size and every texel's bits (FNV-1a), the cache key of its upload.
    fn hdr_key(map: &crate::renderer::env_map::EnvironmentMap) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut mix = |word: u64| {
            hash = (hash ^ word).wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(map.width() as u64);
        mix(map.height() as u64);
        for texel in map.pixels() {
            for channel in texel {
                mix(u64::from(channel.to_bits()));
            }
        }
        hash
    }

    /// The HDR environment of `map` on `ctx`'s device, uploaded once per distinct map.
    fn hdr_for(
        ctx: &GpuContext,
        map: &crate::renderer::env_map::EnvironmentMap,
    ) -> Arc<HdrEnvGpuData> {
        let key = hdr_key(map);
        let mut slot = ctx
            .beam_caches
            .hdr
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((cached, data)) = slot.as_ref()
            && *cached == key
        {
            return Arc::clone(data);
        }
        let data = Arc::new(HdrEnvGpuData::upload(&ctx.device, map));
        *slot = Some((key, Arc::clone(&data)));
        drop(slot);
        data
    }

    /// An image size as `f32`; sizes are far below 2^24, so the cast is exact.
    #[expect(
        clippy::cast_precision_loss,
        reason = "image sizes are far below 2^24, exactly representable"
    )]
    const fn px_f32(v: u32) -> f32 {
        v as f32
    }

    fn device_suffices(ctx: &GpuContext) -> bool {
        let limits = ctx.device.limits();
        limits.max_storage_buffers_per_shader_stage >= STORAGE_BUFFERS
            && limits.max_uniform_buffers_per_shader_stage >= UNIFORM_BUFFERS
            && limits.max_compute_workgroup_storage_size >= WORKGROUP_STORAGE_BYTES
            && limits.max_compute_invocations_per_workgroup >= WORKGROUP_SIZE
            && limits.max_compute_workgroup_size_x >= WORKGROUP_SIZE
    }

    /// The compiled kernels of `ctx`'s device, compiled once; `None` when compilation fails.
    fn pipelines_for(ctx: &GpuContext) -> Option<Arc<BeamFragmentPipelines>> {
        let mut slot = ctx
            .beam_caches
            .fragment_pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = slot.as_ref() {
            return Some(Arc::clone(found));
        }
        let fragments_source = fragment_kernel_source()?;
        let refine_source = refine_kernel_source()?;
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let built = catch_unwind(AssertUnwindSafe(|| BeamFragmentPipelines {
            fragments: compute::create_compute_pipeline(
                &ctx.device,
                "beam fragments",
                &fragments_source,
                FRAGMENTS_ENTRY,
            ),
            refine: compute::create_compute_pipeline(
                &ctx.device,
                "beam refine",
                &refine_source,
                REFINE_ENTRY,
            ),
        }));
        let error = pollster::block_on(scope.pop());
        if let Some(error) = error {
            tracing::warn!(%error, "beam fragment kernels failed to compile");
            return None;
        }
        let pipelines = Arc::new(built.ok()?);
        *slot = Some(Arc::clone(&pipelines));
        drop(slot);
        Some(pipelines)
    }

    /// The megakernel's camera and params uniforms for the scene's environment.
    fn scene_uniforms(scene: &GpuSceneRef<'_>) -> (GpuCameraParams, GpuTransportParams) {
        let camera = scene.camera;
        let camera_params = GpuCameraParams {
            origin: camera.origin.to_array(),
            fov_tan: camera.fov_tan,
            forward: camera.forward.to_array(),
            width: px_f32(scene.width),
            right: camera.right.to_array(),
            height: px_f32(scene.height),
            up: camera.up.to_array(),
            num_samples: 1,
        };
        let env = scene.environment;
        let (env_mode, temp_k, spot_mult, exposure, yaw, pitch, use_d65, model, backdrop) =
            environment_params(env);
        let params = GpuTransportParams::new(
            1, 1, 0, env_mode, 0.0, temp_k, spot_mult, exposure, yaw, pitch, [1.0; 3],
        )
        .with_studio_use_d65(use_d65)
        .with_studio_model(model)
        .with_tent_of(env)
        .with_hdr_of(env)
        .with_backdrop(backdrop)
        .with_surface_glare(env.surface_glare())
        .with_head_shadow(head_shadow_cosines(env.head_shadow_deg()));
        (camera_params, params)
    }

    /// `(groups_x, groups_y)` covering `total` fragments within the device's dispatch limit.
    fn grid(ctx: &GpuContext, total: u32) -> Option<(u32, u32)> {
        let limit = ctx.device.limits().max_compute_workgroups_per_dimension;
        let groups = total.div_ceil(WORKGROUP_SIZE).max(1);
        let x = groups.min(MAX_GROUPS_X.min(limit));
        let y = groups.div_ceil(x);
        (y <= limit).then_some((x, y))
    }

    fn empty_slots(ctx: &GpuContext) -> FragmentSlots {
        FragmentSlots {
            slots: compute::zeroed_buffer::<[f32; 4]>(
                &ctx.device,
                "beam fragment slots (empty)",
                1,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            count: 0,
        }
    }

    /// Runs the fragment kernel and its refine pass with explicit options.
    ///
    /// [`dispatch_fragments_cancellable`] with a token that is never set; `None` on any decline.
    #[must_use]
    pub fn dispatch_fragments_with(
        ctx: &GpuContext,
        scene: &GpuSceneRef<'_>,
        leaves: &LeafBufferSet,
        plan: &FragmentPlan,
        options: &FragmentOptions,
    ) -> Option<FragmentSlots> {
        dispatch_fragments_cancellable(ctx, scene, leaves, plan, options, &AtomicBool::new(false))
            .ok()
    }

    /// Compiles the two beam kernels of `ctx`'s device ahead of the first batch; `false` when the
    /// device cannot run them or compilation fails.
    #[must_use]
    pub fn precompile_pipelines(ctx: &GpuContext) -> bool {
        device_suffices(ctx) && pipelines_for(ctx).is_some()
    }

    /// Runs the fragment kernel and its refine pass in chunks of [`FRAGMENT_CHUNK`] slots.
    ///
    /// Every fragment chunk runs first, then every refine chunk over `0..plan.total` (the refine
    /// list is global; its kernel bounds itself by the atomic count). Each chunk's submission is
    /// waited for with [`BEAM_CHUNK_WAIT_TIMEOUT`] and `cancel` is polled between chunks, so one
    /// piece can neither run unbounded nor ignore a cancel. A chunk that misses the bound is
    /// recorded as an abandoned dispatch of the device.
    ///
    /// # Errors
    ///
    /// [`FragmentStop::Cancelled`] when `cancel` was observed set; [`FragmentStop::Declined`] for
    /// a device that is busy, too small or lost, a plan that does not match the leaves, a
    /// validation error, or a chunk that exceeded the time bound.
    pub fn dispatch_fragments_cancellable(
        ctx: &GpuContext,
        scene: &GpuSceneRef<'_>,
        leaves: &LeafBufferSet,
        plan: &FragmentPlan,
        options: &FragmentOptions,
        cancel: &AtomicBool,
    ) -> Result<FragmentSlots, FragmentStop> {
        if cancel.load(Ordering::Relaxed) {
            return Err(FragmentStop::Cancelled);
        }
        if !plan_matches(plan, leaves) || !device_suffices(ctx) || ctx.failure().is_some() {
            return Err(FragmentStop::Declined(DECLINE_KERNEL));
        }
        if let Some(reason) = busy_reason(ctx) {
            return Err(FragmentStop::Declined(reason));
        }
        if plan.total == 0 {
            return Ok(empty_slots(ctx));
        }
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = run(ctx, scene, leaves, plan, options, cancel);
        let error = pollster::block_on(scope.pop());
        if let Some(error) = error {
            tracing::warn!(%error, "beam fragment dispatch declined");
            return Err(FragmentStop::Declined(DECLINE_KERNEL));
        }
        if ctx.failure().is_some() {
            return Err(FragmentStop::Declined(DECLINE_KERNEL));
        }
        result
    }

    /// The buffers every chunk of one dispatch binds, and what a chunk's parameters start from.
    struct ChunkJob<'a> {
        ctx: &'a GpuContext,
        cancel: &'a AtomicBool,
        total: u32,
        params: FragParams,
        shared: Vec<(u32, &'a wgpu::Buffer)>,
    }

    impl ChunkJob<'_> {
        /// Submits `pipeline` over the chunk starting at slot `base` and waits for it.
        fn run(
            &self,
            pipeline: &wgpu::ComputePipeline,
            label: &str,
            base: u32,
        ) -> Result<(), FragmentStop> {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(FragmentStop::Cancelled);
            }
            let device = &self.ctx.device;
            let size = (self.total - base).min(FRAGMENT_CHUNK);
            let (groups_x, groups_y) =
                grid(self.ctx, size).ok_or(FragmentStop::Declined(DECLINE_KERNEL))?;
            let params = compute::upload(
                device,
                "beam fragment params",
                &[FragParams {
                    total: size,
                    grid_width: groups_x * WORKGROUP_SIZE,
                    slot_base: base,
                    ..self.params
                }],
                wgpu::BufferUsages::UNIFORM,
            );
            let mut entries = self.shared.clone();
            entries.push((208, &params));
            let group = compute::bind_buffers(device, label, pipeline, &entries);
            let index = compute::dispatch(
                device,
                &self.ctx.queue,
                pipeline,
                &group,
                (groups_x, groups_y, 1),
            );
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(index.clone()),
                    timeout: Some(BEAM_CHUNK_WAIT_TIMEOUT),
                })
                .map_err(|error| {
                    tracing::warn!(%error, base, "beam fragment chunk missed the GPU time bound");
                    note_abandoned(self.ctx, index);
                    FragmentStop::Declined(DECLINE_TIME_BOUND)
                })?;
            Ok(())
        }
    }

    fn run(
        ctx: &GpuContext,
        scene: &GpuSceneRef<'_>,
        leaves: &LeafBufferSet,
        plan: &FragmentPlan,
        options: &FragmentOptions,
        cancel: &AtomicBool,
    ) -> Result<FragmentSlots, FragmentStop> {
        let declined = FragmentStop::Declined(DECLINE_KERNEL);
        let device = &ctx.device;
        let pipelines = pipelines_for(ctx).ok_or(declined)?;
        grid(ctx, FRAGMENT_CHUNK.min(plan.total)).ok_or(declined)?;
        let (camera_params, transport) = scene_uniforms(scene);
        let hdr = match scene.environment {
            EnvironmentSource::HdrMap { map, .. } => {
                if !HdrEnvGpuData::fits_storage_binding(device, map) {
                    return Err(declined);
                }
                hdr_for(ctx, map)
            }
            EnvironmentSource::Studio { .. } => Arc::new(HdrEnvGpuData::dummy(device)),
        };
        let uniform = wgpu::BufferUsages::UNIFORM;
        let storage = wgpu::BufferUsages::STORAGE;
        let camera = compute::upload(device, "beam camera", &[camera_params], uniform);
        let params = compute::upload(device, "beam scene params", &[transport], uniform);
        let first = compute::upload(device, "beam first fragment", &plan.first_fragment, storage);
        let rects = compute::upload(device, "beam pixel rects", &plan.pixel_rects, storage);
        let slots = compute::zeroed_buffer::<[f32; 4]>(
            device,
            "beam fragment slots",
            plan.total as usize,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let list =
            compute::zeroed_buffer::<u32>(device, "beam refine list", plan.total as usize, storage);
        let count = compute::zeroed_buffer::<u32>(device, "beam refine count", 1, storage);
        let job = ChunkJob {
            ctx,
            cancel,
            total: plan.total,
            params: FragParams {
                total: plan.total,
                leaf_count: leaves.leaf_count,
                n_lambda: options.n_lambda.max(1),
                max_levels: options.max_levels.min(MAX_REFINE_LEVELS),
                tolerance: options.tolerance,
                sigma: sigma_l(&scene.environment),
                grid_width: 0,
                slot_base: 0,
            },
            shared: vec![
                (0, &camera),
                (1, &params),
                (10, &hdr.texels),
                (11, &hdr.dims),
                (200, &leaves.headers),
                (201, &leaves.vertices),
                (202, &leaves.nodes),
                (203, &first),
                (204, &rects),
                (205, &slots),
                (206, &list),
                (207, &count),
            ],
        };
        let bases = || (0..plan.total).step_by(FRAGMENT_CHUNK as usize);
        for base in bases() {
            job.run(&pipelines.fragments, "beam fragments bind group", base)?;
        }
        for base in bases() {
            job.run(&pipelines.refine, "beam refine bind group", base)?;
        }
        Ok(FragmentSlots {
            slots,
            count: plan.total,
        })
    }
}
