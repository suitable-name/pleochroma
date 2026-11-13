//! Fragment lighting and spectral node integration (beam guide §11.3, §11.4, §16.1).

use std::sync::atomic::{AtomicBool, Ordering};

use glam::{DVec2, DVec3, Vec3};

use super::{
    beam::{BranchPrefix, GROUP, Leaf, SpectralInterval},
    geom::polygon::ConvexPolygon,
    lighting::{
        FragmentInput, LightingScene, RADON_7, integrate_fragment, integrate_fragment_cancellable,
        integrate_fragment_counted, sigma_l, sun_disc_term, tau_omega,
    },
};
use crate::{
    optics::{
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, color::integrate_nodes_to_xyz,
            environment::sample_environment_channels, integrate_channels_to_xyz, studio_spec_power,
        },
        studio_rig::StudioRig,
    },
    renderer::env_map::EnvironmentMap,
};

/// A coarse frame: a pixel spans about 0.65 degrees, so the sun disc (radius 0.27 degrees) is
/// smaller than a pixel and a texel of a 64-wide map is a few pixels.
const COARSE: u16 = 64;
/// A fine frame: a pixel spans about 0.02 degrees, the footprint the guide assumes (§11.4).
const FINE: u16 = 2048;
/// `τ_L` of the test scenes (the guide's default).
const TAU_L: f32 = 1e-3;
/// Relative tolerance of `f32` sums of a few terms.
const F32_REL: f64 = 1e-5;
/// Relative tolerance of the sun-disc term against the dense point-sampled reference (the
/// reference lattice itself resolves the disc to about 3e-5).
const SUN_REL_TOL: f64 = 2e-4;
/// Points per axis of the dense sun-disc reference.
const SUN_GRID: u32 = 3000;
/// Monte Carlo points of the centroid-rule reference.
const MC_POINTS: u32 = 20_000;

/// The camera of every test frame.
fn camera() -> Camera {
    Camera::new(0.6, 0.45, 3.0, 40.0)
}

/// An image-space `f64` as the tracer's `f32`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "test coordinates are far inside the f32 range"
)]
const fn f32_of(v: f64) -> f32 {
    v as f32
}

/// A mirror-reflected camera field: the exit direction of an image point is the camera
/// direction `forward + right·u + up·v` reflected in the plane with the unit `mirror` normal,
/// affine in the image like a reflection leaf's.
struct Geo {
    forward: DVec3,
    right: DVec3,
    up: DVec3,
    fov_tan: f64,
    mirror: DVec3,
    size: u16,
}

impl Geo {
    /// A field whose centre pixel exits along `target`.
    fn towards(cam: &Camera, target: Vec3, size: u16) -> Self {
        let forward = cam.forward.as_dvec3();
        Self {
            forward,
            right: cam.right.as_dvec3(),
            up: cam.up.as_dvec3(),
            fov_tan: f64::from(cam.fov_tan),
            mirror: (forward - target.as_dvec3()).normalize(),
            size,
        }
    }

    /// The image coordinate of the frame centre.
    fn centre(&self) -> f64 {
        f64::from(self.size) / 2.0
    }

    /// The unnormalised exit direction at the image point `p`.
    fn direction(&self, p: DVec2) -> DVec3 {
        let size = f64::from(self.size);
        let u = (p.x / size - 0.5) * 2.0 * self.fov_tan;
        let v = (0.5 - p.y / size) * 2.0 * self.fov_tan;
        let d = self.forward + self.right * u + self.up * v;
        d - self.mirror * (2.0 * d.dot(self.mirror))
    }
}

/// A reflection leaf over `corners` with `nodes` nodes per vertex, weights `weight(p)`.
fn reflection_leaf(
    geo: &Geo,
    corners: &[DVec2],
    weight: impl Fn(DVec2) -> f64,
    nodes: usize,
    glare: bool,
) -> Leaf {
    let image = ConvexPolygon::from_image_points(corners).expect("a convex leaf polygon");
    let mut dirs = Vec::new();
    let mut weights = Vec::new();
    for v in image.vertices() {
        let p = DVec2::new(v.x, v.y);
        for _ in 0..nodes {
            dirs.push(geo.direction(p).as_vec3());
            weights.push(f32_of(weight(p)));
        }
    }
    Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: SpectralInterval {
            first: 0,
            last: u16::try_from(nodes - 1).expect("few nodes"),
        },
        sig: [0, 0],
        prefix: BranchPrefix::EMPTY,
        glare,
        reflection: true,
        tau_omega: 1e-4,
        pre_clip: None,
    }
}

