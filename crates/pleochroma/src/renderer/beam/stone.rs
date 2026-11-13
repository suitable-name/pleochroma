//! `BeamStone`: f64 planes, facet rings, vertex ids, adjacency and the plane alias map (beam
//! guide §7.1).
//!
//! The stone is the renderer's stone: the facet planes are the f32 planes widened to f64, and the
//! rings come from `build_solid_mesh`, which computes every vertex once as a three-plane
//! intersection. The rings hold coordinates only, so this module derives what the beam tracer
//! needs on top of them:
//!
//! 1. vertex ids, by exact-bit matching of the ring coordinates;
//! 2. a one-time clean-up of near-duplicate and flat ring vertices (never per beam, so
//!    neighbouring facets agree);
//! 3. edge adjacency, each edge stored once with its vertex ids ascending;
//! 4. the plane alias map from the deduplicated planes back to every original facet index.
//!
//! A facet is addressed by its dense index `0..facet_count()`, in the order of the mesh rings
//! (ascending original index). [`BeamStone::original_facet`] and [`BeamStone::aliases`] translate
//! to the indices of the renderer's plane list.

use glam::DVec3;

use super::BeamOff;
use crate::geometry::{
    GpuFacetPlane,
    cuts::normals_coincide,
    stone_metrics::{SolidStatus, build_solid_mesh},
};

/// Vertices closer than this many bounding radii are one vertex.
///
/// The arrangement keeps a vertex in a facet ring when it lies within `EPS_FACE` (2e-5, stone of
/// unit scale) of the plane. Near-degenerate meets, where four planes almost share a point, leave
/// their vertices in all four rings up to twice `EPS_FACE` apart; this distance (2.5 `EPS_FACE`)
/// merges the whole cluster into one vertex.
pub const NEAR_DUPLICATE_REL: f64 = 5e-5;

/// A ring vertex closer than this many bounding radii to the line through its two ring neighbours
/// counts as flat.
pub const COLLINEAR_REL: f64 = 1e-9;

/// Two planes with coinciding normals and offsets within this are one plane (the arrangement's
/// own plane dedup rule).
const COINCIDENT_OFFSET: f64 = 1e-9;

/// A convex stone for beam tracing: planes, vertex ids, facet rings and edge adjacency.
#[derive(Clone, Debug)]
pub struct BeamStone {
    /// The unique vertices, ids ascending in order of first appearance in the rings.
    vertices: Vec<DVec3>,
    /// Per facet, the vertex ids of its ring, counter-clockwise seen from outside.
    rings: Vec<Vec<u32>>,
    /// Per facet, the outward normal and the offset `m` with "inside is `n . x <= m`".
    planes: Vec<(DVec3, f64)>,
    /// Per facet, every original facet index that is this plane, ascending.
    aliases: Vec<Vec<u32>>,
    /// Per original facet index, the dense facet it belongs to; `None` when the plane is cut away.
    facet_of_original: Vec<Option<u32>>,
    /// Per facet, per ring edge `i` (from `ring[i]` to `ring[i + 1]`), the facet across it.
    neighbours: Vec<Vec<Option<u32>>>,
    /// Every edge once, as `[lo, hi]` vertex ids, sorted ascending.
    edges: Vec<[u32; 2]>,
    /// The largest distance of a vertex from the origin.
    radius: f64,
}

impl BeamStone {
    /// Builds the stone from the renderer's facet planes.
    ///
    /// # Errors
    ///
    /// [`BeamOff::Concave`] when `tools_present` is true; [`BeamOff::DegenerateStone`] when the
    /// planes do not bound a closed solid, or when the solid's rings are not a closed convex
    /// surface after the clean-up.
    pub fn new(planes: &[GpuFacetPlane], tools_present: bool) -> Result<Self, BeamOff> {
        if tools_present {
            return Err(BeamOff::Concave);
        }
        if planes.len() < 4 {
            return Err(BeamOff::DegenerateStone);
        }
        let planes_f64: Vec<(DVec3, f64)> = planes.iter().map(|p| p.to_halfspace_f64()).collect();
        match build_solid_mesh(&planes_f64) {
            SolidStatus::Closed(mesh) => Self::from_rings(&planes_f64, &mesh.rings),
            SolidStatus::Unbounded { .. } | SolidStatus::Degenerate { .. } => {
                Err(BeamOff::DegenerateStone)
            }
        }
    }

