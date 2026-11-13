//! Tests of `geom::circle`.

use std::f64::consts::PI;

use glam::{DMat2, DMat3, DVec2, DVec3};

use super::{
    circle::{
        ImageEllipse, MAX_INSCRIBED_VERTICES, circle_polygon_area, ellipse_polygon_area,
        inscribed_polygon, sun_disc_preimage, sun_disc_preimage_jacobian,
    },
    homography::camera_direction_map,
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
}

const SAMPLES: u32 = 1_000_000;

/// Even-odd point in polygon (valid for any simple polygon, either winding).
fn polygon_contains(poly: &[DVec2], p: DVec2) -> bool {
    let mut inside = false;
    for (a, b) in poly.iter().zip(poly.iter().cycle().skip(poly.len() - 1)) {
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Asserts `exact` is within 4 sigma of a 10^6-point Monte Carlo estimate of the area of the
/// polygon intersected with the ellipse, sampled in the ellipse's bounding box.
///
/// 4 sigma, not 3: the seeds are fixed, and with about twenty estimates a 3 sigma bound would fail
/// by chance for one seed in thirty.
fn assert_matches_monte_carlo(centre: DVec2, axes: DMat2, poly: &[DVec2], exact: f64, seed: u64) {
    let half = DVec2::new(
        axes.x_axis.x.hypot(axes.y_axis.x),
        axes.x_axis.y.hypot(axes.y_axis.y),
    );
    let box_area = 4.0 * half.x * half.y;
    let inv = axes.inverse();
    let mut rng = Rng(seed);
    let mut hits = 0_u32;
    for _ in 0..SAMPLES {
        let p = centre
            + half
                * DVec2::new(
                    2.0_f64.mul_add(rng.unit(), -1.0),
                    2.0_f64.mul_add(rng.unit(), -1.0),
                );
        if (inv * (p - centre)).length_squared() <= 1.0 && polygon_contains(poly, p) {
            hits += 1;
        }
    }
    let estimate = box_area * f64::from(hits) / f64::from(SAMPLES);
    let fraction = (exact / box_area).clamp(1e-9, 1.0 - 1e-9);
    let sigma = box_area * (fraction * (1.0 - fraction) / f64::from(SAMPLES)).sqrt();
    assert!(
        (estimate - exact).abs() <= 4.0_f64.mul_add(sigma, 1e-12),
        "exact {exact} vs Monte Carlo {estimate} (sigma {sigma})"
    );
}

fn reversed(poly: &[DVec2]) -> Vec<DVec2> {
    poly.iter().rev().copied().collect()
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<DVec2> {
    vec![
        DVec2::new(x0, y0),
        DVec2::new(x1, y0),
        DVec2::new(x1, y1),
        DVec2::new(x0, y1),
    ]
}

fn shoelace(poly: &[DVec2]) -> f64 {
    0.5 * poly
        .iter()
        .zip(poly.iter().cycle().skip(1))
        .map(|(a, b)| a.perp_dot(*b))
        .sum::<f64>()
}

#[test]
fn circle_polygon_area_exact_cases() {
    let o = DVec2::ZERO;
    // Disc inside the polygon, polygon inside the disc, disjoint.
    assert!((circle_polygon_area(o, 1.0, &square(-3.0, -3.0, 3.0, 3.0)) - PI).abs() < 1e-12);
    assert!((circle_polygon_area(o, 5.0, &square(-1.0, -1.0, 1.0, 1.0)) - 4.0).abs() < 1e-12);
    assert!(circle_polygon_area(o, 1.0, &square(2.0, 2.0, 3.0, 3.0)).abs() < 1e-15);
    // A half disc and a quarter disc.
    assert!((circle_polygon_area(o, 1.0, &square(0.0, -2.0, 2.0, 2.0)) - PI / 2.0).abs() < 1e-12);
    assert!((circle_polygon_area(o, 1.0, &square(0.0, 0.0, 2.0, 2.0)) - PI / 4.0).abs() < 1e-12);
    // The centre is not at the origin.
    let c = DVec2::new(3.0, -2.0);
    assert!(
        (circle_polygon_area(c, 0.5, &square(3.0, -4.0, 5.0, 0.0)) - PI * 0.25 / 2.0).abs() < 1e-12
    );
    // Winding does not matter.
    let sq = square(0.0, 0.0, 2.0, 2.0);
    assert!((circle_polygon_area(o, 1.0, &reversed(&sq)) - PI / 4.0).abs() < 1e-12);
}

#[test]
fn circle_polygon_area_degenerate_inputs_are_zero() {
    let sq = square(-1.0, -1.0, 1.0, 1.0);
    assert_eq!(circle_polygon_area(DVec2::ZERO, 0.0, &sq), 0.0);
    assert_eq!(circle_polygon_area(DVec2::ZERO, -1.0, &sq), 0.0);
    assert_eq!(circle_polygon_area(DVec2::ZERO, f64::NAN, &sq), 0.0);
    assert_eq!(circle_polygon_area(DVec2::ZERO, 1.0, &sq[..2]), 0.0);
    assert_eq!(circle_polygon_area(DVec2::ZERO, 1.0, &[]), 0.0);
}

fn test_polygons() -> Vec<Vec<DVec2>> {
    vec![
        square(-0.5, -1.0, 1.5, 0.8),
        vec![
            DVec2::new(-1.0, -1.0),
            DVec2::new(2.5, 0.3),
            DVec2::new(0.4, 1.9),
        ],
        // Concave L shape.
        vec![
            DVec2::new(-2.0, -2.0),
            DVec2::new(1.0, -2.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(0.0, 0.0),
            DVec2::new(0.0, 1.5),
            DVec2::new(-2.0, 1.5),
        ],
        // A sliver crossing the circle.
        vec![
            DVec2::new(-3.0, 0.1),
            DVec2::new(3.0, 0.2),
            DVec2::new(3.0, 0.35),
        ],
        // Fully inside, and one containing the circle.
        square(0.0, -0.3, 0.4, 0.2),
        square(-5.0, -5.0, 5.0, 5.0),
        // Circle centre outside the polygon, circle crossing one edge.
        square(0.9, -1.0, 3.0, 1.0),
    ]
}

#[test]
fn circle_polygon_area_matches_monte_carlo() {
    let centre = DVec2::new(0.2, -0.1);
    let r = 1.3;
    let axes = DMat2::from_diagonal(DVec2::splat(r));
    for (i, poly) in (0_u64..).zip(test_polygons().iter()) {
        let exact = circle_polygon_area(centre, r, poly);
        assert!(exact >= 0.0);
        assert_matches_monte_carlo(centre, axes, poly, exact, 0x1000 + i * 7919 + 1);
        let flipped = circle_polygon_area(centre, r, &reversed(poly));
        assert!((flipped - exact).abs() < 1e-12);
    }
}

#[test]
fn ellipse_polygon_area_exact_cases() {
    let centre = DVec2::new(1.0, 2.0);
    let axes = DMat2::from_cols(DVec2::new(2.0, 0.5), DVec2::new(-0.3, 1.0));
    let det = axes.determinant().abs();
    let big = square(-50.0, -50.0, 50.0, 50.0);
    assert!(
        PI.mul_add(-det, ellipse_polygon_area(centre, axes, &big))
            .abs()
            < 1e-11
    );
    // The ellipse is symmetric about its centre: a line through it halves it. The polygon is the
    // half plane to one side of the line through the centre along the first axis.
    let d = axes.x_axis;
    let n = DVec2::new(-d.y, d.x);
    let half: Vec<DVec2> = vec![
        centre - d * 100.0,
        centre + d * 100.0,
        centre + d * 100.0 + n * 100.0,
        centre - d * 100.0 + n * 100.0,
    ];
    assert!((ellipse_polygon_area(centre, axes, &half) - PI * det / 2.0).abs() < 1e-10);
    assert_eq!(ellipse_polygon_area(centre, DMat2::ZERO, &big), 0.0);
    assert_eq!(
        ellipse_polygon_area(centre, DMat2::from_cols(DVec2::X, DVec2::X), &big),
        0.0
    );
}

#[test]
fn ellipse_polygon_area_matches_monte_carlo() {
    let centre = DVec2::new(0.4, -0.2);
    let axes = DMat2::from_cols(DVec2::new(1.6, 0.5), DVec2::new(-0.3, 0.9));
    for (i, poly) in (0_u64..).zip(test_polygons().iter()) {
        let exact = ellipse_polygon_area(centre, axes, poly);
        assert!(exact >= 0.0);
        assert_matches_monte_carlo(centre, axes, poly, exact, 0x9000 + i * 104_729 + 3);
        // A mirrored (negative determinant) parametrisation gives the same ellipse.
        let mirrored = DMat2::from_cols(axes.y_axis, axes.x_axis);
        let same = ellipse_polygon_area(centre, mirrored, poly);
        assert!((same - exact).abs() < 1e-12);
    }
}

#[test]
fn inscribed_polygon_respects_the_sagitta_bound() {
    let centre = DVec2::new(-1.0, 4.0);
    for r in [0.5, 1.0, 37.0] {
        for sagitta in [0.5, 0.1, 1e-2, 1e-3, 1e-4] {
            let poly = inscribed_polygon(centre, r, sagitta);
            let n = poly.len();
            assert!(n >= 3);
            assert!(shoelace(&poly) > 0.0, "counter-clockwise");
            for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
                assert!(
                    ((*a - centre).length() - r).abs() < 1e-12 * r,
                    "on the circle"
                );
                let gap = r - ((*a + *b) * 0.5 - centre).length();
                assert!(
                    gap <= 1e-12_f64.mul_add(r, sagitta),
                    "r {r} s {sagitta}: gap {gap}"
                );
            }
            // The polygon is inside the circle, so the intersection is the polygon itself.
            let area = shoelace(&poly);
            assert!((circle_polygon_area(centre, r, &poly) - area).abs() < 1e-9 * r * r);
            assert!(area < PI * r * r);
            // And it is the smallest such polygon: one vertex fewer breaks the bound.
            if n > 3 {
                let half = PI / f64::from(u32::try_from(n - 1).unwrap());
                assert!(r * (1.0 - half.cos()) > sagitta);
            }
        }
    }
}

#[test]
fn inscribed_polygon_edge_cases() {
    assert_eq!(inscribed_polygon(DVec2::ZERO, 0.0, 0.1), Vec::new());
    assert_eq!(inscribed_polygon(DVec2::ZERO, -1.0, 0.1), Vec::new());
    assert_eq!(inscribed_polygon(DVec2::ZERO, 1.0, f64::NAN), Vec::new());
    // A huge allowance is the triangle.
    assert_eq!(inscribed_polygon(DVec2::ZERO, 1.0, 10.0).len(), 3);
    // No allowance is the capped polygon.
    let cap = usize::try_from(MAX_INSCRIBED_VERTICES).unwrap();
    assert_eq!(inscribed_polygon(DVec2::ZERO, 1.0, 0.0).len(), cap);
    assert_eq!(inscribed_polygon(DVec2::ZERO, 1.0, -1.0).len(), cap);
}

/// The sun-disc setup: a camera, a mirror and a sun near the mirrored view of the image centre.
struct SunSetup {
    dir_map: DMat3,
    sun: DVec3,
}

fn sun_setup(offset: DVec2) -> SunSetup {
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let map = camera_direction_map(&camera, 640, 480);
    let normal = DVec3::new(0.3, 0.8, -0.5).normalize();
    let mirror = DMat3::IDENTITY
        - DMat3::from_cols(normal * normal.x, normal * normal.y, normal * normal.z) * 2.0;
    let dir_map = mirror * map;
    let sun = (dir_map * DVec3::new(320.0 + offset.x, 240.0 + offset.y, 1.0)).normalize();
    SunSetup { dir_map, sun }
}

/// The angle between `v` and `s`, accurate for small angles.
fn angle_between(v: DVec3, s: DVec3) -> f64 {
    v.cross(s).length().atan2(v.dot(s))
}

#[test]
fn sun_disc_preimage_contains_exactly_the_points_inside_the_disc() {
    for (offset, radius_deg) in [
        (DVec2::new(12.0, -7.0), 0.27_f64),
        (DVec2::new(-30.0, 25.0), 0.27),
        (DVec2::new(5.0, 5.0), 3.0),
        (DVec2::new(-60.0, 10.0), 8.0),
    ] {
        let setup = sun_setup(offset);
        let r = radius_deg.to_radians();
        let ellipse = sun_disc_preimage(setup.dir_map, setup.sun, r.tan()).expect("an ellipse");
        let reach = DVec2::new(
            ellipse.axes.x_axis.x.hypot(ellipse.axes.y_axis.x),
            ellipse.axes.x_axis.y.hypot(ellipse.axes.y_axis.y),
        ) * 1.5;
        let (mut inside, mut checked) = (0_u32, 0_u32);
        for iy in 0..300_u32 {
            for ix in 0..300_u32 {
                let p = ellipse.centre
                    + reach
                        * DVec2::new(
                            2.0 * f64::from(ix) / 299.0 - 1.0,
                            2.0 * f64::from(iy) / 299.0 - 1.0,
                        );
                let v = setup.dir_map * p.extend(1.0);
                let angle = angle_between(v, setup.sun);
                if (angle - r).abs() < 1e-9 {
                    continue;
                }
                let truth = angle < r;
                assert_eq!(
                    ellipse.contains(p),
                    truth,
                    "r {radius_deg} deg, point {p:?}: angle {angle} vs {r}"
                );
                checked += 1;
                inside += u32::from(truth);
            }
        }
        assert!(checked > 80_000);
        assert!(
            inside > 100,
            "the grid must actually hit the disc ({inside})"
        );
        // The ellipse area agrees with its parametrisation: a polygon far larger than it.
        let big = square(
            ellipse.centre.x - 1e4,
            ellipse.centre.y - 1e4,
            ellipse.centre.x + 1e4,
            ellipse.centre.y + 1e4,
        );
        assert!((ellipse.polygon_area(&big) - ellipse.area()).abs() < 1e-9 * ellipse.area());
    }
}

#[test]
fn sun_disc_preimage_rejects_the_antipodal_sun_and_bad_inputs() {
    let setup = sun_setup(DVec2::new(3.0, 4.0));
    let tan = 0.27_f64.to_radians().tan();
    assert!(sun_disc_preimage(setup.dir_map, setup.sun, tan).is_some());
    // The same cone, the other nappe: the sun is behind the leaf's directions.
    assert_eq!(sun_disc_preimage(setup.dir_map, -setup.sun, tan), None);
    assert_eq!(sun_disc_preimage(setup.dir_map, DVec3::ZERO, tan), None);
    assert_eq!(sun_disc_preimage(setup.dir_map, setup.sun, 0.0), None);
    assert_eq!(sun_disc_preimage(setup.dir_map, setup.sun, f64::NAN), None);
    // A sun far off to the side of the picture: the disc is not in the image's field of view
    // as a bounded ellipse on the right nappe, or it is one far away; either way no panic.
    let _ = sun_disc_preimage(setup.dir_map, DVec3::new(1.0, 0.0, 0.0), tan);
}

#[test]
fn sun_disc_preimage_scale_of_the_direction_field_does_not_matter() {
    let setup = sun_setup(DVec2::new(-9.0, 14.0));
    let tan = 3.0_f64.to_radians().tan();
    let a = sun_disc_preimage(setup.dir_map, setup.sun, tan).unwrap();
    let b = sun_disc_preimage(setup.dir_map * 7.5, setup.sun * 0.1, tan).unwrap();
    assert!((a.centre - b.centre).length() < 1e-9);
    assert!((a.area() - b.area()).abs() < 1e-9 * a.area());
}

/// The unit exit direction of the setup's field at the image point `p`.
fn field(setup: &SunSetup, p: DVec2) -> DVec3 {
    (setup.dir_map * p.extend(1.0)).normalize()
}

#[test]
fn jacobian_preimage_agrees_with_the_exact_conic_to_first_order() {
    let setup = sun_setup(DVec2::new(1.5, -0.8));
    let tan = 0.27_f64.to_radians().tan();
    let exact = sun_disc_preimage(setup.dir_map, setup.sun, tan).unwrap();
    // Linearise the normalised field at the image centre.
    let x0 = DVec2::new(320.0, 240.0);
    let v0 = setup.dir_map * x0.extend(1.0);
    let omega0 = v0.normalize();
    let project = |column: DVec3| (column - omega0 * omega0.dot(column)) / v0.length();
    let d_screen_x = project(setup.dir_map.x_axis);
    let d_screen_y = project(setup.dir_map.y_axis);
    let first_order =
        sun_disc_preimage_jacobian(x0, omega0, d_screen_x, d_screen_y, setup.sun, tan).unwrap();
    assert!(
        (first_order.centre - exact.centre).length() < 0.05,
        "{:?} vs {:?}",
        first_order.centre,
        exact.centre
    );
    let ratio = first_order.area() / exact.area();
    assert!((ratio - 1.0).abs() < 1e-2, "area ratio {ratio}");
    // Both contain the point the sun points at, and the linearised field is exactly affine.
    let aimed = exact.centre;
    assert!(first_order.contains(aimed));
    assert!(angle_between(field(&setup, aimed), setup.sun) < 0.27_f64.to_radians());
    // For a truly affine field the Jacobian version is the exact conic.
    let affine = DMat3::from_cols(
        d_screen_x,
        d_screen_y,
        omega0 - d_screen_x * x0.x - d_screen_y * x0.y,
    );
    let direct = sun_disc_preimage(affine, setup.sun, tan).unwrap();
    assert_eq!(first_order, direct);
}

#[test]
fn image_ellipse_contains_and_area_agree() {
    let ellipse = ImageEllipse {
        centre: DVec2::new(10.0, -5.0),
        axes: DMat2::from_cols(DVec2::new(3.0, 1.0), DVec2::new(-0.5, 2.0)),
    };
    assert!(ellipse.contains(ellipse.centre));
    assert!(ellipse.contains(ellipse.centre + ellipse.axes.x_axis * 0.999));
    assert!(!ellipse.contains(ellipse.centre + ellipse.axes.y_axis * 1.001));
    assert!(
        PI.mul_add(-0.5_f64.mul_add(1.0, 3.0 * 2.0), ellipse.area())
            .abs()
            < 1e-12
    );
}
