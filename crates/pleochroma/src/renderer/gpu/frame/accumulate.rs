//! The per-frame accumulate loop: [`GpuFrameRenderer::accumulate`]/
//! [`GpuFrameRenderer::accumulate_cancellable`] and the turn-granular machinery
//! (`accumulate_turn`/`prepare_turn`/`run_turn_chunks`) `renderer::gpu_backend::GpuBackend`
//! drives for fairness across concurrent requests. Per-chunk GPU dispatch itself lives in
//! [`super::dispatch`]; this module is the loop that decides WHEN to dispatch a chunk and
//! sums its result.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;

use crate::{
    geometry::tool::{ToolPrimitive, pack_planes_and_tools},
    optics::raytracer::environment::environment_white_balance,
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial},
        gpu::timing::{self, TurnSlot},
    },
};

use super::{
    AccumulateOutcome, ChunkCursor, ChunkFrameState, ChunkSlotResources, ChunkTurnOutcome,
    GpuFrameError, GpuFrameScene, GpuPipelineKind, PendingChunk, TurnRequest, TurnSetup,
    bind_groups::wavefront_pixel_cap,
    classify_material,
    dispatch::{chunk_pixels_for, environment_params},
    renderer::{GpuFrameRenderer, collect_applies, resolve_estimator},
    scene_buffers::{FrameSceneBuffers, SceneBufferInputs},
};

impl GpuFrameRenderer {
    /// Traces `spp` samples per pixel, starting at sample index `sample_offset`, and
    /// ADDS each pixel's summed XYZ into `accum`.
    ///
    /// Accumulating rather than overwriting mirrors the CPU path's
    /// `acc_chunk[i] += sample_sum`, so a caller's progressive-accumulation buffer and its
    /// sample counter mean the same thing regardless of backend. `sample_offset` must be
    /// the number of samples already in `accum` for these pixels: the shader derives each
    /// thread's seed and stratified jitter from the absolute sample index, so reusing an
    /// offset would re-draw identical samples and bias the average.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if the scene uses an environment the
    /// megakernel has no `env_mode` for -- currently unreachable (see that variant's own
    /// doc comment; HDR maps render on the GPU).
    /// [`GpuFrameError::TrayVariant`] if the scene carries a Stone Shadow or a Tray Reflection
    /// this renderer cannot bind (the legacy wavefront, or a device short of buffer bindings);
    /// nothing is written.
    /// [`GpuFrameError::DeviceLost`] if the device stops making forward progress
    /// mid-frame -- see that variant's own doc comment; `renderer::gpu_backend::GpuBackend`
    /// is the caller expected to react to it by suspending GPU rendering, then replacing
    /// this renderer once its recovery cool-down has passed.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `width * height`.
    pub fn accumulate(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        // classify_material makes the class decision. accumulate_via_pipeline takes the class
        // as a parameter so run_specialisation_equivalence can force other classes. No cancel
        // flag: this entry point never stops early.
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(scene, pipeline_class, sample_offset, spp, accum, None)?;
        Ok(())
    }

