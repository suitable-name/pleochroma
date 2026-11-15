//! [`SceneSnapshot`]: a read-only capture of everything a render needs, taken out of the
//! live `RenderContext` under one short lock.

use crate::bridge::{
    frame_cache::stone_width::StoneWidthCache,
    render_effects::words_for_planes,
    render_thread::{
        MaterialOverrides, RenderContext, apply_material_overrides, resolve_material_with_override,
    },
};
use pleochroma::{
    geometry::{
        facet_finishes_with_frosted, girdle_facet_finishes,
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, FacetFinish, LightingPreset,
            environment::tray_plane_y,
            surface::{FacetSurface, ScratchBasis, SurfaceScene},
        },
    },
    render_setup::{StonePose, pose_stone},
    renderer::env_map::{EnvironmentMap, HdrOrientation},
};
use pleochroma_net::scene::{
    effects::{MachineSpec, SceneEffects},
    material_inclusions::MaterialInclusions,
};
use pleochroma_solid::preview::StoneGeometryBuf;
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

/// A read-only snapshot of everything a render needs, captured out of the live `RenderContext`
/// under one short lock.
///
/// It excludes `width`/`height` and the accumulation buffer: the export worker sizes its own buffer
/// from the requested export dimensions.
///
/// `Clone`: a preset fan-out clones one capture and overlays each preset's light, camera and
/// env-map fields (`gui::render_export::apply_preset_to_scene`). All fanned-out renders then
/// share one material, geometry and bounce cap.
#[derive(Clone)]
pub struct SceneSnapshot {
    /// Frozen renderer and its deterministic options.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// Explicit opt-in, frozen at export start; absent for clean renders/thumbnails.
    pub optical_diagnostics: Option<pleochroma::renderer::optical_overlay::OpticalPresentation>,
    /// Camera yaw.
    pub yaw: f32,
    /// Camera pitch.
    pub pitch: f32,
    /// Clockwise image-plane roll, frozen with the viewport pose.
    pub camera_roll: f32,
    /// Where the stone sits in the picture (`RenderContext::camera_pan`, image half-heights in the
    /// picture's axes), frozen with the viewport pose; `[0.0, 0.0]` is centred.
    pub camera_pan: [f32; 2],
    /// The stone's pose on the tray (`RenderContext::stone_pose`, the Live Render pose even when
    /// the capture is made from the Edit tab): the identity is the stone as designed. The planes,
    /// tools and material of this snapshot stay as designed; the local tracer turns them with
    /// [`Self::posed`], and a remote worker or a job file receives the pose beside them.
    pub stone_pose: StonePose,
    /// Camera distance from the stone.
    pub distance: f32,
    /// Light yaw.
    pub light_yaw: f32,
    /// Light pitch.
    pub light_pitch: f32,
    /// Gem material the stone is rendered with.
    pub material: GemMaterial,
    /// Lighting preset the render uses.
    pub lighting_preset: LightingPreset,
    /// Maximum number of ray bounces per path.
    pub max_bounces: u32,
    /// Exposure multiplier the renderer applies: `RenderContext::effective_exposure`, the
    /// cutter's exposure on top of the rig's metered reading; an HDR map is lit at the plain one.
    pub exposure: f32,
    /// Tray radiance at exposure 1 (`0.0` = As Lit) -- `RenderContext::backdrop` resolved
    /// through `Backdrop::resolve` at this snapshot's own lighting preset and light pose.
    /// Seen by every escape from the stone and by the camera alike; every pass-through
    /// (remote scene state, jobs, overlay, tilt preview) carries this value unchanged.
    pub backdrop: f32,
    /// Surface-glare scale (`RenderContext::surface_glare`, `0.0..=1.0`): exports follow
    /// the viewport's value the way they follow its lighting preset. Analytic presets
    /// only; an HDR map ignores it.
    pub surface_glare: f32,
    /// Head-shadow radius in degrees (`RenderContext::head_shadow_deg`, `0.0` = off): exports
    /// follow the viewport's value. Lit analytic presets only; an HDR map ignores it.
    pub head_shadow_deg: f32,
    /// Whether the stone casts its spectral shadow onto the backdrop paper.
    pub stone_shadow: bool,
    /// Head-on reflectance of the glossy tray under the stone (`RenderContext::tray_reflection`,
    /// `0.0..=1.0`, `0.0` = off): exports follow the viewport's value. Studio lights with a
    /// backdrop only; see [`Self::effective_tray_reflection`].
    pub tray_reflection: f32,
    /// Whether the stone is a closed, nondegenerate solid, so the tray plane under it exists.
    /// Resolved once at capture (and once per job scene) from [`Self::active_planes`] and
    /// [`Self::tools`], and only when the shadow or the reflection is asked for; `true`
    /// otherwise. A stone that is not closed renders its local part matte, and the scenes
    /// handed to a remote worker, a joined worker or a job carry the shadow off and the
    /// reflection `0` (see [`Self::remote_stone_shadow`]). Because it is resolved lazily, a
    /// writer of the shadow or the reflection after capture must re-resolve it: the saved
    /// lighting overlay (`apply_preset_to_scene`) is the second one.
    pub stone_closed: bool,
    /// Facet planes of the active design.
    pub active_planes: Vec<GpuFacetPlane>,
    /// The concave tools cut out of `active_planes` (`RenderContext::active_tools`);
    /// empty for a planar design, which is then exported exactly as before. A tool-bearing
    /// scene is traced by the GPU kernels with its tools (`scene_routes_to_gpu_with_tools`, up
    /// to `GPU_MAX_TOOL_PRIMITIVES`; beyond that on the CPU) and sent to a remote worker as
    /// `SceneState::tools`.
    pub tools: Vec<ToolPrimitive>,
    /// The material's fluorescent emitters (`RenderContext::active_fluorescence`, resolved
    /// from the physics recipe), empty for every non-fluorescent material, which then exports
    /// exactly as before. A fluorescent scene is traced on the CPU alone and sent to a remote
    /// worker as `SceneState::fluorescence`. An `Arc` so a fan-out's clones share the sampling
    /// tables the first trace builds.
    pub fluorescence: Arc<Fluorescence>,
    /// Frosted girdle and authored frosting: `girdle_facet_finishes(&active_planes)` when
    /// `RenderContext::girdle_frosted` was on at capture time, with the planes the design
    /// frosts (`RenderContext::active_frosted`, or the finished stone's own) marked `Frosted`
    /// too; empty when neither applies -- already resolved here (rather than a bare `bool`
    /// re-classified per batch) since `active_planes` never changes mid-export.
    /// `pleochroma::geometry::finishes_wire_form` splits it back into the pieces a remote
    /// `SceneState` carries.
    pub facet_finishes: Vec<FacetFinish>,
    /// A loaded HDR environment map, captured from `RenderContext::env_map` as the live
    /// viewport reads it. `None` means the analytic studio rig. The map is lit at
    /// [`Self::exposure`], as in `render_thread`, and the GPU traces it like any environment.
    ///
    /// `Arc`, not a bare `EnvironmentMap`: a decoded panorama can be tens of megabytes, and
    /// this snapshot is cloned once per fanned-out preset render.
    pub env_map: Option<Arc<EnvironmentMap>>,
    /// How [`Self::env_map`] is turned (`RenderContext::hdr_orientation`): its yaw and whether it
    /// turns with the camera. Captured only with a map and the default (unturned) otherwise, so a
    /// studio scene never carries a stale value. Every export, remote scene, job file and tilt
    /// frame reads it through [`Self::environment_at`], the way the live view does.
    pub hdr_orientation: HdrOrientation,
    /// The design's render physics (inclusions, play-of-color, facet surface finish, abraded
    /// edge) with the capturing user's faceting machine; the default record when the design
    /// sets none, which renders exactly as before. Local tracers take [`Self::traced_material`]
    /// and [`Self::surface_scene`]; a remote worker is sent the record as it is
    /// (`SceneState::effects`) and applies it to [`Self::material`] itself. Ray-Traced scenes
    /// carry the default record (the plain image engine ignores the physics).
    pub effects: SceneEffects,
}

