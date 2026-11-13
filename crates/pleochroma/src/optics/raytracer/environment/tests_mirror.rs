//! The glossy tray on an [`EnvironmentSource`]: its scope, the builders, and the directional
//! lookups it must not change.

use super::{
    EnvironmentSource, LightingPreset, TrayMirror,
    tests_shadow::{assert_same_as_reference, sphere_dirs},
};
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;

fn mirror() -> Option<TrayMirror> {
    TrayMirror::new(-0.5, 0.04)
}

#[test]
fn the_mirror_is_carried_only_by_a_studio_rig_with_a_backdrop() {
    assert!(mirror().is_some());
    for preset in LightingPreset::ALL {
        let plain = preset.studio(1.0, 0.3, 0.9);
        assert!(plain.tray_mirror().is_none(), "{preset:?}: default is off");
        // As Lit (no backdrop): attached but not read.
        let as_lit = plain.with_backdrop(0.0).with_tray_mirror(mirror());
        assert!(as_lit.tray_mirror().is_none(), "{preset:?}: backdrop 0");
        let lit = plain.with_backdrop(0.23).with_tray_mirror(mirror());
        assert_eq!(lit.tray_mirror(), mirror(), "{preset:?}: backdrop > 0");
        // Detaching returns to off.
        assert!(lit.with_tray_mirror(None).tray_mirror().is_none());
    }
}

#[test]
fn an_hdr_map_never_carries_a_mirror() {
    let map = EnvironmentMap::from_rgb(4, 2, vec![[0.5, 0.4, 0.3]; 8])
        .expect("self-consistent by construction");
    let env = EnvironmentSource::hdr_at(&map, 1.5).with_tray_mirror(mirror());
    assert!(env.tray_mirror().is_none());
    assert_eq!(env.exposure().to_bits(), 1.5_f32.to_bits());
    assert!(env.hdr_map().is_some());
}

#[test]
fn every_builder_keeps_the_mirror() {
    let env = LightingPreset::LightTent
        .studio(1.0, 0.3, 0.9)
        .with_backdrop(0.23)
        .with_tray_mirror(mirror());
    assert_eq!(env.with_backdrop(0.7).tray_mirror(), mirror());
    assert_eq!(env.with_head_shadow(12.0).tray_mirror(), mirror());
    assert_eq!(env.with_surface_glare(0.5).tray_mirror(), mirror());
    assert_eq!(env.with_tray_shadow(None).tray_mirror(), mirror());
    // And the other way round: a mirror attached last leaves the other fields alone.
    let before = LightingPreset::LightTent
        .studio(2.0, 0.3, 0.9)
        .with_backdrop(0.4)
        .with_head_shadow(12.0)
        .with_surface_glare(0.5);
    let after = before.with_tray_mirror(mirror());
    assert_eq!(after.exposure().to_bits(), before.exposure().to_bits());
    assert_eq!(
        after.head_shadow_deg().to_bits(),
        before.head_shadow_deg().to_bits()
    );
    assert_eq!(
        after.surface_glare().to_bits(),
        before.surface_glare().to_bits()
    );
    assert_eq!(after.lighting_preset(), before.lighting_preset());
}

#[test]
fn without_a_mirror_the_lookups_are_unchanged() {
    for preset in LightingPreset::ALL {
        let plain = preset.studio(1.2, 0.84, 1.2).with_backdrop(0.7);
        // `None` and `Some` alike: the directional lookups never read the mirror.
        for env in [
            plain.with_tray_mirror(None),
            plain.with_tray_mirror(mirror()),
        ] {
            for dir in sphere_dirs() {
                for origin in [Vec3::ZERO, Vec3::Y, Vec3::new(4.0, -0.2, 2.0)] {
                    assert_same_as_reference(env, plain, dir, origin);
                }
            }
        }
    }
}
