//! The yaw of an HDR map, end to end through the spectral tracer.
//!
//! Three equivalences pin the sense and the arithmetic of the turn, each against a trace that
//! never rotates anything: a map turned by a whole number of texel columns is the map with its
//! columns shifted (per sample, a polished non-scattering stone, so no random decision reads the
//! light); a map turned by `psi` lights a stone and a camera exactly as the unturned map lights
//! the stone and the camera turned by `-psi` (compared by means, for a polished stone and for a
//! scattering one whose Henyey-Greenstein NEE draws toward the turned map); and an unturned
//! source traces bit for bit like the plain one.

use super::super::{
    super::{
        camera::{FacetFinish, Ray},
        environment::{EnvRotation, EnvironmentSource},
        intersect::build_plane_soa,
        sampling::hash_u32,
    },
    trace_spectral_ray_with_finish_soa,
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::materials::GemMaterial,
    renderer::env_map::EnvironmentMap,
};
use glam::Vec3;
use std::f32::consts::TAU;

/// Samples of the per-sample equivalence.
const SAMPLES: u32 = 128;

/// Samples of each mean in the statistical equivalences.
const MEAN_SAMPLES: u32 = 2048;

/// A sky whose radiance changes with the azimuth everywhere (so any wrong turn shows), with a hot
/// patch and a dim, tinted ground.
fn sky() -> EnvironmentMap {
    let (width, height) = (16, 8);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let u = x as f32 / width as f32;
                let v = y as f32 / height as f32;
                if y < height / 2 {
                    let hot: f32 = if (5..8).contains(&x) && y == 2 {
                        6.0
                    } else {
                        1.0
                    };
                    let base = (1.6 * u).mul_add(u, 0.2) * hot;
                    [base, 0.8f32.mul_add(base, 0.1 * v), 1.2 * base]
                } else {
                    [0.12, 0.08, 0.05]
                }
            })
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
}

/// `map` with every row moved `columns` texels toward larger `u` (wrapping).
fn shifted(map: &EnvironmentMap, columns: usize) -> EnvironmentMap {
    let (width, height) = (map.width(), map.height());
    let pixels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (y, x)))
        .map(|(y, x)| map.texels()[y * width + (x + width - columns) % width])
        .collect();
    EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
}

/// The oblique ray at the table the samples are traced along.
fn table_ray() -> Ray {
    Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    }
}

/// The planes with every normal turned into the map frame of `rotation` (a turn by `-yaw`).
fn turned_planes(planes: &[GpuFacetPlane], rotation: EnvRotation) -> Vec<GpuFacetPlane> {
    planes
        .iter()
        .map(|plane| GpuFacetPlane::new(rotation.to_map(Vec3::from_array(plane.normal)), plane.d))
        .collect()
}

/// `ray` turned the same way.
fn turned_ray(ray: Ray, rotation: EnvRotation) -> Ray {
    Ray {
        origin: rotation.to_map(ray.origin),
        dir: rotation.to_map(ray.dir),
    }
}

/// `count` samples of `ray` through `planes`, same seeds every call.
fn traces(
    planes: &[GpuFacetPlane],
    ray: Ray,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    count: u32,
) -> Vec<Vec3> {
    let plane_soa = build_plane_soa(planes);
    let no_finishes: [FacetFinish; 0] = [];
    (0..count)
        .map(|i| {
            let seed = 9100 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            trace_spectral_ray_with_finish_soa(
                ray,
                planes,
                &plane_soa,
                &no_finishes,
                material,
                12,
                environment,
                seed,
                hero_rand,
                None,
            )
        })
        .collect()
}

/// Per component: the mean and the sample variance.
fn moments(samples: &[Vec3]) -> ([f64; 3], [f64; 3]) {
    let n = samples.len() as f64;
    let mean: [f64; 3] =
        std::array::from_fn(|c| samples.iter().map(|s| f64::from(s[c])).sum::<f64>() / n);
    let variance: [f64; 3] = std::array::from_fn(|c| {
        samples
            .iter()
            .map(|s| (f64::from(s[c]) - mean[c]).powi(2))
            .sum::<f64>()
            / (n - 1.0)
    });
    (mean, variance)
}

/// The two sample sets have the same mean to within five standard errors (and a hair of
/// rounding), in every XYZ component.
fn assert_same_mean(label: &str, a: &[Vec3], b: &[Vec3]) {
    let (mean_a, var_a) = moments(a);
    let (mean_b, var_b) = moments(b);
    let n = a.len() as f64;
    let components = mean_a.into_iter().zip(mean_b).zip(var_a).zip(var_b);
    for (c, (((m_a, m_b), v_a), v_b)) in components.enumerate() {
        let sigma = ((v_a + v_b) / n).sqrt();
        let gap = (m_a - m_b).abs();
        assert!(
            gap <= 5.0f64.mul_add(sigma, 1e-6 * m_a.abs()),
            "{label}: component {c} means {m_a} and {m_b} differ by {gap}, {} standard errors",
            gap / sigma.max(1e-30)
        );
    }
}

