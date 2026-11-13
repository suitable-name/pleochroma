//! The kept index per pixel and the CPU `kept` callback (beam guide §12.3).
//!
//! A residual sample at image point `x` with branch signature `s` and wavelength `λ` is "kept
//! here" when a leaf with signature `s`, a node set holding the bin of `λ` and `x` inside its
//! image polygon was traced by the beams. [`KeptIndex`] holds the kept regions of one tile with a
//! per-pixel list of the regions that can contain a point of that pixel, so the test touches a
//! handful of polygons. [`pack_kept`] lays a batch of them out for the GPU (`GpuKeptRegion` and
//! the per-pixel CSR of `PackedKept`), where `kept.wgsl` runs the same test.

use glam::{DVec2, Vec2};

use super::{
    beam::{
        GpuKeptRegion, KEPT_FLAG_SUBTREE_COMPLETE, KEPT_MAX_VERTICES, KeptRegion, Leaf, PackedKept,
        SpectralNode,
    },
    primary::TileRect,
};
use crate::renderer::path_cache::signature::pack_sig;

/// The slack (pixels) added to a region's bounding box when it is assigned to pixels, so the `f32`
/// rounding of its vertices never hides it from a pixel it touches.
const BBOX_SLACK_PX: f64 = 1.0e-4;

/// The kept regions of one tile with the per-pixel lists of the regions overlapping each pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptIndex {
    /// The tile the index covers.
    tile: TileRect,
    /// One region per leaf, in the leaf order of the tile.
    regions: Vec<KeptRegion>,
    /// CSR offsets into `indices`, one per tile pixel (row-major) plus one.
    offsets: Vec<u32>,
    /// Region indices per pixel, ascending.
    indices: Vec<u32>,
}

/// The kept region of a leaf: its image polygon in `f32`.
fn region_of(leaf: &Leaf) -> KeptRegion {
    let (points, count) = leaf.image.points_2d();
    let count = count.min(KEPT_MAX_VERTICES);
    let mut polygon = [Vec2::ZERO; KEPT_MAX_VERTICES];
    for (slot, point) in polygon.iter_mut().zip(&points[..count]) {
        *slot = point.as_vec2();
    }
    KeptRegion {
        polygon,
        len: u8::try_from(count).unwrap_or(u8::MAX),
        sig: leaf.sig,
        interval: leaf.spectrum,
        subtree_complete: false,
    }
}

/// `floor(v)` as `u32`, clamped to `lo..=hi`.
fn floor_clamped(v: f64, lo: u32, hi: u32) -> u32 {
    let low = f64::from(lo);
    let high = f64::from(hi);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to a valid u32 range just before the cast"
    )]
    let index = v.floor().clamp(low, high) as u32;
    index
}

