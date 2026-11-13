//! Leaf buffers on the GPU (beam guide §14.7 item 1).
//!
//! They hold the batch's packed leaves and per-tile
//! offsets, uploaded once; the prefix-sum fragment plan; the caps rule that halves a batch;
//! and the fixed-order per-pixel reduction of the fragment slots (§14.7 item 2).

use std::{
    ops::Range,
    sync::{Arc, PoisonError, atomic::AtomicBool},
};

use bytemuck::{Pod, Zeroable};

use crate::renderer::{
    beam::beam::{GpuLeafHeader, GpuLeafNode, GpuLeafVertex, PackedLeaves},
    gpu::{compute, context::GpuContext},
};

/// The WGSL source of the per-pixel reduction.
pub const REDUCE_BEAM_FRAGMENTS_SRC: &str =
    include_str!("../../shaders/beam/reduce_beam_fragments.wgsl");

/// The entry point of [`REDUCE_BEAM_FRAGMENTS_SRC`].
pub const REDUCE_ENTRY: &str = "beam_reduce_main";

/// Invocations per workgroup of the reduction (matches `@workgroup_size` in the shader).
const REDUCE_GROUP_SIZE: u32 = 64;

/// The most workgroups along x of one reduction dispatch (the shader flattens the 2-D grid).
const REDUCE_MAX_GROUPS_X: u32 = 4096;

/// Bytes of one fragment slot (`vec4<f32>`).
const SLOT_BYTES: u64 = 16;

/// Bytes of one entry of the per-pixel slot-index list.
const SLOT_ID_BYTES: u64 = 4;

const HEADER_BYTES: u64 = size_of::<GpuLeafHeader>() as u64;
const VERTEX_BYTES: u64 = size_of::<GpuLeafVertex>() as u64;
const NODE_BYTES: u64 = size_of::<GpuLeafNode>() as u64;
const OFFSET_BYTES: u64 = size_of::<u32>() as u64;

/// A batch's leaf data resident on the GPU.
pub struct LeafBufferSet {
    /// `GpuLeafHeader` array.
    pub headers: wgpu::Buffer,
    /// `GpuLeafVertex` array.
    pub vertices: wgpu::Buffer,
    /// `GpuLeafNode` array.
    pub nodes: wgpu::Buffer,
    /// The per-tile CSR offsets.
    pub tile_offsets: wgpu::Buffer,
    /// The number of leaves.
    pub leaf_count: u32,
}

/// The largest storage binding the device accepts: the smaller of its
/// `max_storage_buffer_binding_size` and `max_buffer_size`.
#[must_use]
pub fn leaf_binding_limit(ctx: &GpuContext) -> u64 {
    let limits = ctx.device.limits();
    limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size)
}

/// The byte size of the largest of the four leaf buffers of `packed` (each is padded to one
/// element on upload, so an empty array counts one element).
#[must_use]
pub fn max_leaf_buffer_bytes(packed: &PackedLeaves) -> u64 {
    [
        packed.headers.len() as u64 * HEADER_BYTES,
        packed.vertices.len() as u64 * VERTEX_BYTES,
        packed.nodes.len() as u64 * NODE_BYTES,
        packed.tile_offsets.len() as u64 * OFFSET_BYTES,
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// Uploads `data` as a storage buffer, padding an empty array to one zero element (a storage
/// binding cannot be empty).
fn upload_padded<T: Pod>(device: &wgpu::Device, label: &str, data: &[T]) -> wgpu::Buffer {
    let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    if data.is_empty() {
        compute::upload(device, label, &[T::zeroed()], usage)
    } else {
        compute::upload(device, label, data, usage)
    }
}

impl LeafBufferSet {
    /// Uploads `packed`; `None` when a buffer would exceed the device's storage-binding limit or
    /// the batch is empty (the caller halves the batch or declines the tile to the CPU).
    #[must_use]
    pub fn upload(ctx: &GpuContext, packed: &PackedLeaves) -> Option<Self> {
        if packed.headers.is_empty() || max_leaf_buffer_bytes(packed) > leaf_binding_limit(ctx) {
            return None;
        }
        let leaf_count = u32::try_from(packed.headers.len()).ok()?;
        let device = &ctx.device;
        Some(Self {
            headers: upload_padded(device, "beam leaf headers", &packed.headers),
            vertices: upload_padded(device, "beam leaf vertices", &packed.vertices),
            nodes: upload_padded(device, "beam leaf nodes", &packed.nodes),
            tile_offsets: upload_padded(device, "beam tile offsets", &packed.tile_offsets),
            leaf_count,
        })
    }
}

/// Where each leaf's fragments start: one fragment per (leaf, covered pixel), enumerated by a
/// prefix sum over the leaves' pixel counts (§14.7 item 2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FragmentPlan {
    /// Per leaf, the first fragment slot; one extra entry holds the total.
    pub first_fragment: Vec<u32>,
    /// Per leaf, its pixel-bounding rectangle `(x0, y0, x1, y1)`, half-open.
    pub pixel_rects: Vec<[u32; 4]>,
    /// The total fragment count.
    pub total: u32,
}

/// `floor(value)` clamped to `0..=max` as a pixel coordinate.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is floored and clamped to 0..=max first, so the conversion is exact"
)]
fn clamp_to_pixel(value: f64, max: u32) -> u32 {
    value.floor().clamp(0.0, f64::from(max)) as u32
}

