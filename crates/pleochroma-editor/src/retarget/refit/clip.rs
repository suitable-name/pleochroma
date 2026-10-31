//! The facet a convex solid has on one of its planes, cut out of that plane alone.
//!
//! The refit's bisection asks for the area of ONE facet at dozens of masts. Building the whole
//! solid for each answer (`build_solid_mesh`: every vertex of the arrangement, every ring) pays
//! for ~170 facets to read one. The facet of a convex solid on its plane `P` is the part of `P`
//! inside every other half-space, so [`facet_area`] cuts exactly that: it starts from a large
//! square in `P` and clips it by each other plane (Sutherland-Hodgman), O(planes x polygon size)
//! instead of an arrangement walk.
//!
//! The mesh measures a facet differently in small ways (it merges vertices closer than 1e-6 and
//! counts a vertex within 2e-5 of the plane, all in units of the stone's scale), so the two agree
//! to rounding wherever the facet is an ordinary polygon, and can differ by those tolerances on a
//! sliver. And the mesh answers `0.0` for a solid that is not closed. [`facet_area`] therefore
//! answers only where it can prove the mesh would give the same figure, and returns `None` (the
//! caller builds the mesh) everywhere else:
//!
//! - Without a [`Reference`]: a closed solid with the same normals, seen once by the caller. The
//!   planes of a probe differ only in one offset, and a polytope's boundedness depends on the
//!   normals alone, so a solid that was closed stays bounded wherever it is nonempty.
//! - When another plane lies nearly on top of the probed one, so the mesh's merge and
//!   duplicate rules decide which facet owns the ring.
//! - When the clipped facet is too thin for the solid to certainly be solid around it (see
//!   [`INTERIOR_MARGIN`]), so a zero-volume or `< 4` vertex mesh is possible.
//! - When the facet is gone but the plane is within [`MISS_MARGIN`] of the solid, where the mesh
//!   may still collect vertices that lie within its tolerance of the plane.

use glam::{DVec2, DVec3};
use pleochroma::geometry::{cuts::normals_coincide, stone_metrics::SolidMesh};

/// Another plane with a coinciding normal this close (as a fraction of the stone's largest
/// offset) to the probed plane makes the answer the mesh's to give: its duplicate rule drops
/// planes within 1e-9, and its face rings gather every vertex within 2e-5 of the plane.
const COPLANAR_BAND: f64 = 1e-3;

/// A plane that the solid misses by less than this (fraction of the largest offset) is not
/// declared missed: the mesh's tolerances are 100 times smaller, which is the margin for a
/// solid whose corners are ill-conditioned.
const MISS_MARGIN: f64 = 1e-3;

/// The ball, as a fraction of the largest offset, that must fit inside the solid around a facet
/// for the mesh to certainly be nonempty, bounded away from zero volume and rich in vertices.
const INTERIOR_MARGIN: f64 = 1e-4;

/// Half the side of the square the facet is clipped out of, as a multiple of the largest offset:
/// beyond the mesh's own bounding box (64 in units of the offset scale rounded to a power of
/// two), so anything the mesh could still see lies inside it.
const SQUARE_HALF_SIDE: f64 = 100.0;

/// What a closed solid with the same normals tells about every probe: a point inside it.
///
/// Made from the mesh of a probe the solid was closed at ([`Self::from_mesh`]); the point is the
/// mean of the mesh's ring vertices, inside the convex solid, and so inside the solid of every
/// other plane set that drops the probed plane.
#[derive(Debug, Clone, Copy)]
pub(super) struct Reference {
    inside: DVec3,
}

impl Reference {
    /// The reference of a closed solid's mesh, `None` for a mesh with no usable vertex.
    pub(super) fn from_mesh(mesh: &SolidMesh) -> Option<Self> {
        let (sum, count) = mesh
            .rings
            .iter()
            .flat_map(|(_, ring)| ring.iter())
            .fold((DVec3::ZERO, 0_usize), |(sum, count), vertex| {
                (sum + *vertex, count + 1)
            });
        if count == 0 {
            return None;
        }
        let inside = sum / count as f64;
        inside.is_finite().then_some(Self { inside })
    }
}

/// The plane at `level` along `normal`, with an orthonormal basis for the points on it.
struct Frame {
    origin: DVec3,
    along_u: DVec3,
    along_v: DVec3,
}