/// The pixel columns and rows `(x0, x1, y0, y1)` (inclusive) of `tile` that the bounding box of
/// `region` can overlap; `None` when it misses the tile.
fn pixel_box(region: &KeptRegion, tile: TileRect) -> Option<(u32, u32, u32, u32)> {
    if tile.is_empty() || region.len == 0 {
        return None;
    }
    let ring = &region.polygon[..usize::from(region.len)];
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for &p in ring {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let cell = |v: f32, slack: f64| f64::from(v) + 0.5 + slack;
    let (first_x, last_x) = (cell(lo.x, -BBOX_SLACK_PX), cell(hi.x, BBOX_SLACK_PX));
    let (first_y, last_y) = (cell(lo.y, -BBOX_SLACK_PX), cell(hi.y, BBOX_SLACK_PX));
    if !(first_x.is_finite() && last_x.is_finite() && first_y.is_finite() && last_y.is_finite()) {
        return None;
    }
    if last_x < f64::from(tile.x0)
        || first_x.floor() >= f64::from(tile.x1)
        || last_y < f64::from(tile.y0)
        || first_y.floor() >= f64::from(tile.y1)
    {
        return None;
    }
    Some((
        floor_clamped(first_x, tile.x0, tile.x1 - 1),
        floor_clamped(last_x, tile.x0, tile.x1 - 1),
        floor_clamped(first_y, tile.y0, tile.y1 - 1),
        floor_clamped(last_y, tile.y0, tile.y1 - 1),
    ))
}

/// `true` when `q` lies in the counter-clockwise polygon of `region`, edges and vertices
/// inclusive (the convention of `ConvexPolygon::contains_2d(q, 0.0)` and of `kept.wgsl`).
fn region_contains(region: &KeptRegion, q: DVec2) -> bool {
    let n = usize::from(region.len);
    if n < 3 {
        return false;
    }
    (0..n).all(|i| {
        let a = region.polygon[i].as_dvec2();
        let b = region.polygon[(i + 1) % n].as_dvec2();
        (b - a).perp_dot(q - a) >= 0.0
    })
}

impl KeptIndex {
    /// The index of the kept regions of `leaves`, the leaves of `tile` in canonical order.
    ///
    /// Every leaf contributes its image polygon (at most [`KEPT_MAX_VERTICES`] vertices, the same
    /// bound as a `ConvexPolygon`) as an `f32` polygon with its signature and node interval;
    /// `subtree_complete` is false. The per-pixel lists are ascending in region index.
    #[must_use]
    pub fn from_leaves(tile: TileRect, leaves: &[Leaf]) -> Self {
        let regions: Vec<KeptRegion> = leaves.iter().map(region_of).collect();
        let pixels = tile.pixel_count();
        let width = tile.width() as usize;
        let boxes: Vec<Option<(u32, u32, u32, u32)>> =
            regions.iter().map(|r| pixel_box(r, tile)).collect();
        let slot = |x: u32, y: u32| (y - tile.y0) as usize * width + (x - tile.x0) as usize;
        let mut counts = vec![0_u32; pixels];
        for &(x0, x1, y0, y1) in boxes.iter().flatten() {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    counts[slot(x, y)] += 1;
                }
            }
        }
        let mut offsets = Vec::with_capacity(pixels + 1);
        let mut total = 0_u32;
        offsets.push(0);
        for &count in &counts {
            total += count;
            offsets.push(total);
        }
        let mut cursor: Vec<u32> = offsets[..pixels].to_vec();
        let mut indices = vec![0_u32; total as usize];
        for (index, bounds) in boxes.iter().enumerate() {
            let Some(&(x0, x1, y0, y1)) = bounds.as_ref() else {
                continue;
            };
            let region = u32::try_from(index).unwrap_or(u32::MAX);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let s = slot(x, y);
                    indices[cursor[s] as usize] = region;
                    cursor[s] += 1;
                }
            }
        }
        Self {
            tile,
            regions,
            offsets,
            indices,
        }
    }

    /// The tile the index covers.
    #[must_use]
    pub const fn tile(&self) -> TileRect {
        self.tile
    }

    /// The kept regions, in leaf order.
    #[must_use]
    pub fn regions(&self) -> &[KeptRegion] {
        &self.regions
    }

    /// The number of kept regions.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.regions.len()
    }

    /// `true` when nothing is kept.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The bytes the index holds on the heap: its regions, its per-pixel offsets and its region
    /// lists (the buffers' capacities, so the leaf cache's byte count is honest).
    #[must_use]
    pub fn heap_bytes(&self) -> u64 {
        let bytes = self.regions.capacity() * size_of::<KeptRegion>()
            + self.offsets.capacity() * size_of::<u32>()
            + self.indices.capacity() * size_of::<u32>();
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }

    /// The indices (ascending) of the regions that can hold a point of `pixel`; empty for a pixel
    /// outside the tile.
    #[must_use]
    pub fn pixel_regions(&self, pixel: (u32, u32)) -> &[u32] {
        if !self.tile.contains(pixel) {
            return &[];
        }
        let slot = (pixel.1 - self.tile.y0) as usize * self.tile.width() as usize
            + (pixel.0 - self.tile.x0) as usize;
        match (self.offsets.get(slot), self.offsets.get(slot + 1)) {
            (Some(&start), Some(&end)) => self
                .indices
                .get(start as usize..end as usize)
                .unwrap_or_default(),
            _ => &[],
        }
    }

    /// The pixel `(x, y)` holding `image_point` (cell `[x − 0.5, x + 0.5)`), when it is in the
    /// tile.
    fn pixel_of(&self, image_point: DVec2) -> Option<(u32, u32)> {
        let (fx, fy) = ((image_point.x + 0.5).floor(), (image_point.y + 0.5).floor());
        let inside = fx >= f64::from(self.tile.x0)
            && fx < f64::from(self.tile.x1)
            && fy >= f64::from(self.tile.y0)
            && fy < f64::from(self.tile.y1);
        inside.then(|| {
            (
                floor_clamped(fx, self.tile.x0, self.tile.x1 - 1),
                floor_clamped(fy, self.tile.y0, self.tile.y1 - 1),
            )
        })
    }

    /// The CPU `kept` adapter (beam guide §12.3): `true` when a residual sample at `image_point`
    /// with packed branch signature `sig` and wavelength `lambda_nm` is covered by a kept leaf.
    ///
    /// The node is `SpectralNode::of_wavelength(lambda_nm, n_lambda)`, the bin rule `kept.wgsl`
    /// uses. A region matches when `pack_sig` of its signature equals `sig`, its interval holds
    /// the node, and `image_point` lies in its polygon (counter-clockwise, edges inclusive). A
    /// point outside the tile is never kept.
    #[must_use]
    pub fn kept(&self, image_point: DVec2, sig: u64, lambda_nm: f32, n_lambda: u32) -> bool {
        let Some(pixel) = self.pixel_of(image_point) else {
            return false;
        };
        let node = SpectralNode::of_wavelength(lambda_nm, n_lambda).0;
        self.pixel_regions(pixel).iter().any(|&index| {
            self.regions.get(index as usize).is_some_and(|region| {
                pack_sig(region.sig) == sig
                    && region.interval.contains(node)
                    && region_contains(region, image_point)
            })
        })
    }
}