impl SceneSnapshot {
    /// The frozen lighting with the backdrop resolved at the captured light pose, for the
    /// scene's own camera yaw ([`Self::environment_at`] at [`Self::yaw`]).
    pub(super) fn environment(&self) -> EnvironmentSource<'_> {
        self.environment_at(self.yaw)
    }

    /// The frozen lighting for a render seen from `cam_yaw`: an HDR map is turned by
    /// [`Self::hdr_orientation`] (its yaw, plus `cam_yaw` when it turns with the camera), so a
    /// tilt frame or an axis that orbits the stone gets its own rotation.
    pub(super) fn environment_at(&self, cam_yaw: f32) -> EnvironmentSource<'_> {
        self.env_map.as_deref().map_or_else(
            || {
                self.lighting_preset
                    .studio(self.exposure, self.light_yaw, self.light_pitch)
                    .with_backdrop(self.backdrop)
                    .with_surface_glare(self.surface_glare)
                    .with_head_shadow(self.head_shadow_deg)
            },
            |map| {
                EnvironmentSource::hdr_at(map, self.exposure)
                    .with_hdr_rotation(self.hdr_orientation.rotation_for(cam_yaw))
            },
        )
    }

    /// Whether this lighting can apply its saved Stone Shadow preference to the paper.
    #[must_use]
    pub fn stone_shadow_enabled(&self) -> bool {
        self.stone_shadow
            && self.env_map.is_none()
            && self.backdrop > 0.0
            && self.lighting_preset != LightingPreset::Aset
            && !self.lighting_preset.is_uv_lamp()
    }

    /// The tray reflectance this lighting can apply: [`Self::tray_reflection`] on a studio light
    /// with a backdrop (not As Lit, an HDR map, ASET or an ultraviolet lamp), otherwise `0.0`.
    /// [`Self::remote_tray_reflection`] is what the remote scenes carry.
    #[must_use]
    pub fn effective_tray_reflection(&self) -> f32 {
        if self.tray_reflection > 0.0
            && self.env_map.is_none()
            && self.backdrop > 0.0
            && self.lighting_preset != LightingPreset::Aset
            && !self.lighting_preset.is_uv_lamp()
        {
            self.tray_reflection
        } else {
            0.0
        }
    }

    /// Whether this lighting applies a tray reflection at all
    /// ([`Self::effective_tray_reflection`] above zero).
    #[must_use]
    pub fn tray_reflection_applies(&self) -> bool {
        self.effective_tray_reflection() > 0.0
    }

    /// The Stone Shadow flag every scene that leaves the program carries (remote worker,
    /// coordinator, joined worker, job file): [`Self::stone_shadow_enabled`] for a closed stone,
    /// off for a stone that is not closed, which the worker would refuse.
    #[must_use]
    pub fn remote_stone_shadow(&self) -> bool {
        self.stone_closed && self.stone_shadow_enabled()
    }

    /// The tray reflectance every scene that leaves the program carries:
    /// [`Self::effective_tray_reflection`] for a closed stone, `0.0` for a stone that is not
    /// closed, which the worker would refuse.
    #[must_use]
    pub fn remote_tray_reflection(&self) -> f32 {
        if self.stone_closed {
            self.effective_tray_reflection()
        } else {
            0.0
        }
    }

    /// Whether the stone bounds a closed solid, resolved once per scene. Only a scene that asks
    /// for the Stone Shadow or a Tray Reflection looks: the tray plane is the one both build on,
    /// so a stone without it has neither. Any other scene reports `true`.
    #[must_use]
    pub fn stone_closed_for(
        stone_shadow: bool,
        tray_reflection: f32,
        planes: &[GpuFacetPlane],
        tools: &[ToolPrimitive],
    ) -> bool {
        let asked = stone_shadow || tray_reflection > 0.0;
        !asked || tray_plane_y(StoneGeometry { planes, tools }).is_some()
    }

    /// This scene with its stone turned into [`Self::stone_pose`], the scene every local tracer,
    /// the stone shadow and the tray mirror read: the planes and tools rotated (the `d` values
    /// kept), [`Self::material`] replaced by the posed traced material (crystal axes, color zones
    /// and the inclusions, which then ride in the material), the effects that would apply an
    /// unrotated inclusion set cleared, and the scratch directions of the surface finish turned
    /// with the stone. The finishes are index-based and stay as captured, and the pose of the
    /// result is the identity, so nothing turns the stone twice.
    ///
    /// Borrowed and untouched for the identity pose, which keeps every picture, hash and golden
    /// as it was. A remote worker takes the pose from the original scene instead (the inclusions
    /// would not survive the wire, and the worker poses its own scene).
    #[must_use]
    pub fn posed(&self) -> Cow<'_, Self> {
        if self.stone_pose.is_identity() {
            return Cow::Borrowed(self);
        }
        let posed = pose_stone(
            self.stone_pose,
            &self.active_planes,
            &self.tools,
            &self.traced_material(),
        );
        let mut scene = self.clone();
        scene.active_planes = posed.planes;
        scene.tools = posed.tools;
        scene.material = posed.material;
        scene.effects.inclusions = None;
        scene.effects.inclusion_second_axis = None;
        scene.effects.structure = None;
        scene.effects.material_inclusions = None;
        scene.effects.scratch = ScratchBasis::new(posed.up, posed.side);
        scene.stone_pose = StonePose::IDENTITY;
        Cow::Owned(scene)
    }

    /// Clears [`Self::stone_pose`]: the scene of a tilt view, tilt curve or tilt video, which
    /// measure and show the stone as designed.
    pub const fn clear_stone_pose(&mut self) {
        self.stone_pose = StonePose::IDENTITY;
    }

    /// The material the tracer traces: [`Self::material`] with the design's inclusions and
    /// play-of-color applied (borrowed and untouched without them).
    #[must_use]
    pub fn traced_material(&self) -> Cow<'_, GemMaterial> {
        self.effects.apply_to_material(&self.material)
    }

    /// The facet surfaces of [`Self::effects`] as the tracer's list, parallel to the planes;
    /// empty for a stone without a surface finish. Pair with [`Self::surface_scene`].
    #[must_use]
    pub fn facet_surfaces(&self) -> Vec<FacetSurface> {
        self.effects.surfaces()
    }

    /// The surface input of a trace over `surfaces` ([`Self::facet_surfaces`], which the caller
    /// keeps alive): the ideal scene for a stone without a surface finish.
    #[must_use]
    pub fn surface_scene<'a>(&self, surfaces: &'a [FacetSurface]) -> SurfaceScene<'a> {
        self.effects.surface_scene(surfaces)
    }

    /// # Errors
    ///
    /// Refuses (returning the cutter-facing reason) rather than capturing a scene
    /// that would trace as the wrong stone: when `RenderContext::material_unresolved`
    /// is already set, or when -- despite that check -- neither `material_override`
    /// nor `material_name` resolves to a real material. Every caller must handle
    /// this by aborting the export/dispatch/tilt-video render, not by substituting
    /// a default material (see `resolve_material`'s own doc comment for why).
    pub fn capture(ctx: &Mutex<RenderContext>) -> Result<Self, String> {
        Self::capture_finished(ctx, None)
    }

    /// [`Self::capture`] for a scene that leaves the program (the high-resolution export,
    /// the tilt video, the tilt curves): `finished` is the whole stone to draw in place of
    /// the planes and concave tools the context holds, which follow the Cut slider.
    ///
    /// `None` -- and an empty `finished`, which would draw nothing -- keeps the context's
    /// own stone, so the live remote dispatch and every caller that wants what the viewport
    /// shows go through [`Self::capture`] unchanged. The substitute is applied before
    /// anything is derived from the planes (the stone-width scale of the material, the
    /// frosted-girdle classification), so the whole scene describes one stone.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::capture`].
    pub fn capture_finished(
        ctx: &Mutex<RenderContext>,
        finished: Option<&StoneGeometryBuf>,
    ) -> Result<Self, String> {
        Self::capture_with_engine(ctx, finished, None)
    }

    /// [`Self::capture_finished`] for a scene that will be rendered with `engine`, which can
    /// differ from the engine the view shows: the render physics (inclusions, silk,
    /// play-of-color, facet surfaces, abraded edge, machine) are captured when `engine` reads
    /// them (Path-Traced) and left out when it does not (Ray-Traced), whatever the view uses.
    /// The snapshot carries `engine`.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::capture`].
    pub fn capture_finished_for_engine(
        ctx: &Mutex<RenderContext>,
        finished: Option<&StoneGeometryBuf>,
        engine: pleochroma::renderer::engine::EngineSpec,
    ) -> Result<Self, String> {
        Self::capture_with_engine(ctx, finished, Some(engine))
    }

    /// The capture behind [`Self::capture_finished`] (`engine` is `None`: the view's) and
    /// [`Self::capture_finished_for_engine`].
    fn capture_with_engine(
        ctx: &Mutex<RenderContext>,
        finished: Option<&StoneGeometryBuf>,
        engine: Option<pleochroma::renderer::engine::EngineSpec>,
    ) -> Result<Self, String> {
        let finished = finished.filter(|stone| !stone.planes.is_empty());
        let guard = ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reason) = &guard.material_unresolved {
            return Err(reason.clone());
        }
        let engine = engine.unwrap_or(guard.engine);
        let materials = GemMaterial::all_materials();
        // The export uses the same RI/custom material override as the live viewport, so it
        // does not fall back to the by-name lookup. The override also carries the Live Render
        // toolbar's view-only color, so the export, tilt video and remote workers match the
        // cutter's choice.
        let material_override = guard.tinted_material_override();
        let Some(material) = resolve_material_with_override(
            &materials,
            &guard.custom_materials,
            material_override.as_ref(),
            &guard.material_name,
        ) else {
            return Err(format!(
                "'{}' is not a built-in preset or a saved custom material -- nothing to \
                 export.",
                guard.material_name
            ));
        };
        // The sliders and toggles are part of what the user sees, so the export carries them.
        // They are applied here, not in the shared `resolve_material`, so an export with
        // nothing dialled in stays identical to a plain by-name resolve. A fresh
        // `StoneWidthCache` is used because this runs once per export.
        //
        // `active_planes`/`tools` are the stone the scene draws, taken before the overrides
        // that measure the planes. The copy here is the one deep copy `capture` makes.
        let (active_planes, tools, frosted) = stone_drawn(&guard, finished);
        let material = apply_material_overrides(
            material,
            &MaterialOverrides {
                inclusion_sigma_s: guard.inclusion_sigma_s,
                c_axis_override: guard.c_axis_override,
                alpha_axis_override: guard.alpha_axis_override,
                edge_rounding_radius: guard.edge_rounding_radius,
                stone_width_mm: guard.stone_width_mm,
            },
            &active_planes,
            &mut StoneWidthCache::new(),
        );
        let facet_finishes = facet_finishes_for(&active_planes, &frosted, guard.girdle_frosted);
        // The render physics follow the engine the scene is rendered with, not the view's.
        // The material's own inclusions are not in its serde bytes, so they ride in the record.
        // A set the wire cannot represent is left out, and the remote guard keeps the render
        // local.
        let effects = effects_for(
            &guard,
            engine,
            finished.is_some(),
            &active_planes,
            &material,
        );
        let mut snapshot = Self {
            engine,
            optical_diagnostics: None,
            yaw: guard.yaw,
            pitch: guard.pitch,
            camera_roll: guard.camera_roll,
            camera_pan: guard.camera_pan,
            // The Live Render pose whichever tab the capture is made from: an export or a queued
            // job shows what the Live Render tab shows.
            stone_pose: guard.stone_pose.clamped(),
            distance: guard.distance,
            light_yaw: guard.light_yaw,
            light_pitch: guard.light_pitch,
            material,
            lighting_preset: guard.lighting_preset,
            max_bounces: guard.max_bounces,
            // The metered exposure the live view renders with, so an export, a render job and
            // the remote scene show what the viewport shows.
            exposure: guard.effective_exposure(),
            backdrop: guard.backdrop.resolve(
                guard.lighting_preset,
                guard.light_yaw,
                guard.light_pitch,
            ),
            surface_glare: guard.surface_glare,
            head_shadow_deg: guard.head_shadow_deg,
            stone_shadow: guard.stone_shadow,
            tray_reflection: guard.tray_reflection,
            // Resolved below, once the context's lock is released.
            stone_closed: true,
            active_planes,
            tools,
            fluorescence: guard.active_fluorescence().unwrap_or_default(),
            facet_finishes,
            // `Arc::clone`, not a deep copy of the decoded panorama.
            env_map: guard.env_map.clone(),
            // Only a loaded map is turned; a studio scene carries the default.
            hdr_orientation: if guard.env_map.is_some() {
                guard.hdr_orientation()
            } else {
                HdrOrientation::default()
            },
            effects,
        };
        drop(guard);
        // The B-rep is built outside the lock: every live dispatch and hover preview captures.
        snapshot.stone_closed = Self::stone_closed_for(
            snapshot.stone_shadow,
            snapshot.tray_reflection,
            &snapshot.active_planes,
            &snapshot.tools,
        );
        Ok(snapshot)
    }
}