    /// Like [`Self::accumulate`], but stops between chunks once `cancel` is set.
    ///
    /// `cancel` is polled between chunks, never mid-chunk. Chunks already summed stay in `accum`;
    /// the chunk in flight is waited on but not summed (see [`AccumulateOutcome::Cancelled`]).
    /// Used by `renderer::gpu_backend::GpuBackend::try_accumulate_cancellable` so that an
    /// abandoned request, such as a disconnected client, frees the GPU.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub fn accumulate_cancellable(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(
            scene,
            pipeline_class,
            sample_offset,
            spp,
            accum,
            Some(cancel),
        )
    }

    /// [`Self::accumulate`] for a concave stone: the stone is `scene.planes` minus `tools`.
    ///
    /// The tool records ride in the `planes` storage binding after the planes (no extra
    /// binding, so the storage-buffer budget is unchanged); `tools` empty is exactly
    /// [`Self::accumulate`]. `scene.facet_finishes` is indexed by facet id, tool `k` being
    /// `scene.planes.len() + k`. At most `GPU_MAX_TOOL_PRIMITIVES` tools: route with
    /// `renderer::gpu_backend::scene_routes_to_gpu_with_tools` first.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `width * height`.
    pub fn accumulate_with_tools(
        &mut self,
        scene: &GpuFrameScene<'_>,
        tools: &[ToolPrimitive],
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline_geom(
            scene,
            tools,
            pipeline_class,
            sample_offset,
            spp,
            accum,
            None,
        )?;
        Ok(())
    }

    /// The body behind [`Self::accumulate`] and [`Self::accumulate_cancellable`].
    ///
    /// The pipeline class (one of `super::material_class`'s values) is passed in rather than
    /// derived from `scene.material`. [`super::equivalence::run_specialisation_equivalence`]
    /// uses this to force the GENERIC pipeline for a material that [`classify_material`] would
    /// route elsewhere, and the reverse, which is what shows the pipelines agree.
    ///
    /// Drives [`Self::accumulate_turn`] with an unlimited chunk budget, so the request runs
    /// to `Done` or `Cancelled` without yielding the renderer. `cancel` is `None` for every
    /// self-test and for [`Self::accumulate`].
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub(super) fn accumulate_via_pipeline(
        &mut self,
        scene: &GpuFrameScene<'_>,
        pipeline_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: Option<&AtomicBool>,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        self.accumulate_via_pipeline_geom(
            scene,
            &[],
            pipeline_class,
            sample_offset,
            spp,
            accum,
            cancel,
        )
    }

    /// [`Self::accumulate_via_pipeline`] for a concave stone: `tools` are the stone's
    /// carving tools (`&[]` for a convex one, which is exactly the planar path).
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    #[expect(
        clippy::too_many_arguments,
        reason = "the planar entry point's parameter list plus the tool slice; bundling them would re-introduce TurnRequest one level removed"
    )]
    pub(super) fn accumulate_via_pipeline_geom(
        &mut self,
        scene: &GpuFrameScene<'_>,
        tools: &[ToolPrimitive],
        pipeline_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: Option<&AtomicBool>,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let mut cursor = ChunkCursor::default();
        let request = TurnRequest {
            scene,
            tools,
            pipeline_class,
            sample_offset,
            spp,
            cancel,
            max_chunks: usize::MAX,
            surface: None,
            estimator: None,
        };
        loop {
            match self.accumulate_turn(&request, accum, &mut cursor)? {
                ChunkTurnOutcome::Done => return Ok(AccumulateOutcome::Done),
                ChunkTurnOutcome::Cancelled => return Ok(AccumulateOutcome::Cancelled),
                // Unreachable with an unlimited turn budget. Matched rather than
                // `unreachable!()` so this driver stays correct if that budget changes.
                ChunkTurnOutcome::MoreWork => {}
            }
        }
    }

    /// One fairness turn: dispatches up to `request.max_chunks` chunks, advancing `cursor`.
    ///
    /// Returns with no GPU work in flight (see [`ChunkTurnOutcome::MoreWork`]).
    ///
    /// Every turn, resumed or not, re-verifies and re-uploads `self.scene_buffers` via
    /// [`FrameSceneBuffers::ensure`] (see [`Self::prepare_turn`]). Another request's turn may
    /// have overwritten those persistent buffers in between, and this is the only way to know.
    ///
    /// Split into [`Self::prepare_turn`] and [`Self::run_turn_chunks`] to stay under clippy's
    /// function-length limit. [`TurnRequest`] bundles the arguments shared with
    /// [`Self::accumulate_via_pipeline`] to stay under the argument-count limit.
    ///
    /// A thin poisoning wrapper around [`Self::accumulate_turn_body`], so no early return in
    /// the body needs to clear `self.turn_in_progress`. A poisoned renderer declines at once.
    /// `turn_in_progress` still set on entry means a previous call unwound mid-turn, which can
    /// leave a staging buffer mapped; see that field's doc comment.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s, plus [`GpuFrameError::DeviceLost`] if
    /// this renderer is (or just became) poisoned.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub(crate) fn accumulate_turn(
        &mut self,
        request: &TurnRequest<'_>,
        accum: &mut [Vec3],
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
        timing::begin_turn();
        let result = self.accumulate_turn_body(request, accum, cursor);
        timing::end_turn();
        self.turn_in_progress = false;
        if matches!(&result, Err(GpuFrameError::DeviceLost(_))) {
            self.abandon_in_flight();
        }
        result
    }

    /// The body [`Self::accumulate_turn`] wraps with poisoning checks -- see that
    /// function's own doc comment for why the split exists.
    fn accumulate_turn_body(
        &mut self,
        request: &TurnRequest<'_>,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        if request.spp == 0 || num_pixels == 0 {
            return Ok(ChunkTurnOutcome::Done);
        }

        // GemMaterial::gpu_supported is the crate's routing predicate; a caller assembling
        // a full scene must consult it before routing to the GPU. Currently unreachable
        // (unconditionally true), but enforced so a future material kind the megakernel
        // cannot handle produces an error here rather than a plausible-looking wrong image.
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }

        let mut prepare_mark = timing::Mark::now();
        let TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        } = self.prepare_turn(request)?;
        prepare_mark.lap_turn(TurnSlot::Prepare);

        // frame_buffers is passed by value so run_turn_chunks can borrow self mutably. On an
        // Err it is not put back: a DeviceLost discards this renderer, and an early error has
        // already taken the buffers out, so the next call just rebuilds them.
        let (frame_buffers, cancelled) = self.run_turn_chunks(
            request,
            &state,
            frame_buffers,
            byte_budget_pixels,
            accum,
            cursor,
        )?;

        // Put the scene buffers back for the next turn, from this request or another.
        self.scene_buffers = Some(frame_buffers);

        self.check_uncaptured_error()?;

        if cancelled {
            return Ok(ChunkTurnOutcome::Cancelled);
        }
        if cursor.first_pixel >= num_pixels {
            return Ok(ChunkTurnOutcome::Done);
        }
        Ok(ChunkTurnOutcome::MoreWork)
    }

    /// The setup [`Self::accumulate_turn`] redoes on every turn before dispatching a chunk.
    /// Split out to keep `accumulate_turn` under clippy's function-length limit.
    ///
    /// Fallible: [`FrameSceneBuffers::ensure`] can fail to (re)build the HDR environment
    /// buffers for an oversized map. Every `EnvironmentSource` maps to an `env_mode` (see
    /// [`environment_params`]). [`GpuFrameError::UnsupportedMaterial`] is checked by
    /// [`Self::accumulate_turn_body`] before this is called.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] -- see [`FrameSceneBuffers::ensure`]'s
    /// `# Errors`. [`GpuFrameError::TrayVariant`] -- the scene carries a Stone Shadow or a Tray
    /// Reflection this renderer cannot bind (the legacy wavefront, or a device short of buffer
    /// bindings). [`GpuFrameError::LegacyWavefrontCollect`] -- see [`Self::turn_estimator`].
    pub(super) fn prepare_turn<'a>(
        &mut self,
        request: &TurnRequest<'a>,
    ) -> Result<TurnSetup<'a>, GpuFrameError> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;

        let packed = pack_planes_and_tools(scene.planes, request.tools);

        let (
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            use_d65,
            studio_model,
            backdrop,
        ) = environment_params(scene.environment);
        // The per-preset adaptation the CPU tracer applies (identity for an HDR map and
        // for the lit D65 models, Planckian for the rest), so a hybrid frame's CPU and
        // GPU tiles agree.
        let white_balance = environment_white_balance(scene.environment);

        let surface = request.surface.unwrap_or(&self.surface);
        let gpu_material = surface.apply_to_material(GpuGemMaterial::encode(scene.material));
        // One entry per packed `planes` entry, not per facet: the binding sizes `planes` and
        // `facet_finishes` from the same length, and a facet id (planes, then tools) only
        // ever indexes the first `planes + tools` of them -- the rest are never read.
        let gpu_finishes = surface.encode_finishes(scene.facet_finishes, packed.len());

        // Exit collection needs the surface the CPU tracer calls ideal; the estimator is the
        // request's, else the renderer's. Both are part of what the frame costs (`note_workload`).
        let ideal_surface = surface.is_ideal_for_cpu();
        let estimator = self.turn_estimator(request, env_mode, studio_model, ideal_surface)?;
        self.note_workload(request, &packed, &gpu_material, &gpu_finishes, estimator);

        let pipeline_class = self.prepare_turn_pipeline(request, &gpu_material)?;

        // byte_budget_pixels is chunk_budget_bytes's hard ceiling in pixels -- output
        // buffers are sized against it ONCE, up front, so every dispatch below fits
        // without growing buffers mid-frame. See next_chunk_pixels for why the actual
        // per-chunk pixel count is computed fresh every iteration instead.
        let byte_budget_pixels = chunk_pixels_for(self.chunk_budget_bytes, request.spp, num_pixels);
        // chunk_pixels_for sizes its ceiling for the XYZ-only output (12 bytes per tuple), not
        // for the wavefront's larger per-tuple footprint. See cap_byte_budget_for_wavefront.
        let byte_budget_pixels =
            self.cap_byte_budget_for_wavefront(byte_budget_pixels, request.spp);
        self.ensure_capacity(byte_budget_pixels * request.spp as usize);
        self.ensure_pixel_capacity(byte_budget_pixels);
        self.ensure_staging_capacity(byte_budget_pixels);
        self.ensure_params_buffers();
        self.ensure_reduce_params_buffers();

        // The camera, material, geometry and facet-finish buffers persist across chunks and
        // calls (see FrameSceneBuffers::ensure). camera_params carries the full frame's size,
        // not a chunk's.
        let camera_params = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: request.spp,
        };
        // Taken out of `self.scene_buffers` and returned by value, so dispatch_chunk and
        // drain_pending_chunk can borrow `self` freely. The caller puts it back once the turn
        // is done. Called on every turn (see `accumulate_turn`).
        let mut upload_mark = timing::Mark::now();
        let frame_buffers = FrameSceneBuffers::ensure(
            self.scene_buffers.take(),
            &self.ctx,
            &SceneBufferInputs {
                camera_params: &camera_params,
                material: &gpu_material,
                planes: &packed,
                facet_finishes: &gpu_finishes,
                environment: scene.environment,
            },
        )?;
        upload_mark.lap_turn(TurnSlot::SceneUpload);

        self.check_uncaptured_error()?;
        let state = ChunkFrameState {
            scene,
            packed_len: packed.len(),
            tool_count: u32::try_from(request.tools.len())
                .expect("a stone's tool count is bounded far below u32::MAX"),
            pipeline_class,
            sample_offset: request.sample_offset,
            spp: request.spp,
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            white_balance,
            use_d65,
            studio_model,
            backdrop,
            estimator,
            ideal_surface,
        };

        Ok(TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        })
    }

    /// The estimator this turn's frame asks for: the request's own, else the renderer's, else the
    /// process default ([`Self::frame_estimator_for`]). A Tray Reflection turns exit collection
    /// off on the GPU as on the CPU (a collected leaf cannot follow a floor bounce), so such a
    /// frame asks for the branching estimator. Part of [`Self::prepare_turn`].
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LegacyWavefrontCollect`] when the frame would collect (the request's
    /// own estimator, else the renderer's, else the process default, which the apps set to
    /// exit collection), where collection applies to it, on a renderer running the legacy
    /// bounce wavefront, which never collects: the frame is traced elsewhere (the CPU collects)
    /// rather than with the other estimator.
    fn turn_estimator(
        &self,
        request: &TurnRequest<'_>,
        env_mode: u32,
        studio_model: u32,
        ideal_surface: bool,
    ) -> Result<crate::optics::raytracer::Estimator, GpuFrameError> {
        let tray_mirror = request.scene.environment.tray_mirror().is_some();
        // What the frame would run on an engine that collects: the precedence of
        // `resolve_estimator` without the legacy wavefront's own answer.
        let collect_asked = resolve_estimator(
            request.estimator,
            self.estimator,
            crate::optics::raytracer::default_estimator(),
            GpuPipelineKind::Megakernel,
        ) == crate::optics::raytracer::Estimator::ExitCollection;
        if collect_asked
            && matches!(self.pipeline_kind, GpuPipelineKind::Wavefront)
            && collect_applies(
                env_mode,
                studio_model,
                ideal_surface,
                request.tools.len(),
                tray_mirror,
            )
        {
            return Err(GpuFrameError::LegacyWavefrontCollect);
        }
        Ok(if tray_mirror {
            crate::optics::raytracer::Estimator::Branching
        } else {
            self.frame_estimator_for(request.estimator)
        })
    }

    /// The pipeline class of this turn, with its resources ready: the class as this renderer can
    /// run it (the optics-table bit dropped when it cannot, the tray bits set when the scene
    /// carries a Stone Shadow or a Tray Reflection), the tray and table buffers prepared and the
    /// transport pipeline built. Part of [`Self::prepare_turn`].
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::TrayVariant`] when this renderer cannot bind the scene's Stone Shadow or
    /// Tray Reflection; [`GpuFrameError::DeviceLost`] when the device reported an error.
    fn prepare_turn_pipeline(
        &mut self,
        request: &TurnRequest<'_>,
        gpu_material: &GpuGemMaterial,
    ) -> Result<u32, GpuFrameError> {
        let scene = request.scene;
        let mut pipeline_class = self.effective_class(
            request.pipeline_class
                & !(super::material_class::TRAY_SHADOW | super::material_class::TRAY_MIRROR),
        );
        let shadow = scene.environment.tray_shadow();
        let mirror = scene.environment.tray_mirror();
        if shadow.is_some() {
            pipeline_class |= super::material_class::TRAY_SHADOW;
        }
        if mirror.is_some() {
            pipeline_class |= super::material_class::TRAY_MIRROR;
        }
        if super::material_class::uses_tray_variant(pipeline_class)
            && !self.supports_tray_shadow_class(pipeline_class, false)
        {
            return Err(GpuFrameError::TrayVariant);
        }
        self.prepare_tray_shadow(shadow, mirror);
        self.prepare_tables(pipeline_class, scene.material, gpu_material);

        self.check_uncaptured_error()?;
        self.ensure_transport_pipeline(pipeline_class);
        self.check_uncaptured_error()?;
        Ok(pipeline_class)
    }

    /// Extra ceiling on `byte_budget_pixels` for [`GpuPipelineKind::Wavefront`] only; a
    /// no-op for [`GpuPipelineKind::Megakernel`].
    ///
    /// The wavefront's widest per-chunk buffer, `stokes`, takes 128 bytes per (pixel, sample)
    /// tuple, while [`chunk_pixels_for`] budgets 12 bytes per tuple. Uncapped, a chunk near the
    /// dispatch limit would request about 537 MB for `stokes` alone, above the WebGPU baseline
    /// `max_storage_buffer_binding_size` of 128 MiB, and `create_buffer` would reject it. The
    /// cap prevents that size error instead of relying on the uncaptured-error handler (see
    /// `GpuContext::acquire_async`).
    ///
    /// The limit is read from the device, since adapters can advertise more or less than the
    /// baseline.
    pub(super) fn cap_byte_budget_for_wavefront(
        &self,
        byte_budget_pixels: usize,
        spp: u32,
    ) -> usize {
        if self.pipeline_kind != GpuPipelineKind::Wavefront {
            return byte_budget_pixels;
        }
        // Saturating: a binding limit beyond the address space is effectively unlimited.
        let max_binding_bytes =
            usize::try_from(self.ctx.device.limits().max_storage_buffer_binding_size)
                .unwrap_or(usize::MAX);
        wavefront_pixel_cap(max_binding_bytes, spp, byte_budget_pixels)
    }

    /// The dispatch/drain loop body of [`Self::accumulate_turn`]: dispatches at most
    /// `request.max_chunks` chunks starting from `cursor`'s progress, then drains
    /// whatever chunk the overlapped pipeline may still have in flight before returning
    /// -- see [`ChunkTurnOutcome::MoreWork`]'s doc comment for why that final drain is
    /// required regardless of how this loop ends. Updates `cursor` in place and returns
    /// `frame_buffers` back to the caller (to put back into `self.scene_buffers`)
    /// alongside whether `request.cancel` fired during this turn. Split out of
    /// `accumulate_turn` purely to keep that function under clippy's function-length
    /// limit.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    fn run_turn_chunks(
        &mut self,
        request: &TurnRequest<'_>,
        state: &ChunkFrameState<'_>,
        frame_buffers: FrameSceneBuffers,
        byte_budget_pixels: usize,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<(FrameSceneBuffers, bool), GpuFrameError> {
        let num_pixels = state.scene.width as usize * state.scene.height as usize;

        // Each chunk is dispatched before the previous one is drained, so the GPU has the next
        // chunk queued while the CPU waits on the previous readback. Starts from `cursor`'s
        // progress, so a resumed turn continues where the last one stopped.
        let mut pending: Option<PendingChunk> = None;
        let mut first_pixel = cursor.first_pixel;
        let mut chunk_index = cursor.chunk_index;
        let mut cancelled = false;
        let mut chunks_this_turn = 0usize;
        while first_pixel < num_pixels {
            self.check_uncaptured_error()?;
            if request
                .cancel
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                cancelled = true;
                break;
            }
            if chunks_this_turn >= request.max_chunks {
                // This turn's budget is spent -- stop dispatching new chunks, but a
                // chunk already dispatched this turn (if any) is still drained below
                // exactly like every other exit from this loop, so nothing is ever left
                // pending across a yielded turn.
                break;
            }

            let chunk_pixels = Self::next_chunk_pixels(
                self.ns_per_tuple_ema,
                self.growth_limited_pixels(state.spp, byte_budget_pixels),
                state.spp,
                num_pixels,
            );
            let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);
            // The sorted wavefront's or the staged megakernel's pipelines and buffers for this
            // chunk (a no-op while both switches are off).
            self.ensure_chunk_engine(state, pixels_this_chunk);

            // Dispatch, then drain: chunk i+1 is queued before this call blocks on chunk i's
            // readback inside `drain_pending_chunk`.
            let new_pending = self.dispatch_chunk(
                state,
                &frame_buffers,
                first_pixel,
                pixels_this_chunk,
                chunk_index,
            )?;
            if let Some(prev) = pending.take() {
                // Chunk i+1 is already queued. If this drain fails, abandon both slots now:
                // draining chunk i+1 could fail or hang too, and a later chunk reusing chunk
                // i's slot would submit into a still-mapped buffer.
                if let Err(err) = self.drain_pending_chunk(prev, state.spp, Some(&mut *accum)) {
                    self.abandon_in_flight();
                    return Err(err);
                }
            }
            pending = Some(new_pending);

            first_pixel += pixels_this_chunk;
            chunk_index += 1;
            chunks_this_turn += 1;
        }

        // The chunk still in flight is drained here whatever ends the turn (done, cancelled or
        // out of chunk budget), so no GPU work outlives the turn (see
        // `ChunkTurnOutcome::MoreWork`). A cancelled chunk is waited on but not summed (see
        // `AccumulateOutcome::Cancelled`). Every other exit keeps its samples.
        if let Some(chunk) = pending.take() {
            let mut tail_mark = timing::Mark::now();
            let drain_result = if cancelled {
                self.drain_pending_chunk(chunk, state.spp, None)
            } else {
                self.drain_pending_chunk(chunk, state.spp, Some(&mut *accum))
            };
            tail_mark.lap_turn(TurnSlot::TailDrain);
            // As above: a failed drain leaves a staging slot mapped, so the renderer must not
            // dispatch again.
            if let Err(err) = drain_result {
                self.abandon_in_flight();
                return Err(err);
            }
        }

        cursor.first_pixel = first_pixel;
        cursor.chunk_index = chunk_index;

        Ok((frame_buffers, cancelled))
    }

    /// Resolves the four per-slot GPU resources [`super::GpuFrameRenderer::dispatch_chunk`]
    /// needs for one chunk (bundled as [`ChunkSlotResources`]), asserting each is large
    /// enough for this dispatch. Split out of `dispatch_chunk` purely to keep that method
    /// under clippy's function-length limit.
    pub(super) fn resolve_chunk_slot_resources(
        &self,
        staging_slot: usize,
        tuples: usize,
        pixels_this_chunk: usize,
    ) -> ChunkSlotResources<'_> {
        let outputs = self.outputs[staging_slot]
            .as_ref()
            .expect("ensure_capacity just populated both slots");
        assert!(
            tuples <= outputs.capacity,
            "dispatch of {tuples} tuples exceeds output capacity {}",
            outputs.capacity
        );

        let pixel_output = self.pixel_outputs[staging_slot]
            .as_ref()
            .expect("ensure_pixel_capacity just populated both slots");
        assert!(
            pixels_this_chunk <= pixel_output.capacity,
            "reduced-pixel dispatch of {pixels_this_chunk} pixels exceeds pixel output \
             capacity {}",
            pixel_output.capacity
        );

        let staging_buffer = &self.staging[staging_slot]
            .as_ref()
            .expect("ensure_staging_capacity just populated both slots")
            .buffer;

        let params_buf = self.params_buffers[staging_slot]
            .as_ref()
            .expect("ensure_params_buffers just populated both slots");

        ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        }
    }
}