impl Frame {
    fn new(normal: DVec3, level: f64) -> Self {
        let hat = normal.normalize();
        let along_u = hat.any_orthonormal_vector();
        Self {
            origin: normal * (level / normal.length_squared()),
            along_u,
            along_v: hat.cross(along_u),
        }
    }

    /// The 3D point at plane coordinates `at`.
    fn point(&self, at: DVec2) -> DVec3 {
        self.origin + self.along_u * at.x + self.along_v * at.y
    }
}

/// Keeps the part of the convex `polygon` where `a x + b y <= c`.
///
/// `scratch` is a reusable buffer; the clipped polygon ends up in `polygon`, which is left
/// untouched (and nothing is allocated) when every vertex is already inside.
fn clip(polygon: &mut Vec<DVec2>, scratch: &mut Vec<DVec2>, a: f64, b: f64, c: f64) {
    let excess = |at: DVec2| a.mul_add(at.x, b * at.y) - c;
    if polygon.iter().all(|&at| excess(at) <= 0.0) {
        return;
    }
    scratch.clear();
    if let Some(&last) = polygon.last() {
        let (mut previous, mut previous_excess) = (last, excess(last));
        for &current in polygon.iter() {
            let current_excess = excess(current);
            if (previous_excess < 0.0 && current_excess > 0.0)
                || (previous_excess > 0.0 && current_excess < 0.0)
            {
                let along = previous_excess / (previous_excess - current_excess);
                scratch.push(previous + (current - previous) * along);
            }
            if current_excess <= 0.0 {
                scratch.push(current);
            }
            (previous, previous_excess) = (current, current_excess);
        }
    }
    std::mem::swap(polygon, scratch);
}

/// The part of the plane at `level` along `planes[skip]`'s normal that lies inside every other
/// plane: empty (no vertex), a point or segment (fewer than three vertices), or a convex polygon.
fn clip_face(
    planes: &[(DVec3, f64)],
    skip: usize,
    level: f64,
    half_side: f64,
) -> (Frame, Vec<DVec2>) {
    let frame = Frame::new(planes[skip].0, level);
    let mut polygon = vec![
        DVec2::new(-half_side, -half_side),
        DVec2::new(half_side, -half_side),
        DVec2::new(half_side, half_side),
        DVec2::new(-half_side, half_side),
    ];
    let mut scratch = Vec::with_capacity(16);
    for (index, &(normal, offset)) in planes.iter().enumerate() {
        if index == skip {
            continue;
        }
        clip(
            &mut polygon,
            &mut scratch,
            normal.dot(frame.along_u),
            normal.dot(frame.along_v),
            offset - normal.dot(frame.origin),
        );
        if polygon.is_empty() {
            break;
        }
    }
    (frame, polygon)
}

/// Area of a convex polygon (shoelace about its first vertex).
fn polygon_area(polygon: &[DVec2]) -> f64 {
    let Some(&origin) = polygon.first() else {
        return 0.0;
    };
    let twice: f64 = polygon
        .windows(2)
        .skip(1)
        .map(|pair| (pair[0] - origin).perp_dot(pair[1] - origin))
        .sum();
    0.5 * twice.abs()
}

/// Whether the solid around `polygon` (the facet of `planes[plane_index]`) holds a ball of
/// [`INTERIOR_MARGIN`] of `extent`, and the polygon stays clear of the clipping square.
///
/// The ball is centred half way from the polygon's centroid towards the opposite side of the
/// solid; its radius is at least half the least distance from the centroid to any other plane.
fn has_room(
    planes: &[(DVec3, f64)],
    plane_index: usize,
    frame: &Frame,
    polygon: &[DVec2],
    extent: f64,
    half_side: f64,
) -> bool {
    if polygon
        .iter()
        .any(|at| at.x.abs() > 0.5 * half_side || at.y.abs() > 0.5 * half_side)
    {
        return false;
    }
    let centroid = frame.point(polygon.iter().copied().sum::<DVec2>() / polygon.len() as f64);
    let hat = planes[plane_index].0.normalize();
    let mut nearest = f64::INFINITY;
    let mut depth = f64::INFINITY;
    for (index, &(normal, offset)) in planes.iter().enumerate() {
        if index == plane_index {
            continue;
        }
        let length = normal.length();
        let slack = (offset - normal.dot(centroid)) / length;
        nearest = nearest.min(slack);
        // How squarely plane `index` faces back at the probed plane.
        let facing = -normal.dot(hat) / length;
        if facing > 1e-9 {
            depth = depth.min(slack / facing);
        }
    }
    nearest >= 2.0 * INTERIOR_MARGIN * extent && depth.is_finite()
}

