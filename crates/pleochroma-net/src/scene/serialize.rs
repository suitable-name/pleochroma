//! `Serialize` for [`SceneState`]: every field, in declaration order, except that the default
//! render-effects record, a `false` `stone_shadow`, a `0.0` `tray_reflection`, an unturned
//! `hdr_orientation`, a `Branching` `estimator`, a centred `camera_pan` and an identity
//! `stone_pose` are left out of human-readable output.
//!
//! The wire form (postcard) is positional: a reader decodes by field count, so every field is
//! always written there, the default record's bytes, the shadow flag's byte, the
//! reflection's four bytes, the orientation's five, the estimator's one and the pan's eight
//! and the pose's twelve included. A JSON job file or a debug dump has no such need, and a scene without effects, a
//! stone shadow, a tray reflection, a turned HDR map, the exit collection estimator or a pan
//! (almost every scene) reads better, and stays byte-identical to a file written before the
//! fields existed, without an `"effects"` object or a `"stone_shadow"` / `"tray_reflection"` /
//! `"hdr_orientation"` / `"estimator"` / `"camera_pan"` / `"stone_pose"` key. The decoding side is the derived
//! one: `#[serde(default)]` on each field reads a file without it.

use serde::ser::{Serialize, SerializeStruct, Serializer};

use super::{SceneState, effects::SceneEffects};
use pleochroma::optics::raytracer::Estimator;

