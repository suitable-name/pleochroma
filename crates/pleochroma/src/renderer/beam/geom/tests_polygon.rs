//! Tests of `geom::polygon`.

use std::f64::consts::TAU;

use glam::{DMat3, DVec2, DVec3};

use super::{
    frame::Frame2,
    polygon::{ConvexPolygon, MAX_VERTICES},
};

/// A seeded xorshift64 generator (no dependency); shared with the other geometry tests.
pub struct Rng(u64);

impl Rng {
    pub const fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        (hi - lo).mul_add(self.unit(), lo)
    }

    pub fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.next_u64() % n as u64).unwrap()
    }

    /// A uniformly distributed unit vector (rejection sampling).
    pub fn unit_vec3(&mut self) -> DVec3 {
        loop {
            let v = DVec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let l = v.length();
            if l > 0.1 && l <= 1.0 {
                return v / l;
            }
        }
    }
}

/// Random 2D vertices of a convex polygon (on an ellipse), counter-clockwise.
pub fn random_convex_2d(rng: &mut Rng, max_n: usize) -> Vec<DVec2> {
    let n = 3 + rng.index(max_n - 2);
    let (rx, ry) = (rng.range(1.0, 4.0), rng.range(1.0, 4.0));
    let (cx, cy) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
    (0..n)
        .map(|i| {
            let a = TAU * (i as f64 + rng.range(0.1, 0.9)) / n as f64;
            DVec2::new(rx.mul_add(a.cos(), cx), ry.mul_add(a.sin(), cy))
        })
        .collect()
}

/// A random convex polygon on a random plane in 3D.
pub fn random_polygon(rng: &mut Rng, max_n: usize) -> ConvexPolygon {
    let q = random_convex_2d(rng, max_n);
    let origin = rng.unit_vec3() * 3.0;
    let frame = Frame2::from_normal(origin, rng.unit_vec3()).unwrap();
    ConvexPolygon::from_uv(&q, frame).unwrap()
}

fn bits(v: DVec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// A random cutting plane `(n, m)` through the neighbourhood of the polygon's centroid.
fn random_plane(rng: &mut Rng, poly: &ConvexPolygon) -> (DVec3, f64) {
    let n = rng.unit_vec3();
    let size = poly.area().sqrt();
    let m = rng.range(-0.4, 0.4).mul_add(size, n.dot(poly.centroid()));
    (n, m)
}

fn regular_polygon(n: usize, radius: f64) -> ConvexPolygon {
    let pts: Vec<DVec2> = (0..n)
        .map(|i| {
            let a = TAU * i as f64 / n as f64;
            DVec2::new(radius * a.cos(), radius * a.sin())
        })
        .collect();
    ConvexPolygon::from_image_points(&pts).unwrap()
}

fn unit_square() -> ConvexPolygon {
    ConvexPolygon::from_image_points(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ])
    .unwrap()
}

#[test]
fn basic_square_metrics() {
    let sq = unit_square();
    assert_eq!(sq.len(), 4);
    assert!((sq.area() - 1.0).abs() < 1e-15);
    let c = sq.centroid();
    assert!((c - DVec3::new(0.5, 0.5, 0.0)).length() < 1e-15);
    assert!(sq.contains(DVec3::new(0.5, 0.5, 0.0), 1e-12));
    assert!(sq.contains(DVec3::new(1.0, 0.5, 0.0), 1e-12));
    assert!(!sq.contains(DVec3::new(1.1, 0.5, 0.0), 1e-12));
    assert!(!sq.contains(DVec3::new(0.5, 0.5, 0.1), 1e-12));
}

