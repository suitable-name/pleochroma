//! CPU checks of the tray reflection split at the chain's escape sites (no GPU needed). The
//! worklist-overflow fallback is tested beside `Worklist` in `chain.rs`.

use glam::Vec3;

use super::{
    WhittedEnv, WhittedPrep,
    chain::{trace_chain, trace_chain_with_cutoff},
    fresnel::fresnel_unpolarised_f32,
    tests_shadow::{EXPOSURE, PITCH, RATIOS, TRAY, YAW, bits, lit, map},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, Ray, build_plane_soa,
            environment::TrayMirror,
        },
    },
    renderer::{engine::RayOptions, frame_scene::FrameScene},
};

const OBSERVER: Vec3 = Vec3::Y;
/// A diamond-like index; the cube's reflectance at the test angle is well above a percent.
const N: f32 = 2.417;

/// The largest per-channel difference, relative to the larger magnitude (at least 1).
fn distance(a: Vec3, b: Vec3) -> f32 {
    (a - b).abs().max_element() / a.abs().max_element().max(b.abs().max_element()).max(1.0)
}

/// A cube of half-width 0.5 about the origin.
fn cube() -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .map(|normal| GpuFacetPlane::new(normal, -0.5))
    .to_vec()
}

/// Schlick reflectance at `cos`, evaluated in f64 independently of the kernel's code.
fn schlick(r0: f32, cos: f32) -> f32 {
    let m = 1.0 - f64::from(cos);
    let r0 = f64::from(r0);
    #[expect(clippy::cast_possible_truncation, reason = "a reflectance in [0, 1]")]
    let f = (1.0 - r0).mul_add(m.powi(5), r0) as f32;
    f
}

/// The chain's radiance for `ray` over `planes`, with `max_bounces` bounding the depth.
fn trace(
    planes: &[GpuFacetPlane],
    environment: EnvironmentSource<'_>,
    max_bounces: u32,
    ray: Ray,
) -> Vec3 {
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let soa = build_plane_soa(planes);
    let scene = FrameScene {
        camera: &camera,
        width: 4,
        height: 4,
        planes,
        facet_finishes: &[],
        material: &material,
        max_bounces,
        environment,
    };
    let prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        RayOptions {
            aa_taps: 1,
            fire: false,
        },
    )
    .expect("prep");
    trace_chain(&scene, &[], &soa, &prep, ray, N, OBSERVER, None)
}

#[test]
fn a_full_mirror_straight_down_reads_the_sky_straight_up() {
    let mirror = TrayMirror::new(-1.0, 1.0).expect("mirror");
    let ray = Ray {
        origin: Vec3::new(0.0, 1.0, 0.0),
        dir: Vec3::NEG_Y,
    };
    // The eye looks along the sky direction here, so the default head shadow would darken it.
    let source = || lit(TRAY).with_head_shadow(0.0);
    let actual = trace(&[], source().with_tray_mirror(Some(mirror)), 12, ray);
    let sky = WhittedEnv::new(source()).leaf(Vec3::Y, OBSERVER);
    assert!(sky.max_element() > 0.0);
    assert!(distance(actual, sky) < 1e-6, "{actual:?} vs {sky:?}");
}

#[test]
fn a_four_percent_tray_mixes_paper_and_reflection_by_schlick() {
    let mirror = TrayMirror::new(-1.0, 0.04).expect("mirror");
    let dir = Vec3::new(0.6, -0.5, 0.2).normalize();
    let ray = Ray {
        origin: Vec3::new(0.0, 1.0, 0.0),
        dir,
    };
    let actual = trace(&[], lit(TRAY).with_tray_mirror(Some(mirror)), 12, ray);
    let env = WhittedEnv::new(lit(TRAY));
    let fresnel = schlick(0.04, -dir.y);
    assert!(fresnel > 0.04 && fresnel < 1.0);
    let reflected = Vec3::new(dir.x, -dir.y, dir.z);
    let expected = (1.0 - fresnel) * env.leaf_at(ray.origin, dir, OBSERVER)
        + fresnel * env.leaf_at(ray.origin, reflected, OBSERVER);
    assert!(
        distance(actual, expected) < 1e-5,
        "{actual:?} vs {expected:?}"
    );
    // The reflection really contributes: the matte tray alone is a different color.
    let matte = trace(&[], lit(TRAY), 12, ray);
    assert!(distance(actual, matte) > 1e-4);
}

