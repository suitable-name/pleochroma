//! Tests of the cache key and its canonical encoding.

use glam::{Mat4, Vec3};

use super::{
    key::{
        CACHE_FORMAT_VERSION, CacheKey, CacheKeyInputs, OPTICS_TABLES_REVISION, StableHasher,
        canonical_bytes,
    },
    positions::BuildSettings,
    tests_support::{Rig, glass, uniaxial_glass},
};
use crate::{
    optics::{
        materials::{GemMaterial, ametrine_zoning},
        raytracer::{Camera, FacetFinish, LightingPreset},
        tables::OpticsTables,
    },
    renderer::adaptive::NoiseMetric,
};

/// The frame edge of the reference scenes.
const SIZE: u32 = 12;

/// The key of a rig seen at `size` pixels square with the default settings.
fn key_of(rig: &Rig, size: u32) -> CacheKey {
    CacheKey::from_scene(
        &rig.scene(size, 8),
        Mat4::IDENTITY,
        &BuildSettings::default(),
    )
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hasher = StableHasher::new();
    hasher.write(bytes);
    hasher.lane_a()
}

#[test]
fn the_hasher_lane_a_is_fnv1a_64() {
    // The published FNV-1a 64-bit test vectors.
    assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn the_digest_depends_on_every_byte_and_on_the_length() {
    let digest = |bytes: &[u8]| {
        let mut hasher = StableHasher::new();
        hasher.write(bytes);
        hasher.finish()
    };
    assert_eq!(digest(b"abc"), digest(b"abc"));
    assert_ne!(digest(b"abc"), digest(b"abd"));
    assert_ne!(digest(b"abc"), digest(b"abc\0"));
    assert_ne!(digest(b""), digest(b"\0"));
    // Feeding in pieces is the same stream.
    let mut split = StableHasher::new();
    split.write(b"ab");
    split.write(b"c");
    assert_eq!(split.finish(), digest(b"abc"));
}

#[test]
fn the_encoding_starts_with_the_documented_header_and_the_plane_list() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let bytes = canonical_bytes(&CacheKeyInputs::from_scene(
        &scene,
        Mat4::IDENTITY,
        &settings,
    ));
    assert_eq!(&bytes[0..6], b"PCKEY1");
    assert_eq!(bytes[6..10], CACHE_FORMAT_VERSION.to_le_bytes());
    assert_eq!(bytes[10..14], 6_u32.to_le_bytes());
    // First plane: the +X face, normal (1, 0, 0), offset -1.
    assert_eq!(bytes[14..18], 1.0_f32.to_bits().to_le_bytes());
    assert_eq!(bytes[18..22], 0.0_f32.to_bits().to_le_bytes());
    assert_eq!(bytes[22..26], 0.0_f32.to_bits().to_le_bytes());
    assert_eq!(bytes[26..30], (-1.0_f32).to_bits().to_le_bytes());
    // Six planes of 16 bytes, then the count of non-default finishes (none).
    assert_eq!(bytes[14 + 6 * 16..14 + 6 * 16 + 4], 0_u32.to_le_bytes());
}

#[test]
fn the_encoding_and_the_key_are_canonical() {
    let rig = Rig::cube_from_above(uniaxial_glass());
    let settings = BuildSettings::default();
    let first = canonical_bytes(&CacheKeyInputs::from_scene(
        &rig.scene(SIZE, 8),
        Mat4::IDENTITY,
        &settings,
    ));
    let second = canonical_bytes(&CacheKeyInputs::from_scene(
        &rig.scene(SIZE, 8),
        Mat4::IDENTITY,
        &settings,
    ));
    assert_eq!(first, second);
    assert_eq!(key_of(&rig, SIZE), key_of(&rig, SIZE));
    // A rig rebuilt from scratch with equal inputs has the same key.
    assert_eq!(
        key_of(&Rig::cube_from_above(uniaxial_glass()), SIZE),
        key_of(&rig, SIZE)
    );
}