    /// Builds the stone from f64 half-space planes and the facet rings of a solid mesh.
    ///
    /// `rings` pairs each ring with its index into `planes`; planes without a ring are cut away.
    /// Each ring lists its vertices counter-clockwise seen from outside.
    ///
    /// # Errors
    ///
    /// [`BeamOff::DegenerateStone`] when `rings` is empty, a ring index is out of range, a ring
    /// has fewer than three vertices after the clean-up, or an edge is not in exactly two rings.
    pub fn from_rings(
        planes: &[(DVec3, f64)],
        rings: &[(usize, Vec<DVec3>)],
    ) -> Result<Self, BeamOff> {
        if rings.is_empty() || rings.iter().any(|(index, _)| *index >= planes.len()) {
            return Err(BeamOff::DegenerateStone);
        }
        let (mut vertices, mut id_rings) = weld_vertices(rings)?;
        let radius = vertices.iter().map(|v| v.length()).fold(0.0, f64::max);
        if !(radius.is_finite() && radius > 0.0) {
            return Err(BeamOff::DegenerateStone);
        }
        merge_near_duplicates(&vertices, &mut id_rings, NEAR_DUPLICATE_REL * radius);
        remove_flat_vertices(&vertices, &mut id_rings, COLLINEAR_REL * radius);
        compact_ids(&mut vertices, &mut id_rings);
        if id_rings.iter().any(|ring| ring.len() < 3) {
            return Err(BeamOff::DegenerateStone);
        }
        let (edges, neighbours) = build_adjacency(&id_rings)?;
        let (facet_planes, aliases, facet_of_original) = build_alias_map(planes, rings)?;
        let radius = vertices.iter().map(|v| v.length()).fold(0.0, f64::max);
        Ok(Self {
            vertices,
            rings: id_rings,
            planes: facet_planes,
            aliases,
            facet_of_original,
            neighbours,
            edges,
            radius,
        })
    }

    /// The number of facets (rings) of the stone.
    #[must_use]
    pub const fn facet_count(&self) -> usize {
        self.rings.len()
    }

    /// The number of unique vertices.
    #[must_use]
    pub const fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// The position of vertex `id`.
    #[must_use]
    pub fn vertex(&self, id: u32) -> DVec3 {
        self.vertices[id as usize]
    }

    /// The vertex ids of `facet`'s ring, counter-clockwise seen from outside.
    ///
    /// Ring edge `i` runs from `ring[i]` to `ring[(i + 1) % len]`.
    #[must_use]
    pub fn ring(&self, facet: u32) -> &[u32] {
        &self.rings[facet as usize]
    }

    /// The outward normal and the offset `m` of `facet`: inside is `n . x <= m`.
    #[must_use]
    pub fn plane(&self, facet: u32) -> (DVec3, f64) {
        self.planes[facet as usize]
    }

    /// The facet across ring edge `edge_index` of `facet`, or `None` when the edge is open.
    ///
    /// For a stone built by this module every edge has a neighbour.
    #[must_use]
    pub fn neighbour(&self, facet: u32, edge_index: usize) -> Option<u32> {
        self.neighbours
            .get(facet as usize)
            .and_then(|edges| edges.get(edge_index))
            .copied()
            .flatten()
    }

