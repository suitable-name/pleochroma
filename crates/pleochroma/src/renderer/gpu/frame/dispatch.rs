//! Per-chunk GPU dispatch of the frame renderer.
//!
//! It holds [`encode_and_dispatch`] (the blocking self-test path shared with
//! `estimator_check::dispatch_transport`), [`GpuFrameRenderer::dispatch_chunk`]'s pipelined
//! production dispatch through either kernel shape (the megakernel, or the wavefront for
//! [`super::GpuPipelineKind::Wavefront`]), and the pure chunk-sizing arithmetic
//! ([`chunk_pixels_for`], [`GpuFrameRenderer::next_chunk_pixels`], [`environment_params`]).

use std::time::Instant;

use crate::{
    optics::raytracer::{EnvironmentSource, head_shadow_cosines, illuminant_temperature_k},
    renderer::{
        buffers::{GpuTransportParams, GpuWavefrontParams, transport_env_mode},
        gpu::{
            compute,
            timing::{self, Slot},
        },
    },
};

use super::{
    ChunkFrameState, ChunkSlotResources, FIRST_DISPATCH_MAX_TUPLES, FLOATS_PER_TUPLE,
    GpuPipelineKind, MAX_WORKGROUPS_PER_DIMENSION, MIN_CHUNK_BYTES, PendingChunk, TARGET_CHUNK_MS,
    WORKGROUP_SIZE,
    bind_groups::{
        TransportDispatchArgs, WavefrontPipelines, WavefrontRoundBindGroups, build_bind_group,
        build_chunk_bind_group, build_wavefront_round_bind_groups, exclusive_prefix_sum,
        wavefront_generate_bindings,
    },
    readback::copy_to_existing_staging,
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, TransportOutputs, WavefrontRayBuffers},
};

/// A chunk's pixel, tuple or workgroup-thread count as the `u32` the shaders and dispatch
/// arithmetic use. The GPU buffer limits cap every such count far below `u32::MAX`, so an
/// overflow here is a bug, not an input.
pub(super) fn count_u32(count: usize) -> u32 {
    u32::try_from(count).expect("a chunk's pixel and tuple counts fit u32")
}

/// Workgroups of [`WORKGROUP_SIZE`] threads covering `count` `u32` items.
pub(super) fn workgroups_for_u32(count: u32) -> u32 {
    count.div_ceil(count_u32(WORKGROUP_SIZE))
}

/// Workgroups of [`WORKGROUP_SIZE`] threads covering `count` items.
pub(super) fn workgroups_for(count: usize) -> u32 {
    workgroups_for_u32(count_u32(count))
}

/// Uploads the scene inputs, binds every buffer, and dispatches `transport_main` over
/// `total_tuples` threads, then BLOCKS until it finishes.
///
/// The blocking megakernel dispatch used by `estimator_check::dispatch_transport` and the
/// other self-tests in `renderer::gpu`, so their equivalence checks run the exact binding code
/// the renderer ships. [`GpuFrameRenderer::dispatch_chunk`]'s pipelined path uses
/// [`build_chunk_bind_group`] instead.
pub fn encode_and_dispatch(args: &TransportDispatchArgs<'_>, total_tuples: usize) {
    assert!(
        total_tuples <= args.outputs.capacity,
        "dispatch of {total_tuples} tuples exceeds output capacity {}",
        args.outputs.capacity
    );
    let bind_group = build_bind_group(args);
    let workgroups = count_u32(total_tuples).div_ceil(64);
    compute::dispatch_and_wait_for(
        &args.ctx.device,
        &args.ctx.queue,
        args.pipeline,
        &bind_group,
        (workgroups, 1, 1),
        compute::SELF_TEST_WAIT_TIMEOUT,
    );
}

/// Builds one chunk's `GpuTransportParams`, split out of [`GpuFrameRenderer::dispatch_chunk`]
/// to keep that method under clippy's function-length limit. The caller writes the result into
/// the chunk slot's persistent uniform buffer. The estimator is the frame's
/// (`state.estimator`), as far as `GpuTransportParams::with_estimator` allows.
pub(super) fn build_chunk_params(
    state: &ChunkFrameState<'_>,
    first_pixel: usize,
    pixels_this_chunk: usize,
) -> GpuTransportParams {
    GpuTransportParams::new(
        count_u32(pixels_this_chunk),
        state.scene.max_bounces,
        state.sample_offset,
        state.env_mode,
        0.0,
        state.temp_k,
        state.spot_mult,
        state.exposure,
        state.light_yaw,
        state.light_pitch,
        state.white_balance.to_array(),
    )
    .with_pixel_offset(count_u32(first_pixel))
    .with_tool_count(state.tool_count)
    .with_debug_buffers_disabled()
    .with_studio_use_d65(state.use_d65)
    .with_studio_model(state.studio_model)
    .with_tent_of(state.scene.environment)
    .with_hdr_of(state.scene.environment)
    .with_backdrop(state.backdrop)
    .with_surface_glare(state.scene.environment.surface_glare())
    .with_head_shadow(head_shadow_cosines(
        state.scene.environment.head_shadow_deg(),
    ))
    .with_estimator(state.estimator, state.ideal_surface)
}

