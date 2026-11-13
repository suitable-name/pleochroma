//! Tests of `geom::homography`.

use glam::{DMat3, DVec2, DVec3};

use super::homography::{
    Plane, PlaneBasis, apply, camera_direction_map, camera_to_plane, central_projection, compose,
    inverse,
};
use crate::optics::raytracer::Camera;

/// A seeded xorshift64 generator (deterministic, no dependencies).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)` with 32 bits of resolution.
    fn unit(&mut self) -> f64 {
        let high = u32::try_from(self.next_u64() >> 32).unwrap();
        f64::from(high) / 4_294_967_296.0
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        (hi - lo).mul_add(self.unit(), lo)
    }

    fn unit_vector(&mut self) -> DVec3 {
        loop {
            let v = DVec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let l2 = v.length_squared();
            if (0.1..=1.0).contains(&l2) {
                return v / l2.sqrt();
            }
        }
    }
}

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const WIDTH_F32: f32 = 640.0;
const HEIGHT_F32: f32 = 480.0;

/// Narrows to the `f32` the camera API takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the camera API is f32; the test values are small and the loss is intended"
)]
fn narrow(x: f64) -> f32 {
    x as f32
}

fn random_camera(rng: &mut Rng) -> Camera {
    let yaw = narrow(rng.range(-3.0, 3.0));
    let pitch = narrow(rng.range(-1.2, 1.2));
    let distance = narrow(rng.range(2.0, 4.0));
    Camera::new(yaw, pitch, distance, 42.0)
}

/// A random plane near the origin whose normal is well inclined to the viewing direction.
fn random_plane(rng: &mut Rng, camera: &Camera) -> Plane {
    let forward = camera.forward.as_dvec3();
    loop {
        let n = rng.unit_vector();
        if n.dot(forward).abs() >= 0.4 {
            return (n, rng.range(-0.4, 0.4));
        }
    }
}

/// The independent `f64` reference: the camera ray through `(sx, sy)` meets `plane`.
fn reference_world_point(camera: &Camera, sx: f64, sy: f64, plane: Plane) -> DVec3 {
    let width_px = f64::from(WIDTH);
    let height_px = f64::from(HEIGHT);
    let aspect = width_px / height_px;
    let tan = f64::from(camera.fov_tan);
    let u = (sx / width_px - 0.5) * 2.0 * aspect * tan;
    let v = (0.5 - sy / height_px) * 2.0 * tan;
    let dir = camera.forward.as_dvec3() + camera.right.as_dvec3() * u + camera.up.as_dvec3() * v;
    let origin = camera.origin.as_dvec3();
    let hit = (plane.1 - plane.0.dot(origin)) / plane.0.dot(dir);
    origin + dir * hit
}

fn close(a: DVec2, b: DVec2, tol: f64) -> bool {
    (a - b).length() <= tol * (1.0 + a.length().max(b.length()))
}

#[test]
fn plane_basis_is_orthonormal_and_lies_in_the_plane() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200 {
        let n = rng.unit_vector() * rng.range(0.5, 3.0);
        let off = rng.range(-2.0, 2.0);
        let basis = PlaneBasis::new((n, off));
        let unit = n.normalize();
        assert!((n.dot(basis.origin) - off).abs() < 1e-12);
        assert!((basis.u.length() - 1.0).abs() < 1e-14);
        assert!((basis.v.length() - 1.0).abs() < 1e-14);
        assert!(basis.u.dot(basis.v).abs() < 1e-14);
        assert!(basis.u.dot(unit).abs() < 1e-14);
        assert!(basis.v.dot(unit).abs() < 1e-14);
        assert!((basis.u.cross(basis.v) - unit).length() < 1e-14);
        let p = DVec2::new(rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let world = basis.to_world(p);
        assert!((n.dot(world) - off).abs() < 1e-12);
        assert!((basis.from_world(world) - p).length() < 1e-13);
    }
}

#[test]
fn plane_basis_of_a_degenerate_normal_is_finite() {
    for plane in [
        (DVec3::ZERO, 1.0),
        (DVec3::new(f64::NAN, 0.0, 1.0), 0.0),
        (DVec3::Z, f64::INFINITY),
    ] {
        let basis = PlaneBasis::new(plane);
        assert!(basis.origin.is_finite());
        assert!(basis.u.is_finite());
        assert!(basis.v.is_finite());
    }
}

#[test]
fn camera_to_plane_matches_the_independent_f64_formula() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..40 {
        let camera = random_camera(&mut rng);
        let plane = random_plane(&mut rng, &camera);
        let basis = PlaneBasis::new(plane);
        let h = camera_to_plane(&camera, WIDTH, HEIGHT, plane);
        for _ in 0..25 {
            let sx = rng.range(0.0, f64::from(WIDTH));
            let sy = rng.range(0.0, f64::from(HEIGHT));
            let world = reference_world_point(&camera, sx, sy, plane);
            let expected = basis.from_world(world);
            let got = apply(h, DVec2::new(sx, sy)).unwrap();
            assert!(
                close(got, expected, 1e-9),
                "({sx}, {sy}): {got:?} vs {expected:?}"
            );
        }
    }
}

