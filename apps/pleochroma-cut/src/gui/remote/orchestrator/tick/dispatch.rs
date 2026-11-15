//! Starting a settled epoch's remote work ([`start_remote_render`]), dispatching its
//! chunks one after another on the persistent connection ([`dispatch_next_chunk`]),
//! the persistent-connection staleness check that gates reusing a cached connection
//! ([`connection_is_stale`]), and building the `pleochroma_net::SceneState` every
//! chunk sends ([`scene_state_from_snapshot`]). See this group's own `mod.rs` doc
//! comment.

use super::{
    decisions::wants_display_only,
    state::{Orchestrator, lock},
    update::handle_remote_update,
};
use crate::{
    MainWindow,
    bridge::{
        export_thread::SceneSnapshot,
        frame_cache::guide_pass::GuideCache,
        remote::{
            live_lane::{LiveLane, grown_chunk_secs},
            remote_render::{self, RemoteRenderRequest, RemoteUpdate},
        },
        render_thread::RenderContext,
        sample_cursor::LiveEpoch,
    },
    settings::{LiveComputeTarget, RemoteEndpoint, WorkerSettings},
};
use pleochroma::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};
use pleochroma_net::SceneState;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::Instant,
};

/// Starts a settled pose's image epoch: installs a fresh [`LiveEpoch`] in `RenderContext`
/// with `remote_active` and `dirty`, in one locked mutation, creates its [`LiveLane`] and
/// dispatches the first chunk. `target_samples` is the single global target: local and
/// remote together trace that many samples.
///
/// Callers have already passed the HDR guard (`bridge::remote::live_remote_dispatch`).
///
/// A display-only epoch (Final picture transfer, see `decisions::wants_display_only`) sends
/// one `DisplayOnly` request over the whole budget. It has no local combining
/// (`RenderContext::live_display_only`) and no guide prepass, since the remote denoises and
/// its decoded frames are the image.
pub(super) fn start_remote_render(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    endpoint: RemoteEndpoint,
    state: &Arc<Mutex<Orchestrator>>,
) {
    // Read under one lock, before the scene is captured: a change after this point makes
    // the render thread see a different scene generation than the one stamped into the
    // epoch, so it releases the epoch rather than merging two scenes
    // (`live_split::epoch_scene_mismatch`).
    let (width, height, target_samples, live_compute_target, scene_generation) = {
        let mut ctx = RenderContext::lock(render_ctx);
        (
            ctx.width,
            ctx.height,
            ctx.engine.target_samples(ctx.target_samples),
            ctx.live_compute_target,
            ctx.scene_generation(),
        )
    };
    if width == 0 || height == 0 || target_samples == 0 {
        return;
    }
    let display_only = wants_display_only(
        live_compute_target,
        endpoint.live_transfer,
        lock(state).display_only_refused,
    );
    let combining = matches!(live_compute_target, LiveComputeTarget::Both) && !display_only;
    // Read before the connection set-up below takes the endpoint's connection settings.
    let transfer_mode = endpoint.connection.transfer_mode;
    // A material that does not resolve makes the capture fail, so the wrong stone is never
    // sent to a worker. The render loop shows the refusal; here the dispatch is skipped and
    // the next settle retries.
    let Ok(snapshot) = capture_live(render_ctx) else {
        return;
    };
    let scene = scene_state_from_snapshot(&snapshot, width, height);

    // Start the guide-buffer prepass at dispatch time, so it overlaps the network round
    // trip and the remote render. Cancel the generation still running for a previous pose.
    //
    // Skipped when combining: local tracing produces the same first-hit guides for this
    // pose as a side effect, and the render thread denoises the merged image with them.
    // Skipped for a display-only epoch too, since the remote's frames arrive denoised.
    if snapshot.engine.denoises() && !combining && !display_only {
        // The path signature refracts at the material's index, so it keys the guides.
        let n_d = snapshot.material.dispersion.n_d();
        let guide_camera = Camera::new(
            snapshot.yaw,
            snapshot.pitch,
            snapshot.distance,
            DEFAULT_FOV_DEG,
        )
        .with_roll(snapshot.camera_roll)
        .with_pan(snapshot.camera_pan);
        // The guides are the first hit of the stone the tracers draw: the posed stone while the
        // scene carries a pose.
        let posed_stone = (!snapshot.stone_pose.is_identity()).then(|| {
            let posed = snapshot.posed().into_owned();
            (posed.active_planes, posed.tools)
        });
        let (guide_planes, guide_tools) =
            posed_stone.unwrap_or((snapshot.active_planes, snapshot.tools));
        let guide_key = GuideCache::key_for_camera(
            width,
            height,
            &guide_camera,
            StoneGeometry {
                planes: &guide_planes,
                tools: &guide_tools,
            },
            n_d,
        );
        let mut s = lock(state);
        if let Some(previous) = s.pending_guide_gen.take() {
            previous.cancel.store(true, Ordering::Relaxed);
        }
        s.pending_guide_gen = Some(super::super::generation::spawn_guide_generation(
            guide_key,
            guide_camera,
            guide_planes, // moved: `snapshot` isn't used again after this
            guide_tools,
            width,
            height,
            n_d,
        ));
    }

    // Discards the local preview and starts this settle's epoch in one locked mutation, so
    // the render thread never sees `remote_active` set together with a stale epoch. The
    // `dirty` flag restarts local accumulation, so the drag preview is not summed into the
    // settled image. This is where `HandoffAction::DiscardLocalPreview` takes effect.
    let epoch = Arc::new(LiveEpoch::new(width, height, target_samples).for_scene(scene_generation));
    {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.remote_active = true;
        ctx.dirty = true;
        ctx.live_epoch = Some(Arc::clone(&epoch));
        ctx.live_display_only = display_only;
    }

    // Reuse the cached connection when its identity matches the endpoint, else create one.
    // This is the only place a `RemoteConnectionHandle` is created.
    {
        let mut s = lock(state);
        if s.remote_connection
            .as_ref()
            .is_none_or(|h| h.worker() != &endpoint.connection)
        {
            s.remote_connection = Some(remote_render::spawn_remote_connection(endpoint.connection));
            s.display_only_refused = false;
            s.remote_hdr = None;
            {
                s.remote_zoning = None;
            }
        }
        s.live_lane = Some(if display_only {
            LiveLane::display_only(epoch)
        } else if combining {
            LiveLane::new(epoch, true)
        } else {
            // Remote Only: no local tracer shares the epoch, so settled chunks grow and the
            // worker idles less between them (see `LiveLane::with_chunk_growth`).
            LiveLane::new(epoch, false).with_chunk_growth(grown_chunk_secs(transfer_mode))
        });
        s.lane_scene = Some(scene);
    }
    super::progress::preparing(ui, render_ctx, scene_generation, target_samples);
    dispatch_next_chunk(&ui.as_weak(), render_ctx, state);
}

