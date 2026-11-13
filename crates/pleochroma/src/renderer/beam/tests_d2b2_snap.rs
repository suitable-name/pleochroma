//! Boundary snapping.

use glam::DVec2;

use super::{
    beam::{
        BoundaryKind, BranchPrefix, Classifier, Partition, PartitionPiece, SeamStore,
        SpectralInterval, SpectralNode,
    },
    geom::{Tolerances, homography::inverse, polygon::ConvexPolygon},
    primary::{TileRect, image_polygon, primary_beams},
    retrace::{RetraceKit, exact_ray_at},
    snap::{
        FacetClassifier, JunctionProblem, SnapFailure, SnapRequest, snap_boundary, snap_junction,
        snap_partition_with, split_snapped,
    },
    stats::BeamStats,
    stone::BeamStone,
    tests_support::{BeamRig, glass},
};
use crate::optics::raytracer::Camera;

const TOL: Tolerances = Tolerances::DEFAULT;
const TILE: TileRect = TileRect {
    x0: 0,
    y0: 0,
    x1: 64,
    y1: 64,
};
const N_LAMBDA: u32 = 48;

/// Class 0 below the parabola `y = 5 + 0.1 (x - 5)^2`, class 1 above.
struct Parabola;

impl Parabola {
    fn curve(x: f64) -> f64 {
        0.1f64.mul_add((x - 5.0) * (x - 5.0), 5.0)
    }
}

impl Classifier for Parabola {
    fn classify(&self, p: DVec2) -> Option<u32> {
        Some(u32::from(p.y >= Self::curve(p.x)))
    }
}

/// Class 0 below the line `y = 4 + 0.05 x`, class 1 above.
struct Slanted;

impl Classifier for Slanted {
    fn classify(&self, p: DVec2) -> Option<u32> {
        Some(u32::from(p.y >= 0.05f64.mul_add(p.x, 4.0)))
    }
}

/// Always class 0: no bracket exists.
struct Constant;

impl Classifier for Constant {
    fn classify(&self, _p: DVec2) -> Option<u32> {
        Some(0)
    }
}

fn square(x0: f64, x1: f64) -> ConvexPolygon {
    ConvexPolygon::from_image_points(&[
        DVec2::new(x0, 0.0),
        DVec2::new(x1, 0.0),
        DVec2::new(x1, 10.0),
        DVec2::new(x0, 10.0),
    ])
    .expect("a square")
}

fn request<'c>(
    parent: &ConvexPolygon,
    model: Vec<DVec2>,
    classifier: &'c dyn Classifier,
) -> SnapRequest<'c> {
    SnapRequest {
        parent: *parent,
        model,
        classifier,
        sides: (1, 0),
        boundary: BoundaryKind::Facet { a: 0, b: 1 },
        tile: TILE,
        lambda_nm: 550.0,
        prefix: BranchPrefix::EMPTY,
        cancel: None,
    }
}

/// Two sibling requests share the crossing on their common edge only when their branch prefixes
/// agree.
#[test]
fn siblings_with_different_prefixes_do_not_share_a_crossing() {
    let keys_after = |right_prefix: BranchPrefix| {
        let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
        let left = request(
            &square(0.0, 10.0),
            vec![DVec2::new(0.0, 3.0), DVec2::new(10.0, 5.0)],
            &Slanted,
        );
        let mut right = request(
            &square(10.0, 20.0),
            vec![DVec2::new(10.0, 6.0), DVec2::new(20.0, 5.0)],
            &Slanted,
        );
        right.prefix = right_prefix;
        snap_boundary(&left, &TOL, &mut seams, &mut stats).expect("left");
        snap_boundary(&right, &TOL, &mut seams, &mut stats).expect("right");
        seams.len()
    };
    let other = BranchPrefix::EMPTY.push(3 << 1).expect("a prefix");
    assert!(
        keys_after(other) > keys_after(BranchPrefix::EMPTY),
        "a different prefix must file its own crossing"
    );
}

/// A set cancel token stops the snap at once, as a failure rather than a hang.
#[test]
fn a_cancelled_snap_fails() {
    let flag = std::sync::atomic::AtomicBool::new(true);
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let mut req = request(
        &square(0.0, 10.0),
        vec![DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0)],
        &Parabola,
    );
    req.cancel = Some(&flag);
    assert_eq!(
        snap_boundary(&req, &TOL, &mut seams, &mut stats),
        Err(SnapFailure)
    );
}

