//! [`GpuFrameRenderer`] itself: the struct definition plus construction and
//! capacity-management methods (`new`, `set_pipeline_kind`, the `ensure_*` buffer-growth
//! methods, and `abandon_in_flight`). The per-frame accumulate loop and per-chunk
//! dispatch live in sibling modules ([`super::accumulate`]/[`super::dispatch`]) since
//! both need `&mut self`/`&self` access to the fields defined here.

use std::{collections::HashMap, sync::OnceLock};

use wgpu::BufferUsages;

use crate::renderer::gpu::{
    GpuContext, MEGAKERNEL_STORAGE_BUFFERS, WAVEFRONT_BOUNCE_TOTAL_BUFFERS,
    WAVEFRONT_STORAGE_BUFFERS,
};

use super::{
    CHUNK_BUDGET_BYTES, GpuFrameError, GpuPipelineKind, REDUCE_SHADER_SRC,
    bind_groups::WavefrontPipelines,
    live::LiveState,
    readback::{GpuReduceParams, PixelXyzOutput, StagingSlot, staging_needs_growth},
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
    tables_buffer::OpticsTablesGpu,
};
use crate::{
    optics::raytracer::Estimator,
    renderer::{
        buffers::{GpuSurfaceScene, studio_model, transport_env_mode},
        gpu::compute,
    },
};

/// The estimator a frame asks for, in order of precedence: the request's own (`asked`), this
/// renderer's own choice (`own`), the process default (`process`).
///
/// Only the megakernel family implements exit collection, so the legacy bounce wavefront
/// (`kind`) always resolves to [`Estimator::Branching`]; a frame that would collect there
/// (asked, own or process) is declined by the turn preparation instead (see
/// `collect_applies`). Whether the
/// resolved estimator is honoured is decided per dispatch by
/// `GpuTransportParams::with_estimator`.
pub const fn resolve_estimator(
    asked: Option<Estimator>,
    own: Option<Estimator>,
    process: Estimator,
    kind: GpuPipelineKind,
) -> Estimator {
    if matches!(kind, GpuPipelineKind::Wavefront) {
        return Estimator::Branching;
    }
    match (asked, own) {
        (Some(estimator), _) | (None, Some(estimator)) => estimator,
        (None, None) => process,
    }
}

/// Whether exit collection applies to a frame with these properties, by the rule
/// `GpuTransportParams::with_estimator` and the CPU tracer share: over an ideal surface, without
/// a light-sampling technique (neither an HDR map nor the Daylight + Sun rig), for a convex stone
/// (`tool_count` zero) and without a Tray Reflection (`tray_mirror`).
pub const fn collect_applies(
    env_mode: u32,
    studio_model: u32,
    ideal_surface: bool,
    tool_count: usize,
    tray_mirror: bool,
) -> bool {
    let no_light_sampling = env_mode != transport_env_mode::HDR_MAP
        && !(env_mode == transport_env_mode::STUDIO_RIG
            && studio_model == studio_model::DAYLIGHT_SUN);
    ideal_surface && no_light_sampling && tool_count == 0 && !tray_mirror
}

