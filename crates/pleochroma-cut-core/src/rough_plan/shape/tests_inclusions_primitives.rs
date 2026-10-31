//! The posed primitives of inclusions (sphere, ellipsoid, box) and the texts of the errors an
//! inclusion that no longer fits gives.

use glam::DVec3;

use super::{
    InclusionFault, InclusionSet, MeshError, PrimitivePose, PrimitiveShape, ShapeError,
    primitive_mesh,
};

#[test]
fn a_sphere_primitive_contains_the_sphere_and_not_much_more() {
    let radius = 4.0;
    let mesh = primitive_mesh(
        PrimitiveShape::Sphere { radius_mm: radius },
        PrimitivePose::at([10.0; 3]),
    )
    .expect("a sphere");
    assert_eq!(mesh.triangles().len(), 80);
    // Every face is at least a radius from the centre: the shell contains the ball.
    for index in 0..80_u32 {
        let (n, d) = mesh.triangle_plane(index);
        let distance = d - n.dot(DVec3::splat(10.0));
        assert!(
            distance >= radius * (1.0 - 1e-9),
            "face {index} at {distance}"
        );
    }
    let ball = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);
    assert!(mesh.volume() >= ball, "{} under {ball}", mesh.volume());
    assert!(mesh.volume() < ball * 1.15, "{} over {ball}", mesh.volume());
    // Points just inside the sphere in many directions are inside the shell.
    for k in 0..40_u32 {
        let t = f64::from(k) / 39.0;
        let z = 2.0_f64.mul_add(t, -1.0);
        let ring = (1.0 - z * z).sqrt();
        let angle = f64::from(k) * 2.399_963;
        let direction = DVec3::new(ring * angle.cos(), ring * angle.sin(), z);
        assert!(mesh.contains_point(DVec3::splat(10.0) + direction * (radius * 0.999)));
    }
}

#[test]
fn an_ellipsoid_primitive_is_posed_and_contains_the_ellipsoid() {
    let semi = [5.0, 3.0, 2.0];
    let pose = PrimitivePose {
        centre_mm: [10.0, 11.0, 12.0],
        rotation_deg: [0.0, 0.0, 30.0],
    };
    let mesh = primitive_mesh(PrimitiveShape::Ellipsoid { semi_axes_mm: semi }, pose)
        .expect("an ellipsoid");
    let rotation = pose.rotation();
    let centre = DVec3::from(pose.centre_mm);
    for k in 0..60_u32 {
        let t = f64::from(k) / 59.0;
        let z = 2.0_f64.mul_add(t, -1.0);
        let ring = (1.0 - z * z).sqrt();
        let angle = f64::from(k) * 2.399_963;
        let unit = DVec3::new(ring * angle.cos(), ring * angle.sin(), z);
        let on_ellipsoid = rotation * (unit * DVec3::from(semi)) * 0.999 + centre;
        assert!(mesh.contains_point(on_ellipsoid), "direction {k}");
    }
    let volume = 4.0 / 3.0 * std::f64::consts::PI * semi[0] * semi[1] * semi[2];
    assert!(mesh.volume() >= volume && mesh.volume() < volume * 1.15);
    // The posed bounding box is the turned one's, around the centre.
    let (lo, hi) = mesh.bounds();
    assert!(((lo + hi) * 0.5 - centre).length() < 1e-9);
}

#[test]
fn a_box_primitive_is_exact_and_can_be_turned() {
    let straight = primitive_mesh(
        PrimitiveShape::Box {
            size_mm: [4.0, 6.0, 8.0],
        },
        PrimitivePose::at([10.0; 3]),
    )
    .expect("a box");
    assert!((straight.volume() - 192.0).abs() < 1e-9);
    let (lo, hi) = straight.bounds();
    assert!((lo - DVec3::new(8.0, 7.0, 6.0)).length() < 1e-9);
    assert!((hi - DVec3::new(12.0, 13.0, 14.0)).length() < 1e-9);
    let turned = primitive_mesh(
        PrimitiveShape::Box {
            size_mm: [4.0, 6.0, 8.0],
        },
        PrimitivePose {
            centre_mm: [10.0; 3],
            rotation_deg: [0.0, 0.0, 90.0],
        },
    )
    .expect("a turned box");
    assert!((turned.volume() - 192.0).abs() < 1e-9);
    let (lo, hi) = turned.bounds();
    assert!(
        (hi - lo - DVec3::new(6.0, 4.0, 8.0)).length() < 1e-9,
        "{lo} {hi}"
    );
}

#[test]
fn a_primitive_needs_a_usable_size_and_pose() {
    let at = PrimitivePose::at([10.0; 3]);
    for shape in [
        PrimitiveShape::Sphere { radius_mm: 0.0 },
        PrimitiveShape::Sphere { radius_mm: -1.0 },
        PrimitiveShape::Sphere {
            radius_mm: f64::NAN,
        },
        PrimitiveShape::Ellipsoid {
            semi_axes_mm: [1.0, 0.0, 1.0],
        },
        PrimitiveShape::Box {
            size_mm: [1.0, 1.0, f64::INFINITY],
        },
    ] {
        assert_eq!(
            primitive_mesh(shape, at),
            Err(MeshError::Degenerate),
            "{shape:?}"
        );
    }
    assert_eq!(
        primitive_mesh(
            PrimitiveShape::Sphere { radius_mm: 1.0 },
            PrimitivePose::at([f64::NAN, 0.0, 0.0])
        ),
        Err(MeshError::Degenerate)
    );
}

#[test]
fn a_primitive_is_deterministic() {
    let shape = PrimitiveShape::Ellipsoid {
        semi_axes_mm: [4.0, 3.0, 2.0],
    };
    let pose = PrimitivePose {
        centre_mm: [10.0; 3],
        rotation_deg: [10.0, 20.0, 30.0],
    };
    assert_eq!(primitive_mesh(shape, pose), primitive_mesh(shape, pose));
    let a =
        InclusionSet::from_meshes(vec![primitive_mesh(shape, pose).expect("mesh")]).expect("set");
    let b =
        InclusionSet::from_meshes(vec![primitive_mesh(shape, pose).expect("mesh")]).expect("set");
    assert_eq!(a, b);
}

#[test]
fn the_error_texts_say_which_inclusion_and_what_to_do() {
    let outside = ShapeError::Inclusion {
        index: 1,
        reason: InclusionFault::Outside,
    };
    let text = outside.to_string();
    assert!(
        text.contains("Inclusion 2") && text.contains("outside"),
        "{text}"
    );
    assert!(text.contains("Move or shrink it"), "{text}");
    let reaches = ShapeError::Inclusion {
        index: 0,
        reason: InclusionFault::ReachesSurface,
    };
    assert!(reaches.to_string().contains("reaches the surface"));
    let crossing = ShapeError::Inclusion {
        index: 2,
        reason: InclusionFault::Overlap,
    };
    assert!(crossing.to_string().contains("Inclusion 3"));
}
