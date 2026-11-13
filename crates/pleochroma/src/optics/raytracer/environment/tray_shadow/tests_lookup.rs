use super::*;
use glam::Vec3;

/// Runs `check` on a round brilliant seen through `environment`, the scene both
/// direction-only accelerators are asked about.
fn with_scene<R>(
    environment: crate::optics::raytracer::environment::EnvironmentSource<'_>,
    check: impl FnOnce(&crate::renderer::frame_scene::FrameScene<'_>) -> R,
) -> R {
    use crate::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::Camera},
        renderer::frame_scene::FrameScene,
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = FrameScene {
        camera: &camera,
        width: 2,
        height: 2,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment,
    };
    check(&scene)
}

/// The reason the path cache declines the scene seen through `environment`, if it does.
fn cache_decline(
    environment: crate::optics::raytracer::environment::EnvironmentSource<'_>,
) -> Option<crate::renderer::path_cache::PathCacheOff> {
    use crate::{
        optics::{
            fluorescence::Fluorescence,
            raytracer::{intersect::build_plane_soa, surface::SurfaceScene},
        },
        renderer::path_cache::{CacheOptions, PathCacheFrame},
    };
    with_scene(environment, |scene| {
        let soa = build_plane_soa(scene.planes);
        PathCacheFrame::new(
            scene,
            &soa,
            (&[], Fluorescence::none(), SurfaceScene::IDEAL),
            CacheOptions::default(),
        )
        .err()
    })
}

/// The reason the beam tracer declines the scene seen through `environment`, if it does.
#[cfg(all(feature = "beam", beam_tests))]
fn beam_decline(
    environment: crate::optics::raytracer::environment::EnvironmentSource<'_>,
) -> Option<crate::renderer::accel_beam::BeamOff> {
    with_scene(environment, |scene| {
        crate::renderer::beam::frame::BeamFrame::new(
            scene,
            &crate::renderer::accel_beam::BeamSettings::default(),
        )
        .err()
    })
}

#[test]
fn direction_only_accelerators_decline_the_stone_shadow() {
    use crate::{optics::raytracer::LightingPreset, renderer::path_cache::PathCacheOff};
    let shadow = map(2, |_, _, _| 0.5);
    let environment = LightingPreset::LightTent
        .studio(1.0, 0.0, 0.5)
        .with_backdrop(0.23)
        .with_tray_shadow(Some(&shadow));
    assert!(matches!(
        cache_decline(environment),
        Some(PathCacheOff::StoneShadow)
    ));
    #[cfg(all(feature = "beam", beam_tests))]
    assert!(matches!(
        beam_decline(environment),
        Some(crate::renderer::accel_beam::BeamOff::StoneShadow)
    ));
}

#[test]
fn direction_only_accelerators_decline_the_tray_reflection() {
    use crate::{
        optics::raytracer::{LightingPreset, environment::TrayMirror},
        renderer::path_cache::PathCacheOff,
    };
    let environment = LightingPreset::LightTent
        .studio(1.0, 0.0, 0.5)
        .with_backdrop(0.23)
        .with_tray_mirror(TrayMirror::new(-1.0, 0.04));
    assert!(matches!(
        cache_decline(environment),
        Some(PathCacheOff::TrayReflection)
    ));
    // A mirror on a rig without a backdrop is never read, so the cache is not declined for it.
    let unread = LightingPreset::LightTent
        .studio(1.0, 0.0, 0.5)
        .with_tray_mirror(TrayMirror::new(-1.0, 0.04));
    assert!(
        !matches!(cache_decline(unread), Some(PathCacheOff::TrayReflection)),
        "an unread mirror must not decline the cache"
    );
    #[cfg(all(feature = "beam", beam_tests))]
    assert!(matches!(
        beam_decline(environment),
        Some(crate::renderer::accel_beam::BeamOff::TrayReflection)
    ));
}

