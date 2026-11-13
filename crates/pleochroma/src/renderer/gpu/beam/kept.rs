//! The kept buffer on the GPU (beam guide §14.7 item 3).
//!
//! It holds a batch's kept regions and the per-pixel CSR, bound by the reference variant's
//! `beam_kept` test. The CSR is dense over the batch's bounding rectangle, or, for a sparse batch,
//! one cell per batch pixel found through a row table (see [`PackedKept`]).
//!
//! The WGSL side is `shaders/beam/kept.wgsl` ([`kept_wgsl`]). Its function `beam_kept_test` is
//! called by the reference patch's `beam_kept` once the buffers are bound.

use std::cell::Cell;

use bytemuck::{Pod, Zeroable};

use super::buffers::leaf_binding_limit;
use crate::renderer::{
    beam::{
        beam::{GpuKeptRegion, PackedKept},
        residual::KeptFootprint,
    },
    gpu::{compute, context::GpuContext},
};

/// The WGSL unit of the kept test: the four bindings and `beam_kept_test`.
const KEPT_SRC: &str = include_str!("../../shaders/beam/kept.wgsl");

/// The binding of the `GpuKeptRegion` array (read-only storage).
pub const KEPT_BINDING_REGIONS: u32 = 210;

/// The binding of the per-pixel CSR offsets (read-only storage).
pub const KEPT_BINDING_PIXEL_OFFSETS: u32 = 211;

/// The binding of the per-pixel region indices (read-only storage).
pub const KEPT_BINDING_INDICES: u32 = 212;

/// The binding of the `KeptParams` uniform.
pub const KEPT_BINDING_PARAMS: u32 = 213;

/// The storage buffers the kept unit adds to a pipeline (regions, offsets, indices).
pub const KEPT_STORAGE_BUFFERS: u32 = 3;

/// The kept unit's WGSL text, to be appended to a unit that calls `beam_kept_test`.
#[must_use]
pub const fn kept_wgsl() -> &'static str {
    KEPT_SRC
}

const REGION_BYTES: u64 = size_of::<GpuKeptRegion>() as u64;
const WORD_BYTES: u64 = size_of::<u32>() as u64;

/// The uniform of the kept unit (`KeptParams` in `kept.wgsl`), 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct KeptParams {
    /// The dense CSR rectangle's top-left pixel `(x, y)`; zero in the tile-slot form.
    pub origin: [u32; 2],
    /// The dense CSR rectangle's size `(width, height)`; zero for an empty batch and in the
    /// tile-slot form.
    pub size: [u32; 2],
    /// The number of spectral nodes `N_λ` of the bin rule.
    pub n_lambda: u32,
    /// The number of regions.
    pub region_count: u32,
    /// The tile-slot form's row table: the number of rows and the index in the indices buffer of
    /// the first row word. `[0, 0]` selects the dense CSR.
    pub spans: [u32; 2],
}

/// The `KeptParams` of `packed` for `n_lambda` nodes, `None` when the CSR tables are malformed.
///
/// Malformed are offsets not one longer than the dense pixel count, a row table that does not fit
/// the indices or points past the offsets, and a count not representable in 32 bits.
#[must_use]
pub fn kept_params(packed: &PackedKept, n_lambda: u32) -> Option<KeptParams> {
    let (width, height) = packed.size;
    let pixels = u64::from(width) * u64::from(height);
    let offsets = packed.pixel_offsets.len() as u64;
    let region_count = u32::try_from(packed.regions.len()).ok()?;
    if packed.span_count > 0 {
        let table = packed.row_table();
        let in_range = table.len() == packed.span_count as usize * 4
            && offsets > 0
            && table.as_chunks::<4>().0.iter().all(|row| {
                row[1] <= row[2] && u64::from(row[3]) + u64::from(row[2] - row[1]) < offsets
            });
        if !in_range {
            return None;
        }
        let first_row = u32::try_from(packed.indices.len() - table.len()).ok()?;
        return Some(KeptParams {
            origin: [0, 0],
            size: [0, 0],
            n_lambda,
            region_count,
            spans: [packed.span_count, first_row],
        });
    }
    let (origin, size) = if offsets == pixels + 1 {
        (<[u32; 2]>::from(packed.origin), [width, height])
    } else if offsets == 0 && width == 0 && height == 0 {
        // Only a true `0 x 0` table may come without offsets; a lopsided size such as `3 x 0`
        // with no offsets is malformed (`malformed_csr_tables_are_refused`).
        ([0, 0], [0, 0])
    } else {
        return None;
    };
    Some(KeptParams {
        origin,
        size,
        n_lambda,
        region_count,
        spans: [0; 2],
    })
}