impl Serialize for SceneState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let skip_effects =
            serializer.is_human_readable() && self.effects == SceneEffects::default();
        let skip_shadow = serializer.is_human_readable() && !self.stone_shadow;
        let skip_reflection = serializer.is_human_readable() && self.tray_reflection == 0.0;
        let skip_orientation = serializer.is_human_readable() && !self.hdr_orientation.is_turned();
        let skip_estimator =
            serializer.is_human_readable() && self.estimator == Estimator::Branching;
        let skip_pan = serializer.is_human_readable() && self.camera_pan == [0.0, 0.0];
        let skip_pose = serializer.is_human_readable() && self.stone_pose == [0.0, 0.0, 0.0];
        let fields = 29
            - usize::from(skip_pose)
            - usize::from(skip_effects)
            - usize::from(skip_shadow)
            - usize::from(skip_reflection)
            - usize::from(skip_orientation)
            - usize::from(skip_estimator)
            - usize::from(skip_pan);
        let mut state = serializer.serialize_struct("SceneState", fields)?;
        state.serialize_field("width", &self.width)?;
        state.serialize_field("height", &self.height)?;
        state.serialize_field("yaw", &self.yaw)?;
        state.serialize_field("pitch", &self.pitch)?;
        state.serialize_field("distance", &self.distance)?;
        state.serialize_field("light_yaw", &self.light_yaw)?;
        state.serialize_field("light_pitch", &self.light_pitch)?;
        state.serialize_field("exposure", &self.exposure)?;
        state.serialize_field("max_bounces", &self.max_bounces)?;
        state.serialize_field("lighting_preset", &self.lighting_preset)?;
        state.serialize_field("material", &self.material)?;
        state.serialize_field("planes", &self.planes)?;
        state.serialize_field("girdle_frosted", &self.girdle_frosted)?;
        state.serialize_field("backdrop", &self.backdrop)?;
        state.serialize_field("environment", &self.environment)?;
        state.serialize_field("surface_glare", &self.surface_glare)?;
        state.serialize_field("tools", &self.tools)?;
        state.serialize_field("fluorescence", &self.fluorescence)?;
        state.serialize_field("head_shadow_deg", &self.head_shadow_deg)?;
        state.serialize_field("frosted_planes", &self.frosted_planes)?;
        state.serialize_field("engine", &self.engine)?;
        if skip_effects {
            state.skip_field("effects")?;
        } else {
            state.serialize_field("effects", &self.effects)?;
        }
        state.serialize_field("camera_roll", &self.camera_roll)?;
        if skip_shadow {
            state.skip_field("stone_shadow")?;
        } else {
            state.serialize_field("stone_shadow", &self.stone_shadow)?;
        }
        if skip_reflection {
            state.skip_field("tray_reflection")?;
        } else {
            state.serialize_field("tray_reflection", &self.tray_reflection)?;
        }
        if skip_orientation {
            state.skip_field("hdr_orientation")?;
        } else {
            state.serialize_field("hdr_orientation", &self.hdr_orientation)?;
        }
        if skip_estimator {
            state.skip_field("estimator")?;
        } else {
            state.serialize_field("estimator", &self.estimator)?;
        }
        if skip_pan {
            state.skip_field("camera_pan")?;
        } else {
            state.serialize_field("camera_pan", &self.camera_pan)?;
        }
        if skip_pose {
            state.skip_field("stone_pose")?;
        } else {
            state.serialize_field("stone_pose", &self.stone_pose)?;
        }
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::effects::EdgeSpec;
    use pleochroma::optics::{materials::GemMaterial, raytracer::LightingPreset};

    fn scene(effects: SceneEffects) -> SceneState {
        SceneState {
            width: 8,
            height: 6,
            yaw: 0.1,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
            estimator: pleochroma::optics::raytracer::Estimator::Branching,
            pitch: 0.2,
            distance: 3.0,
            light_yaw: 0.3,
            light_pitch: 0.4,
            exposure: 1.0,
            max_bounces: 8,
            lighting_preset: LightingPreset::default(),
            material: GemMaterial::diamond(),
            planes: Vec::new(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: crate::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            effects,
            camera_pan: [0.0, 0.0],
            stone_pose: [0.0, 0.0, 0.0],
        }
    }

    /// A scene with no effects writes no `effects` key to JSON, reads back unchanged, and a
    /// scene with effects writes and reads them.
    #[test]
    fn json_leaves_out_the_default_effects_and_keeps_a_set_one() {
        let plain = scene(SceneEffects::default());
        let text = serde_json::to_string(&plain).expect("serialize");
        assert!(!text.contains("\"effects\""), "{text}");
        let back: SceneState = serde_json::from_str(&text).expect("an old-style file reads");
        assert_eq!(back, plain);

        let edged = scene(SceneEffects {
            edge: EdgeSpec {
                width: 0.01,
                alpha: 0.3,
                halo_rq_nm: 0.0,
            },
            ..SceneEffects::default()
        });
        let text = serde_json::to_string(&edged).expect("serialize");
        assert!(text.contains("\"effects\""), "{text}");
        let back: SceneState = serde_json::from_str(&text).expect("reads");
        assert_eq!(back, edged);
    }

    /// The appended bit is mandatory on postcard and omitted only from plain JSON.
    #[test]
    fn json_leaves_out_the_default_shadow_and_keeps_a_set_one() {
        let mut plain = scene(SceneEffects::default());
        let before = serde_json::to_string(&plain).expect("plain JSON");
        assert!(!before.contains("stone_shadow"));
        assert_eq!(
            serde_json::from_str::<SceneState>(&before).expect("legacy JSON"),
            plain
        );
        let bytes = postcard::to_allocvec(&plain).expect("plain wire");
        // The shadow byte sits before the reflection's four bytes, the orientation's five,
        // the estimator's one and the pan's eight and the pose's twelve.
        assert_eq!(bytes[bytes.len() - 31], 0);
        plain.stone_shadow = true;
        let shadowed = serde_json::to_string(&plain).expect("shadow JSON");
        assert!(shadowed.contains("\"stone_shadow\":true"));
        assert_eq!(
            serde_json::from_str::<SceneState>(&shadowed).expect("shadow JSON"),
            plain
        );
        let shadow_bytes = postcard::to_allocvec(&plain).expect("shadow wire");
        assert_eq!(shadow_bytes[shadow_bytes.len() - 31], 1);
        assert_eq!(
            &bytes[..bytes.len() - 31],
            &shadow_bytes[..shadow_bytes.len() - 31]
        );
        assert_eq!(
            postcard::from_bytes::<SceneState>(&shadow_bytes).expect("shadow wire"),
            plain
        );
    }

    /// The appended reflectance is four mandatory bytes on postcard and omitted only from plain
    /// JSON at `0.0`.
    #[test]
    fn json_leaves_out_the_default_reflection_and_keeps_a_set_one() {
        let mut plain = scene(SceneEffects::default());
        let before = serde_json::to_string(&plain).expect("plain JSON");
        assert!(!before.contains("tray_reflection"));
        assert_eq!(
            serde_json::from_str::<SceneState>(&before).expect("legacy JSON"),
            plain
        );
        let bytes = postcard::to_allocvec(&plain).expect("plain wire");
        assert_eq!(
            &bytes[bytes.len() - 30..bytes.len() - 26],
            &0.0_f32.to_le_bytes()
        );
        plain.tray_reflection = 0.25;
        let mirrored = serde_json::to_string(&plain).expect("mirror JSON");
        assert!(mirrored.contains("\"tray_reflection\":0.25"));
        assert_eq!(
            serde_json::from_str::<SceneState>(&mirrored).expect("mirror JSON"),
            plain
        );
        let mirror_bytes = postcard::to_allocvec(&plain).expect("mirror wire");
        assert_eq!(mirror_bytes.len(), bytes.len(), "same layout");
        assert_eq!(
            &mirror_bytes[mirror_bytes.len() - 30..mirror_bytes.len() - 26],
            &0.25_f32.to_le_bytes()
        );
        assert_eq!(
            &bytes[..bytes.len() - 30],
            &mirror_bytes[..mirror_bytes.len() - 30]
        );
        assert_eq!(
            postcard::from_bytes::<SceneState>(&mirror_bytes).expect("mirror wire"),
            plain
        );
    }

    /// The appended orientation is five mandatory bytes on postcard (the yaw's four and the
    /// follow flag, then the estimator's one and the pan's eight) and omitted from plain JSON
    /// until the map is turned.
    #[test]
    fn json_leaves_out_the_unturned_orientation_and_keeps_a_turned_one() {
        use pleochroma::renderer::env_map::HdrOrientation;

        let mut plain = scene(SceneEffects::default());
        let before = serde_json::to_string(&plain).expect("plain JSON");
        assert!(!before.contains("hdr_orientation"));
        assert_eq!(
            serde_json::from_str::<SceneState>(&before).expect("legacy JSON"),
            plain
        );
        let bytes = postcard::to_allocvec(&plain).expect("plain wire");
        assert_eq!(&bytes[bytes.len() - 26..bytes.len() - 21], &[0, 0, 0, 0, 0]);

        for orientation in [
            HdrOrientation {
                yaw: 1.25,
                follows_camera: false,
            },
            HdrOrientation {
                yaw: 0.0,
                follows_camera: true,
            },
            HdrOrientation {
                yaw: 5.5,
                follows_camera: true,
            },
        ] {
            plain.hdr_orientation = orientation;
            let turned = serde_json::to_string(&plain).expect("turned JSON");
            assert!(turned.contains("\"hdr_orientation\""), "{turned}");
            let back: SceneState = serde_json::from_str(&turned).expect("turned JSON");
            assert_eq!(back, plain);
            assert_eq!(
                back.hdr_orientation.yaw.to_bits(),
                orientation.yaw.to_bits()
            );

            let turned_bytes = postcard::to_allocvec(&plain).expect("turned wire");
            assert_eq!(turned_bytes.len(), bytes.len(), "same layout");
            let tail = &turned_bytes[turned_bytes.len() - 26..turned_bytes.len() - 21];
            assert_eq!(&tail[..4], &orientation.yaw.to_le_bytes());
            assert_eq!(tail[4], u8::from(orientation.follows_camera));
            assert_eq!(
                &bytes[..bytes.len() - 26],
                &turned_bytes[..turned_bytes.len() - 26]
            );
            let wire: SceneState = postcard::from_bytes(&turned_bytes).expect("turned wire");
            assert_eq!(wire, plain);
            assert_eq!(
                wire.hdr_orientation.yaw.to_bits(),
                orientation.yaw.to_bits()
            );
        }
    }

    /// JSON written before the estimator existed has no key and reads as branching.
    #[test]
    fn json_without_the_estimator_key_reads_as_branching() {
        let mut plain = scene(SceneEffects::default());
        plain.estimator = Estimator::ExitCollection;
        let mut value = serde_json::to_value(&plain).expect("collect JSON");
        assert!(
            value
                .as_object_mut()
                .expect("an object")
                .remove("estimator")
                .is_some()
        );
        let back: SceneState = serde_json::from_value(value).expect("legacy JSON reads");
        assert_eq!(back.estimator, Estimator::Branching);
    }

    /// Branching writes no key to JSON, exit collection writes its name, and both read back.
    #[test]
    fn json_writes_no_key_for_branching_and_the_name_for_collect() {
        let mut plain = scene(SceneEffects::default());
        let branching = serde_json::to_string(&plain).expect("branching JSON");
        assert!(!branching.contains("estimator"), "{branching}");
        assert_eq!(
            serde_json::from_str::<SceneState>(&branching).expect("branching JSON"),
            plain
        );
        plain.estimator = Estimator::ExitCollection;
        let collect = serde_json::to_string(&plain).expect("collect JSON");
        assert!(
            collect.contains("\"estimator\":\"ExitCollection\""),
            "{collect}"
        );
        assert_eq!(
            serde_json::from_str::<SceneState>(&collect).expect("collect JSON"),
            plain
        );
    }

    /// The estimator is the wire byte before the pan's eight: 0 for branching, 1 for exit
    /// collection, and both round trip through postcard with the rest of the scene unchanged.
    #[test]
    fn postcard_carries_the_estimator_before_the_pan() {
        let mut plain = scene(SceneEffects::default());
        let branching = postcard::to_allocvec(&plain).expect("branching wire");
        assert_eq!(branching[branching.len() - 21], 0);
        assert_eq!(
            postcard::from_bytes::<SceneState>(&branching).expect("branching wire"),
            plain
        );
        plain.estimator = Estimator::ExitCollection;
        let collect = postcard::to_allocvec(&plain).expect("collect wire");
        assert_eq!(collect.len(), branching.len(), "same layout");
        assert_eq!(collect[collect.len() - 21], 1);
        assert_eq!(
            &branching[..branching.len() - 21],
            &collect[..collect.len() - 21]
        );
        assert_eq!(
            postcard::from_bytes::<SceneState>(&collect).expect("collect wire"),
            plain
        );
    }

    /// The appended pan is the last eight wire bytes and is omitted from plain JSON while
    /// centred; a JSON scene without the key reads as centred.
    #[test]
    fn json_leaves_out_the_centred_pan_and_keeps_a_set_one() {
        let mut plain = scene(SceneEffects::default());
        let before = serde_json::to_string(&plain).expect("plain JSON");
        assert!(!before.contains("camera_pan"), "{before}");
        assert_eq!(
            serde_json::from_str::<SceneState>(&before).expect("legacy JSON"),
            plain
        );
        let bytes = postcard::to_allocvec(&plain).expect("plain wire");
        assert_eq!(&bytes[bytes.len() - 20..bytes.len() - 12], &[0_u8; 8]);

        plain.camera_pan = [0.5, -0.25];
        let panned = serde_json::to_string(&plain).expect("pan JSON");
        assert!(panned.contains("\"camera_pan\":[0.5,-0.25]"), "{panned}");
        assert_eq!(
            serde_json::from_str::<SceneState>(&panned).expect("pan JSON"),
            plain
        );
        let pan_bytes = postcard::to_allocvec(&plain).expect("pan wire");
        assert_eq!(pan_bytes.len(), bytes.len(), "same layout");
        assert_eq!(
            &pan_bytes[pan_bytes.len() - 20..pan_bytes.len() - 16],
            &0.5_f32.to_le_bytes()
        );
        assert_eq!(
            &pan_bytes[pan_bytes.len() - 16..pan_bytes.len() - 12],
            &(-0.25_f32).to_le_bytes()
        );
        assert_eq!(
            &bytes[..bytes.len() - 20],
            &pan_bytes[..pan_bytes.len() - 20]
        );
        assert_eq!(
            postcard::from_bytes::<SceneState>(&pan_bytes).expect("pan wire"),
            plain
        );
    }

    /// The appended pose is the last twelve wire bytes and is omitted from plain JSON at
    /// identity; a JSON scene without the key reads as identity.
    #[test]
    fn json_leaves_out_the_identity_pose_and_keeps_a_set_one() {
        let mut plain = scene(SceneEffects::default());
        let before = serde_json::to_string(&plain).expect("plain JSON");
        assert!(!before.contains("stone_pose"), "{before}");
        assert_eq!(
            serde_json::from_str::<SceneState>(&before)
                .expect("legacy JSON")
                .stone_pose,
            [0.0, 0.0, 0.0]
        );
        let bytes = postcard::to_allocvec(&plain).expect("plain wire");
        assert_eq!(&bytes[bytes.len() - 12..], &[0_u8; 12]);

        plain.stone_pose = [35.0, 120.5, 270.0];
        let posed = serde_json::to_string(&plain).expect("pose JSON");
        assert!(
            posed.contains("\"stone_pose\":[35.0,120.5,270.0]"),
            "{posed}"
        );
        assert_eq!(
            serde_json::from_str::<SceneState>(&posed).expect("pose JSON"),
            plain
        );
        let pose_bytes = postcard::to_allocvec(&plain).expect("pose wire");
        assert_eq!(pose_bytes.len(), bytes.len(), "same layout");
        assert_eq!(
            &pose_bytes[pose_bytes.len() - 12..pose_bytes.len() - 8],
            &35.0_f32.to_le_bytes()
        );
        assert_eq!(
            &bytes[..bytes.len() - 12],
            &pose_bytes[..pose_bytes.len() - 12]
        );
        assert_eq!(
            postcard::from_bytes::<SceneState>(&pose_bytes).expect("pose wire"),
            plain
        );
    }

    /// A design's color variant is already resolved into `SceneState::material` (no protocol
    /// field of its own): a recolored stone is a different scene, and a remote worker or a job
    /// file reads back exactly the per-axis tensor and unit the variant gave it.
    #[test]
    fn a_color_variant_material_is_part_of_the_scene_and_survives_both_encodings() {
        use pleochroma::optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            materials::AbsorptionUnit,
        };

        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(550.0, 40.0, 0.35)],
            vec![AbsorptionBand::energy(430.0, 1500.5, 0.12)],
        );
        let mut plain = scene(SceneEffects::default());
        plain.material = GemMaterial::sapphire();
        let mut recolored = plain.clone();
        recolored.material = recolored
            .material
            .with_color_variant_tensor(tensor.clone(), AbsorptionUnit::PerMm);
        assert_ne!(recolored, plain, "the variant is a different scene");

        let plain_text = serde_json::to_string(&plain).expect("serialize");
        let recolored_text = serde_json::to_string(&recolored).expect("serialize");
        assert_ne!(plain_text, recolored_text, "a job file names the variant");
        let back: SceneState = serde_json::from_str(&recolored_text).expect("reads");
        assert_eq!(back.material.absorption, tensor);
        assert_eq!(back.material.absorption_unit, AbsorptionUnit::PerMm);

        let bytes = postcard::to_allocvec(&recolored).expect("encode");
        let back: SceneState = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(back, recolored, "the wire carries the variant exactly");
    }

    /// The wire form always carries the record, so its layout does not depend on the value.
    #[test]
    fn the_wire_form_always_writes_the_effects_record() {
        let plain = scene(SceneEffects::default());
        let bytes = postcard::to_allocvec(&plain).expect("encode");
        let back: SceneState = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(back, plain);
        let edged = scene(SceneEffects {
            edge: EdgeSpec {
                width: 0.01,
                alpha: 0.3,
                halo_rq_nm: 5.0,
            },
            ..SceneEffects::default()
        });
        let longer = postcard::to_allocvec(&edged).expect("encode");
        assert_eq!(longer.len(), bytes.len(), "same layout, different values");
    }
}