/// The number of pixel-to-region memberships [`KeptIndex::from_leaves`] stores for `leaves` in
/// `tile`: the sum of the leaves' pixel-rectangle areas.
///
/// This is the length of the index's region lists, the part of its memory that grows with the
/// overlap of the leaves; the pipeline's admission counts it before the index is built.
#[must_use]
pub fn kept_memberships(tile: TileRect, leaves: &[Leaf]) -> u64 {
    leaves.iter().fold(0_u64, |total, leaf| {
        let area = pixel_box(&region_of(leaf), tile).map_or(0, |(x0, x1, y0, y1)| {
            u64::from(x1 - x0 + 1) * u64::from(y1 - y0 + 1)
        });
        total.saturating_add(area)
    })
}

/// The kept indices of one batch, built from the batch's leaves (beam guide section 12.4).
///
/// The kept index lives one batch long: the pipeline builds this set when a batch's trees are
/// done, the residual engines read it (the CPU `kept` callback, [`pack_kept`] for the GPU), and
/// the returned vector is dropped after the batch's residual, unless the leaf cache keeps the
/// leaves (the indices are then rebuilt from them). One entry per tile of `tiles`, in order;
/// `leaves` holds `(tile index, leaves)` with the tile index in the row-major grid of
/// `tile_size`-pixel
/// tiles over a frame `frame_width` pixels wide, so `tile_size` is the adaptive tile and the beam
/// tile in one. A tile without an entry gets an empty index.
#[must_use]
pub fn batch_kept_indices<L: AsRef<[Leaf]>>(
    tiles: &[TileRect],
    leaves: &[(u32, L)],
    (frame_width, tile_size): (u32, u32),
) -> Vec<(TileRect, KeptIndex)> {
    tiles
        .iter()
        .map(|&rect| {
            (
                rect,
                tile_kept_index(rect, leaves, (frame_width, tile_size)),
            )
        })
        .collect()
}

