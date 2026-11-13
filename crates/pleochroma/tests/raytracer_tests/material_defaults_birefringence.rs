//! Default-orientation checks and birefringence rendering tests for `GemMaterial`.
//!
//! Covers the `c_axis` default and its cut-orientation overrides, Zircon's ordinary and
//! extraordinary split against a flattened clone, cubic materials' bit-exact indifference to
//! `birefringence_delta`, and non-dispersive custom materials under spectral MIS.

use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

#[test]
fn test_gem_materials_default_c_axis_to_y() {
    // `c_axis` is a field on `GemMaterial` consumed by `trace_spectral_ray`. Every built-in
    // material and `new_custom` defaults it to Vec3::Y, except the materials with a
    // deliberate cut-orientation override, where the stone is cut with c in the table plane:
    // - Tourmaline: face-up down the closed e-ray axis is its worst view, so c is Vec3::X.
    // - Aquamarine: its blue is the e-ray and the o-ray is nearly colorless, so c is Vec3::X.
    // - Benitoite: its blue is the e-ray, so c lies in the table.
    // - Titanite: n_gamma (Z) lies in the table, so face-up light shows the full birefringence.
    // - Labradorite: its lamellae are parallel to the table, so n_gamma is tilted 79 degrees
    //   from Y.
    // See each entry's comment in `GemMaterial::all_materials`.
    const CUT_ORIENTATION_OVERRIDES: &[&str] = &[
        "Tourmaline",
        "Aquamarine",
        "Benitoite",
        "Titanite (Sphene)",
        "Labradorite",
    ];

    for m in GemMaterial::all_materials() {
        if CUT_ORIENTATION_OVERRIDES.contains(&m.name.as_str()) {
            assert_ne!(
                m.c_axis,
                Vec3::Y,
                "{} is documented as a cut-orientation override and should NOT default \
                 c_axis to Vec3::Y -- if this is no longer true, remove it from \
                 CUT_ORIENTATION_OVERRIDES",
                m.name
            );
            continue;
        }
        assert_eq!(
            m.c_axis,
            Vec3::Y,
            "material {:?} should default c_axis to Vec3::Y",
            m.name
        );
    }

    let custom = GemMaterial::new_custom("Test Custom", 1.5, 0.01, 0.02, [0.0, 0.0, 0.0]);
    assert_eq!(
        custom.c_axis,
        Vec3::Y,
        "new_custom should default c_axis to Vec3::Y"
    );
}

/// Returns the seed of paired sample `index` in the Zircon tests' sequence.
const fn paired_seed(index: u32) -> u32 {
    index.wrapping_mul(7919).wrapping_add(13)
}

/// Traces the paired samples of the Zircon test's sequence (the same ray, planes, ring lights,
/// 12 bounces and seeds) for `index` in `0..seeds`, birefringent Zircon and the same material
/// with `birefringence_delta` forced to 0, and returns for the X, Y and Z axis the mean of
/// birefringent minus flat and the per-sample standard deviation of that difference (Welford
/// in `f64`).
fn paired_difference_stats(seeds: u32) -> [(f64, f64); 3] {
    let planes = StandardGemCuts::standard_round_brilliant();
    let zircon = GemMaterial::by_name("Zircon").unwrap();
    let mut zircon_flat = zircon.clone();
    zircon_flat.birefringence_delta = 0.0;
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let trace = |material: &GemMaterial, seed: u32| {
        trace_spectral_ray(
            ray,
            &planes,
            material,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        )
    };
    let mut mean = [0.0f64; 3];
    let mut sq_dev = [0.0f64; 3];
    for index in 0..seeds {
        let seed = paired_seed(index);
        let difference = trace(&zircon, seed) - trace(&zircon_flat, seed);
        for (axis, value) in difference.to_array().into_iter().enumerate() {
            // Welford: running mean and sum of squared deviations.
            let value = f64::from(value);
            let delta = value - mean[axis];
            mean[axis] += delta / f64::from(index + 1);
            sq_dev[axis] = delta.mul_add(value - mean[axis], sq_dev[axis]);
        }
    }
    std::array::from_fn(|axis| (mean[axis], (sq_dev[axis] / f64::from(seeds - 1)).sqrt()))
}

