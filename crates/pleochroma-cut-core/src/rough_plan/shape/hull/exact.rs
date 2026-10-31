//! The convex hull with exact visibility, for point sets the float hull cannot build.
//!
//! [`hull_faces`](super::hull_faces) decides "this face sees the point" with a float plane test
//! and a tolerance. On a design outline with clusters of nearly coincident, nearly coplanar
//! corners (two corners 1e-5 apart on a facet edge, say) those decisions contradict each other:
//! a point sees two faces that share an edge in the same direction, the horizon is no longer a
//! loop, and every later point that sees the duplicated faces duplicates them again. The face
//! list then doubles step after step (hundreds of thousands of faces from a few hundred points)
//! and the call never returns. A planner lane stuck in it froze the whole plan.
//!
//! Here a face sees a point when the exact orientation of the face's three corners and the
//! point is positive ([`orient3d`]: a float determinant with an error bound, and exact rational
//! arithmetic when the float cannot tell). Exact signs cannot contradict each other, so the hull
//! stays a closed, convex surface; a point exactly on a face is inside. The incremental
//! insertion is the same as `hull_faces`', so the faces are the same where both are right.

use glam::DVec3;
use num_bigint::{BigInt, Sign};
use num_rational::Ratio;

use super::{Face, first_tetrahedron};

/// The sign of the orientation of `d` against the triangle `a`, `b`, `c`: positive when `d` lies
/// on the side the triangle's winding normal `(b - a) x (c - a)` points to, zero when the four
/// points are coplanar. Exact for every finite input.
#[expect(
    clippy::many_single_char_names,
    reason = "the textbook names of the orientation predicate: corners a-d, edge vectors u-w"
)]
pub(super) fn orient3d(a: DVec3, b: DVec3, c: DVec3, d: DVec3) -> Sign {
    let (u, v, w) = (b - a, c - a, d - a);
    let det = u.cross(v).dot(w);
    // Each difference is rounded once (relative error <= 2^-53) and the determinant adds a
    // handful of roundings more, so its error stays far below this bound: a float result beyond
    // it has the exact sign.
    let size = |x: DVec3| x.abs().element_sum();
    let bound = 1e-12 * size(u) * size(v) * size(w);
    if det > bound {
        return Sign::Plus;
    }
    if det < -bound {
        return Sign::Minus;
    }
    exact_orient3d(a, b, c, d)
}

/// [`orient3d`] in exact rational arithmetic (every finite `f64` is a rational number).
#[expect(
    clippy::many_single_char_names,
    reason = "the same corner and edge names as `orient3d`"
)]
fn exact_orient3d(a: DVec3, b: DVec3, c: DVec3, d: DVec3) -> Sign {
    let exact = |p: DVec3| {
        p.to_array().map(|x| {
            Ratio::<BigInt>::from_float(x).expect("orient3d is only called for finite points")
        })
    };
    let [a, b, c, d] = [exact(a), exact(b), exact(c), exact(d)];
    let minus = |p: &[Ratio<BigInt>; 3]| -> [Ratio<BigInt>; 3] {
        [&p[0] - &a[0], &p[1] - &a[1], &p[2] - &a[2]]
    };
    let (u, v, w) = (minus(&b), minus(&c), minus(&d));
    let cross = [
        &u[1] * &v[2] - &u[2] * &v[1],
        &u[2] * &v[0] - &u[0] * &v[2],
        &u[0] * &v[1] - &u[1] * &v[0],
    ];
    let det = &cross[0] * &w[0] + &cross[1] * &w[1] + &cross[2] * &w[2];
    det.numer().sign()
}

