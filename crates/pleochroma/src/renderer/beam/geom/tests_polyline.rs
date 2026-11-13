//! Tests of `geom::polyline` and `ConvexPolygon::split_by_polyline`.

use glam::{DVec2, DVec3};

use super::{
    frame::Frame2,
    polygon::{ConvexPolygon, MAX_VERTICES},
    polyline::{
        EdgeHit, RingPoint, decompose_convex, dedupe_ring, nearest_edge, ring_area, split_rings,
    },
    tests_polygon::{Rng, random_convex_2d},
};

fn shoelace(points: &[DVec2]) -> f64 {
    let n = points.len();
    0.5 * (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f64>()
}

fn ring(points: &[DVec2]) -> Vec<RingPoint> {
    points
        .iter()
        .map(|&q| RingPoint {
            q,
            p: q.extend(0.0),
        })
        .collect()
}

fn square(side: f64) -> ConvexPolygon {
    ConvexPolygon::from_image_points(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(side, 0.0),
        DVec2::new(side, side),
        DVec2::new(0.0, side),
    ])
    .unwrap()
}

/// Asserts every piece is convex, counter-clockwise and within the cap; returns the total area.
fn checked_area(pieces: &[ConvexPolygon]) -> f64 {
    let mut total = 0.0;
    for piece in pieces {
        assert!(piece.len() <= MAX_VERTICES);
        let (uv, n) = piece.points_2d();
        for i in 0..n {
            let (a, b, c) = (uv[i], uv[(i + 1) % n], uv[(i + 2) % n]);
            assert!((b - a).perp_dot(c - b) >= -1e-9, "reflex vertex in a piece");
        }
        assert!(piece.area() > 0.0);
        total += piece.area();
    }
    total
}

#[test]
fn nearest_edge_finds_the_edge_and_parameter() {
    let poly = [
        DVec2::new(0.0, 0.0),
        DVec2::new(4.0, 0.0),
        DVec2::new(4.0, 4.0),
        DVec2::new(0.0, 4.0),
    ];
    let hit = nearest_edge(&poly, DVec2::new(1.0, 0.0)).unwrap();
    assert_eq!((hit.edge, hit.t, hit.dist), (0, 0.25, 0.0));
    let hit = nearest_edge(&poly, DVec2::new(4.5, 2.0)).unwrap();
    assert_eq!((hit.edge, hit.t, hit.dist), (1, 0.5, 0.5));
    assert_eq!(nearest_edge(&poly[..2], DVec2::ZERO), None);
}

#[test]
fn ring_helpers_dedupe_and_measure() {
    let mut r = ring(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(0.0, 2.0),
        DVec2::new(0.0, 1e-12),
    ]);
    dedupe_ring(&mut r, 1e-9);
    assert_eq!(r.len(), 4);
    assert!((ring_area(&r) - 4.0).abs() < 1e-12);
}

#[test]
fn split_rings_walks_both_arcs() {
    let boundary = ring(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ]);
    let chord = ring(&[DVec2::new(5.0, 0.0), DVec2::new(5.0, 10.0)]);
    let entry = EdgeHit {
        edge: 0,
        t: 0.5,
        dist: 0.0,
    };
    let exit = EdgeHit {
        edge: 2,
        t: 0.5,
        dist: 0.0,
    };
    let (left, right) = split_rings(&boundary, &chord, entry, exit);
    assert!((ring_area(&left) - 50.0).abs() < 1e-12);
    assert!((ring_area(&right) - 50.0).abs() < 1e-12);
    assert_eq!((left.len(), right.len()), (4, 4));
}

#[test]
fn decompose_convex_tiles_an_l_shape() {
    let l_shape = ring(&[
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(1.0, 2.0),
        DVec2::new(0.0, 2.0),
    ]);
    let pieces = decompose_convex(l_shape.clone(), MAX_VERTICES);
    assert!(pieces.len() >= 2);
    let total: f64 = pieces.iter().map(|p| ring_area(p)).sum();
    assert!((total - 3.0).abs() < 1e-12);
    for piece in &pieces {
        // Convex, counter-clockwise, and made of the ring's own vertices.
        let n = piece.len();
        for i in 0..n {
            let (a, b, c) = (piece[i].q, piece[(i + 1) % n].q, piece[(i + 2) % n].q);
            assert!((b - a).perp_dot(c - b) >= -1e-12);
            assert!(l_shape.iter().any(|v| v.q == piece[i].q));
        }
    }
}

#[test]
fn decompose_convex_enforces_the_vertex_cap() {
    let n = 20;
    let pts: Vec<DVec2> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            DVec2::new(a.cos(), a.sin())
        })
        .collect();
    let pieces = decompose_convex(ring(&pts), MAX_VERTICES);
    assert!(pieces.len() >= 2);
    assert!(pieces.iter().all(|p| p.len() <= MAX_VERTICES));
    let total: f64 = pieces.iter().map(|p| ring_area(p)).sum();
    assert!((total - shoelace(&pts)).abs() < 1e-12);
}