#[test]
fn camera_to_plane_points_along_generate_ray() {
    // `generate_ray` works in f32, so its direction is only good to a few f32 ulps.
    let mut rng = Rng(0x0BAD_C0FF_EE12_3457);
    for _ in 0..40 {
        let camera = random_camera(&mut rng);
        let plane = random_plane(&mut rng, &camera);
        let basis = PlaneBasis::new(plane);
        let h = camera_to_plane(&camera, WIDTH, HEIGHT, plane);
        let origin = camera.origin.as_dvec3();
        for _ in 0..25 {
            let sx = rng.range(0.0, f64::from(WIDTH));
            let sy = rng.range(0.0, f64::from(HEIGHT));
            let ray = camera.generate_ray(narrow(sx), narrow(sy), WIDTH_F32, HEIGHT_F32, 0.0, 0.0);
            let world = basis.to_world(apply(h, DVec2::new(sx, sy)).unwrap());
            let dir = (world - origin).normalize();
            let cross = dir.cross(ray.dir.as_dvec3()).length();
            assert!(cross < 2e-6, "({sx}, {sy}): angle {cross}");
        }
    }
}

#[test]
fn camera_direction_map_matches_generate_ray() {
    let mut rng = Rng(0x7777_1111_2222_3333);
    for _ in 0..20 {
        let camera = random_camera(&mut rng);
        let map = camera_direction_map(&camera, WIDTH, HEIGHT);
        for _ in 0..25 {
            let sx = rng.range(0.0, f64::from(WIDTH));
            let sy = rng.range(0.0, f64::from(HEIGHT));
            let ray = camera.generate_ray(narrow(sx), narrow(sy), WIDTH_F32, HEIGHT_F32, 0.0, 0.0);
            let dir = (map * DVec3::new(sx, sy, 1.0)).normalize();
            assert!(dir.cross(ray.dir.as_dvec3()).length() < 2e-6);
            assert!(dir.dot(ray.dir.as_dvec3()) > 0.0);
        }
    }
}

#[test]
fn camera_to_plane_to_image_round_trips_to_a_nanopixel() {
    let mut rng = Rng(0x5555_AAAA_3333_CCCC);
    for _ in 0..40 {
        let camera = random_camera(&mut rng);
        let plane = random_plane(&mut rng, &camera);
        let basis = PlaneBasis::new(plane);
        let h = camera_to_plane(&camera, WIDTH, HEIGHT, plane);
        let h_inv = inverse(h).unwrap();
        let map_inv = camera_direction_map(&camera, WIDTH, HEIGHT).inverse();
        let origin = camera.origin.as_dvec3();
        for _ in 0..25 {
            let image = DVec2::new(
                rng.range(0.0, f64::from(WIDTH)),
                rng.range(0.0, f64::from(HEIGHT)),
            );
            let on_plane = apply(h, image).unwrap();
            let back = apply(h_inv, on_plane).unwrap();
            assert!(
                (back - image).length() < 1e-9,
                "{image:?} -> {on_plane:?} -> {back:?}"
            );
            // The independent direction-to-pixel inversion agrees.
            let dir = basis.to_world(on_plane) - origin;
            let hom = map_inv * dir;
            let pixel = DVec2::new(hom.x / hom.z, hom.y / hom.z);
            assert!((pixel - image).length() < 1e-9, "{image:?} vs {pixel:?}");
        }
    }
}

/// A plane facing `+z` (within ~53 degrees), for the apex-above-the-planes tests.
fn upward_plane(rng: &mut Rng) -> Plane {
    loop {
        let n = rng.unit_vector();
        if n.z >= 0.6 {
            return (n, rng.range(-0.3, 0.3));
        }
    }
}

#[test]
fn central_projection_matches_ray_plane_intersection() {
    let mut rng = Rng(0xDEAD_BEEF_0000_0001);
    for _ in 0..60 {
        let apex = DVec3::new(
            rng.range(-0.5, 0.5),
            rng.range(-0.5, 0.5),
            rng.range(3.0, 4.0),
        );
        let from = upward_plane(&mut rng);
        let to = upward_plane(&mut rng);
        let (from_basis, to_basis) = (PlaneBasis::new(from), PlaneBasis::new(to));
        let g = central_projection(apex, from, to);
        for _ in 0..20 {
            let a = DVec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
            let p = from_basis.to_world(a);
            let dir = p - apex;
            let s = (to.1 - to.0.dot(apex)) / to.0.dot(dir);
            let expected = to_basis.from_world(apex + dir * s);
            let got = apply(g, a).unwrap();
            assert!(
                close(got, expected, 1e-10),
                "{a:?}: {got:?} vs {expected:?}"
            );
        }
    }
}

