//! Loading an HDR environment map, and snapshotting one frame's worth of
//! [`RenderContext`] fields out from under its lock.

use super::RenderContext;
use crate::{
    bridge::sample_cursor::LiveEpoch,
    settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale},
};
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::LightingPreset},
    renderer::env_map::{EnvironmentMap, HdrOrientation},
};
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::{Arc, Mutex, atomic::AtomicBool};

/// Decodes a Radiance `.hdr` file at `path` into an [`EnvironmentMap`], wrapped in the `Arc` that
/// `RenderContext::env_map` stores.
///
/// On `Err` the caller leaves `env_map` untouched, so the previous environment stays active.
///
/// Decodes through `bridge::remote::hdr_asset::load_hdr_file`, which also registers the
/// map as a content-addressed asset so an HDR-capable remote receives its exact bytes.
///
/// # Errors
///
/// Returns `Err` with a human-readable message if `path` cannot be read or does not
/// decode as a valid Radiance HDR image.
pub fn load_env_map(path: &str) -> Result<Arc<EnvironmentMap>, String> {
    crate::bridge::remote::hdr_asset::load_hdr_file(std::path::Path::new(path)).map(|(map, _)| map)
}

/// One frame's worth of inputs read out of `RenderContext` under its lock, copied out
/// so the mutex guard can be dropped immediately.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors RenderContext's flag fields; fields are independent switches"
)]
pub(in crate::bridge::render_thread) struct FrameInputs {
    /// Invalidated by scene/activity edits, shared with asynchronous display work.
    pub(in crate::bridge::render_thread) cancel: Arc<AtomicBool>,
    pub(in crate::bridge::render_thread) engine: pleochroma::renderer::engine::EngineSpec,
    pub(in crate::bridge::render_thread) width: u32,
    pub(in crate::bridge::render_thread) height: u32,
    pub(in crate::bridge::render_thread) yaw: f32,
    pub(in crate::bridge::render_thread) pitch: f32,
    pub(in crate::bridge::render_thread) camera_roll: f32,
    /// See [`RenderContext::camera_pan`].
    pub(in crate::bridge::render_thread) camera_pan: [f32; 2],
    pub(in crate::bridge::render_thread) distance: f32,
    pub(in crate::bridge::render_thread) light_yaw: f32,
    pub(in crate::bridge::render_thread) light_pitch: f32,
    pub(in crate::bridge::render_thread) material_name: String,
    pub(in crate::bridge::render_thread) material_override: Option<GemMaterial>,
    /// See [`RenderContext::material_unresolved`].
    pub(in crate::bridge::render_thread) material_unresolved: Option<String>,
    pub(in crate::bridge::render_thread) lighting_preset: LightingPreset,
    pub(in crate::bridge::render_thread) backdrop: crate::settings::model::Backdrop,
    pub(in crate::bridge::render_thread) surface_glare: f32,
    pub(in crate::bridge::render_thread) head_shadow_deg: f32,
    /// Whether this accumulation requests the stone shadow on the paper: the snapshot of
    /// [`RenderContext::stone_shadow`], still subject to the lighting declining it.
    pub(in crate::bridge::render_thread) stone_shadow: bool,
    /// The snapshot of [`RenderContext::tray_reflection`], still subject to the lighting
    /// declining it.
    pub(in crate::bridge::render_thread) tray_reflection: f32,
    pub(in crate::bridge::render_thread) target_samples: u32,
    pub(in crate::bridge::render_thread) max_bounces: u32,
    pub(in crate::bridge::render_thread) exposure: f32,
    pub(in crate::bridge::render_thread) inclusion_sigma_s: f32,
    pub(in crate::bridge::render_thread) c_axis_override: Option<Vec3>,
    /// See [`RenderContext::alpha_axis_override`].
    pub(in crate::bridge::render_thread) alpha_axis_override: Option<Vec3>,
    pub(in crate::bridge::render_thread) girdle_frosted: bool,
    pub(in crate::bridge::render_thread) edge_rounding_radius: f32,
    /// See [`RenderContext::scene_effects`].
    pub(in crate::bridge::render_thread) scene_effects: SceneEffects,
    pub(in crate::bridge::render_thread) stone_width_mm: f32,
    pub(in crate::bridge::render_thread) active_planes: Arc<Vec<GpuFacetPlane>>,
    /// See [`RenderContext::active_frosted`].
    pub(in crate::bridge::render_thread) active_frosted: Arc<Vec<u32>>,
    /// See [`RenderContext::active_tools`].
    pub(in crate::bridge::render_thread) active_tools: Arc<Vec<ToolPrimitive>>,
    pub(in crate::bridge::render_thread) custom_materials: Arc<Vec<GemMaterial>>,
    /// See [`RenderContext::active_fluorescence`]; `None` is no fluorescence.
    pub(in crate::bridge::render_thread) fluorescence: Option<Arc<Fluorescence>>,
    pub(in crate::bridge::render_thread) running: bool,
    pub(in crate::bridge::render_thread) dirty: bool,
    pub(in crate::bridge::render_thread) paused: bool,
    pub(in crate::bridge::render_thread) tab_visible: bool,
    pub(in crate::bridge::render_thread) denoise_enabled: bool,
    /// See [`RenderContext::redisplay_requested`].
    pub(in crate::bridge::render_thread) redisplay_requested: bool,
    pub(in crate::bridge::render_thread) remote_active: bool,
    /// See [`RenderContext::live_epoch`].
    pub(in crate::bridge::render_thread) live_epoch: Option<Arc<LiveEpoch>>,
    pub(in crate::bridge::render_thread) export_active: bool,
    pub(in crate::bridge::render_thread) live_compute_target: LiveComputeTarget,
    pub(in crate::bridge::render_thread) local_compute_target: LocalComputeTarget,
    pub(in crate::bridge::render_thread) local_preview_scale: LocalPreviewScale,
    /// See [`RenderContext::adaptive_viewport`].
    pub(in crate::bridge::render_thread) adaptive_viewport: bool,
    /// See [`RenderContext::path_cache_viewport`].
    pub(in crate::bridge::render_thread) path_cache_viewport: bool,
    /// See [`RenderContext::instant_lighting`].
    pub(in crate::bridge::render_thread) instant_lighting: bool,
    /// See [`RenderContext::beam_viewport`].
    #[cfg(feature = "beam")]
    pub(in crate::bridge::render_thread) beam_viewport: bool,
    /// See [`RenderContext::beam_instant_lighting`].
    #[cfg(feature = "beam")]
    pub(in crate::bridge::render_thread) beam_instant_lighting: bool,
    pub(in crate::bridge::render_thread) camera_moving: bool,
    /// See [`RenderContext::camera_drag_held`]; the render loop turns it into motion
    /// only once the scene changed under the held button (`local_preview::DragGesture`).
    pub(in crate::bridge::render_thread) camera_drag_held: bool,
    pub(in crate::bridge::render_thread) env_map: Option<Arc<EnvironmentMap>>,
    /// The snapshot of [`RenderContext::hdr_orientation`]; read only with an `env_map`.
    pub(in crate::bridge::render_thread) hdr_orientation: HdrOrientation,
    /// [`RenderContext::scene_generation`] of exactly the fields in this snapshot
    /// (read under the same lock) -- compared against the live epoch's own
    /// generation so a frame never claims from or merges with an epoch that was
    /// dispatched for a different scene.
    pub(in crate::bridge::render_thread) scene_generation: u64,
    /// [`RenderContext::pose_generation`] of the same snapshot: unchanged by a lighting edit.
    pub(in crate::bridge::render_thread) pose_generation: u64,
}

