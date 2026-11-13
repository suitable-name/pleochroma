//! Small deterministic precompute and cache-identity checks.
use super::{
    build::{BuildSettings, build_with_settings, gaussian_kernel, spectral_to_rgb},
    *,
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Test requests share only physically relevant inputs with production requests.
const fn input<'a>(
    planes: &'a [crate::geometry::plane::GpuFacetPlane],
    material: &'a GemMaterial,
) -> TrayShadowInput<'a> {
    TrayShadowInput {
        geom: StoneGeometry::planes_only(planes),
        material,
        preset: LightingPreset::LightTent,
        yaw: 0.0,
        pitch: 0.0,
        max_bounces: 8,
    }
}

#[test]
fn the_map_does_not_depend_on_the_thread_count() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("built-in diamond");
    let input = input(&planes, &material);
    let cancel = AtomicBool::new(false);
    let maps: Vec<_> = [1, 2, 7]
        .map(|threads| {
            build_with_settings(
                &input,
                TrayShadowQuality::Preview,
                &cancel,
                &mut |_| {},
                BuildSettings {
                    // 17 rows span three gather chunks of eight, so the parallel gather
                    // is exercised as well as the 17 photon chunks.
                    resolution: 17,
                    directions: 8,
                    photons: 4_097,
                    photon_chunk_size: 256,
                    threads,
                },
            )
            .expect("small map")
        })
        .into_iter()
        .collect();
    for map in &maps[1..] {
        assert_eq!(maps[0], *map);
    }
}

#[test]
fn a_cancelled_build_stops() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("built-in diamond");
    let input = input(&planes, &material);
    let cancel = AtomicBool::new(false);
    let mut photon_updates = 0;
    let mut progress = |fraction| {
        if fraction > 0.3 {
            photon_updates += 1;
            cancel.store(true, Ordering::Relaxed);
        }
    };
    assert_eq!(
        build_with_settings(
            &input,
            TrayShadowQuality::Preview,
            &cancel,
            &mut progress,
            BuildSettings {
                resolution: 4,
                directions: 4,
                photons: 4_097,
                photon_chunk_size: 64,
                threads: 1
            }
        ),
        Err(TrayShadowOff::Cancelled)
    );
    assert_eq!(
        photon_updates, 1,
        "cancellation stops before another ordered chunk is splatted"
    );
}

#[test]
fn the_key_ignores_exposure_backdrop_and_camera() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("built-in diamond");
    let mut input = input(&planes, &material);
    let quality = TrayShadowQuality::Preview;
    let key = tray_shadow_key(&input, quality);
    assert_eq!(key, tray_shadow_key(&input, quality));
    assert_ne!(key, tray_shadow_key(&input, TrayShadowQuality::Final));
    input.yaw = 0.2;
    assert_ne!(key, tray_shadow_key(&input, quality));
    input.yaw = 0.0;
    input.pitch = 0.2;
    assert_ne!(key, tray_shadow_key(&input, quality));
    input.pitch = 0.0;
    input.max_bounces += 1;
    assert_ne!(key, tray_shadow_key(&input, quality));
    input.max_bounces -= 1;
    input.preset = LightingPreset::DaylightSun;
    assert_ne!(key, tray_shadow_key(&input, quality));
    input.preset = LightingPreset::LightTent;
    let other = StandardGemCuts::emerald_cut();
    input.geom = StoneGeometry::planes_only(&other);
    assert_ne!(key, tray_shadow_key(&input, quality));
    input.geom = StoneGeometry::planes_only(&planes);
    let mut changed = material.clone();
    changed.absorption_path_scale += 0.1;
    input.material = &changed;
    assert_ne!(key, tray_shadow_key(&input, quality));
}

#[test]
fn exposure_backdrop_head_shadow_and_glare_do_not_change_the_resolved_shadow() {
    use crate::render_setup::stone_shadow_for;
    use std::sync::Arc;
    let planes = StandardGemCuts::standard_round_brilliant();
    let mut material = GemMaterial::by_name("Diamond").expect("built-in diamond");
    // A perturbed material gives a key no real scene can have, so the fake map seeded below
    // can only ever be hit by this test.
    material.absorption_path_scale += 1.0e-3;
    let request = input(&planes, &material);
    let quality = TrayShadowQuality::Preview;
    // Seed the cache under this request's key with a tiny neutral map, so a hit is observable
    // without a precompute: a different key would start a full-size build instead.
    let seeded = Arc::new(TrayShadowMap::from_parts(
        0.0,
        [0.0; 2],
        1.0,
        1,
        vec![1.0; TRAY_SHADOW_BANDS],
        vec![[1.0; 3]],
        tray_shadow_key(&request, quality),
    ));
    super::cache::seed_for_test(&seeded);
    let base = request
        .preset
        .studio(1.0, request.yaw, request.pitch)
        .with_backdrop(0.23);
    let variants = [
        base,
        base.with_backdrop(0.7),
        base.with_head_shadow(12.0),
        base.with_surface_glare(0.5),
        request
            .preset
            .studio(3.5, request.yaw, request.pitch)
            .with_backdrop(0.23),
    ];
    for environment in variants {
        let resolved = stone_shadow_for(
            true,
            environment,
            request.geom,
            &material,
            request.max_bounces,
            quality,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("a cached map resolves")
        .expect("the shadow is enabled");
        assert!(
            Arc::ptr_eq(&resolved, &seeded),
            "the scene's own settings must not reach the key"
        );
    }
}

#[test]
fn the_caustic_filter_preserves_a_constant() {
    assert!((gaussian_kernel().iter().sum::<f32>() - 1.0).abs() < 1.0e-6);
}

#[test]
fn neutral_spectral_factors_give_neutral_rgb_with_real_illuminant_weights() {
    use crate::renderer::whitted::tint::bin_weights;
    for preset in LightingPreset::ALL {
        if preset.is_uv_lamp() || preset == LightingPreset::Aset {
            continue;
        }
        let weights = bin_weights(preset.studio(1.0, 0.0, 0.0));
        let rgb = spectral_to_rgb(&[1.0; TRAY_SHADOW_BANDS], &weights);
        for value in rgb {
            assert!((value - 1.0).abs() <= 2.0e-6, "{preset:?}: {rgb:?}");
        }
    }
}