/// The half-open pixel rectangle of one leaf polygon, empty (all zero) when it has no finite
/// vertices or falls outside the image.
fn leaf_pixel_rect(vertices: &[GpuLeafVertex], size: (u32, u32)) -> [u32; 4] {
    const EMPTY: [u32; 4] = [0; 4];
    if vertices.is_empty() {
        return EMPTY;
    }
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for vertex in vertices {
        let [x, y] = vertex.image;
        if !(x.is_finite() && y.is_finite()) {
            return EMPTY;
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let x0 = clamp_to_pixel(f64::from(min_x + 0.5), size.0);
    let y0 = clamp_to_pixel(f64::from(min_y + 0.5), size.1);
    let x1 = clamp_to_pixel(f64::from((max_x + 0.5).floor()) + 1.0, size.0);
    let y1 = clamp_to_pixel(f64::from((max_y + 0.5).floor()) + 1.0, size.1);
    if x1 <= x0 || y1 <= y0 {
        return EMPTY;
    }
    [x0, y0, x1, y1]
}

impl FragmentPlan {
    /// The plan of `packed` for an image of `size` pixels.
    ///
    /// Each leaf's rectangle is `floor(min + 0.5) .. floor(max + 0.5) + 1` of its image polygon,
    /// clipped to the image (an empty rectangle is all zero); `first_fragment` is the exclusive
    /// prefix sum of the rectangles' pixel counts with the total appended. The sum saturates at
    /// `u32::MAX`; [`split_into_batches`] never lets a dispatched batch get near that.
    #[must_use]
    pub fn from_leaves(packed: &PackedLeaves, size: (u32, u32)) -> Self {
        let mut first_fragment = Vec::with_capacity(packed.headers.len() + 1);
        let mut pixel_rects = Vec::with_capacity(packed.headers.len());
        let mut running = 0_u32;
        for header in &packed.headers {
            first_fragment.push(running);
            let first = header.first_vertex as usize;
            let vertices = first
                .checked_add(header.vertex_count as usize)
                .and_then(|end| packed.vertices.get(first..end))
                .unwrap_or(&[]);
            let rect = leaf_pixel_rect(vertices, size);
            let pixels = (rect[2] - rect[0]) * (rect[3] - rect[1]);
            running = running.saturating_add(pixels);
            pixel_rects.push(rect);
        }
        first_fragment.push(running);
        Self {
            first_fragment,
            pixel_rects,
            total: running,
        }
    }

    /// The pixel count of leaf `leaf` (zero outside the plan).
    #[must_use]
    pub fn leaf_pixels(&self, leaf: usize) -> u64 {
        self.pixel_rects.get(leaf).map_or(0, |rect| {
            u64::from(rect[2] - rect[0]) * u64::from(rect[3] - rect[1])
        })
    }
}

/// The bounding rectangle `(x0, y0, x1, y1)` of the non-empty rectangles of `rects`, `None` when
/// every one is empty.
#[must_use]
pub fn bounding_rect(rects: &[[u32; 4]]) -> Option<[u32; 4]> {
    rects
        .iter()
        .filter(|rect| rect[2] > rect[0] && rect[3] > rect[1])
        .copied()
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}

/// The byte size of the largest buffer a reduction over `bounds` allocates, computed before any
/// host or device allocation.
///
/// The output and its staging copy hold one `vec4<f32>` per pixel of the bounding rectangle, the
/// offsets one `u32` per pixel plus one, the slot-index list `slot_ids` entries. The arithmetic
/// saturates, so an absurd rectangle reads as a huge size, never as a wrapped small one.
#[must_use]
pub fn reduction_max_buffer_bytes(bounds: Option<[u32; 4]>, slot_ids: u64) -> u64 {
    let Some(rect) = bounds else {
        return 0;
    };
    let pixels =
        u64::from(rect[2].saturating_sub(rect[0])) * u64::from(rect[3].saturating_sub(rect[1]));
    [
        pixels.saturating_mul(SLOT_BYTES),
        pixels.saturating_add(1).saturating_mul(OFFSET_BYTES),
        slot_ids.saturating_mul(SLOT_ID_BYTES),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// Whether the reduction of `plan` allocates only buffers of at most `limit_bytes` each.
///
/// The check uses the plan's actual bounding rectangle (gaps between distant leaves included), so
/// a sparse piece is declined before [`PixelCsr::from_plan`] builds its host arrays.
#[must_use]
pub fn reduction_fits(plan: &FragmentPlan, limit_bytes: u64) -> bool {
    reduction_max_buffer_bytes(bounding_rect(&plan.pixel_rects), u64::from(plan.total))
        <= limit_bytes
}

/// The per-pixel CSR of fragment slots over the bounding rectangle of a plan's leaves.
///
/// Pixel `i` (row-major in the rectangle) owns `slots[offsets[i]..offsets[i + 1]]`, listed in leaf
/// order and, within a leaf, row-major (the fixed summation order of the reduction).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PixelCsr {
    /// The rectangle's top-left pixel `(x, y)`.
    pub origin: (u32, u32),
    /// The rectangle's size `(width, height)` in pixels; `(0, 0)` for an empty plan.
    pub size: (u32, u32),
    /// `width * height + 1` offsets into [`Self::slots`] (just `[0]` for an empty plan).
    pub offsets: Vec<u32>,
    /// The fragment slot indices of every pixel, in summation order.
    pub slots: Vec<u32>,
}

impl PixelCsr {
    /// The CSR of `plan`, built by a deterministic two-pass counting sort.
    #[must_use]
    pub fn from_plan(plan: &FragmentPlan) -> Self {
        let rects = || {
            plan.pixel_rects
                .iter()
                .zip(&plan.first_fragment)
                .filter(|(rect, _)| rect[2] > rect[0] && rect[3] > rect[1])
        };
        let Some(bounds) = bounding_rect(&plan.pixel_rects) else {
            return Self {
                offsets: vec![0],
                ..Self::default()
            };
        };
        let width = bounds[2] - bounds[0];
        let height = bounds[3] - bounds[1];
        let pixel_count = width as usize * height as usize;
        let mut offsets = vec![0_u32; pixel_count + 1];
        for (rect, _) in rects() {
            for y in rect[1]..rect[3] {
                let row = (y - bounds[1]) as usize * width as usize;
                for x in rect[0]..rect[2] {
                    offsets[row + (x - bounds[0]) as usize + 1] += 1;
                }
            }
        }
        let mut running = 0_u32;
        for offset in &mut offsets {
            running += *offset;
            *offset = running;
        }
        let mut cursor = offsets.clone();
        let mut slots = vec![0_u32; offsets[pixel_count] as usize];
        for (rect, &first) in rects() {
            let rect_width = rect[2] - rect[0];
            for y in rect[1]..rect[3] {
                let row = (y - bounds[1]) as usize * width as usize;
                for x in rect[0]..rect[2] {
                    let pixel = row + (x - bounds[0]) as usize;
                    let slot = first + (y - rect[1]) * rect_width + (x - rect[0]);
                    slots[cursor[pixel] as usize] = slot;
                    cursor[pixel] += 1;
                }
            }
        }
        Self {
            origin: (bounds[0], bounds[1]),
            size: (width, height),
            offsets,
            slots,
        }
    }

    /// The number of pixels of the rectangle.
    #[must_use]
    pub const fn pixel_count(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize
    }

    /// The slot indices of pixel `index` (row-major in the rectangle), empty out of range.
    #[must_use]
    pub fn pixel_slots(&self, index: usize) -> &[u32] {
        match (self.offsets.get(index), self.offsets.get(index + 1)) {
            (Some(&first), Some(&last)) => {
                self.slots.get(first as usize..last as usize).unwrap_or(&[])
            }
            _ => &[],
        }
    }
}

/// The CPU sum the reduction kernel must reproduce bit for bit: per pixel, `0 + slot₀ + slot₁ …`
/// in `f32`, in the CSR order.
#[must_use]
pub fn reduce_on_cpu(slots: &[[f32; 4]], csr: &PixelCsr) -> Vec<[f32; 4]> {
    (0..csr.pixel_count())
        .map(|pixel| {
            let mut sum = [0.0_f32; 3];
            for &slot in csr.pixel_slots(pixel) {
                let value = slots.get(slot as usize).copied().unwrap_or([0.0; 4]);
                for (total, add) in sum.iter_mut().zip(value) {
                    *total += add;
                }
            }
            [sum[0], sum[1], sum[2], 0.0]
        })
        .collect()
}

/// The uniform of the reduction kernel.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ReduceParams {
    /// The pixel count; the other lanes are unused.
    lanes: [u32; 4],
}

/// The reduction pipeline of `ctx`'s device, compiled once.
fn reduce_pipeline_for(ctx: &GpuContext) -> Arc<wgpu::ComputePipeline> {
    let mut slot = ctx
        .beam_caches
        .reduce_pipeline
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = slot.as_ref() {
        return Arc::clone(found);
    }
    let pipeline = Arc::new(compute::create_compute_pipeline(
        &ctx.device,
        "beam reduce fragments",
        REDUCE_BEAM_FRAGMENTS_SRC,
        REDUCE_ENTRY,
    ));
    *slot = Some(Arc::clone(&pipeline));
    drop(slot);
    pipeline
}

/// `(groups_x, groups_y)` covering `count` pixels within the device's dispatch limit, `None` when
/// the grid would not fit.
fn reduce_grid(ctx: &GpuContext, count: u32) -> Option<(u32, u32)> {
    let limit = ctx.device.limits().max_compute_workgroups_per_dimension;
    let groups = count.div_ceil(REDUCE_GROUP_SIZE).max(1);
    let x = groups.min(REDUCE_MAX_GROUPS_X.min(limit));
    let y = groups.div_ceil(x);
    (y <= limit).then_some((x, y))
}

/// Sums every pixel's fragment slots on the GPU in `csr`'s order.
///
/// The sums go into `[x, y, z, 0]` per pixel
/// of the CSR's rectangle (row-major); `None` when the device fails or declines. `slots` must
/// be a storage buffer of `vec4<f32>` fragment slots indexed by the CSR. The compute pipeline is
/// compiled once per device, and the call declines before allocating when a buffer would exceed
/// the device's binding limit or the dispatch grid would exceed its group limit.
#[must_use]
pub fn reduce_beam_fragments(
    ctx: &GpuContext,
    slots: &wgpu::Buffer,
    csr: &PixelCsr,
) -> Option<Vec<[f32; 4]>> {
    let pixels = csr.pixel_count();
    if pixels == 0 {
        return Some(Vec::new());
    }
    let mut scratch = ReduceScratch::default();
    let pending = submit_reduction(ctx, &mut scratch, slots, csr)?;
    let sums = pending.finish(&ctx.device)?;
    if ctx.failure().is_some() || sums.len() != pixels {
        return None;
    }
    Some(sums)
}

/// A grow-only device buffer: its handle and its size in bytes.
struct Grown {
    buffer: wgpu::Buffer,
    bytes: u64,
}

/// The reduction's offsets, slot-id, output, staging and params buffers, kept across the pieces of
/// a batch and reallocated only when a piece needs more than the current capacity.
///
/// A piece writes only the first bytes it needs, the kernel reads only inside the CSR it was
/// given, writes every output pixel below its count, and the readback copies and maps only the
/// piece's own byte range, so a stale tail from a bigger earlier piece is never read. The set
/// belongs to one device: a different context drops every buffer first.
#[derive(Default)]
pub struct ReduceScratch {
    owner: Option<Arc<AtomicBool>>,
    offsets: Option<Grown>,
    slot_ids: Option<Grown>,
    out: Option<Grown>,
    staging: Option<Grown>,
    params: Option<wgpu::Buffer>,
    allocations: u32,
}

impl ReduceScratch {
    /// How many buffer allocations this set has made (a test seam: reuse keeps it flat).
    #[must_use]
    pub const fn allocations(&self) -> u32 {
        self.allocations
    }

    /// Drops every buffer unless they were made on `ctx`'s device.
    fn adopt(&mut self, ctx: &GpuContext) {
        let same = self
            .owner
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &ctx.validation_error_seen));
        if !same {
            *self = Self {
                owner: Some(Arc::clone(&ctx.validation_error_seen)),
                allocations: self.allocations,
                ..Self::default()
            };
        }
    }
}