/// The scene of a `size` frame lit by `environment`.
fn scene<'a>(environment: EnvironmentSource<'a>, cam: &'a Camera, size: u16) -> LightingScene<'a> {
    let rig = match environment {
        EnvironmentSource::Studio {
            light_yaw,
            light_pitch,
            ..
        } => Some(StudioRig::new(light_yaw, light_pitch)),
        EnvironmentSource::HdrMap { .. } => None,
    };
    LightingScene {
        environment,
        rig,
        camera: cam,
        size: (u32::from(size), u32::from(size)),
        tolerance: TAU_L,
        max_levels: 6,
        n_lambda: 8,
    }
}

/// A rectangle given by its corner offsets from the point `(c, c)`.
fn around(c: f64, offsets: &[(f64, f64)]) -> Vec<DVec2> {
    offsets
        .iter()
        .map(|&(dx, dy)| DVec2::new(c + dx, c + dy))
        .collect()
}

/// The leaf square, which contains every test fragment.
fn leaf_square(c: f64) -> Vec<DVec2> {
    around(c, &[(-4.0, -4.0), (4.0, -4.0), (4.0, 4.0), (-4.0, 4.0)])
}

/// The convex quadrilateral of one pixel around the frame centre.
fn pixel_quad(c: f64) -> Vec<DVec2> {
    around(c, &[(-0.4, -0.5), (0.5, -0.4), (0.4, 0.5), (-0.5, 0.4)])
}

/// A unit vector in the upper hemisphere away from the head shadow of the test camera.
fn upper_target() -> Vec3 {
    Vec3::new(0.3, 0.8, 0.2).normalize()
}

/// A unit vector where `DaylightDome` at yaw 0.4, pitch 0.9 is smooth on the scale of one pixel:
/// 60 degrees up, opposite the key in azimuth, so the `0.30 cos^8` aureole is below `1e-4` and
/// flat, and the sky's slope over a pixel is about `2e-4` of its value. At [`upper_target`] the
/// key is 22 degrees away: the aureole's slope alone moves the radiance by about `1e-3` across
/// the pixel, which with the leaf's weight slope exceeds `TAU_L` and splits the fragment.
fn smooth_dome_target() -> Vec3 {
    let (yaw, elevation) = (0.4_f32 + std::f32::consts::PI, 60.0_f32.to_radians());
    Vec3::new(
        elevation.cos() * yaw.sin(),
        elevation.sin(),
        elevation.cos() * yaw.cos(),
    )
    .normalize()
}

/// The polygon of `points`.
fn polygon(points: &[DVec2]) -> ConvexPolygon {
    ConvexPolygon::from_image_points(points).expect("a convex fragment polygon")
}

/// `|a − b| ≤ rel · |b|`.
fn assert_close(a: f64, b: f64, rel: f64, what: &str) {
    assert!(
        (a - b).abs() <= rel * b.abs().max(f64::MIN_POSITIVE),
        "{what}: {a} vs {b}"
    );
}

/// A deterministic uniform `[0, 1)` stream (xorshift64, 32 bits used).
struct Stream(u64);

impl Stream {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        let bits = u32::try_from(self.0 >> 32).unwrap_or(0);
        f64::from(bits) / 4_294_967_296.0
    }
}

/// Whether `q` lies inside the convex polygon `poly` (either winding).
fn inside(poly: &[DVec2], q: DVec2) -> bool {
    let mut positive = false;
    let mut negative = false;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let cross = (b - a).perp_dot(q - a);
        positive |= cross > 0.0;
        negative |= cross < 0.0;
    }
    !(positive && negative)
}

