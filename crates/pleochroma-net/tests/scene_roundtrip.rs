//! `SceneState` must round-trip through `postcard` unchanged, including a material with absorption
//! bands, biaxial data and a full facet-plane set.
//!
//! A name- or id-based scheme would lose exactly those fields (see `pleochroma_net::scene`'s module
//! docs).

use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_net::SceneState;

/// Wire size of a default `SceneEffects` record: five absent `Option`s or empty `Vec`s of one
/// byte each (the last one being `material_inclusions`), the default machine (five `f32`) and
/// the unset edge (three `f32`).
const EFFECTS_TAIL: usize = 5 + 5 * 4 + 3 * 4;
const ROLL_TAIL: usize = size_of::<f32>();
/// The `stone_shadow` bool, which always occupies one byte.
const SHADOW_BYTE: usize = 1;
/// The `tray_reflection` `f32`, which always occupies four bytes.
const REFLECTION_TAIL: usize = size_of::<f32>();
/// The `hdr_orientation`: the yaw's `f32` and the follow flag's byte.
const ORIENTATION_TAIL: usize = size_of::<f32>() + 1;
/// The `estimator`: one postcard varint byte (0 branching, 1 exit collection).
const ESTIMATOR_TAIL: usize = 1;
/// The `camera_pan`: two `f32`, the last bytes of the wire form.
const PAN_TAIL: usize = 2 * size_of::<f32>();
/// The bytes written after `camera_roll`: the shadow byte, the reflectance, the orientation,
/// the estimator and the pan.
const AFTER_ROLL_TAIL: usize =
    SHADOW_BYTE + REFLECTION_TAIL + ORIENTATION_TAIL + ESTIMATOR_TAIL + PAN_TAIL;

fn sample_scene() -> SceneState {
    // Alexandrite has both absorption bands and biaxial_delta_beta_alpha = Some(..) --
    // the two nested-data cases most likely dropped by an incomplete derive.
    let material = GemMaterial::by_name("Alexandrite").expect("Alexandrite is a built-in material");
    assert!(
        !material.absorption.o_ray.is_empty(),
        "test material should actually exercise absorption bands"
    );
    assert!(
        material.biaxial_delta_beta_alpha.is_some(),
        "test material should actually exercise biaxial data"
    );

    let planes = StandardGemCuts::standard_round_brilliant();
    assert!(
        planes.len() >= 57,
        "test should exercise a full facet-plane set"
    );

    SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 640,
        height: 480,
        yaw: 0.37,
        camera_roll: 0.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        estimator: pleochroma::optics::raytracer::Estimator::Branching,
        pitch: -0.12,
        distance: 3.5,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.25,
        max_bounces: 12,
        lighting_preset: LightingPreset::RingLights,
        material,
        planes,
        girdle_frosted: false,
        backdrop: 0.0,
        environment: pleochroma_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        camera_pan: [0.0, 0.0],
    }
}

#[test]
fn scene_state_round_trips_unchanged_through_postcard() {
    let scene = sample_scene();

    let bytes = postcard::to_allocvec(&scene).expect("SceneState must serialize");
    let decoded: SceneState = postcard::from_bytes(&bytes).expect("SceneState must deserialize");

    assert_eq!(
        scene, decoded,
        "SceneState must round-trip bit-for-bit through postcard"
    );
}

/// A scene with concave tools round-trips, and so does a convex one (the empty `Vec`).
/// Postcard cannot omit a trailing field, so both must decode by count.
#[test]
fn scene_state_with_and_without_tools_round_trips_through_postcard() {
    use glam::Vec3;
    use pleochroma::geometry::ToolPrimitive;

    let convex = sample_scene();
    assert_eq!(convex.tools, Vec::<ToolPrimitive>::new());
    let convex_bytes = postcard::to_allocvec(&convex).unwrap();
    let decoded: SceneState = postcard::from_bytes(&convex_bytes).unwrap();
    assert_eq!(convex, decoded);

    let mut concave = sample_scene();
    concave.tools = vec![
        ToolPrimitive::ball(Vec3::new(0.0, 0.0, 0.4), 0.2),
        ToolPrimitive::cylinder(Vec3::new(0.1, 0.0, 0.3), Vec3::X, 0.05, 0.3),
    ];
    let concave_bytes = postcard::to_allocvec(&concave).unwrap();
    let decoded: SceneState = postcard::from_bytes(&concave_bytes).unwrap();
    assert_eq!(concave, decoded);
    assert!(
        concave_bytes.len() > convex_bytes.len(),
        "tools are a length-prefixed trailing Vec, so they only add bytes"
    );
}

