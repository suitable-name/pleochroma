//! Tests of `BeamStone`.

use std::f32::consts::{PI, TAU};

use glam::{DVec3, Vec3};

use super::{BeamOff, stone::BeamStone};
use crate::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    stone_metrics::{SolidStatus, build_solid_mesh},
};

/// The facet count of a stone as a facet id bound.
fn facet_ids(stone: &BeamStone) -> u32 {
    u32::try_from(stone.facet_count()).expect("facet count fits in u32")
}

/// The six faces of the cube `[-1, 1]^3`.
fn cube_planes() -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .iter()
    .map(|&normal| GpuFacetPlane::new(normal, -1.0))
    .collect()
}

/// A five-fold stone with a table, five crown facets and five pavilion facets at the girdle
/// corners, and five vertical girdle facets: a Pentalume-like plane set.
fn pentagonal_planes() -> Vec<GpuFacetPlane> {
    let crown = 35.0_f32.to_radians();
    let pavilion = 40.0_f32.to_radians();
    let mut planes = vec![GpuFacetPlane::new(Vec3::Y, -0.5)];
    for k in 0..5_u8 {
        let wall = TAU * f32::from(k) / 5.0;
        // Half a sector (36 degrees) past the wall: std has no `FRAC_PI_5`.
        let corner = wall + PI / 5.0;
        planes.push(GpuFacetPlane::new(
            Vec3::new(
                crown.cos() * corner.cos(),
                crown.sin(),
                crown.cos() * corner.sin(),
            ),
            -1.0,
        ));
        planes.push(GpuFacetPlane::new(
            Vec3::new(wall.cos(), 0.0, wall.sin()),
            -0.9,
        ));
        planes.push(GpuFacetPlane::new(
            Vec3::new(
                pavilion.cos() * corner.cos(),
                -pavilion.sin(),
                pavilion.cos() * corner.sin(),
            ),
            -1.0,
        ));
    }
    planes
}

/// A square pyramid whose four side planes almost share the apex: one offset is off by a few
/// `f32` ulps, so the apex splits into several vertices a fraction of a micro-unit apart.
fn near_degenerate_pyramid() -> Vec<GpuFacetPlane> {
    let slope = std::f32::consts::FRAC_PI_4;
    let (s, c) = slope.sin_cos();
    vec![
        GpuFacetPlane::new(Vec3::new(s, c, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, c, s), -1.000_001),
        GpuFacetPlane::new(Vec3::new(-s, c, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, c, -s), -1.0),
        GpuFacetPlane::new(Vec3::NEG_Y, -0.5),
    ]
}

/// Asserts the structural properties of a closed convex stone.
fn assert_closed_convex(stone: &BeamStone) {
    let mut half_edges: Vec<(u32, u32)> = Vec::new();
    let mut used = vec![false; stone.vertex_count()];
    for facet in 0..facet_ids(stone) {
        let ring = stone.ring(facet);
        assert!(ring.len() >= 3, "facet {facet} has {} vertices", ring.len());
        let mut sorted = ring.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ring.len(),
            "facet {facet} repeats a vertex id"
        );
        let (normal, offset) = stone.plane(facet);
        for (at, &a) in ring.iter().enumerate() {
            used[a as usize] = true;
            assert!(
                (normal.dot(stone.vertex(a)) - offset).abs() < 1e-4,
                "vertex {a} is off the plane of facet {facet}"
            );
            let b = ring[(at + 1) % ring.len()];
            half_edges.push((a.min(b), a.max(b)));
            assert_eq!(stone.edge_vertices(facet, at), [a.min(b), a.max(b)]);
            let across = stone
                .neighbour(facet, at)
                .expect("a closed stone has no open edge");
            let back = (0..stone.ring(across).len())
                .find(|&j| stone.edge_vertices(across, j) == stone.edge_vertices(facet, at))
                .expect("the neighbour holds the same edge");
            assert_eq!(stone.neighbour(across, back), Some(facet));
        }
        assert_counter_clockwise(stone, facet);
    }
    assert!(used.iter().all(|&u| u), "a vertex belongs to no ring");
    half_edges.sort_unstable();
    for group in half_edges.chunk_by(|x, y| x == y) {
        assert_eq!(
            group.len(),
            2,
            "edge {:?} is not in exactly two rings",
            group[0]
        );
    }
    let mut unique = half_edges.clone();
    unique.dedup();
    let listed: Vec<(u32, u32)> = stone.edges().iter().copied().map(Into::into).collect();
    assert_eq!(listed, unique, "the edge list is each edge once, ascending");
    assert_eq!(
        stone.vertex_count() + stone.facet_count(),
        stone.edges().len() + 2,
        "Euler's formula"
    );
}

