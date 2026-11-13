//! The beam tracer's data types (beam guide §6.1, §6.4, §11.1, §14.7): beams, sample states,
//! spectral intervals, leaves, fragments, tile buffers, kept regions and the GPU leaf layout.

use bytemuck::{Pod, Zeroable};
use glam::{DMat3, DVec2, DVec3, Vec2, Vec3};

use super::geom::polygon::ConvexPolygon;
use crate::optics::{polarization::StokesVector, raytracer::Ray};

/// Channels per spectral group: the tracer's comb width (guide §9.1).
pub const GROUP: usize = 8;
/// The most vertices a leaf polygon has on the GPU; longer polygons are pre-split (§14.7 item 1).
pub const GPU_LEAF_MAX_VERTICES: usize = 8;
/// The most vertices of a kept region (§14.7 item 3).
pub const KEPT_MAX_VERTICES: usize = 12;
/// The longest branch prefix a leaf stores (facet events; above any `max_bounces` in use).
pub const MAX_PREFIX: usize = 32;
/// The visible range the spectral nodes cover, in nm (§9.1).
pub const LAMBDA_MIN_NM: f32 = 380.0;
/// The width of the visible range, in nm.
pub const LAMBDA_SPAN_NM: f32 = 400.0;

/// `n_lambda` as `f32`; node counts are small (at most a few hundred), so the cast is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "node counts are far below 2^24, exactly representable"
)]
const fn n_f32(n_lambda: u32) -> f32 {
    n_lambda as f32
}

/// One spectral node of the fixed node set (§9.1): node `index` of `n_lambda`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpectralNode(pub u16);

impl SpectralNode {
    /// The node centre `380 + (index + ½) · 400 / n_lambda` in nm.
    #[must_use]
    pub fn centre_nm(self, n_lambda: u32) -> f32 {
        (f32::from(self.0) + 0.5).mul_add(LAMBDA_SPAN_NM / n_f32(n_lambda), LAMBDA_MIN_NM)
    }

    /// The node's bin `[380 + index·Δ, 380 + (index+1)·Δ)` in nm, `Δ = 400 / n_lambda`.
    #[must_use]
    pub fn bin_nm(self, n_lambda: u32) -> (f32, f32) {
        let delta = LAMBDA_SPAN_NM / n_f32(n_lambda);
        let lo = f32::from(self.0).mul_add(delta, LAMBDA_MIN_NM);
        (lo, lo + delta)
    }

    /// The bin rule `node(λ) = min(floor((λ − 380) · n_lambda / 400), n_lambda − 1)` (§9.2),
    /// identical in WGSL. Wavelengths below 380 nm map to node 0.
    #[must_use]
    pub fn of_wavelength(lambda_nm: f32, n_lambda: u32) -> Self {
        // No division: `kept.wgsl` computes the same two correctly rounded operations (WGSL's
        // division is not correctly rounded). 0.0025 = 1 / 400 nm.
        let raw = ((lambda_nm - LAMBDA_MIN_NM) * (n_f32(n_lambda) * 0.0025)).floor();
        let last = n_lambda.saturating_sub(1);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to [0, n_lambda - 1] first, and n_lambda fits in u16"
        )]
        let node = raw.clamp(0.0, n_f32(last)) as u16;
        Self(node)
    }
}

/// An inclusive range of spectral nodes `first..=last` a beam carries (§9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpectralInterval {
    /// The first node.
    pub first: u16,
    /// The last node (inclusive).
    pub last: u16,
}

impl SpectralInterval {
    /// Every node of an `n_lambda` set.
    #[must_use]
    pub fn all(n_lambda: u32) -> Self {
        Self {
            first: 0,
            last: u16::try_from(n_lambda.saturating_sub(1)).unwrap_or(u16::MAX),
        }
    }

    /// One node.
    #[must_use]
    pub const fn single(node: u16) -> Self {
        Self {
            first: node,
            last: node,
        }
    }

    /// The number of nodes.
    #[must_use]
    pub const fn len(self) -> usize {
        (self.last - self.first) as usize + 1
    }

    /// Always `false`: an interval holds at least one node.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// The centre node, the hero of the shared `expand` calls (§9.2): `(first + last) / 2`.
    #[must_use]
    pub const fn centre(self) -> u16 {
        self.first + (self.last - self.first) / 2
    }

