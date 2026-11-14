//! Immutable resolved scene used by the local analytical pass for every beauty backend.
use crate::bridge::{
    frame_cache::stone_width::StoneWidthCache,
    render_thread::{
        MaterialOverrides, RenderContext, apply_material_overrides, resolve_material_with_override,
    },
};
use pleochroma::{
    geometry::{
        facet_finishes_with_frosted,
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, LightingPreset},
    },
    renderer::env_map::{EnvRotation, EnvironmentMap},
};
use std::sync::Arc;
#[derive(Clone)]
pub struct OpticalSnapshot {
    pub generation: u64,
    pub owner: crate::bridge::render_thread::PlanesOwner,
    pub labels: Option<Arc<Vec<String>>>,
    pub planes: Arc<Vec<GpuFacetPlane>>,
    pub tools: Arc<Vec<ToolPrimitive>>,
    /// The facet finishes the viewport draws: the authored frosted facets and the frosted
    /// girdle. Empty for a polished stone.
    pub finishes: Arc<Vec<FacetFinish>>,
    pub material: GemMaterial,
    pub pose: [f32; 3],
    pub camera_roll: f32,
    /// The live view's pan (`RenderContext::camera_pan`), so the overlay lies on the panned
    /// picture.
    pub camera_pan: [f32; 2],
    pub lighting: LightingPreset,
    pub light_pose: [f32; 2],
    pub head_shadow: f32,
    pub hdr: Option<Arc<EnvironmentMap>>,
    /// The turn of [`Self::hdr`] at the captured camera yaw (the map's yaw, plus the camera's
    /// when the map follows it); the identity without a map.
    pub hdr_rotation: EnvRotation,
    pub limitations: String,
}
impl OpticalSnapshot {
    /// Why the diagnostic pass cannot run for this scene, if it cannot. Cheap: the
    /// controller asks every tick, before it decides whether a cached capture is reusable.
    pub fn refusal(ctx: &RenderContext) -> Option<String> {
        if let Some(reason) = &ctx.material_unresolved {
            return Some(reason.clone());
        }
        if ctx.active_planes.is_empty() {
            return Some("No solved geometry".into());
        }
        if ctx.env_map.is_none() && ctx.lighting_preset == LightingPreset::Aset {
            return Some(
                "ASET already uses diagnostic colors; select another lighting preset".into(),
            );
        }
        None
    }
    /// Captures the scene. `width_cache` is the controller's persistent cache: a
    /// per-millimeter material measures the stone once per design, not once per capture.
    pub fn capture(
        ctx: &mut RenderContext,
        width_cache: &mut StoneWidthCache,
    ) -> Result<Self, String> {
        if let Some(reason) = Self::refusal(ctx) {
            return Err(reason);
        }
        let base = resolve_material_with_override(
            GemMaterial::built_in(),
            &ctx.custom_materials,
            ctx.tinted_material_override().as_ref(),
            &ctx.material_name,
        )
        .ok_or("Material cannot be resolved")?;
        let material = apply_material_overrides(
            base,
            &MaterialOverrides {
                inclusion_sigma_s: ctx.inclusion_sigma_s,
                c_axis_override: ctx.c_axis_override,
                alpha_axis_override: ctx.alpha_axis_override,
                edge_rounding_radius: ctx.edge_rounding_radius,
                stone_width_mm: ctx.stone_width_mm,
            },
            &ctx.active_planes,
            width_cache,
        );
        let mut notices =
            vec!["Analytical light paths; ten-bounce estimate. Not spectral pixel attribution."];
        if !material.absorption.o_ray.is_empty()
            || !material.absorption.e_ray.is_empty()
            || material.zoning.is_some()
        {
            notices.push("Body absorption and color zoning are not included.");
        }
        if material.birefringence_delta != 0.0 || material.biaxial_delta_beta_alpha.is_some() {
            notices.push("Scalar dispersion; polarization and birefringence are not included.");
        }
        if material.scattering_sigma_s > 0.0 || ctx.active_fluorescence().is_some() {
            notices.push("Scattering and fluorescence are not included.");
        }
        if material.edge_rounding_radius > 0.0 {
            notices.push("Sharp interfaces; rounding is not included.");
        }
        if ctx.env_map.is_some()
            || ctx.lighting_preset.model() == pleochroma::optics::raytracer::LightingModel::Studio
        {
            notices.push("The analytical observer shadow uses the standard cone for Studio/HDR.");
        }
        notices.push("Surface glare is not part of these internal light-path classes.");
        Ok(Self {
            generation: ctx.scene_generation(),
            owner: ctx.planes_owner,
            labels: super::labels::captured_labels(ctx),
            planes: ctx.active_planes.clone(),
            tools: ctx.active_tools.clone(),
            finishes: Arc::new(facet_finishes_with_frosted(
                &ctx.active_planes,
                ctx.girdle_frosted,
                &ctx.active_frosted,
            )),
            material,
            pose: [ctx.yaw, ctx.pitch, ctx.distance],
            camera_roll: ctx.camera_roll,
            camera_pan: ctx.camera_pan,
            lighting: ctx.lighting_preset,
            light_pose: [ctx.light_yaw, ctx.light_pitch],
            head_shadow: ctx.head_shadow_deg,
            hdr: ctx.env_map.clone(),
            hdr_rotation: ctx.env_map.as_ref().map_or(EnvRotation::IDENTITY, |_| {
                ctx.hdr_orientation().rotation_for(ctx.yaw)
            }),
            limitations: notices.join(" "),
        })
    }
    pub fn camera(&self) -> Camera {
        Camera::new(self.pose[0], self.pose[1], self.pose[2], DEFAULT_FOV_DEG)
            .with_roll(self.camera_roll)
            .with_pan(self.camera_pan)
    }
    pub fn geometry(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: &self.planes,
            tools: &self.tools,
        }
    }
    /// Unit exposure: illumination qualification is relative to environment ambient.
    /// Backdrop and surface glare are left at their defaults: the metrics ignore both
    /// (`color::metrics::lighting`), so they are not part of the optical input either.
    pub fn environment(&self) -> EnvironmentSource<'_> {
        self.hdr.as_deref().map_or_else(
            || {
                self.lighting
                    .studio(1.0, self.light_pose[0], self.light_pose[1])
                    .with_head_shadow(self.head_shadow)
            },
            |map| EnvironmentSource::hdr(map).with_hdr_rotation(self.hdr_rotation),
        )
    }
    /// Optical identity excludes presentation, exposure, beauty sample and bounce targets.
    pub fn same_input(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.pose.map(f32::to_bits) == other.pose.map(f32::to_bits)
            && self.camera_roll.to_bits() == other.camera_roll.to_bits()
            && self.camera_pan.map(f32::to_bits) == other.camera_pan.map(f32::to_bits)
            && self.material == other.material
            && self.lighting == other.lighting
            && self.light_pose.map(f32::to_bits) == other.light_pose.map(f32::to_bits)
            && self.head_shadow.to_bits() == other.head_shadow.to_bits()
            && self.hdr_rotation == other.hdr_rotation
            && (Arc::ptr_eq(&self.planes, &other.planes) || self.planes == other.planes)
            && (Arc::ptr_eq(&self.tools, &other.tools) || self.tools == other.tools)
            && (Arc::ptr_eq(&self.finishes, &other.finishes) || self.finishes == other.finishes)
            && match (&self.hdr, &other.hdr) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(ctx: &mut RenderContext) -> OpticalSnapshot {
        OpticalSnapshot::capture(ctx, &mut StoneWidthCache::new()).unwrap()
    }
    #[test]
    fn exposure_and_sample_targets_reuse_data_but_lighting_material_and_geometry_do_not() {
        let mut ctx = RenderContext::default();
        let a = capture(&mut ctx);
        ctx.exposure *= 2.0;
        ctx.target_samples += 1;
        let redisplay = capture(&mut ctx);
        assert!(a.same_input(&redisplay));
        assert_ne!(a.generation, redisplay.generation);
        ctx.head_shadow_deg = 32.0;
        let shadow = capture(&mut ctx);
        assert!(!a.same_input(&shadow));
        ctx.material_override = Some(GemMaterial::sapphire());
        let material = capture(&mut ctx);
        assert!(!shadow.same_input(&material));
        ctx.active_tools = Arc::new(vec![ToolPrimitive::cylinder(
            glam::Vec3::ZERO,
            glam::Vec3::X,
            0.1,
            1.0,
        )]);
        let tools = capture(&mut ctx);
        assert!(!material.same_input(&tools));
    }
    /// The overlay lies on the panned picture: the pan is captured, moves the camera's eye and
    /// is part of the optical input.
    #[test]
    fn a_pan_is_part_of_the_optical_input_and_the_camera() {
        let mut ctx = RenderContext::default();
        let centred = capture(&mut ctx);
        assert_eq!(centred.camera_pan, [0.0, 0.0]);
        ctx.camera_pan = [0.3, -0.2];
        let panned = capture(&mut ctx);
        assert_eq!(panned.camera_pan, [0.3, -0.2]);
        assert!(!centred.same_input(&panned));
        assert!(panned.same_input(&capture(&mut ctx)));
        assert_ne!(centred.camera().origin, panned.camera().origin);
        assert_eq!(centred.camera().forward, panned.camera().forward);
    }
    /// The overlay is scored under the turned map: another yaw is another optical input, and a
    /// map that follows the camera carries the camera's yaw in.
    #[test]
    fn a_turned_hdr_map_is_part_of_the_optical_input() {
        let mut ctx = RenderContext {
            env_map: Some(Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))),
            ..RenderContext::default()
        };
        let plain = capture(&mut ctx);
        assert!(plain.hdr_rotation.is_identity());
        ctx.hdr_yaw = 0.7;
        let turned = capture(&mut ctx);
        assert!(!turned.hdr_rotation.is_identity());
        assert!(!plain.same_input(&turned));
        ctx.hdr_yaw = 0.0;
        ctx.hdr_follows_camera = true;
        ctx.yaw = 0.7;
        let following = capture(&mut ctx);
        assert_eq!(following.hdr_rotation, turned.hdr_rotation);
    }
    #[test]
    fn the_snapshot_carries_the_finishes_the_viewport_draws() {
        let mut ctx = RenderContext::default();
        let polished = capture(&mut ctx);
        assert!(
            polished.finishes.is_empty(),
            "no frosting, the all-polished form"
        );
        ctx.girdle_frosted = true;
        let girdle = capture(&mut ctx);
        assert_eq!(
            *girdle.finishes,
            facet_finishes_with_frosted(&ctx.active_planes, true, &[])
        );
        assert!(girdle.finishes.contains(&FacetFinish::Frosted));
        ctx.girdle_frosted = false;
        ctx.active_frosted = Arc::new(vec![1, 3]);
        let facets = capture(&mut ctx);
        assert_eq!(
            *facets.finishes,
            facet_finishes_with_frosted(&ctx.active_planes, false, &[1, 3])
        );
        assert_eq!(facets.finishes[1], FacetFinish::Frosted);
        assert_eq!(facets.finishes[3], FacetFinish::Frosted);
        assert!(
            !polished.same_input(&girdle),
            "a frosted girdle is another stone"
        );
        assert!(!girdle.same_input(&facets));
        assert!(
            !polished.same_input(&facets),
            "frosted facets are another stone"
        );
    }
    #[test]
    fn frosting_is_scored_so_only_rounding_is_listed_as_not_included() {
        let mut ctx = RenderContext {
            girdle_frosted: true,
            active_frosted: Arc::new(vec![2]),
            ..RenderContext::default()
        };
        let text = capture(&mut ctx).limitations;
        assert!(!text.contains("frosting"), "{text}");
        assert!(!text.contains("rounding"), "{text}");
        ctx.edge_rounding_radius = 0.05;
        let rounded = capture(&mut ctx).limitations;
        assert!(rounded.contains("rounding is not included"), "{rounded}");
        assert!(!rounded.contains("frosting"), "{rounded}");
    }
    #[test]
    fn backdrop_and_glare_do_not_change_the_optical_input() {
        let mut ctx = RenderContext::default();
        let a = capture(&mut ctx);
        ctx.surface_glare = 0.25;
        ctx.backdrop = crate::settings::model::Backdrop::White;
        let b = capture(&mut ctx);
        assert_ne!(a.generation, b.generation, "the beauty scene did change");
        assert!(
            a.same_input(&b),
            "the metrics read neither backdrop nor glare"
        );
    }
    #[test]
    fn capture_reuses_the_width_cache() {
        let mut ctx = RenderContext::default();
        let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        ctx.material_override = Some(banded);
        let mut cache = StoneWidthCache::new();
        let fresh = format!("{cache:?}");
        let first = OpticalSnapshot::capture(&mut ctx, &mut cache).unwrap();
        let measured = format!("{cache:?}");
        assert_ne!(
            measured, fresh,
            "a per-millimeter material measures the stone into the controller's cache"
        );
        ctx.exposure *= 2.0;
        let second = OpticalSnapshot::capture(&mut ctx, &mut cache).unwrap();
        assert_eq!(
            format!("{cache:?}"),
            measured,
            "the same planes are served from the cache"
        );
        assert!(first.same_input(&second));
    }
    #[test]
    fn the_limitations_text_uses_american_spelling() {
        let mut ctx = RenderContext::default();
        let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        ctx.material_override = Some(banded);
        let text = capture(&mut ctx).limitations;
        assert!(text.contains("color zoning"), "{text}");
        assert!(!text.to_lowercase().contains("colour"), "{text}");
        ctx.lighting_preset = LightingPreset::Aset;
        let refusal = OpticalSnapshot::refusal(&ctx).expect("ASET is refused");
        assert!(refusal.contains("colors"), "{refusal}");
        assert!(!refusal.to_lowercase().contains("colour"), "{refusal}");
    }
}
