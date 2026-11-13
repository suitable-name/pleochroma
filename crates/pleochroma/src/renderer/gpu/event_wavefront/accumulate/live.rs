//! Adaptive (live-tile) passes of the event wavefront renderer:
//! [`EventWavefrontRenderer::accumulate_live_with_tools`].
//!
//! The same pass contract as `GpuFrameRenderer::accumulate_live_turn` (`frame/live.rs`):
//! samples `sample_offset..sample_offset + spp` are added to every pixel of the live tiles,
//! into the summed accumulator and into half-buffer B, which receives only the samples with an
//! odd absolute index. Other pixels are neither traced nor touched.
//!
//! Only `ew_generate` differs: it is the one kernel that calls `transport_generate_ray`, and it
//! is compiled with the pixel lookup replaced by the live-list lookup (`frame::live_source`).
//! Every other kernel is the ordinary one, over the same ray slots. Each chunk's per-ray sums
//! are reduced by `reduce_xyz_live.wgsl` and added to the caller's buffers at the real pixel
//! indices as the chunk completes, as ordinary accumulation does; a failed call leaves the
//! chunks already summed.

use std::collections::HashMap;

use glam::Vec3;
use wgpu::BufferUsages;

use super::{
    EventWavefrontRenderer, check_tool_limit, chunk_pixels_for, chunk_transport_params, count_u32,
    next_chunk_pixels, tool_count_u32,
};
use crate::{
    geometry::{
        GpuFacetPlane,
        tool::{ToolPrimitive, pack_planes_and_tools},
    },
    render_setup::hash_planes,
    renderer::{
        adaptive::LiveTiles,
        frame_scene::FrameScene,
        gpu::{
            compute,
            event_wavefront::{
                EwError,
                chunk::{bind_generate_live, dispatch_chunk_live},
                clock,
                readback::map_read,
            },
            frame::{
                GpuReduceLiveParams, LIVE_REDUCE_ENTRY, LIVE_REDUCE_SHADER_SRC, classify_material,
                event_wavefront_live_generate_source_for, live_pixel_indices, material_class,
            },
        },
    },
};

/// Floats read back per live pixel: the summed XYZ, then the odd-sample XYZ.
const LIVE_FLOATS_PER_PIXEL: usize = 6;

/// A chunk's reduced output and the staging buffer it is read back through.
struct LiveEwOut {
    buffer: wgpu::Buffer,
    staging: wgpu::Buffer,
    capacity: usize,
}

/// What an [`EventWavefrontRenderer`] holds for live passes. All empty until the first one.
#[derive(Default)]
pub(super) struct LiveEwState {
    /// The live `ew_generate` pipelines by material class value. Looked up, never iterated.
    generate: HashMap<u32, wgpu::ComputePipeline>,
    /// `reduce_xyz_live_main`.
    reduce: Option<wgpu::ComputePipeline>,
    /// The persistent [`GpuReduceLiveParams`] uniform.
    reduce_params: Option<wgpu::Buffer>,
    /// The live pixel list of the pass in progress.
    list: Option<wgpu::Buffer>,
    /// The reduced output of the chunk in flight.
    out: Option<LiveEwOut>,
}