#[test]
fn s_shaped_polyline_tiles_the_parent() {
    let parent = square(10.0);
    let chords = [
        DVec2::new(3.0, 0.0),
        DVec2::new(7.0, 2.5),
        DVec2::new(3.0, 5.0),
        DVec2::new(7.0, 7.5),
        DVec2::new(6.0, 10.0),
    ];
    let (left, right) = parent.split_by_polyline(&chords, 1e-9).unwrap();
    // Both sides are non-convex rings, so each is decomposed into several convex pieces.
    assert!(left.len() >= 2 && right.len() >= 2);
    let (left_area, right_area) = (checked_area(&left), checked_area(&right));
    // The expected areas come from the two rings, summed independently of the kernel.
    let mut west: Vec<DVec2> = chords.to_vec();
    west.extend([DVec2::new(0.0, 10.0), DVec2::new(0.0, 0.0)]);
    assert!((left_area - shoelace(&west)).abs() < 1e-9);
    assert!((left_area + right_area - 100.0).abs() < 1e-9);
    // The polyline's vertices appear in both sides bit for bit.
    for c in &chords {
        let p = c.extend(0.0);
        assert!(left.iter().any(|piece| piece.vertices().contains(&p)));
        assert!(right.iter().any(|piece| piece.vertices().contains(&p)));
    }
    // Reversing the polyline swaps the sides.
    let reversed: Vec<DVec2> = chords.iter().rev().copied().collect();
    let (l2, r2) = parent.split_by_polyline(&reversed, 1e-9).unwrap();
    assert!((checked_area(&l2) - right_area).abs() < 1e-9);
    assert!((checked_area(&r2) - left_area).abs() < 1e-9);
}

#[test]
fn straight_chords_give_one_convex_piece_per_side() {
    let parent = square(10.0);
    // Corner to corner: two triangles (the end points are polygon vertices).
    let (l, r) = parent
        .split_by_polyline(&[DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)], 1e-9)
        .unwrap();
    assert_eq!((l.len(), r.len()), (1, 1));
    assert!((l[0].area() - 50.0).abs() < 1e-12 && (r[0].area() - 50.0).abs() < 1e-12);
    assert_eq!((l[0].len(), r[0].len()), (3, 3));
    // End points away from the boundary are refused, as are one-point polylines.
    assert_eq!(
        parent.split_by_polyline(&[DVec2::new(5.0, 5.0), DVec2::new(5.0, 10.0)], 1e-9),
        None
    );
    assert_eq!(
        parent.split_by_polyline(&[DVec2::new(5.0, 0.0)], 1e-9),
        None
    );
}

#[test]
fn a_chord_on_a_single_edge_wraps_the_whole_polygon() {
    // Both end points on the bottom edge: a bulge cuts a cap off; the rest wraps around.
    let parent = square(10.0);
    let chords = [
        DVec2::new(2.0, 0.0),
        DVec2::new(3.0, 2.0),
        DVec2::new(5.0, 3.0),
        DVec2::new(7.0, 2.0),
        DVec2::new(8.0, 0.0),
    ];
    let (l, r) = parent.split_by_polyline(&chords, 1e-9).unwrap();
    let (la, ra) = (checked_area(&l), checked_area(&r));
    assert!((la + ra - 100.0).abs() < 1e-9);
    // The cap under the bulge (about 12 square units) is the right side of the travel direction;
    // the left side is the rest of the square, wrapped around the whole boundary.
    assert!(la.min(ra) < 20.0);
}

#[test]
fn random_straight_cuts_tile_random_polygons() {
    let mut rng = Rng::new(41);
    for _ in 0..200 {
        let pts = random_convex_2d(&mut rng, 9);
        let n = pts.len();
        let poly = ConvexPolygon::from_image_points(&pts).unwrap();
        let e0 = rng.index(n);
        let e1 = (e0 + 1 + rng.index(n - 1)) % n;
        let on_edge = |e: usize, rng: &mut Rng| {
            let t = rng.range(0.1, 0.9);
            pts[e].lerp(pts[(e + 1) % n], t)
        };
        let chord = [on_edge(e0, &mut rng), on_edge(e1, &mut rng)];
        let (l, r) = poly.split_by_polyline(&chord, 1e-9).unwrap();
        assert_eq!((l.len(), r.len()), (1, 1));
        let total = checked_area(&l) + checked_area(&r);
        assert!((total - poly.area()).abs() <= 1e-9 * poly.area());
    }
}

#[test]
fn polylines_split_polygons_on_tilted_planes() {
    let frame = Frame2::from_normal(DVec3::new(1.0, 2.0, 3.0), DVec3::new(0.3, -0.5, 0.8)).unwrap();
    let q = [
        DVec2::new(-3.0, -2.0),
        DVec2::new(3.0, -2.0),
        DVec2::new(3.0, 2.0),
        DVec2::new(-3.0, 2.0),
    ];
    let parent = ConvexPolygon::from_uv(&q, frame).unwrap();
    let chords = [
        DVec2::new(-1.0, -2.0),
        DVec2::new(1.0, -0.5),
        DVec2::new(-1.0, 0.5),
        DVec2::new(1.0, 2.0),
    ];
    let (l, r) = parent.split_by_polyline(&chords, 1e-9).unwrap();
    let total = checked_area(&l) + checked_area(&r);
    assert!((total - 24.0).abs() < 1e-9);
    for piece in l.iter().chain(&r) {
        assert_eq!(piece.frame(), &frame);
        for v in piece.vertices() {
            assert!(frame.signed_distance(*v).abs() < 1e-12);
        }
    }
}