#[test]
fn node_integration_matches_the_plain_integration_of_a_non_dispersive_sample() {
    let lambdas = [405.0, 455.0, 505.0, 555.0, 605.0, 655.0, 705.0, 755.0];
    let radiance = [0.4, 0.9, 1.3, 2.0, 1.1, 0.7, 0.3, 0.1];
    let plain = integrate_channels_to_xyz(&radiance, &lambdas, &[1.0; 8], 0);
    let nodes = integrate_nodes_to_xyz(&radiance, 0, 8);
    for (a, b, what) in [
        (nodes.x, plain.x, "X"),
        (nodes.y, plain.y, "Y"),
        (nodes.z, plain.z, "Z"),
    ] {
        assert_close(f64::from(a), f64::from(b), F32_REL, what);
    }
    // 16 nodes: the same table at the centres of nodes 4..12, half the weight per node.
    let lambdas = [492.5, 517.5, 542.5, 567.5, 592.5, 617.5, 642.5, 667.5];
    let plain = integrate_channels_to_xyz(&radiance, &lambdas, &[1.0; 8], 0);
    let nodes = integrate_nodes_to_xyz(&radiance, 4, 16);
    for (a, b, what) in [
        (nodes.x, 0.5 * plain.x, "X16"),
        (nodes.y, 0.5 * plain.y, "Y16"),
        (nodes.z, 0.5 * plain.z, "Z16"),
    ] {
        assert_close(f64::from(a), f64::from(b), F32_REL, what);
    }
}

#[test]
fn a_constant_radiance_environment_integrates_to_area_times_value() {
    let cam = camera();
    let map = EnvironmentMap::uniform(64, 32, [1.0, 0.5, 0.25]);
    let scene = scene(EnvironmentSource::hdr(&map), &cam, COARSE);
    let geo = Geo::towards(&cam, upper_target(), COARSE);
    let c = geo.centre();
    let leaf = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, GROUP, false);
    let fragment = polygon(&around(
        c,
        &[
            (-2.8, -2.9),
            (1.7, -2.5),
            (2.1, 1.2),
            (-1.0, 2.4),
            (-3.0, 0.0),
        ],
    ));
    let input = FragmentInput {
        leaf: &leaf,
        polygon: &fragment,
        group: 0,
    };
    let lambdas = leaf.spectrum.group_lambdas(0, 8);
    let expected =
        sample_environment_channels(scene.environment, upper_target(), &lambdas, None, Vec3::Y);
    let (got, work) = integrate_fragment_counted(&scene, &input);
    assert!(
        work.deepest >= 1,
        "a fragment wider than a half texel is split"
    );
    for (g, e) in got.iter().zip(&expected) {
        assert_close(
            f64::from(*g),
            fragment.area() * f64::from(*e),
            F32_REL,
            "area times value",
        );
    }
}

#[test]
fn the_cancellable_integral_equals_the_plain_one_and_stops_on_a_set_token() {
    let cam = camera();
    let map = EnvironmentMap::uniform(64, 32, [1.0, 0.5, 0.25]);
    let scene = scene(EnvironmentSource::hdr(&map), &cam, COARSE);
    let geo = Geo::towards(&cam, upper_target(), COARSE);
    let c = geo.centre();
    let leaf = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, GROUP, false);
    let fragment = polygon(&around(
        c,
        &[(-2.8, -2.9), (1.7, -2.5), (2.1, 1.2), (-3.0, 0.0)],
    ));
    let input = FragmentInput {
        leaf: &leaf,
        polygon: &fragment,
        group: 0,
    };
    let token = AtomicBool::new(false);
    let free = integrate_fragment_cancellable(&scene, &input, &token).expect("not cancelled");
    assert_eq!(free, integrate_fragment(&scene, &input), "bit for bit");
    token.store(true, Ordering::Relaxed);
    assert!(
        integrate_fragment_cancellable(&scene, &input, &token).is_none(),
        "a set token stops the integral"
    );
}