/// Class 1 above a curve that crosses the entry and exit edges at `y = 5` but bulges to
/// `y = 17.5` at `x = 5`, beyond the unit square's top at `y = 10`.
struct Bulge;

impl Classifier for Bulge {
    fn classify(&self, p: DVec2) -> Option<u32> {
        let dx = p.x - 5.0;
        let curve = 0.5f64.mul_add(dx.mul_add(-dx, 25.0), 5.0);
        Some(u32::from(p.y >= curve))
    }
}

/// A crossing that exists only outside the parent is a failure, not a point outside it.
#[test]
fn a_crossing_outside_the_parent_is_a_bracket_failure() {
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let req = request(
        &square(0.0, 10.0),
        vec![
            DVec2::new(0.0, 5.0),
            DVec2::new(5.0, 5.0),
            DVec2::new(10.0, 5.0),
        ],
        &Bulge,
    );
    assert_eq!(
        snap_boundary(&req, &TOL, &mut seams, &mut stats),
        Err(SnapFailure)
    );
    assert_eq!(stats.bracket_failures, 1);
}

fn parabola_polyline(stats: &mut BeamStats, seams: &mut SeamStore) -> Vec<DVec2> {
    let req = request(
        &square(0.0, 10.0),
        vec![DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0)],
        &Parabola,
    );
    snap_boundary(&req, &TOL, seams, stats).expect("a bracket exists")
}

#[test]
fn a_parabola_is_snapped_to_the_chord_tolerance_with_exact_endpoints() {
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let line = parabola_polyline(&mut stats, &mut seams);
    assert!(line.len() > 2, "a bent curve needs inserted points");
    let (first, last) = (line[0], line[line.len() - 1]);
    assert!(
        first.x.abs() < 1e-12 && (first.y - 7.5).abs() < 1e-8,
        "{first}"
    );
    assert!(
        (last.x - 10.0).abs() < 1e-12 && (last.y - 7.5).abs() < 1e-8,
        "{last}"
    );
    for w in line.windows(2) {
        let mid = (w[0] + w[1]) * 0.5;
        let off = (mid.y - Parabola::curve(mid.x)).abs();
        assert!(off <= 2.0 * TOL.chord_px, "midpoint {mid} is {off} off");
    }
    for p in &line {
        assert!((p.y - Parabola::curve(p.x)).abs() < 1e-7, "vertex {p}");
    }
    assert!(stats.snapped_vertices >= line.len() as u64);
    assert_eq!(stats.bracket_failures, 0);
}

#[test]
fn siblings_sharing_an_edge_get_bit_identical_endpoints() {
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let left = request(
        &square(0.0, 10.0),
        vec![DVec2::new(0.0, 3.0), DVec2::new(10.0, 5.0)],
        &Slanted,
    );
    let right = request(
        &square(10.0, 20.0),
        vec![DVec2::new(10.0, 6.0), DVec2::new(20.0, 5.0)],
        &Slanted,
    );
    let a = snap_boundary(&left, &TOL, &mut seams, &mut stats).expect("left");
    let before = stats.snapped_vertices;
    let b = snap_boundary(&right, &TOL, &mut seams, &mut stats).expect("right");
    let (end, start) = (a[a.len() - 1], b[0]);
    assert_eq!(end.x.to_bits(), start.x.to_bits());
    assert_eq!(end.y.to_bits(), start.y.to_bits());
    assert!((end.y - 4.5).abs() < 1e-8, "{end}");
    // The shared crossing was not computed a second time: the right side computed its far end only
    // (plus chord values).
    assert!(stats.snapped_vertices - before < before + 4);
}

#[test]
fn split_snapped_children_tile_the_parent() {
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let line = parabola_polyline(&mut stats, &mut seams);
    let parent = square(0.0, 10.0);
    let (left, right) = split_snapped(&parent, &line, &TOL).expect("a split");
    assert!(!left.is_empty() && !right.is_empty());
    let total: f64 = left.iter().chain(&right).map(ConvexPolygon::area).sum();
    assert!((total - parent.area()).abs() < 1e-10, "{total}");
}

