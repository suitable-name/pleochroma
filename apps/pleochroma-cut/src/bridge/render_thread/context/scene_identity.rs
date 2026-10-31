//! [`SceneIdentity`]: a content-derived scene generation counter for `RenderContext`.
//!
//! Every backend contributing to one live image must render the IDENTICAL scene
//! (invariant 2 of the hybrid guide). A settle's remote epoch is dispatched from a
//! scene captured on the UI thread, while the render thread reads the scene again on
//! its next frame; a scene change landing in between must never be merged into that
//! epoch. `RenderContext::scene_generation` turns "the scene the render thread / the
//! dispatch is looking at right now" into a number: it bumps whenever any
//! scene-defining field differs from the last time it was asked, so two readers that
//! get the same generation saw the same scene. The live epoch carries the generation
//! it was dispatched for; the render loop only claims from and merges with an epoch
//! whose generation matches the scene it is about to trace.
//!
//! No call site has to remember to bump anything: the key compares field CONTENT (and
//! `Arc` identity for the large shared buffers), so every writer of a scene field is
//! covered automatically.

use super::RenderContext;
use crate::settings::model::Backdrop;
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::LightingPreset},
    renderer::env_map::EnvironmentMap,
};
use pleochroma_cut_core::material::pattern::ColorPattern;
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::Arc;

/// Everything a remote `SceneState` (and the local tracer) is built from, captured for
/// comparison. Floats are compared by bit pattern so a `NaN` can never make a key
/// unequal to itself; the big shared buffers by `Arc` identity (a writer always
/// replaces the `Arc` when their content changes).
struct SceneKey {
    engine: pleochroma::renderer::engine::EngineSpec,
    planes_owner: super::PlanesOwner,
    dims: [u32; 3],
    floats: [u32; 12],
    c_axis: Option<[u32; 3]>,
    /// The Second Axis override, as bit patterns like [`Self::c_axis`].
    alpha_axis: Option<[u32; 3]>,
    girdle_frosted: bool,
    lighting_preset: LightingPreset,
    backdrop: Backdrop,
    material_name: String,
    material_override: Option<GemMaterial>,
    /// The view-only body colour that actually applies (`None` while it is dormant), triple
    /// and band rows as bit patterns -- see `RenderContext::effective_view_body_color`.
    view_color: Option<ViewColorBits>,
    view_pattern: Option<ColorPattern>,
    active_planes: Arc<Vec<GpuFacetPlane>>,
    /// Compared by value: a claim that finds the list unchanged keeps the `Arc`, and a
    /// changed list is a changed scene either way.
    active_frosted: Arc<Vec<u32>>,
    active_tools: Arc<Vec<ToolPrimitive>>,
    custom_materials: Arc<Vec<GemMaterial>>,
    fluorescence: Option<Arc<Fluorescence>>,
    env_map: Option<Arc<EnvironmentMap>>,
    /// Compared by value: the record is empty (no allocation) for every stone without render
    /// physics, and small otherwise.
    scene_effects: SceneEffects,
}

const fn dims(ctx: &RenderContext) -> [u32; 3] {
    [ctx.width, ctx.height, ctx.max_bounces]
}

const fn floats(ctx: &RenderContext) -> [u32; 12] {
    [
        ctx.yaw.to_bits(),
        ctx.pitch.to_bits(),
        ctx.camera_roll.to_bits(),
        ctx.distance.to_bits(),
        ctx.light_yaw.to_bits(),
        ctx.light_pitch.to_bits(),
        ctx.exposure.to_bits(),
        ctx.inclusion_sigma_s.to_bits(),
        ctx.edge_rounding_radius.to_bits(),
        ctx.stone_width_mm.to_bits(),
        ctx.surface_glare.to_bits(),
        ctx.head_shadow_deg.to_bits(),
    ]
}