    /// `true` when `node` lies in the interval.
    #[must_use]
    pub const fn contains(self, node: u16) -> bool {
        node >= self.first && node <= self.last
    }

    /// The number of 8-node groups, the last one possibly partial.
    #[must_use]
    pub const fn groups(self) -> usize {
        self.len().div_ceil(GROUP)
    }

    /// The node centres of group `g` (nodes `first + 8g ..`); slots past `last` repeat the last
    /// node and must carry zero weight.
    #[must_use]
    pub fn group_lambdas(self, g: usize, n_lambda: u32) -> [f32; GROUP] {
        std::array::from_fn(|i| {
            let node = (usize::from(self.first) + g * GROUP + i).min(usize::from(self.last));
            SpectralNode(u16::try_from(node).unwrap_or(u16::MAX)).centre_nm(n_lambda)
        })
    }
}

/// The exact path state at one sample point of a beam (§6.4), as the path tracer would have it
/// for a ray through that image point along this branch. Always retraced, never interpolated.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleState {
    /// The image point (screen coordinates, §5.2).
    pub image: DVec2,
    /// Position and exact ray (Poynting) direction S.
    pub ray: Ray,
    /// The wave normal k (equal to S outside birefringent media).
    pub k: Vec3,
    /// The current segment's origin, so `expand_at` can derive `t`.
    pub segment_origin: Vec3,
    /// The previous bounce's plane-of-incidence normal (`rotate_stokes_to_plane_of_incidence`).
    pub prev_plane: Option<Vec3>,
    /// Inside the stone.
    pub inside: bool,
    /// The eigenmode label (extraordinary / mode B).
    pub is_ext: bool,
    /// The Stokes vectors per 8-node group of the beam's interval (§9.1).
    pub stokes: Vec<[StokesVector; GROUP]>,
    /// The optical depth accumulated so far per absorption sub-node (`S_λ` per node); empty when
    /// the material's absorption is not tracked.
    pub depth: Vec<f32>,
    /// The geometric path length inside the stone so far, in model units.
    pub path_length: f32,
}

/// The sample points of a beam: the vertices first, then the centroid, then the edge midpoints
/// (§6.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleStates {
    /// All states in that order.
    pub points: Vec<SampleState>,
    /// The number of vertices (the polygon's `len`).
    pub vertices: usize,
}

impl SampleStates {
    /// The state at vertex `i`.
    #[must_use]
    pub fn vertex(&self, i: usize) -> Option<&SampleState> {
        (i < self.vertices).then(|| self.points.get(i)).flatten()
    }

    /// The state at the centroid, when present.
    #[must_use]
    pub fn centroid(&self) -> Option<&SampleState> {
        self.points.get(self.vertices)
    }

    /// The state at the midpoint of edge `i` (from vertex `i` to `i + 1`), when present.
    #[must_use]
    pub fn midpoint(&self, i: usize) -> Option<&SampleState> {
        (i < self.vertices)
            .then(|| self.points.get(self.vertices + 1 + i))
            .flatten()
    }
}

/// The facet events of a branch, in order (§11.1), to retrace its rays later (re-light,
/// refinement). Each step is the word `branch_step` mixes: `(facet << 8) | (code << 1) | is_ext`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BranchPrefix {
    /// The step words; the first `len` are used.
    pub steps: [u32; MAX_PREFIX],
    /// The number of steps.
    pub len: u8,
}

impl Default for BranchPrefix {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl BranchPrefix {
    /// The empty prefix (a primary beam).
    pub const EMPTY: Self = Self {
        steps: [0; MAX_PREFIX],
        len: 0,
    };

    /// The used steps.
    #[must_use]
    pub fn steps(&self) -> &[u32] {
        &self.steps[..usize::from(self.len)]
    }