/// The buffer in `slot`, reallocated (and counted in `allocations`) when it holds fewer than
/// `need` bytes. Growth doubles up to `limit` but never below `need`.
fn grow<'a>(
    slot: &'a mut Option<Grown>,
    allocations: &mut u32,
    (device, limit): (&wgpu::Device, u64),
    (label, usage): (&str, wgpu::BufferUsages),
    need: u64,
) -> &'a wgpu::Buffer {
    let previous = slot.as_ref().map_or(0, |grown| grown.bytes);
    let make = |bytes: u64| {
        let bytes = bytes.next_multiple_of(16);
        Grown {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage,
                mapped_at_creation: false,
            }),
            bytes,
        }
    };
    let grown = if previous >= need {
        slot.get_or_insert_with(|| make(need))
    } else {
        *allocations += 1;
        slot.insert(make(need.max(previous.saturating_mul(2).min(limit))))
    };
    &grown.buffer
}

/// A reduction that is submitted and mapped but not yet read back.
pub struct PendingReduction {
    pixels: usize,
    staging: Option<wgpu::Buffer>,
    copy: Option<wgpu::SubmissionIndex>,
    receiver: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

impl PendingReduction {
    /// Waits for the readback (bounded by `compute::GPU_WAIT_TIMEOUT`), copies the sums of the
    /// piece's own byte range out and unmaps the staging buffer; `None` on a wait or map failure.
    ///
    /// Needs only the device, so a caller can release the renderer lock before calling it.
    #[must_use]
    pub fn finish(self, device: &wgpu::Device) -> Option<Vec<[f32; 4]>> {
        self.finish_with(device, None)
    }

