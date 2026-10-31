//! Native bounded, reusable dispatch/readback slots. A FIFO turn queues at most
//! two chunks before waiting, so readback of one overlaps execution of the other.
use super::{WhittedGpuError, WhittedGpuRenderer, WhittedGpuScene};
use crate::renderer::{buffers::whitted::GpuWhittedParams, gpu::compute};
use glam::Vec3;
use std::{sync::mpsc, time::Instant};

/// Absolute raster and tap coordinates. Chunking never renumbers either axis.
#[derive(Clone, Copy, Debug)]
pub struct WhittedGpuRange {
    pub first_pixel: u32,
    pub pixels: u32,
    pub sample_offset: u32,
    pub spp: u32,
}

/// Hard per-turn limits. The scheduler may choose smaller chunks.
#[derive(Clone, Copy, Debug)]
pub struct WhittedGpuTurnLimits {
    pub max_chunk_pixels: u32,
    pub max_chunks: usize,
}

pub(super) struct Slot {
    capacity: u32,
    uniform: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
    binding: Option<(u64, wgpu::BindGroup)>,
}

struct Pending {
    staging: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    receiver: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    pixels: u32,
}

impl Drop for Pending {
    fn drop(&mut self) {
        // Also cancels a map left pending by cancellation or a failed readback.
        self.staging.unmap();
    }
}

impl Slot {
    fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let create = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            capacity,
            uniform: create(
                "Ray reusable params",
                size_of::<GpuWhittedParams>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            output: create(
                "Ray reusable XYZ",
                u64::from(capacity) * 12,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            staging: create(
                "Ray reusable readback",
                u64::from(capacity) * 12,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            binding: None,
        }
    }

    fn bind(&mut self, ctx: &super::pipeline::WhittedGpuRenderer, scene: &WhittedGpuScene) {
        if self
            .binding
            .as_ref()
            .is_none_or(|(id, _)| *id != scene.upload_id)
        {
            let binding = compute::bind_buffers(
                &ctx.ctx.device,
                "Ray reusable bindings",
                &ctx.pipeline,
                &[
                    (0, &scene.camera),
                    (1, &self.uniform),
                    (2, &scene.planes),
                    (3, &scene.environment),
                    (4, &scene.dimensions),
                    (5, &scene.frost),
                    (6, &self.output),
                ],
            );
            self.binding = Some((scene.upload_id, binding));
        }
    }

    fn submit(
        &self,
        renderer: &WhittedGpuRenderer,
        range: WhittedGpuRange,
        scene: &WhittedGpuScene,
    ) -> Pending {
        let mut params = scene.params;
        params.range = [
            range.first_pixel,
            range.pixels,
            range.sample_offset,
            range.spp,
        ];
        renderer
            .ctx
            .queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
        let mut encoder =
            renderer
                .ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Ray dispatch and readback"),
                });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ray overlapped compute"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&renderer.pipeline);
            pass.set_bind_group(
                0,
                &self.binding.as_ref().expect("slot bound before submit").1,
                &[],
            );
            pass.dispatch_workgroups(range.pixels.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.output,
            0,
            &self.staging,
            0,
            u64::from(range.pixels) * 12,
        );
        let submission = renderer.ctx.queue.submit(Some(encoder.finish()));
        let receiver = compute::begin_map_read(&self.staging);
        Pending {
            staging: self.staging.clone(),
            submission,
            receiver,
            pixels: range.pixels,
        }
    }
}

impl Pending {
    fn read(&self, device: &wgpu::Device, result: &mut Vec<Vec3>) -> Result<(), WhittedGpuError> {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission.clone()),
                timeout: Some(compute::GPU_WAIT_TIMEOUT),
            })
            .map_err(|error| WhittedGpuError::DeviceLost(error.to_string()))?;
        self.receiver
            .recv_timeout(compute::GPU_WAIT_TIMEOUT)
            .map_err(|error| WhittedGpuError::DeviceLost(error.to_string()))?
            .map_err(|error| WhittedGpuError::DeviceLost(error.to_string()))?;
        let data = self
            .staging
            .slice(..u64::from(self.pixels) * 12)
            .get_mapped_range()
            .map_err(|error| WhittedGpuError::DeviceLost(error.to_string()))?;
        let floats: &[f32] = bytemuck::cast_slice(&data);
        for xyz in floats.as_chunks::<3>().0 {
            let value = Vec3::new(xyz[0], xyz[1], xyz[2]);
            if !value.is_finite() {
                return Err(WhittedGpuError::DeviceLost(
                    "Ray-Traced GPU returned non-finite output".into(),
                ));
            }
            result.push(value);
        }
        Ok(())
    }
}

impl WhittedGpuRenderer {
    /// Traces an exact, bounded pixel range using a reusable slot.
    ///
    /// # Errors
    ///
    /// Declines stale uploads or invalid ranges; device/readback failure loses the device.
    pub(super) fn trace_native_chunk(
        &mut self,
        scene: &WhittedGpuScene,
        first_pixel: u32,
        pixels: u32,
        sample_offset: u32,
        spp: u32,
    ) -> Result<Vec<Vec3>, WhittedGpuError> {
        pollster::block_on(self.trace_native_chunk_async(
            scene,
            first_pixel,
            pixels,
            sample_offset,
            spp,
        ))
    }