/// The kept index of one tile `rect` of a batch, built from the leaves of its entry in `leaves`.
///
/// The entry is empty when the tile has none; the tile index is taken in the row-major grid of
/// `tile_size`-pixel tiles over a frame `frame_width` pixels wide. The one rule of
/// [`batch_kept_indices`] and of the pipeline's cancellable per-tile build, for owned and lent
/// (`Cow`) leaves alike.
#[must_use]
pub fn tile_kept_index<L: AsRef<[Leaf]>>(
    rect: TileRect,
    leaves: &[(u32, L)],
    (frame_width, tile_size): (u32, u32),
) -> KeptIndex {
    let tile_size = tile_size.max(1);
    let columns = frame_width.div_ceil(tile_size);
    let index = (rect.y0 / tile_size) * columns + rect.x0 / tile_size;
    let of_tile: &[Leaf] = leaves
        .iter()
        .find(|(tile, _)| *tile == index)
        .map_or(&[], |(_, tile_leaves)| tile_leaves.as_ref());
    KeptIndex::from_leaves(rect, of_tile)
}

/// The GPU form of a kept region.
fn gpu_region(region: &KeptRegion) -> GpuKeptRegion {
    let mut polygon = [[0.0_f32; 2]; KEPT_MAX_VERTICES];
    for (slot, point) in polygon.iter_mut().zip(&region.polygon) {
        *slot = point.to_array();
    }
    GpuKeptRegion {
        polygon,
        sig: region.sig,
        len: u32::from(region.len),
        first_node: u32::from(region.interval.first),
        last_node: u32::from(region.interval.last),
        flags: if region.subtree_complete {
            KEPT_FLAG_SUBTREE_COMPLETE
        } else {
            0
        },
        pad: [0; 2],
    }
}

/// What packing the kept indices of a batch will allocate, known before anything is allocated.
///
/// The packed form has two layouts (see [`PackedKept`]). The dense CSR spans the bounding
/// rectangle of the batch's tiles; it is used while that rectangle is at most [`DENSE_SLACK`]
/// times the
/// batch's own pixels. A sparser batch (two tiles at opposite corners of an 8K frame) gets the
/// tile-slot CSR, one cell per batch pixel plus a row table, so the table follows the batch and
/// never the frame. This type keeps both sizes, so a caller can budget, split or decline the batch
/// before packing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeptFootprint {
    /// The top-left pixel of the bounding rectangle of the non-empty tiles.
    pub origin: (u32, u32),
    /// The size of that rectangle; zero when there is no non-empty tile.
    pub size: (u32, u32),
    /// The pixels of the batch's tiles themselves (the compact slots of the batch).
    pub tile_pixels: u64,
    /// The pixel rows of the non-empty tiles: the entries of the tile-slot row table.
    pub rows: u64,
    /// The kept regions of all the tiles.
    pub regions: u64,
    /// The pixel-to-region memberships of all the tiles.
    pub memberships: u64,
}

/// The dense CSR is used while the bounding rectangle holds at most this many times the batch's
/// own pixels; the tile-slot CSR beyond.
pub const DENSE_SLACK: u64 = 2;

impl KeptFootprint {
    /// The pixels of the bounding rectangle: the length of the dense offsets table less one.
    #[must_use]
    pub fn bounding_pixels(&self) -> u64 {
        u64::from(self.size.0) * u64::from(self.size.1)
    }