    /// [`Self::finish`] that also records a timed-out wait as an abandoned dispatch of `ctx`'s
    /// device, so later beam entries decline at once (see [`busy_reason`]).
    #[must_use]
    pub fn finish_noting(self, ctx: &GpuContext) -> Option<Vec<[f32; 4]>> {
        self.finish_with(&ctx.device, Some(ctx))
    }

    /// The body of [`Self::finish`]; `note` is the context to record an abandoned wait on.
    ///
    /// On every failure the staging buffer is destroyed, so its pending map callback fires with an
    /// error now instead of keeping a mapped buffer alive after the caller drops its scratch.
    fn finish_with(
        self,
        device: &wgpu::Device,
        note: Option<&GpuContext>,
    ) -> Option<Vec<[f32; 4]>> {
        if self.pixels == 0 {
            return Some(Vec::new());
        }
        let (staging, copy, receiver) = (self.staging?, self.copy?, self.receiver?);
        if let Err(error) = device.poll(wgpu::PollType::Wait {
            submission_index: Some(copy.clone()),
            timeout: Some(compute::GPU_WAIT_TIMEOUT),
        }) {
            tracing::warn!(%error, pixels = self.pixels, "beam reduction readback wait failed");
            if let Some(ctx) = note {
                note_abandoned(ctx, copy);
            }
            staging.destroy();
            return None;
        }
        match receiver.recv_timeout(compute::GPU_WAIT_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "beam reduction readback mapping failed");
                staging.destroy();
                return None;
            }
            Err(error) => {
                tracing::warn!(%error, "beam reduction readback callback did not fire");
                staging.destroy();
                return None;
            }
        }
        let bytes = self.pixels as u64 * SLOT_BYTES;
        let sums = {
            let Ok(data) = staging.slice(0..bytes).get_mapped_range() else {
                staging.destroy();
                return None;
            };
            bytemuck::cast_slice::<u8, [f32; 4]>(&data).to_vec()
        };
        staging.unmap();
        Some(sums)
    }
}