impl GpuFrameRenderer {
    /// Builds and submits one chunk: the transport dispatch, the reduction and the readback copy.
    ///
    /// The returned [`PendingChunk`] is waited on later by
    /// [`super::GpuFrameRenderer::drain_pending_chunk`].
    ///
    /// # Why a second dispatch
    ///
    /// `transport_main` writes one XYZ triple per (pixel, sample) THREAD into
    /// `outputs.xyz()` -- `pixels_this_chunk * spp` triples. Copying and mapping all of
    /// that back to sum it on the CPU would scale readback with sample
    /// count: at 1080p x 8 spp, ~200MB per progressive pass. `reduce_xyz_main` instead
    /// sums each pixel's `spp` consecutive triples ON the GPU, one thread per PIXEL, into
    /// `self.pixel_outputs[staging_slot]` -- `pixels_this_chunk` triples, independent of
    /// `spp`. Only THAT smaller buffer is copied to staging below; `outputs.xyz()` itself
    /// never leaves the GPU. See `reduce_xyz.wgsl`'s header comment for the determinism
    /// argument (fixed ascending summation order, matching the order the CPU sums them
    /// in).
    ///
    /// Called once per chunk by `run_turn_chunks`. `state` bundles the per-frame constants
    /// (see [`ChunkFrameState`]).
    pub(super) fn dispatch_chunk(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        first_pixel: usize,
        pixels_this_chunk: usize,
        chunk_index: usize,
    ) -> Result<PendingChunk, super::GpuFrameError> {
        self.check_device_error()?;
        timing::reset_chunk();
        let mut total_mark = timing::Mark::now();
        let tuples = pixels_this_chunk * state.spp as usize;
        // The scene buffers in frame_buffers persist across calls, so only params is written
        // here, into this slot's persistent uniform buffer.
        let params = build_chunk_params(state, first_pixel, pixels_this_chunk);

        let staging_slot = chunk_index % 2;
        let ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        } = self.resolve_chunk_slot_resources(staging_slot, tuples, pixels_this_chunk);

        // Reusing this slot two chunks later is safe because the queue runs submissions in
        // order (see GpuFrameRenderer::params_buffers).
        let mut mark = timing::Mark::now();
        self.ctx
            .queue
            .write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
        mark.lap(Slot::ParamsWrite);

        let submitted_at = Instant::now();
        self.dispatch_transport(state, frame_buffers, params_buf, outputs, tuples)?;
        // GPU-side sample reduction (see "Why a second dispatch" in this function's doc). It
        // reads outputs.xyz(), written by the dispatch above, and writes pixel_output.buffer.
        // No barrier is needed: wgpu runs queue submissions in order. Split out to keep this
        // function under clippy's function-length limit.
        self.dispatch_reduce(
            state.spp,
            staging_slot,
            pixels_this_chunk,
            outputs,
            pixel_output,
        )?;

        self.check_device_error()?;
        let copy_index = copy_to_existing_staging::<f32>(
            &self.ctx.device,
            &self.ctx.queue,
            &pixel_output.buffer,
            staging_buffer,
            pixels_this_chunk * 3,
        );
        self.check_device_error()?;
        let mut mark = timing::Mark::now();
        let rx = compute::begin_map_read(staging_buffer);
        mark.lap(Slot::MapBegin);
        total_mark.lap(Slot::DispatchTotal);

