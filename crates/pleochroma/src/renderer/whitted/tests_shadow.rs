//! CPU checks of the Ray-Traced engine's stone shadow on the paper: `WhittedEnv::leaf_at` and a
//! small frame. No GPU is involved, so these run everywhere.

use glam::Vec3;

use super::{WhittedEnv, WhittedPrep, tint};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingPreset,
            build_plane_soa,
            environment::{
                TrayShadowKey, TrayShadowMap,
                sample_studio_environment_with_rig_spec_power_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    renderer::{engine::RayOptions, frame_scene::FrameScene},
};

pub(super) const EXPOSURE: f32 = 1.3;
pub(super) const YAW: f32 = 0.85;
pub(super) const PITCH: f32 = 0.95;
pub(super) const TRAY: f32 = 0.81;
/// Per-channel ratios of the test map; the first is zero so the floor is exercised.
pub(super) const RATIOS: [f32; 3] = [0.0, 0.2, 0.5];

/// A one-texel map on the plane `y = -0.9`, 6 units wide, with `ratios` as its RGB ratio.
pub(super) fn map(ratios: [f32; 3]) -> TrayShadowMap {
    TrayShadowMap::from_parts(
        -0.9,
        [0.0; 2],
        3.0,
        1,
        vec![0.05; 20],
        vec![ratios],
        TrayShadowKey(1),
    )
}

pub(super) fn lit(backdrop: f32) -> EnvironmentSource<'static> {
    LightingPreset::IsoHemisphere
        .studio(EXPOSURE, YAW, PITCH)
        .with_backdrop(backdrop)
}

pub(super) fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

#[test]
fn below_the_horizon_each_channel_reads_its_own_tray() {
    let shadow = map(RATIOS);
    let source = lit(TRAY).with_tray_shadow(Some(&shadow));
    let env = WhittedEnv::new(source);
    let rig = StudioRig::new(YAW, PITCH);
    let rgb = tint::illuminant_rgb(source);
    let dir = Vec3::new(0.1, -1.0, 0.05).normalize();
    let observer = Vec3::Y;
    let actual = env.leaf_at(Vec3::ZERO, dir, observer);
    let expected = rgb
        * Vec3::from_array(RATIOS.map(|ratio| {
            sample_studio_environment_with_rig_spec_power_shadow_tray(
                dir,
                LightingPreset::IsoHemisphere,
                EXPOSURE,
                &rig,
                observer,
                1.0,
                DEFAULT_HEAD_SHADOW_COSINES,
                (TRAY * ratio).max(f32::MIN_POSITIVE),
            )
        }));
    assert_eq!(bits(actual), bits(expected));
    // A dark map darkens every channel, the fully shadowed one included.
    let open = env.leaf(dir, observer);
    assert!(actual.x < open.x && actual.y < open.y && actual.z < open.z);
    assert!(actual.x >= 0.0);
}

#[test]
fn leaf_at_is_the_leaf_wherever_no_shadow_applies() {
    let shadow = map(RATIOS);
    let observer = Vec3::Y;
    let down = Vec3::new(0.1, -1.0, 0.05).normalize();
    let up = Vec3::new(0.2, 0.9, 0.1).normalize();

    let shadowed = WhittedEnv::new(lit(TRAY).with_tray_shadow(Some(&shadow)));
    // Above the horizon the map is never read.
    assert_eq!(
        bits(shadowed.leaf_at(Vec3::ZERO, up, observer)),
        bits(shadowed.leaf(up, observer))
    );
    // A ray that lands outside the map is the directional lookup.
    let far = Vec3::new(50.0, 0.0, 0.0);
    assert_eq!(
        bits(shadowed.leaf_at(far, down, observer)),
        bits(shadowed.leaf(down, observer))
    );
    // No map attached.
    let plain = WhittedEnv::new(lit(TRAY));
    assert_eq!(
        bits(plain.leaf_at(Vec3::ZERO, down, observer)),
        bits(plain.leaf(down, observer))
    );
    // A map with no tray (backdrop 0, As Lit) is ignored.
    let as_lit = WhittedEnv::new(lit(0.0).with_tray_shadow(Some(&shadow)));
    assert_eq!(
        bits(as_lit.leaf_at(Vec3::ZERO, down, observer)),
        bits(as_lit.leaf(down, observer))
    );
}

#[test]
fn a_ray_traced_frame_shows_the_shadow_on_the_tray() {
    let shadow = map([0.05; 3]);
    let camera = Camera::new(0.6, std::f32::consts::FRAC_PI_2 - 0.01, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let soa = build_plane_soa(&[]);
    let total_y = |environment: EnvironmentSource<'_>| {
        let scene = FrameScene {
            camera: &camera,
            width: 4,
            height: 4,
            planes: &[],
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
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
        (0..16)
            .map(|pixel| super::whitted_tap_xyz(&scene, &[], &soa, &prep, pixel, 0, None).y)
            .sum::<f32>()
    };
    let open = total_y(lit(TRAY));
    let shaded = total_y(lit(TRAY).with_tray_shadow(Some(&shadow)));
    assert!(open > 0.0);
    assert!(shaded < open, "{shaded} !< {open}");
}
