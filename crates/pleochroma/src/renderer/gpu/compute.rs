//! Minimal compute-pipeline harness shared by every `gpu`-feature self-test.
//!
//! Shader-module/pipeline creation, POD buffer upload, and blocking storage-buffer
//! readback, used across `renderer::gpu`. Generic `wgpu` plumbing, not specific to
//! `pleochroma`'s physics -- no render passes, no textures, no async scheduling beyond
//! a single blocking [`wgpu::Device::poll`] per readback.

use std::time::Duration;

use wgpu::util::DeviceExt;

/// Bound on every production-path GPU wait.
///
/// It covers [`finish_map_read`]'s `Device::poll` AND the
/// paired `mpsc::Receiver::recv_timeout` for the buffer-mapping callback (also used by
/// [`readback`]/[`dispatch_and_wait`]).
///
/// Without a bound a hung driver blocks forever; 4s sits well above the ~50ms per-chunk
/// target (`TARGET_CHUNK_MS`) even on a slow integrated GPU, while still surfacing a
/// wedged driver as `GpuFrameError::DeviceLost`.
pub const GPU_WAIT_TIMEOUT: Duration = Duration::from_secs(4);

/// The bound on a self-test's one-shot whole-image dispatch.
///
/// [`dispatch_and_wait_for`] from `frame::encode_and_dispatch` may run a Tier 3 comparison
/// with 48 x 48 pixels x 500 samples in one dispatch, far beyond a production chunk. A stone
/// with oriented inclusions in a zoned host took longer than [`GPU_WAIT_TIMEOUT`] on an
/// integrated GPU (2026-10-22).
pub const SELF_TEST_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a bounded, non-panicking production-path GPU wait ([`finish_map_read`]) failed to
/// produce a result.
///
/// Every variant means the device/driver can't be trusted to make forward progress; the
/// right response (see `GpuBackend`'s `lost` flag) is to stop using this
/// `GpuFrameRenderer`, never `.expect()`/panic.
#[derive(Debug)]
pub enum ComputeWaitError {
    /// `Device::poll` reported an error -- usually [`wgpu::PollError::Timeout`] but also
    /// a stale submission index.
    Poll(wgpu::PollError),
    /// The mapping callback did not resolve within [`GPU_WAIT_TIMEOUT`].
    MapTimeout,
    /// The buffer-mapping callback itself reported failure.
    Map(wgpu::BufferAsyncError),
    /// Mapping succeeded, but the mapped range could not be read.
    MapRange(wgpu::MapRangeError),
}

impl std::fmt::Display for ComputeWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poll(e) => write!(f, "device poll failed or timed out: {e}"),
            Self::MapTimeout => write!(
                f,
                "buffer-mapping callback did not fire within {GPU_WAIT_TIMEOUT:?}"
            ),
            Self::Map(e) => write!(f, "buffer mapping failed: {e}"),
            Self::MapRange(e) => write!(f, "mapped buffer range could not be read: {e}"),
        }
    }
}

impl std::error::Error for ComputeWaitError {}

/// Creates a compute pipeline from inline WGSL source with a single entry point.
///
/// Lets `wgpu` infer the bind group layout from the shader (`layout: None`) -- every
/// self-test kernel binds a small, fixed set of buffers, so hand-declaring a
/// [`wgpu::BindGroupLayout`] has no benefit. Uses default compilation options, so any
/// `override` declarations keep their defaults -- see
/// [`create_compute_pipeline_with_constants`] to set them.
///
/// # Panics
///
/// Panics if `wgsl_source` fails to parse/validate, or `entry_point` doesn't name a
/// `@compute` entry point -- a bad shader here is a bug in this crate, not runtime input.
#[must_use]
pub fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    create_compute_pipeline_with_constants(device, label, wgsl_source, entry_point, &[])
}

