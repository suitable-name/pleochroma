// Adapted from pleochroma geometry/stone_metrics; MIT, Copyright (c) 2026 Niklas Gussek.
// Full license: LICENSE-MIT in this directory.
use super::{caliper::caliper_extents, normalize::SolidVertex};
use glam::DVec3;
use pleochroma::geometry::stone_metrics::{SolidMesh, SolidMetrics, SolidStatus};
const EPS_FACE: f64 = 2e-5;
const GIRDLE_NY: f64 = 1e-3_f32 as f64;
pub(super) type FaceRing = Option<(Vec<DVec3>, DVec3)>;
pub(super) struct Arrangement {
    pub deduped: Vec<(DVec3, f64)>,
    pub origin: Vec<usize>,
    pub scale: f64,
    pub verts: Vec<SolidVertex>,
}
pub(super) fn face_rings(arr: &Arrangement) -> Vec<FaceRing> {
    arr.deduped
        .iter()
        .map(|&(n, m)| face_ring(n, m, &arr.verts))
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
                .map_or(0.0, |(ring, centroid)| ring_area(n, ring, *centroid))
                / 3.0
        })
        .sum()
}
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

pub(super) fn face_ring(normal: DVec3, offset: f64, verts: &[SolidVertex]) -> FaceRing {
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

pub(super) fn ring_area(normal: DVec3, ring: &[DVec3], centroid: DVec3) -> f64 {
    let mut cross_sum = DVec3::ZERO;
    for i in 0..ring.len() {
        let a = ring[i] - centroid;
        let b = ring[(i + 1) % ring.len()] - centroid;
        cross_sum += a.cross(b);
    }
    0.5 * normal.dot(cross_sum).abs()
}

pub(super) fn measure_from_arrangement(
    arr: &Arrangement,
    rings: &[FaceRing],
) -> Option<(SolidMetrics, Vec<DVec3>)> {
    let planes = &arr.deduped;
    let verts = &arr.verts;
    let scale = arr.scale;
    let volume = volume_from_rings(arr, rings);
    if !(volume.is_finite() && volume > 0.0) {
        return None;
    }

    let y_max = verts
        .iter()
        .map(|s| s.v.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let y_min = verts.iter().map(|s| s.v.y).fold(f64::INFINITY, f64::min);

    // Girdle band: vertical extent of the vertices lying on any vertical plane.
    let mut girdle_top = f64::NEG_INFINITY;
    let mut girdle_bottom = f64::INFINITY;
    for &(n, m) in planes.iter().filter(|(n, _)| n.y.abs() <= GIRDLE_NY) {
        for s in verts {
            if (n.dot(s.v) - m).abs() <= EPS_FACE {
                girdle_top = girdle_top.max(s.v.y);
                girdle_bottom = girdle_bottom.min(s.v.y);
            }
        }
    }
    let has_girdle = girdle_top >= girdle_bottom;

    let x_max = verts
        .iter()
        .map(|s| s.v.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let x_min = verts.iter().map(|s| s.v.x).fold(f64::INFINITY, f64::min);
    let z_max = verts
        .iter()
        .map(|s| s.v.z)
        .fold(f64::NEG_INFINITY, f64::max);
    let z_min = verts.iter().map(|s| s.v.z).fold(f64::INFINITY, f64::min);
    let (dx, dz) = (x_max - x_min, z_max - z_min);
    let (width_axis, length_axis) = if dx <= dz { (dx, dz) } else { (dz, dx) };

    let outline: Vec<(f64, f64)> = verts.iter().map(|s| (s.v.x, s.v.z)).collect();
    let (width_caliper, length_caliper) =
        caliper_extents(&outline).unwrap_or((width_axis, length_axis));

    let vertex_count = verts.len();
    let scaled_verts = verts.iter().map(|s| s.v * scale).collect();

    Some((
        SolidMetrics {
            volume: volume * scale * scale * scale,
            width_axis: width_axis * scale,
            length_axis: length_axis * scale,
            width_caliper: width_caliper * scale,
            length_caliper: length_caliper * scale,
            total_height: (y_max - y_min) * scale,
            crown_height: has_girdle.then_some((y_max - girdle_top) * scale),
            pavilion_depth: has_girdle.then_some((girdle_bottom - y_min) * scale),
            girdle_thickness: has_girdle.then_some((girdle_top - girdle_bottom) * scale),
            vertex_count,
        },
        scaled_verts,
    ))
}