/// Claims the current epoch's next remote chunk and sends it as one
/// `RenderRequest{first_sample, samples}` on the persistent connection, recording its
/// request id as current. Returns `false` (sending nothing) when there is no lane, no
/// connection, or nothing left to claim -- in the last case the lane is now finished.
///
/// Called once per settle by [`start_remote_render`] and then once per finished (or
/// retried) chunk by `update::handle_remote_update`, so the next request goes out the
/// moment the previous chunk's `DONE` arrives.
pub(super) fn dispatch_next_chunk(
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) -> bool {
    let mut s = lock(state);
    let request_id = s.alloc_request_id();
    let Orchestrator {
        live_lane,
        lane_scene,
        remote_connection,
        ..
    } = &mut *s;
    let (Some(lane), Some(scene), Some(connection)) = (
        live_lane.as_mut(),
        lane_scene.as_ref(),
        remote_connection.as_ref(),
    ) else {
        return false;
    };
    let Some(chunk) = lane.next_chunk(request_id, Instant::now()) else {
        return false;
    };
    let (width, height) = lane.epoch().dimensions();
    let request = RemoteRenderRequest {
        worker: connection.worker().clone(),
        request_id: chunk.request_id,
        scene: scene.clone(),
        first_sample: chunk.first_sample,
        samples: chunk.samples,
        width,
        height,
        // The live viewport: latency first (a coordinator serves it from its own lane).
        intent: pleochroma_net::messages::RequestIntent::Interactive,
        display_only: lane.is_display_only(),
    };
    let ui_weak = ui_weak.clone();
    let render_ctx = Arc::clone(render_ctx);
    let state_for_updates = Arc::clone(state);
    // The chunk's accumulator also holds its latest display frame (final-picture live
    // transfer), which the update handler decodes.
    let chunk_accumulator = Arc::clone(&chunk.accumulator);
    let handle = connection.render(request, chunk.accumulator, move |update: RemoteUpdate| {
        handle_remote_update(
            &ui_weak,
            &render_ctx,
            &state_for_updates,
            (width, height, &chunk_accumulator),
            update,
        );
    });
    // Recorded on this same UI-thread call, before any update for this request could
    // reach the event loop -- see `Orchestrator::current_request_id`'s doc comment.
    s.current_request_id = Some(request_id);
    s.remote_handle = Some(handle);
    true
}