    /// `true` when nothing is kept, so the batch packs to the single offset 0 without a table.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.regions == 0 || self.size.0 == 0 || self.size.1 == 0
    }

    /// `true` when the batch packs to the tile-slot CSR: it keeps something and its bounding
    /// rectangle holds more than [`DENSE_SLACK`] times its own pixels.
    #[must_use]
    pub fn uses_tile_slots(&self) -> bool {
        !self.is_empty() && self.bounding_pixels() > DENSE_SLACK.saturating_mul(self.tile_pixels)
    }

    /// The cells of the packed offsets table (one more entry follows): the batch's pixels in the
    /// tile-slot form, the bounding rectangle's in the dense form.
    #[must_use]
    pub fn cells(&self) -> u64 {
        if self.uses_tile_slots() {
            self.tile_pixels
        } else {
            self.bounding_pixels()
        }
    }

    /// The words the row table adds to the region lists: four per tile row in the tile-slot form,
    /// none in the dense form.
    #[must_use]
    pub fn row_table_words(&self) -> u64 {
        if self.uses_tile_slots() {
            self.rows.saturating_mul(4)
        } else {
            0
        }
    }

    /// The bytes the packed batch holds: the regions, the offsets table and the region lists with
    /// the row table. Zero for an empty kept set, which is represented directly.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        if self.is_empty() {
            return 0;
        }
        self.regions
            .saturating_mul(size_of::<GpuKeptRegion>() as u64)
            .saturating_add(self.cells().saturating_add(1).saturating_mul(4))
            .saturating_add(
                self.memberships
                    .saturating_add(self.row_table_words())
                    .saturating_mul(4),
            )
    }
}

/// The [`KeptFootprint`] of `indices`, from the tile rectangles and the index lengths alone.
#[must_use]
pub fn kept_footprint(indices: &[(TileRect, KeptIndex)]) -> KeptFootprint {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
    let mut footprint = KeptFootprint::default();
    for (rect, index) in indices {
        footprint.regions += index.regions.len() as u64;
        footprint.memberships += index.indices.len() as u64;
        if rect.is_empty() {
            continue;
        }
        footprint.tile_pixels += rect.pixel_count() as u64;
        footprint.rows += u64::from(rect.height());
        x0 = x0.min(rect.x0);
        y0 = y0.min(rect.y0);
        x1 = x1.max(rect.x1);
        y1 = y1.max(rect.y1);
    }
    if x0 < x1 && y0 < y1 {
        footprint.origin = (x0, y0);
        footprint.size = (x1 - x0, y1 - y0);
    }
    footprint
}

/// [`pack_kept`] unless the packed batch would pass `max_bytes`; `None` then, before any table is
/// allocated, so the caller can hand the batch to an engine that needs no packed form.
///
/// The check is [`KeptFootprint::bytes`], which an empty kept set always passes.
#[must_use]
pub fn try_pack_kept(indices: &[(TileRect, KeptIndex)], max_bytes: u64) -> Option<PackedKept> {
    (kept_footprint(indices).bytes() <= max_bytes).then(|| pack_kept(indices))
}