/// Asserts the ring of `facet` winds counter-clockwise seen from outside.
fn assert_counter_clockwise(stone: &BeamStone, facet: u32) {
    let ring = stone.ring(facet);
    let origin = stone.vertex(ring[0]);
    let mut area = DVec3::ZERO;
    for pair in ring[1..].windows(2) {
        area += (stone.vertex(pair[0]) - origin).cross(stone.vertex(pair[1]) - origin);
    }
    assert!(
        area.dot(stone.plane(facet).0) > 0.0,
        "facet {facet} is not counter-clockwise from outside"
    );
}

#[test]
fn the_cube_has_eight_vertices_twelve_edges_and_six_facets() {
    let stone = BeamStone::new(&cube_planes(), false).expect("a cube is a stone");
    assert_eq!(stone.facet_count(), 6);
    assert_eq!(stone.vertex_count(), 8);
    assert_eq!(stone.edges().len(), 12);
    assert_closed_convex(&stone);
    assert!((stone.bounding_radius() - 3.0_f64.sqrt()).abs() < 1e-9);
    for facet in 0..6 {
        assert_eq!(stone.ring(facet).len(), 4);
        assert_eq!(stone.aliases(facet), [facet]);
        assert_eq!(stone.original_facet(facet), facet);
        assert_eq!(stone.facet_of_original(facet), Some(facet));
    }
}

#[test]
fn the_brilliant_is_closed_and_convex() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let stone = BeamStone::new(&planes, false).expect("the brilliant is a stone");
    assert_closed_convex(&stone);
    assert!(stone.facet_count() > 10);
    assert!(stone.bounding_radius() > 0.0);
}

#[test]
fn the_pentagonal_stone_is_closed_and_convex() {
    let planes = pentagonal_planes();
    let stone = BeamStone::new(&planes, false).expect("the pentagonal stone is a stone");
    assert_closed_convex(&stone);
    eprintln!(
        "pentagonal stone: {} planes, {} facets, {} vertices, {} edges",
        planes.len(),
        stone.facet_count(),
        stone.vertex_count(),
        stone.edges().len()
    );
}

#[test]
fn coplanar_duplicates_become_aliases_of_the_lowest_index() {
    let mut planes = cube_planes();
    planes.push(planes[0]);
    planes.push(planes[3]);
    let stone = BeamStone::new(&planes, false).expect("duplicates do not change the stone");
    assert_eq!(stone.facet_count(), 6);
    assert_closed_convex(&stone);
    assert_eq!(stone.aliases(0), [0, 6]);
    assert_eq!(stone.aliases(3), [3, 7]);
    assert_eq!(stone.original_facet(0), 0);
    assert_eq!(stone.original_facet(3), 3);
    assert_eq!(stone.facet_of_original(6), Some(0));
    assert_eq!(stone.facet_of_original(7), Some(3));
    assert_eq!(stone.facet_of_original(8), None);
}

#[test]
fn every_brilliant_plane_maps_to_a_facet_and_aliases_start_at_the_lowest_index() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let stone = BeamStone::new(&planes, false).expect("the brilliant is a stone");
    let mut mapped = 0;
    for facet in 0..facet_ids(&stone) {
        let aliases = stone.aliases(facet);
        assert!(aliases.windows(2).all(|w| w[0] < w[1]), "aliases ascend");
        assert_eq!(stone.original_facet(facet), aliases[0]);
        for &original in aliases {
            assert_eq!(stone.facet_of_original(original), Some(facet));
        }
        mapped += aliases.len();
    }
    let unmapped = (0..u32::try_from(planes.len()).expect("fits"))
        .filter(|&i| stone.facet_of_original(i).is_none())
        .count();
    assert_eq!(mapped + unmapped, planes.len());
}

#[test]
fn a_near_degenerate_meet_is_cleaned_once_and_stays_closed() {
    let stone = BeamStone::new(&near_degenerate_pyramid(), false).expect("a pyramid is a stone");
    assert_eq!(stone.facet_count(), 5);
    assert_eq!(stone.vertex_count(), 5, "the apex cluster is one vertex");
    assert_eq!(stone.edges().len(), 8);
    assert_closed_convex(&stone);
}

/// Index of the point of `ring` within `1e-9` of `target`.
fn position_near(ring: &[DVec3], target: DVec3) -> Option<usize> {
    ring.iter().position(|p| p.distance(target) < 1e-9)
}