    /// The two vertex ids of ring edge `edge_index` of `facet`, ascending.
    ///
    /// Ascending order is the canonical order for the shared pyramid planes (guide §13.3).
    #[must_use]
    pub fn edge_vertices(&self, facet: u32, edge_index: usize) -> [u32; 2] {
        let ring = self.ring(facet);
        let a = ring[edge_index];
        let b = ring[(edge_index + 1) % ring.len()];
        [a.min(b), a.max(b)]
    }

    /// Every edge once, as `[lo, hi]` vertex ids, sorted ascending.
    #[must_use]
    pub const fn edges(&self) -> &[[u32; 2]] {
        self.edges.as_slice()
    }

    /// The lowest original facet index of the plane of `facet`.
    #[must_use]
    pub fn original_facet(&self, facet: u32) -> u32 {
        self.aliases[facet as usize].first().copied().unwrap_or(0)
    }

    /// Every original facet index that is the plane of `facet`, ascending; the first is
    /// [`BeamStone::original_facet`].
    #[must_use]
    pub fn aliases(&self, facet: u32) -> &[u32] {
        &self.aliases[facet as usize]
    }

    /// The facet of an original facet index, or `None` for a plane that is cut away or out of
    /// range.
    #[must_use]
    pub fn facet_of_original(&self, original: u32) -> Option<u32> {
        self.facet_of_original
            .get(original as usize)
            .copied()
            .flatten()
    }

    /// The facets whose outward normal faces the point `eye`, in ascending order.
    ///
    /// A facet qualifies when `eye` lies strictly outside its plane: `n . eye > m`.
    #[must_use]
    pub fn front_facets(&self, eye: DVec3) -> Vec<u32> {
        self.planes
            .iter()
            .enumerate()
            .filter(|&(_, &(n, m))| n.dot(eye) > m)
            .map(|(index, _)| id_from(index))
            .collect()
    }

    /// The largest distance of a vertex from the origin.
    #[must_use]
    pub const fn bounding_radius(&self) -> f64 {
        self.radius
    }
}