/// The planes, concave tools and frosted plane indices the scene draws: the finished stone's
/// when the caller passed one, else the context's own.
fn stone_drawn(
    ctx: &RenderContext,
    finished: Option<&StoneGeometryBuf>,
) -> (Vec<GpuFacetPlane>, Vec<ToolPrimitive>, Vec<u32>) {
    finished.map_or_else(
        || {
            (
                ctx.active_planes.to_vec(),
                ctx.active_tools.to_vec(),
                ctx.active_frosted.to_vec(),
            )
        },
        |stone| {
            (
                stone.planes.clone(),
                stone.tools.clone(),
                stone.frosted.clone(),
            )
        },
    )
}

/// The finish of every facet. An empty `Vec` is the off position: `trace_spectral_ray_with_finish`
/// treats it as `trace_spectral_ray`, since every facet reads `FacetFinish::default()`.
fn facet_finishes_for(
    active_planes: &[GpuFacetPlane],
    frosted: &[u32],
    girdle_frosted: bool,
) -> Vec<FacetFinish> {
    if !frosted.is_empty() {
        facet_finishes_with_frosted(active_planes, girdle_frosted, frosted)
    } else if girdle_frosted {
        girdle_facet_finishes(active_planes)
    } else {
        Vec::new()
    }
}