        Ok(PendingChunk {
            staging_slot,
            copy_index,
            rx,
            first_pixel,
            pixels_this_chunk,
            submitted_at,
            workload_key: self.last_scene_key,
            timing: timing::take_dispatch(chunk_index, pixels_this_chunk, tuples),
        })
    }

    /// Submit transport only, stopping before reduction/copy on a rejected operation.
    fn dispatch_transport(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        outputs: &TransportOutputs,
        tuples: usize,
    ) -> Result<(), super::GpuFrameError> {
        self.check_device_error()?;
        // The sorted wavefront (`super::sorted`), when its switch is on and `ensure_sorted`
        // prepared this chunk; it comes before the staged megakernel.
        if let Some(plan) = self.sorted_plan_for(state, tuples) {
            self.dispatch_sorted(state, frame_buffers, params_buf, outputs, &plan);
            return self.check_device_error();
        }
        // The staged megakernel (`super::staged`), when the switch, the class and the device
        // allow it and `ensure_staged` prepared this turn; every other chunk is unchanged.
        if let Some(plan) = self.staged_plan_for(state, tuples) {
            self.dispatch_staged(state, frame_buffers, params_buf, outputs, tuples, &plan);
            return self.check_device_error();
        }
        match self.pipeline_kind {
            GpuPipelineKind::Megakernel => {
                let pipeline = self.pipeline_for_class(state.pipeline_class);
                let mut mark = timing::Mark::now();
                let bind_group = build_chunk_bind_group(
                    &self.ctx,
                    pipeline,
                    frame_buffers,
                    params_buf,
                    state.packed_len,
                    outputs,
                    (
                        self.optics_tables_for(state.pipeline_class),
                        self.tray_shadow_for(state.pipeline_class),
                    ),
                );
                mark.lap(Slot::TransportBind);
                self.check_device_error()?;
                let mut mark = timing::Mark::now();
                let _ = compute::dispatch_with(
                    &self.ctx.device,
                    &self.ctx.queue,
                    pipeline,
                    &bind_group,
                    (workgroups_for(tuples), 1, 1),
                    || mark.lap(Slot::TransportEncode),
                );
                mark.lap(Slot::TransportSubmit);
            }
            GpuPipelineKind::Wavefront => self.dispatch_chunk_wavefront(
                state,
                frame_buffers,
                params_buf,
                state.packed_len,
                outputs,
                tuples,
            ),
        }
        self.check_device_error()
    }

    /// Runs one chunk through the wavefront pipeline instead of the megakernel.
    ///
    /// `wavefront_generate` runs first, then `wavefront_bounce` and the compaction kernels once
    /// per bounce round until every ray has died or `state.scene.max_bounces` rounds have run.
    /// `wavefront_finalize_survivors` handles whatever remains. Fills `outputs.xyz()` as the
    /// megakernel would for the same chunk.
    ///
    /// Allocates new ray-state buffers on every call, sized to `tuples`, unlike the persistent
    /// `outputs` and staging buffers. The wavefront pipeline is opt-in, so it skips the
    /// megakernel's persistent growth and double buffering.
    ///
    /// Each bounce round reads the compaction block counts back to the CPU, a synchronous
    /// stall. An order-preserving compaction was kept anyway (see `wavefront_compact_scan` in
    /// `wavefront_transport.wgsl`). This stall, and the lack of overlap with `dispatch_chunk`'s
    /// pipeline, are why the wavefront stays opt-in.
    fn dispatch_chunk_wavefront(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        planes_len: usize,
        outputs: &TransportOutputs,
        tuples: usize,
    ) {
        let pipelines = self
            .wavefront_pipelines
            .get(&state.pipeline_class)
            .expect("scene preparation must build the matching wavefront pipelines");
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;

        let rays = WavefrontRayBuffers::new(device, tuples);
        let wf_params_buf = compute::upload(
            device,
            "wavefront params",
            &[GpuWavefrontParams {
                chunk_rays: count_u32(tuples),
                active_count: count_u32(tuples),
                bounce: 0,
                workgroup_count: workgroups_for(tuples),
            }],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        // 1. wavefront_generate -- see `wavefront_transport.wgsl`'s header comment,
        // kernel sequence step 1. Needs only `camera`/`params` from the shared scene
        // bindings (camera-ray generation) plus its own ray-state/active-list buffers.
        let generate_bind_group = compute::bind_buffers(
            device,
            "wavefront generate bind group",
            &pipelines.generate,
            &wavefront_generate_bindings(&frame_buffers.camera, params_buf, &wf_params_buf, &rays),
        );
        let generate_workgroups = workgroups_for(tuples);
        let _ = compute::dispatch(
            device,
            queue,
            &pipelines.generate,
            &generate_bind_group,
            (generate_workgroups, 1, 1),
        );

        let bind_groups = build_wavefront_round_bind_groups(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            &wf_params_buf,
            &rays,
        );

        // 2-4. Bounce rounds: dispatch, order-preserving compact, repeat -- see
        // `wavefront_transport.wgsl`'s header comment, kernel sequence steps 2-4.
        let (active_count, bounce) = self.run_wavefront_bounce_rounds(
            pipelines,
            state.scene.max_bounces,
            count_u32(tuples),
            &rays,
            &wf_params_buf,
            &bind_groups,
        );

        // 5. wavefront_finalize_survivors -- see `wavefront_transport.wgsl`'s header
        // comment, kernel sequence step 5. Only reached when the bounce budget ran out
        // with rays still alive, not when every ray already died (those are finalized
        // inside `wavefront_bounce` itself, above).
        if active_count > 0 {
            queue.write_buffer(
                &wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays: count_u32(tuples),
                    active_count,
                    bounce,
                    workgroup_count: workgroups_for_u32(active_count),
                }),
            );
            let finalize_workgroups = workgroups_for_u32(active_count);
            let _ = compute::dispatch(
                device,
                queue,
                &pipelines.finalize_survivors,
                &bind_groups.finalize,
                (finalize_workgroups, 1, 1),
            );
        }
    }

    /// Runs the bounce and compaction loop (steps 2-4 of `wavefront_transport.wgsl`'s kernel
    /// sequence) until no ray is alive or `max_bounces` rounds have run. Returns the final
    /// `(active_count, bounce)` for step 5 in [`Self::dispatch_chunk_wavefront`]. Split out to
    /// keep that function under clippy's function-length limit.
    fn run_wavefront_bounce_rounds(
        &self,
        pipelines: &WavefrontPipelines,
        max_bounces: u32,
        chunk_rays: u32,
        rays: &WavefrontRayBuffers,
        wf_params_buf: &wgpu::Buffer,
        bind_groups: &WavefrontRoundBindGroups,
    ) -> (u32, u32) {
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let mut active_count = chunk_rays;
        let mut bounce = 0u32;
        // Rays alive after each round, for the survival curve. Recorded only when
        // `PLEOCHROMA_GPU_TIMING` is set; the compaction reads the count back anyway.
        let log_alive = timing::enabled();
        let mut alive_after_round: Vec<u32> = Vec::new();
        while active_count > 0 && bounce < max_bounces {
            let workgroup_count = workgroups_for_u32(active_count);
            queue.write_buffer(
                wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays,
                    active_count,
                    bounce,
                    workgroup_count,
                }),
            );

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.bounce,
                &bind_groups.bounce,
                (workgroup_count, 1, 1),
            );
            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scan,
                &bind_groups.compact_scan,
                (workgroup_count, 1, 1),
            );

            // Deterministic CPU-side exclusive prefix sum over this round's per-workgroup
            // alive counts -- see `wavefront_compact_scan`'s own doc comment
            // (`wavefront_transport.wgsl`) for why this stays a synchronous host
            // round-trip rather than a GPU-side atomic counter.
            let block_counts: Vec<u32> = compute::readback(
                device,
                queue,
                &rays.block_alive_count,
                workgroup_count as usize,
            );
            let (block_offsets, new_active_count) = exclusive_prefix_sum(&block_counts);
            queue.write_buffer(&rays.block_offset, 0, bytemuck::cast_slice(&block_offsets));

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scatter,
                &bind_groups.compact_scatter,
                (workgroup_count, 1, 1),
            );

            // `active_ray_indices_next` becomes the next round's `active_ray_indices` --
            // a plain buffer-to-buffer copy rather than swapping which buffer each bind
            // group references (bind groups are immutable once created in `wgpu`), so
            // every bind group built above stays valid for every round.
            if new_active_count > 0 {
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("wavefront active-list copy encoder"),
                });
                encoder.copy_buffer_to_buffer(
                    &rays.active_out,
                    0,
                    &rays.active_in,
                    0,
                    u64::from(new_active_count) * size_of::<u32>() as u64,
                );
                queue.submit(Some(encoder.finish()));
            }

            active_count = new_active_count;
            bounce += 1;
            if log_alive {
                alive_after_round.push(new_active_count);
            }
        }
        if log_alive {
            tracing::info!(
                target: "pleochroma_cut::render_bench",
                "gpu_timing wavefront_chunk rays={chunk_rays} \
                 alive_after_round={alive_after_round:?}"
            );
        }
        (active_count, bounce)
    }

    /// Picks how many pixels the NEXT chunk covers.
    ///
    /// `byte_budget_pixels` (from [`chunk_pixels_for`] against `self.chunk_budget_bytes`)
    /// is always the hard upper bound -- time-budgeted sizing only ever shrinks a chunk
    /// below it to approach [`TARGET_CHUNK_MS`], never raises it, so `chunk_budget_bytes`
    /// keeps meaning a byte-budget ceiling on one dispatch's output buffers.
    ///
    /// `ema_ns_per_tuple` being `None` (no chunk has drained yet) uses
    /// [`FIRST_DISPATCH_MAX_TUPLES`] instead of the full byte budget, so a cold
    /// integrated GPU's first dispatch(es) cannot alone trip Windows' TDR watchdog. Once
    /// a measurement exists, the target pixel count is clamped between
    /// [`MIN_CHUNK_BYTES`] worth of tuples (floor) and `byte_budget_pixels` (ceiling).
    pub(super) fn next_chunk_pixels(
        ema_ns_per_tuple: Option<f64>,
        byte_budget_pixels: usize,
        spp: u32,
        num_pixels: usize,
    ) -> usize {
        let spp_usize = (spp as usize).max(1);
        let Some(ns_per_tuple) = ema_ns_per_tuple.filter(|v| *v > 0.0) else {
            let first_dispatch_pixels = (FIRST_DISPATCH_MAX_TUPLES / spp_usize).max(1);
            return byte_budget_pixels
                .min(first_dispatch_pixels)
                .min(num_pixels)
                .max(1);
        };
        let target_tuples = (TARGET_CHUNK_MS * 1.0e6 / ns_per_tuple).max(1.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "target_tuples is clamped to >= 1.0 and finite for a positive ns_per_tuple; `as` saturates and the result is clamped to the byte budget below"
        )]
        let target_pixels = ((target_tuples / spp_usize as f64).floor() as usize).max(1);
        let min_pixels = chunk_pixels_for(MIN_CHUNK_BYTES, spp, num_pixels).min(byte_budget_pixels);
        target_pixels.clamp(min_pixels, byte_budget_pixels)
    }
}

