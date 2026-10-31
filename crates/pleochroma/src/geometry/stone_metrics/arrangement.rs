//! One plane arrangement, built once and shared by the mesh and the measures.
//!
//! [`build_solid_mesh`](super::build_solid_mesh) and
//! [`measure_solid`](super::measure_solid) both start from the same steps:
//! [`dedup_planes`], scale the offsets by [`pow2_scale_norm`], then enumerate the
//! feasible vertices ([`feasible_vertices`], the expensive part). An
//! [`Arrangement`] holds the result of those steps, and [`face_rings`] holds the
//! per-plane polygon rings both consumers read, so a caller that wants the mesh
//! *and* the figures
//! ([`build_solid_mesh_and_measure`](super::build_solid_mesh_and_measure))
//! pays for one enumeration and one ring per plane instead of two and three.
//!
//! The call order and arguments of the shared steps are exactly those the two
//! entry points always used, so every figure and every mesh position is
//! bit-identical to the separate calls. An arrangement and its rings live for
//! the duration of one call; nothing here is cached across calls.

use glam::DVec3;

use super::{
    max_abs_offset,
    mesh::{self, FaceRing},
    pow2_scale_norm,
    types::SolidStatus,
    vertices::{
        SolidVertex, dedup_origin_indices, dedup_planes, escaping_plane_indices, feasible_vertices,
    },
};

/// The deduplicated, normalised plane arrangement of one call.
pub(super) struct Arrangement {
    /// The deduplicated planes with every offset already divided by `scale`.
    pub(super) deduped: Vec<(DVec3, f64)>,
    /// For each entry of `deduped`, its index in the caller's original plane slice.
    pub(super) origin: Vec<usize>,
    /// The power-of-two normalisation factor ([`pow2_scale_norm`]).
    pub(super) scale: f64,
    /// The feasible vertices of `deduped`, in first-seen order (at least four).
    pub(super) verts: Vec<SolidVertex>,
}

/// Why an arrangement could not be built, with what the cheaper consumers need
/// and the expensive diagnostic left unrun.
pub(super) enum ArrangeFailure {
    /// A well-conditioned feasible vertex reached the blank box. Carries the
    /// normalised planes so [`arrange`] can run the escaping-plane scan; the
    /// measures never do.
    Unbounded {
        deduped: Vec<(DVec3, f64)>,
        origin: Vec<usize>,
    },
    /// Bounded, but fewer than four distinct vertices.
    TooFewVertices(usize),
}

/// Dedups, normalises and enumerates the vertices of `planes`.
///
/// The measures use this directly: on an unbounded arrangement they only need
/// to know it failed, and skipping [`escaping_plane_indices`] keeps their
/// failure path as cheap as it always was.
pub(super) fn try_arrange(planes: &[(DVec3, f64)]) -> Result<Arrangement, ArrangeFailure> {
    let deduped = dedup_planes(planes);
    let origin = dedup_origin_indices(planes, &deduped);
    let scale = pow2_scale_norm(max_abs_offset(&deduped));
    let deduped: Vec<(DVec3, f64)> = deduped.iter().map(|&(n, m)| (n, m / scale)).collect();

    let Some(verts) = feasible_vertices(&deduped) else {
        return Err(ArrangeFailure::Unbounded { deduped, origin });
    };
    if verts.len() < 4 {
        return Err(ArrangeFailure::TooFewVertices(verts.len()));
    }
    Ok(Arrangement {
        deduped,
        origin,
        scale,
        verts,
    })
}

/// [`try_arrange`], with a failure reported as the [`SolidStatus`] the mesh
/// builder returns: `Unbounded` naming the escaping planes (as indices into
/// `planes`, sorted and deduplicated) or `Degenerate` for fewer than four
/// vertices.
pub(super) fn arrange(planes: &[(DVec3, f64)]) -> Result<Arrangement, Box<SolidStatus>> {
    try_arrange(planes).map_err(|failure| match failure {
        ArrangeFailure::Unbounded { deduped, origin } => {
            let mut escaping: Vec<usize> = escaping_plane_indices(&deduped)
                .into_iter()
                .map(|i| origin[i])
                .collect();
            escaping.sort_unstable();
            escaping.dedup();
            Box::new(SolidStatus::Unbounded { escaping })
        }
        ArrangeFailure::TooFewVertices(vertex_count) => Box::new(SolidStatus::Degenerate {
            vertex_count,
            volume: None,
        }),
    })
}

/// One [`mesh::face_ring`] per deduplicated plane, in plane order.
///
/// Each entry carries the ring and the **pre-sort** centroid `face_ring`
/// returns; both the shoelace area and the triangulation must read that one
/// pair (see [`mesh::face_area`] for why the centroid must not be recomputed).
pub(super) fn face_rings(arr: &Arrangement) -> Vec<FaceRing> {
    arr.deduped
        .iter()
        .map(|&(n, m)| mesh::face_ring(n, m, &arr.verts))
        .collect()
}

/// The divergence-theorem volume `sum(m * area / 3)` over the planes, in
/// normalised units, from rings already built. Summed in plane order with
/// `Iterator::sum`, the same expression the separate entry points always used.
pub(super) fn volume_from_rings(arr: &Arrangement, rings: &[FaceRing]) -> f64 {
    arr.deduped
        .iter()
        .zip(rings)
        .map(|(&(n, m), r)| {
            m * r
                .as_ref()
                .map_or(0.0, |(ring, centroid)| mesh::ring_area(n, ring, *centroid))
                / 3.0
        })
        .sum()
}