#[test]
fn the_reflected_branch_reaches_the_stone() {
    // The ray lands on the tray beside the cube (x = 0.8), so it never meets the stone on the
    // way down; its reflection climbs back into the cube's +x face.
    let planes = cube();
    let mirror = TrayMirror::new(-0.501, 0.5).expect("mirror");
    let origin = Vec3::new(2.0, 0.0, 0.0);
    let dir = (Vec3::new(0.8, -0.501, 0.0) - origin).normalize();
    let ray = Ray { origin, dir };
    let floor = mirror.hit(origin, dir).expect("floor");
    assert!(floor.point.x > 0.5);
    let env = WhittedEnv::new(lit(TRAY));
    let down = env.leaf_at(origin, dir, OBSERVER);
    let up = env.leaf_at(floor.point, floor.reflected, OBSERVER);
    assert!(up.element_sum() > 0.0);
    // Depth 1: the reflected branch is traced, but the stone's refracted children (depth 2) are
    // dropped. What remains is the paper plus the stone's surface reflection of the mirrored sky
    // off the +x face (cosine `|reflected.x|`, index ratio `1 / N`), leaving with x flipped.
    let actual = trace(&planes, lit(TRAY).with_tray_mirror(Some(mirror)), 1, ray);
    let off_the_stone = Vec3::new(-floor.reflected.x, floor.reflected.y, 0.0);
    let surface = fresnel_unpolarised_f32(floor.reflected.x.abs(), 1.0 / N);
    let reflection = floor.fresnel * surface * env.leaf_at(floor.point, off_the_stone, OBSERVER);
    assert!(reflection.max_element() > 1e-4, "{reflection:?}");
    let expected = (1.0 - floor.fresnel) * down + reflection;
    assert!(
        distance(actual, expected) < 1e-5,
        "{actual:?} vs {expected:?}"
    );
}

#[test]
fn a_shadowed_tray_and_a_mirror_work_together() {
    // No stone: the ray lands on the shadow map's plane. The paper part reads the shadowed
    // tray, the reflected part (upward) never reads the map.
    let shadow = map(RATIOS);
    let mirror = TrayMirror::new(-0.9, 0.5).expect("mirror");
    let origin = Vec3::new(0.0, 1.0, 0.0);
    let dir = Vec3::new(0.3, -0.5, 0.1).normalize();
    let ray = Ray { origin, dir };
    let floor = mirror.hit(origin, dir).expect("floor");
    let source = lit(TRAY).with_tray_shadow(Some(&shadow));
    let actual = trace(&[], source.with_tray_mirror(Some(mirror)), 12, ray);
    let env = WhittedEnv::new(source);
    let expected = (1.0 - floor.fresnel) * env.leaf_at(origin, dir, OBSERVER)
        + floor.fresnel * env.leaf_at(floor.point, floor.reflected, OBSERVER);
    assert!(
        distance(actual, expected) < 1e-5,
        "{actual:?} vs {expected:?}"
    );
    // The shadow really reaches the result: the unshadowed mirror frame differs.
    let unshadowed = trace(&[], lit(TRAY).with_tray_mirror(Some(mirror)), 12, ray);
    assert!(distance(actual, unshadowed) > 1e-4);
}

#[test]
fn a_mirror_over_a_clear_stone_conserves_energy() {
    // A constant environment of radiance 1 and no absorption: every split sums to one.
    let planes = cube();
    let soa = build_plane_soa(&planes);
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let scene = FrameScene {
        camera: &camera,
        width: 4,
        height: 4,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: lit(TRAY),
    };
    let mirror = TrayMirror::new(-0.501, 0.5).expect("mirror");
    let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::ONE, 64, 1);
    let prep = WhittedPrep {
        env: prep.env.with_mirror(Some(mirror)),
        ..prep
    };
    let origin = Vec3::new(2.0, 0.0, 0.0);
    let dir = (Vec3::new(0.8, -0.501, 0.0) - origin).normalize();
    let sum = trace_chain_with_cutoff(
        &scene,
        &[],
        &soa,
        &prep,
        Ray { origin, dir },
        1.5,
        OBSERVER,
        None,
        0.0,
        None,
    );
    assert!(
        sum.min_element() >= 0.95 && sum.max_element() <= 1.0 + 1e-4,
        "{sum:?}"
    );
}

#[test]
fn without_a_mirror_the_chain_is_the_lit_tray_lookup() {
    let ray = Ray {
        origin: Vec3::new(0.0, 1.0, 0.0),
        dir: Vec3::new(0.6, -0.5, 0.2).normalize(),
    };
    let expected = WhittedEnv::new(lit(TRAY)).leaf_at(ray.origin, ray.dir, OBSERVER);
    let plain = trace(&[], lit(TRAY), 12, ray);
    assert_eq!(bits(plain), bits(expected));
    let none = trace(&[], lit(TRAY).with_tray_mirror(None), 12, ray);
    assert_eq!(bits(none), bits(expected));
    // As Lit has no tray to polish: the source drops the mirror.
    let mirror = TrayMirror::new(-1.0, 0.5).expect("mirror");
    let as_lit = trace(&[], lit(0.0).with_tray_mirror(Some(mirror)), 12, ray);
    let as_lit_expected = WhittedEnv::new(lit(0.0)).leaf_at(ray.origin, ray.dir, OBSERVER);
    assert_eq!(bits(as_lit), bits(as_lit_expected));
    // ASET is a direction-only table with no tray to hit: the sampler drops the mirror too.
    let aset = LightingPreset::Aset
        .studio(EXPOSURE, YAW, PITCH)
        .with_backdrop(TRAY)
        .with_tray_mirror(Some(mirror));
    assert!(WhittedEnv::new(aset).mirror().is_none());
}