/// The render physics for a scene rendered with `engine`; `substituted` is whether the stone is
/// the finished one, whose planes have their own facet words from the whole-stone table.
fn effects_for(
    ctx: &RenderContext,
    engine: pleochroma::renderer::engine::EngineSpec,
    substituted: bool,
    active_planes: &[GpuFacetPlane],
    material: &GemMaterial,
) -> SceneEffects {
    if engine.is_ray() {
        SceneEffects::default()
    } else {
        let mut effects = ctx.scene_effects.clone();
        if substituted {
            effects.facet_surfaces = words_for_planes(&ctx.surface_table, active_planes);
        }
        effects.machine = if effects.has_surface() {
            MachineSpec::from_machine(&ctx.faceting_machine)
        } else {
            MachineSpec::default()
        };
        effects.material_inclusions = MaterialInclusions::of_material(material);
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use pleochroma::renderer::env_map::EnvRotation;

    /// The captured orientation turns the HDR environment: the map's yaw alone while it is
    /// fixed in the world, plus the camera yaw of each call while it turns with the camera,
    /// and an unturned or studio scene stays on the identity rotation.
    #[test]
    fn environment_at_turns_an_hdr_map_by_its_orientation_and_the_camera_yaw() {
        let map = Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]));
        let capture = |yaw: f32, follows: bool, with_map: bool| {
            SceneSnapshot::capture(&Mutex::new(RenderContext {
                env_map: with_map.then(|| Arc::clone(&map)),
                hdr_yaw: yaw,
                hdr_follows_camera: follows,
                ..Default::default()
            }))
            .expect("the default material resolves")
        };

        let fixed = capture(0.7, false, true);
        assert_eq!(fixed.hdr_orientation.yaw.to_bits(), 0.7_f32.to_bits());
        for cam_yaw in [0.0, 0.4, 2.5] {
            assert_eq!(
                fixed.environment_at(cam_yaw).hdr_rotation(),
                EnvRotation::from_yaw(0.7),
                "a map fixed in the world ignores the camera yaw ({cam_yaw})"
            );
        }

        let follows = capture(0.7, true, true);
        assert!(follows.hdr_orientation.follows_camera);
        for cam_yaw in [0.0, 0.4, 2.5] {
            assert_eq!(
                follows.environment_at(cam_yaw).hdr_rotation(),
                EnvRotation::from_yaw(0.7 + cam_yaw),
                "a map that turns with the camera adds each call's yaw ({cam_yaw})"
            );
        }
        assert_ne!(
            follows.environment_at(0.4).hdr_rotation(),
            follows.environment_at(1.4).hdr_rotation(),
            "two axes get two rotations"
        );

        let unturned = capture(0.0, false, true);
        assert!(unturned.environment_at(1.0).hdr_rotation().is_identity());
        let studio = capture(0.7, true, false);
        assert_eq!(studio.hdr_orientation, HdrOrientation::default());
        assert!(studio.environment_at(1.0).hdr_rotation().is_identity());
    }

    /// The export resolves the effective material (RI override or unlisted custom material)
    /// as the live viewport does, not by a plain name lookup that cannot hold an override.
    #[test]
    fn capture_prefers_the_material_override_over_the_by_name_lookup() {
        let override_dispersion = pleochroma::optics::dispersion::DispersionModel::Cauchy {
            a: 1.62,
            b: 0.0,
            c: 0.0,
        };
        let mut overridden = GemMaterial::diamond();
        overridden.name = "Custom RI 1.62".to_string();
        overridden.dispersion = override_dispersion;

        let with_override = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            material_override: Some(overridden),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            with_override.material.dispersion, override_dispersion,
            "the override's flattened dispersion (its RI) must reach the exported \
             scene, not the by-name material's own dispersion curve"
        );

        let without_override = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            material_override: None,
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            without_override.material.dispersion,
            GemMaterial::diamond().dispersion,
            "with no override, the export keeps resolving by name exactly as before"
        );
    }

    /// The inclusion slider must reach the exported scene. Guards the override in
    /// `SceneSnapshot::capture` against a bare `resolve_material` call.
    #[test]
    fn capture_carries_the_inclusion_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            inclusion_sigma_s: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.scattering_sigma_s, 0.0,
            "the off position must leave the material untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            inclusion_sigma_s: 1.25,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.material.scattering_sigma_s, 1.25,
            "a dialled-in inclusion amount must reach the exported scene"
        );
        assert_eq!(
            on.material.scattering_g,
            GemMaterial::DEFAULT_SCATTERING_G,
            "anisotropy comes from the crate's default, matching the live path"
        );
    }

    /// Crystal-axis override must reach the export, but must leave an isotropic
    /// material's `c_axis` alone even when the override is on
    /// (`RenderContext::default().material_name` is "Diamond", isotropic).
    #[test]
    fn capture_carries_the_c_axis_override_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            c_axis_override: None,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.c_axis,
            GemMaterial::diamond().c_axis,
            "the off (\"as cut\") position must leave the material's own c_axis untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Sapphire".to_string(),
            c_axis_override: Some(Vec3::X),
            ..Default::default()
        }))
        .expect("Sapphire resolves");
        assert_eq!(
            on.material.c_axis,
            Vec3::X,
            "a dialled-in override on an anisotropic material must reach the exported scene"
        );

        let isotropic_guarded = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            c_axis_override: Some(Vec3::X),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            isotropic_guarded.material.c_axis,
            GemMaterial::diamond().c_axis,
            "an override dialled in for an isotropic material must be ignored, matching \
             apply_material_overrides's own guard"
        );
    }

    /// The Second Axis reaches the export for a biaxial material only: a uniaxial or an
    /// isotropic one keeps its own (unset) alpha axis, matching `apply_material_overrides`.
    #[test]
    fn capture_carries_the_second_axis_into_a_biaxial_scene_only() {
        let alpha = Vec3::Z;
        let biaxial = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Tanzanite".to_string(),
            c_axis_override: Some(Vec3::X),
            alpha_axis_override: Some(alpha),
            ..Default::default()
        }))
        .expect("Tanzanite resolves");
        assert_eq!(biaxial.material.alpha_axis, Some(alpha));

        let uniaxial = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Sapphire".to_string(),
            c_axis_override: Some(Vec3::X),
            alpha_axis_override: Some(alpha),
            ..Default::default()
        }))
        .expect("Sapphire resolves");
        assert_eq!(uniaxial.material.alpha_axis, None);

        let unset = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Tanzanite".to_string(),
            ..Default::default()
        }))
        .expect("Tanzanite resolves");
        assert_eq!(
            unset.material.alpha_axis, None,
            "no override leaves alpha alone"
        );
    }

    /// Edge-rounding's own seam guard, same shape as the inclusion test above.
    #[test]
    fn capture_carries_the_edge_rounding_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            edge_rounding_radius: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.edge_rounding_radius, 0.0,
            "the off position must leave the material untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            edge_rounding_radius: 0.02,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.material.edge_rounding_radius, 0.02,
            "a dialled-in edge-rounding radius must reach the exported scene"
        );
    }

    /// The off position must leave `absorption_path_scale` at the base material's
    /// default (`1.0`), and a dialled-in width must scale a per-model-unit material (the
    /// default Diamond) by `width / 7 mm` and a per-millimetre one by the ratio to the
    /// design's measured model-unit girdle width, matching
    /// `apply_material_overrides`'s computation exactly.
    #[test]
    fn capture_carries_the_stone_width_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_width_mm: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        let face_up = pleochroma::render_setup::MODEL_UNIT_FACE_UP_PATH;
        assert_eq!(
            off.material.absorption_path_scale,
            1.0 / face_up,
            "the off position gives a per-model-unit material the face-up calibration scale only"
        );

        let default_ctx = RenderContext::default();
        let model_width = pleochroma::geometry::stone_metrics::measure_solid(
            &default_ctx
                .active_planes
                .iter()
                .map(|p| {
                    (
                        glam::DVec3::new(
                            f64::from(p.normal[0]),
                            f64::from(p.normal[1]),
                            f64::from(p.normal[2]),
                        ),
                        -f64::from(p.d),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .expect("default active_planes must measure")
        .width_axis;

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_width_mm: 6.5,
            ..Default::default()
        }))
        .expect("default resolves");
        let expected_scale = 6.5_f32 / 7.0 / face_up;
        assert!(
            (on.material.absorption_path_scale - expected_scale).abs() < 1e-4,
            "a dialled-in stone width must reach the exported scene as the expected \
             absorption_path_scale: got {}, expected {expected_scale}",
            on.material.absorption_path_scale
        );

        // A per-millimetre (band) colour takes the design's model width, and 7 mm while no size
        // is set.
        let banded = pleochroma::optics::materials::GemMaterial::diamond()
            .with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        for (width_mm, expected_mm) in [(0.0_f32, 7.0_f64), (6.5, 6.5)] {
            let banded_on = SceneSnapshot::capture(&Mutex::new(RenderContext {
                stone_width_mm: width_mm,
                material_override: Some(banded.clone()),
                ..Default::default()
            }))
            .expect("the override resolves");
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f32 is the render precision of absorption_path_scale; the ratio is small"
            )]
            let expected = (expected_mm / model_width) as f32;
            assert!(
                (banded_on.material.absorption_path_scale - expected).abs() < 1e-4,
                "{width_mm} mm: got {}, expected {expected}",
                banded_on.material.absorption_path_scale
            );
        }
    }

    /// The Stone Shadow toggle reaches the export snapshot, and `stone_shadow_enabled` drops it
    /// when the backdrop is As Lit.
    #[test]
    fn capture_carries_the_stone_shadow_to_the_export() {
        let ctx = Mutex::new(RenderContext::default());
        let capture = || SceneSnapshot::capture(&ctx).expect("the default stone resolves");
        assert!(!capture().stone_shadow_enabled());
        ctx.lock().expect("context").stone_shadow = true;
        assert!(capture().stone_shadow_enabled());
        ctx.lock().expect("context").backdrop = crate::settings::model::Backdrop::AsLit;
        assert!(!capture().stone_shadow_enabled());
    }

    /// The Tray Reflection reaches the export snapshot, and `effective_tray_reflection` drops it
    /// when the backdrop is As Lit.
    #[test]
    fn capture_carries_the_tray_reflection_to_the_export() {
        let ctx = Mutex::new(RenderContext::default());
        let capture = || SceneSnapshot::capture(&ctx).expect("the default stone resolves");
        assert_eq!(capture().effective_tray_reflection().to_bits(), 0);
        assert!(!capture().tray_reflection_applies());
        ctx.lock().expect("context").tray_reflection = 0.04;
        let on = capture();
        assert_eq!(on.tray_reflection.to_bits(), 0.04f32.to_bits());
        assert_eq!(on.effective_tray_reflection().to_bits(), 0.04f32.to_bits());
        assert!(on.tray_reflection_applies());
        ctx.lock().expect("context").backdrop = crate::settings::model::Backdrop::AsLit;
        assert_eq!(capture().effective_tray_reflection().to_bits(), 0);
        assert!(!capture().tray_reflection_applies());
    }

    /// A stone that is not closed keeps the toggles for the local matte fallback, but the scenes
    /// that leave the program carry the shadow off and the reflection `0`.
    #[test]
    fn a_stone_that_is_not_closed_sends_no_shadow_and_no_reflection() {
        let degenerate = Mutex::new(RenderContext {
            stone_shadow: true,
            tray_reflection: 0.04,
            active_planes: Arc::new(Vec::new()),
            ..Default::default()
        });
        let open = SceneSnapshot::capture(&degenerate).expect("the default material resolves");
        assert!(!open.stone_closed);
        assert!(open.stone_shadow_enabled(), "the local fallback still asks");
        assert!(
            open.tray_reflection_applies(),
            "the local fallback still asks"
        );
        assert!(!open.remote_stone_shadow());
        assert_eq!(open.remote_tray_reflection().to_bits(), 0);

        let closed = Mutex::new(RenderContext {
            stone_shadow: true,
            tray_reflection: 0.04,
            ..Default::default()
        });
        let closed = SceneSnapshot::capture(&closed).expect("the default material resolves");
        assert!(closed.stone_closed);
        assert!(closed.remote_stone_shadow());
        assert_eq!(closed.remote_tray_reflection().to_bits(), 0.04f32.to_bits());

        // Nothing asked for, nothing looked at: the flag stays `true`.
        let plain = Mutex::new(RenderContext {
            active_planes: Arc::new(Vec::new()),
            ..Default::default()
        });
        let plain = SceneSnapshot::capture(&plain).expect("the default material resolves");
        assert!(plain.stone_closed);
    }

    /// The viewport's pan reaches the export snapshot, and an untouched context captures the
    /// centred stone.
    #[test]
    fn capture_carries_the_camera_pan_into_the_exported_scene() {
        let centred = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(centred.camera_pan, [0.0, 0.0]);

        let panned = SceneSnapshot::capture(&Mutex::new(RenderContext {
            camera_pan: [0.4, -0.25],
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(panned.camera_pan, [0.4, -0.25]);
    }

    /// The export takes the Live Render pose from the context, also while the Edit tab is shown,
    /// and an untouched context captures the stone as designed.
    #[test]
    fn capture_carries_the_stone_pose_into_the_exported_scene() {
        let plain = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert!(plain.stone_pose.is_identity());
        assert!(
            matches!(plain.posed(), Cow::Borrowed(_)),
            "no pose, no copy"
        );

        let pose = StonePose {
            tilt_deg: 60.0,
            turn_deg: 30.0,
            spin_deg: 10.0,
        };
        for live_tab_shown in [true, false] {
            let posed = SceneSnapshot::capture(&Mutex::new(RenderContext {
                stone_pose: pose,
                live_tab_shown,
                ..Default::default()
            }))
            .expect("default resolves");
            assert_eq!(posed.stone_pose, pose, "live tab shown: {live_tab_shown}");
        }
    }

    /// A tilt scene shows the stone as designed.
    #[test]
    fn clearing_the_stone_pose_gives_the_stone_as_designed() {
        let mut scene = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_pose: StonePose {
                tilt_deg: 45.0,
                turn_deg: 0.0,
                spin_deg: 0.0,
            },
            ..Default::default()
        }))
        .expect("default resolves");
        assert!(!scene.stone_pose.is_identity());
        scene.clear_stone_pose();
        assert!(scene.stone_pose.is_identity());
        assert!(matches!(scene.posed(), Cow::Borrowed(_)));
    }

    /// The posed scene holds the turned stone, keeps the finishes and the closed flag, and has no
    /// pose left to apply twice.
    #[test]
    fn a_posed_scene_holds_the_turned_stone_and_no_pose() {
        let pose = StonePose {
            tilt_deg: 90.0,
            turn_deg: 0.0,
            spin_deg: 0.0,
        };
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_pose: pose,
            girdle_frosted: true,
            ..Default::default()
        }))
        .expect("default resolves");
        let posed = scene.posed();
        assert!(matches!(posed, Cow::Owned(_)));
        assert!(posed.stone_pose.is_identity());
        assert_eq!(posed.active_planes.len(), scene.active_planes.len());
        assert_eq!(posed.facet_finishes, scene.facet_finishes, "index-based");
        let r = pose.rotation();
        for (before, after) in scene.active_planes.iter().zip(&posed.active_planes) {
            let expected = r * Vec3::from(before.normal);
            assert!((Vec3::from(after.normal) - expected).length() < 1e-5);
            assert!((after.d - before.d).abs() < 1e-6, "d is kept");
        }
        assert!(
            (posed.material.c_axis - r * scene.material.c_axis).length() < 1e-5,
            "the crystal axis turns with the stone"
        );
        assert_eq!(
            posed.effects.scratch,
            ScratchBasis::new(r * Vec3::Y, r * Vec3::X)
        );
        assert!(
            matches!(posed.traced_material(), Cow::Borrowed(_)),
            "the effects hold no inclusion set that could be applied again"
        );
        assert_eq!(scene.stone_pose, pose, "the captured scene is not changed");
    }

    /// The viewport's surface glare reaches the export snapshot, and the default
    /// (`1.0`) is what an untouched context captures.
    #[test]
    fn capture_carries_the_surface_glare_into_the_exported_scene() {
        let default = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(default.surface_glare.to_bits(), 1.0f32.to_bits());

        let dimmed = SceneSnapshot::capture(&Mutex::new(RenderContext {
            surface_glare: 0.25,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(dimmed.surface_glare.to_bits(), 0.25f32.to_bits());
    }

    /// The viewport's head-shadow radius reaches the export snapshot. The default is 16.
    #[test]
    fn capture_carries_the_head_shadow_into_the_exported_scene() {
        let default = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(default.head_shadow_deg.to_bits(), 16.0f32.to_bits());

        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            head_shadow_deg: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(off.head_shadow_deg.to_bits(), 0.0f32.to_bits());
    }

    /// The girdle-frosted toggle is captured as a resolved per-facet finish list, not
    /// a bare `bool`, so `run_export`/`render_batch` need no further classification.
    #[test]
    fn capture_carries_the_girdle_frosted_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            girdle_frosted: false,
            ..Default::default()
        }))
        .expect("default resolves");
        assert!(
            off.facet_finishes.is_empty(),
            "the off position must carry no per-facet finish data"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            girdle_frosted: true,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.facet_finishes,
            girdle_facet_finishes(&RenderContext::default().active_planes),
            "the on position must carry the same classification the live viewport uses"
        );
    }

    /// A finished stone handed to `capture_finished` replaces the context's planes and
    /// tools -- and the derived frosted-girdle list follows it -- while an empty one (a
    /// design that did not solve) leaves the context's own stone alone.
    #[test]
    fn capture_finished_draws_the_substitute_stone_and_ignores_an_empty_one() {
        let ctx = Mutex::new(RenderContext {
            girdle_frosted: true,
            ..Default::default()
        });
        let own = ctx.lock().unwrap().active_planes.to_vec();
        assert!(own.len() > 4);
        let finished = StoneGeometryBuf {
            planes: own[..4].to_vec(),
            tools: Vec::new(),
            placements: Vec::new(),
            frosted: Vec::new(),
        };

        let swapped = SceneSnapshot::capture_finished(&ctx, Some(&finished)).expect("resolves");
        assert_eq!(swapped.active_planes, finished.planes);
        assert_eq!(
            swapped.facet_finishes,
            girdle_facet_finishes(&finished.planes),
            "the girdle classification describes the substitute stone"
        );

        let empty = StoneGeometryBuf {
            planes: Vec::new(),
            tools: Vec::new(),
            placements: Vec::new(),
            frosted: Vec::new(),
        };
        let kept = SceneSnapshot::capture_finished(&ctx, Some(&empty)).expect("resolves");
        assert_eq!(kept.active_planes, own);
        let plain = SceneSnapshot::capture_finished(&ctx, None).expect("resolves");
        assert_eq!(plain.active_planes, own);
    }

    /// The render physics follow the engine the scene is rendered with, not the one the view
    /// shows: a Path-Traced capture taken while the view is Ray-Traced keeps them, a Ray-Traced
    /// capture carries none, and the plain capture still follows the view.
    #[test]
    fn capture_keys_the_render_effects_to_the_exports_engine() {
        use pleochroma::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
        let ray = EngineSpec {
            engine: RenderEngine::RayTraced,
            ray: RayOptions {
                aa_taps: 1,
                fire: false,
            },
        };
        let path = EngineSpec::default();
        let effects = SceneEffects {
            facet_surfaces: vec![1],
            ..SceneEffects::default()
        };
        let context = |view: EngineSpec| {
            Mutex::new(RenderContext {
                engine: view,
                scene_effects: effects.clone(),
                ..Default::default()
            })
        };

        let kept = SceneSnapshot::capture_finished_for_engine(&context(ray), None, path)
            .expect("resolves");
        assert_eq!(kept.engine, path);
        assert_eq!(
            kept.effects.facet_surfaces,
            vec![1],
            "a Ray-Traced view loses nothing"
        );

        let dropped = SceneSnapshot::capture_finished_for_engine(&context(path), None, ray)
            .expect("resolves");
        assert_eq!(dropped.engine, ray);
        assert!(
            dropped.effects.is_none(),
            "a Ray-Traced capture carries none"
        );

        let followed = SceneSnapshot::capture_finished(&context(ray), None).expect("resolves");
        assert_eq!(followed.engine, ray);
        assert!(
            followed.effects.is_none(),
            "the plain capture follows the view"
        );
        let unchanged = SceneSnapshot::capture_finished(&context(path), None).expect("resolves");
        assert_eq!(unchanged.effects.facet_surfaces, vec![1]);
    }

    /// The design's frosted planes reach the export as `Frosted` finishes on exactly those
    /// planes -- the live viewport's list for the context's own stone, the finished stone's
    /// own for a substitute -- and a design without frosting still carries none.
    #[test]
    fn capture_carries_the_authored_frosting_into_the_exported_scene() {
        let ctx = Mutex::new(RenderContext {
            active_frosted: Arc::new(vec![2, 5]),
            ..Default::default()
        });
        let planes = ctx.lock().unwrap().active_planes.to_vec();
        let scene = SceneSnapshot::capture(&ctx).expect("default resolves");
        assert_eq!(scene.facet_finishes.len(), planes.len());
        for (index, finish) in scene.facet_finishes.iter().enumerate() {
            let expected = if index == 2 || index == 5 {
                FacetFinish::Frosted
            } else {
                FacetFinish::Polished
            };
            assert_eq!(*finish, expected, "plane {index}");
        }

        let finished = StoneGeometryBuf {
            planes: planes[..8].to_vec(),
            tools: Vec::new(),
            placements: Vec::new(),
            frosted: vec![1],
        };
        let swapped = SceneSnapshot::capture_finished(&ctx, Some(&finished)).expect("resolves");
        assert_eq!(
            swapped.facet_finishes,
            facet_finishes_with_frosted(&finished.planes, false, &[1]),
            "the substitute stone's own frosting wins over the context's"
        );

        let plain = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(
            plain.facet_finishes,
            [] as [pleochroma::optics::raytracer::FacetFinish; 0]
        );
    }

    /// A design's color variant is resolved into the context's `material_override`: the species
    /// stays and the absorption tensor is the variant's. The export receives the whole per-axis
    /// tensor and unit from this capture.
    #[test]
    fn capture_carries_a_design_color_variant_into_the_exported_scene() {
        use pleochroma::optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            materials::AbsorptionUnit,
        };

        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(550.0, 40.0, 0.35)],
            vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
        );
        let recolored = GemMaterial::sapphire()
            .with_color_variant_tensor(tensor.clone(), AbsorptionUnit::PerMm);
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Sapphire".to_string(),
            material_override: Some(recolored),
            ..Default::default()
        }))
        .expect("Sapphire resolves");

        assert_eq!(scene.material.name, "Sapphire");
        assert_eq!(scene.material.absorption, tensor);
        assert_ne!(
            scene.material.absorption,
            GemMaterial::sapphire().absorption
        );
        assert_eq!(scene.material.absorption_unit, AbsorptionUnit::PerMm);
        assert!(
            scene.material.absorption.is_pleochroic,
            "the variant keeps its own per-axis sets"
        );
        assert_eq!(
            scene.material.dispersion,
            GemMaterial::sapphire().dispersion,
            "only the absorption changed"
        );
    }

    /// The Live Render toolbar's view-only color must reach the export, and the tilt video and
    /// remote workers with it. It stays out while the linked open design supplies the color.
    #[test]
    fn capture_carries_the_view_only_colour_into_the_exported_scene() {
        use crate::bridge::render_thread::{PlanesOwner, ViewColor};

        let yellow = [0.2f32, 0.4, 2.8];
        let capture = |view_body_color, planes_owner, material_linked| {
            SceneSnapshot::capture(&Mutex::new(RenderContext {
                material_name: "Sapphire".to_string(),
                view_body_color,
                planes_owner,
                material_linked,
                ..Default::default()
            }))
            .expect("Sapphire resolves")
        };

        let plain = capture(None, PlanesOwner::Builtin, false);
        assert_eq!(
            plain.material.absorption,
            GemMaterial::sapphire().absorption
        );

        let tinted = capture(
            Some(ViewColor::triple_only(yellow)),
            PlanesOwner::Builtin,
            false,
        );
        assert_eq!(tinted.material.name, "Sapphire");
        assert_eq!(
            tinted.material.absorption,
            GemMaterial::sapphire().with_body_color(yellow).absorption,
            "the chosen colour must reach the exported scene"
        );
        assert_eq!(tinted.material.dispersion, plain.material.dispersion);

        // The L*C*h editor's band rows travel in the same recoloured material (the remote
        // workers receive it as it is), per millimetre and sized to the stone.
        let rows = vec![[460.0f32, 45.0, 0.25]];
        let banded = capture(
            Some(ViewColor {
                triple: yellow,
                bands: Some(rows.clone()),
                variant: None,
            }),
            PlanesOwner::Builtin,
            false,
        );
        assert_eq!(
            banded.material.absorption,
            GemMaterial::sapphire()
                .with_body_color_bands(&rows, 1.0)
                .absorption,
            "the band rows must reach the exported scene"
        );
        assert_eq!(
            banded.material.absorption_unit,
            pleochroma::optics::materials::AbsorptionUnit::PerMm
        );

        let design_driven = capture(
            Some(ViewColor::triple_only(yellow)),
            PlanesOwner::Editor { generation: 1 },
            true,
        );
        assert_eq!(
            design_driven.material.absorption, plain.material.absorption,
            "while the linked open design drives the colour, the view setting stays out"
        );
    }
}