    pub(super) async fn trace_native_chunk_async(
        &mut self,
        scene: &WhittedGpuScene,
        first_pixel: u32,
        pixels: u32,
        sample_offset: u32,
        spp: u32,
    ) -> Result<Vec<Vec3>, WhittedGpuError> {
        let range = WhittedGpuRange {
            first_pixel,
            pixels,
            sample_offset,
            spp,
        };
        self.validate_range(scene, range)?;
        if pixels > super::CHUNK_PIXELS {
            return Err(WhittedGpuError::Declined(
                "Ray chunk exceeds bounded slot capacity".into(),
            ));
        }
        let scope = self
            .ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.trace_ranges_inner(scene, &[range], &|| false);
        let validation = scope.pop().await;
        self.finish_trace_scope(result, validation)
            .map(Option::unwrap_or_default)
    }

    /// Queues at most two chunks, checks cancellation between submissions/readbacks,
    /// and returns detached results. `None` means cancellation; no results are committed.
    ///
    /// # Errors
    ///
    /// Same range and device-health requirements as [`Self::trace_chunk`].
    pub fn trace_turn(
        &mut self,
        scene: &WhittedGpuScene,
        remaining: WhittedGpuRange,
        limits: WhittedGpuTurnLimits,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<Vec3>>, WhittedGpuError> {
        self.validate_range(scene, remaining)?;
        let chunk =
            self.timing
                .chunk_pixels(scene.upload_id, remaining.spp, limits.max_chunk_pixels);
        let ranges = split_turn(remaining, chunk, limits.max_chunks);
        let started = Instant::now();
        let result = self.trace_ranges(scene, &ranges, cancel)?;
        if let Some(values) = &result {
            let tuples = values.len() as u128 * u128::from(remaining.spp);
            self.timing
                .observe(started.elapsed().as_nanos(), tuples, chunk);
        }
        Ok(result)
    }

    fn validate_range(
        &self,
        scene: &WhittedGpuScene,
        range: WhittedGpuRange,
    ) -> Result<(), WhittedGpuError> {
        self.healthy()?;
        if scene.device_id != self.id || !range.admitted(scene.pixels, scene.taps) {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced pixel/tap range or device is invalid".into(),
            ));
        }
        Ok(())
    }

    fn trace_ranges(
        &mut self,
        scene: &WhittedGpuScene,
        ranges: &[WhittedGpuRange],
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<Vec3>>, WhittedGpuError> {
        let scope = self
            .ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.trace_ranges_inner(scene, ranges, cancel);
        let validation = pollster::block_on(scope.pop());
        self.finish_trace_scope(result, validation)
    }

    fn finish_trace_scope(
        &mut self,
        result: Result<Option<Vec<Vec3>>, WhittedGpuError>,
        validation: Option<wgpu::Error>,
    ) -> Result<Option<Vec<Vec3>>, WhittedGpuError> {
        if result.is_err() || validation.is_some() {
            // Validation can invalidate bind groups/buffers; do not reuse those.
            self.slots = [None, None];
        }
        let values = result?;
        if let Some(error) = validation {
            return Err(WhittedGpuError::Declined(format!(
                "Ray-Traced dispatch: {error}"
            )));
        }
        self.healthy()?;
        Ok(values)
    }

    fn trace_ranges_inner(
        &mut self,
        scene: &WhittedGpuScene,
        ranges: &[WhittedGpuRange],
        cancel: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<Vec3>>, WhittedGpuError> {
        let mut pending = Vec::with_capacity(2);
        for (index, range) in ranges.iter().enumerate() {
            if cancel() {
                return Ok(None);
            }
            // Take the slot while building bindings to avoid aliasing mutable self.
            let mut slot = self.slots[index]
                .take()
                .filter(|slot| slot.capacity >= range.pixels)
                .unwrap_or_else(|| {
                    Slot::new(
                        &self.ctx.device,
                        range.pixels.next_power_of_two().min(super::CHUNK_PIXELS),
                    )
                });
            slot.bind(self, scene);
            pending.push(slot.submit(self, *range, scene));
            self.slots[index] = Some(slot);
        }
        let capacity = ranges.iter().map(|range| range.pixels as usize).sum();
        let mut values = Vec::with_capacity(capacity);
        for chunk in pending {
            if cancel() {
                return Ok(None);
            }
            chunk.read(&self.ctx.device, &mut values)?;
        }
        if cancel() {
            return Ok(None);
        }
        Ok(Some(values))
    }
}

impl WhittedGpuRange {
    fn admitted(self, pixels: u32, taps: u32) -> bool {
        self.pixels != 0
            && self.spp != 0
            && self
                .first_pixel
                .checked_add(self.pixels)
                .is_some_and(|end| end <= pixels)
            && self
                .sample_offset
                .checked_add(self.spp)
                .is_some_and(|end| end <= taps)
    }
}

fn split_turn(mut range: WhittedGpuRange, chunk: u32, chunks: usize) -> Vec<WhittedGpuRange> {
    let mut ranges = Vec::with_capacity(2);
    for _ in 0..chunks.clamp(1, 2) {
        if range.pixels == 0 {
            break;
        }
        let pixels = range.pixels.min(chunk.max(1));
        ranges.push(WhittedGpuRange { pixels, ..range });
        range.first_pixel += pixels;
        range.pixels -= pixels;
    }
    ranges
}

pub(super) use super::scheduling::ChunkTiming;

#[cfg(test)]
#[path = "native_dispatch_tests.rs"]
mod tests;
