//! Tests of `geom::frame`.

use glam::{DVec2, DVec3};

use super::{frame::Frame2, tests_polygon::Rng};

#[test]
fn frame_is_orthonormal_and_right_handed() {
    let mut rng = Rng::new(31);
    for _ in 0..300 {
        let normal = rng.unit_vec3() * rng.range(0.5, 4.0);
        let origin = rng.unit_vec3() * 2.0;
        let f = Frame2::from_normal(origin, normal).unwrap();
        assert!((f.u.length() - 1.0).abs() < 1e-14);
        assert!((f.v.length() - 1.0).abs() < 1e-14);
        assert!((f.normal.length() - 1.0).abs() < 1e-14);
        assert!(f.u.dot(f.v).abs() < 1e-14);
        assert!(f.u.dot(f.normal).abs() < 1e-14);
        assert!(f.v.dot(f.normal).abs() < 1e-14);
        assert!((f.u.cross(f.v) - f.normal).length() < 1e-14);
        assert!((f.normal - normal.normalize()).length() < 1e-14);
    }
}

#[test]
fn frame_round_trips_between_2d_and_3d() {
    let mut rng = Rng::new(32);
    for _ in 0..200 {
        let f = Frame2::from_normal(rng.unit_vec3(), rng.unit_vec3()).unwrap();
        let q = DVec2::new(rng.range(-5.0, 5.0), rng.range(-5.0, 5.0));
        let p = f.to_3d(q);
        assert!(f.signed_distance(p).abs() < 1e-14);
        assert!((f.to_2d(p) - q).length() < 1e-14);
        // A point off the plane projects onto it; the projection is idempotent.
        let off = p + f.normal * 0.7;
        assert!((f.signed_distance(off) - 0.7).abs() < 1e-14);
        assert!((f.project(off) - p).length() < 1e-14);
        assert!((f.project(f.project(off)) - f.project(off)).length() < 1e-14);
        assert!((f.offset() - f.normal.dot(f.origin)).abs() < 1e-15);
    }
}

#[test]
fn frame_rejects_a_bad_normal_and_the_image_frame_is_the_identity() {
    assert_eq!(Frame2::from_normal(DVec3::ZERO, DVec3::ZERO), None);
    assert_eq!(
        Frame2::from_normal(DVec3::ZERO, DVec3::new(f64::NAN, 0.0, 1.0)),
        None
    );
    assert_eq!(
        Frame2::from_normal(DVec3::new(f64::INFINITY, 0.0, 0.0), DVec3::Z),
        None
    );
    let q = DVec2::new(3.5, -2.25);
    assert_eq!(Frame2::IMAGE.to_3d(q), DVec3::new(3.5, -2.25, 0.0));
    assert_eq!(Frame2::IMAGE.to_2d(DVec3::new(3.5, -2.25, 9.0)), q);
    // Axis-aligned normals take the deterministic axis choice.
    let f = Frame2::from_normal(DVec3::ZERO, DVec3::Z).unwrap();
    assert!((f.u.cross(f.v) - DVec3::Z).length() < 1e-15);
}