/// A scene with fluorescent emitters round-trips through postcard, as does a non-fluorescent
/// one (which costs one zero length byte). The cached sampling tables a trace builds are not
/// part of the wire form.
#[test]
fn scene_state_with_and_without_fluorescence_round_trips_through_postcard() {
    use pleochroma::optics::{
        absorption::AbsorptionBand,
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter},
    };

    let plain = sample_scene();
    assert!(plain.fluorescence.is_empty());
    let plain_bytes = postcard::to_allocvec(&plain).unwrap();
    let decoded: SceneState = postcard::from_bytes(&plain_bytes).unwrap();
    assert_eq!(plain, decoded);

    let mut glowing = sample_scene();
    glowing.lighting_preset = LightingPreset::UvLamp365;
    glowing.fluorescence = Fluorescence::new(vec![FluorescentEmitter {
        excitation: vec![
            AbsorptionBand::energy(410.0, 3500.0, 2.0),
            AbsorptionBand::energy(556.0, 2800.0, 2.0),
        ],
        emission: vec![
            EmissionBand::new(692.9, 1.0, 0.4),
            EmissionBand::new(694.3, 1.0, 0.6),
        ],
        quantum_yield: 0.9,
    }]);
    // Build the sampling tables, as a trace would: they must not change the wire bytes.
    let before = postcard::to_allocvec(&glowing).unwrap();
    let _ = glowing.fluorescence.pseudo_extinction(694.0);
    let bytes = postcard::to_allocvec(&glowing).unwrap();
    assert_eq!(before, bytes);
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(glowing, decoded);
    assert_eq!(decoded.lighting_preset, LightingPreset::UvLamp365);
    assert_eq!(decoded.fluorescence.emitters().len(), 1);
    assert!(
        bytes.len() > plain_bytes.len(),
        "emitters are a length-prefixed trailing Vec, so they only add bytes"
    );
    // The emitter list is followed on the wire by `head_shadow_deg` (an f32), the empty
    // frosted-plane list (one zero length byte), the engine (three bytes), the default effects
    // record, `camera_roll` (f32), `stone_shadow` (one byte), `tray_reflection` (f32),
    // `hdr_orientation` (five bytes), `estimator` (one byte) and `camera_pan` (eight bytes).
    let tail = postcard::to_allocvec(&glowing.fluorescence).unwrap();
    let emitters_end = bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL - 8;
    assert_eq!(
        &bytes[emitters_end - tail.len()..emitters_end],
        tail.as_slice()
    );
}

/// A layout without the trailing `head_shadow_deg` (the v20 layout) does not decode: postcard
/// cannot skip a missing trailing field, so the bytes run out. Pins that the decode fails
/// rather than yielding a scene.
#[test]
fn a_v20_scene_byte_layout_does_not_decode_as_v21() {
    let scene = sample_scene();
    let bytes = postcard::to_allocvec(&scene).unwrap();
    // Strip the effects record, the roll and the tail bytes: this test is about the layout
    // before them.
    let bytes = &bytes[..bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL];
    assert_eq!(
        &bytes[bytes.len() - 3..],
        &[0, 8, 1],
        "v28 appends Path-Traced engine, eight taps, Fire"
    );
    let current = &bytes[..bytes.len() - 3];
    // Without the trailing empty frosted-plane list (one zero length byte) this is the v21
    // layout.
    assert_eq!(
        current[current.len() - 1],
        0,
        "an empty frosted-plane list is one zero trailing byte"
    );
    let v21 = &current[..current.len() - 1];
    // Without the trailing head-shadow f32 (16.0 = 0, 0, 128, 65) this is the v20 layout.
    assert_eq!(
        v21[v21.len() - 4..],
        16.0f32.to_le_bytes(),
        "the head shadow is four bytes before the frosted-plane list"
    );
    let v20 = &v21[..v21.len() - 4];
    assert!(postcard::from_bytes::<SceneState>(v20).is_err());
    // Also without the empty emitter list's zero byte (the v19 layout), it fails too.
    let v19 = &v21[..v21.len() - 5];
    assert!(postcard::from_bytes::<SceneState>(v19).is_err());
}