#[test]
fn a_constant_classifier_is_a_bracket_failure() {
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let req = request(
        &square(0.0, 10.0),
        vec![DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0)],
        &Constant,
    );
    assert_eq!(
        snap_boundary(&req, &TOL, &mut seams, &mut stats),
        Err(SnapFailure)
    );
    assert_eq!(stats.bracket_failures, 1);
}

/// A zero or subnormal chord tolerance fails at once as a bracket failure: the half-width would
/// stay zero on an interior seed and the expansion would never end.
#[test]
fn a_zero_or_subnormal_chord_tolerance_fails_instead_of_hanging() {
    for chord_px in [0.0, f64::from_bits(1)] {
        let tol = Tolerances { chord_px, ..TOL };
        let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
        // The curve crosses the entry edge at y = 7.5; the model seed (0, 5) is interior to the
        // edge and both probes land on class 0 when the half-width is zero.
        let req = request(
            &square(0.0, 10.0),
            vec![DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0)],
            &Parabola,
        );
        assert_eq!(
            snap_boundary(&req, &tol, &mut seams, &mut stats),
            Err(SnapFailure),
            "chord {chord_px:e}"
        );
        assert_eq!(stats.bracket_failures, 1);
    }
}

fn cube_rig() -> BeamRig {
    let mut rig = BeamRig::cube(glass());
    rig.camera = Camera::new(0.4, 1.0, 5.0, 40.0);
    rig
}

#[test]
fn the_junction_is_found_once_and_shared() {
    let rig = cube_rig();
    let stone = BeamStone::new(&rig.planes, false).expect("the cube");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (64, 64),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    let prefix = BranchPrefix::EMPTY;
    let truth = DVec2::new(30.25, 28.5);
    let ray = exact_ray_at(truth, 550.0, &prefix, &kit).expect("a camera ray");
    let target = ray.origin + ray.dir * 3.0;
    let start = truth + DVec2::new(0.7, -0.4);
    let problem = JunctionProblem::new(&kit, &prefix, 550.0, target, start).expect("a problem");
    let (mut stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let first = snap_junction(&problem, 7, 3, &TOL, &mut seams, &mut stats).expect("converges");
    assert!(first.distance(truth) < 1e-7, "{first} vs {truth}");
    let second = snap_junction(&problem, 7, 3, &TOL, &mut seams, &mut stats).expect("shared");
    assert_eq!(first, second);
    assert_eq!(seams.len(), 1);
    assert_eq!(stats.snapped_vertices, 1);
}

/// The distance from `p` to the segment `a b`.
fn segment_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let edge = b - a;
    let along = ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
    edge.mul_add(DVec2::splat(along), a).distance(p)
}

/// The crossing between two points of different class, by bisection.
fn bisect(c: &dyn Classifier, mut a: DVec2, mut b: DVec2) -> DVec2 {
    let ca = c.classify(a);
    for _ in 0..60 {
        let m = (a + b) * 0.5;
        if c.classify(m) == ca {
            a = m;
        } else {
            b = m;
        }
    }
    (a + b) * 0.5
}