fn map(n: u32, value: impl Fn(u32, u32, usize) -> f32) -> TrayShadowMap {
    let mut factors = Vec::new();
    for y in 0..n {
        for x in 0..n {
            for b in 0..TRAY_SHADOW_BANDS {
                factors.push(value(x, y, b));
            }
        }
    }
    TrayShadowMap::from_parts(
        0.0,
        [0.0; 2],
        1.0,
        n,
        factors,
        vec![[1.0; 3]; (n * n) as usize],
        TrayShadowKey(17),
    )
}

#[test]
fn outside_the_map_the_factor_is_one() {
    // The builder bakes the fade at texel centres; lookup must preserve its monotonicity.
    let map = map(128, |x, y, _| {
        let u = (x as f32 + 0.5) / 64.0 - 1.0;
        let v = (y as f32 + 0.5) / 64.0 - 1.0;
        let t = ((u.abs().max(v.abs()) - 0.85) / 0.15).clamp(0.0, 1.0);
        let smooth = t * t * (-2.0_f32).mul_add(t, 3.0);
        (0.2_f32 - 1.0).mul_add(1.0 - smooth, 1.0)
    });
    for uv in [[1.01, 0.0], [0.0, -1.01], [2.0, 2.0]] {
        assert_eq!(map.factor_at(uv, 390.0), 1.0);
        assert!(
            map.plane_uv(Vec3::new(uv[0], 1.0, uv[1]), -Vec3::Y)
                .is_none()
        );
    }
    let mut previous = map.factor_at([0.85, 0.0], 390.0);
    for step in 1..=150 {
        let value = map.factor_at([(step as f32).mul_add(0.001, 0.85), 0.0], 390.0);
        assert!(value >= previous);
        previous = value;
    }
    assert!(map.plane_uv(Vec3::Y, Vec3::Y).is_none());
    assert!(map.plane_uv(Vec3::ZERO, -Vec3::Y).is_none());
}

#[test]
fn the_lookup_returns_texel_values_at_texel_centres() {
    let map = map(4, |x, y, b| (b as f32).mul_add(16.0, (x + 4 * y) as f32));
    for y in 0..4 {
        for x in 0..4 {
            let uv = [
                f32::midpoint(x as f32, 0.5) - 1.0,
                f32::midpoint(y as f32, 0.5) - 1.0,
            ];
            for b in 0..TRAY_SHADOW_BANDS {
                assert_eq!(
                    map.factor_at(uv, 20.0f32.mul_add(b as f32, 390.0)),
                    (b as f32).mul_add(16.0, (x + 4 * y) as f32)
                );
            }
        }
    }
    let left = map.factor_at([-0.5 - 1e-5, 0.0], 400.0);
    let right = map.factor_at([-0.5 + 1e-5, 0.0], 400.0);
    assert!((left - right).abs() < 1e-3);
    assert_eq!(map.factor_at([-0.75, -0.75], 400.0), 8.0);
}

#[test]
fn a_neutral_map_gives_neutral_rgb() {
    let map = map(4, |_, _, _| 1.0);
    for x in [-0.99, -0.5, 0.0, 0.37, 0.99] {
        assert_eq!(
            map.factor_rgb_for(Vec3::new(x, 1.0, 0.1), -Vec3::Y),
            Some([1.0; 3])
        );
        assert_eq!(
            map.factors_for(Vec3::new(x, 1.0, 0.1), -Vec3::Y, &[380.0, 410.0, 780.0]),
            Some([1.0; 3])
        );
    }
    let words = pack_gpu_words(&map);
    assert_eq!(
        &words[..10],
        &[1.0, 1.0, 4.0, 20.0, 0.0, 0.0, 0.0, 1.0, 390.0, 20.0]
    );
    assert_eq!(&words[16..], map.factors());
    assert_eq!(pack_disabled_gpu_words(), vec![0.0; 16]);
    assert_eq!(pack_whitted_rgb(&map), vec![[1.0, 1.0, 1.0, 0.0]; 16]);
}