/// A layout without the trailing `stone_shadow` byte (v39) does not decode. Postcard cannot
/// skip a missing trailing field, so the protocol version check in the handshake protects peers.
#[test]
fn a_v39_scene_byte_layout_does_not_decode_as_v40() {
    let bytes = postcard::to_allocvec(&sample_scene()).unwrap();
    let v39 = &bytes[..bytes.len() - AFTER_ROLL_TAIL];
    assert!(
        postcard::from_bytes::<SceneState>(v39).is_err(),
        "the old layout must not decode as a scene"
    );
}

/// A layout without the trailing `tray_reflection` f32 (v40) does not decode. The reflectance
/// is the four bytes before the orientation's five in a scene that carries one.
#[test]
fn a_v40_scene_byte_layout_does_not_decode_as_v41() {
    let bytes = postcard::to_allocvec(&sample_scene()).unwrap();
    let v40 =
        &bytes[..bytes.len() - REFLECTION_TAIL - ORIENTATION_TAIL - ESTIMATOR_TAIL - PAN_TAIL];
    assert!(
        postcard::from_bytes::<SceneState>(v40).is_err(),
        "the old layout must not decode as a scene"
    );

    let mut mirrored = sample_scene();
    mirrored.tray_reflection = 0.04;
    let mirrored_bytes = postcard::to_allocvec(&mirrored).unwrap();
    assert_eq!(mirrored_bytes.len(), bytes.len(), "same layout");
    let reflection_end = mirrored_bytes.len() - ORIENTATION_TAIL - ESTIMATOR_TAIL - PAN_TAIL;
    assert_eq!(
        &mirrored_bytes[reflection_end - REFLECTION_TAIL..reflection_end],
        &0.04_f32.to_le_bytes()
    );
    assert_eq!(
        postcard::from_bytes::<SceneState>(&mirrored_bytes).unwrap(),
        mirrored
    );
    let mut json = serde_json::to_value(&mirrored).unwrap();
    json.as_object_mut().unwrap().remove("tray_reflection");
    assert!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .tray_reflection
            .abs()
            < f32::EPSILON,
        "an old scene.json reads as the matte tray"
    );
}

/// A layout without the trailing `hdr_orientation` (v41) does not decode. In a scene that
/// carries one, the orientation is the five bytes before the estimator's byte. A JSON scene
/// without the key reads as an unturned map, and an unturned scene's JSON has no key.
#[test]
fn a_v41_scene_byte_layout_does_not_decode_as_v42() {
    use pleochroma::renderer::env_map::HdrOrientation;

    let plain = sample_scene();
    let bytes = postcard::to_allocvec(&plain).unwrap();
    let orientation_end = bytes.len() - ESTIMATOR_TAIL - PAN_TAIL;
    assert_eq!(
        &bytes[orientation_end - ORIENTATION_TAIL..orientation_end],
        &[0, 0, 0, 0, 0]
    );
    let v41 = &bytes[..orientation_end - ORIENTATION_TAIL];
    assert!(
        postcard::from_bytes::<SceneState>(v41).is_err(),
        "the old layout must not decode as a scene"
    );

    let mut turned = sample_scene();
    turned.hdr_orientation = HdrOrientation {
        yaw: 2.5,
        follows_camera: true,
    };
    let turned_bytes = postcard::to_allocvec(&turned).unwrap();
    assert_eq!(turned_bytes.len(), bytes.len(), "same layout");
    let turned_end = turned_bytes.len() - ESTIMATOR_TAIL - PAN_TAIL;
    let tail = &turned_bytes[turned_end - ORIENTATION_TAIL..turned_end];
    assert_eq!(&tail[..4], &2.5_f32.to_le_bytes());
    assert_eq!(tail[4], 1);
    let decoded: SceneState = postcard::from_bytes(&turned_bytes).unwrap();
    assert_eq!(decoded, turned);
    assert_eq!(decoded.hdr_orientation.yaw.to_bits(), 2.5_f32.to_bits());

    let plain_json = serde_json::to_value(&plain).unwrap();
    assert!(
        plain_json.get("hdr_orientation").is_none(),
        "an unturned scene's JSON has no key"
    );
    let mut json = serde_json::to_value(&turned).unwrap();
    assert!(json.get("hdr_orientation").is_some());
    assert_eq!(
        serde_json::from_value::<SceneState>(json.clone()).unwrap(),
        turned
    );
    json.as_object_mut().unwrap().remove("hdr_orientation");
    assert_eq!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .hdr_orientation,
        HdrOrientation::default(),
        "an old scene.json reads as an unturned map"
    );
}