/// The area of the facet on `planes[plane_index]` of the convex solid `planes` bound, as
/// `build_solid_mesh` would report it for a closed solid -- or `None` when that cannot be
/// vouched for (see the module doc), in which case the caller measures the mesh.
///
/// `reference` is from a closed solid whose planes differ from `planes` only in the offset of
/// `planes[plane_index]`. A plane list with a non-finite entry, or an index past its end, is
/// never vouched for.
pub(super) fn facet_area(
    planes: &[(DVec3, f64)],
    plane_index: usize,
    reference: &Reference,
) -> Option<f64> {
    let &(normal, offset) = planes.get(plane_index)?;
    let extent = planes.iter().try_fold(0.0_f64, |largest, &(n, m)| {
        (n.is_finite() && m.is_finite() && n.length_squared() > 0.25)
            .then_some(largest.max(m.abs()))
    })?;
    if extent <= 0.0 {
        return None;
    }
    let near_coplanar = planes.iter().enumerate().any(|(index, &(n, m))| {
        index != plane_index
            && normals_coincide(normal, n)
            && (offset - m).abs() < COPLANAR_BAND * extent
    });
    if near_coplanar {
        return None;
    }

    let half_side = SQUARE_HALF_SIDE * extent;
    let (frame, polygon) = clip_face(planes, plane_index, offset, half_side);
    if polygon.len() >= 3 {
        let area = polygon_area(&polygon);
        return (area > 0.0 && has_room(planes, plane_index, &frame, &polygon, extent, half_side))
            .then_some(area);
    }

    // No facet. The mesh answers `0.0` whether or not the solid closes, as long as no vertex lies
    // near the plane: shift the plane in by a margin and find the solid still entirely on its
    // inner side (the reference point is inside the solid of the other planes, so a shifted plane
    // that misses that solid has it wholly below).
    let shifted = MISS_MARGIN.mul_add(-extent, offset);
    let (_, still_there) = clip_face(planes, plane_index, shifted, half_side);
    (still_there.is_empty() && normal.dot(reference.inside) < shifted).then_some(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::stone_metrics::{SolidStatus, build_solid_mesh};

    /// The axis-aligned box `[-1, 1]^3`, in the order +x -x +y -y +z -z.
    fn cube() -> Vec<(DVec3, f64)> {
        vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 1.0),
            (DVec3::NEG_Y, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ]
    }

    /// What the refit measured before: the shoelace area of the mesh's ring for `plane_index`,
    /// and the reference of the mesh.
    fn mesh_area(planes: &[(DVec3, f64)], plane_index: usize) -> (f64, Option<Reference>) {
        match build_solid_mesh(planes) {
            SolidStatus::Closed(mesh) => {
                let area = mesh
                    .rings
                    .iter()
                    .filter(|(plane, ring)| *plane == plane_index && ring.len() >= 3)
                    .map(|(_, ring)| {
                        let sum: DVec3 = ring
                            .windows(2)
                            .skip(1)
                            .map(|pair| (pair[0] - ring[0]).cross(pair[1] - ring[0]))
                            .sum();
                        0.5 * sum.length()
                    })
                    .sum();
                (area, Reference::from_mesh(&mesh))
            }
            _ => (0.0, None),
        }
    }

    fn reference_of(planes: &[(DVec3, f64)]) -> Reference {
        mesh_area(planes, 0).1.expect("the planes close")
    }

    #[test]
    fn a_cube_face_is_clipped_to_its_square() {
        let planes = cube();
        let reference = reference_of(&planes);
        for index in 0..planes.len() {
            let area = facet_area(&planes, index, &reference).expect("an ordinary facet");
            assert!((area - 4.0).abs() < 1e-12, "face {index}: {area}");
        }
    }

    #[test]
    fn a_corner_cut_matches_the_mesh() {
        let mut planes = cube();
        let normal = DVec3::new(1.0, 1.0, 1.0).normalize();
        planes.push((normal, 1.2));
        let reference = reference_of(&planes);
        let (expected, _) = mesh_area(&planes, 6);
        assert!(expected > 0.1, "the cut must leave a facet: {expected}");
        let area = facet_area(&planes, 6, &reference).expect("an ordinary facet");
        assert!((area - expected).abs() < 1e-12, "{area} vs {expected}");
        // The faces it cut into a pentagon.
        let (expected, _) = mesh_area(&planes, 0);
        let area = facet_area(&planes, 0, &reference).expect("an ordinary facet");
        assert!((area - expected).abs() < 1e-12, "{area} vs {expected}");
    }

    #[test]
    fn a_plane_that_misses_the_solid_has_no_area() {
        let mut planes = cube();
        planes.push((DVec3::Y, 5.0));
        let reference = reference_of(&planes);
        assert_eq!(mesh_area(&planes, 6).0, 0.0);
        assert_eq!(facet_area(&planes, 6, &reference), Some(0.0));
        // A plane that misses the corner by 5e-4 is inside the margin: the shifted plane reaches
        // the corner, so the miss is not vouched for.
        let mut planes = cube();
        let normal = DVec3::new(1.0, 1.0, 1.0).normalize();
        planes.push((normal, 3.0_f64.sqrt() + 5e-4));
        let reference = reference_of(&planes);
        assert_eq!(mesh_area(&planes, 6).0, 0.0);
        assert_eq!(facet_area(&planes, 6, &reference), None);
    }

    #[test]
    fn a_plane_that_swallows_the_solid_is_not_vouched_for() {
        // The plane cuts the whole box away on the far side of the reference point.
        let reference = reference_of(&cube());
        let mut planes = cube();
        planes.push((DVec3::Y, -3.0));
        assert_eq!(facet_area(&planes, 6, &reference), None);
    }

    #[test]
    fn planes_lying_on_top_of_each_other_are_left_to_the_mesh() {
        let reference = reference_of(&cube());
        let mut planes = cube();
        planes.push((DVec3::Y, 1.0));
        assert_eq!(facet_area(&planes, 2, &reference), None);
        assert_eq!(facet_area(&planes, 6, &reference), None);
        // Within the band but not equal: the mesh may merge their vertices.
        let mut planes = cube();
        planes.push((DVec3::Y, 1.0 + 5e-6));
        assert_eq!(facet_area(&planes, 2, &reference), None);
    }

    #[test]
    fn a_sliver_of_a_facet_is_left_to_the_mesh() {
        let reference = reference_of(&cube());
        // A plane nicking the corner: a facet a few ten-thousandths across, whose corners lie
        // closer to the neighbouring faces than the room a facet must have around it.
        let mut planes = cube();
        let normal = DVec3::new(1.0, 1.0, 1.0).normalize();
        planes.push((normal, 3.0_f64.sqrt() - 1e-4));
        assert_eq!(facet_area(&planes, 6, &reference), None);
        let (nicked, _) = mesh_area(&planes, 6);
        assert!(nicked > 0.0 && nicked < 1e-6, "{nicked}");
    }

    #[test]
    fn a_thin_slab_is_left_to_the_mesh() {
        let reference = reference_of(&cube());
        let mut planes = cube();
        // The box squeezed to a plate 2e-5 thick.
        planes[2] = (DVec3::Y, -1.0 + 1e-5);
        planes[3] = (DVec3::NEG_Y, 1.0 + 1e-5);
        assert_eq!(facet_area(&planes, 0, &reference), None);
    }

    #[test]
    fn a_solid_that_does_not_close_has_no_reference() {
        let mut planes = cube();
        planes.pop();
        assert!(!matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)));
        assert!(mesh_area(&planes, 0).1.is_none());
    }

    #[test]
    fn non_finite_planes_are_left_to_the_mesh() {
        let reference = reference_of(&cube());
        let mut planes = cube();
        planes.push((DVec3::new(f64::NAN, 0.0, 1.0), 0.5));
        assert_eq!(facet_area(&planes, 0, &reference), None);
        assert_eq!(facet_area(&cube(), 9, &reference), None);
    }

    #[test]
    fn clipping_keeps_a_polygon_that_is_already_inside() {
        let mut polygon = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(0.0, 1.0),
        ];
        let mut scratch = Vec::new();
        clip(&mut polygon, &mut scratch, 1.0, 0.0, 5.0);
        assert!((polygon_area(&polygon) - 1.0).abs() < 1e-15);
        // Half the square.
        clip(&mut polygon, &mut scratch, 1.0, 0.0, 0.5);
        assert!((polygon_area(&polygon) - 0.5).abs() < 1e-15);
        // All of it.
        clip(&mut polygon, &mut scratch, 1.0, 0.0, -1.0);
        assert!(polygon.is_empty());
    }
}
