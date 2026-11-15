//! Building the `pleochroma_net::SceneState` a remote worker needs from an export's own
//! scene snapshot.

use crate::bridge::export_thread::scene_snapshot::SceneSnapshot;
use pleochroma::optics::raytracer::{Estimator, default_estimator};
use pleochroma_net::SceneState;

/// Builds the remote `SceneState` for an export's [`SceneSnapshot`] at `width x height`.
///
/// This is a copy of the GUI-side builder, which is private to another module tree.
///
/// The camera pose is passed in, not read from the snapshot, so a sweep of frames over one
/// snapshot (tilt video) can pose each frame. The pose comes from the same source as the local
/// engines' `Camera`, so the two cannot drift.
#[must_use]
pub(in crate::bridge::export_thread) fn scene_state_from_snapshot(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
    cam_yaw: f32,
    cam_pitch: f32,
) -> SceneState {
    let estimator = default_estimator();
    scene_state_from_snapshot_with(snapshot, width, height, cam_yaw, cam_pitch, estimator)
}

/// [`scene_state_from_snapshot`] with an explicit `estimator` instead of the process default.
#[must_use]
pub(in crate::bridge::export_thread) fn scene_state_from_snapshot_with(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
    cam_yaw: f32,
    cam_pitch: f32,
    estimator: Estimator,
) -> SceneState {
    // Girdle toggle and frosted planes, in wire form.
    let (girdle_frosted, frosted_planes) =
        pleochroma::geometry::finishes_wire_form(&snapshot.active_planes, &snapshot.facet_finishes);
    SceneState {
        engine: snapshot.engine,
        width,
        height,
        yaw: cam_yaw,
        camera_roll: snapshot.camera_roll,
        camera_pan: snapshot.camera_pan,
        // The pose beside the planes as designed: the worker turns the stone itself.
        stone_pose: snapshot.stone_pose.to_array(),
        // The shadow and the reflectance only where the lighting can apply them and the stone
        // is closed; the worker resolves its own plane and refuses a stone that is not closed.
        stone_shadow: snapshot.remote_stone_shadow(),
        tray_reflection: snapshot.remote_tray_reflection(),
        // Map yaw and follow-camera flag. The worker turns the map by this scene's `yaw`.
        hdr_orientation: snapshot.hdr_orientation,
        estimator,
        pitch: cam_pitch,
        distance: snapshot.distance,
        light_yaw: snapshot.light_yaw,
        light_pitch: snapshot.light_pitch,
        exposure: snapshot.exposure,
        max_bounces: snapshot.max_bounces,
        lighting_preset: snapshot.lighting_preset,
        material: snapshot.material.clone(),
        planes: snapshot.active_planes.clone(),
        girdle_frosted,
        backdrop: snapshot.backdrop,
        surface_glare: snapshot.surface_glare,
        // Concave tools; empty for a planar design.
        tools: snapshot.tools.clone(),
        // The HDR map by content hash, else the studio rig.
        environment: crate::bridge::remote::hdr_asset::scene_environment(snapshot.env_map.as_ref()),
        // Material emitters.
        fluorescence: snapshot.fluorescence.as_ref().clone(),
        head_shadow_deg: snapshot.head_shadow_deg,
        frosted_planes,
        // Render physics (silk, play-of-color, surface finish, abraded edge) and faceting.
        effects: snapshot.effects.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use pleochroma::geometry::tool::ToolPrimitive;
    use std::sync::{Arc, Mutex};

    fn state_for(tools: Vec<ToolPrimitive>) -> SceneState {
        let ctx = Mutex::new(RenderContext {
            active_tools: Arc::new(tools),
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&ctx).expect("the default material resolves");
        scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch)
    }

    #[test]
    fn a_remote_scene_carries_the_designs_concave_tools_so_the_worker_traces_the_same_stone() {
        let tools = vec![ToolPrimitive::ball(glam::Vec3::new(0.0, 0.3, 0.0), 0.1)];
        assert_eq!(state_for(tools.clone()).tools, tools);
    }

    /// Frosted planes travel as indices; a design without frosting sends none.
    #[test]
    fn a_remote_scene_carries_the_designs_frosted_planes_and_a_plain_one_carries_none() {
        let plain = Mutex::new(RenderContext::default());
        let snapshot = SceneSnapshot::capture(&plain).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.frosted_planes, [] as [u32; 0]);
        assert!(!state.girdle_frosted);

        let frosted = Mutex::new(RenderContext {
            active_frosted: Arc::new(vec![3, 9]),
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&frosted).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.frosted_planes, vec![3, 9]);
        assert!(
            !state.girdle_frosted,
            "authored frosting is not the girdle toggle"
        );

        let both = Mutex::new(RenderContext {
            active_frosted: Arc::new(vec![3, 9]),
            girdle_frosted: true,
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&both).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        // The worker rebuilds the same finishes from the wire form.
        assert_eq!(
            pleochroma::geometry::facet_finishes_with_frosted(
                &state.planes,
                state.girdle_frosted,
                &state.frosted_planes
            ),
            snapshot.facet_finishes
        );
    }

    /// The Tray Reflection travels as the scene's `tray_reflection`, off for a plain scene.
    #[test]
    fn a_remote_scene_carries_the_tray_reflection_and_a_plain_one_carries_none() {
        let plain = Mutex::new(RenderContext::default());
        let snapshot = SceneSnapshot::capture(&plain).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.tray_reflection.to_bits(), 0);

        let glossy = Mutex::new(RenderContext {
            tray_reflection: 0.04,
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&glossy).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.tray_reflection.to_bits(), 0.04f32.to_bits());
    }

    /// A stone that is not closed sends no shadow and a zero reflection, so the worker does not
    /// refuse the scene.
    #[test]
    fn a_remote_scene_of_a_stone_that_is_not_closed_carries_no_shadow_and_no_reflection() {
        let scene_for = |planes: Vec<pleochroma::geometry::plane::GpuFacetPlane>| {
            let ctx = Mutex::new(RenderContext {
                stone_shadow: true,
                tray_reflection: 0.04,
                active_planes: Arc::new(planes),
                ..RenderContext::default()
            });
            let snapshot = SceneSnapshot::capture(&ctx).expect("the default material resolves");
            scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch)
        };
        let degenerate = scene_for(Vec::new());
        assert!(!degenerate.stone_shadow);
        assert_eq!(degenerate.tray_reflection.to_bits(), 0);

        let closed =
            scene_for(pleochroma::geometry::cuts::StandardGemCuts::standard_round_brilliant());
        assert!(closed.stone_shadow);
        assert_eq!(closed.tray_reflection.to_bits(), 0.04f32.to_bits());
    }

    /// A turned HDR map travels as yaw and the follow switch, at the frame's own camera yaw.
    /// A studio scene sends the default orientation.
    #[test]
    fn a_remote_scene_carries_the_hdr_orientation_and_a_studio_one_carries_none() {
        use pleochroma::renderer::env_map::{EnvironmentMap, HdrOrientation};
        let capture = |with_map: bool| {
            let ctx = Mutex::new(RenderContext {
                env_map: with_map.then(|| Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))),
                hdr_yaw: 1.25,
                hdr_follows_camera: true,
                ..RenderContext::default()
            });
            SceneSnapshot::capture(&ctx).expect("the default material resolves")
        };
        let snapshot = capture(true);
        let state = scene_state_from_snapshot(&snapshot, 32, 24, 0.5, snapshot.pitch);
        assert_eq!(state.hdr_orientation.yaw.to_bits(), 1.25_f32.to_bits());
        assert!(state.hdr_orientation.follows_camera);
        assert_eq!(
            state.yaw.to_bits(),
            0.5_f32.to_bits(),
            "the frame's own camera yaw"
        );
        let bytes = postcard::to_allocvec(&state).expect("the scene encodes");
        let decoded: SceneState = postcard::from_bytes(&bytes).expect("the scene decodes");
        assert_eq!(decoded.hdr_orientation, state.hdr_orientation);

        let studio = capture(false);
        let state = scene_state_from_snapshot(&studio, 32, 24, studio.yaw, studio.pitch);
        assert_eq!(state.hdr_orientation, HdrOrientation::default());
    }

    /// The pan travels as the scene's `camera_pan`, through the wire, so the worker frames the
    /// stone where the viewport does; a centred view sends `[0.0, 0.0]`.
    #[test]
    fn a_remote_scene_carries_the_camera_pan() {
        let centred = Mutex::new(RenderContext::default());
        let snapshot = SceneSnapshot::capture(&centred).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.camera_pan, [0.0, 0.0]);

        let panned = Mutex::new(RenderContext {
            camera_pan: [0.5, -0.75],
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&panned).expect("the default material resolves");
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.camera_pan, [0.5, -0.75]);
        let bytes = postcard::to_allocvec(&state).expect("the scene encodes");
        let decoded: SceneState = postcard::from_bytes(&bytes).expect("the scene decodes");
        assert_eq!(decoded.camera_pan, [0.5, -0.75]);
    }

    /// The scene carries its estimator, so the worker traces with the same one. The plain
    /// builder uses the process default.
    #[test]
    fn a_remote_scene_carries_the_estimator_it_is_built_with() {
        let ctx = Mutex::new(RenderContext::default());
        let snapshot = SceneSnapshot::capture(&ctx).expect("the default material resolves");
        for estimator in [Estimator::Branching, Estimator::ExitCollection] {
            let state = scene_state_from_snapshot_with(
                &snapshot,
                32,
                24,
                snapshot.yaw,
                snapshot.pitch,
                estimator,
            );
            assert_eq!(state.estimator, estimator);
        }
        let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
        assert_eq!(state.estimator, default_estimator());
    }

    #[test]
    fn a_planar_remote_scene_sends_no_tools() {
        assert_eq!(
            state_for(Vec::new()).tools.len(),
            0,
            "no tools in the scene"
        );
    }

    /// A custom material's dispersion curve (Sellmeier or Cauchy coefficients) is part of the
    /// `GemMaterial` the scene carries, so a remote worker traces the colour play the cutter
    /// set rather than Diamond's. Guards the whole path: the context's material override, the
    /// snapshot, the `SceneState`, and the wire (postcard) the worker reads it from.
    #[test]
    fn a_remote_scene_carries_a_custom_materials_dispersion_model() {
        use pleochroma::optics::{dispersion::DispersionModel, materials::GemMaterial};
        let models = [
            DispersionModel::Sellmeier1 { b1: 1.4, c1: 0.012 },
            DispersionModel::Sellmeier3 {
                b: [1.0, 0.25, 0.5],
                c: [0.006, 0.02, 100.0],
            },
            DispersionModel::Cauchy {
                a: 1.72,
                b: 0.0121,
                c: 0.000_2,
            },
        ];
        for model in models {
            let custom = GemMaterial::new_custom_with_dispersion("Custom", model, 0.0, [0.0; 3]);
            let ctx = Mutex::new(RenderContext {
                material_override: Some(custom),
                ..RenderContext::default()
            });
            let snapshot = SceneSnapshot::capture(&ctx).expect("the custom material resolves");
            let state = scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch);
            assert_eq!(state.material.dispersion, model);
            assert_ne!(
                state.material.dispersion,
                GemMaterial::diamond().dispersion,
                "the scene must not fall back to the default material's curve"
            );
            let bytes = postcard::to_allocvec(&state).expect("the scene encodes");
            let decoded: SceneState = postcard::from_bytes(&bytes).expect("the scene decodes");
            assert_eq!(decoded.material.dispersion, model);
        }
    }
}