/// The reason later beam entries decline while an abandoned dispatch still occupies the device.
pub const BUSY_REASON: &str = "GPU still busy with an abandoned beam dispatch";

/// Records that a wait on `index` timed out on `ctx`'s device and the GPU may still be running it.
///
/// Queue order makes the latest abandoned submission cover every earlier one, so one is kept.
pub fn note_abandoned(ctx: &GpuContext, index: wgpu::SubmissionIndex) {
    *ctx.beam_caches
        .abandoned
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(index);
}

/// [`BUSY_REASON`] while `ctx`'s device has not finished its last abandoned beam submission.
///
/// A poll of that submission that returns clears the state; a poll that times out (checked for a
/// millisecond only, never blocking the caller) leaves it set.
#[must_use]
pub fn busy_reason(ctx: &GpuContext) -> Option<&'static str> {
    let mut slot = ctx
        .beam_caches
        .abandoned
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let index = slot.as_ref()?.clone();
    let finished = ctx
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(std::time::Duration::from_millis(1)),
        })
        .is_ok();
    if finished {
        *slot = None;
    }
    drop(slot);
    (!finished).then_some(BUSY_REASON)
}

/// The scratch buffers one reduction binds, sized for its CSR and filled with its inputs.
struct ReduceBuffers<'a> {
    /// The CSR offsets (binding 201).
    offsets: &'a wgpu::Buffer,
    /// The slot ids the offsets point into (binding 202).
    slot_ids: &'a wgpu::Buffer,
    /// The per-pixel sums the shader writes (binding 203).
    out: &'a wgpu::Buffer,
    /// The mappable copy of the sums the host reads back.
    staging: &'a wgpu::Buffer,
    /// The uniform with the lane count (binding 204).
    params: &'a wgpu::Buffer,
}