/// A sample count as an index count.
fn count(samples: u32) -> usize {
    usize::try_from(samples).expect("a small count")
}

/// How many samples of `a` differ from `b` by more than `relative` of the larger component.
fn differing(a: &[Vec3], b: &[Vec3], relative: f32) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| {
            let scale = y.abs().max_element().max(1e-3);
            (**x - **y).abs().max_element() > relative * scale
        })
        .count()
}

/// A map turned by whole texel columns is the map with its columns shifted, sample by sample.
#[test]
fn a_whole_column_turn_equals_shifted_columns() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let map = sky();
    let columns = 5;
    let yaw = TAU * columns as f32 / map.width() as f32;
    let turned = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
    let shifted_map = shifted(&map, columns);
    let a = traces(&planes, table_ray(), &diamond, turned, SAMPLES);
    let b = traces(
        &planes,
        table_ray(),
        &diamond,
        EnvironmentSource::hdr(&shifted_map),
        SAMPLES,
    );
    let outliers = differing(&a, &b, 1e-4);
    assert!(
        outliers <= count(SAMPLES / 50),
        "{outliers} of {SAMPLES} samples differ between the turned map and the shifted columns"
    );
    let plain = traces(
        &planes,
        table_ray(),
        &diamond,
        EnvironmentSource::hdr(&map),
        SAMPLES,
    );
    assert!(
        differing(&plain, &b, 1e-3) > count(SAMPLES / 10),
        "the fixture must show the turn: the unturned map renders differently"
    );
}

/// The map turned by `psi` lights the stone as the unturned map lights the stone and camera
/// turned by `-psi`.
fn assert_turned_map_equals_turned_stone(label: &str, material: &GemMaterial) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let map = sky();
    let rotation = EnvRotation::from_yaw(1.1);
    let turned = EnvironmentSource::hdr(&map).with_hdr_rotation(rotation);
    let a = traces(&planes, table_ray(), material, turned, MEAN_SAMPLES);
    let b = traces(
        &turned_planes(&planes, rotation),
        turned_ray(table_ray(), rotation),
        material,
        EnvironmentSource::hdr(&map),
        MEAN_SAMPLES,
    );
    assert_same_mean(label, &a, &b);
    let plain = traces(
        &planes,
        table_ray(),
        material,
        EnvironmentSource::hdr(&map),
        MEAN_SAMPLES,
    );
    assert!(
        differing(&plain, &a, 1e-3) > count(MEAN_SAMPLES / 10),
        "{label}: the fixture must show the turn"
    );
}

#[test]
fn a_turned_map_is_a_turned_stone_and_camera_for_a_polished_stone() {
    assert_turned_map_equals_turned_stone("polished diamond", &GemMaterial::diamond());
}

/// The scattering stone takes the Henyey-Greenstein NEE draws toward the map; they must come out
/// of the turned map's importance distribution in world directions.
#[test]
fn a_turned_map_is_a_turned_stone_and_camera_for_a_scattering_stone() {
    let hazy = GemMaterial::new_custom("haze probe", 1.6, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(1.0, 0.2);
    assert_turned_map_equals_turned_stone("scattering stone", &hazy);
}

/// A uniform map looks the same from every side: turning it changes nothing on average, with
/// the NEE draws of a scattering stone landing in other directions.
#[test]
fn a_turned_uniform_map_changes_nothing_on_average() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let hazy = GemMaterial::new_custom("haze probe", 1.6, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(1.0, 0.2);
    let map = EnvironmentMap::uniform(8, 4, [1.0, 1.0, 1.0]);
    let plain = traces(
        &planes,
        table_ray(),
        &hazy,
        EnvironmentSource::hdr(&map),
        MEAN_SAMPLES,
    );
    let turned = traces(
        &planes,
        table_ray(),
        &hazy,
        EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(2.3)),
        MEAN_SAMPLES,
    );
    assert_same_mean("uniform map", &plain, &turned);
}

/// A source whose rotation is the identity, however it was built, traces bit for bit like the
/// plain one: a polished stone and a scattering one with NEE.
#[test]
fn an_unturned_source_traces_bit_for_bit_like_the_plain_one() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let map = sky();
    let hazy = GemMaterial::new_custom("haze probe", 1.6, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(1.0, 0.2);
    for material in [GemMaterial::diamond(), hazy] {
        let plain = traces(
            &planes,
            table_ray(),
            &material,
            EnvironmentSource::hdr(&map),
            SAMPLES,
        );
        for yaw in [0.0, -0.0, TAU, f32::NAN] {
            let source = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
            let other = traces(&planes, table_ray(), &material, source, SAMPLES);
            for (a, b) in plain.iter().zip(&other) {
                assert_eq!(
                    a.to_array().map(f32::to_bits),
                    b.to_array().map(f32::to_bits)
                );
            }
        }
    }
}