    /// This prefix plus one step; `None` when it is full.
    #[must_use]
    pub fn push(mut self, word: u32) -> Option<Self> {
        let i = usize::from(self.len);
        if i >= MAX_PREFIX {
            return None;
        }
        self.steps[i] = word;
        self.len += 1;
        Some(self)
    }
}

/// A convex bundle of rays sharing one branch: facet sequence, modes, spectral interval (§6.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Beam {
    /// The carrier facet, a `BeamStone` facet id.
    pub facet: u32,
    /// The polygon on the carrier plane (f64, stone coordinates).
    pub polygon: ConvexPolygon,
    /// Every ray of the beam passes through the apex (behind the carrier plane).
    pub apex: DVec3,
    /// Maps image space `(sx, sy, 1)` to carrier-plane coordinates (in `PlaneBasis` of the plane).
    pub image_to_plane: DMat3,
    /// The beam travels inside the stone.
    pub inside: bool,
    /// The branch signature so far (`branch_step` lanes, §14.3).
    pub sig: [u32; 2],
    /// The facet events so far.
    pub prefix: BranchPrefix,
    /// The interaction count.
    pub bounces: u32,
    /// The spectral interval this beam carries.
    pub spectrum: SpectralInterval,
    /// The exact states at the sample points.
    pub state: SampleStates,
}

/// A leaf: lighting-independent exit data of one branch over one image polygon (§11.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    /// The tile the leaf belongs to (row-major tile index).
    pub tile: u32,
    /// The image polygon (screen coordinates, in `Frame2::IMAGE`).
    pub image: ConvexPolygon,
    /// Per vertex and node, the exit direction ω (vertex-major: `dirs[v * nodes + n]`).
    /// Unnormalised for reflection leaves (exact by linear precision, §11.2).
    pub dirs: Vec<Vec3>,
    /// Per vertex and node, the Stokes intensity with absorption folded, without glare.
    pub weights: Vec<f32>,
    /// The spectral interval.
    pub spectrum: SpectralInterval,
    /// The branch signature.
    pub sig: [u32; 2],
    /// The facet events of the branch.
    pub prefix: BranchPrefix,
    /// A first-surface reflection: the fragment stage applies `surface_glare` (§11.1).
    pub glare: bool,
    /// The directions are the unnormalised reflected camera directions (exact interpolation).
    pub reflection: bool,
    /// The exit-direction tolerance `τ_ω` the leaf was built with (§11.2).
    pub tau_omega: f32,
    /// The image polygon before the bin-edge clipping of a dispersive stone (`spectral::shrink_leaf`
    /// cut `image` out of it), `None` when no clipping changed the leaf. A refinement bisects this
    /// polygon and re-clips the pieces, as a fresh render does (§14.10).
    pub pre_clip: Option<ConvexPolygon>,
}

impl Leaf {
    /// The number of nodes per vertex.
    #[must_use]
    pub const fn nodes(&self) -> usize {
        self.spectrum.len()
    }
}

/// One piece of a leaf inside one pixel cell (§11.5).
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    /// The leaf's index in its tile's leaf list.
    pub leaf: u32,
    /// The pixel `(x, y)`.
    pub pixel: (u32, u32),
    /// The piece, in image space.
    pub polygon: ConvexPolygon,
}

/// A tile's beam image: per-pixel XYZ in f64, row-major over the tile rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct TileBuffer {
    /// The tile's first pixel `(x0, y0)`.
    pub origin: (u32, u32),
    /// The tile's size `(width, height)` in pixels.
    pub size: (u32, u32),
    /// Per-pixel XYZ (before white balance and the end clamp).
    pub xyz: Vec<DVec3>,
}

impl TileBuffer {
    /// A zero buffer for the tile at `origin` of `size`.
    #[must_use]
    pub fn new(origin: (u32, u32), size: (u32, u32)) -> Self {
        Self {
            origin,
            size,
            xyz: vec![DVec3::ZERO; size.0 as usize * size.1 as usize],
        }
    }
}

/// A region of the image whose branch the beams computed (§12.3): residual samples with the
/// same signature and a wavelength in the interval's bins are not counted there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeptRegion {
    /// The region polygon in image space; the first `len` vertices are used.
    pub polygon: [Vec2; KEPT_MAX_VERTICES],
    /// The number of vertices.
    pub len: u8,
    /// The branch signature.
    pub sig: [u32; 2],
    /// The nodes the region keeps.
    pub interval: SpectralInterval,
    /// Every branch below this one is kept too (nothing of its subtree went to the residual).
    pub subtree_complete: bool,
}