/// A layout without the trailing frosted-plane list (v25) does not decode. Postcard cannot
/// skip a missing trailing field, so the protocol version check in the handshake protects
/// peers. A scene with frosted planes round-trips, and the planes only add bytes.
#[test]
fn scene_state_frosted_planes_round_trip_and_a_v25_layout_does_not_decode() {
    let plain = sample_scene();
    assert_eq!(plain.frosted_planes, Vec::<u32>::new());
    let plain_bytes = postcard::to_allocvec(&plain).unwrap();
    let before_effects =
        &plain_bytes[..plain_bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL];
    assert_eq!(&before_effects[before_effects.len() - 3..], &[0, 8, 1]);
    let v25 = &before_effects[..before_effects.len() - 4];
    assert!(
        postcard::from_bytes::<SceneState>(v25).is_err(),
        "the old layout must not decode as a scene"
    );

    let mut frosted = sample_scene();
    frosted.frosted_planes = vec![2, 5, 40];
    let bytes = postcard::to_allocvec(&frosted).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.frosted_planes, vec![2, 5, 40]);
    assert_eq!(frosted, decoded);
    assert!(
        bytes.len() > plain_bytes.len(),
        "frosted planes are a length-prefixed trailing Vec, so they only add bytes"
    );
}

/// The wire carries `LightingPreset` as its derived-serde variant index, which is the
/// declaration order, not the `ALL` / `index()` UI order. New presets are appended. This pins
/// every variant to its position, so a reorder of the declaration fails here.
#[test]
fn lighting_preset_postcard_index_is_the_declaration_order() {
    use LightingPreset::*;
    let declared = [
        Daylight,
        Incandescent,
        RingLights,
        DarkSpotlight,
        IsoHemisphere,
        LightTent,
        DaylightDome,
        UvLamp365,
        UvLamp395,
        DaylightSun,
        Aset,
        ShopLights,
        WindowDaylight,
        WhiteTray,
        IlluminantA,
    ];
    assert_eq!(declared.len(), LightingPreset::ALL.len());
    for (position, preset) in declared.into_iter().enumerate() {
        assert_eq!(
            postcard::to_allocvec(&preset).unwrap(),
            vec![u8::try_from(position).unwrap()],
            "{preset:?} must encode as its declaration position"
        );
        assert!(
            LightingPreset::ALL.contains(&preset),
            "{preset:?} is in ALL"
        );
    }
}

#[test]
fn scene_state_round_trip_preserves_every_lighting_preset() {
    for preset in LightingPreset::ALL {
        let mut scene = sample_scene();
        scene.lighting_preset = preset;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.lighting_preset, preset);
    }
}

#[test]
fn scene_state_round_trip_preserves_a_diamond_with_empty_absorption() {
    // The opposite corner case from Alexandrite: empty absorption bands and
    // biaxial_delta_beta_alpha = None must round-trip too.
    let mut scene = sample_scene();
    scene.material = GemMaterial::diamond();
    assert_eq!(
        scene.material.absorption.o_ray,
        Vec::new(),
        "diamond should have no absorption bands"
    );
    assert!(scene.material.biaxial_delta_beta_alpha.is_none());

    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(scene, decoded);
}