#[test]
fn test_birefringence_splits_produce_measurable_difference_zircon_vs_flat() {
    // Zircon (birefringence_delta = 0.059) and the same material with delta 0.0 (so
    // `is_anisotropic` is false and the ordinary/extraordinary split never triggers) must
    // render measurably differently. Paired samples share a seed, which cancels unrelated
    // noise.
    let planes = StandardGemCuts::standard_round_brilliant();
    let zircon = GemMaterial::by_name("Zircon").unwrap();
    assert!(
        zircon.birefringence_delta.abs() > 0.01,
        "test assumes Zircon is strongly birefringent"
    );

    let mut zircon_flat = zircon.clone();
    zircon_flat.birefringence_delta = 0.0;

    // An off-axis ray so the wave normal is genuinely oblique to c_axis (Vec3::Y) --
    // at exactly normal incidence along the c-axis, ordinary and extraordinary rays
    // are degenerate (zero walk-off, n_eff == n_o) and there is nothing to detect.
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    let samples = 64u32;
    let mut sum_birefringent = Vec3::ZERO;
    let mut sum_flat = Vec3::ZERO;
    for i in 0..samples {
        let seed = paired_seed(i);
        sum_birefringent += trace_spectral_ray(
            ray,
            &planes,
            &zircon,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        sum_flat += trace_spectral_ray(
            ray,
            &planes,
            &zircon_flat,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let avg_birefringent = sum_birefringent / samples as f32;
    let avg_flat = sum_flat / samples as f32;
    let diff = (avg_birefringent - avg_flat).length();

    // With the split disabled both traces are identical, so this bound is a floor only. The
    // traces share seeds but not their discrete per-sample decisions: the extraordinary
    // companions spread at a different angle, so a few of the 64 samples can differ by a large
    // share of the sum. No bound on 64 samples separates a physics error from that, so the
    // expectation is checked by the 65,536-seed comparison below, where each paired mean lies
    // within about one standard error of 0. The extraordinary mode's Snell and Fresnel physics
    // uses the wave normal `k`, not the walked-off `S`; the lib test
    // `plane_parallel_uniaxial_slab_extraordinary_ray_exits_parallel_and_displaced` checks it.
    assert!(
        diff > 3e-4,
        "birefringent Zircon should render measurably differently (averaged over {samples} samples) than an \
         otherwise-identical flat (delta=0) material -- got diff={diff} (birefringent={avg_birefringent:?}, flat={avg_flat:?})"
    );
    for (tag, (mean, std_dev)) in ["X", "Y", "Z"]
        .into_iter()
        .zip(paired_difference_stats(65_536))
    {
        let std_err = std_dev / 65_536f64.sqrt();
        assert!(
            mean.abs() < 4.0f64.mul_add(std_err, 3e-3),
            "birefringence must not shift the expected render: the paired mean of birefringent \
             minus flat Zircon over 65,536 seeds on axis {tag} is {mean:+.3e} (standard error \
             {std_err:.1e}); an extraordinary mode evaluated against the walked-off direction \
             would move it"
        );
    }
}

/// Diagnostic for [`test_birefringence_splits_produce_measurable_difference_zircon_vs_flat`].
///
/// Prints that test's 64-sample difference, the eight of its 64 paired samples with the largest
/// difference (birefringent minus flat) with each one's share of the sum, and the paired mean
/// difference over 65,536 seeds (the first 64 are the test's) with its standard error and
/// per-sample standard deviation.
#[test]
#[ignore = "diagnostic, run on request: cargo test -p pleochroma --test raytracer_tests \
            zircon_vs_flat_difference_by_sample -- --ignored --nocapture"]
fn zircon_vs_flat_difference_by_sample() {
    const SEEDS: u32 = 65_536;
    let planes = StandardGemCuts::standard_round_brilliant();
    let zircon = GemMaterial::by_name("Zircon").unwrap();
    let mut zircon_flat = zircon.clone();
    zircon_flat.birefringence_delta = 0.0;
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let trace = |material: &GemMaterial, seed: u32| {
        trace_spectral_ray(
            ray,
            &planes,
            material,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        )
    };

    let mut rows: Vec<(u32, Vec3, Vec3)> = (0..64u32)
        .map(|index| {
            let seed = paired_seed(index);
            (index, trace(&zircon, seed), trace(&zircon_flat, seed))
        })
        .collect();
    let (sum_biref, sum_flat) = rows.iter().fold(
        (Vec3::ZERO, Vec3::ZERO),
        |(acc_biref, acc_flat), &(_, biref, flat)| (acc_biref + biref, acc_flat + flat),
    );
    let diff_64 = sum_biref / 64.0 - sum_flat / 64.0;
    println!("64 test seeds: diff={} {diff_64:?}", diff_64.length());
    let sum = sum_biref - sum_flat;
    rows.sort_by(|first, second| {
        (second.1 - second.2)
            .length()
            .total_cmp(&(first.1 - first.2).length())
    });
    for &(index, biref, flat) in rows.iter().take(8) {
        let difference = biref - flat;
        println!(
            "  sample {index:2}: birefringent={biref:?} flat={flat:?} difference={difference:?} \
             share of the sum {:.3}",
            difference.dot(sum) / sum.length_squared()
        );
    }

    let stats = paired_difference_stats(SEEDS);
    for (tag, (mean, std_dev)) in ["X", "Y", "Z"].into_iter().zip(stats) {
        let std_err = std_dev / f64::from(SEEDS).sqrt();
        println!(
            "{SEEDS} seeds: {tag} birefringent - flat = {mean:+.3e} (paired se {std_err:.1e}, \
             per-sample sd {std_dev:.3})"
        );
    }
}

#[test]
fn test_cubic_material_ignores_birefringence_delta_bit_identical() {
    // `is_anisotropic` gates on `crystal_system != Cubic`, so the ordinary/extraordinary split
    // never runs for a cubic gem like Diamond and `birefringence_delta` has no effect.
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    assert_eq!(
        diamond.crystal_system,
        pleochroma::optics::materials::CrystalSystem::Cubic
    );

    let mut diamond_spurious_biref = diamond.clone();
    diamond_spurious_biref.birefringence_delta = 0.5;

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    for seed in [1337u32, 42, 999, 7] {
        let xyz_a = trace_spectral_ray(
            ray,
            &planes,
            &diamond,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        let xyz_b = trace_spectral_ray(
            ray,
            &planes,
            &diamond_spurious_biref,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert_eq!(
            xyz_a, xyz_b,
            "Cubic material rendering must be bit-identical regardless of birefringence_delta (seed={seed})"
        );
    }
}

#[test]
fn test_non_dispersive_custom_material_renders_stably_under_spectral_mis() {
    // `new_custom` with `dispersion_delta = 0.0` gives a flat Cauchy model (b = c = 0), so n is
    // identical across the visible spectrum. Diamond's Sellmeier model is dispersive. The
    // raytracer's `mis_weighted_radiance` (`optics/raytracer/color.rs`) is the identity, so each
    // channel's radiance is summed unweighted, whatever the dispersion. This test checks the
    // render stays finite and non-negative for a representative non-dispersive material.
    let flat = GemMaterial::new_custom("Flat Test Gem", 1.62, 0.0, 0.0, [0.3, 0.3, 0.3]);
    let n_at_400 = flat.dispersion.evaluate(400.0);
    let n_at_700 = flat.dispersion.evaluate(700.0);
    assert!(
        (n_at_400 - n_at_700).abs() < 1e-6,
        "dispersion_delta=0.0 must yield a genuinely flat index across the visible spectrum (got n(400)={n_at_400}, n(700)={n_at_700})"
    );

    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    for seed in [1u32, 2, 3, 100, 4242] {
        let xyz = trace_spectral_ray(
            ray,
            &planes,
            &flat,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert!(
            xyz.x.is_finite() && xyz.y.is_finite() && xyz.z.is_finite(),
            "non-dispersive material rendering must stay finite under spectral MIS (seed={seed}, got {xyz:?})"
        );
        assert!(
            xyz.y >= 0.0,
            "luminance must be non-negative (seed={seed}, got {xyz:?})"
        );
    }
}