#[test]
fn a_refracted_cube_beam_classifies_correctly_on_both_sides() {
    let rig = cube_rig();
    let stone = BeamStone::new(&rig.planes, false).expect("the cube");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (64, 64),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    let beams = primary_beams(
        &stone,
        &rig.camera,
        (64, 64),
        TILE,
        SpectralInterval::all(N_LAMBDA),
    );
    let Some(mut beam) = beams.into_iter().max_by(|a, b| {
        let area = |x: &super::beam::Beam| image_polygon(x).map_or(0.0, |p| p.area());
        area(a).total_cmp(&area(b))
    }) else {
        panic!("the cube has a front facet");
    };
    let word = (stone.original_facet(beam.facet) << 8) | (3 << 1);
    beam.prefix = BranchPrefix::EMPTY.push(word).expect("a short prefix");
    let lambda = SpectralNode(beam.spectrum.centre()).centre_nm(N_LAMBDA);
    let classifier = FacetClassifier::new(kit, beam.prefix, lambda);
    let image = image_polygon(&beam).expect("an image polygon");
    let (pts, pt_count) = image.points_2d();
    // The exact crossings on the polygon's edges.
    let mut crossings = Vec::new();
    for edge_index in 0..pt_count {
        let (a, b) = (pts[edge_index], pts[(edge_index + 1) % pt_count]);
        for step in 0..64u32 {
            let (t0, t1) = (f64::from(step) / 64.0, f64::from(step + 1) / 64.0);
            let (p0, p1) = (a.lerp(b, t0), a.lerp(b, t1));
            let (Some(c0), Some(c1)) = (classifier.classify(p0), classifier.classify(p1)) else {
                eprintln!("skipped: an edge sample has no exact ray");
                return;
            };
            if c0 != c1 {
                crossings.push((bisect(&classifier, p0, p1), (c0.min(c1), c0.max(c1))));
            }
        }
    }
    if crossings.len() != 2 || crossings[0].1 != crossings[1].1 {
        eprintln!(
            "skipped: {} edge crossings, no single boundary",
            crossings.len()
        );
        return;
    }
    let chord = [crossings[0].0, crossings[1].0];
    let (left, right) = image
        .split_by_polyline(&chord, 1e-9)
        .expect("a model split");
    let (Some(left_piece), Some(right_piece)) = (left.first(), right.first()) else {
        eprintln!("skipped: the model chord does not split the polygon");
        return;
    };
    let carrier = |p: &ConvexPolygon| p.to_plane(beam.image_to_plane, *beam.polygon.frame());
    let class_of = |p: &ConvexPolygon| classifier.classify(p.centroid().truncate());
    let (Some(cl), Some(cr)) = (class_of(left_piece), class_of(right_piece)) else {
        eprintln!("skipped: a piece centroid has no exact ray");
        return;
    };
    assert_ne!(cl, cr);
    let partition = Partition {
        pieces: vec![
            PartitionPiece {
                to: cl,
                polygon: carrier(left_piece).expect("carrier"),
            },
            PartitionPiece {
                to: cr,
                polygon: carrier(right_piece).expect("carrier"),
            },
        ],
        sliver_area: 0.0,
        conserved: true,
    };
    let (mut beam_stats, mut seams) = (BeamStats::default(), SeamStore::default());
    let pieces = snap_partition_with(&beam, &partition, &kit, TILE, &mut seams, &mut beam_stats)
        .expect("the partition snaps");
    assert_eq!(beam_stats.bracket_failures, 0);
    let back = inverse(beam.image_to_plane).expect("invertible");
    let images: Vec<(u32, ConvexPolygon)> = pieces
        .iter()
        .map(|p| (p.to, p.polygon.to_image(back).expect("image")))
        .collect();
    let total: f64 = images.iter().map(|(_, p)| p.area()).sum();
    assert!(
        (total - image.area()).abs() < 1e-9 * image.area(),
        "{total}"
    );
    assert_points_classify_as_pieces(&classifier, &pts[..pt_count], &images);
}

/// Checks 1000 deterministic points over the polygon's box against the pieces; those near a piece
/// edge are skipped.
fn assert_points_classify_as_pieces(
    classifier: &dyn Classifier,
    corners: &[DVec2],
    images: &[(u32, ConvexPolygon)],
) {
    let (lo, hi) = corners.iter().fold(
        (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN)),
        |(low, high), q| (low.min(*q), high.max(*q)),
    );
    let (mut state, mut checked) = (0x2545_F491_4F6C_DD1D_u64, 0u32);
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap_or(0)) / f64::from(1u32 << 24)
    };
    for _ in 0..1000 {
        let p = lo + (hi - lo) * DVec2::new(next(), next());
        let Some((to, poly)) = images.iter().find(|(_, poly)| poly.contains_2d(p, 0.0)) else {
            continue;
        };
        let (verts, vert_count) = poly.points_2d();
        let near = (0..vert_count).any(|edge_index| {
            segment_distance(p, verts[edge_index], verts[(edge_index + 1) % vert_count])
                < 5.0 * TOL.chord_px
        });
        if near {
            continue;
        }
        checked += 1;
        assert_eq!(classifier.classify(p), Some(*to), "point {p}");
    }
    assert!(checked > 100, "only {checked} points checked");
}