#[test]
fn scene_state_round_trip_preserves_an_empty_plane_set() {
    let mut scene = sample_scene();
    scene.planes = Vec::new();

    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(scene, decoded);
    assert_eq!(decoded.planes, Vec::new());
}

/// Both values of `girdle_frosted` (the viewer's frosted-girdle toggle) must round-trip, not
/// just the default.
#[test]
fn scene_state_round_trip_preserves_girdle_frosted_both_ways() {
    for girdle_frosted in [false, true] {
        let mut scene = sample_scene();
        scene.girdle_frosted = girdle_frosted;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.girdle_frosted, girdle_frosted);
        assert_eq!(scene, decoded);
    }
}

/// The environment round-trips through postcard in both variants. A `scene.json` without the
/// field loads as the studio rig (`#[serde(default)]`).
#[test]
fn scene_state_round_trip_preserves_the_environment_both_ways() {
    use pleochroma_net::scene::{HdrEnvironment, SceneEnvironment};
    let hdr = HdrEnvironment {
        content_hash: [0x5A; 32],
        width: 4096,
        height: 2048,
    };
    for environment in [SceneEnvironment::Studio, SceneEnvironment::Hdr(hdr)] {
        let mut scene = sample_scene();
        scene.environment = environment;
        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.environment, environment);
        assert_eq!(scene, decoded);
    }
    let mut scene = sample_scene();
    scene.environment = SceneEnvironment::Hdr(hdr);
    assert_eq!(scene.hdr(), Some(&hdr));
    scene.environment = SceneEnvironment::Studio;
    assert_eq!(scene.hdr(), None);
}

/// `surface_glare` round-trips at the default, at both ends of its `0.0..=1.0` range and at
/// an intermediate value. Its serde default is the unscaled `1.0`.
#[test]
fn scene_state_round_trip_preserves_surface_glare() {
    assert_eq!(
        pleochroma_net::scene::default_surface_glare().to_bits(),
        1.0f32.to_bits()
    );
    assert_eq!(sample_scene().surface_glare.to_bits(), 1.0f32.to_bits());
    for glare in [1.0f32, 0.0, 0.35] {
        let mut scene = sample_scene();
        scene.surface_glare = glare;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.surface_glare.to_bits(), glare.to_bits());
        assert_eq!(scene, decoded);
    }
}

/// `GemMaterial::absorption_path_scale` is embedded inside `SceneState::material`, not a
/// top-level field. The default (`1.0`) and a non-default scale must both survive the wire
/// unchanged.
#[test]
fn scene_state_round_trip_preserves_absorption_path_scale_both_default_and_scaled() {
    for scale in [1.0f32, 0.42, 2.75] {
        let mut scene = sample_scene();
        scene.material = scene.material.with_absorption_path_scale(scale);

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.material.absorption_path_scale, scale);
        assert_eq!(scene, decoded);
    }
}

// NOTE on `#[serde(default)]` and postcard: unlike JSON, a postcard-encoded struct has
// no field names or "present/absent" signal on the wire. A byte stream one field shorter
// than the current shape does not default the field; postcard reports
// `DeserializeUnexpectedEnd`. `#[serde(default)]` on `SceneState::girdle_frosted` serves
// `pleochroma-worker::render_cmd`'s on-disk `scene.json` (self-describing `serde_json`),
// not this crate's postcard wire format. The network path's cross-version protection is
// `PROTOCOL_VERSION`.

/// The head-shadow radius round-trips through postcard.
#[test]
fn scene_state_head_shadow_round_trips_through_postcard() {
    let mut scene = sample_scene();
    scene.head_shadow_deg = 22.5;
    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.head_shadow_deg.to_bits(), 22.5f32.to_bits());
}