/// A GPU-backed renderer for arbitrary scenes, owning its device and pipeline across
/// frames.
///
/// Construct once and keep it: [`GpuFrameRenderer::new`] acquires an adapter and
/// compiles the small reduction kernel; the megakernel for a material class compiles on
/// that class's first frame. Both take long enough to be worth doing off the frame loop.
pub struct GpuFrameRenderer {
    /// The GENERIC (`MATERIAL_CLASS = 0`) pipeline, compiled on first request through
    /// [`Self::pipeline_for_class`](super::specialisation): only the self-tests and the
    /// equivalence checks ask for it, because `classify_material` never returns GENERIC
    /// for a real material, so a production session never pays its compile. A `OnceLock`
    /// because the dispatch paths that request it hold only `&self`. See the parent
    /// module's doc comment's "Material-class kernel specialisation" section.
    pub(super) pipeline_generic: OnceLock<wgpu::ComputePipeline>,
    /// The specialised pipelines by class value (class plus the feature bits, see
    /// `material_class`), built LAZILY (see
    /// [`Self::ensure_specialized_pipeline`](super::specialisation)) on first use of that
    /// value, so a session that only renders one class never pays the others' compile cost.
    pub(super) pipelines_specialised: HashMap<u32, wgpu::ComputePipeline>,
    /// `reduce_xyz_main`'s pipeline (see `reduce_xyz.wgsl`'s header comment) -- built
    /// eagerly in [`Self::new`]/`Self::new_async`, since every production dispatch
    /// (`dispatch_chunk` and wasm32's `accumulate_async`) uses it unconditionally and it
    /// is small.
    pub(super) reduce_pipeline: wgpu::ComputePipeline,
    pub(super) adapter_label: String,
    /// TWO chunk-output buffer sets, alternated by chunk index so one chunk's dispatch
    /// can be queued into the other slot while the previous chunk's readback is still in
    /// flight -- see the parent module's doc comment's "Overlapped chunk pipeline"
    /// section.
    pub(super) outputs: [Option<TransportOutputs>; 2],
    /// TWO double-buffered [`PixelXyzOutput`]s, alternated in lockstep with `outputs` --
    /// `reduce_xyz_main`'s destination, and what `Self::dispatch_chunk`'s readback copy
    /// reads from instead of `outputs[..].xyz()` directly (see `Self::dispatch_chunk`'s
    /// "Why a second dispatch" doc comment).
    pub(super) pixel_outputs: [Option<PixelXyzOutput>; 2],
    /// TWO persistent staging buffers, alternated by chunk index in lockstep with
    /// `outputs` -- reused across every chunk of every frame and grown (never shrunk)
    /// only when [`Self::ensure_staging_capacity`] finds one too small. See the parent
    /// module's doc comment's "Per-frame uploads, persistent staging" section.
    pub(super) staging: [Option<StagingSlot>; 2],
    /// TWO persistent per-output-slot `GpuTransportParams` uniform buffers, written via
    /// `queue.write_buffer` immediately before that slot's dispatch rather than recreated
    /// every chunk (see `Self::dispatch_chunk`). Safe to overwrite a
    /// slot's buffer two chunks later because `wgpu` executes queue operations in
    /// submission order: the write is submitted strictly after the earlier chunk that
    /// read the same slot, matching `outputs`/`staging`'s own double-buffering depth.
    pub(super) params_buffers: [Option<wgpu::Buffer>; 2],
    /// TWO persistent per-output-slot [`GpuReduceParams`] uniform buffers for
    /// `reduce_xyz_main`, mirroring `params_buffers`' same write-before-dispatch,
    /// two-deep-safe reuse policy.
    pub(super) reduce_params_buffers: [Option<wgpu::Buffer>; 2],
    /// The four scene-input buffers, persistent across every `accumulate` call (not just
    /// within one) -- see [`FrameSceneBuffers::ensure`]. `None` only before this
    /// renderer's very first `accumulate` call.
    pub(super) scene_buffers: Option<FrameSceneBuffers>,
    /// Reusable scratch buffer for `Self::drain_pending_chunk`'s per-chunk GPU-reduced
    /// XYZ readback -- avoids a fresh `Vec` allocation every chunk (see
    /// `compute::finish_map_read_into`). Safe to reuse across chunks: the overlapped
    /// pipeline's pending queue is exactly one chunk deep (see the parent module's doc
    /// comment's "Overlapped chunk pipeline" section), so at most one
    /// `drain_pending_chunk` call is ever using it at a time.
    pub(super) xyz_scratch: Vec<f32>,
    pub(super) chunk_budget_bytes: usize,
    /// Running exponential moving average of nanoseconds-per-(pixel,sample)-tuple,
    /// measured from dispatch submission to readback completion in
    /// `Self::drain_pending_chunk` -- `None` until the first chunk has drained.
    /// `Self::next_chunk_pixels` sizes every later chunk toward `TARGET_CHUNK_MS`
    /// from this estimate.
    pub(super) ns_per_tuple_ema: Option<f64>,
    /// Hash of geometry, resolution, encoded material/surfaces, lighting, bounce
    /// limit and pipeline kind. A changed cost identity discards the old estimate.
    pub(super) last_scene_key: Option<u64>,
    /// Tuple count of the most recently completed chunk, used to bound growth.
    pub(super) measured_chunk_tuples: Option<usize>,
    /// Which kernel `Self::dispatch_chunk` dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. `Default::default()` (`Megakernel`)
    /// until [`Self::set_pipeline_kind`] is called.
    pub(super) pipeline_kind: GpuPipelineKind,
    /// Legacy wavefront pipelines, built lazily per complete material/feature key.
    /// Preparing a scene supplies the same overrides as the megakernel and event wavefront.
    pub(super) wavefront_pipelines: HashMap<u32, WavefrontPipelines>,
    /// Set once, permanently, by [`Self::abandon_in_flight`] -- never cleared.
    /// A `true` here means a previous chunk readback failed (or a previous turn
    /// unwound, see `Self::turn_in_progress`) and this renderer's staging slots were
    /// dropped rather than `unmap()`d, so any state a resumed dispatch would depend on
    /// (persistent buffers, the chunk-timing EMA) is no longer trustworthy. Checked at
    /// the very top of `Self::accumulate_turn`, before touching anything else, so a
    /// poisoned renderer declines every later call instead of ever dispatching into it
    /// again -- mirrors `GpuFrameError::DeviceLost`'s "stop using this renderer" contract:
    /// `GpuBackend` suspends GPU rendering and, after its recovery cool-down, replaces the
    /// renderer with a freshly acquired one.
    pub(super) poisoned: bool,
    /// `true` for exactly the duration of one `Self::accumulate_turn` call
    /// -- set at entry, cleared just before that call returns. Exists to catch a panic
    /// that unwinds OUT of `accumulate_turn` (a wgpu validation panic this module did
    /// not manage to intercept via `GpuContext::validation_error_seen`, or any other
    /// unexpected panic mid-chunk): the unwind skips the "clear" step, so the NEXT call
    /// into `accumulate_turn` finds this still `true`, knows the previous call never
    /// finished, and poisons the renderer via [`Self::abandon_in_flight`] before doing
    /// anything else -- a `catch_unwind`-free way to detect "the last turn didn't
    /// complete" without needing `std::panic::catch_unwind` (which this crate avoids: it
    /// would need `UnwindSafe` bounds on every `wgpu` type this module touches).
    pub(super) turn_in_progress: bool,
    /// The surface finish (facet surfaces, faceting machine, abraded edge) every later frame
    /// is traced with; ideal until [`Self::set_surface_scene`].
    pub(super) surface: GpuSurfaceScene,
    /// The pipelines and buffers of adaptive (live-tile) passes -- see `super::live`. Empty
    /// (nothing allocated, nothing compiled) until the first such pass, so a session that
    /// never asks for adaptive sampling is untouched by it.
    pub(super) live: LiveState,
    /// The optics lookup table buffer of the material being rendered -- empty (nothing built)
    /// until a class with `material_class::OPTICS_TABLES` is prepared, see `super::tables_buffer`.
    pub(super) optics_tables: OpticsTablesGpu,
    /// Whether the most recently prepared turn used the optics lookup tables.
    pub(super) tables_in_use: bool,
    /// Cached tray buffer of the scene (the shadow map and the mirror words), empty until a turn
    /// with a Stone Shadow or a Tray Reflection.
    pub(super) tray_shadow: super::tray_shadow_buffer::TrayShadowGpu,
    /// The estimator of every later frame; `None` follows the process default
    /// (`optics::raytracer::default_estimator`). See [`Self::set_estimator`].
    pub(super) estimator: Option<Estimator>,
    /// The staged megakernel's pipelines and buffers -- see `super::staged`. Empty until a chunk
    /// runs with stage ends: the default ends (except in the browser, on Android and on Apple
    /// systems), `PLEOCHROMA_STAGED` or [`Self::set_stage_ends`].
    pub(super) staged: super::staged::StagedState,
    /// The sorted wavefront's switch, pipelines and buffers -- see `super::sorted`. Empty until a
    /// chunk runs sorted (`PLEOCHROMA_SORTED` or [`Self::set_sorted`]).
    pub(super) sorted: super::sorted::SortedState,
    /// The device. Declared last on purpose: fields drop in declaration order, so every
    /// pipeline and buffer above is gone when the context's last clone tears the device down
    /// inside its lifecycle turn (`gpu::lifecycle`).
    pub(super) ctx: GpuContext,
}