#[test]
fn the_key_prints_as_32_lowercase_hex_digits_and_parses_back() {
    let key = key_of(&Rig::cube_from_above(glass()), SIZE);
    let text = key.to_string();
    assert_eq!(text.len(), 32);
    assert!(
        text.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(CacheKey::from_hex(&text), Some(key));
    assert_eq!(CacheKey::from_bytes(*key.as_bytes()), key);
    assert_eq!(CacheKey::from_hex(&text.to_uppercase()), None);
    assert_eq!(CacheKey::from_hex("abc"), None);
    assert_eq!(CacheKey::from_hex(&"g".repeat(32)), None);
}

#[test]
fn lighting_exposure_and_background_are_not_in_the_key() {
    let rig = Rig::cube_from_above(glass());
    let mut scene = rig.scene(SIZE, 8);
    let before = CacheKey::from_scene(&scene, Mat4::IDENTITY, &BuildSettings::default());
    scene.environment = LightingPreset::LightTent.studio(3.5, 0.9, 0.1);
    let other_light = CacheKey::from_scene(&scene, Mat4::IDENTITY, &BuildSettings::default());
    assert_eq!(before, other_light);
}

/// The optics-table mode is part of the key; `Off` leaves the canonical bytes unchanged.
#[test]
fn the_optics_table_mode_changes_the_key_and_off_changes_nothing() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings);
    let off = inputs.with_optics_tables(OpticsTables::Off);
    let on = inputs.with_optics_tables(OpticsTables::On);
    assert_ne!(CacheKey::new(&off), CacheKey::new(&on));
    assert_eq!(CacheKey::new(&on), CacheKey::new(&on));
    // Off writes nothing extra; On appends the tag and the revision.
    let (off_bytes, on_bytes) = (canonical_bytes(&off), canonical_bytes(&on));
    assert_eq!(on_bytes[..off_bytes.len()], off_bytes[..]);
    assert_eq!(on_bytes.len(), off_bytes.len() + 6);
    assert_eq!(on_bytes[off_bytes.len()..][..2], *b"OT");
    assert_eq!(
        on_bytes[off_bytes.len() + 2..],
        OPTICS_TABLES_REVISION.to_le_bytes()
    );
    #[cfg(all(feature = "beam", beam_tests))]
    {
        use super::key::beam_cache_key;
        use crate::renderer::beam::BeamSettings;
        let beam = BeamSettings::default();
        assert_ne!(
            beam_cache_key(&off, &beam, 8),
            beam_cache_key(&on, &beam, 8)
        );
        assert_eq!(beam_cache_key(&on, &beam, 8), beam_cache_key(&on, &beam, 8));
    }
}

/// A finer lighting tolerance is another beam key: it changes the leaves, so kept ones do not fit.
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn the_lighting_tolerance_changes_the_beam_key() {
    use super::key::beam_cache_key;
    use crate::renderer::beam::BeamSettings;
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let build = BuildSettings::default();
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &build);
    let beam = BeamSettings::default();
    let finer = BeamSettings {
        lighting_tolerance: beam.lighting_tolerance * 0.5,
        ..beam
    };
    assert_ne!(
        beam_cache_key(&inputs, &beam, 8),
        beam_cache_key(&inputs, &finer, 8)
    );
}

#[test]
fn the_pose_changes_the_key() {
    let base = Rig::cube_from_above(glass());
    let moved = Rig::new(
        base.planes.clone(),
        glass(),
        Camera::new(0.02, std::f32::consts::FRAC_PI_2, 8.0, 24.0),
    );
    let zoomed = Rig::new(
        base.planes.clone(),
        glass(),
        Camera::new(0.0, std::f32::consts::FRAC_PI_2, 7.0, 24.0),
    );
    let wider = Rig::new(
        base.planes.clone(),
        glass(),
        Camera::new(0.0, std::f32::consts::FRAC_PI_2, 8.0, 30.0),
    );
    let key = key_of(&base, SIZE);
    assert_ne!(key, key_of(&moved, SIZE));
    assert_ne!(key, key_of(&zoomed, SIZE));
    assert_ne!(key, key_of(&wider, SIZE));

    let scene = base.scene(SIZE, 8);
    let rotated = CacheKey::from_scene(
        &scene,
        Mat4::from_rotation_y(0.01),
        &BuildSettings::default(),
    );
    assert_ne!(key, rotated);
}