/// A vertex or facet index as an id. Counts are checked against `u32` before ids are made, so the
/// saturation never occurs.
fn id_from(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// A count as a `u32`, or a refusal when it does not fit.
fn checked_id(index: usize) -> Result<u32, BeamOff> {
    u32::try_from(index).map_err(|_| BeamOff::DegenerateStone)
}

/// The bits of `x` with `-0.0` folded onto `+0.0`, so equal values compare equal.
fn canonical_bits(x: f64) -> u64 {
    // `-0.0 + 0.0` is `+0.0` in round-to-nearest; every other value is unchanged.
    (x + 0.0).to_bits()
}

/// Gives every distinct ring coordinate (exact bits) an id and rewrites the rings as ids.
///
/// Ids are assigned in order of first appearance in the rings, so they do not depend on bit
/// patterns. No hash map is involved.
fn weld_vertices(rings: &[(usize, Vec<DVec3>)]) -> Result<(Vec<DVec3>, Vec<Vec<u32>>), BeamOff> {
    let mut coords: Vec<DVec3> = Vec::new();
    let mut entries: Vec<([u64; 3], usize)> = Vec::new();
    for (_, ring) in rings {
        for &p in ring {
            let key = [
                canonical_bits(p.x),
                canonical_bits(p.y),
                canonical_bits(p.z),
            ];
            entries.push((key, coords.len()));
            coords.push(p);
        }
    }
    checked_id(coords.len())?;
    entries.sort_unstable();
    let mut run_first: Vec<usize> = Vec::new();
    let mut run_of: Vec<usize> = vec![0; coords.len()];
    let mut previous: Option<[u64; 3]> = None;
    for &(key, flat) in &entries {
        if previous != Some(key) {
            run_first.push(flat);
            previous = Some(key);
        }
        run_of[flat] = run_first.len() - 1;
    }
    let mut order: Vec<usize> = (0..run_first.len()).collect();
    order.sort_unstable_by_key(|&run| run_first[run]);
    let mut id_of_run: Vec<u32> = vec![0; run_first.len()];
    for (rank, &run) in order.iter().enumerate() {
        id_of_run[run] = id_from(rank);
    }
    let positions: Vec<DVec3> = order.iter().map(|&run| coords[run_first[run]]).collect();
    let mut start = 0;
    let mut id_rings: Vec<Vec<u32>> = Vec::with_capacity(rings.len());
    for (_, ring) in rings {
        let end = start + ring.len();
        id_rings.push((start..end).map(|f| id_of_run[run_of[f]]).collect());
        start = end;
    }
    Ok((positions, id_rings))
}

/// Merges vertices closer than `tol` into the lowest-id one and repairs the rings.
///
/// Done once on the whole stone, so every facet sees the same merge.
fn merge_near_duplicates(positions: &[DVec3], rings: &mut [Vec<u32>], tol: f64) {
    let mut representative: Vec<usize> = (0..positions.len()).collect();
    for i in 0..positions.len() {
        let found =
            (0..i).find(|&j| representative[j] == j && positions[i].distance(positions[j]) <= tol);
        representative[i] = found.unwrap_or(i);
    }
    for ring in rings.iter_mut() {
        let mut merged = false;
        for id in ring.iter_mut() {
            let target = id_from(representative[*id as usize]);
            merged |= target != *id;
            *id = target;
        }
        if merged {
            keep_first_occurrences(ring);
        }
    }
}

/// Drops every repeated id of a ring, keeping the first occurrence.
fn keep_first_occurrences(ring: &mut Vec<u32>) {
    let mut unique: Vec<u32> = Vec::with_capacity(ring.len());
    for &id in ring.iter() {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    *ring = unique;
}

/// Whether vertex `id` lies on the segment between its two ring neighbours within `tol`.
fn is_flat_in_ring(ring: &[u32], id: u32, positions: &[DVec3], tol: f64) -> bool {
    let Some(at) = ring.iter().position(|&x| x == id) else {
        return false;
    };
    let count = ring.len();
    let before = positions[ring[(at + count - 1) % count] as usize];
    let after = positions[ring[(at + 1) % count] as usize];
    let point = positions[id as usize];
    let span = after - before;
    let length = span.length();
    if length <= tol {
        return true;
    }
    let along = (point - before).dot(span);
    let on_segment = along >= 0.0 && along <= length * length;
    on_segment && (point - before).cross(span).length() <= tol * length
}

/// Removes a vertex that is flat in every ring holding it and held by at most two rings.
///
/// A real corner is held by three or more rings and is never flat in all of them, so only a
/// spurious point inside an edge goes. It leaves both rings, so the surface stays closed.
fn remove_flat_vertices(positions: &[DVec3], rings: &mut [Vec<u32>], tol: f64) {
    for index in 0..positions.len() {
        let id = id_from(index);
        let holders: Vec<usize> = rings
            .iter()
            .enumerate()
            .filter_map(|(r, ring)| ring.contains(&id).then_some(r))
            .collect();
        if holders.is_empty() || holders.len() > 2 {
            continue;
        }
        let removable = holders
            .iter()
            .all(|&r| rings[r].len() > 3 && is_flat_in_ring(&rings[r], id, positions, tol));
        if removable {
            for &r in &holders {
                rings[r].retain(|&x| x != id);
            }
        }
    }
}

/// Renumbers the vertices that rings still use, keeping their order, and drops the rest.
fn compact_ids(positions: &mut Vec<DVec3>, rings: &mut [Vec<u32>]) {
    let mut new_id: Vec<Option<u32>> = vec![None; positions.len()];
    for ring in rings.iter() {
        for &id in ring {
            new_id[id as usize] = Some(0);
        }
    }
    let mut kept: Vec<DVec3> = Vec::new();
    for (old, slot) in new_id.iter_mut().enumerate() {
        if slot.is_some() {
            *slot = Some(id_from(kept.len()));
            kept.push(positions[old]);
        }
    }
    for ring in rings.iter_mut() {
        for id in ring.iter_mut() {
            *id = new_id[*id as usize].unwrap_or(*id);
        }
    }
    *positions = kept;
}

/// The unique edges and, per ring edge, the facet across it.
type Adjacency = (Vec<[u32; 2]>, Vec<Vec<Option<u32>>>);

/// The unique edges and the facet across every ring edge.
///
/// An edge is shared iff both of its vertex ids occur consecutively in two rings; any other count
/// means the surface is not closed.
fn build_adjacency(rings: &[Vec<u32>]) -> Result<Adjacency, BeamOff> {
    let mut half_edges: Vec<(u32, u32, u32, usize)> = Vec::new();
    for (facet, ring) in rings.iter().enumerate() {
        let facet_id = checked_id(facet)?;
        for (at, &a) in ring.iter().enumerate() {
            let b = ring[(at + 1) % ring.len()];
            half_edges.push((a.min(b), a.max(b), facet_id, at));
        }
    }
    half_edges.sort_unstable();
    let mut neighbours: Vec<Vec<Option<u32>>> =
        rings.iter().map(|ring| vec![None; ring.len()]).collect();
    let mut edges: Vec<[u32; 2]> = Vec::new();
    for group in half_edges.chunk_by(|x, y| x.0 == y.0 && x.1 == y.1) {
        let [first, second] = group else {
            return Err(BeamOff::DegenerateStone);
        };
        if first.2 == second.2 {
            return Err(BeamOff::DegenerateStone);
        }
        edges.push([first.0, first.1]);
        neighbours[first.2 as usize][first.3] = Some(second.2);
        neighbours[second.2 as usize][second.3] = Some(first.2);
    }
    Ok((edges, neighbours))
}

/// Whether two planes are one plane by the arrangement's dedup rule.
fn same_plane(a: (DVec3, f64), b: (DVec3, f64)) -> bool {
    normals_coincide(a.0, b.0) && (a.1 - b.1).abs() < COINCIDENT_OFFSET
}

/// The plane bookkeeping: facet planes, aliases and the original-to-facet map.
type AliasMap = (Vec<(DVec3, f64)>, Vec<Vec<u32>>, Vec<Option<u32>>);

/// Maps every original facet index to the dense facet of its plane and picks each facet's plane.
///
/// Each ring seeds its own original index; any other original plane that coincides with a ring's
/// plane becomes an alias of the first such ring. A facet's plane is the plane of its lowest
/// alias, so the choice is the same on every call.
fn build_alias_map(
    planes: &[(DVec3, f64)],
    rings: &[(usize, Vec<DVec3>)],
) -> Result<AliasMap, BeamOff> {
    let mut aliases: Vec<Vec<u32>> = Vec::with_capacity(rings.len());
    let mut facet_of_original: Vec<Option<u32>> = vec![None; planes.len()];
    for (facet, (index, _)) in rings.iter().enumerate() {
        aliases.push(vec![checked_id(*index)?]);
        facet_of_original[*index] = Some(checked_id(facet)?);
    }
    for (original, &plane) in planes.iter().enumerate() {
        if facet_of_original[original].is_some() {
            continue;
        }
        let owner = rings
            .iter()
            .position(|(index, _)| same_plane(planes[*index], plane));
        if let Some(facet) = owner {
            aliases[facet].push(checked_id(original)?);
            facet_of_original[original] = Some(checked_id(facet)?);
        }
    }
    let mut facet_planes: Vec<(DVec3, f64)> = Vec::with_capacity(rings.len());
    for list in &mut aliases {
        list.sort_unstable();
        list.dedup();
        let lowest = list.first().copied().unwrap_or(0);
        facet_planes.push(planes[lowest as usize]);
    }
    Ok((facet_planes, aliases, facet_of_original))
}