/// The GPU leaf header (§14.7 item 1), 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuLeafHeader {
    /// The branch signature lanes.
    pub sig: [u32; 2],
    /// The first vertex in the vertex array.
    pub first_vertex: u32,
    /// The vertex count, at most [`GPU_LEAF_MAX_VERTICES`].
    pub vertex_count: u32,
    /// The first node of the interval.
    pub first_node: u32,
    /// The node count of the interval.
    pub node_count: u32,
    /// Bit 0: glare. Bit 1: reflection (unnormalised directions). Bit 2: per-node deltas present.
    pub flags: u32,
    /// The recorded `τ_ω`.
    pub tau_omega: f32,
}

/// [`GpuLeafHeader::flags`] bit: apply `surface_glare`.
pub const LEAF_FLAG_GLARE: u32 = 1;
/// [`GpuLeafHeader::flags`] bit: the directions are unnormalised reflected camera directions.
pub const LEAF_FLAG_REFLECTION: u32 = 2;
/// [`GpuLeafHeader::flags`] bit: per-node direction deltas are present (dispersive interval).
pub const LEAF_FLAG_DELTAS: u32 = 4;

/// One GPU leaf vertex (§14.7 item 1), 24 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuLeafVertex {
    /// The image point (screen coordinates).
    pub image: [f32; 2],
    /// The direction of the interval's hero (centre) node.
    pub hero_dir: [f32; 3],
    /// The first [`GpuLeafNode`] of this vertex (`node_count` follow).
    pub first_node: u32,
}

/// Per vertex and node (§14.7 item 1), 8 bytes: the f16 weight and the f16 direction delta to the
/// hero direction (zero when the interval is not dispersive).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuLeafNode {
    /// Low 16 bits: the weight (f16). High 16 bits: delta x (f16).
    pub weight_dx: u32,
    /// Low 16 bits: delta y (f16). High 16 bits: delta z (f16).
    pub dy_dz: u32,
}

/// A batch's leaves in the GPU layout, with the per-tile CSR in canonical order (§14.7 item 1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackedLeaves {
    /// One header per (pre-split) leaf.
    pub headers: Vec<GpuLeafHeader>,
    /// The vertices of every leaf.
    pub vertices: Vec<GpuLeafVertex>,
    /// The per-vertex node data.
    pub nodes: Vec<GpuLeafNode>,
    /// The tile indices of the batch, in order.
    pub tiles: Vec<u32>,
    /// CSR offsets: the leaves of `tiles[i]` are `headers[tile_offsets[i]..tile_offsets[i + 1]]`.
    pub tile_offsets: Vec<u32>,
}

// ---- Partitions, exact boundaries, kept regions on the GPU ----

/// One piece of a beam's partition by exit facet (§7.2): the region of the beam polygon whose
/// model rays reach facet `to` next.
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionPiece {
    /// The facet the piece's rays reach (a `BeamStone` facet id).
    pub to: u32,
    /// The piece on the beam's carrier plane.
    pub polygon: ConvexPolygon,
}

/// A beam's partition by exit facet (§7.2), before projection to the next facets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Partition {
    /// The pieces, in the flood's deterministic order.
    pub pieces: Vec<PartitionPiece>,
    /// The area removed as slivers (carrier-plane units), for drop accounting (§12.2).
    pub sliver_area: f64,
    /// The pieces plus the slivers tile the beam polygon within 1e-9 relative (§13.6).
    pub conserved: bool,
}

/// What a snapped boundary separates (§7.4 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BoundaryKind {
    /// Two children reaching different facets (the partition, §7.2).
    Facet {
        /// The lower facet id.
        a: u32,
        /// The higher facet id.
        b: u32,
    },
    /// The total-internal-reflection curve on a facet (§8.5).
    Tir {
        /// The facet.
        facet: u32,
    },
    /// A mode-relabelling or topology split (§10.2).
    Mode,
    /// A seam between two halves of one branch with different apexes.
    Seam,
    /// A tile edge.
    TileEdge,
}

/// The canonical key of one snapped value, computed once and shared (§7.4 steps 2, 3, 6, §13.3).
///
/// It is a kind tag and four words built from exact `f64` bit patterns and ids, ordered so both
/// neighbours build the same key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapKey {
    /// 0: endpoint on a parent edge; 1: junction; 2: seam crossing; 3: tile-edge crossing;
    /// 4: chord crossing.
    pub kind: u8,
    /// The key words (bit patterns of canonical endpoints, ids, crossing ordinals).
    pub words: [u64; 4],
}