/// Grows `scratch`'s buffers to the reduction of `csr` (`count` pixels, binding `limit` in bytes)
/// and writes the offsets, the slot ids and the parameters into them.
///
/// The caller has checked that no buffer passes `limit`; a buffer that is large enough is reused.
fn stage_reduction_inputs<'a>(
    ctx: &GpuContext,
    scratch: &'a mut ReduceScratch,
    csr: &PixelCsr,
    (count, limit): (u32, u64),
) -> ReduceBuffers<'a> {
    let (device, queue) = (&ctx.device, &ctx.queue);
    let sizes = (device, limit);
    let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    let out_bytes = u64::from(count) * SLOT_BYTES;
    let offsets = grow(
        &mut scratch.offsets,
        &mut scratch.allocations,
        sizes,
        ("beam reduce offsets", storage),
        csr.offsets.len() as u64 * OFFSET_BYTES,
    );
    queue.write_buffer(offsets, 0, bytemuck::cast_slice(&csr.offsets));
    let zero = [0_u32];
    let ids: &[u32] = if csr.slots.is_empty() {
        &zero
    } else {
        &csr.slots
    };
    let slot_ids = grow(
        &mut scratch.slot_ids,
        &mut scratch.allocations,
        sizes,
        ("beam reduce slot ids", storage),
        ids.len() as u64 * SLOT_ID_BYTES,
    );
    queue.write_buffer(slot_ids, 0, bytemuck::cast_slice(ids));
    let out = grow(
        &mut scratch.out,
        &mut scratch.allocations,
        sizes,
        (
            "beam reduce out",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        ),
        out_bytes,
    );
    let staging = grow(
        &mut scratch.staging,
        &mut scratch.allocations,
        sizes,
        (
            "beam reduce readback",
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        ),
        out_bytes,
    );
    let params = scratch.params.get_or_insert_with(|| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("beam reduce params"),
            size: size_of::<ReduceParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    });
    let uniform = ReduceParams {
        lanes: [count, 0, 0, 0],
    };
    queue.write_buffer(params, 0, bytemuck::bytes_of(&uniform));
    ReduceBuffers {
        offsets,
        slot_ids,
        out,
        staging,
        params,
    }
}