#[test]
fn the_material_axis_and_zoning_change_the_key() {
    let key = key_of(&Rig::cube_from_above(uniaxial_glass()), SIZE);

    assert_ne!(key, key_of(&Rig::cube_from_above(glass()), SIZE));

    let mut tilted = uniaxial_glass();
    tilted.c_axis = Vec3::X;
    assert_ne!(key, key_of(&Rig::cube_from_above(tilted), SIZE));

    let mut turned = uniaxial_glass();
    turned.alpha_axis = Some(Vec3::Z);
    assert_ne!(key, key_of(&Rig::cube_from_above(turned), SIZE));

    let zoned: GemMaterial = uniaxial_glass().with_zoning(ametrine_zoning());
    assert_ne!(key, key_of(&Rig::cube_from_above(zoned), SIZE));

    let mut denser = uniaxial_glass();
    denser.absorption_path_scale = 2.0;
    assert_ne!(key, key_of(&Rig::cube_from_above(denser), SIZE));
}

#[test]
fn the_design_and_the_resolution_change_the_key() {
    let cube = Rig::cube_from_above(glass());
    let key = key_of(&cube, SIZE);
    assert_ne!(key, key_of(&cube, SIZE + 1));
    assert_ne!(key, key_of(&Rig::brilliant(glass()), SIZE));

    let mut planes = cube.planes.clone();
    planes[0].d = -1.0001;
    // `Camera` is not `Clone`: each rig takes the camera of a fresh copy of the cube.
    let nudged = Rig::new(planes, glass(), Rig::cube_from_above(glass()).camera);
    assert_ne!(key, key_of(&nudged, SIZE));

    // A different plane order is a different design for the cache.
    let mut swapped = cube.planes;
    swapped.swap(0, 1);
    let swapped = Rig::new(swapped, glass(), Rig::cube_from_above(glass()).camera);
    assert_ne!(key, key_of(&swapped, SIZE));
}

#[test]
fn the_build_settings_change_the_key() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let key = |settings: BuildSettings| CacheKey::from_scene(&scene, Mat4::IDENTITY, &settings);
    let base = key(BuildSettings::default());
    assert_ne!(
        base,
        key(BuildSettings {
            tile: 8,
            ..BuildSettings::default()
        })
    );
    assert_ne!(
        base,
        key(BuildSettings {
            k0: 8,
            ..BuildSettings::default()
        })
    );
    assert_ne!(
        base,
        key(BuildSettings {
            k_max: 32,
            ..BuildSettings::default()
        })
    );
    assert_ne!(
        base,
        key(BuildSettings {
            cutoff: 2.0e-3,
            ..BuildSettings::default()
        })
    );
    assert_ne!(
        base,
        key(BuildSettings {
            metric: NoiseMetric::AbsoluteDeltaE,
            ..BuildSettings::default()
        })
    );
    // Settings that sanitize to the same values give the same key.
    assert_eq!(
        key(BuildSettings {
            k0: 0,
            ..BuildSettings::default()
        }),
        key(BuildSettings {
            k0: 1,
            ..BuildSettings::default()
        })
    );
    // The bounce limit shapes the records too.
    assert_ne!(
        base,
        CacheKey::from_scene(
            &rig.scene(SIZE, 9),
            Mat4::IDENTITY,
            &BuildSettings::default()
        )
    );
}

#[test]
fn finishes_count_by_plane_not_by_slice_length() {
    let rig = Rig::cube_from_above(glass());
    let settings = BuildSettings::default();
    let key = |finishes: &[FacetFinish]| {
        let mut scene = rig.scene(SIZE, 8);
        scene.facet_finishes = finishes;
        CacheKey::from_scene(&scene, Mat4::IDENTITY, &settings)
    };
    let none = key(&[]);
    // A slice padded with the default finish renders the same, so it keys the same.
    assert_eq!(none, key(&[FacetFinish::Polished; 6]));
    assert_eq!(none, key(&[FacetFinish::Polished; 3]));
    // A frosted facet changes it, and which facet matters.
    let first = key(&[FacetFinish::Frosted]);
    let second = key(&[FacetFinish::Polished, FacetFinish::Frosted]);
    assert_ne!(none, first);
    assert_ne!(none, second);
    assert_ne!(first, second);
}
