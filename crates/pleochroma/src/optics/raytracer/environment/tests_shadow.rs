//! Position-aware lookups and escape paths through an emerald cut.

use super::{
    LightingPreset, TrayShadowKey, TrayShadowMap, sample_environment_channel,
    sample_environment_channel_at, sample_environment_channels, sample_environment_channels_at,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Ray, hash_u32, trace_spectral_ray},
        studio_rig::StudioRig,
    },
};
use glam::Vec3;

#[test]
fn without_a_map_the_lookup_is_unchanged() {
    let rig = StudioRig::new(0.84, 1.2);
    let lambdas = [391.0, 477.0, 535.0, 701.0];
    for preset in LightingPreset::ALL {
        let env = preset.studio(1.2, 0.84, 1.2).with_backdrop(0.7);
        for i in 0..48 {
            let y = (i as f32 + 0.5) / 24.0 - 1.0;
            let (s, c) = (i as f32 * 2.4).sin_cos();
            let r = y.mul_add(-y, 1.0).sqrt();
            let dir = Vec3::new(r * c, y, r * s);
            for origin in [Vec3::ZERO, Vec3::Y, Vec3::new(4.0, -0.2, 2.0)] {
                let packet =
                    sample_environment_channels_at(env, origin, dir, &lambdas, Some(&rig), Vec3::Y);
                let old = sample_environment_channels(env, dir, &lambdas, Some(&rig), Vec3::Y);
                assert_eq!(packet.map(f32::to_bits), old.map(f32::to_bits));
                let single = sample_environment_channel_at(
                    env,
                    origin,
                    dir,
                    lambdas[0],
                    Some(&rig),
                    Vec3::Y,
                );
                assert_eq!(
                    single.to_bits(),
                    sample_environment_channel(env, dir, lambdas[0], Some(&rig), Vec3::Y).to_bits()
                );
            }
        }
    }
}

/// Every direction on a spiral over the sphere, `y` running from -1 to 1.
pub(super) fn sphere_dirs() -> impl Iterator<Item = Vec3> {
    (0..48).map(|i| {
        let y = (i as f32 + 0.5) / 24.0 - 1.0;
        let (s, c) = (i as f32 * 2.4).sin_cos();
        let r = y.mul_add(-y, 1.0).sqrt();
        Vec3::new(r * c, y, r * s)
    })
}

/// Asserts the origin-aware lookups equal the directional ones bit for bit.
fn assert_same_as_directional(env: super::EnvironmentSource<'_>, dir: Vec3, origin: Vec3) {
    assert_same_as_reference(env, env, dir, origin);
}

/// Asserts the origin-aware lookups of `env` equal the directional lookups of `reference` bit
/// for bit.
pub(super) fn assert_same_as_reference(
    env: super::EnvironmentSource<'_>,
    reference: super::EnvironmentSource<'_>,
    dir: Vec3,
    origin: Vec3,
) {
    let rig = StudioRig::new(0.84, 1.2);
    let lambdas = [391.0, 477.0, 535.0, 701.0];
    let packet = sample_environment_channels_at(env, origin, dir, &lambdas, Some(&rig), Vec3::Y);
    let old = sample_environment_channels(reference, dir, &lambdas, Some(&rig), Vec3::Y);
    assert_eq!(packet.map(f32::to_bits), old.map(f32::to_bits));
    let single = sample_environment_channel_at(env, origin, dir, lambdas[0], Some(&rig), Vec3::Y);
    assert_eq!(
        single.to_bits(),
        sample_environment_channel(reference, dir, lambdas[0], Some(&rig), Vec3::Y).to_bits()
    );
}

#[test]
fn a_map_with_backdrop_zero_is_ignored() {
    let map = dark_map();
    for preset in LightingPreset::ALL {
        let env = preset
            .studio(1.2, 0.84, 1.2)
            .with_backdrop(0.0)
            .with_tray_shadow(Some(&map));
        for dir in sphere_dirs() {
            for origin in [Vec3::ZERO, Vec3::Y, Vec3::new(0.5, -0.2, 0.3)] {
                assert_same_as_directional(env, dir, origin);
            }
        }
    }
}

#[test]
fn an_hdr_source_ignores_an_attached_map() {
    let map = dark_map();
    let panorama = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [0.6, 0.4, 0.2]);
    let env = super::EnvironmentSource::hdr_at(&panorama, 1.5).with_tray_shadow(Some(&map));
    for dir in sphere_dirs() {
        for origin in [Vec3::ZERO, Vec3::new(0.5, -0.2, 0.3)] {
            assert_same_as_directional(env, dir, origin);
        }
    }
}

#[test]
fn upward_rays_never_read_the_map() {
    let map = dark_map();
    for preset in LightingPreset::ALL {
        let env = preset
            .studio(1.2, 0.84, 1.2)
            .with_backdrop(0.7)
            .with_tray_shadow(Some(&map));
        for dir in sphere_dirs().filter(|dir| dir.y > 0.0) {
            for origin in [Vec3::ZERO, Vec3::Y, Vec3::new(0.5, -0.2, 0.3)] {
                assert_same_as_directional(env, dir, origin);
            }
        }
    }
}

fn dark_map() -> TrayShadowMap {
    TrayShadowMap::from_parts(
        -0.9,
        [0.0; 2],
        3.0,
        1,
        vec![0.05; 20],
        vec![[0.05; 3]],
        TrayShadowKey(1),
    )
}

#[test]
fn far_from_the_stone_the_backdrop_is_unchanged() {
    let map = dark_map();
    let env = LightingPreset::IsoHemisphere
        .studio(1.0, 0.84, 1.2)
        .with_backdrop(0.9);
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::diamond();
    let ray = Ray {
        origin: Vec3::new(5.0, 2.5, 0.0),
        dir: -Vec3::Y,
    };
    let trace = |light| trace_spectral_ray(ray, &planes, &material, 32, light, 7, 0.25, None);
    assert_eq!(
        trace(env).to_array().map(f32::to_bits),
        trace(env.with_tray_shadow(Some(&map)))
            .to_array()
            .map(f32::to_bits)
    );
}

#[test]
fn a_shadowed_window_is_darker() {
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::diamond();
    let map = dark_map();
    let env = LightingPreset::IsoHemisphere
        .studio(1.0, 0.84, 1.2)
        .with_backdrop(0.9);
    let shadowed = env.with_tray_shadow(Some(&map));
    // Stay inside the table outline, where a lower radiance must have arrived through the stone.
    for i in 0..256 {
        let x = ((i % 16) as f32 - 7.5) / 16.0;
        let z = ((i / 16) as f32 - 7.5) / 16.0;
        let ray = Ray {
            origin: Vec3::new(x, 2.5, z),
            dir: Vec3::new(0.05, -1.0, 0.03).normalize(),
        };
        let trace = |light| {
            trace_spectral_ray(ray, &planes, &material, 32, light, hash_u32(i), 0.5, None).y
        };
        let lit = trace(env);
        if lit > 0.0 && trace(shadowed) < 0.95 * lit {
            return;
        }
    }
    panic!("no window of the emerald cut resolved its shadowed backdrop");
}