impl GpuFrameRenderer {
    /// The device this renderer runs on, for the beam tracer's kernels (they share the
    /// renderer's device, queue and lost-device handling).
    #[must_use]
    pub const fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// Acquires a GPU device and compiles `reduce_xyz_main`; `transport_main` compiles
    /// lazily per material class (see [`Self::pipeline_generic`]'s field doc comment).
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::Acquire`] if this machine has no usable adapter or device --
    /// an expected outcome, not a bug; the caller should fall back to the CPU tracer.
    pub fn new() -> Result<Self, GpuFrameError> {
        let ctx = GpuContext::acquire().map_err(GpuFrameError::Acquire)?;
        let available = ctx.device.limits().max_storage_buffers_per_shader_stage;
        if available < MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS,
                available,
            });
        }
        let info = ctx.adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);
        let reduce_pipeline = compute::create_persisted_compute_pipeline(
            &ctx.device,
            &info,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
            &[],
        );
        Ok(Self {
            ctx,
            pipeline_generic: OnceLock::new(),
            pipelines_specialised: HashMap::new(),
            reduce_pipeline,
            adapter_label,
            outputs: [None, None],
            pixel_outputs: [None, None],
            staging: [None, None],
            params_buffers: [None, None],
            reduce_params_buffers: [None, None],
            scene_buffers: None,
            xyz_scratch: Vec::new(),
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
            ns_per_tuple_ema: None,
            last_scene_key: None,
            measured_chunk_tuples: None,
            pipeline_kind: GpuPipelineKind::default(),
            wavefront_pipelines: HashMap::new(),
            poisoned: false,
            turn_in_progress: false,
            surface: GpuSurfaceScene::default(),
            live: LiveState::default(),
            optics_tables: OpticsTablesGpu::default(),
            tables_in_use: false,
            tray_shadow: super::tray_shadow_buffer::TrayShadowGpu::default(),
            estimator: None,
            staged: super::staged::StagedState::default(),
            sorted: super::sorted::SortedState::default(),
        })
    }

    /// Sets the surface finish of every LATER frame: the per-facet surfaces (indexed like the
    /// planes), the faceting machine the scratch directions come from, and the abraded edge.
    /// The GPU twin of passing a `SurfaceScene` to the CPU tracer; an ideal scene (the
    /// default) leaves a frame byte-identical to one without the feature. Takes effect on the
    /// next `accumulate` call, which re-encodes the material and the facet words.
    pub fn set_surface_scene(
        &mut self,
        scene: &crate::optics::raytracer::surface::SurfaceScene<'_>,
    ) {
        self.surface = GpuSurfaceScene::from_scene(scene);
    }

    /// Chooses the estimator of every LATER frame at the partial Fresnel events: the GPU twin
    /// of `PreparedMaterial::with_estimator`. `None` (the default) follows the process default,
    /// `optics::raytracer::default_estimator`, which the applications and the command line set at
    /// start-up.
    /// [`Estimator::ExitCollection`] runs on the megakernel and its live, table and
    /// tray-shadow variants, for a convex stone over an ideal surface without a
    /// light-sampling technique (`GpuTransportParams::with_estimator`); every other frame, the
    /// bounce wavefront, the path cache's passes, a beam residual pass and every frame with a
    /// Tray Reflection (a collected leaf cannot follow a floor bounce) run
    /// [`Estimator::Branching`].
    pub const fn set_estimator(&mut self, estimator: Option<Estimator>) {
        self.estimator = estimator;
    }

    /// The estimator the next frame asks for when the request names none: this renderer's own
    /// choice, else the process default. Only the megakernel implements exit collection.
    /// Native frames go through [`Self::frame_estimator_for`] with the request's own estimator.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn frame_estimator(&self) -> Estimator {
        self.frame_estimator_for(None)
    }

    /// The estimator a frame asks for: the request's own (`asked`) wins over this renderer's
    /// choice, which wins over the process default; see [`resolve_estimator`].
    pub(super) fn frame_estimator_for(&self, asked: Option<Estimator>) -> Estimator {
        resolve_estimator(
            asked,
            self.estimator,
            crate::optics::raytracer::default_estimator(),
            self.pipeline_kind,
        )
    }

    /// Overrides the per-dispatch output-buffer budget (default [`CHUNK_BUDGET_BYTES`]).
    ///
    /// Exists so a check can force a frame through many small chunks and confirm the
    /// result is identical to the single-chunk one -- see
    /// [`super::run_chunk_equivalence`]. Lowering it trades more dispatches for less
    /// peak VRAM; it never changes what a frame renders.
    pub const fn set_chunk_budget_bytes(&mut self, bytes: usize) {
        self.chunk_budget_bytes = bytes;
    }

    /// Selects which kernel every LATER chunk dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. Takes effect from the next
    /// `dispatch_chunk` call onward; a chunk already dispatched (or in flight in the
    /// overlapped pipeline) is unaffected. The accumulate entry points
    /// (`Self::accumulate`, `Self::accumulate_cancellable`) take no kind argument; the kind is
    /// an opt-in setter.
    ///
    /// Checks device limits here. Pipeline compilation is deferred until scene preparation,
    /// when the complete material/feature key is known.
    pub fn set_pipeline_kind(&mut self, kind: GpuPipelineKind) {
        let device_limits = self.ctx.device.limits();
        let available = device_limits.max_storage_buffers_per_shader_stage;
        // A device can grant enough storage bindings and still fall short of the combined
        // uniform+storage+acceleration-structure count that `wavefront_bounce`'s derived
        // pipeline layout needs (see `WAVEFRONT_BOUNCE_TOTAL_BUFFERS`). Checked here, before
        // `WavefrontPipelines::new` can fail with "Unable to derive an implicit layout", so
        // such a device falls back to the megakernel.
        let available_total =
            device_limits.max_buffers_and_acceleration_structures_per_shader_stage;
        if kind == GpuPipelineKind::Wavefront
            && (available < WAVEFRONT_STORAGE_BUFFERS
                || available_total < WAVEFRONT_BOUNCE_TOTAL_BUFFERS)
        {
            tracing::warn!(
                available,
                needed = WAVEFRONT_STORAGE_BUFFERS,
                available_total,
                needed_total = WAVEFRONT_BOUNCE_TOTAL_BUFFERS,
                "device cannot bind enough buffers for the wavefront pipeline; staying on \
                 the megakernel"
            );
            self.pipeline_kind = GpuPipelineKind::Megakernel;
            return;
        }
        self.pipeline_kind = kind;
    }

    /// Builds the selected renderer's pipelines before immutable chunk dispatch begins.
    pub(super) fn ensure_transport_pipeline(&mut self, class: u32) {
        if self.pipeline_kind == GpuPipelineKind::Wavefront {
            self.wavefront_pipelines
                .entry(class)
                .or_insert_with(|| WavefrontPipelines::new(&self.ctx.device, class));
        } else {
            self.ensure_specialized_pipeline(class);
        }
    }

    /// Which kernel `Self::dispatch_chunk` currently dispatches through -- see
    /// [`Self::set_pipeline_kind`].
    #[must_use]
    pub const fn pipeline_kind(&self) -> GpuPipelineKind {
        self.pipeline_kind
    }

    /// Human-readable adapter name and backend, for logging and for telling the user
    /// which device is actually rendering.
    #[must_use]
    pub fn adapter_label(&self) -> &str {
        &self.adapter_label
    }

    /// Drops both persistent staging slots and marks this renderer unusable for good.
    ///
    /// A staging buffer `compute::finish_map_read_into` failed to finish reading (a
    /// poll timeout, a `recv_timeout` failure, or a `get_mapped_range` error -- see that
    /// function's `# Errors`) is left in wgpu's `Waiting`/`Active` map state forever:
    /// calling `unmap()` on it from here would itself error (a buffer's map state only
    /// clears via ITS OWN pending callback resolving, not an unrelated call site), and
    /// the next `dispatch_chunk` that reuses the same slot would submit a write into a
    /// still-mapped buffer, which panics with the exact
    /// `"transport out xyz staging (persistent)" ... still mapped` text wgpu produces
    /// for a buffer still mapped when reused. Dropping the buffer instead is legal in
    /// wgpu 30 even mid-map (the
    /// pending callback is simply never delivered) and immediately releases the GPU
    /// allocation.
    ///
    /// Never rebuilds the dropped slots: `self.poisoned = true` here is permanent for
    /// this renderer (no method ever clears it) exactly because a renderer this was
    /// called on has no trustworthy way to know how much of a chunk's work actually
    /// completed before the failure -- matching [`GpuFrameError::DeviceLost`]'s "stop
    /// using this renderer" contract. `renderer::gpu_backend::GpuBackend` recovers by
    /// replacing the whole renderer after a cool-down, never by reviving this one.
    ///
    /// `pub(crate)`: lets this crate's own `#[cfg(all(test, feature =
    /// "gpu"))]` hardware tests simulate "a previous chunk readback already failed"
    /// directly, without needing to actually wedge a real GPU dispatch to reach this
    /// state -- see `renderer::gpu::frame`'s test module.
    pub(crate) fn abandon_in_flight(&mut self) {
        self.staging = [None, None];
        self.poisoned = true;
    }