/// Like [`create_compute_pipeline`], but also sets `constants` on
/// [`wgpu::PipelineCompilationOptions`] -- WGSL `override` name/value pairs resolved at
/// pipeline creation.
///
/// `GpuFrameRenderer`'s material-class-specialised pipelines are the
/// only caller with non-empty `constants`: fixing `MATERIAL_CLASS` per pipeline lets
/// naga/the driver dead-code-eliminate other material classes' per-ray state.
///
/// # Panics
///
/// Same as [`create_compute_pipeline`], plus if `constants` names an identifier with no
/// matching `override`, or a value that doesn't fit its declared type.
#[must_use]
pub fn create_compute_pipeline_with_constants(
    device: &wgpu::Device,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
    constants: &[(&str, f64)],
) -> wgpu::ComputePipeline {
    create_compute_pipeline_cached(device, None, label, wgsl_source, entry_point, constants)
}

/// Names the pipeline being compiled when a panic unwinds out of wgpu.
///
/// naga's backends panic (rather than return an error) on IR they cannot lower, for example
/// "Expression [89] is not cached!" in the SPIR-V writer. That message names neither the
/// kernel nor the material class, which is the first thing a failing run needs; this guard
/// prints both as the panic unwinds through [`create_compute_pipeline_cached`]. It does
/// nothing on the normal path.
struct CompileTrace<'a> {
    label: &'a str,
    entry_point: &'a str,
    constants: &'a [(&'a str, f64)],
}

impl Drop for CompileTrace<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "panic while compiling compute pipeline `{}` (entry point `{}`, pipeline constants {:?})",
                self.label, self.entry_point, self.constants
            );
        }
    }
}

/// Like [`create_compute_pipeline_with_constants`], but compiles through `cache` when one
/// is given (see [`create_persisted_compute_pipeline`]); `None` behaves exactly as the uncached call.
///
/// # Panics
///
/// Same as [`create_compute_pipeline_with_constants`].
#[must_use]
pub fn create_compute_pipeline_cached(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
    constants: &[(&str, f64)],
) -> wgpu::ComputePipeline {
    let _trace = CompileTrace {
        label,
        entry_point,
        constants,
    };
    tracing::info!(
        label,
        entry_point,
        ?constants,
        "GPU pipeline compilation started"
    );
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &shader,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants,
            zero_initialize_workgroup_memory: true,
        },
        cache,
    });
    #[cfg(not(target_arch = "wasm32"))]
    tracing::info!(
        label,
        elapsed_ms = started.elapsed().as_millis(),
        "GPU pipeline compilation finished"
    );
    #[cfg(target_arch = "wasm32")]
    tracing::info!(label, "GPU pipeline compilation finished");
    pipeline
}

/// Compiles a compute pipeline through its own persistent cache shard.
///
/// The shard belongs to this shader, entry point and override set, so other
/// renderers cannot overwrite its variants.
/// Devices without `PIPELINE_CACHE` follow normal uncached compilation.
///
/// # Panics
///
/// Same shader validation requirements as [`create_compute_pipeline`].
#[must_use]
pub fn create_persisted_compute_pipeline(
    device: &wgpu::Device,
    adapter_info: &wgpu::AdapterInfo,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
    constants: &[(&str, f64)],
) -> wgpu::ComputePipeline {
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return create_compute_pipeline_with_constants(
            device,
            label,
            wgsl_source,
            entry_point,
            constants,
        );
    }
    let shard = super::pipeline_cache::shard_key(wgsl_source, entry_point, constants);
    let cache = create_pipeline_cache_shard(device, adapter_info, &shard);
    let pipeline = create_compute_pipeline_cached(
        device,
        cache.as_ref(),
        label,
        wgsl_source,
        entry_point,
        constants,
    );
    if let Some(cache) = &cache {
        super::pipeline_cache::persist(cache, adapter_info, &shard);
    }
    pipeline
}

/// An in-memory cache for callers that share one across multiple pipeline builds.
///
/// Use [`create_persisted_compute_pipeline`] for persistent, isolated variant shards.
/// Returns `None` when `PIPELINE_CACHE` is not enabled on the device.
#[must_use]
pub fn create_pipeline_cache(
    device: &wgpu::Device,
    _adapter_info: &wgpu::AdapterInfo,
) -> Option<wgpu::PipelineCache> {
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return None;
    }
    // SAFETY: no initial driver data is supplied.
    Some(unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("pleochroma session pipeline cache"),
            data: None,
            fallback: true,
        })
    })
}