#[test]
fn v28_appends_engine_and_old_layout_is_refused() {
    use pleochroma::renderer::engine::{RayOptions, RenderEngine};
    let mut scene = sample_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    scene.engine.ray = RayOptions {
        aa_taps: 16,
        fire: false,
    };
    let bytes = postcard::to_allocvec(&scene).unwrap();
    let before_effects = &bytes[..bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL];
    assert_eq!(&before_effects[before_effects.len() - 3..], &[1, 16, 0]);
    assert_eq!(postcard::from_bytes::<SceneState>(&bytes).unwrap(), scene);
    assert!(
        postcard::from_bytes::<SceneState>(&before_effects[..before_effects.len() - 3]).is_err()
    );
    let mut json = serde_json::to_value(&scene).unwrap();
    json.as_object_mut().unwrap().remove("engine");
    assert_eq!(
        serde_json::from_value::<SceneState>(json).unwrap().engine,
        pleochroma::renderer::engine::EngineSpec::default()
    );
}

/// The effects record sits between the engine and the camera roll. An untouched scene writes
/// exactly the default record, a busy one round-trips, a layout without the record is refused,
/// and a `scene.json` written before the field loads as "no effects".
#[test]
fn v32_appends_the_effects_record_and_old_layouts_are_refused() {
    use pleochroma::optics::{
        inclusions::spec::InclusionSpec,
        raytracer::surface::{FacetSurface, PolishPreset},
    };
    use pleochroma_net::scene::effects::{EdgeSpec, SceneEffects};

    let plain = sample_scene();
    assert!(plain.effects.is_none());
    let plain_bytes = postcard::to_allocvec(&plain).unwrap();
    let default_tail = postcard::to_allocvec(&SceneEffects::default()).unwrap();
    assert_eq!(default_tail.len(), EFFECTS_TAIL);
    assert_eq!(
        &plain_bytes[plain_bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL
            ..plain_bytes.len() - ROLL_TAIL - AFTER_ROLL_TAIL],
        default_tail
    );
    assert!(
        postcard::from_bytes::<SceneState>(
            &plain_bytes[..plain_bytes.len() - EFFECTS_TAIL - ROLL_TAIL - AFTER_ROLL_TAIL]
        )
        .is_err(),
        "the v31 layout must not decode as a scene"
    );

    let mut busy = sample_scene();
    busy.effects = SceneEffects {
        inclusions: Some(InclusionSpec {
            preset: "Rutile Silk".to_owned(),
            density: 0.5,
            turn_deg: Some(20.0),
            sheen: None,
            color: None,
        }),
        facet_surfaces: vec![FacetSurface::from_preset(PolishPreset::Fine, 0.3).packed(); 4],
        edge: EdgeSpec {
            width: 0.01,
            alpha: 0.3,
            halo_rq_nm: 0.0,
        },
        ..SceneEffects::default()
    };
    let bytes = postcard::to_allocvec(&busy).unwrap();
    assert_eq!(postcard::from_bytes::<SceneState>(&bytes).unwrap(), busy);
    assert!(bytes.len() > plain_bytes.len());

    let mut json = serde_json::to_value(&busy).unwrap();
    json.as_object_mut().unwrap().remove("effects");
    assert!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .effects
            .is_none()
    );
}

/// The worker traces `traced_material()`: the stored material when there are no effects, and
/// the same material with the design's silk when there are.
#[test]
fn traced_material_applies_the_effects_record() {
    use pleochroma::optics::inclusions::spec::InclusionSpec;
    let mut scene = sample_scene();
    scene.material = GemMaterial::sapphire();
    assert!(matches!(
        scene.traced_material(),
        std::borrow::Cow::Borrowed(_)
    ));
    scene.effects.inclusions = Some(InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 0.35,
        turn_deg: None,
        sheen: None,
        color: None,
    });
    assert!(scene.traced_material().inclusions.is_some());
    assert!(scene.material.inclusions.is_none());
}

#[test]
fn camera_roll_round_trips_and_legacy_json_defaults_to_zero() {
    let mut scene = sample_scene();
    scene.camera_roll = std::f32::consts::FRAC_PI_2;
    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded, scene);
    assert_eq!(
        &bytes[bytes.len() - ROLL_TAIL - AFTER_ROLL_TAIL..bytes.len() - AFTER_ROLL_TAIL],
        &scene.camera_roll.to_le_bytes()
    );
    assert!(
        postcard::from_bytes::<SceneState>(&bytes[..bytes.len() - ROLL_TAIL - AFTER_ROLL_TAIL])
            .is_err()
    );
    let mut json = serde_json::to_value(&scene).unwrap();
    json.as_object_mut().unwrap().remove("camera_roll");
    assert_eq!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .camera_roll,
        0.0
    );
    let mut changed = scene.clone();
    changed.camera_roll = 0.0;
    assert_ne!(changed, scene, "roll changes remote scene identity");
}