/// Packs the kept indices of a batch into the GPU layout (beam guide §14.7 item 3).
///
/// The regions of the tiles follow one another in the order given. The region indices of a pixel
/// are ascending. A batch that keeps nothing (no tile, or no region) gives a `0 x 0` CSR with the
/// single offset 0 and no table over the tiles.
///
/// The CSR is dense (the bounding rectangle of the tiles, row-major; a pixel outside every tile
/// has an empty list) while the rectangle holds at most [`DENSE_SLACK`] times the batch's pixels,
/// and otherwise the tile-slot form: one cell per batch pixel, tile after tile each row-major,
/// found through the row table at the tail of [`PackedKept::indices`].
#[must_use]
pub fn pack_kept(indices: &[(TileRect, KeptIndex)]) -> PackedKept {
    let footprint = kept_footprint(indices);
    if footprint.is_empty() {
        return PackedKept {
            pixel_offsets: vec![0],
            ..PackedKept::default()
        };
    }
    // The first cell of each tile in the tile-slot form: the prefix of the tiles' pixel counts.
    let mut starts = Vec::with_capacity(indices.len());
    let mut next = 0_usize;
    for (_, index) in indices {
        starts.push(next);
        next += index.tile.pixel_count();
    }
    let tile_slots = footprint.uses_tile_slots();
    let (x0, y0) = footprint.origin;
    let width = footprint.size.0 as usize;
    // The footprint passed the memory cap, so its cells fit in memory (and in `usize`).
    let cells = usize::try_from(footprint.cells()).unwrap_or(usize::MAX);
    let (regions, pixel_offsets, lists) = if tile_slots {
        pack_tables(indices, cells, |at, x, y| {
            let tile = indices[at].1.tile;
            starts[at] + (y - tile.y0) as usize * tile.width() as usize + (x - tile.x0) as usize
        })
    } else {
        pack_tables(indices, cells, |_, x, y| {
            (y - y0) as usize * width + (x - x0) as usize
        })
    };
    let mut packed = PackedKept {
        regions,
        origin: footprint.origin,
        size: footprint.size,
        pixel_offsets,
        indices: lists,
        span_count: 0,
    };
    if tile_slots {
        append_row_table(&mut packed, indices, &starts);
    }
    packed
}

/// The regions, the offsets table (`cells + 1` entries) and the region lists of `indices`, with
/// `slot(tile number, x, y)` giving the cell of a tile pixel.
fn pack_tables(
    indices: &[(TileRect, KeptIndex)],
    cells: usize,
    slot: impl Fn(usize, u32, u32) -> usize,
) -> (Vec<GpuKeptRegion>, Vec<u32>, Vec<u32>) {
    let mut regions = Vec::new();
    let mut bases = Vec::with_capacity(indices.len());
    // One table serves as the counts, then the offsets, then the insertion cursors.
    let mut pixel_offsets = vec![0_u32; cells + 1];
    for (at, (_, index)) in indices.iter().enumerate() {
        bases.push(u32::try_from(regions.len()).unwrap_or(u32::MAX));
        regions.extend(index.regions.iter().map(gpu_region));
        let tile = index.tile;
        for y in tile.y0..tile.y1 {
            for x in tile.x0..tile.x1 {
                pixel_offsets[slot(at, x, y) + 1] +=
                    u32::try_from(index.pixel_regions((x, y)).len()).unwrap_or(0);
            }
        }
    }
    for at in 1..=cells {
        pixel_offsets[at] += pixel_offsets[at - 1];
    }
    let mut lists = vec![0_u32; pixel_offsets[cells] as usize];
    for (at, ((_, index), &base)) in indices.iter().zip(&bases).enumerate() {
        let tile = index.tile;
        for y in tile.y0..tile.y1 {
            for x in tile.x0..tile.x1 {
                let s = slot(at, x, y);
                for &region in index.pixel_regions((x, y)) {
                    lists[pixel_offsets[s] as usize] = base + region;
                    pixel_offsets[s] += 1;
                }
            }
        }
    }
    // Each start became its end: shift the table back by one slot.
    pixel_offsets.copy_within(0..cells, 1);
    pixel_offsets[0] = 0;
    (regions, pixel_offsets, lists)
}

/// Appends the row table of the tile-slot form to `packed.indices`: one `[y, x0, x1, first cell]`
/// entry per tile row, sorted by `(y, x0)` so the shader finds a pixel's row by binary search.
fn append_row_table(packed: &mut PackedKept, indices: &[(TileRect, KeptIndex)], starts: &[usize]) {
    let cell = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
    let mut rows: Vec<[u32; 4]> = Vec::new();
    for ((_, index), &start) in indices.iter().zip(starts) {
        let tile = index.tile;
        for y in tile.y0..tile.y1 {
            let first = start + (y - tile.y0) as usize * tile.width() as usize;
            rows.push([y, tile.x0, tile.x1, cell(first)]);
        }
    }
    rows.sort_unstable();
    packed.span_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    packed.indices.extend(rows.into_iter().flatten());
}