/// Creates a cache for one pipeline identity; wgpu checks driver compatibility.
fn create_pipeline_cache_shard(
    device: &wgpu::Device,
    adapter_info: &wgpu::AdapterInfo,
    shard: &str,
) -> Option<wgpu::PipelineCache> {
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return None;
    }
    let blob = super::pipeline_cache::load_blob(adapter_info, shard);
    // SAFETY: data comes from get_data in the application-controlled cache
    // directory. wgpu's cache header validation and fallback reject incompatible
    // or corrupt blobs; shader and pipeline validation stay enabled on every load.
    let cache = unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("pleochroma gpu pipeline shard"),
            data: blob.as_deref(),
            fallback: true,
        })
    };
    Some(cache)
}

/// Uploads `data` into a new buffer with `usage` via
/// [`wgpu::util::DeviceExt::create_buffer_init`].
#[must_use]
pub fn upload<T: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage,
    })
}

/// Creates a new, zero-initialized buffer sized for `count` `T`s with `usage`.
///
/// Used for self-test output buffers: starting from known zeros makes a byte the
/// kernel fails to write (e.g. a struct-layout mismatch clobbering padding)
/// distinguishable from allocator garbage.
#[must_use]
pub fn zeroed_buffer<T: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    count: usize,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let zeros = vec![T::zeroed(); count];
    upload(device, label, &zeros, usage)
}

/// Blocking readback of `count` `T`s from a `COPY_SRC` buffer.
///
/// Copies into a fresh `MAP_READ` staging buffer, submits, blocks (bounded by
/// [`GPU_WAIT_TIMEOUT`]) on the poll, and returns the mapped bytes reinterpreted as `T`.
/// Self-test-only: kept panicking (unlike [`finish_map_read`]) since converting every
/// caller to `Result` has no production benefit, but the wait is still bounded.
///
/// # Panics
///
/// Panics if the device poll times out or errors, or the buffer never finishes mapping.
#[must_use]
pub fn readback<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    count: usize,
) -> Vec<T> {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pleochroma gpu self-test readback staging buffer"),
        size: byte_len,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pleochroma gpu self-test readback encoder"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, byte_len);
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .expect("device poll failed or timed out during readback");
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .expect("map_async callback did not fire within GPU_WAIT_TIMEOUT")
        .expect("failed to map readback staging buffer");

    let out = {
        let data = slice
            .get_mapped_range()
            .expect("staging buffer should be mapped at this point");
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    staging.unmap();
    out
}

/// Dispatches `pipeline` once with a single bind group (group 0), then blocks until the
/// GPU finishes (bounded by [`GPU_WAIT_TIMEOUT`]).
///
/// # Panics
///
/// Panics if the device poll times out or reports an error -- see [`readback`].
pub fn dispatch_and_wait(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
) {
    dispatch_and_wait_for(
        device,
        queue,
        pipeline,
        bind_group,
        workgroups,
        GPU_WAIT_TIMEOUT,
    );
}

/// [`dispatch_and_wait`] with its own bound on the wait.
///
/// # Panics
///
/// Panics if the device poll times out or reports an error.
pub fn dispatch_and_wait_for(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
    timeout: Duration,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pleochroma gpu self-test dispatch encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("pleochroma gpu self-test compute pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
    }
    queue.submit(Some(encoder.finish()));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(timeout),
        })
        .expect("device poll failed or timed out during dispatch");
}

/// The non-blocking counterpart to [`dispatch_and_wait`]: submits `pipeline` once but
/// does not poll or wait.
///
/// `GpuFrameRenderer::accumulate`'s chunked dispatch pairs this with
/// [`copy_to_staging`]/[`begin_map_read`]/[`finish_map_read`] to queue a chunk's work
/// without stalling the CPU before the next chunk. Self-tests use [`dispatch_and_wait`].
#[must_use]
pub fn dispatch(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
) -> wgpu::SubmissionIndex {
    dispatch_with(device, queue, pipeline, bind_group, workgroups, || {})
}

