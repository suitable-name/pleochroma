//! [`build_solid_mesh`]: triangulated mesh extraction from a plane arrangement.
//!
//! It also holds the per-face polygon reconstruction ([`face_ring`],
//! [`face_area`], [`ring_area`]) it shares with
//! [`measure_solid`](super::measure_solid).
//!
//! The mesh is built from one [`Arrangement`] and one [`face_rings`] pass: the
//! ring (with its pre-sort centroid) each plane yields feeds both the volume
//! check and the triangulation, so no ring is built twice. A caller that also
//! wants the measures passes the same arrangement and rings to
//! `measure_from_arrangement` (see
//! [`build_solid_mesh_and_measure`](super::build_solid_mesh_and_measure)).

use glam::DVec3;

use crate::geometry::tool::ToolPrimitive;

use super::{
    EPS_FACE,
    arrangement::{Arrangement, arrange, face_rings, volume_from_rings},
    concave::{self, ToolBounds},
    types::{SolidMesh, SolidStatus},
    vertices::SolidVertex,
};

/// A plane's face ring and its pre-sort centroid, `None` when fewer than three
/// vertices lie on the plane.
pub(super) type FaceRing = Option<(Vec<DVec3>, DVec3)>;

#[cfg(test)]
thread_local! {
    /// Per-thread count of [`face_ring`] calls (test-only; each test runs on its own thread).
    pub(super) static FACE_RING_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Builds a triangulated, watertight mesh of the solid `planes` bounds -- or
/// reports why it doesn't bound one yet (see [`SolidStatus`]).
///
/// Only the *target* differs from [`measure_solid`](super::measure_solid); the
/// vertex enumeration is identical (the same plane dedup then vertex
/// enumeration, the same tolerances -- one shared `arrange` step), so a
/// [`SolidStatus::Closed`] mesh's own divergence-theorem volume (computed from
/// its triangles) matches `measure_solid`'s figure for the same `planes` --
/// exercised in this module's tests. A caller that wants both the mesh and the
/// figures should call [`build_solid_mesh_and_measure`](super::build_solid_mesh_and_measure),
/// which runs that enumeration once.
///
/// # Triangulation
///
/// Each face is fan-triangulated **about its own centroid**, not its first
/// ring vertex. A schedule's facets are frequently thin slivers (a tier
/// meeting a crease at a shallow angle), and fanning from a corner of a thin
/// polygon produces triangles whose long edge is nearly the whole polygon
/// diagonal and whose opposite angle is near zero -- exactly the
/// degenerate-triangle shape rasterizers and normal-dependent shading handle
/// worst. A centroid fan instead produces exactly `k` triangles for a
/// `k`-gon, each spanning one ring edge and the centroid, so no triangle's
/// area can exceed roughly `1/k` of the face's -- bounded regardless of how
/// thin the polygon is.
///
/// Internally normalises every plane offset by `pow2_scale_norm` of
/// the arrangement's own representative scale before running any of the
/// vertex-arrangement geometry below (the same absolute, order-1-tuned
/// epsilons [`measure_solid`](super::measure_solid) shares), then scales
/// every returned position and volume figure back up by the same factor --
/// see that function's doc comment for the full reasoning (identical here).
/// A design whose own scale already rounds to `2^0` builds a bit-identical
/// mesh to before this normalisation existed.
///
/// # Panics
///
/// Panics if the mesh has more than `u32::MAX` vertices, which a convex stone never reaches.
#[must_use]
pub fn build_solid_mesh(planes: &[(DVec3, f64)]) -> SolidStatus {
    let arr = match arrange(planes) {
        Ok(arr) => arr,
        Err(status) => return *status,
    };
    let rings = face_rings(&arr);
    mesh_from_arrangement(&arr, &rings)
}

/// The volume check and triangulation of [`build_solid_mesh`], from an
/// [`Arrangement`] and its [`face_rings`] (each ring is built once and feeds
/// both the volume and the triangles).
pub(super) fn mesh_from_arrangement(arr: &Arrangement, rings: &[FaceRing]) -> SolidStatus {
    let scale = arr.scale;
    let volume = volume_from_rings(arr, rings);
    if !(volume.is_finite() && volume > 0.0) {
        return SolidStatus::Degenerate {
            vertex_count: arr.verts.len(),
            volume: volume.is_finite().then_some(volume * scale * scale * scale),
        };
    }

    let mut mesh = SolidMesh::default();
    for (i, face) in rings.iter().enumerate() {
        let Some((ring, centroid)) = face else {
            continue;
        };
        let (normal, centroid) = (arr.deduped[i].0, *centroid);
        let facet_idx = arr.origin[i];
        let base = u32::try_from(mesh.positions.len()).expect("a mesh's vertex count fits in u32");

        // Centroid vertex first, then the ring in angular order -- see this
        // function's doc comment for why the fan pivots on the centroid
        // rather than `ring[0]`. Positions are scaled back to `planes`' own
        // real mast units here, at the point they enter the returned mesh --
        // `ring` itself (pushed into `mesh.rings` below) gets the same
        // treatment so a caller never observes the internal normalised scale.
        mesh.positions.push(centroid * scale);
        mesh.normals.push(normal);
        mesh.facet_id.push(facet_idx);
        for &v in ring {
            mesh.positions.push(v * scale);
            mesh.normals.push(normal);
            mesh.facet_id.push(facet_idx);
        }

        let k = u32::try_from(ring.len()).expect("a facet ring's vertex count fits in u32");
        for e in 0..k {
            let a = base + 1 + e;
            let b = base + 1 + (e + 1) % k;
            mesh.indices.push(base);
            mesh.indices.push(a);
            mesh.indices.push(b);
        }
        let scaled_ring: Vec<DVec3> = ring.iter().map(|&v| v * scale).collect();
        mesh.rings.push((facet_idx, scaled_ring));
    }

    SolidStatus::Closed(mesh)
}

/// Ordered polygon ring of the face polygon that plane `(normal, offset)`
/// contributes to the solid: the vertices lying on the plane (within
/// [`EPS_FACE`]), sorted by angle about their centroid using a deterministic
/// in-plane basis (the world axis least aligned with the normal). `None`
/// when fewer than three vertices lie on the plane -- the facet was cut away
/// entirely, the same case [`face_area`] reports as zero area.
///
/// This is the vertex-collection-and-ordering half of `face_area`'s work,
/// factored out so [`build_solid_mesh`] can reuse the
/// ring itself (for triangulation, and for an edge/picking pass) instead of
/// only the scalar area `face_area` shoelaces it down to.
pub(super) fn face_ring(normal: DVec3, offset: f64, verts: &[SolidVertex]) -> FaceRing {
    #[cfg(test)]
    FACE_RING_CALLS.with(|c| c.set(c.get() + 1));
    let on_face: Vec<DVec3> = verts
        .iter()
        .map(|s| s.v)
        .filter(|v| (normal.dot(*v) - offset).abs() <= EPS_FACE)
        .collect();
    if on_face.len() < 3 {
        return None;
    }

    // Deterministic in-plane basis: start from the world axis least aligned
    // with the normal.
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        DVec3::X
    } else if normal.y.abs() <= normal.z.abs() {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let basis_u = (seed - normal * normal.dot(seed)).normalize();
    let basis_v = normal.cross(basis_u);

    let centroid = on_face.iter().copied().sum::<DVec3>() / on_face.len() as f64;
    let mut angled: Vec<(f64, DVec3)> = on_face
        .into_iter()
        .map(|vert| {
            let d = vert - centroid;
            (basis_v.dot(d).atan2(basis_u.dot(d)), vert)
        })
        .collect();
    angled.sort_by(|x, y| x.0.total_cmp(&y.0));
    Some((angled.into_iter().map(|(_, v)| v).collect(), centroid))
}

/// Area of the face polygon that plane `(normal, offset)` contributes to the
/// solid: [`face_ring`]'s ordered polygon, shoelace-summed about its
/// centroid. Zero when fewer than three vertices lie on the plane (the facet
/// was cut away entirely).
///
/// Takes the centroid from [`face_ring`] rather than recomputing it from the
/// returned ring. That is not a convenience: `face_ring` sorts the ring by
/// angle, and float addition is not associative, so summing the same points
/// in post-sort order can differ in the last bit from the pre-sort
/// (as-filtered) order this function used before `face_ring` was split out.
/// [`measure_solid`](super::measure_solid)'s volume is a sum of `offset *
/// face_area(...) / 3` over every plane, so that last bit is observable -- the
/// volume is contractually byte-identical across that refactor (see this
/// module's tests). `face_ring` computing the centroid before it sorts is
/// what preserves the order for free; recomputing it here from the sorted
/// ring would silently break the guarantee, and recomputing it from a second
/// filter pass would cost an extra scan and allocation per face on a function
/// that runs over every plane of every design in the catalogue.
pub(super) fn face_area(normal: DVec3, offset: f64, verts: &[SolidVertex]) -> f64 {
    face_ring(normal, offset, verts)
        .map_or(0.0, |(ring, centroid)| ring_area(normal, &ring, centroid))
}

/// Shoelace area of one [`face_ring`] about the centroid that call returned:
/// the body [`face_area`] always ran, so an area computed from a stored ring is
/// bit-identical to one computed by `face_area` for the same plane.
pub(super) fn ring_area(normal: DVec3, ring: &[DVec3], centroid: DVec3) -> f64 {
    let mut cross_sum = DVec3::ZERO;
    for i in 0..ring.len() {
        let a = ring[i] - centroid;
        let b = ring[(i + 1) % ring.len()] - centroid;
        cross_sum += a.cross(b);
    }
    0.5 * normal.dot(cross_sum).abs()
}

/// Segments around a tool's axis in [`tessellate_tool`]'s polygon.
///
/// Halved on `wasm32` to bound the web build's mesh size; the equal-area radius
/// keeps the removed volume of a cylinder exact at either count.
pub const TOOL_SEGMENTS: usize = if cfg!(target_arch = "wasm32") { 24 } else { 48 };

/// Subdivision level of the icosphere that tessellates a ball tool (320 faces).
pub const TOOL_ICOSPHERE_LEVEL: u32 = 2;

/// Like [`build_solid_mesh`], but for a stone that is the plane arrangement
/// minus a union of convex `tools` (concave and fantasy facets).
///
/// With no tools this **is** [`build_solid_mesh`], so every plane-only caller is
/// bit-identical. Otherwise each tool is tessellated into a convex polytope
/// ([`tessellate_tool`]), the facets of the planar mesh are cut into the convex
/// pieces that lie outside every tool, and the tool surfaces inside the stone
/// are added with their winding reversed, giving a closed, consistently
/// oriented mesh of `P \ (T1 u T2 u ..)`. `piece_normals` and `edge_visible` are
/// filled. Tool `k` has facet id `planes.len() + k`.
///
/// Anything other than a closed planar solid is returned unchanged. If the tools
/// remove the whole stone the result is [`SolidStatus::Degenerate`].
#[must_use]
pub fn build_solid_mesh_geom(planes: &[(DVec3, f64)], tools: &[ToolPrimitive]) -> SolidStatus {
    let status = build_solid_mesh(planes);
    if tools.is_empty() {
        return status;
    }
    match status {
        SolidStatus::Closed(mesh) => concave::build(planes, tools, &mesh),
        other => other,
    }
}

/// Convex polytope approximating one tool, as outward half-spaces
/// `n . x <= m` with unit `n`.
///
/// Cylinders, frusta and bicones use `segments` planes around the axis at the
/// **equal-area** radius `r * sqrt(2 pi / (N sin(2 pi / N)))`, so a
/// cross-section keeps its area and a through-cut removes exactly the right
/// volume; balls use an icosphere of level [`TOOL_ICOSPHERE_LEVEL`] scaled to
/// the volume of the true sphere. Sweeps are the Minkowski sum with the stroke.
/// Empty for an invalid tool.
#[must_use]
pub fn tessellate_tool(tool: &ToolPrimitive, segments: usize) -> Vec<(DVec3, f64)> {
    concave::polytope(tool, segments).map_or_else(Vec::new, |p| p.planes)
}

/// An oriented box that contains the polytope [`tessellate_tool`] builds for `tool` at
/// the same `segments`, or `None` for an invalid tool.
///
/// For cheap exclusion tests ahead of a mesh build: tools whose boxes are separated
/// cannot overlap, and a plane that the whole box lies inside cannot cut the tool. The
/// box is never smaller than the polytope (the polygon around the axis reaches the
/// equal-area circumradius, which is a little more than the tool's own radius).
#[must_use]
pub fn tool_bounds(tool: &ToolPrimitive, segments: usize) -> Option<ToolBounds> {
    concave::bounds(tool, segments)
}

/// Signed volume of the solid `mesh` bounds by the divergence formula over
/// `rings`: each ring contributes `p0 . A / 3` with `A` its vector area.
///
/// Valid for any closed, consistently oriented surface, so it covers concave
/// meshes whose facets are several rings. T-junctions between pieces cost
/// nothing because only the polygon geometry enters. Coordinates are taken
/// relative to the first ring vertex to keep cancellation small far from the
/// origin; a closed surface's volume does not depend on that choice.
#[must_use]
pub fn mesh_volume(mesh: &SolidMesh) -> f64 {
    let Some(origin) = mesh
        .rings
        .iter()
        .find_map(|(_, ring)| ring.first().copied())
    else {
        return 0.0;
    };
    let mut acc = 0.0;
    for (_, ring) in &mesh.rings {
        let Some(&p0) = ring.first() else { continue };
        let mut area = DVec3::ZERO;
        for w in ring[1..].windows(2) {
            area += (w[0] - p0).cross(w[1] - p0);
        }
        acc += (p0 - origin).dot(area);
    }
    acc / 6.0
}