#[test]
fn from_rings_merges_near_duplicates_and_drops_flat_vertices() {
    let planes: Vec<(DVec3, f64)> = cube_planes().iter().map(|p| p.to_halfspace_f64()).collect();
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("the cube is a closed solid");
    };
    let mut rings = mesh.rings;
    let corner = DVec3::ONE;
    let twin = corner + DVec3::new(2e-6, 0.0, 0.0);
    let edge_end = DVec3::new(1.0, 1.0, -1.0);
    let middle = DVec3::new(1.0, 1.0, 0.0);
    let mut twins = 0;
    let mut middles = 0;
    for (_, ring) in &mut rings {
        let a = position_near(ring, corner);
        let b = position_near(ring, edge_end);
        if let (Some(a), Some(b)) = (a, b) {
            let len = ring.len();
            let after = if (a + 1) % len == b { a } else { b };
            ring.insert(after + 1, middle);
            middles += 1;
        }
    }
    for (_, ring) in &mut rings {
        if let Some(at) = position_near(ring, corner) {
            ring.insert(at + 1, twin);
            twins += 1;
        }
    }
    assert_eq!(twins, 3, "three rings hold the corner");
    assert_eq!(middles, 2, "two rings hold the edge");
    let stone = BeamStone::from_rings(&planes, &rings).expect("the cleaned cube is closed");
    assert_eq!(stone.facet_count(), 6);
    assert_eq!(stone.vertex_count(), 8);
    assert_eq!(stone.edges().len(), 12);
    assert_closed_convex(&stone);
    for facet in 0..6 {
        assert_eq!(stone.ring(facet).len(), 4);
    }
}

#[test]
fn from_rings_refuses_an_open_surface() {
    let planes: Vec<(DVec3, f64)> = cube_planes().iter().map(|p| p.to_halfspace_f64()).collect();
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("the cube is a closed solid");
    };
    let open = &mesh.rings[..5];
    assert_eq!(
        BeamStone::from_rings(&planes, open).unwrap_err(),
        BeamOff::DegenerateStone
    );
}

#[test]
fn tools_an_empty_stone_and_an_unbounded_stone_are_refused() {
    assert_eq!(
        BeamStone::new(&cube_planes(), true).unwrap_err(),
        BeamOff::Concave
    );
    assert_eq!(
        BeamStone::new(&[], false).unwrap_err(),
        BeamOff::DegenerateStone
    );
    let open: Vec<GpuFacetPlane> = cube_planes().into_iter().take(5).collect();
    assert_eq!(
        BeamStone::new(&open, false).unwrap_err(),
        BeamOff::DegenerateStone
    );
}

#[test]
fn building_twice_gives_the_same_stone() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let a = BeamStone::new(&planes, false).expect("the brilliant is a stone");
    let b = BeamStone::new(&planes, false).expect("the brilliant is a stone");
    assert_eq!(a.vertex_count(), b.vertex_count());
    assert_eq!(a.edges(), b.edges());
    for facet in 0..facet_ids(&a) {
        assert_eq!(a.ring(facet), b.ring(facet));
        assert_eq!(a.aliases(facet), b.aliases(facet));
    }
    for id in 0..u32::try_from(a.vertex_count()).expect("fits") {
        assert_eq!(
            a.vertex(id).to_array().map(f64::to_bits),
            b.vertex(id).to_array().map(f64::to_bits)
        );
    }
}

#[test]
fn front_facets_agree_with_a_per_facet_dot_product() {
    let stones = [
        BeamStone::new(&StandardGemCuts::standard_round_brilliant(), false)
            .expect("the brilliant is a stone"),
        BeamStone::new(&pentagonal_planes(), false).expect("the pentagonal stone is a stone"),
        BeamStone::new(&cube_planes(), false).expect("a cube is a stone"),
    ];
    let eyes = [
        DVec3::new(0.0, 5.0, 0.0),
        DVec3::new(3.0, 0.2, 1.0),
        DVec3::new(-2.0, -4.0, 1.5),
        DVec3::new(0.3, 0.1, -6.0),
        DVec3::ZERO,
    ];
    for stone in &stones {
        for &eye in &eyes {
            let reference: Vec<u32> = (0..facet_ids(stone))
                .filter(|&facet| {
                    let (normal, offset) = stone.plane(facet);
                    normal.dot(eye) - offset > 0.0
                })
                .collect();
            assert_eq!(stone.front_facets(eye), reference);
        }
    }
}

#[test]
fn only_the_origin_sees_no_facet_and_a_far_eye_sees_the_facing_ones() {
    let stone = BeamStone::new(&cube_planes(), false).expect("a cube is a stone");
    assert_eq!(stone.front_facets(DVec3::ZERO), Vec::<u32>::new());
    assert_eq!(stone.front_facets(DVec3::new(0.0, 0.0, 5.0)), [4]);
    assert_eq!(stone.front_facets(DVec3::new(5.0, 5.0, 5.0)), [0, 2, 4]);
}
