//! `build_solid_mesh_and_measure` must equal `(build_solid_mesh, measure_solid)` bit for
//! bit while running ONE vertex enumeration and ONE face ring per plane.

use glam::DVec3;

use super::{
    SolidMesh, SolidMetrics, SolidStatus, build_solid_mesh, build_solid_mesh_and_measure,
    build_solid_mesh_and_measure_with_vertices, measure_solid, measure_solid_with_vertices,
    mesh::FACE_RING_CALLS,
    vertices::{FEASIBLE_VERTEX_CALLS, dedup_planes},
};
use crate::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane};

fn bits(v: DVec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

fn halfspaces(planes: Vec<GpuFacetPlane>) -> Vec<(DVec3, f64)> {
    planes
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect()
}

fn brilliant() -> Vec<(DVec3, f64)> {
    halfspaces(StandardGemCuts::standard_round_brilliant())
}

fn assert_metrics_bits(got: &SolidMetrics, want: &SolidMetrics, label: &str) {
    let f = |m: &SolidMetrics| {
        (
            [
                m.volume.to_bits(),
                m.width_axis.to_bits(),
                m.length_axis.to_bits(),
                m.width_caliper.to_bits(),
                m.length_caliper.to_bits(),
                m.total_height.to_bits(),
            ],
            [
                m.crown_height.map(f64::to_bits),
                m.pavilion_depth.map(f64::to_bits),
                m.girdle_thickness.map(f64::to_bits),
            ],
            m.vertex_count,
        )
    };
    assert_eq!(f(got), f(want), "{label}: metrics differ bitwise");
}

fn assert_mesh_bits(got: &SolidMesh, want: &SolidMesh, label: &str) {
    let pos = |v: &[DVec3]| v.iter().map(|&p| bits(p)).collect::<Vec<_>>();
    assert_eq!(
        pos(&got.positions),
        pos(&want.positions),
        "{label}: positions"
    );
    assert_eq!(pos(&got.normals), pos(&want.normals), "{label}: normals");
    assert_eq!(got.facet_id, want.facet_id, "{label}: facet_id");
    assert_eq!(got.indices, want.indices, "{label}: indices");
    assert_eq!(got.rings.len(), want.rings.len(), "{label}: ring count");
    for ((gi, gr), (wi, wr)) in got.rings.iter().zip(&want.rings) {
        assert_eq!(gi, wi, "{label}: ring owner");
        assert_eq!(pos(gr), pos(wr), "{label}: ring {gi}");
    }
    assert!(got.piece_normals.is_none() && want.piece_normals.is_none());
    assert!(got.edge_visible.is_none() && want.edge_visible.is_none());
}

fn assert_status_bits(got: &SolidStatus, want: &SolidStatus, label: &str) {
    match (got, want) {
        (SolidStatus::Closed(g), SolidStatus::Closed(w)) => assert_mesh_bits(g, w, label),
        (SolidStatus::Unbounded { escaping: g }, SolidStatus::Unbounded { escaping: w }) => {
            assert_eq!(g, w, "{label}: escaping planes");
        }
        (
            SolidStatus::Degenerate {
                vertex_count: gc,
                volume: gv,
            },
            SolidStatus::Degenerate {
                vertex_count: wc,
                volume: wv,
            },
        ) => {
            assert_eq!(gc, wc, "{label}: degenerate vertex count");
            assert_eq!(
                gv.map(f64::to_bits),
                wv.map(f64::to_bits),
                "{label}: degenerate volume"
            );
        }
        (g, w) => panic!("{label}: status variants differ: combined {g:?} vs separate {w:?}"),
    }
}

/// The combined calls against the separate ones, both entry points, bit for bit.
fn assert_combined_matches_separate(planes: &[(DVec3, f64)], label: &str) {
    let want_status = build_solid_mesh(planes);
    let want_metrics = measure_solid(planes);
    let want_with_vertices = measure_solid_with_vertices(planes);

    let (status, metrics) = build_solid_mesh_and_measure(planes);
    assert_status_bits(&status, &want_status, label);
    match (&metrics, &want_metrics) {
        (Some(g), Some(w)) => assert_metrics_bits(g, w, label),
        (None, None) => {}
        (g, w) => panic!("{label}: metrics presence differs: {g:?} vs {w:?}"),
    }

    let (status, with_vertices) = build_solid_mesh_and_measure_with_vertices(planes);
    assert_status_bits(&status, &want_status, label);
    match (&with_vertices, &want_with_vertices) {
        (Some((gm, gv)), Some((wm, wv))) => {
            assert_metrics_bits(gm, wm, label);
            let vb = |v: &[DVec3]| v.iter().map(|&p| bits(p)).collect::<Vec<_>>();
            assert_eq!(vb(gv), vb(wv), "{label}: vertices");
        }
        (None, None) => {}
        _ => panic!("{label}: vertex-measure presence differs"),
    }
}

fn scaled(planes: &[(DVec3, f64)], k: f64) -> Vec<(DVec3, f64)> {
    planes.iter().map(|&(n, m)| (n, m * k)).collect()
}

fn unit_box(y_half: f64) -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, y_half),
        (DVec3::NEG_Y, y_half),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]
}