    /// Grows the cached output buffers to hold at least `tuples`, reusing them when
    /// already large enough (the common case: a progressive render re-dispatches the same
    /// resolution until the camera moves). Grows BOTH double-buffered slots identically,
    /// since a chunk can land in either.
    pub(super) fn ensure_capacity(&mut self, tuples: usize) {
        for slot in &mut self.outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= tuples);
            if !big_enough {
                *slot = Some(TransportOutputs::new_production(&self.ctx.device, tuples));
            }
        }
    }

    /// Grows the persistent staging buffers to hold at least `pixels` pixels' worth of
    /// (GPU-reduced) XYZ floats, reusing them when already large enough. Mirrors
    /// [`Self::ensure_capacity`]'s policy for `outputs` -- grow both slots identically,
    /// never shrink -- via the same pure predicate, [`staging_needs_growth`]. Sized in
    /// PIXELS, not (pixel, sample) tuples, since `Self::dispatch_chunk`'s readback copy
    /// reads from [`PixelXyzOutput`], not `TransportOutputs::xyz`
    /// directly -- see [`StagingSlot`]'s own doc comment.
    pub(super) fn ensure_staging_capacity(&mut self, pixels: usize) {
        for slot in &mut self.staging {
            let current_capacity = slot.as_ref().map_or(0, |s| s.capacity);
            if staging_needs_growth(current_capacity, pixels) {
                *slot = Some(StagingSlot::new(
                    &self.ctx.device,
                    "transport out xyz staging (persistent)",
                    pixels,
                ));
            }
        }
    }

    /// Grows the cached [`PixelXyzOutput`] buffers to hold at least `pixels` pixels,
    /// reusing them when already large enough. Mirrors [`Self::ensure_capacity`]'s policy
    /// for `outputs` exactly, just sized in pixels rather than (pixel, sample) tuples.
    pub(super) fn ensure_pixel_capacity(&mut self, pixels: usize) {
        for slot in &mut self.pixel_outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= pixels);
            if !big_enough {
                *slot = Some(PixelXyzOutput::new(&self.ctx.device, pixels));
            }
        }
    }

    /// Creates both persistent `GpuTransportParams` uniform buffers on first use --
    /// see [`Self::params_buffers`]'s doc comment. Fixed-size (the struct never changes
    /// size), so unlike [`Self::ensure_capacity`] this only ever creates, never grows.
    pub(super) fn ensure_params_buffers(&mut self) {
        for slot in &mut self.params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("transport params (persistent)"),
                    size: size_of::<crate::renderer::buffers::GpuTransportParams>()
                        as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }

    /// Like [`Self::ensure_params_buffers`], for [`Self::reduce_params_buffers`].
    pub(super) fn ensure_reduce_params_buffers(&mut self) {
        for slot in &mut self.reduce_params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("reduce xyz params (persistent)"),
                    size: size_of::<GpuReduceParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }
}
