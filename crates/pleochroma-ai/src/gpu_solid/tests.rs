//! Opt-in GPU fixtures. Do not run without an explicitly authorized CUDA host.
use super::*;
use pleochroma::geometry::stone_metrics::{
    build_solid_mesh_and_measure_with_vertices, build_solid_mesh_geom, feasible_plane_triples,
    mesh_volume,
};

fn box_planes(scale: f64) -> Vec<(DVec3, f64)> {
    [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .map(|n| (n, scale))
    .to_vec()
}

fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 2e-12 * a.abs().max(b.abs()).max(f64::MIN_POSITIVE),
        "{a} != {b}"
    );
}

fn check_planar(gpu: &GpuSolid, planes: &[(DVec3, f64)]) {
    let actual = gpu.build_and_measure(planes, &[]).expect("GPU arrangement");
    let (expected, measured) = build_solid_mesh_and_measure_with_vertices(planes);
    match (actual.status, expected) {
        (SolidStatus::Closed(a), SolidStatus::Closed(b)) => {
            assert_eq!(a.facet_id, b.facet_id);
            assert_eq!(a.indices, b.indices);
            assert_eq!(a.rings.len(), b.rings.len());
            assert_eq!(a.positions.len(), b.positions.len());
            for (a, b) in a.positions.iter().zip(&b.positions) {
                let error = (*a - *b).abs().max_element();
                assert!(error <= 2e-12 * a.abs().max_element().max(1e-30));
            }
            let (expected, vertices) = measured.expect("CPU metrics");
            let got = actual.metrics.expect("GPU metrics");
            close(got.volume, expected.volume);
            close(got.width_axis, expected.width_axis);
            close(got.length_axis, expected.length_axis);
            close(got.total_height, expected.total_height);
            close(got.width_caliper, expected.width_caliper);
            close(got.length_caliper, expected.length_caliper);
            assert_eq!(got.vertex_count, expected.vertex_count);
            assert_eq!(actual.vertices.len(), vertices.len());
            for (a, b) in actual.vertices.iter().zip(vertices) {
                assert!((*a - b).abs().max_element() <= 2e-12 * a.abs().max_element().max(1e-30));
            }
        }
        (SolidStatus::Unbounded { escaping: a }, SolidStatus::Unbounded { escaping: b }) => {
            assert_eq!(a, b)
        }
        (
            SolidStatus::Degenerate {
                vertex_count: a,
                volume: va,
            },
            SolidStatus::Degenerate {
                vertex_count: b,
                volume: vb,
            },
        ) => {
            assert_eq!(a, b);
            assert_eq!(va, vb);
        }
        (a, b) => panic!("GPU/CPU status mismatch: {a:?}, {b:?}"),
    }
}

#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn scaled_unbounded_near_boundary_dedup_and_degenerate_parity() {
    let gpu = GpuSolid::new_cuda(0).expect("CUDA device");
    for scale in [2.0_f64.powi(-40), 1.0, 2.0_f64.powi(40)] {
        check_planar(&gpu, &box_planes(scale));
    }
    let mut planes = box_planes(1.0);
    planes.pop();
    check_planar(&gpu, &planes);
    let mut planes = box_planes(1.0);
    let normal = DVec3::new(1.0, 0.002, 0.0).normalize();
    planes.push((normal, 1.0 - 1e-5));
    check_planar(&gpu, &planes);
    planes.extend([
        planes[0],
        planes[0],
        (DVec3::new(1.0, 1e-8, 0.0).normalize(), 1.0),
    ]);
    check_planar(&gpu, &planes);
    let mut flat = box_planes(1.0);
    flat[2].1 = 0.0;
    flat[3].1 = 0.0;
    check_planar(&gpu, &flat);
    gpu.check().expect("no native failures");
}

