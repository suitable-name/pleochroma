//! The optics lookup tables through the production entry point.
//!
//! Off (the default) is bit-identical to the unprepared entry point, which has no table path at
//! all. On keeps the mean: the same `(seed, hero_rand)` draws traced with and without the tables
//! differ by the table error only, so the paired mean difference must sit within a few standard
//! errors of zero.

use super::{
    super::{
        camera::Ray, environment::LightingPreset, intersect::build_plane_soa, sampling::hash_u32,
        surface::SurfaceScene,
    },
    PreparedMaterial, trace_spectral_ray_prepared, trace_spectral_ray_with_finish_soa_geom_surface,
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{fluorescence::Fluorescence, materials::GemMaterial, tables::OpticsTables},
};
use glam::Vec3;

/// Samples per material of the on/off mean comparison.
const SAMPLES: u32 = 3_000;

fn centred(i: u32, salt: u32) -> f32 {
    (hash_u32(i ^ salt) as f32) / 4_294_967_295.0 - 0.5
}

/// An oblique camera ray from above that mostly meets the stone, varied per `i`.
fn ray_for(i: u32) -> Ray {
    Ray {
        origin: Vec3::new(centred(i, 0x1111) * 1.4, 2.5, centred(i, 0x2222) * 1.4),
        dir: Vec3::new(centred(i, 0x3333) * 0.8, -1.0, centred(i, 0x4444) * 0.8).normalize(),
    }
}

fn seed_and_hero(i: u32) -> (u32, f32) {
    let seed = 71_000 + i;
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// An isotropic absorbing stone: every bounce takes the scalar Fresnel path and every interior
/// segment the Beer-Lambert exponential.
fn absorbing_probe() -> GemMaterial {
    GemMaterial::new_custom("tables probe", 1.76, 0.02, 0.0, [0.5, 1.0, 1.5])
}

fn builtin(name: &str) -> GemMaterial {
    GemMaterial::by_name(name).expect("a built-in material")
}

/// Traces `samples` draws on a brilliant through `prepared`.
fn trace_all(prepared: &PreparedMaterial<'_>, samples: u32) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    (0..samples)
        .map(|i| {
            let (seed, hero) = seed_and_hero(i);
            trace_spectral_ray_prepared(
                ray_for(i),
                geom,
                &plane_soa,
                &[],
                prepared,
                Fluorescence::none(),
                12,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            )
        })
        .collect()
}

/// The unprepared entry point, which has no table path.
fn trace_unprepared(material: &GemMaterial, samples: u32) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    (0..samples)
        .map(|i| {
            let (seed, hero) = seed_and_hero(i);
            trace_spectral_ray_with_finish_soa_geom_surface(
                ray_for(i),
                geom,
                &plane_soa,
                &[],
                material,
                Fluorescence::none(),
                12,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            )
        })
        .collect()
}

#[test]
fn with_the_tables_off_a_prepared_trace_is_bit_identical_to_the_unprepared_one() {
    for material in [
        absorbing_probe(),
        builtin("Diamond"),
        builtin("Zircon"),
        builtin("Tanzanite"),
    ] {
        let plain = trace_unprepared(&material, 128);
        let default = PreparedMaterial::new(&material);
        assert_eq!(
            default.optics_tables(),
            OpticsTables::Off,
            "{}",
            material.name
        );
        let explicit = PreparedMaterial::new(&material).with_optics_tables(OpticsTables::Off);
        let with_default = trace_all(&default, 128);
        let with_explicit = trace_all(&explicit, 128);
        for (i, expected) in plain.iter().enumerate() {
            assert_eq!(
                bits(with_default[i]),
                bits(*expected),
                "{} sample {i} (default)",
                material.name
            );
            assert_eq!(
                bits(with_explicit[i]),
                bits(*expected),
                "{} sample {i} (explicit off)",
                material.name
            );
        }
        assert!(
            plain.iter().any(|xyz| xyz.max_element() > 0.0),
            "{}: no sample returned any light",
            material.name
        );
    }
}

/// The paired mean difference of one axis and its standard error.
fn paired_axis(on: &[Vec3], off: &[Vec3], axis: usize) -> (f64, f64, f64) {
    let diffs: Vec<f64> = on
        .iter()
        .zip(off)
        .map(|(a, b)| f64::from(a[axis]) - f64::from(b[axis]))
        .collect();
    let n = diffs.len() as f64;
    let mean = diffs.iter().sum::<f64>() / n;
    let var = diffs.iter().map(|d| (d - mean) * (d - mean)).sum::<f64>() / n;
    let reference = off.iter().map(|v| f64::from(v[axis])).sum::<f64>() / n;
    (mean, (var / n).sqrt(), reference)
}

#[test]
fn with_the_tables_on_the_mean_stays_within_the_paired_z_tolerance() {
    for material in [absorbing_probe(), builtin("Diamond"), builtin("Tourmaline")] {
        let off = PreparedMaterial::new(&material).with_optics_tables(OpticsTables::Off);
        let on = PreparedMaterial::new(&material).with_optics_tables(OpticsTables::On);
        assert_eq!(on.optics_tables(), OpticsTables::On);
        assert!(on.optics_table_set().is_some());
        let off_xyz = trace_all(&off, SAMPLES);
        let on_xyz = trace_all(&on, SAMPLES);
        for axis in 0..3 {
            let (mean, se, reference) = paired_axis(&on_xyz, &off_xyz, axis);
            println!(
                "{} axis {axis}: paired mean diff {mean:.3e}, se {se:.3e}, mean {reference:.3e}",
                material.name
            );
            assert!(
                mean.abs() <= 4.0f64.mul_add(se, 1e-3 * reference.abs()),
                "{} axis {axis}: paired mean difference {mean:e} (se {se:e}, mean {reference:e})",
                material.name
            );
        }
    }
}

#[test]
fn with_the_tables_on_the_table_path_is_actually_reached() {
    // The isotropic absorbing probe uses the Beer-Lambert exponential on every interior segment,
    // so at least some samples must differ from the polynomial result in the last bits.
    let material = absorbing_probe();
    let off = PreparedMaterial::new(&material).with_optics_tables(OpticsTables::Off);
    let on = PreparedMaterial::new(&material).with_optics_tables(OpticsTables::On);
    let off_xyz = trace_all(&off, 256);
    let on_xyz = trace_all(&on, 256);
    let differing = on_xyz
        .iter()
        .zip(&off_xyz)
        .filter(|(a, b)| bits(**a) != bits(**b))
        .count();
    assert!(
        differing > 0,
        "the tables changed no sample: the table path is not reached"
    );
}