/// Snapped values by key: a sorted `Vec` (no hash iteration), shared by the siblings of a beam.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeamStore {
    entries: Vec<(SnapKey, DVec2)>,
}

impl SeamStore {
    /// The stored value of `key`.
    #[must_use]
    pub fn get(&self, key: &SnapKey) -> Option<DVec2> {
        self.entries
            .binary_search_by(|(k, _)| k.cmp(key))
            .ok()
            .map(|i| self.entries[i].1)
    }

    /// Stores `value` under `key` unless the key is present, and returns the stored value (the
    /// first one wins, so every user of a key sees one value).
    pub fn get_or_insert(&mut self, key: SnapKey, value: DVec2) -> DVec2 {
        match self.entries.binary_search_by(|(k, _)| k.cmp(&key)) {
            Ok(i) => self.entries[i].1,
            Err(i) => {
                self.entries.insert(i, (key, value));
                value
            }
        }
    }

    /// The number of stored values.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when nothing is stored.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Classifies the exact ray through an image point for one boundary (§7.4).
///
/// The class id says which side the ray falls on (facet `Gi` or `Gj`, TIR or not, mode a or b); it
/// is `None` when the exact ray fits neither side (a bracket failure, §7.4 step 8).
pub trait Classifier {
    /// The class of the exact ray through `image_point`.
    fn classify(&self, image_point: DVec2) -> Option<u32>;
}

/// One kept region in the GPU layout (§14.7 item 3), 128 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuKeptRegion {
    /// The polygon in image space; the first `len` vertices are used.
    pub polygon: [[f32; 2]; KEPT_MAX_VERTICES],
    /// The branch signature lanes.
    pub sig: [u32; 2],
    /// The vertex count.
    pub len: u32,
    /// The first kept node.
    pub first_node: u32,
    /// The last kept node (inclusive).
    pub last_node: u32,
    /// Bit 0: subtree complete (§12.4 prefix early-out).
    pub flags: u32,
    /// Padding to 128 bytes.
    pub pad: [u32; 2],
}

/// [`GpuKeptRegion::flags`] bit: every branch below the region is kept too.
pub const KEPT_FLAG_SUBTREE_COMPLETE: u32 = 1;

/// A batch's kept regions in the GPU layout with the per-pixel CSR of overlapping regions
/// (§14.7 item 3).
///
/// The CSR has one of two forms. The dense form (`span_count == 0`)
/// covers the bounding rectangle `origin`/`size` of the batch's tiles, row-major. The tile-slot
/// form (`span_count > 0`) covers only the batch's own pixels: the cell of a pixel is its slot in
/// the tiles taken in order, each tile row-major, and the pixel is found through the row table at
/// the tail of `indices`. `origin` and `size` still describe the bounding rectangle in both forms,
/// but the shader reads them only in the dense form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackedKept {
    /// The regions.
    pub regions: Vec<GpuKeptRegion>,
    /// The bounding rectangle's first pixel `(x0, y0)` (the batch's tiles); the dense CSR's origin.
    pub origin: (u32, u32),
    /// The bounding rectangle's size `(width, height)`; the dense CSR's size.
    pub size: (u32, u32),
    /// Per cell (the dense CSR's pixels row-major over `size`, or the tile slots), the start into
    /// the region lists of `indices`; one extra entry at the end.
    pub pixel_offsets: Vec<u32>,
    /// Region indices per cell, ascending, then (tile-slot form only) the row table: `span_count`
    /// entries of four words `[y, x0, x1, first cell]`, sorted by `(y, x0)`, one per tile row.
    pub indices: Vec<u32>,
    /// The number of rows in the row table at the tail of `indices`; zero for the dense form.
    pub span_count: u32,
}

impl PackedKept {
    /// The words of the row table at the tail of `indices` (four per row); empty for the dense
    /// form or when `indices` is shorter than the table claims.
    #[must_use]
    pub fn row_table(&self) -> &[u32] {
        let words = self.span_count as usize * 4;
        self.indices
            .len()
            .checked_sub(words)
            .and_then(|from| self.indices.get(from..))
            .unwrap_or_default()
    }