#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn lexicographic_alias_triples_match_production_and_repeat_deterministically() {
    let gpu = GpuSolid::new_cuda(0).expect("CUDA device");
    let mut planes = box_planes(1.0);
    planes.extend([planes[0], planes[2], planes[4]]);
    let cpu = feasible_plane_triples(&planes, 2.0).expect("CPU triples");
    let actual = gpu
        .feasible_plane_triples(&planes, 2.0)
        .expect("GPU triples")
        .expect("bounded");
    let again = gpu
        .feasible_plane_triples(&planes, 2.0)
        .expect("GPU repeat")
        .expect("bounded");
    assert_eq!(actual, again);
    assert_eq!(actual.len(), cpu.len());
    for ((ta, pa), (tb, pb)) in actual.iter().zip(cpu) {
        assert_eq!(*ta, tb);
        assert!((*pa - pb).abs().max_element() < 2e-12);
    }
}

#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn concave_gpu_tool_faces_keep_surface_provenance_and_count_cpu_topology() {
    let gpu = GpuSolid::new_cuda(0).expect("CUDA device");
    let planes = box_planes(1.0);
    let tool = ToolPrimitive {
        kind: 0,
        sweep_kind: 0,
        _pad: [0; 2],
        origin: [0.0, 1.0, 0.0, 0.4],
        axis: [0.0, 1.0, 0.0, 0.0],
        profile: [0.0; 4],
        sweep_dir: [1.0, 0.0, 0.0, 0.0],
    };
    let tools = [tool];
    let actual = gpu
        .build_and_measure(&planes, &tools)
        .expect("CUDA concave mesh");
    let expected = build_solid_mesh_geom(&planes, &tools);
    let (SolidStatus::Closed(a), SolidStatus::Closed(b)) = (actual.status, expected) else {
        panic!("closed concave fixture");
    };
    close(mesh_volume(&a), mesh_volume(&b));
    assert_eq!(a.facet_id, b.facet_id);
    assert_eq!(a.indices, b.indices);
    assert_eq!(a.edge_visible, b.edge_visible);
    assert_eq!(a.piece_normals, b.piece_normals);
    assert_eq!(a.rings.len(), b.rings.len());
    for ((fa, ra), (fb, rb)) in a.rings.iter().zip(&b.rings) {
        assert_eq!(fa, fb);
        assert_eq!(ra.len(), rb.len());
        for (pa, pb) in ra.iter().zip(rb) {
            assert!((*pa - *pb).abs().max_element() < 2e-12);
        }
    }
    let stats = gpu.diagnostics();
    assert!(stats.cuda_tool_faces > 0);
    assert_eq!(stats.cpu_concave_fallbacks, 1);
    assert!(!stats.cpu_fallback_reasons.is_empty());
    close(
        gpu.measure_width(&planes)
            .expect("planar width")
            .expect("bounded"),
        2.0,
    );
}

#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn optical_f32_width_and_fully_carved_width_cache_keep_planar_contract() {
    use pleochroma::geometry::plane::GpuFacetPlane;
    let gpu = GpuSolid::new_cuda(0).expect("CUDA device");
    let (s, c) = 0.23_f32.sin_cos();
    let planes = [
        glam::Vec3::new(c, 0.0, s),
        glam::Vec3::new(-c, 0.0, -s),
        glam::Vec3::Y,
        glam::Vec3::NEG_Y,
        glam::Vec3::new(-s, 0.0, c),
        glam::Vec3::new(s, 0.0, -c),
    ]
    .map(|n| GpuFacetPlane::new(n, -1.123_456_7));
    let widened: Vec<_> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let expected =
        pleochroma::render_setup::measure_model_width(&planes).expect("CPU optical width");
    close(
        gpu.measure_width(&widened)
            .expect("CUDA width")
            .expect("bounded"),
        expected,
    );
    let flat = box_planes(1.0);
    let tool = ToolPrimitive {
        kind: 0,
        sweep_kind: 0,
        _pad: [0; 2],
        origin: [0.0, 0.0, 0.0, 5.0],
        axis: [0.0, 1.0, 0.0, 0.0],
        profile: [0.0; 4],
        sweep_dir: [1.0, 0.0, 0.0, 0.0],
    };
    let carved = gpu
        .build_and_measure(&flat, &[tool])
        .expect("fully carved candidate");
    assert!(matches!(carved.status, SolidStatus::Degenerate { .. }));
    close(
        gpu.measure_width(&flat)
            .expect("planar cached width")
            .expect("planar remains closed"),
        2.0,
    );
    assert!(gpu.diagnostics().width_cache_hits > 0);
}