/// The byte size of the largest of the three storage buffers of `packed` (an empty array counts
/// one element, as on upload).
#[must_use]
pub fn max_kept_buffer_bytes(packed: &PackedKept) -> u64 {
    [
        packed.regions.len().max(1) as u64 * REGION_BYTES,
        packed.pixel_offsets.len().max(1) as u64 * WORD_BYTES,
        packed.indices.len().max(1) as u64 * WORD_BYTES,
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// The byte size of the largest of the three storage buffers a batch of footprint `footprint` will
/// pack to, known before the batch is packed.
///
/// Equals [`max_kept_buffer_bytes`] of the packed batch; an empty kept set is the single zero
/// element of each buffer, as on upload. A sparse batch is sized by its own pixels (the tile-slot
/// CSR: one offset per batch pixel, the row table after the region lists), a compact one by its
/// bounding rectangle.
#[must_use]
pub fn planned_kept_buffer_bytes(footprint: &KeptFootprint) -> u64 {
    let (regions, offsets, indices) = if footprint.is_empty() {
        (1, 1, 1)
    } else {
        (
            footprint.regions,
            footprint.cells().saturating_add(1),
            footprint
                .memberships
                .saturating_add(footprint.row_table_words())
                .max(1),
        )
    };
    [
        regions.saturating_mul(REGION_BYTES),
        offsets.saturating_mul(WORD_BYTES),
        indices.saturating_mul(WORD_BYTES),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// `true` when a batch of footprint `footprint` will fit one storage binding of `limit` bytes, so
/// the device check can decline before the batch is packed.
#[must_use]
pub fn kept_footprint_fits(footprint: &KeptFootprint, limit: u64) -> bool {
    planned_kept_buffer_bytes(footprint) <= limit
}

/// Uploads `data` as a read-only storage buffer, padding an empty array to one zero element (a
/// storage binding cannot be empty).
fn upload_storage<T: Pod>(device: &wgpu::Device, label: &str, data: &[T]) -> wgpu::Buffer {
    let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    if data.is_empty() {
        compute::upload(device, label, &[T::zeroed()], usage)
    } else {
        compute::upload(device, label, data, usage)
    }
}

thread_local! {
    /// The real uploads this thread made ([`upload_count`]).
    static UPLOADS: Cell<u64> = const { Cell::new(0) };
}

/// The number of real kept uploads ([`KeptBufferSet::upload`] that created buffers) the calling
/// thread has made: a counter for tests of "uploaded once per batch".
#[must_use]
pub fn upload_count() -> u64 {
    UPLOADS.with(Cell::get)
}

/// A batch's kept regions resident on the GPU.
///
/// Cloning shares the buffers (they are reference counted), so one upload serves every pass of a
/// batch.
#[derive(Clone)]
pub struct KeptBufferSet {
    /// `GpuKeptRegion` array.
    pub regions: wgpu::Buffer,
    /// The per-pixel CSR offsets (one extra entry).
    pub pixel_offsets: wgpu::Buffer,
    /// The per-pixel region indices.
    pub indices: wgpu::Buffer,
    /// A small uniform: the CSR origin and size, `N_λ`.
    pub params: wgpu::Buffer,
    /// The region count.
    pub region_count: u32,
    /// The number of spectral nodes `N_λ` the set was uploaded for.
    pub n_lambda: u32,
}

impl KeptBufferSet {
    /// Uploads `packed` for a render with `n_lambda` nodes; `None` when a buffer would exceed
    /// the storage-binding limit. An empty batch uploads one zero region so the bindings exist.
    ///
    /// `None` too when the CSR tables are malformed (see [`kept_params`]). The buffers are bound
    /// at [`KEPT_BINDING_REGIONS`], [`KEPT_BINDING_PIXEL_OFFSETS`], [`KEPT_BINDING_INDICES`] and
    /// [`KEPT_BINDING_PARAMS`].
    #[must_use]
    pub fn upload(ctx: &GpuContext, packed: &PackedKept, n_lambda: u32) -> Option<Self> {
        if max_kept_buffer_bytes(packed) > leaf_binding_limit(ctx) {
            return None;
        }
        let params = kept_params(packed, n_lambda)?;
        let device = &ctx.device;
        UPLOADS.with(|count| count.set(count.get() + 1));
        Some(Self {
            regions: upload_storage(device, "beam kept regions", &packed.regions),
            pixel_offsets: upload_storage(device, "beam kept pixel offsets", &packed.pixel_offsets),
            indices: upload_storage(device, "beam kept indices", &packed.indices),
            params: compute::upload(
                device,
                "beam kept params",
                &[params],
                wgpu::BufferUsages::UNIFORM,
            ),
            region_count: params.region_count,
            n_lambda,
        })
    }

    /// The `(binding, buffer)` pairs of the four kept bindings, for `compute::bind_buffers`.
    #[must_use]
    pub const fn bindings(&self) -> [(u32, &wgpu::Buffer); 4] {
        [
            (KEPT_BINDING_REGIONS, &self.regions),
            (KEPT_BINDING_PIXEL_OFFSETS, &self.pixel_offsets),
            (KEPT_BINDING_INDICES, &self.indices),
            (KEPT_BINDING_PARAMS, &self.params),
        ]
    }
}
