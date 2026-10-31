//! `Serialize` for [`SceneState`]: every field, in declaration order, except that the default
//! render-effects record is left out of human-readable output.
//!
//! The wire form (postcard) is positional: a reader decodes by field count, so every field is
//! always written there, the default record's 37 bytes included. A JSON job file or a debug dump
//! has no such need, and a scene without effects (almost every scene) reads better, and stays
//! byte-identical to a file written before the field existed, without an `"effects"` object. The
//! decoding side is the derived one: `#[serde(default)]` on the field reads a file without it.

use serde::ser::{Serialize, SerializeStruct, Serializer};

use super::{SceneState, effects::SceneEffects};

impl Serialize for SceneState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let skip_effects =
            serializer.is_human_readable() && self.effects == SceneEffects::default();
        let fields = if skip_effects { 22 } else { 23 };
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