#[test]
fn combined_matches_separate_on_the_standard_cuts() {
    assert_combined_matches_separate(&brilliant(), "round brilliant");
    assert_combined_matches_separate(&halfspaces(StandardGemCuts::emerald_cut()), "emerald cut");
}

#[test]
fn combined_matches_separate_with_duplicate_and_coplanar_planes() {
    let base = brilliant();
    for i in [0, 7, base.len() - 1] {
        let mut doubled = base.clone();
        doubled.push(base[i]);
        assert_combined_matches_separate(&doubled, &format!("brilliant + duplicate of {i}"));
    }
    let mut boxed = unit_box(0.6);
    boxed.insert(1, boxed[0]);
    assert_combined_matches_separate(&boxed, "box with a duplicated +X");
}

#[test]
fn combined_matches_separate_at_other_scales() {
    let base = brilliant();
    for k in [128.0, 0.013] {
        assert_combined_matches_separate(&scaled(&base, k), &format!("brilliant scaled by {k}"));
    }
}

#[test]
fn combined_matches_separate_on_unbounded_and_degenerate_sets() {
    let mut open = unit_box(0.6);
    open.remove(2); // no +Y closing plane
    assert_combined_matches_separate(&open, "unbounded: missing closing plane");

    // Every plane through the origin: bounded, one vertex.
    let point: Vec<(DVec3, f64)> = unit_box(0.6).iter().map(|&(n, _)| (n, 0.0)).collect();
    assert_combined_matches_separate(&point, "degenerate: single vertex");

    // Four vertices, zero volume.
    assert_combined_matches_separate(&unit_box(0.0), "degenerate: zero volume");
}

#[test]
fn combined_matches_separate_on_near_tolerance_slivers() {
    for y_half in [1.0e-6, 1.0e-5, 1.25e-5, 4.0e-5] {
        assert_combined_matches_separate(&unit_box(y_half), &format!("sliver slab {y_half}"));
    }
}

fn counted<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    FEASIBLE_VERTEX_CALLS.with(|c| c.set(0));
    FACE_RING_CALLS.with(|c| c.set(0));
    let result = f();
    (
        result,
        FEASIBLE_VERTEX_CALLS.with(std::cell::Cell::get),
        FACE_RING_CALLS.with(std::cell::Cell::get),
    )
}

#[test]
fn combined_builds_one_arrangement_and_one_ring_per_plane() {
    let planes = brilliant();
    let deduped = dedup_planes(&planes).len();
    let ((status, metrics), enumerations, rings) =
        counted(|| build_solid_mesh_and_measure(&planes));
    assert!(matches!(status, SolidStatus::Closed(_)));
    assert!(metrics.is_some());
    assert_eq!(enumerations, 1, "one vertex enumeration");
    assert_eq!(rings, deduped, "one face ring per deduplicated plane");
}

#[test]
fn the_mesh_alone_builds_one_ring_per_plane() {
    let planes = brilliant();
    let deduped = dedup_planes(&planes).len();
    let (status, enumerations, rings) = counted(|| build_solid_mesh(&planes));
    assert!(matches!(status, SolidStatus::Closed(_)));
    assert_eq!(enumerations, 1);
    assert_eq!(
        rings, deduped,
        "the volume and the triangulation share each ring"
    );
}

#[test]
fn separate_calls_enumerate_twice() {
    let planes = brilliant();
    let deduped = dedup_planes(&planes).len();
    let (_, enumerations, rings) = counted(|| {
        let mesh = build_solid_mesh(&planes);
        let metrics = measure_solid(&planes);
        (mesh, metrics)
    });
    assert_eq!(enumerations, 2);
    assert_eq!(rings, 2 * deduped);
}