/// Whether the cached connection identity `cached` must be torn down for the `wanted`
/// worker of the settle being checked. `wanted` is `None` when no remote is configured
/// (`LiveComputeTarget::LocalOnly`, or the worker was removed).
///
/// Compares the whole `WorkerSettings`. A tweak to a non-identity field costs one extra
/// handshake on save, which is rare and off the drag path. Address and cert dir always
/// force a reconnect: a connection authenticated under the old certificates must never be
/// reused (see `RemoteConnectionHandle`).
///
/// Pure, so it is unit-tested with plain `WorkerSettings` values.
#[must_use]
pub(super) fn connection_is_stale(
    cached: Option<&WorkerSettings>,
    wanted: Option<&WorkerSettings>,
) -> bool {
    match (cached, wanted) {
        (Some(c), Some(w)) => c != w,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The scene a live chunk traces: [`SceneSnapshot::capture`] with the stone pose the Live Render
/// tab shows ([`RenderContext::effective_stone_pose`]), so a settle while the Edit tab's traced
/// mode is shown dispatches the stone as designed, as the local tracer draws it. An export takes
/// the raw Live Render pose instead.
fn capture_live(render_ctx: &Arc<Mutex<RenderContext>>) -> Result<SceneSnapshot, String> {
    let mut snapshot = SceneSnapshot::capture(render_ctx)?;
    snapshot.stone_pose = RenderContext::lock(render_ctx).effective_stone_pose();
    Ok(snapshot)
}

/// Builds the fully resolved `pleochroma_net::SceneState` a remote worker renders from a
/// local `SceneSnapshot` and the session's resolution. Every field is a resolved value,
/// since the worker cannot look up names or ids (see `pleochroma_net::scene::SceneState`).
///
/// Frosting: `SceneState` carries only the girdle bit and the authored frosted plane
/// indices (`pleochroma::geometry::finishes_wire_form`). The worker re-derives the same
/// finishes from the `planes` it also receives, so a scene without frosting is unchanged on
/// the wire.
fn scene_state_from_snapshot(snapshot: &SceneSnapshot, width: u32, height: u32) -> SceneState {
    let (girdle_frosted, frosted_planes) =
        pleochroma::geometry::finishes_wire_form(&snapshot.active_planes, &snapshot.facet_finishes);
    SceneState {
        engine: snapshot.engine,
        width,
        height,
        yaw: snapshot.yaw,
        camera_roll: snapshot.camera_roll,
        camera_pan: snapshot.camera_pan,
        // The pose beside the planes as designed: the worker turns the stone itself.
        stone_pose: snapshot.stone_pose.to_array(),
        // Off for a stone that is not closed, which the worker would refuse.
        stone_shadow: snapshot.remote_stone_shadow(),
        tray_reflection: snapshot.remote_tray_reflection(),
        // The map's yaw and follow switch; the worker turns it at this scene's `yaw`.
        hdr_orientation: snapshot.hdr_orientation,
        estimator: pleochroma::optics::raytracer::default_estimator(),
        pitch: snapshot.pitch,
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
        // The concave tools, so the worker traces the same stone as the local tracer; empty
        // for a planar design.
        tools: snapshot.tools.clone(),
        // The loaded HDR map by content hash, else the studio rig.
        environment: crate::bridge::remote::hdr_asset::scene_environment(snapshot.env_map.as_ref()),
        // The material's emitters, so the worker traces the same glow as the local tracer;
        // empty for a non-fluorescent material.
        fluorescence: snapshot.fluorescence.as_ref().clone(),
        head_shadow_deg: snapshot.head_shadow_deg,
        frosted_planes,
        // The stone's render physics (silk, play-of-color, surface finish, abraded edge) and the
        // faceting machine, so the worker traces the same stone.
        effects: snapshot.effects.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- `connection_is_stale`: when the cached persistent connection must be torn
    // down and replaced -- pure logic, no socket, no `Orchestrator`. ------------------

    fn worker_named(name: &str) -> WorkerSettings {
        WorkerSettings {
            name: name.to_string(),
            address: format!("{name}.local:9443"),
            cert_dir: format!("/certs/{name}"),
            ..WorkerSettings::default()
        }
    }

    #[test]
    fn the_live_dispatch_scene_carries_the_viewports_surface_glare() {
        let ctx = Mutex::new(RenderContext {
            surface_glare: 0.4,
            ..Default::default()
        });
        let snapshot = SceneSnapshot::capture(&ctx).expect("default resolves");
        let state = scene_state_from_snapshot(&snapshot, 64, 64);
        assert_eq!(state.surface_glare.to_bits(), 0.4f32.to_bits());
    }

    /// The live chunks trace the viewport's framing: the scene carries its pan.
    #[test]
    fn the_live_dispatch_scene_carries_the_viewports_pan() {
        let ctx = Mutex::new(RenderContext {
            camera_pan: [-0.6, 0.3],
            ..Default::default()
        });
        let snapshot = SceneSnapshot::capture(&ctx).expect("default resolves");
        let state = scene_state_from_snapshot(&snapshot, 64, 64);
        assert_eq!(state.camera_pan, [-0.6, 0.3]);
        let centred = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(
            scene_state_from_snapshot(&centred, 64, 64).camera_pan,
            [0.0, 0.0]
        );
    }

    /// The live chunks carry the pose the Live Render tab shows, and the stone as designed while
    /// the Edit tab is shown; the planes stay as designed either way (the worker turns them).
    #[test]
    fn the_live_dispatch_scene_carries_the_stone_pose_of_the_live_tab_only() {
        let pose = pleochroma::render_setup::StonePose {
            tilt_deg: 60.0,
            turn_deg: 30.0,
            spin_deg: 5.0,
        };
        let planes = pleochroma::geometry::cuts::StandardGemCuts::standard_round_brilliant();
        let scene_for = |live_tab_shown: bool| {
            let ctx = Arc::new(Mutex::new(RenderContext {
                stone_pose: pose,
                live_tab_shown,
                ..Default::default()
            }));
            let snapshot = capture_live(&ctx).expect("default resolves");
            scene_state_from_snapshot(&snapshot, 64, 64)
        };
        let live = scene_for(true);
        assert_eq!(live.stone_pose, pose.to_array());
        assert_eq!(live.planes, planes, "the planes stay as designed");
        assert_eq!(scene_for(false).stone_pose, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_live_dispatch_scene_of_a_stone_that_is_not_closed_has_no_shadow_and_no_reflection() {
        let scene_for = |planes: Vec<pleochroma::geometry::plane::GpuFacetPlane>| {
            let ctx = Mutex::new(RenderContext {
                stone_shadow: true,
                tray_reflection: 0.04,
                active_planes: std::sync::Arc::new(planes),
                ..Default::default()
            });
            let snapshot = SceneSnapshot::capture(&ctx).expect("default resolves");
            scene_state_from_snapshot(&snapshot, 64, 64)
        };
        let degenerate = scene_for(Vec::new());
        assert!(!degenerate.stone_shadow);
        assert_eq!(degenerate.tray_reflection.to_bits(), 0);

        let closed =
            scene_for(pleochroma::geometry::cuts::StandardGemCuts::standard_round_brilliant());
        assert!(closed.stone_shadow);
        assert_eq!(closed.tray_reflection.to_bits(), 0.04f32.to_bits());
    }

    #[test]
    fn connection_is_stale_is_false_when_nothing_is_cached_yet() {
        // Before the first ever dispatch: nothing to tear down, regardless of whether
        // a worker is configured.
        assert!(!connection_is_stale(None, None));
        assert!(!connection_is_stale(None, Some(&worker_named("a"))));
    }

    #[test]
    fn connection_is_stale_is_false_when_the_cached_worker_is_unchanged() {
        let w = worker_named("a");
        assert!(!connection_is_stale(Some(&w), Some(&w)));
    }

    #[test]
    fn connection_is_stale_is_true_when_the_address_or_cert_dir_differs() {
        let cached = worker_named("a");
        let mut edited_address = cached.clone();
        edited_address.address = "somewhere-else.local:9443".to_string();
        assert!(
            connection_is_stale(Some(&cached), Some(&edited_address)),
            "an edited address must force a reconnect"
        );

        let mut edited_cert_dir = cached.clone();
        edited_cert_dir.cert_dir = "/certs/somewhere-else".to_string();
        assert!(
            connection_is_stale(Some(&cached), Some(&edited_cert_dir)),
            "an edited cert_dir must force a reconnect -- the whole point of mutual \
             TLS here is that a connection authenticated under the OLD certificates \
             must never be reused as if the new ones had been verified"
        );
    }

    #[test]
    fn connection_is_stale_is_true_when_the_configured_worker_was_removed() {
        assert!(
            connection_is_stale(Some(&worker_named("a")), None),
            "no worker configured any more (removed from the list, or remote compute \
             switched off) must tear down a cached connection rather than leave it \
             dangling"
        );
    }

    #[test]
    fn connection_is_stale_is_true_when_a_different_worker_is_now_first() {
        assert!(connection_is_stale(
            Some(&worker_named("a")),
            Some(&worker_named("b"))
        ));
    }
}