#[test]
fn the_centroid_rule_agrees_with_a_dense_monte_carlo_reference_on_a_smooth_fragment() {
    let cam = camera();
    let env = LightingPreset::DaylightDome.studio(1.0, 0.4, 0.9);
    let scene = scene(env, &cam, FINE);
    let geo = Geo::towards(&cam, smooth_dome_target(), FINE);
    let c = geo.centre();
    let weight = |p: DVec2| 0.0002_f64.mul_add(p.y - c, 0.0001_f64.mul_add(p.x - c, 0.6));
    let leaf = reflection_leaf(&geo, &leaf_square(c), weight, GROUP, false);
    let quad = pixel_quad(c);
    let fragment = polygon(&quad);
    let input = FragmentInput {
        leaf: &leaf,
        polygon: &fragment,
        group: 0,
    };
    let (got, work) = integrate_fragment_counted(&scene, &input);
    assert_eq!(
        work.centroid_pieces, 1,
        "the pixel is smooth: one centroid piece"
    );
    assert_eq!(work.deepest, 0);

    let lambdas = leaf.spectrum.group_lambdas(0, 8);
    let side = f32_of(f64::from(FINE));
    let lo = DVec2::new(c - 0.5, c - 0.5);
    let mut stream = Stream(0x9E37_79B9_7F4A_7C15);
    let mut sum = [0.0_f64; GROUP];
    let mut sum2 = [0.0_f64; GROUP];
    let mut count = 0_u32;
    while count < MC_POINTS {
        let p = lo + DVec2::new(stream.uniform(), stream.uniform());
        if !inside(&quad, p) {
            continue;
        }
        count += 1;
        let dir = geo.direction(p).as_vec3().normalize();
        let ray = cam.generate_ray(f32_of(p.x), f32_of(p.y), side, side, 0.0, 0.0);
        let radiance =
            sample_environment_channels(env, dir, &lambdas, scene.rig.as_ref(), -ray.dir);
        for ((s, s2), r) in sum.iter_mut().zip(sum2.iter_mut()).zip(&radiance) {
            let f = weight(p) * f64::from(*r);
            *s += f;
            *s2 = f.mul_add(f, *s2);
        }
    }
    let taken = f64::from(count);
    let area = fragment.area();
    for (k, ((g, s), s2)) in got.iter().zip(&sum).zip(&sum2).enumerate() {
        let mean = s / taken;
        let mean_sq = s2 / taken;
        let var = mean.mul_add(-mean, mean_sq).max(0.0);
        let stderr = area * (var / taken).sqrt();
        let reference = area * mean;
        let diff = (f64::from(*g) - reference).abs();
        assert!(
            diff <= f64::from(TAU_L).mul_add(reference, 4.0 * stderr),
            "slot {k}: {g} vs {reference} (z = {})",
            diff / stderr.max(f64::MIN_POSITIVE)
        );
    }
}

#[test]
fn the_seven_point_rule_is_exact_for_quadratics() {
    let (a, b, c) = (
        DVec2::new(0.3, -0.2),
        DVec2::new(2.1, 0.4),
        DVec2::new(0.8, 1.9),
    );
    let area = 0.5 * (b - a).perp_dot(c - a).abs();
    let weight_sum: f64 = RADON_7.iter().map(|(_, w)| w).sum();
    assert_close(weight_sum, 1.0, 1e-14, "weights");
    let integrate = |f: &dyn Fn(DVec2) -> f64| -> f64 {
        RADON_7
            .iter()
            .map(|(w, weight)| {
                assert_close(w[0] + w[1] + w[2], 1.0, 1e-14, "barycentric sum");
                weight * f(a * w[0] + b * w[1] + c * w[2])
            })
            .sum::<f64>()
            * area
    };
    let (sx, sy) = (a.x + b.x + c.x, a.y + b.y + c.y);
    let sxx = c.x.mul_add(c.x, b.x.mul_add(b.x, a.x * a.x));
    let syy = c.y.mul_add(c.y, b.y.mul_add(b.y, a.y * a.y));
    let sxy = c.x.mul_add(c.y, b.x.mul_add(b.y, a.x * a.y));
    assert_close(integrate(&|_| 1.0), area, 1e-13, "1");
    assert_close(integrate(&|p| p.x), area * sx / 3.0, 1e-13, "x");
    assert_close(integrate(&|p| p.y), area * sy / 3.0, 1e-13, "y");
    assert_close(
        integrate(&|p| p.x * p.x),
        area / 12.0 * sx.mul_add(sx, sxx),
        1e-13,
        "x^2",
    );
    assert_close(
        integrate(&|p| p.y * p.y),
        area / 12.0 * sy.mul_add(sy, syy),
        1e-13,
        "y^2",
    );
    assert_close(
        integrate(&|p| p.x * p.y),
        area / 12.0 * sx.mul_add(sy, sxy),
        1e-13,
        "xy",
    );
}