impl RenderContext {
    /// The manual pause belongs to Path-Traced; Ray-Traced has no pause control.
    /// Preserve the stored value so returning to Path-Traced restores its pause.
    pub const fn effective_paused(&self) -> bool {
        self.paused && !self.engine.is_ray()
    }
}

/// Snapshots every field the render loop needs for one frame out of `RenderContext`,
/// clearing `dirty` in the same locked section so a `dirty` set by a callback between
/// the read and the clear is never lost.
pub(in crate::bridge::render_thread) fn snapshot_frame_inputs(
    ctx: &Arc<Mutex<RenderContext>>,
) -> FrameInputs {
    // Recovers from a poisoned lock like every other `RenderContext` lock. A panic here
    // would stop rendering while the window keeps answering, which looks like a hang.
    // A poisoning writer can only leave a stale but valid frame, replaced next tick.
    let mut ctx = RenderContext::lock(ctx);
    let dirty = ctx.dirty;
    let cancel = ctx.frame_cancellation();
    ctx.dirty = false;
    // Read and cleared with `dirty` in this one locked section, so a request set in
    // between is not lost.
    let redisplay_requested = ctx.redisplay_requested;
    ctx.redisplay_requested = false;
    let scene_generation = ctx.scene_generation();
    let target_samples = ctx.engine.target_samples(ctx.target_samples);
    if let Some(epoch) = ctx.live_epoch.as_ref()
        && epoch.scene_generation() == scene_generation
        && epoch.dimensions() == (ctx.width, ctx.height)
    {
        // Both lanes retain the same cursor and sums across a target-only edit.
        // Existing claims remain owned until committed or returned on cancellation.
        epoch.set_target_samples(target_samples);
    }
    FrameInputs {
        cancel,
        engine: ctx.engine,
        width: ctx.width,
        height: ctx.height,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        camera_roll: ctx.camera_roll,
        camera_pan: ctx.camera_pan,
        distance: ctx.distance,
        light_yaw: ctx.light_yaw,
        light_pitch: ctx.light_pitch,
        material_name: ctx.material_name.clone(),
        // The override with the Live Render toolbar's view-only colour applied (a plain
        // clone while none is in force) -- see `RenderContext::tinted_material_override`.
        material_override: ctx.tinted_material_override(),
        material_unresolved: ctx.material_unresolved.clone().or_else(|| {
            super::super::engine::decline_message(
                ctx.engine,
                ctx.env_map.is_none().then_some(ctx.lighting_preset),
                ctx.active_fluorescence().as_deref(),
            )
            .map(str::to_owned)
        }),
        lighting_preset: ctx.lighting_preset,
        backdrop: ctx.backdrop,
        surface_glare: ctx.surface_glare,
        head_shadow_deg: ctx.head_shadow_deg,
        stone_shadow: ctx.stone_shadow,
        tray_reflection: ctx.tray_reflection,
        target_samples,
        max_bounces: ctx.max_bounces,
        exposure: ctx.effective_exposure(),
        inclusion_sigma_s: ctx.inclusion_sigma_s,
        c_axis_override: ctx.c_axis_override,
        alpha_axis_override: ctx.alpha_axis_override,
        girdle_frosted: ctx.girdle_frosted,
        edge_rounding_radius: ctx.edge_rounding_radius,
        scene_effects: ctx.scene_effects.clone(),
        stone_width_mm: ctx.stone_width_mm,
        // `Arc::clone`, not a deep copy -- see `RenderContext::active_planes`'s doc
        // comment.
        active_planes: Arc::clone(&ctx.active_planes),
        active_frosted: Arc::clone(&ctx.active_frosted),
        // `Arc::clone`, like the planes beside it.
        active_tools: Arc::clone(&ctx.active_tools),
        // `Arc::clone`, not a deep copy -- see `RenderContext::custom_materials`'s doc
        // comment.
        custom_materials: Arc::clone(&ctx.custom_materials),
        // `Arc::clone` of the active material's emitters, `None` for the common case.
        fluorescence: ctx.active_fluorescence(),
        running: ctx.running,
        dirty,
        paused: ctx.effective_paused(),
        tab_visible: ctx.tab_visible,
        denoise_enabled: ctx.denoise_enabled && ctx.engine.denoises(),
        redisplay_requested,
        remote_active: ctx.remote_active,
        // `Arc::clone`, not a deep copy -- see `live_epoch`'s doc comment.
        live_epoch: ctx.live_epoch.clone(),
        export_active: ctx.export_active(),
        // The EFFECTIVE target: a final-picture epoch behaves as `RemoteOnly`.
        live_compute_target: if super::super::remote_allowed(ctx.engine) {
            ctx.effective_live_target()
        } else {
            LiveComputeTarget::LocalOnly
        },
        local_compute_target: ctx.local_compute_target,
        local_preview_scale: ctx.local_preview_scale,
        adaptive_viewport: ctx.adaptive_viewport,
        path_cache_viewport: ctx.path_cache_viewport,
        instant_lighting: ctx.instant_lighting,
        #[cfg(feature = "beam")]
        beam_viewport: ctx.beam_viewport,
        #[cfg(feature = "beam")]
        beam_instant_lighting: ctx.beam_instant_lighting,
        camera_moving: ctx.camera_moving,
        // A held drag enters preview as soon as the pose moves, before the handoff
        // timer's tick -- but a press that never moves stays Final.
        camera_drag_held: ctx.camera_drag_held,
        // `Arc::clone`, not a deep copy of the decoded panorama -- see `env_map`'s doc comment.
        env_map: ctx.env_map.clone(),
        hdr_orientation: ctx.hdr_orientation(),
        scene_generation,
        pose_generation: ctx.pose_generation(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::renderer::engine::RenderEngine;

    #[test]
    fn target_changes_keep_the_shared_epoch_and_resume_after_committed_samples() {
        let mut state = RenderContext {
            width: 1,
            height: 1,
            target_samples: 128,
            ..RenderContext::default()
        };
        let generation = state.scene_generation();
        let epoch = Arc::new(LiveEpoch::new(1, 1, 128).for_scene(generation));
        state.live_epoch = Some(Arc::clone(&epoch));
        let ctx = Arc::new(Mutex::new(state));
        snapshot_frame_inputs(&ctx);
        assert_eq!(epoch.claim_local(128), Some((0, 128)));
        RenderContext::lock(&ctx).target_samples = 1024;
        let raised = snapshot_frame_inputs(&ctx);
        assert!(!raised.dirty);
        assert_eq!(raised.scene_generation, generation);
        assert!(Arc::ptr_eq(raised.live_epoch.as_ref().unwrap(), &epoch));
        assert_eq!(epoch.claim_local(4), Some((128, 4)));
        RenderContext::lock(&ctx).target_samples = 8;
        snapshot_frame_inputs(&ctx);
        // The cancelled pass's complete range remains recoverable after shrinking.
        epoch.return_to_local(128, 4);
        assert_eq!(epoch.claim_local(4), None);
        RenderContext::lock(&ctx).target_samples = 1024;
        snapshot_frame_inputs(&ctx);
        assert_eq!(epoch.claim_local(4), Some((128, 4)));
        assert_eq!(epoch.claim_local(4), Some((132, 4)));
    }

    #[test]
    fn ray_snapshots_ignore_manual_path_pause_and_restore_it_on_return() {
        let ctx = Arc::new(Mutex::new(RenderContext {
            paused: true,
            ..RenderContext::default()
        }));
        assert!(snapshot_frame_inputs(&ctx).paused);
        RenderContext::lock(&ctx).engine.engine = RenderEngine::RayTraced;
        assert!(!snapshot_frame_inputs(&ctx).paused);
        assert!(
            RenderContext::lock(&ctx).paused,
            "keep the Path-Traced preference"
        );
        RenderContext::lock(&ctx).engine.engine = RenderEngine::PathTraced;
        assert!(snapshot_frame_inputs(&ctx).paused);
    }
}