/// [`dispatch`], calling `on_encoded` once the command buffer is finished and just before it
/// is submitted -- the seam `renderer::gpu::timing` splits encode time from submit time
/// with. The commands recorded are exactly [`dispatch`]'s.
#[must_use]
pub(crate) fn dispatch_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
    on_encoded: impl FnOnce(),
) -> wgpu::SubmissionIndex {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pleochroma gpu pipelined dispatch encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("pleochroma gpu pipelined compute pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
    }
    let commands = encoder.finish();
    on_encoded();
    queue.submit(Some(commands))
}

/// Submits a copy of `count` `T`s from `source` (must carry `COPY_SRC`) into a fresh
/// `MAP_READ` staging buffer, without blocking or mapping.
///
/// Pairs with [`begin_map_read`]/[`finish_map_read`] so the eventual blocking wait
/// overlaps later queued GPU work rather than idling.
#[must_use]
pub fn copy_to_staging<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    count: usize,
    label: &str,
) -> (wgpu::Buffer, wgpu::SubmissionIndex) {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: byte_len,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pleochroma gpu pipelined readback copy encoder"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, byte_len);
    let index = queue.submit(Some(encoder.finish()));
    (staging, index)
}

/// Begins the async map for a staging buffer created by [`copy_to_staging`]. Returns a
/// receiver that resolves once [`finish_map_read`] drives the device's callbacks.
///
/// Splitting "begin" from "block until done" lets a caller queue several chunks before
/// blocking on any one.
#[must_use]
pub fn begin_map_read(
    staging: &wgpu::Buffer,
) -> std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    rx
}

/// Blocks (bounded by [`GPU_WAIT_TIMEOUT`]) until `staging`'s producing submission completes.
///
/// The submission is `copy_index`, from [`copy_to_staging`]; `rx` (from [`begin_map_read`])
/// must resolve too, then it reads the buffer back and unmaps it.
///
/// The read-back length is whatever `count` [`copy_to_staging`] sized `staging` for.
/// Waiting on `copy_index` specifically (rather than "most recent submission") avoids
/// also waiting for a later chunk already queued. This is the ONE production-path wait
/// (`GpuFrameRenderer::accumulate`'s chunk pipeline is its only caller); every failure
/// returns [`ComputeWaitError`] rather than panicking, for the caller to turn into
/// `GpuFrameError::DeviceLost`.
///
/// # Errors
///
/// [`ComputeWaitError`] if the poll times out or errors, the mapping callback doesn't
/// resolve in time, the callback reports failure, or the mapped range can't be read.
pub fn finish_map_read<T: bytemuck::Pod>(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
    copy_index: wgpu::SubmissionIndex,
    rx: &std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
) -> Result<Vec<T>, ComputeWaitError> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(copy_index),
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .map_err(ComputeWaitError::Poll)?;
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .map_err(|_| ComputeWaitError::MapTimeout)?
        .map_err(ComputeWaitError::Map)?;

    let out = {
        let data = staging
            .slice(..)
            .get_mapped_range()
            .map_err(ComputeWaitError::MapRange)?;
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    staging.unmap();
    Ok(out)
}

/// Builds a single-group bind group from `(binding, buffer)` pairs, using `pipeline`'s
/// auto-inferred layout for group 0 (see [`create_compute_pipeline`]'s `layout: None`).
#[must_use]
pub fn bind_buffers(
    device: &wgpu::Device,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    let layout = pipeline.get_bind_group_layout(0);
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &layout,
        entries: &entries,
    })
}

/// Like [`bind_buffers`], but each entry may bind an explicit BYTE size rather than the
/// buffer's whole allocation.
///
/// `renderer::gpu::frame`'s persistent scene buffers (`planes`/`facet_finishes`) are
/// grown-never-shrunk across frames -- a buffer can be LARGER than the live scene
/// currently bound to it. Binding the whole (possibly stale, larger) allocation via
/// [`bind_buffers`]'s `as_entire_binding` would make the shader's `arrayLength(&planes)`
/// see the buffer's full capacity rather than the live element count. Passing
/// `Some(bytes)` here binds exactly the first `bytes` bytes instead; `None` binds the
/// whole buffer, identical to [`bind_buffers`].
///
/// `Some(0)` degenerates to `None` (binding the whole buffer) rather than a genuine
/// empty range -- `wgpu::BufferSize` cannot represent zero, and in practice this crate
/// never dispatches a zero-facet scene, so the fallback is unreachable in normal use.
#[must_use]
pub fn bind_buffers_sized(
    device: &wgpu::Device,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer, Option<wgpu::BufferAddress>)],
) -> wgpu::BindGroup {
    let layout = pipeline.get_bind_group_layout(0);
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .map(|(binding, buffer, size)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: size.and_then(wgpu::BufferSize::new),
            }),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &layout,
        entries: &entries,
    })
}