/// The camera pan is the last eight wire bytes (two `f32`). A scene without it does not decode,
/// a set pan survives postcard and JSON, a centred one leaves the JSON key out, and JSON without
/// the key reads as centred.
#[test]
fn v43_appends_the_camera_pan_as_the_last_bytes() {
    let plain = sample_scene();
    assert_eq!(plain.camera_pan, [0.0, 0.0]);
    let bytes = postcard::to_allocvec(&plain).unwrap();
    assert!(
        postcard::from_bytes::<SceneState>(&bytes[..bytes.len() - PAN_TAIL]).is_err(),
        "the layout without the pan must not decode as a scene"
    );
    assert!(
        serde_json::to_value(&plain)
            .unwrap()
            .get("camera_pan")
            .is_none(),
        "a centred scene's JSON has no key"
    );

    let mut panned = sample_scene();
    panned.camera_pan = [0.375, -1.25];
    let panned_bytes = postcard::to_allocvec(&panned).unwrap();
    assert_eq!(panned_bytes.len(), bytes.len(), "same layout");
    let tail = &panned_bytes[panned_bytes.len() - PAN_TAIL..];
    assert_eq!(&tail[..4], &0.375_f32.to_le_bytes());
    assert_eq!(&tail[4..], &(-1.25_f32).to_le_bytes());
    assert_eq!(
        &panned_bytes[..panned_bytes.len() - PAN_TAIL],
        &bytes[..bytes.len() - PAN_TAIL]
    );
    let decoded: SceneState = postcard::from_bytes(&panned_bytes).unwrap();
    assert_eq!(decoded, panned);
    assert_eq!(decoded.camera_pan[0].to_bits(), 0.375_f32.to_bits());

    let mut json = serde_json::to_value(&panned).unwrap();
    assert!(json.get("camera_pan").is_some());
    assert_eq!(
        serde_json::from_value::<SceneState>(json.clone()).unwrap(),
        panned
    );
    json.as_object_mut().unwrap().remove("camera_pan");
    assert_eq!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .camera_pan,
        [0.0, 0.0],
        "an old scene.json reads as centred"
    );
    assert_ne!(plain, panned, "the pan changes remote scene identity");
}