/// Records and submits the reduction of `csr` over `slots` with `scratch`'s buffers and starts
/// the staging map, without waiting; `None` when the device declines.
///
/// Declines (before allocating) when a buffer would exceed the binding limit or the dispatch
/// grid would exceed the group limit. Call [`PendingReduction::finish`] for the sums.
#[must_use]
pub fn submit_reduction(
    ctx: &GpuContext,
    scratch: &mut ReduceScratch,
    slots: &wgpu::Buffer,
    csr: &PixelCsr,
) -> Option<PendingReduction> {
    let pixels = csr.pixel_count();
    if pixels == 0 {
        return Some(PendingReduction {
            pixels,
            staging: None,
            copy: None,
            receiver: None,
        });
    }
    let count = u32::try_from(pixels).ok()?;
    let limit = leaf_binding_limit(ctx);
    let largest =
        reduction_max_buffer_bytes(Some([0, 0, csr.size.0, csr.size.1]), csr.slots.len() as u64);
    if largest > limit {
        return None;
    }
    let (groups_x, groups_y) = reduce_grid(ctx, count)?;
    let pipeline = reduce_pipeline_for(ctx);
    scratch.adopt(ctx);
    let (device, queue) = (&ctx.device, &ctx.queue);
    let out_bytes = pixels as u64 * SLOT_BYTES;
    let ReduceBuffers {
        offsets,
        slot_ids,
        out,
        staging,
        params,
    } = stage_reduction_inputs(ctx, scratch, csr, (count, limit));
    let bind_group = compute::bind_buffers(
        device,
        "beam reduce bind group",
        &pipeline,
        &[
            (200, slots),
            (201, offsets),
            (202, slot_ids),
            (203, out),
            (204, params),
        ],
    );
    let _ = compute::dispatch(
        device,
        queue,
        &pipeline,
        &bind_group,
        (groups_x, groups_y, 1),
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("beam reduce readback copy encoder"),
    });
    encoder.copy_buffer_to_buffer(out, 0, staging, 0, out_bytes);
    let copy = queue.submit(Some(encoder.finish()));
    let (sender, receiver) = std::sync::mpsc::channel();
    staging
        .slice(0..out_bytes)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    Some(PendingReduction {
        pixels,
        staging: Some(staging.clone()),
        copy: Some(copy),
        receiver: Some(receiver),
    })
}

/// Prefix sums over the leaves of a batch, the inputs of the caps rule.
struct LeafPrefix<'a> {
    vertices: Vec<u64>,
    nodes: Vec<u64>,
    pixels: Vec<u64>,
    /// The leaves' pixel rectangles, for the bounding rectangle of a range.
    rects: &'a [[u32; 4]],
}

impl<'a> LeafPrefix<'a> {
    fn new(packed: &PackedLeaves, plan: &'a FragmentPlan) -> Self {
        let mut prefix = Self {
            vertices: vec![0],
            nodes: vec![0],
            pixels: vec![0],
            rects: &plan.pixel_rects,
        };
        for (leaf, header) in packed.headers.iter().enumerate() {
            let vertices = u64::from(header.vertex_count);
            let last = |list: &[u64]| list.last().copied().unwrap_or(0);
            prefix.vertices.push(last(&prefix.vertices) + vertices);
            prefix
                .nodes
                .push(last(&prefix.nodes) + vertices * u64::from(header.node_count));
            prefix
                .pixels
                .push(last(&prefix.pixels) + plan.leaf_pixels(leaf));
        }
        prefix
    }