fn c_axis(ctx: &RenderContext) -> Option<[u32; 3]> {
    ctx.c_axis_override
        .map(|v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
}

fn alpha_axis(ctx: &RenderContext) -> Option<[u32; 3]> {
    ctx.alpha_axis_override
        .map(|v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
}

/// An effective view color as bit patterns: the triple AND the band rows, so a change of
/// only the rows (same triple) is still a changed scene.
#[derive(Debug, PartialEq, Eq)]
struct ViewColorBits {
    triple: [u32; 3],
    bands: Option<Vec<[u32; 3]>>,
    /// The view color variant's identity (material, key and every tensor number), so a new
    /// variant or a changed peak restarts the scene like a changed triple does.
    variant: Option<Vec<u32>>,
}

fn view_color_bits(ctx: &RenderContext) -> Option<ViewColorBits> {
    ctx.effective_view_body_color().map(|color| ViewColorBits {
        triple: color.triple.map(f32::to_bits),
        bands: color
            .band_rows()
            .map(|rows| rows.iter().map(|row| row.map(f32::to_bits)).collect()),
        variant: color
            .variant
            .as_ref()
            .map(super::view_color::ViewVariant::identity_words),
    })
}

fn same_fluorescence(a: Option<&Arc<Fluorescence>>, b: Option<&Arc<Fluorescence>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

fn same_env(a: Option<&Arc<EnvironmentMap>>, b: Option<&Arc<EnvironmentMap>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

impl SceneKey {
    fn of(ctx: &RenderContext) -> Self {
        Self {
            engine: ctx.engine,
            planes_owner: ctx.planes_owner,
            dims: dims(ctx),
            floats: floats(ctx),
            c_axis: c_axis(ctx),
            alpha_axis: alpha_axis(ctx),
            girdle_frosted: ctx.girdle_frosted,
            lighting_preset: ctx.lighting_preset,
            backdrop: ctx.backdrop,
            material_name: ctx.material_name.clone(),
            material_override: ctx.material_override.clone(),
            view_color: view_color_bits(ctx),
            view_pattern: ctx.current_view_color_pattern().cloned(),
            active_planes: Arc::clone(&ctx.active_planes),
            active_frosted: Arc::clone(&ctx.active_frosted),
            active_tools: Arc::clone(&ctx.active_tools),
            custom_materials: Arc::clone(&ctx.custom_materials),
            fluorescence: ctx.active_fluorescence(),
            env_map: ctx.env_map.clone(),
            scene_effects: ctx.scene_effects.clone(),
        }
    }

    /// Whether `ctx`'s scene still equals this key -- compared in place, with no
    /// allocation but the few band rows of a view color that carries them, since the render
    /// loop asks every frame.
    fn matches(&self, ctx: &RenderContext) -> bool {
        self.engine == ctx.engine
            && self.planes_owner == ctx.planes_owner
            && self.dims == dims(ctx)
            && self.floats == floats(ctx)
            && self.c_axis == c_axis(ctx)
            && self.alpha_axis == alpha_axis(ctx)
            && self.girdle_frosted == ctx.girdle_frosted
            && self.lighting_preset == ctx.lighting_preset
            && self.backdrop == ctx.backdrop
            && self.material_name == ctx.material_name
            && self.material_override == ctx.material_override
            && self.view_color == view_color_bits(ctx)
            && self.view_pattern.as_ref() == ctx.current_view_color_pattern()
            && Arc::ptr_eq(&self.active_planes, &ctx.active_planes)
            && *self.active_frosted == *ctx.active_frosted
            && Arc::ptr_eq(&self.active_tools, &ctx.active_tools)
            && Arc::ptr_eq(&self.custom_materials, &ctx.custom_materials)
            && same_fluorescence(
                self.fluorescence.as_ref(),
                ctx.active_fluorescence().as_ref(),
            )
            && same_env(self.env_map.as_ref(), ctx.env_map.as_ref())
            && self.scene_effects == ctx.scene_effects
    }
}

/// The last scene key `RenderContext::scene_generation` saw and its generation number
/// -- see the module doc comment. `Default` starts at generation 0 with no key, so the
/// very first query always bumps to 1.
#[derive(Default)]
pub struct SceneIdentity {
    key: Option<SceneKey>,
    generation: u64,
}

impl RenderContext {
    /// The current scene's generation: unchanged while every scene-defining field is
    /// unchanged since the previous call, bumped otherwise. Called under the context
    /// lock by the render loop (every frame, via `snapshot_frame_inputs`), by the
    /// remote dispatch (stamped into the live epoch) and by the orchestrator's poll
    /// tick (scene-stability debounce for re-dispatch).
    pub fn scene_generation(&mut self) -> u64 {
        self.discard_stale_view_color_pattern();
        let unchanged = self
            .scene_identity
            .key
            .as_ref()
            .is_some_and(|key| key.matches(self));
        if !unchanged {
            self.scene_identity.key = Some(SceneKey::of(self));
            self.scene_identity.generation += 1;
        }
        self.scene_identity.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_the_image_engine_or_its_taps_restarts_the_scene() {
        let mut ctx = RenderContext::default();
        let spectral = ctx.scene_generation();
        ctx.engine.engine = pleochroma::renderer::engine::RenderEngine::RayTraced;
        let ray = ctx.scene_generation();
        assert!(ray > spectral);
        ctx.engine.ray.aa_taps = 16;
        assert!(ctx.scene_generation() > ray);
    }

    #[test]
    fn changing_only_the_view_color_band_rows_restarts_the_scene() {
        use super::super::ViewColor;

        let triple = [0.2f32, 0.4, 2.8];
        let banded = |amplitude: f32| ViewColor {
            triple,
            bands: Some(vec![[460.0, 45.0, amplitude]]),
            variant: None,
        };
        let mut ctx = RenderContext {
            material_linked: false,
            ..Default::default()
        };
        ctx.view_body_color = Some(ViewColor::triple_only(triple));
        let first = ctx.scene_generation();
        assert_eq!(ctx.scene_generation(), first, "nothing changed since");

        ctx.view_body_color = Some(banded(0.25));
        let second = ctx.scene_generation();
        assert!(
            second > first,
            "adding the rows to the same triple restarts"
        );
        assert_eq!(ctx.scene_generation(), second);

        ctx.view_body_color = Some(banded(0.30));
        let third = ctx.scene_generation();
        assert!(third > second, "changing only the rows restarts");
        assert_eq!(ctx.scene_generation(), third);
    }

    /// The Live Render view color can be a variant: picking one restarts the scene, repeating it
    /// does not, and moving one peak of the same variant restarts it again.
    #[test]
    fn a_view_color_variant_restarts_the_scene_and_so_does_a_changed_peak() {
        use super::super::ViewColor;
        use pleochroma::optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            materials::AbsorptionUnit,
        };
        use pleochroma_cut_core::material::VariantChoice;

        let variant = |peak: f32| {
            ViewColor::from_variant(
                "Sapphire",
                VariantChoice {
                    key: "sapphire.test".to_string(),
                    label: "Test".to_string(),
                    absorption: AbsorptionTensor::uniaxial(
                        vec![AbsorptionBand::new(550.0, 40.0, peak)],
                        vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
                    ),
                    unit: AbsorptionUnit::PerMm,
                    reference_path_mm: 7.0,
                },
            )
        };
        let mut ctx = RenderContext {
            material_linked: false,
            ..Default::default()
        };
        ctx.view_body_color = Some(ViewColor::triple_only([0.2, 0.4, 2.8]));
        let first = ctx.scene_generation();

        ctx.view_body_color = Some(variant(0.2));
        let second = ctx.scene_generation();
        assert!(second > first, "a variant view color restarts");
        assert_eq!(ctx.scene_generation(), second, "nothing changed since");

        ctx.view_body_color = Some(variant(0.25));
        let third = ctx.scene_generation();
        assert!(
            third > second,
            "a changed peak of the same variant restarts"
        );
        assert_eq!(ctx.scene_generation(), third);
    }

    /// A design's color variant reaches the context inside the resolved `material_override`
    /// (`MaterialSelection::apply_overrides` recolors the absorption tensor, the name stays), so
    /// the scene key sees it through the material it compares by value: picking a variant, and
    /// moving one band of the same variant, each start a new scene; repeating one does not.
    #[test]
    fn a_color_variant_on_the_material_override_restarts_the_scene() {
        use pleochroma::optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            materials::AbsorptionUnit,
        };

        let recolored = |peak: f32| {
            GemMaterial::sapphire().with_color_variant_tensor(
                AbsorptionTensor::uniaxial(
                    vec![AbsorptionBand::new(550.0, 40.0, peak)],
                    vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
                ),
                AbsorptionUnit::PerMm,
            )
        };
        let mut ctx = RenderContext {
            material_name: "Sapphire".to_string(),
            material_override: Some(GemMaterial::sapphire()),
            ..Default::default()
        };
        let plain = ctx.scene_generation();
        assert_eq!(ctx.scene_generation(), plain, "nothing changed since");

        ctx.material_override = Some(recolored(0.35));
        let picked = ctx.scene_generation();
        assert!(picked > plain, "picking a variant restarts the scene");
        ctx.material_override = Some(recolored(0.35));
        assert_eq!(
            ctx.scene_generation(),
            picked,
            "the same variant again is no change"
        );

        ctx.material_override = Some(recolored(0.36));
        assert!(
            ctx.scene_generation() > picked,
            "another amplitude of the same band is another scene"
        );
    }

    #[test]
    fn a_new_second_axis_restarts_the_scene() {
        let mut ctx = RenderContext::default();
        let first = ctx.scene_generation();
        ctx.alpha_axis_override = Some(Vec3::X);
        let second = ctx.scene_generation();
        assert!(
            second > first,
            "a Second Axis change must bump the generation"
        );
        assert_eq!(ctx.scene_generation(), second);
        ctx.alpha_axis_override = None;
        assert!(ctx.scene_generation() > second);
    }

    #[test]
    fn an_unchanged_scene_keeps_its_generation() {
        let mut ctx = RenderContext::default();
        let first = ctx.scene_generation();
        assert_eq!(ctx.scene_generation(), first);
        // Non-scene fields (visibility, pause, compute targets) never bump it.
        ctx.tab_visible = false;
        ctx.paused = true;
        ctx.dirty = true;
        ctx.camera_drag_held = true;
        assert_eq!(ctx.scene_generation(), first);
    }

    #[test]
    fn surface_glare_restarts_like_a_lighting_change_and_one_is_the_default() {
        let mut ctx = RenderContext::default();
        assert_eq!(ctx.surface_glare.to_bits(), 1.0f32.to_bits());
        let first = ctx.scene_generation();
        // Writing the default value back is not a scene change.
        ctx.surface_glare = 1.0;
        assert_eq!(ctx.scene_generation(), first);
        ctx.surface_glare = 0.0;
        let second = ctx.scene_generation();
        assert!(second > first, "a glare change must bump the generation");
        ctx.lighting_preset = LightingPreset::Daylight;
        assert!(
            ctx.scene_generation() > second,
            "a lighting change bumps it the same way"
        );
    }

    #[test]
    fn a_changed_planes_owner_bumps_the_generation() {
        let mut ctx = RenderContext::default();
        let owner = |generation| super::super::PlanesOwner::Editor { generation };
        ctx.planes_owner = owner(1);
        let first = ctx.scene_generation();
        assert_eq!(ctx.scene_generation(), first, "the same owner is no change");
        ctx.planes_owner = owner(2);
        assert!(
            ctx.scene_generation() > first,
            "another owner of the same planes is another scene"
        );
    }

    #[test]
    fn a_changed_frosted_set_bumps_the_generation_and_an_unchanged_one_does_not() {
        let mut ctx = RenderContext::default();
        let first = ctx.scene_generation();
        ctx.claim_active_frosted(Vec::new());
        assert_eq!(
            ctx.scene_generation(),
            first,
            "no frosting claimed over no frosting is no change"
        );
        ctx.claim_active_frosted(vec![3, 4]);
        let second = ctx.scene_generation();
        assert!(second > first, "frosting a plane is a scene change");
        ctx.claim_active_frosted(vec![3, 4]);
        assert_eq!(
            ctx.scene_generation(),
            second,
            "the same frosted planes again is no change"
        );
        ctx.claim_active_frosted(vec![3]);
        assert!(ctx.scene_generation() > second, "so is a changed set");
    }

    #[test]
    fn a_changed_render_physics_record_bumps_the_generation() {
        let mut ctx = RenderContext::default();
        let first = ctx.scene_generation();
        ctx.scene_effects = SceneEffects::default();
        assert_eq!(
            ctx.scene_generation(),
            first,
            "the default record again is no change"
        );
        ctx.scene_effects.facet_surfaces = vec![5];
        let second = ctx.scene_generation();
        assert!(second > first, "a surface finish is a scene change");
        ctx.scene_effects.facet_surfaces = vec![6];
        assert!(ctx.scene_generation() > second, "so is another one");
    }

    #[test]
    fn every_kind_of_scene_change_bumps_the_generation() {
        let mut ctx = RenderContext::default();
        let mut last = ctx.scene_generation();
        let changes: [fn(&mut RenderContext); 12] = [
            |c| c.yaw += 0.1,
            |c| c.camera_roll = std::f32::consts::FRAC_PI_2,
            |c| c.exposure = 2.0,
            |c| c.material_name = "Spinel".to_string(),
            |c| c.material_override = Some(GemMaterial::diamond()),
            |c| c.active_planes = Arc::new(c.active_planes.as_ref().clone()),
            |c| {
                c.active_tools = Arc::new(vec![ToolPrimitive::ball(
                    glam::Vec3::new(0.0, 0.3, 0.0),
                    0.1,
                )]);
            },
            |c| c.env_map = Some(Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))),
            |c| c.girdle_frosted = true,
            |c| c.width = 640,
            |c| c.surface_glare = 0.5,
            |c| c.head_shadow_deg = 0.0,
        ];
        for change in changes {
            change(&mut ctx);
            let now = ctx.scene_generation();
            assert!(now > last, "a scene change must bump the generation");
            last = now;
        }
    }
}