impl EventWavefrontRenderer {
    /// [`Self::accumulate_with_tools`] restricted to the live tiles of `live`, with the
    /// odd-sample half-buffer: one adaptive sampling pass.
    ///
    /// Adds the samples `sample_offset..sample_offset + spp` (absolute indices) to `sum` at
    /// every pixel of every live tile, and the samples with an ODD absolute index only to
    /// `half_b` at the same pixels. A pixel draws exactly the samples it would in an ordinary
    /// pass, so `LiveTiles::all` equals [`Self::accumulate_with_tools`] over the same range.
    ///
    /// # Errors
    ///
    /// [`EwError::InvalidBufferSize`] if `sum` or `half_b` does not hold one entry per pixel or
    /// `live` describes another frame size; [`EwError::LivePassUnavailable`] if the live pixel
    /// list does not fit one storage binding of the device; otherwise the errors of
    /// [`Self::accumulate_via_class_with_tools`].
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
        )
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "accumulate_with_tools' parameter list plus the live tiles and the half-buffer"
    )]
    pub async fn accumulate_live_with_tools(
        &mut self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        live: &LiveTiles,
        sample_offset: u32,
        spp: u32,
        sum: &mut [Vec3],
        half_b: &mut [Vec3],
    ) -> Result<(), EwError> {
        let num_pixels = (scene.width * scene.height) as usize;
        for actual in [sum.len(), half_b.len()] {
            if actual != num_pixels {
                return Err(EwError::InvalidBufferSize {
                    expected: num_pixels,
                    actual,
                });
            }
        }
        if live.width() != scene.width || live.height() != scene.height {
            return Err(EwError::InvalidBufferSize {
                expected: num_pixels,
                actual: (live.width() * live.height()) as usize,
            });
        }
        if spp == 0 || live.is_empty() {
            return Ok(());
        }
        if scene.environment.tray_shadow().is_some() || scene.environment.tray_mirror().is_some() {
            return Err(EwError::TrayVariant);
        }
        if !scene.material.gpu_supported() {
            return Err(EwError::UnsupportedMaterial(scene.material.name.clone()));
        }
        check_tool_limit(tools)?;

        let material_class = self.supported_class(classify_material(scene.material));
        let packed = pack_planes_and_tools(scene.planes, tools);
        let scene_key = (
            scene.width,
            scene.height,
            hash_planes(&packed),
            material_class,
            scene.material.scatters(),
        );
        if self.last_scene_key != Some(scene_key) {
            self.ns_per_tuple_ema = None;
            self.last_scene_key = Some(scene_key);
        }
        self.check_uncaptured_error()?;

        let pixels = live_pixel_indices(live);
        let result = self
            .accumulate_live_prepared(
                scene,
                (&packed, tool_count_u32(tools), material_class),
                (&pixels, sample_offset, spp),
                (sum, half_b),
            )
            .await;
        if result.is_err() {
            // A failed or timed-out chunk can leave the staging buffer mapped-pending.
            self.chunk_buffers = None;
            self.live.out = None;
        }
        result
    }

    /// Builds the live `ew_generate` pipeline of `material_class`, the live reduce pipeline and
    /// the reduce uniform, if not built yet.
    fn ensure_live_pipelines(&mut self, material_class: u32) -> Result<(), EwError> {
        if self.live.reduce.is_none() {
            self.live.reduce = Some(compute::create_compute_pipeline_cached(
                &self.ctx.device,
                None,
                LIVE_REDUCE_ENTRY,
                LIVE_REDUCE_SHADER_SRC,
                LIVE_REDUCE_ENTRY,
                &[],
            ));
        }
        if self.live.reduce_params.is_none() {
            self.live.reduce_params =
                Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ew live reduce params"),
                    size: size_of::<GpuReduceLiveParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        if !self.live.generate.contains_key(&material_class) {
            let source =
                event_wavefront_live_generate_source_for(material_class).ok_or_else(|| {
                    EwError::LivePassUnavailable(
                        "the ew_generate unit no longer holds the pixel lookup line".to_string(),
                    )
                })?;
            let pipeline = compute::create_compute_pipeline_cached(
                &self.ctx.device,
                None,
                "ew_generate live",
                source,
                "ew_generate",
                &material_class::pipeline_constants(material_class),
            );
            self.live.generate.insert(material_class, pipeline);
        }
        Ok(())
    }

    /// Uploads the pass's pixel list and sizes the reduced-output buffers for `pixels` pixels.
    fn ensure_live_buffers(&mut self, list: &[u32], pixels: usize) -> Result<(), EwError> {
        let limits = self.ctx.device.limits();
        let list_bytes = size_of_val(list) as u64;
        if list_bytes > limits.max_storage_buffer_binding_size
            || list_bytes > limits.max_buffer_size
        {
            return Err(EwError::LivePassUnavailable(format!(
                "{} live pixels do not fit one storage binding of this device",
                list.len()
            )));
        }
        self.live.list = Some(compute::upload(
            &self.ctx.device,
            "ew live pixel list",
            list,
            BufferUsages::STORAGE,
        ));
        let big_enough = self.live.out.as_ref().is_some_and(|o| o.capacity >= pixels);
        if !big_enough {
            let bytes = (pixels * LIVE_FLOATS_PER_PIXEL * size_of::<f32>()) as wgpu::BufferAddress;
            self.live.out = Some(LiveEwOut {
                buffer: compute::zeroed_buffer::<f32>(
                    &self.ctx.device,
                    "ew live reduced output",
                    pixels * LIVE_FLOATS_PER_PIXEL,
                    BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                ),
                staging: self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ew live staging buffer"),
                    size: bytes,
                    usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                capacity: pixels,
            });
        }
        Ok(())
    }

    /// The chunk loop of a live pass. `pixels` is the live pixel list (real pixel indices in
    /// virtual order), `(sample_offset, spp)` the sample range, `(sum, half_b)` the caller's
    /// full-frame accumulators.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
        )
    )]
    async fn accumulate_live_prepared(
        &mut self,
        scene: &FrameScene<'_>,
        (packed, tool_count, material_class): (&[GpuFacetPlane], u32, u32),
        (pixels, sample_offset, spp): (&[u32], u32, u32),
        (sum, half_b): (&mut [Vec3], &mut [Vec3]),
    ) -> Result<(), EwError> {
        let live_count = pixels.len();
        self.upload_scene(scene, packed, (spp, material_class))?;
        self.ensure_live_pipelines(material_class)?;

        let max_storage_binding = self.ctx.device.limits().max_storage_buffer_binding_size;
        let byte_budget_pixels = chunk_pixels_for(
            self.chunk_budget_bytes,
            spp,
            live_count,
            max_storage_binding,
        );
        self.ensure_live_buffers(pixels, byte_budget_pixels.min(live_count))?;

        let mut first = 0usize;
        while first < live_count {
            self.check_uncaptured_error()?;
            let remaining = live_count - first;
            let pixels_this_chunk =
                next_chunk_pixels(self.ns_per_tuple_ema, byte_budget_pixels, spp, remaining)
                    .min(remaining);
            let chunk_rays = pixels_this_chunk * spp as usize;
            self.ensure_chunk_buffers(pixels_this_chunk, chunk_rays);
            let chunk_params =
                chunk_transport_params(scene, tool_count, sample_offset, pixels_this_chunk, first)
                    .with_estimator(self.frame_estimator(), self.surface.is_ideal_for_cpu());

            clock::set_readback_deadline_ms(clock::chunk_deadline_ms(
                self.ns_per_tuple_ema,
                chunk_rays,
            ));
            let start = clock::Instant::now();

            let data = self
                .run_live_chunk(
                    scene,
                    material_class,
                    (&chunk_params, chunk_rays),
                    (pixels_this_chunk, spp, sample_offset),
                )
                .await
                .map_err(|error| match self.ctx.failure() {
                    Some(_) => {
                        EwError::DeviceLost(self.ctx.failure_with_symptom(&error.to_string()))
                    }
                    None => error,
                })?;
            // A validation error never fails the dispatch call itself: the pass is skipped and
            // the readback returns zeros or the previous chunk. Never accept that.
            self.check_uncaptured_error()?;

            let ns_per_tuple = start.elapsed().as_nanos() as f64 / chunk_rays as f64;
            self.ns_per_tuple_ema = Some(clock::blend_ema(self.ns_per_tuple_ema, ns_per_tuple));

            for (local, floats) in data
                .as_chunks::<LIVE_FLOATS_PER_PIXEL>()
                .0
                .iter()
                .take(pixels_this_chunk)
                .enumerate()
            {
                let pixel = pixels[first + local] as usize;
                sum[pixel] += Vec3::new(floats[0], floats[1], floats[2]);
                half_b[pixel] += Vec3::new(floats[3], floats[4], floats[5]);
            }
            first += pixels_this_chunk;
        }
        Ok(())
    }

    /// Dispatches one live chunk (generate through the live kernel, the bounce rounds, the
    /// finalize), reduces it with the live reduce kernel and reads the result back.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
        )
    )]
    async fn run_live_chunk(
        &mut self,
        scene: &FrameScene<'_>,
        material_class: u32,
        (chunk_params, chunk_rays): (&crate::renderer::buffers::GpuTransportParams, usize),
        (pixels_this_chunk, spp, sample_offset): (usize, u32, u32),
    ) -> Result<Vec<f32>, EwError> {
        let unavailable = |what: &str| EwError::LivePassUnavailable(format!("{what} not prepared"));
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let (
            Some(scene_buffers),
            Some(chunk_buffers),
            Some(generate),
            Some(reduce),
            Some(reduce_params),
            Some(list),
            Some(out),
        ) = (
            self.scene_buffers.as_ref(),
            self.chunk_buffers.as_ref(),
            self.live.generate.get(&material_class),
            self.live.reduce.as_ref(),
            self.live.reduce_params.as_ref(),
            self.live.list.as_ref(),
            self.live.out.as_ref(),
        )
        else {
            return Err(unavailable("live buffers or pipelines"));
        };

        let generate_bind = bind_generate_live(
            device,
            generate,
            (scene_buffers, chunk_buffers),
            list,
            material_class,
        );
        dispatch_chunk_live(
            device,
            queue,
            &mut self.pipelines,
            (scene_buffers, chunk_buffers),
            scene,
            (material_class, self.surface.needs_ground_arm()),
            chunk_params,
            chunk_rays,
            (generate, &generate_bind),
        )?;

        queue.write_buffer(
            reduce_params,
            0,
            bytemuck::bytes_of(&GpuReduceLiveParams {
                num_pixels: count_u32(pixels_this_chunk),
                num_samples: spp,
                sample_offset,
                _pad0: 0,
            }),
        );
        let reduce_bind = compute::bind_buffers(
            device,
            "ew live reduce bind group",
            reduce,
            &[
                (0, reduce_params),
                (1, &chunk_buffers.out_xyz),
                (2, &out.buffer),
            ],
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ew live reduce and copy encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ew live reduce pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(reduce);
            pass.set_bind_group(0, &reduce_bind, &[]);
            pass.dispatch_workgroups(count_u32(pixels_this_chunk).div_ceil(64), 1, 1);
        }
        let copy_bytes =
            (pixels_this_chunk * LIVE_FLOATS_PER_PIXEL * size_of::<f32>()) as wgpu::BufferAddress;
        encoder.copy_buffer_to_buffer(&out.buffer, 0, &out.staging, 0, copy_bytes);
        queue.submit(Some(encoder.finish()));

        map_read(
            device,
            queue,
            &out.staging,
            pixels_this_chunk * LIVE_FLOATS_PER_PIXEL,
        )
        .await
    }
}