/// The triangles of the convex hull of `pts`, outward wound, with exact visibility; `None` when
/// `pts` has no volume (the same seed test as `hull_faces`). Points are added in index order, so
/// the result is deterministic. Every point is on or inside every face.
pub(super) fn exact_hull_faces(pts: &[DVec3], eps: f64) -> Option<Vec<Face>> {
    let (seed, mut faces) = first_tetrahedron(pts, eps)?;
    for (index, &p) in pts.iter().enumerate() {
        if seed.contains(&index) {
            continue;
        }
        let faces_point =
            |f: &Face| orient3d(pts[f.v[0]], pts[f.v[1]], pts[f.v[2]], p) == Sign::Plus;
        if !faces.iter().any(faces_point) {
            continue;
        }
        let (lit, rest): (Vec<Face>, Vec<Face>) = std::mem::take(&mut faces)
            .into_iter()
            .partition(faces_point);
        let lit_edges: std::collections::BTreeSet<(usize, usize)> =
            lit.iter().flat_map(Face::edges).collect();
        faces = rest;
        faces.extend(
            lit.iter()
                .flat_map(Face::edges)
                .filter(|&(a, b)| !lit_edges.contains(&(b, a)))
                .map(|(a, b)| Face::new(pts, a, b, index)),
        );
    }
    Some(faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The outline corners of a library design whose float hull doubled its faces without end
    /// (the second plan of a rough with a locked stone hung in it).
    const STUCK_OUTLINE: &str = include_str!("testdata/stuck_design_outline.txt");

    fn stuck_points() -> Vec<DVec3> {
        STUCK_OUTLINE
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let xyz: Vec<f64> = line
                    .split_whitespace()
                    .map(|word| word.parse().expect("a number"))
                    .collect();
                DVec3::new(xyz[0], xyz[1], xyz[2])
            })
            .collect()
    }

    #[test]
    fn orientation_is_exact_where_the_float_cannot_tell() {
        let (a, b, c) = (DVec3::ZERO, DVec3::X, DVec3::Y);
        assert_eq!(orient3d(a, b, c, DVec3::Z), Sign::Plus);
        assert_eq!(orient3d(a, b, c, -DVec3::Z), Sign::Minus);
        assert_eq!(orient3d(a, b, c, DVec3::new(0.3, 0.4, 0.0)), Sign::NoSign);
        // The smallest normal number above the plane: far inside the float bound, decided
        // exactly.
        let tiny = DVec3::new(0.5, 0.5, f64::MIN_POSITIVE);
        assert_eq!(orient3d(a, b, c, tiny), Sign::Plus);
        assert_eq!(orient3d(a, c, b, tiny), Sign::Minus);
    }

    #[test]
    fn the_float_hull_hands_a_stuck_outline_to_the_exact_hull_instead_of_hanging() {
        let pts = stuck_points();
        let lo = pts.iter().fold(DVec3::INFINITY, |m, &p| m.min(p));
        let hi = pts.iter().fold(DVec3::NEG_INFINITY, |m, &p| m.max(p));
        let eps = 1e-9 * (hi - lo).max_element();
        let faces = super::super::hull_faces(&pts, eps).expect("the outline has volume");
        assert_eq!(
            faces.iter().map(|f| f.v).collect::<Vec<_>>(),
            exact_hull_faces(&pts, eps)
                .expect("the outline has volume")
                .iter()
                .map(|f| f.v)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_exact_hull_of_a_stuck_outline_is_closed_and_holds_every_point() {
        let pts = stuck_points();
        assert!(pts.len() > 300, "the fixture has {} points", pts.len());
        let faces = exact_hull_faces(&pts, 1e-9).expect("the outline has volume");
        // A triangulated convex hull of n points has at most 2n - 4 faces.
        assert!(faces.len() <= 2 * pts.len() - 4, "{} faces", faces.len());
        // Closed: every directed edge has its reverse exactly once.
        let edges: Vec<(usize, usize)> = faces.iter().flat_map(Face::edges).collect();
        for &(a, b) in &edges {
            assert_eq!(
                edges.iter().filter(|&&e| e == (b, a)).count(),
                1,
                "edge {a}-{b}"
            );
        }
        // Convex: no point is strictly outside any face.
        for face in &faces {
            for &p in &pts {
                assert_ne!(
                    orient3d(pts[face.v[0]], pts[face.v[1]], pts[face.v[2]], p),
                    Sign::Plus
                );
            }
        }
    }
}