/// The area of `quad` covered by the sun disc, by a dense lattice of exact `f64` direction tests.
fn reference_covered_area(geo: &Geo, key: DVec3, quad: &[DVec2]) -> f64 {
    let cos = f64::from(0.999_988_9_f32);
    let (mut lo, mut hi) = (quad[0], quad[0]);
    for q in quad {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    let step = (hi - lo) / f64::from(SUN_GRID);
    let mut count = 0_u32;
    for iy in 0..SUN_GRID {
        let y = (f64::from(iy) + 0.5).mul_add(step.y, lo.y);
        for ix in 0..SUN_GRID {
            let p = DVec2::new((f64::from(ix) + 0.5).mul_add(step.x, lo.x), y);
            if inside(quad, p) && geo.direction(p).normalize().dot(key) >= cos {
                count += 1;
            }
        }
    }
    f64::from(count) * step.x * step.y
}

#[test]
fn the_sun_disc_term_matches_a_dense_point_sampled_reference_on_fragments_that_cut_the_disc() {
    let cam = camera();
    let env = LightingPreset::DaylightSun.studio(1.0, 0.5, 1.2);
    let scene = scene(env, &cam, COARSE);
    let rig = scene.rig.expect("a studio rig");
    let geo = Geo::towards(&cam, rig.key_dir, COARSE);
    let c = geo.centre();
    let key = rig.key_dir.as_dvec3().normalize();
    let leaf = reflection_leaf(&geo, &leaf_square(c), |_| 0.8, GROUP, false);
    let lambdas = leaf.spectrum.group_lambdas(0, 8);
    let cut = around(c, &[(-0.2, -0.5), (0.6, -0.4), (0.5, 0.3), (-0.3, 0.2)]);
    let whole = around(c, &[(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]);
    let full_area = reference_covered_area(&geo, key, &whole);
    let cut_area = reference_covered_area(&geo, key, &cut);
    assert!(cut_area > 0.0);
    assert!(
        cut_area < 0.9 * full_area,
        "the cut fragment covers part of the disc"
    );
    for (quad, covered) in [(&cut, cut_area), (&whole, full_area)] {
        let fragment = polygon(quad);
        let input = FragmentInput {
            leaf: &leaf,
            polygon: &fragment,
            group: 0,
        };
        let got = sun_disc_term(&scene, &input);
        for (g, &lambda) in got.iter().zip(&lambdas) {
            let radiance =
                40_000.0 * f64::from(studio_spec_power(LightingPreset::DaylightSun, lambda));
            assert_close(
                f64::from(*g),
                0.8 * radiance * covered,
                SUN_REL_TOL,
                "sun disc term",
            );
        }
    }
    // The whole-fragment integral contains the disc term on top of the sky.
    let fragment = polygon(&whole);
    let input = FragmentInput {
        leaf: &leaf,
        polygon: &fragment,
        group: 0,
    };
    let total = integrate_fragment(&scene, &input);
    let sun = sun_disc_term(&scene, &input);
    for (t, s) in total.iter().zip(&sun) {
        assert!(*t > *s, "the sky adds to the disc");
    }
}

#[test]
fn the_sun_constants_match_the_tracers_jump_at_the_disc_edge() {
    let cam = camera();
    let env = LightingPreset::DaylightSun.studio(1.0, 0.5, 1.2);
    let rig = scene(env, &cam, COARSE).rig.expect("a studio rig");
    let key = rig.key_dir;
    let radius = 0.999_988_9_f32.acos();
    let tangent = key.any_orthonormal_vector();
    let at = |angle: f32| key * angle.cos() + tangent * angle.sin();
    let look = |d: Vec3| sample_environment_channels(env, d, &[550.0], Some(&rig), Vec3::Y)[0];
    let lit = look(at(0.98 * radius));
    let dark = look(at(1.02 * radius));
    let expected = 40_000.0 * studio_spec_power(LightingPreset::DaylightSun, 550.0);
    assert_close(
        f64::from(lit - dark),
        f64::from(expected),
        1e-3,
        "the sun radiance",
    );
}

#[test]
fn surface_glare_is_applied_only_with_the_glare_flag() {
    let cam = camera();
    let base = LightingPreset::DaylightDome.studio(1.0, 0.4, 0.9);
    let dim = base.with_surface_glare(0.25);
    let geo = Geo::towards(&cam, upper_target(), FINE);
    let c = geo.centre();
    let fragment = polygon(&pixel_quad(c));
    let run = |env: EnvironmentSource<'_>, glare: bool| {
        let leaf = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, GROUP, glare);
        let scene = scene(env, &cam, FINE);
        integrate_fragment(
            &scene,
            &FragmentInput {
                leaf: &leaf,
                polygon: &fragment,
                group: 0,
            },
        )
    };
    let plain = run(base, false);
    let flagged = run(dim, true);
    let unflagged = run(dim, false);
    assert_eq!(unflagged, plain, "no glare flag: the scale is not applied");
    assert_eq!(run(base, true), plain, "glare 1.0 is the identity");
    for (k, (f, p)) in flagged.iter().zip(&plain).enumerate() {
        assert!(*p > 0.0, "slot {k} is lit");
        assert_close(f64::from(*f), 0.25 * f64::from(*p), F32_REL, "glare scale");
    }
}