/// How many pixels one dispatch covers, given a byte budget for its output buffers.
///
/// A pixel's `spp` samples always stay in one dispatch, so the budget is divided by `spp`
/// first. Always at least 1 -- a single
/// pixel over budget is still dispatched rather than looping forever on a zero-width
/// chunk -- and never more than the frame has. Also never large enough to need more than
/// [`MAX_WORKGROUPS_PER_DIMENSION`] workgroups -- see that constant's doc comment.
pub fn chunk_pixels_for(budget_bytes: usize, spp: u32, num_pixels: usize) -> usize {
    let budget_tuples = budget_bytes / (FLOATS_PER_TUPLE * size_of::<f32>());
    let dispatch_limited_tuples = MAX_WORKGROUPS_PER_DIMENSION * WORKGROUP_SIZE;
    let tuples_per_chunk = budget_tuples.min(dispatch_limited_tuples);
    (tuples_per_chunk / spp as usize).max(1).min(num_pixels)
}

/// [`environment_params`]'s return value: `(env_mode, temp_k, spot_mult, exposure,
/// light_yaw, light_pitch, use_d65, studio_model, backdrop)`. `use_d65` mirrors
/// `optics::raytracer::environment::sample_studio_environment_with_rig`'s own
/// `preset.uses_d65()` check -- see
/// `renderer::buffers::GpuTransportParams::studio_use_d65`'s doc comment.
pub type EnvironmentParams = (u32, f32, f32, f32, f32, f32, bool, u32, f32);

/// Maps an [`EnvironmentSource`] onto the megakernel's `env_mode` and its studio-rig
/// parameters. See [`EnvironmentParams`] for the tuple's fields.
///
/// An HDR map's studio-rig fields are unused by the shader's `HDR_MAP` branch, so they are
/// zeroed here rather than passed through. The map's exposure and yaw ride in
/// `GpuTransportParams` (`with_hdr_of`), not in this tuple.
///
/// Infallible, so it returns the tuple directly rather than a `Result`.
/// [`super::GpuFrameError::UnsupportedEnvironment`] is raised by `FrameSceneBuffers::ensure`
/// for an oversized HDR map.
pub const fn environment_params(environment: EnvironmentSource<'_>) -> EnvironmentParams {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            backdrop,
            ..
        } => (
            transport_env_mode::STUDIO_RIG,
            illuminant_temperature_k(preset),
            preset.params().spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            preset.uses_d65(),
            preset.model().gpu_id(),
            backdrop,
        ),
        EnvironmentSource::HdrMap { .. } => (
            transport_env_mode::HDR_MAP,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            0.0,
        ),
    }
}