    /// The largest buffer the leaves `first..last` (in `tiles` tiles) need, in bytes.
    ///
    /// The reduction allocates by the bounding rectangle of the leaves, gaps included, so that
    /// size enters the maximum next to the sum of the leaves' own rectangles.
    fn max_bytes(&self, first: usize, last: usize, tiles: u64) -> u64 {
        let between = |list: &[u64]| list[last] - list[first];
        let pixels = between(&self.pixels);
        let bounds = bounding_rect(self.rects.get(first..last).unwrap_or(&[]));
        let reduction = reduction_max_buffer_bytes(bounds, pixels);
        [
            reduction,
            (last - first) as u64 * HEADER_BYTES,
            between(&self.vertices) * VERTEX_BYTES,
            between(&self.nodes) * NODE_BYTES,
            (tiles + 1) * OFFSET_BYTES,
            pixels * SLOT_BYTES,
            pixels * SLOT_ID_BYTES,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
}

/// Splits the tiles `tiles` into the fewest halves whose buffers all fit `limit_bytes`,
/// appending the tile-index ranges in order; `false` when a single tile still does not fit.
fn split_tiles(
    packed: &PackedLeaves,
    prefix: &LeafPrefix<'_>,
    tiles: Range<usize>,
    limit_bytes: u64,
    out: &mut Vec<Range<usize>>,
) -> bool {
    let first = packed.tile_offsets[tiles.start] as usize;
    let last = packed.tile_offsets[tiles.end] as usize;
    if prefix.max_bytes(first, last, (tiles.end - tiles.start) as u64) <= limit_bytes {
        out.push(tiles);
        return true;
    }
    if tiles.end - tiles.start <= 1 {
        return false;
    }
    let middle = tiles.start + (tiles.end - tiles.start) / 2;
    split_tiles(packed, prefix, tiles.start..middle, limit_bytes, out)
        && split_tiles(packed, prefix, middle..tiles.end, limit_bytes, out)
}

/// The caps rule (§14.7 item 6).
///
/// The tile-index ranges (indices into `packed.tiles`, in order) of the sub-batches obtained by
/// halving by tiles until each one's leaf buffers, fragment slots and slot-index list all fit
/// `limit_bytes`. `plan` is the whole batch's plan. `None` when the tile tables are malformed or a
/// single tile still does not fit (it is shaded on the CPU). A batch with no tiles gives an empty
/// list.
#[must_use]
pub fn split_into_batches(
    packed: &PackedLeaves,
    plan: &FragmentPlan,
    limit_bytes: u64,
) -> Option<Vec<Range<usize>>> {
    let tiles = packed.tiles.len();
    let offsets = &packed.tile_offsets;
    let well_formed = offsets.len() == tiles + 1
        && offsets.windows(2).all(|pair| pair[0] <= pair[1])
        && offsets
            .last()
            .is_some_and(|&end| end as usize <= packed.headers.len())
        && plan.pixel_rects.len() == packed.headers.len();
    if !well_formed {
        return None;
    }
    let mut ranges = Vec::new();
    if tiles == 0 {
        return Some(ranges);
    }
    let prefix = LeafPrefix::new(packed, plan);
    split_tiles(packed, &prefix, 0..tiles, limit_bytes, &mut ranges).then_some(ranges)
}

/// The sub-batch of `packed` holding the tiles `tiles`, with the vertex, node and CSR indices
/// rebased to start at zero; `None` when the tables are malformed.
///
/// A header's `first_node` is the first spectral node of the leaf's interval (the kernel derives
/// the node wavelengths from it), so it is copied unchanged.
#[must_use]
pub fn sub_batch(packed: &PackedLeaves, tiles: Range<usize>) -> Option<PackedLeaves> {
    let leaf_start = *packed.tile_offsets.get(tiles.start)? as usize;
    let leaf_end = *packed.tile_offsets.get(tiles.end)? as usize;
    let mut sub = PackedLeaves {
        tiles: packed.tiles.get(tiles.clone())?.to_vec(),
        tile_offsets: packed
            .tile_offsets
            .get(tiles.start..=tiles.end)?
            .iter()
            .map(|&offset| offset - packed.tile_offsets[tiles.start])
            .collect(),
        ..PackedLeaves::default()
    };
    for header in packed.headers.get(leaf_start..leaf_end)? {
        let first = header.first_vertex as usize;
        let source = packed
            .vertices
            .get(first..first.checked_add(header.vertex_count as usize)?)?;
        let first_vertex = u32::try_from(sub.vertices.len()).ok()?;
        for vertex in source {
            let start = vertex.first_node as usize;
            let nodes = packed
                .nodes
                .get(start..start.checked_add(header.node_count as usize)?)?;
            let mut moved = *vertex;
            moved.first_node = u32::try_from(sub.nodes.len()).ok()?;
            sub.vertices.push(moved);
            sub.nodes.extend_from_slice(nodes);
        }
        // `first_node` is the interval's first spectral node (not a position in the node array),
        // so it is kept; only the vertices carry node-array positions.
        sub.headers.push(GpuLeafHeader {
            first_vertex,
            ..*header
        });
    }
    Some(sub)
}