#[test]
fn hdr_subdivision_depth_is_bounded_by_half_a_texel() {
    let cam = camera();
    let uniform = EnvironmentMap::uniform(64, 32, [1.0; 3]);
    let sigma = f64::from(sigma_l(&EnvironmentSource::hdr(&uniform)));
    let per_pixel = 2.0 * f64::from(cam.fov_tan) / f64::from(COARSE);
    let side = 4.0;
    let extent = side * std::f64::consts::SQRT_2 * per_pixel;
    let halvings = std::iter::successors(Some(extent), |e| Some(*e * 0.5))
        .take_while(|&e| e > sigma)
        .count();
    let levels = u32::try_from(halvings).expect("a handful of halvings");
    let texels: Vec<[f32; 3]> = (0..64_u32 * 32)
        .map(|i| {
            if (i % 64 + i / 64) % 2 == 0 {
                [0.1; 3]
            } else {
                [2.0; 3]
            }
        })
        .collect();
    let checker = EnvironmentMap::from_rgb(64, 32, texels).expect("a 64 x 32 map");
    let geo = Geo::towards(&cam, upper_target(), COARSE);
    let c = geo.centre();
    let leaf = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, GROUP, false);
    let fragment = polygon(&around(
        c,
        &[(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)],
    ));
    for (map, must_split) in [(&uniform, true), (&checker, false)] {
        let scene = scene(EnvironmentSource::hdr(map), &cam, COARSE);
        let (_, work) = integrate_fragment_counted(
            &scene,
            &FragmentInput {
                leaf: &leaf,
                polygon: &fragment,
                group: 0,
            },
        );
        assert!(
            work.deepest <= levels + 2,
            "depth {} exceeds the half-texel bound {}",
            work.deepest,
            levels + 2
        );
        assert!(work.deepest <= scene.max_levels);
        if must_split {
            assert!(
                work.deepest >= 1,
                "a fragment wider than a half texel is split"
            );
        }
    }
}

#[test]
fn sigma_l_follows_the_guide_table() {
    let deg = f32::to_radians;
    for (preset, expected) in [
        (LightingPreset::RingLights, deg(1.0)),
        (LightingPreset::Daylight, deg(1.0)),
        (LightingPreset::LightTent, deg(0.5)),
        (LightingPreset::DaylightSun, deg(0.1)),
        (LightingPreset::IsoHemisphere, deg(2.0)),
        (LightingPreset::DaylightDome, deg(2.0)),
        (LightingPreset::Aset, deg(2.0)),
    ] {
        let env = preset.studio(1.0, 0.5, 0.9);
        assert_close(
            f64::from(sigma_l(&env)),
            f64::from(expected),
            1e-6,
            "sigma_l",
        );
        assert_close(
            f64::from(tau_omega(&env)),
            f64::from((expected / 20.0).min(1e-4)),
            1e-6,
            "tau_omega",
        );
    }
    let map = EnvironmentMap::uniform(4096, 2048, [1.0; 3]);
    let hdr = EnvironmentSource::hdr(&map);
    assert_close(
        f64::from(sigma_l(&hdr)),
        std::f64::consts::PI / 4096.0,
        1e-6,
        "half a texel",
    );
    assert!(tau_omega(&hdr) < 1e-4);
}

