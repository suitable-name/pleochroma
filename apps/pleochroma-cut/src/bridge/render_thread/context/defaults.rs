//! Initial state of the live renderer.

use super::{PlanesOwner, RenderContext, scene_identity::SceneIdentity};
use crate::settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale};
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{
        DEFAULT_MAX_BOUNCES, DEFAULT_POSE, LightingPreset, surface::FacetingMachine,
    },
};
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::{Arc, atomic::AtomicBool};

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 800,
            height: 600,
            yaw: DEFAULT_POSE.yaw,
            pitch: DEFAULT_POSE.pitch,
            camera_roll: 0.0,
            distance: DEFAULT_POSE.distance,
            light_yaw: 0.85, // ~48 degrees azimuth
            // 72 degrees elevation, the `AppSettings` default.
            light_pitch: crate::settings::model::DEFAULT_LIGHT_PITCH_DEG.to_radians(),
            material_name: "Diamond".to_string(),
            material_override: None,
            view_body_color: None,
            view_color_pattern: None,
            material_linked: true,
            lighting_preset: LightingPreset::RingLights,
            backdrop: crate::settings::model::Backdrop::default(),
            surface_glare: 1.0,
            head_shadow_deg: pleochroma::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG,
            target_samples: 256,
            max_bounces: DEFAULT_MAX_BOUNCES,
            exposure: 1.0,
            inclusion_sigma_s: 0.0,
            c_axis_override: None,
            alpha_axis_override: None,
            girdle_frosted: false,
            edge_rounding_radius: 0.0,
            scene_effects: SceneEffects::default(),
            surface_table: Vec::new(),
            faceting_machine: FacetingMachine::DEFAULT,
            stone_width_mm: 0.0,
            active_planes: Arc::new(StandardGemCuts::standard_round_brilliant()),
            active_frosted: Arc::new(Vec::new()),
            active_tools: Arc::new(Vec::new()),
            active_placements: Vec::new(),
            design_gear: None,
            planes_owner: PlanesOwner::Builtin,
            editor_generation: None,
            catalogue_over_generation: None,
            material_unresolved: None,
            editor_material_stash: None,
            custom_materials: Arc::new(Vec::new()),
            custom_material_specific_gravity: Arc::new(Vec::new()),
            custom_material_physics: Arc::new(Vec::new()),
            custom_material_fluorescence: Arc::new(Vec::new()),
            pending_color_choice: None,
            running: true,
            dirty: true,
            paused: false,
            tab_visible: true,
            denoise_enabled: true,
            redisplay_requested: false,
            remote_active: false,
            live_epoch: None,
            export_active_count: 0,
            live_compute_target: LiveComputeTarget::Both,
            live_display_only: false,
            local_compute_target: LocalComputeTarget::CpuGpu,
            local_preview_scale: LocalPreviewScale::Off,
            camera_moving: false,
            camera_drag_held: false,
            env_map: None,
            scene_identity: SceneIdentity::default(),
            frame_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}