    /// The region lists of `indices`: everything before the row table.
    #[must_use]
    pub fn region_lists(&self) -> &[u32] {
        let words = self.span_count as usize * 4;
        self.indices
            .get(..self.indices.len().saturating_sub(words))
            .unwrap_or_default()
    }
}

// ---- Spectral intervals ----

impl SpectralInterval {
    /// The interval's outer bin edges `(λ_lo, λ_hi)` in nm: `380 + first·Δ` and `380 + (last+1)·Δ`
    /// (§9.2), where the kept region is snapped under `RegionMode::EndIntersection`.
    #[must_use]
    pub fn bin_edges_nm(self, n_lambda: u32) -> (f32, f32) {
        let (lo, _) = SpectralNode(self.first).bin_nm(n_lambda);
        let (_, hi) = SpectralNode(self.last).bin_nm(n_lambda);
        (lo, hi)
    }

    /// The two halves of the interval (split at the centre node, the first half keeping it), or
    /// `None` for a single node.
    #[must_use]
    pub const fn halves(self) -> Option<(Self, Self)> {
        if self.first == self.last {
            return None;
        }
        let mid = self.centre();
        Some((
            Self {
                first: self.first,
                last: mid,
            },
            Self {
                first: mid + 1,
                last: self.last,
            },
        ))
    }
}

/// Why a spectral interval was split (§9.2 validity checks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntervalSplit {
    /// The branch signature or the child set differs between the end nodes and the centre.
    Topology,
    /// An end node's own-hero Stokes values differ from the shared ones beyond `τ_w`.
    StateCheck,
    /// A child of the shared expansion is `diverged` (§8.5).
    Diverged,
    /// The exact boundary does not move monotonically with λ (the crossings at the centre and the
    /// two bin edges are not ordered).
    NotMonotone,
}

/// The number of [`IntervalSplit`] causes.
pub const INTERVAL_SPLIT_CAUSES: usize = 4;

impl IntervalSplit {
    /// The counter slot of this cause.
    #[must_use]
    pub const fn slot(self) -> usize {
        match self {
            Self::Topology => 0,
            Self::StateCheck => 1,
            Self::Diverged => 2,
            Self::NotMonotone => 3,
        }
    }
}

/// The trace of one end node (or bin edge) of an interval as its own hero (§9.2): the exact states
/// at the beam's sample points at `lambda_nm`, `None` where the retracer refused.
#[derive(Clone, Debug, PartialEq)]
pub struct EndNodeTrace {
    /// The node the trace belongs to.
    pub node: u16,
    /// The wavelength traced: the node centre for the state check, a bin edge for snapping.
    pub lambda_nm: f32,
    /// One state per sample point of the beam, in the beam's sample order.
    pub states: Vec<Option<SampleState>>,
}

// ---- Birefringence ----

/// The propagation mode of a beam inside a birefringent stone (§10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BeamMode {
    /// Isotropic medium, or outside the stone.
    Isotropic,
    /// Uniaxial ordinary ray (`is_ext == false`).
    Ordinary,
    /// Uniaxial extraordinary ray (`is_ext == true`): its Poynting vector leaves the plane of
    /// incidence, its apex is fitted (§10.1).
    Extraordinary,
    /// Biaxial resolved mode A (`is_ext == false`), the tracer's 50/50 scalar split (§10.3).
    BiaxialA,
    /// Biaxial resolved mode B (`is_ext == true`).
    BiaxialB,
}

impl BeamMode {
    /// The mode bit the signature and the prefix carry.
    #[must_use]
    pub const fn is_ext(self) -> bool {
        matches!(self, Self::Extraordinary | Self::BiaxialB)
    }

    /// `true` when reflecting this mode is not a mirror image, so its apex is refitted (§10.1).
    #[must_use]
    pub const fn needs_refit(self) -> bool {
        matches!(self, Self::Extraordinary | Self::BiaxialA | Self::BiaxialB)
    }
}

/// `θ_axis`: sample-point k-vectors within this angle of an optic axis stop subdividing and go to
/// the residual (§10.2), matching `expand`'s scalar switch `|k × c|² ≤ 1e-6`.
pub const OPTIC_AXIS_GUARD_RAD: f64 = 1e-3;