#[test]
fn slots_past_the_intervals_last_node_are_zero() {
    let cam = camera();
    let map = EnvironmentMap::uniform(64, 32, [1.0, 0.8, 0.6]);
    let mut scene = scene(EnvironmentSource::hdr(&map), &cam, FINE);
    scene.n_lambda = 16;
    let geo = Geo::towards(&cam, upper_target(), FINE);
    let c = geo.centre();
    let fragment = polygon(&pixel_quad(c));
    let first = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, 3, false);
    let got = integrate_fragment(
        &scene,
        &FragmentInput {
            leaf: &first,
            polygon: &fragment,
            group: 0,
        },
    );
    assert!(got[..3].iter().all(|v| *v > 0.0));
    assert_eq!(got[3..], [0.0; 5]);
    let second = reflection_leaf(&geo, &leaf_square(c), |_| 1.0, 11, false);
    let got = integrate_fragment(
        &scene,
        &FragmentInput {
            leaf: &second,
            polygon: &fragment,
            group: 1,
        },
    );
    assert!(got[..3].iter().all(|v| *v > 0.0));
    assert_eq!(got[3..], [0.0; 5]);
    let none = integrate_fragment(
        &scene,
        &FragmentInput {
            leaf: &second,
            polygon: &fragment,
            group: 2,
        },
    );
    assert_eq!(none, [0.0; 8]);
}

#[test]
fn the_head_shadow_edge_forces_subdivision_and_a_far_exit_does_not() {
    let cam = camera();
    let env = LightingPreset::DaylightDome.studio(1.0, 0.4, 0.9);
    let scene = scene(env, &cam, FINE);
    let centre = f32_of(f64::from(FINE) / 2.0);
    let side = f32_of(f64::from(FINE));
    let observer = -cam.generate_ray(centre, centre, side, side, 0.0, 0.0).dir;
    let tangent = observer.any_orthonormal_vector();
    let angle = 16.0_f32.to_radians();
    let on_edge = observer * angle.cos() + tangent * angle.sin();
    let geo_centre = f64::from(FINE) / 2.0;
    let fragment = polygon(&pixel_quad(geo_centre));
    let run = |target: Vec3| {
        let geo = Geo::towards(&cam, target, FINE);
        let leaf = reflection_leaf(&geo, &leaf_square(geo_centre), |_| 1.0, GROUP, false);
        integrate_fragment_counted(
            &scene,
            &FragmentInput {
                leaf: &leaf,
                polygon: &fragment,
                group: 0,
            },
        )
        .1
    };
    assert!(
        run(on_edge).deepest >= 1,
        "the head-shadow transition is not smooth"
    );
    let far = run(upper_target());
    assert_eq!(far.deepest, 0);
    assert_eq!(far.centroid_pieces, 1);
}

/// A refracted (non-reflection) leaf over `corners` whose every vertex exits along `dir` with
/// weight `weight`.
fn refracted_leaf(corners: &[DVec2], dir: Vec3, weight: f32) -> Leaf {
    let image = ConvexPolygon::from_image_points(corners).expect("a convex leaf polygon");
    let count = image.vertices().len() * GROUP;
    Leaf {
        tile: 0,
        image,
        dirs: vec![dir; count],
        weights: vec![weight; count],
        spectrum: SpectralInterval {
            first: 0,
            last: u16::try_from(GROUP - 1).expect("few nodes"),
        },
        sig: [0, 0],
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1e-4,
        pre_clip: None,
    }
}

#[test]
fn a_tiny_transmitted_fragment_inside_the_sun_disc_keeps_its_sunlight() {
    // A fragment wholly inside the sun disc keeps its sunlight even when its
    // sides are below 3e-4 px, so the affine fit must accept polygons that small.
    let cam = camera();
    let env = LightingPreset::DaylightSun.studio(1.0, 0.5, 1.2);
    let scene = scene(env, &cam, COARSE);
    let rig = scene.rig.expect("a studio rig");
    let c = f64::from(COARSE) / 2.0;
    for half in [5e-5, 5e-7] {
        let corners = around(
            c,
            &[(-half, -half), (half, -half), (half, half), (-half, half)],
        );
        let leaf = refracted_leaf(&corners, rig.key_dir, 0.8);
        let fragment = polygon(&corners);
        let lambdas = leaf.spectrum.group_lambdas(0, 8);
        let input = FragmentInput {
            leaf: &leaf,
            polygon: &fragment,
            group: 0,
        };
        let got = sun_disc_term(&scene, &input);
        let area = fragment.area();
        for (g, &lambda) in got.iter().zip(&lambdas) {
            let radiance =
                40_000.0 * f64::from(studio_spec_power(LightingPreset::DaylightSun, lambda));
            assert_close(
                f64::from(*g),
                0.8 * radiance * area,
                SUN_REL_TOL,
                "tiny transmitted sun term",
            );
        }
    }
}