#[test]
fn clockwise_input_is_reversed() {
    let cw = [
        DVec2::new(0.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(1.0, 0.0),
    ];
    let poly = ConvexPolygon::from_image_points(&cw).unwrap();
    assert!((poly.area() - 1.0).abs() < 1e-15);
    assert_eq!(ConvexPolygon::from_image_points(&cw[..2]), None);
}

#[test]
fn clip_conserves_area_and_shares_cut_vertices() {
    let mut rng = Rng::new(11);
    let mut cuts = 0;
    for _ in 0..400 {
        let poly = random_polygon(&mut rng, 9);
        let (normal, offset) = random_plane(&mut rng, &poly);
        let (inside, outside) = poly.clip(normal, offset, 1e-10).unwrap();
        let (Some(inner_piece), Some(outer_piece)) = (inside, outside) else {
            continue;
        };
        cuts += 1;
        let total = inner_piece.area() + outer_piece.area();
        assert!((total - poly.area()).abs() <= 1e-12 * poly.area());
        let cut_vertices = |piece: &ConvexPolygon| {
            let mut c: Vec<[u64; 3]> = piece
                .vertices()
                .iter()
                .filter(|v| !poly.vertices().contains(v))
                .map(|v| bits(*v))
                .collect();
            c.sort_unstable();
            c
        };
        assert_eq!(cut_vertices(&inner_piece), cut_vertices(&outer_piece));
        assert!(cut_vertices(&inner_piece).len() <= 2);
    }
    assert!(cuts > 100, "only {cuts} random planes cut the polygon");
}

#[test]
fn clip_with_the_negated_plane_swaps_the_pieces_bit_for_bit() {
    let mut rng = Rng::new(12);
    for _ in 0..200 {
        let poly = random_polygon(&mut rng, 9);
        let (n, m) = random_plane(&mut rng, &poly);
        let (a_in, a_out) = poly.clip(n, m, 1e-10).unwrap();
        let (b_in, b_out) = poly.clip(-n, -m, 1e-10).unwrap();
        // The cut vertices are identical in both orientations.
        let verts = |p: &Option<ConvexPolygon>| p.map(|p| p.vertices().to_vec());
        assert_eq!(verts(&a_in), verts(&b_out));
        assert_eq!(verts(&a_out), verts(&b_in));
    }
}

#[test]
fn clip_counts_on_vertices_as_inside() {
    let sq = unit_square();
    let x = DVec3::X;
    // Plane x = 1 touches the right edge: nothing is outside.
    assert_eq!(sq.clip(x, 1.0, 1e-9), Some((Some(sq), None)));
    // Plane x = 0 with the normal +x: the left edge is on the plane, the right edge outside.
    assert_eq!(sq.clip(x, 0.0, 1e-9), Some((None, Some(sq))));
    // A plane through a vertex only (the diagonal) splits into two triangles.
    let (a, b) = sq.clip(DVec3::new(1.0, 1.0, 0.0), 1.0, 1e-9).unwrap();
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!((a.len(), b.len()), (3, 3));
    assert!((a.area() - 0.5).abs() < 1e-15 && (b.area() - 0.5).abs() < 1e-15);
}

#[test]
fn many_random_cuts_tile_the_polygon() {
    let mut rng = Rng::new(13);
    for _ in 0..20 {
        let parent = random_polygon(&mut rng, 9);
        let mut pieces = vec![parent];
        for _ in 0..60 {
            let k = rng.index(pieces.len());
            let piece = pieces[k];
            let (n, m) = random_plane(&mut rng, &piece);
            let (inside, outside) = piece.clip_capped(n, m, 1e-10);
            if inside.is_empty() || outside.is_empty() {
                continue;
            }
            pieces.swap_remove(k);
            pieces.extend(inside);
            pieces.extend(outside);
        }
        let total: f64 = pieces.iter().map(ConvexPolygon::area).sum();
        assert!((total - parent.area()).abs() <= 1e-9 * parent.area());
        assert!(pieces.len() > 10);
        for p in &pieces {
            assert!(p.len() <= MAX_VERTICES);
            assert!(p.area() > 0.0);
        }
    }
}

#[test]
fn vertex_cap_splits_along_the_longest_diagonal() {
    let poly = regular_polygon(MAX_VERTICES, 1.0);
    assert_eq!(poly.len(), MAX_VERTICES);
    // Only the vertex at angle 0 is outside: the inside piece would have 13 vertices.
    let plane = (DVec3::X, 0.95);
    assert_eq!(poly.clip(plane.0, plane.1, 1e-12), None);
    let (inside, outside) = poly.clip_capped(plane.0, plane.1, 1e-12);
    // Which diameter is "longest" depends on rounding; the outlying vertex lies in one or both
    // halves, so there are one or two outside pieces and exactly two inside pieces.
    assert!((1..=2).contains(&outside.len()));
    assert_eq!(inside.len(), 2);
    for p in inside.iter().chain(&outside) {
        assert!(p.len() <= MAX_VERTICES);
    }
    let total: f64 = inside.iter().chain(&outside).map(ConvexPolygon::area).sum();
    assert!((total - poly.area()).abs() < 1e-12);
    // The diagonal split keeps the original vertices bit for bit.
    let (first, second) = poly.split_longest_diagonal().unwrap();
    assert!((first.area() + second.area() - poly.area()).abs() < 1e-12);
    for v in first.vertices().iter().chain(second.vertices()) {
        assert!(poly.vertices().contains(v));
    }
    assert_eq!(poly.split_diagonal(0, 1), None);
}

#[test]
fn split_longest_edge_and_quadrisect_tile() {
    let mut rng = Rng::new(14);
    for _ in 0..100 {
        let poly = random_polygon(&mut rng, 9);
        let (a, b) = poly.split_longest_edge().unwrap();
        assert!((a.area() + b.area() - poly.area()).abs() <= 1e-12 * poly.area());
        let quads = poly.quadrisect();
        assert!(quads.len() <= 4 && quads.len() >= 2);
        let total: f64 = quads.iter().map(ConvexPolygon::area).sum();
        assert!((total - poly.area()).abs() <= 1e-12 * poly.area());
    }
    let sq = unit_square();
    let (a, b) = sq.split_longest_edge().unwrap();
    assert!((a.area() - 0.5).abs() < 1e-12 && (b.area() - 0.5).abs() < 1e-12);
    assert_eq!(sq.quadrisect().len(), 4);
}

#[test]
fn wachspress_partition_of_unity_and_affine_precision() {
    let mut rng = Rng::new(15);
    for _ in 0..300 {
        let poly = random_polygon(&mut rng, 10);
        let (uv, n) = poly.points_2d();
        // A random interior point: a convex combination of the vertices.
        let mut lambda: Vec<f64> = (0..n).map(|_| rng.range(0.05, 1.0)).collect();
        let s: f64 = lambda.iter().sum();
        for l in &mut lambda {
            *l /= s;
        }
        let point = (0..n).fold(DVec2::ZERO, |acc, i| acc + uv[i] * lambda[i]);
        let w = poly.wachspress(point);
        let sum: f64 = w[..n].iter().sum();
        assert!((sum - 1.0).abs() < 1e-12, "sum {sum}");
        assert!(w[..n].iter().all(|&x| x > 0.0));
        // Linear precision: the weights reproduce the point and any affine field.
        let rebuilt = (0..n).fold(DVec2::ZERO, |acc, i| acc + uv[i] * w[i]);
        assert!((rebuilt - point).length() < 1e-11);
        let field = |q: DVec2| 2.1_f64.mul_add(-q.y, 1.3_f64.mul_add(q.x, 0.7));
        let interpolated: f64 = (0..n).map(|i| w[i] * field(uv[i])).sum();
        assert!((interpolated - field(point)).abs() < 1e-11);
    }
}

#[test]
fn wachspress_at_vertices_and_edges_is_linear() {
    let sq = unit_square();
    let w = sq.wachspress(DVec2::new(1.0, 0.0));
    assert!((w[1] - 1.0).abs() < 1e-15 && w[0].abs() < 1e-15 && w[2].abs() < 1e-15);
    let w = sq.wachspress(DVec2::new(0.25, 0.0));
    assert!((w[0] - 0.75).abs() < 1e-15 && (w[1] - 0.25).abs() < 1e-15);
    assert!(w[2].abs() < 1e-15 && w[3].abs() < 1e-15);
    let w = sq.wachspress(DVec2::new(0.5, 0.5));
    assert!(w[..4].iter().all(|x| (x - 0.25).abs() < 1e-15));
}

#[test]
fn remove_degeneracies_cleans_duplicates_collinear_points_and_slivers() {
    let messy = ConvexPolygon::from_image_points(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(0.5, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1e-13),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ])
    .unwrap();
    let clean = messy.remove_degeneracies(1e-9, 1e-12).unwrap();
    assert_eq!(clean.len(), 4);
    assert!((clean.area() - 1.0).abs() < 1e-12);
    // A sliver below the area threshold disappears.
    let sliver = ConvexPolygon::from_image_points(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.5, 1e-9),
    ])
    .unwrap();
    assert_eq!(sliver.remove_degeneracies(1e-12, 1e-6), None);
    // A clean polygon is unchanged.
    let sq = unit_square();
    assert_eq!(sq.remove_degeneracies(1e-9, 1e-12), Some(sq));
}