#[test]
fn central_projection_round_trips_to_1e_12() {
    let mut rng = Rng(0xFACE_FEED_0000_0002);
    for _ in 0..60 {
        let apex = DVec3::new(
            rng.range(-0.5, 0.5),
            rng.range(-0.5, 0.5),
            rng.range(3.0, 4.0),
        );
        let from = upward_plane(&mut rng);
        let to = upward_plane(&mut rng);
        let g = central_projection(apex, from, to);
        let g_inv = inverse(g).unwrap();
        // The inverse is the projection the other way round.
        let reverse = central_projection(apex, to, from);
        for _ in 0..20 {
            let a = DVec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
            let b = apply(g, a).unwrap();
            let back = apply(g_inv, b).unwrap();
            assert!((back - a).length() < 1e-12, "{a:?} -> {b:?} -> {back:?}");
            let back_reverse = apply(reverse, b).unwrap();
            assert!((back_reverse - a).length() < 1e-10);
        }
    }
}

#[test]
fn camera_homography_composed_with_a_projection_is_the_camera_homography_of_the_next_plane() {
    let mut rng = Rng(0x0F0F_F0F0_1234_4321);
    for _ in 0..40 {
        let camera = random_camera(&mut rng);
        let first = random_plane(&mut rng, &camera);
        let next = random_plane(&mut rng, &camera);
        let image_to_first = camera_to_plane(&camera, WIDTH, HEIGHT, first);
        let direct = camera_to_plane(&camera, WIDTH, HEIGHT, next);
        let g = central_projection(camera.origin.as_dvec3(), first, next);
        let chained = compose(g, image_to_first);
        for _ in 0..25 {
            let image = DVec2::new(
                rng.range(0.0, f64::from(WIDTH)),
                rng.range(0.0, f64::from(HEIGHT)),
            );
            let want = apply(direct, image).unwrap();
            let got = apply(chained, image).unwrap();
            assert!(close(got, want, 1e-9), "{image:?}: {got:?} vs {want:?}");
        }
    }
}

fn random_matrix(rng: &mut Rng) -> DMat3 {
    let mut col = || {
        DVec3::new(
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
        )
    };
    DMat3::from_cols(col(), col(), col()) + DMat3::from_diagonal(DVec3::splat(2.5))
}

#[test]
fn compose_applies_the_right_operand_first() {
    let mut rng = Rng(0x1357_9BDF_2468_ACE0);
    let mut differs = false;
    for _ in 0..50 {
        let a = random_matrix(&mut rng);
        let b = random_matrix(&mut rng);
        let p = DVec2::new(rng.range(-2.0, 2.0), rng.range(-2.0, 2.0));
        let (Some(via_b), Some(composed)) = (apply(b, p), apply(compose(a, b), p)) else {
            continue;
        };
        let Some(stepwise) = apply(a, via_b) else {
            continue;
        };
        assert!(close(composed, stepwise, 1e-9));
        if let Some(swapped) = apply(compose(b, a), p) {
            differs |= (swapped - composed).length() > 1e-6;
        }
        // compose is a plain matrix product in a fixed order.
        assert_eq!(compose(a, b), a * b);
    }
    assert!(differs, "the order of compose must matter");
}

#[test]
fn inverse_composes_to_the_identity() {
    let mut rng = Rng(0x2468_ACE0_1357_9BDF);
    for _ in 0..50 {
        let a = random_matrix(&mut rng);
        let inv = inverse(a).unwrap();
        let id = compose(inv, a);
        for (got, want) in id
            .to_cols_array()
            .iter()
            .zip(DMat3::IDENTITY.to_cols_array())
        {
            assert!((got - want).abs() < 1e-12);
        }
    }
}

#[test]
fn singular_and_infinite_cases_return_none() {
    let singular = DMat3::from_cols(DVec3::X, DVec3::Y, DVec3::ZERO);
    assert_eq!(inverse(singular), None);
    assert_eq!(apply(singular, DVec2::new(1.0, 2.0)), None);
    let rank_one = DMat3::from_cols(DVec3::X, DVec3::X * 2.0, DVec3::X * 3.0);
    assert_eq!(inverse(rank_one), None);
    assert_eq!(
        inverse(DMat3::from_cols(DVec3::NAN, DVec3::Y, DVec3::Z)),
        None
    );
    assert_eq!(inverse(DMat3::IDENTITY), Some(DMat3::IDENTITY));
    assert_eq!(
        apply(DMat3::IDENTITY, DVec2::new(3.0, -4.0)),
        Some(DVec2::new(3.0, -4.0))
    );
}
