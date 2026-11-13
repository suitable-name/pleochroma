//! Tests of the exact-ray retracer.

use glam::DVec2;

use super::{
    beam::{BranchPrefix, SpectralNode},
    retrace::{RetraceError, RetraceKit, camera_direction, exact_ray, is_total_reflection},
    stone::BeamStone,
    tests_support::{BeamRig, glass},
};
use crate::renderer::path_cache::bounce::EventKind;

/// The frame size the tests use.
const SIZE: u32 = 64;
/// The spectral node count.
const N_LAMBDA: u32 = 48;
/// The cube's top facet (+Y) and bottom facet (−Y), and the ±X sides (original indices).
const TOP: u32 = 2;
const BOTTOM: u32 = 3;
const PLUS_X: u32 = 0;
const MINUS_X: u32 = 1;

/// One prefix step word: the facet index in the high bits, the event code in bits 1 to 2.
fn word(facet: u32, kind: EventKind) -> u32 {
    let code = match kind {
        EventKind::Reflect | EventKind::TotalReflect => 1,
        EventKind::Transmit => 3,
    };
    (facet << 8) | (code << 1)
}

fn prefix(steps: &[(u32, EventKind)]) -> BranchPrefix {
    steps.iter().fold(BranchPrefix::EMPTY, |p, &(f, k)| {
        p.push(word(f, k)).expect("short prefix")
    })
}

fn with_kit<R>(f: impl FnOnce(&RetraceKit<'_>) -> R) -> R {
    let rig = BeamRig::cube(glass());
    let stone = BeamStone::new(&rig.planes, false).expect("the cube is a stone");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (SIZE, SIZE),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    f(&kit)
}

#[test]
fn the_camera_direction_is_the_tracers_unjittered_ray() {
    let rig = BeamRig::cube(glass());
    for (x, y) in [(0u32, 0u32), (17, 40), (32, 32), (63, 5)] {
        let traced = rig.pixel_ray(SIZE, (x, y)).dir.as_dvec3();
        let ours = camera_direction(
            &rig.camera,
            (SIZE, SIZE),
            DVec2::new(f64::from(x), f64::from(y)),
        )
        .normalize();
        assert!(
            (traced - ours).length() < 1e-6,
            "pixel ({x}, {y}): {traced} vs {ours}"
        );
    }
}

#[test]
fn an_empty_prefix_is_the_camera_ray() {
    with_kit(|kit| {
        let p = DVec2::new(20.5, 33.25);
        let ray = exact_ray(p, SpectralNode(10), &BranchPrefix::EMPTY, kit).expect("camera ray");
        assert_eq!(ray.len, 0);
        assert!(!ray.inside);
        let expected = camera_direction(kit.camera, kit.size, p).normalize();
        assert!((ray.dir - expected).length() < 1e-15);
    });
}

#[test]
fn a_slab_passes_the_ray_through_parallel() {
    with_kit(|kit| {
        let p = DVec2::new(40.0, 27.0);
        let through = prefix(&[(TOP, EventKind::Transmit), (BOTTOM, EventKind::Transmit)]);
        let ray = exact_ray(p, SpectralNode(20), &through, kit).expect("through the slab");
        let incident = camera_direction(kit.camera, kit.size, p).normalize();
        assert!(!ray.inside);
        assert!(
            (ray.dir - incident).length() < 1e-12,
            "{} vs {incident}",
            ray.dir
        );
        assert!((ray.hits[0].y - 1.0).abs() < 1e-9);
        assert!((ray.hits[1].y + 1.0).abs() < 1e-9);
    });
}

#[test]
fn a_first_surface_reflection_mirrors_the_camera_ray() {
    with_kit(|kit| {
        let p = DVec2::new(31.0, 36.0);
        let ray = exact_ray(
            p,
            SpectralNode(0),
            &prefix(&[(TOP, EventKind::Reflect)]),
            kit,
        )
        .expect("reflection");
        let incident = camera_direction(kit.camera, kit.size, p).normalize();
        assert!(!ray.inside);
        assert!((ray.dir.y + incident.y).abs() < 1e-12);
        assert!((ray.dir.x - incident.x).abs() < 1e-12);
        assert!((ray.dir.z - incident.z).abs() < 1e-12);
    });
}

#[test]
fn a_steep_side_exit_is_total_internal_reflection() {
    with_kit(|kit| {
        // Pixel 52 enters the top inside the cube's footprint (|x| about 0.93) and the refracted
        // ray reaches the side wall before the bottom; pixel 60 enters at |x| about 1.3, outside
        // the cube, so its side plane lies behind the ray and the retracer rightly reports a miss.
        let p = DVec2::new(52.0, 32.0);
        let node = SpectralNode(24);
        let entered =
            exact_ray(p, node, &prefix(&[(TOP, EventKind::Transmit)]), kit).expect("entry");
        assert!(entered.inside);
        let side = if entered.dir.x > 0.0 { PLUS_X } else { MINUS_X };
        let side_facet = kit.stone.facet_of_original(side).expect("side facet");
        assert!(is_total_reflection(
            entered.dir,
            side_facet,
            node.centre_nm(N_LAMBDA),
            kit
        ));
        let out = exact_ray(
            p,
            node,
            &prefix(&[(TOP, EventKind::Transmit), (side, EventKind::Transmit)]),
            kit,
        );
        assert_eq!(out, Err(RetraceError::TotalReflection { step: 1 }));
        let reflected = exact_ray(
            p,
            node,
            &prefix(&[(TOP, EventKind::Transmit), (side, EventKind::TotalReflect)]),
            kit,
        )
        .expect("total internal reflection");
        assert!(reflected.inside);
        assert!((reflected.dir.x + entered.dir.x).abs() < 1e-12);
    });
}

#[test]
fn an_unknown_facet_is_refused() {
    with_kit(|kit| {
        let out = exact_ray(
            DVec2::new(32.0, 32.0),
            SpectralNode(0),
            &prefix(&[(999, EventKind::Reflect)]),
            kit,
        );
        assert_eq!(out, Err(RetraceError::UnknownFacet { step: 0 }));
    });
}