/// A custom material's own inclusions (stored families and play-of-color structure) travel in
/// the effects record, because the material's serde bytes skip them. They survive postcard and
/// `scene.json`, and the worker rebuilds a material that scatters like the sender's.
#[test]
fn v34_a_materials_own_inclusions_travel_beside_the_material() {
    use glam::Vec3;
    use pleochroma::optics::inclusions::{
        CrystalDir, CrystalFrame, Family, FamilyKind,
        bragg::{StructureKind, StructureSpec},
    };
    use pleochroma_net::scene::material_inclusions::MaterialInclusions;

    let families = [
        Family {
            kind: FamilyKind::Needle,
            dir: CrystalDir::AlongC,
            sigma: 0.2,
            p: 2,
            lobe: 0.05,
            rho: 0.0,
        },
        Family {
            kind: FamilyKind::Platelet,
            dir: CrystalDir::Tilted {
                tilt_deg: 60.0,
                turn_deg: 30.0,
            },
            sigma: 0.1,
            p: 1,
            lobe: 0.2,
            rho: 0.0,
        },
    ];
    let structure = StructureSpec::default_for("Labradorite").expect("labradorite has a default");
    let local = GemMaterial::new_custom("My Silk", 1.76, 0.018, 0.008, [0.0; 3])
        .with_inclusions(CrystalFrame::new(Vec3::Y, Some(Vec3::X)).resolve(&families))
        .with_structure_spec_as(StructureKind::Labradorite, &structure);
    let local_set = local.inclusions.expect("the custom material carries a set");
    assert_eq!(local_set.families().len(), 2);
    assert!(local_set.structure().is_some());

    let mut scene = sample_scene();
    scene.material = local.clone();
    scene.effects.material_inclusions = MaterialInclusions::of_material(&local);
    assert!(scene.effects.material_inclusions.is_some());
    assert!(!scene.effects.is_none());
    assert!(scene.effects.validate(scene.planes.len()).is_ok());

    let same_scatterers = |traced: &GemMaterial| {
        let rebuilt = traced.inclusions.expect("the worker rebuilds the set");
        assert_eq!(rebuilt.families().len(), local_set.families().len());
        for (a, b) in rebuilt.families().iter().zip(local_set.families()) {
            assert_eq!((a.kind, a.p), (b.kind, b.p));
            assert_eq!((a.sigma, a.lobe, a.rho), (b.sigma, b.lobe, b.rho));
            assert!(
                (a.dir - b.dir).length() < 1e-6,
                "{:?} vs {:?}",
                a.dir,
                b.dir
            );
        }
        assert_eq!(rebuilt.structure(), local_set.structure());
    };

    // Postcard: the material's own bytes drop the set, the record brings it back.
    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert!(decoded.material.inclusions.is_none());
    assert_eq!(decoded.effects, scene.effects);
    same_scatterers(&decoded.traced_material());

    // scene.json (a job file): the same.
    let json = serde_json::to_string(&scene).unwrap();
    let from_json: SceneState = serde_json::from_str(&json).unwrap();
    assert_eq!(from_json.effects, scene.effects);
    same_scatterers(&from_json.traced_material());

    // A scene.json written before the field existed reads as "none carried".
    let mut value = serde_json::to_value(&scene).unwrap();
    value["effects"]
        .as_object_mut()
        .unwrap()
        .remove("material_inclusions");
    let old: SceneState = serde_json::from_value(value).unwrap();
    assert!(old.effects.material_inclusions.is_none());
}

/// The estimator is the wire byte before the pan (0 branching, 1 exit collection). A scene
/// without it does not decode, both values round-trip, and JSON writes no key for branching and
/// reads a missing one as branching.
#[test]
fn v42_appends_the_estimator_before_the_pan() {
    use pleochroma::optics::raytracer::Estimator;

    let plain = sample_scene();
    assert_eq!(plain.estimator, Estimator::Branching);
    let bytes = postcard::to_allocvec(&plain).unwrap();
    let estimator_at = bytes.len() - PAN_TAIL - ESTIMATOR_TAIL;
    assert_eq!(bytes[estimator_at], 0);
    assert!(
        postcard::from_bytes::<SceneState>(&bytes[..estimator_at]).is_err(),
        "the layout without the estimator must not decode as a scene"
    );

    let mut collect = sample_scene();
    collect.estimator = Estimator::ExitCollection;
    let collect_bytes = postcard::to_allocvec(&collect).unwrap();
    assert_eq!(collect_bytes.len(), bytes.len(), "same layout");
    assert_eq!(collect_bytes[estimator_at], 1);
    assert_eq!(&collect_bytes[..estimator_at], &bytes[..estimator_at]);
    assert_eq!(
        postcard::from_bytes::<SceneState>(&collect_bytes).unwrap(),
        collect
    );
    assert_eq!(postcard::from_bytes::<SceneState>(&bytes).unwrap(), plain);

    let plain_json = serde_json::to_value(&plain).unwrap();
    assert!(
        plain_json.get("estimator").is_none(),
        "a branching scene's JSON has no key"
    );
    let mut json = serde_json::to_value(&collect).unwrap();
    assert_eq!(json["estimator"], "ExitCollection");
    assert_eq!(
        serde_json::from_value::<SceneState>(json.clone()).unwrap(),
        collect
    );
    json.as_object_mut().unwrap().remove("estimator");
    assert_eq!(
        serde_json::from_value::<SceneState>(json)
            .unwrap()
            .estimator,
        Estimator::Branching,
        "an old scene.json reads as branching"
    );
}