#[test]
fn snap_to_moves_close_vertices_only() {
    let mut sq = unit_square();
    let target = DVec3::new(1.0 + 1e-11, 1e-11, 0.0);
    let far = DVec3::new(5.0, 5.0, 0.0);
    assert_eq!(sq.snap_to(&[far, target], 1e-9), 1);
    assert_eq!(sq.vertices()[1], target);
    assert_eq!(sq.vertices()[0], DVec3::ZERO);
    assert_eq!(sq.snap_to(&[far, target], 1e-9), 0);
}

#[test]
fn image_conversions_round_trip() {
    let mut rng = Rng::new(16);
    // A projective map that keeps the unit-ish region in front of the centre (w > 0).
    let homography = DMat3::from_cols_array(&[1.2, 0.1, 0.001, -0.2, 0.9, 0.002, 3.0, -1.0, 1.0]);
    for _ in 0..100 {
        let quad_uv: Vec<DVec2> = random_convex_2d(&mut rng, 8)
            .into_iter()
            .map(|p| p * 0.25 + DVec2::splat(0.5))
            .collect();
        let frame = Frame2::from_normal(rng.unit_vec3(), rng.unit_vec3()).unwrap();
        let carrier = ConvexPolygon::from_uv(&quad_uv, frame).unwrap();
        let image = carrier.to_image(homography).unwrap();
        assert_eq!(image.len(), carrier.len());
        let back = image.to_plane(homography.inverse(), frame).unwrap();
        for (p0, p1) in carrier.vertices().iter().zip(back.vertices()) {
            assert!((*p0 - *p1).length() < 1e-10);
        }
        // A projective image of a convex polygon in front of the centre stays convex.
        let (uv, count) = image.points_2d();
        for corner in 0..count {
            let (p0, p1, p2) = (
                uv[corner],
                uv[(corner + 1) % count],
                uv[(corner + 2) % count],
            );
            assert!((p1 - p0).perp_dot(p2 - p1) > 0.0);
        }
    }
    // A vertex at the horizon (w = 0) has no image.
    let sq = unit_square();
    let horizon = DMat3::from_cols_array(&[1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    assert_eq!(sq.to_image(horizon), None);
}