/// Replaces `out`'s contents with the first `count` elements of `src` (clamped to
/// `src.len()`) and returns the number of elements copied.
///
/// A pure helper (no GPU access) so the "copy only the live prefix of a grown-never-shrunk
/// staging buffer" rule of [`finish_map_read_into`] is unit-testable. It clamps silently;
/// the caller decides whether a short source is a bug.
fn copy_prefix<T: Copy>(src: &[T], count: usize, out: &mut Vec<T>) -> usize {
    let n = count.min(src.len());
    out.clear();
    out.extend_from_slice(&src[..n]);
    n
}

/// Like [`finish_map_read`], but copies only the first `count` elements into a
/// caller-provided, reusable `Vec<T>` instead of allocating a fresh one on every call.
///
/// `GpuFrameRenderer::drain_pending_chunk` holds one scratch `Vec<f32>` across every
/// chunk of every frame and passes it here, rather than paying a heap allocation per
/// chunk the way the plain [`finish_map_read`] does. `count` is the chunk's live element
/// count: persistent staging buffers are grown-never-shrunk, so the mapped range can be
/// far larger than what this chunk wrote, and copying the whole of it would both waste
/// bandwidth and pollute the caller's timing measurement. The mapping itself still covers
/// the whole buffer. After a successful call `out.len() == count` (a debug build asserts
/// the staging buffer was at least that large). [`finish_map_read`] itself is kept
/// unchanged (and is still what every self-test uses): converting every caller to a
/// reusable buffer has no benefit for a one-shot self-test dispatch.
///
/// # Errors
///
/// Same conditions as [`finish_map_read`]'s.
pub fn finish_map_read_into<T: bytemuck::Pod>(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
    copy_index: wgpu::SubmissionIndex,
    rx: &std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    out: &mut Vec<T>,
    count: usize,
) -> Result<(), ComputeWaitError> {
    let mut mark = super::timing::Mark::now();
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(copy_index),
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .map_err(ComputeWaitError::Poll)?;
    mark.lap(super::timing::Slot::PollWait);
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .map_err(|_| ComputeWaitError::MapTimeout)?
        .map_err(ComputeWaitError::Map)?;
    mark.lap(super::timing::Slot::MapRecv);

    {
        let data = staging
            .slice(..)
            .get_mapped_range()
            .map_err(ComputeWaitError::MapRange)?;
        let src: &[T] = bytemuck::cast_slice(&data);
        let copied = copy_prefix(src, count, out);
        debug_assert_eq!(copied, count, "staging sized below the chunk's live length");
    }
    staging.unmap();
    mark.lap(super::timing::Slot::MapCopy);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::copy_prefix;

    #[test]
    fn copy_prefix_copies_only_the_live_prefix() {
        let src = [1.0_f32, 2.0, 3.0, 9.0, 9.0, 9.0];
        let mut out = vec![7.0_f32; 10];
        let n = copy_prefix(&src, 3, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn copy_prefix_zero_count_empties_the_output() {
        let src = [1.0_f32, 2.0, 3.0];
        let mut out = vec![7.0_f32; 4];
        assert_eq!(copy_prefix(&src, 0, &mut out), 0);
        assert!(out.is_empty());
    }

    /// The helper itself clamps silently (it never asserts), so this holds in debug and
    /// release alike; the debug assertion lives in `finish_map_read_into`, which needs a
    /// GPU and is covered by the gpu-feature hardware test instead.
    #[test]
    fn copy_prefix_clamps_a_count_beyond_the_source() {
        let src = [1.0_f32, 2.0, 3.0];
        let mut out = Vec::new();
        assert_eq!(copy_prefix(&src, 99, &mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }
}
