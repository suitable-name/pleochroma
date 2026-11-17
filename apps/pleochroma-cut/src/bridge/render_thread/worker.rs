//! Progressive live worker; preview lifetimes never enter the final sample epoch.

#[cfg(feature = "adaptive-sampling")]
use super::adaptive_view;
use super::{
    FirstHitSnapshot, MaterialOverrides, MaterialSources, RenderContext, context, display_thread,
    engine, frame_helpers, gpu_backend, interactive, live_split, local_preview, metrics_worker,
    pacing, posed_stone, remote_lane, reshade_view, tray_shadow_worker, viewport_cache,
};
#[cfg(feature = "beam")]
use super::{ray_beams, still_beams};
use crate::bridge::{
    frame_cache::{girdle_finish::GirdleFinishCache, stone_width::StoneWidthCache},
    sample_cursor::LiveEpoch,
};
#[cfg(feature = "adaptive-sampling")]
use adaptive_view::{AdaptiveView, PassOutcome, Wanted};
use context::{FrameInputs, resolve_frame_material, snapshot_frame_inputs};
use display_thread::spawn_display_thread;
use glam::Vec3;
use gpu_backend::{BackendFrame, FrameOutputs, HybridPacing, ViewportGpu};
use interactive::{PassBudget, PassClaim};
use local_preview::{DragGesture, PreviewState, RenderPhase};
use metrics_worker::{MetricsWorker, SharedGeometry, push_if_changed, push_if_fresh};
use pleochroma::{
    color::metrics::GemOpticalMetrics,
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, surface::ScratchBasis,
        },
    },
};
use pleochroma_net::scene::effects::SceneEffects;
#[cfg(feature = "beam")]
use ray_beams::{RayBeamStep, RayBeamWanted, RayBeams, RayLaunch};
use reshade_view::{RayView, Relight, RelightJob, ReshadeView, ReshadeWanted};
use slint::Weak;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::Instant,
};
#[cfg(feature = "beam")]
use still_beams::{BeamStep, BeamWanted, Launch, StillBeams};
use viewport_cache::{CacheOutcome, CacheWanted, ViewportCache};

use pacing::{DENOISE_MIN_INTERVAL, FRAME_PACE};

use frame_helpers::{
    AccumulationBuffers, FrameActivityFlags, SuspensionFlags, TraceActivitySink, pacing_sleep,
    push_trace_progress, remote_suspends_local, resolve_remote_ownership, should_combine_remote,
    update_accumulation_state,
};

/// The adaptive view of a build without the `adaptive-sampling` feature: it never engages, so the
/// viewport accumulates as usual whatever the saved "Adaptive Sampling in the Viewport" switch
/// says.
#[cfg(not(feature = "adaptive-sampling"))]
struct AdaptiveView;

#[cfg(not(feature = "adaptive-sampling"))]
#[expect(
    clippy::unused_self,
    reason = "the same calls as the real view's, which answer from its run"
)]
impl AdaptiveView {
    /// No run is ever engaged, so none has finished.
    const fn is_finished(&self) -> bool {
        false
    }

    /// No run is ever engaged, so it has shown no samples.
    const fn samples_shown(&self) -> u32 {
        0
    }

    /// No run is ever engaged, so there are no per-pixel means.
    const fn means(&self) -> Option<&[Vec3]> {
        None
    }
}

/// The Tray Reflection resolved for one scene generation: the glossy plane under the stone, or
/// the reason the requested reflection is not available.
struct TrayMirrorPlan {
    /// The scene generation the plane was resolved for.
    generation: u64,
    /// The plane and its head-on reflectance; `None` when the reflection is off or declined.
    mirror: Option<pleochroma::optics::raytracer::environment::TrayMirror>,
    /// The note that says why a requested reflection is not available.
    refusal: Option<&'static str>,
}

/// The finish of each of the frame's facets: the authored frosted planes and the frosted
/// girdle. `&[]` means all-polished, which is every design that frosts nothing.
///
/// One list for the picture and for the metrics, so the numbers describe the stone that is
/// drawn.
fn frame_finishes<'c>(
    cache: &'c mut GirdleFinishCache,
    planes: &[GpuFacetPlane],
    girdle_frosted: bool,
    frosted: &[u32],
) -> &'c [FacetFinish] {
    if !frosted.is_empty() {
        cache.ensure_with_frosted(planes, girdle_frosted, frosted)
    } else if girdle_frosted {
        cache.ensure(planes)
    } else {
        &[]
    }
}

/// Spawns the live render thread. It runs until `running` is cleared.
#[expect(
    clippy::too_many_lines,
    reason = "the render worker thread's full loop (resize/dirty handling, sampling, \
              progressive accumulation, UI callbacks); splitting it apart risks changing \
              GUI render-thread behaviour verifiable only by launching the app"
)]
pub fn spawn_render_thread<T, F, M, S, R, N>(
    ui_weak: Weak<T>,
    ctx: Arc<Mutex<RenderContext>>,
    update_image: F,
    update_metrics: M,
    update_gpu_status: S,
    update_trace_refusal: R,
    update_trace_note: N,
) where
    // `TraceActivitySink` lets this function stay generic over `T`.
    T: TraceActivitySink + 'static,
    F: Fn(&T, slint::SharedPixelBuffer<slint::Rgba8Pixel>, u64) + Send + 'static + Clone,
    M: Fn(&T, GemOpticalMetrics, [f32; 19], [f32; 19], [f32; 19], f32) + Send + 'static + Clone,
    // Callback to push GPU status messages to the UI. `slint::SharedString` must
    // outlive the `upgrade_in_event_loop` closure.
    S: Fn(&T, slint::SharedString) + Send + 'static + Clone,
    // Callback to push `RenderContext::material_unresolved`'s cutter-facing reason (or
    // an empty string once resolved) to `ViewportModel.trace_refusal`.
    R: Fn(&T, slint::SharedString) + Send + 'static + Clone,
    // Callback to push the non-blocking trace note (the Stone Shadow's, the Tray Reflection's and
    // the accelerators' soft notes, empty when none) to `ViewportModel.trace_note`; the picture
    // stays valid.
    N: Fn(&T, slint::SharedString) + Send + 'static + Clone,
{
    thread::spawn(move || {
        let mut last_width = 0;
        let mut last_height = 0;

        let materials = GemMaterial::all_materials();
        let mut accum_buffer: Vec<Vec3> = Vec::new();
        // Reuse committed guides and accumulation across progressive passes.
        let mut first_hit_depth: Vec<f32> = Vec::new();
        let mut first_hit_normal: Vec<Vec3> = Vec::new();
        let mut first_hit_facet_id: Vec<i32> = Vec::new();
        // Denoiser path signatures are copied from the guide cache.
        let mut first_hit_path_sig: Vec<u32> = Vec::new();
        // Last remote contribution sent to the display thread.
        let mut last_displayed_remote_samples: u32 = 0;
        let mut accum_samples: u32 = 0;
        let mut preview = PreviewState::default();
        let mut drag_gesture = DragGesture::default();
        let mut publication_pending = true;
        let mut last_pass_elapsed = std::time::Duration::ZERO;
        // None forces immediate display after a reset.
        let mut last_denoise_at: Option<Instant> = None;
        // Metrics run independently of interactive tracing.
        let mut metrics_worker = MetricsWorker::spawn();
        let mut shadow_worker = tray_shadow_worker::TrayShadowWorker::spawn();
        let mut last_shadow_key = None;
        // The shadow request of the scene generation it was built for (`None` inside: no
        // shadow wanted), so the idle loop does not re-resolve the material or re-hash keys.
        let mut shadow_plan: Option<(u64, Option<tray_shadow_worker::Planned>)> = None;
        // The Tray Reflection's plane of the scene generation it was resolved for, and the
        // reason a requested reflection is not available. The plane is a few vertex reads, so
        // it is computed here on the render thread, once per scene generation.
        let mut mirror_plan: Option<TrayMirrorPlan> = None;
        // Publish each metrics version once, including while idle.
        let mut pushed_metrics_version: u64 = 0;
        let mut last_pushed_pitch_deg: Option<f32> = None;
        // Acquire once; material pipelines are compiled lazily and cached.
        let mut gpu_backend = ViewportGpu::acquire();
        // Last GPU status message pushed to UI, cached to avoid redundant writes.
        let mut last_gpu_status: Option<String> = None;
        // None ensures the initial refusal status is published once.
        let mut last_trace_refusal: Option<String> = None;
        // The non-blocking trace note last pushed; same convention as `last_trace_refusal`.
        let mut last_trace_note: Option<String> = None;
        let mut hybrid_pacing = HybridPacing::new();
        let mut engine_cache = engine::EngineAccumulator::default();
        let mut pass_budget = PassBudget::default();
        // The opt-in adaptive sampling of a still view; idle while the preference is off.
        #[cfg(feature = "adaptive-sampling")]
        let mut adaptive_view = AdaptiveView::default();
        #[cfg(not(feature = "adaptive-sampling"))]
        let adaptive_view = AdaptiveView;
        // The opt-in path cache of a still view (memory only); idle while the preference is off.
        let mut viewport_cache = ViewportCache::default();
        // The opt-in stored light paths of a still Ray-Traced view: a lighting-only change
        // re-lights them at once; idle while the preference is off.
        let mut reshade_view = ReshadeView::default();
        // The opt-in beam job of a still view; inert while the preference is off.
        #[cfg(feature = "beam")]
        let mut still_beams = StillBeams::default();
        // The opt-in beam job of a still Ray-Traced view (instant lighting with beams); inert
        // while the preference is off.
        #[cfg(feature = "beam")]
        let mut ray_beams = RayBeams::default();
        // Half-buffer B of the cached GPU pass, which the viewport has no use for.
        let mut cache_scratch: Vec<Vec3> = Vec::new();
        // Recomputed only when the active design's geometry actually changes.
        let mut girdle_cache = GirdleFinishCache::new();
        let mut stone_width_cache = StoneWidthCache::new();
        // The Stone Shadow's pause around a pose drag, and the posed stone of the traced frame
        // (turned once per scene generation).
        let mut pose_hold = posed_stone::PoseDragHold::default();
        let mut traced_pose = posed_stone::PosedFrameCache::default();
        // Denoise+tonemap+push runs on a dedicated thread. This loop keeps copies
        // of `ui_weak`/`update_metrics` for the metrics-only push on the suspended
        // path, which bypasses `display` so invisible tabs don't pay for
        // denoise+tonemap cycles.
        let ui_weak_metrics_only = ui_weak.clone();
        let update_metrics_metrics_only = update_metrics.clone();
        let display = spawn_display_thread(ui_weak, update_image, update_metrics);
        // Single choke point for releasing stale remote-image ownership -- see
        // `resolve_remote_ownership`. `false` matches `RenderContext::remote_active`'s
        // `Default`, so the first iteration never misfires as a release.
        let mut prev_remote_active = false;

        loop {
            let iteration_start = Instant::now();
            let FrameInputs {
                cancel,
                engine,
                width,
                height,
                yaw,
                pitch,
                camera_roll,
                camera_pan,
                distance,
                light_yaw,
                light_pitch,
                material_name,
                material_override,
                material_unresolved,
                lighting_preset,
                backdrop,
                surface_glare,
                head_shadow_deg,
                stone_shadow,
                tray_reflection,
                stone_pose,
                pose_drag_held,
                target_samples,
                max_bounces,
                exposure,
                inclusion_sigma_s,
                c_axis_override,
                alpha_axis_override,
                girdle_frosted,
                edge_rounding_radius,
                scene_effects,
                stone_width_mm,
                active_planes,
                active_frosted,
                active_tools,
                custom_materials,
                fluorescence,
                running,
                dirty,
                paused,
                tab_visible,
                denoise_enabled,
                redisplay_requested,
                remote_active: remote_active_snapshot,
                live_epoch: live_epoch_snapshot,
                export_active,
                live_compute_target,
                local_compute_target,
                local_preview_scale,
                adaptive_viewport,
                path_cache_viewport,
                instant_lighting,
                #[cfg(feature = "beam")]
                beam_viewport,
                #[cfg(feature = "beam")]
                beam_instant_lighting,
                camera_moving,
                camera_drag_held,
                env_map,
                hdr_orientation,
                scene_generation,
                pose_generation,
            } = snapshot_frame_inputs(&ctx);
            // A build without the `adaptive-sampling` feature keeps the saved switch but never
            // samples adaptively, whatever it says.
            #[cfg(not(feature = "adaptive-sampling"))]
            let _ = adaptive_viewport;
            let camera_moving =
                drag_gesture.moving(camera_moving, camera_drag_held, scene_generation);
            // Exit collection, the default estimator, does not use beam tracing: the
            // Path-Traced view never beam traces while the app collects. The Ray-Traced view
            // has no estimator and keeps its beams.
            #[cfg(feature = "beam")]
            let beams_collect_refused = beam_viewport
                && pleochroma::optics::raytracer::default_estimator()
                    == pleochroma::optics::raytracer::Estimator::ExitCollection;

            if !running {
                break;
            }

            // Poll before suspension and convergence exits, including remote-only views.
            // The request (resolved material and both keys) is rebuilt only when the scene
            // generation moves: every input of it is part of the scene key, so an idle or
            // converged loop pays nothing for it.
            if shadow_plan
                .as_ref()
                .is_none_or(|(generation, _)| *generation != scene_generation)
            {
                let plan = (stone_shadow && env_map.is_none()).then(|| {
                    let material = resolve_frame_material(
                        &MaterialSources {
                            materials: &materials,
                            custom_materials: &custom_materials,
                            material_override: material_override.as_ref(),
                            material_name: &material_name,
                        },
                        &MaterialOverrides {
                            inclusion_sigma_s,
                            c_axis_override,
                            alpha_axis_override,
                            edge_rounding_radius,
                            stone_width_mm,
                        },
                        &active_planes,
                        &mut stone_width_cache,
                    );
                    let material = match scene_effects.apply_to_material(&material) {
                        std::borrow::Cow::Borrowed(_) => material,
                        std::borrow::Cow::Owned(traced) => traced,
                    };
                    // The shadow of the stone as it stands on the tray: the pose turns the planes,
                    // tools and traced material after the material is resolved.
                    let (planes, tools, material) = posed_stone::posed_shadow_stone(
                        stone_pose,
                        &active_planes,
                        &active_tools,
                        material,
                    );
                    tray_shadow_worker::Planned::new(tray_shadow_worker::Request {
                        planes,
                        tools,
                        material,
                        preset: lighting_preset,
                        yaw: light_yaw,
                        pitch: light_pitch,
                        backdrop: backdrop.resolve(lighting_preset, light_yaw, light_pitch),
                        max_bounces,
                    })
                });
                shadow_plan = Some((scene_generation, plan));
            }
            let shadow_request = shadow_plan.as_ref().and_then(|(_, plan)| plan.as_ref());
            // A hidden, paused, exporting or unresolved view traces nothing, so it starts no new
            // build either (the ready map stays). A remote render does not count: it needs the map.
            // A held pose drag starts none either, nor does the quarter second after its release;
            // the Tray Reflection below has nothing to build and stays live.
            let pose_settled = pose_hold.builds_allowed(pose_drag_held, Instant::now());
            let shadow_builds_allowed =
                !(paused || !tab_visible || export_active || material_unresolved.is_some())
                    && pose_settled;
            let remote_shadow = remote_active_snapshot
                || live_epoch_snapshot.is_some()
                || matches!(
                    live_compute_target,
                    crate::settings::model::LiveComputeTarget::RemoteOnly
                );
            let shadow_eligible = stone_shadow
                && env_map.is_none()
                && lighting_preset != pleochroma::optics::raytracer::LightingPreset::Aset
                && !lighting_preset.is_uv_lamp()
                && backdrop.resolve(lighting_preset, light_yaw, light_pitch) > 0.0;
            let (shadow_map, _) =
                shadow_worker.update(shadow_request, remote_shadow, shadow_builds_allowed);
            let map_key = shadow_map.as_ref().map(|map| map.key());
            let shadow_reset = map_key != last_shadow_key;
            last_shadow_key = map_key;

            // The tray reflection: the plane under the stone, resolved when the scene generation
            // moves (every input of it is part of the scene key). A scope the reflection does not
            // reach (HDR, ASET, an ultraviolet lamp, no backdrop) leaves it off with a note.
            if mirror_plan
                .as_ref()
                .is_none_or(|plan| plan.generation != scene_generation)
            {
                let scope = env_map.as_deref().map_or_else(
                    || {
                        lighting_preset
                            .studio(exposure, light_yaw, light_pitch)
                            .with_backdrop(backdrop.resolve(
                                lighting_preset,
                                light_yaw,
                                light_pitch,
                            ))
                    },
                    |map| {
                        EnvironmentSource::hdr_at(map, exposure)
                            .with_hdr_rotation(hdr_orientation.rotation_for(yaw))
                    },
                );
                // The plane follows the stone as it stands on the tray: its lowest vertex.
                let (mirror_planes, mirror_tools) =
                    posed_stone::posed_geometry(stone_pose, &active_planes, &active_tools);
                let (mirror, refusal) = match pleochroma::render_setup::tray_mirror_for(
                    tray_reflection,
                    scope,
                    pleochroma::geometry::tool::StoneGeometry {
                        planes: &mirror_planes,
                        tools: &mirror_tools,
                    },
                ) {
                    Ok(mirror) => (mirror, None),
                    Err(reason) => (None, Some(reason.message())),
                };
                mirror_plan = Some(TrayMirrorPlan {
                    generation: scene_generation,
                    mirror,
                    refusal,
                });
            }
            let (tray_mirror, mirror_refusal) = mirror_plan
                .as_ref()
                .map_or((None, None), |plan| (plan.mirror, plan.refusal));

            // Release stale remote-image ownership the instant a fresh `dirty` arrives
            // for a reason other than the handoff's own start -- see
            // `resolve_remote_ownership`. Shadows `remote_active` with the resolved
            // value for the rest of this frame so a release also resumes tracing THIS
            // iteration, not after a 100ms suspended sleep.
            let remote_active =
                resolve_remote_ownership(dirty, remote_active_snapshot, prev_remote_active);
            // An epoch dispatched for a different scene than this frame traces (a scene
            // change raced the dispatch) is released too -- see
            // `live_split::epoch_scene_mismatch`.
            let scene_mismatch = live_split::epoch_scene_mismatch(
                remote_active,
                live_epoch_snapshot
                    .as_deref()
                    .map(LiveEpoch::scene_generation),
                scene_generation,
            );
            let remote_active = remote_active && !scene_mismatch;
            if remote_active != remote_active_snapshot {
                // Write back so other readers (`orchestrator::poll_tick`'s lane and
                // served_by reconciliation) observe the release promptly -- but only
                // if the context still holds the epoch this frame judged (a newer
                // settle may already have installed a fresh one, with its own `dirty`).
                // A plain ownership release clears the epoch together with
                // `remote_active` without setting `dirty` again (see `RenderContext::
                // clear_remote_state`); a scene mismatch uses the full
                // `release_remote`, since this frame's `dirty` may not be set and local
                // must not keep accumulating on top of indices claimed from the epoch.
                let mut guard = RenderContext::lock(&ctx);
                let same_epoch = guard.live_epoch.as_ref().map(Arc::as_ptr)
                    == live_epoch_snapshot.as_ref().map(Arc::as_ptr);
                if same_epoch {
                    if scene_mismatch {
                        guard.release_remote();
                    } else {
                        guard.clear_remote_state();
                    }
                }
            }
            prev_remote_active = remote_active;

            // The epoch this frame claims from and combines with -- only while
            // combining (`Both` with a resolved `remote_active`); a release this very
            // frame drops it immediately, so local falls back to its own offsets.
            let live_epoch = if should_combine_remote(remote_active, live_compute_target) {
                live_epoch_snapshot
            } else {
                None
            };

            // Read the remote side's current sample count once per iteration (a cheap
            // lock + `u32` read, never the full buffer -- that happens at the display
            // cadence below). Used to decide whether this iteration can skip tracing
            // (the combined total may already reach `target_samples`) and as part of the
            // combined image's true sample count.
            let remote_samples_done_now: u32 =
                live_epoch.as_ref().map_or(0, |epoch| epoch.remote_done());

            if width == 0 || height == 0 {
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // Instant lighting changes: a still Ray-Traced view that holds the light paths of
            // exactly this pose re-lights straight to the final picture. Only the lighting
            // moved (the pose is the cached one), so a light drag is not camera motion either,
            // and the quarter-resolution bootstrap is skipped.
            let relight_possible = reshade_view::possible(
                instant_lighting && !shadow_eligible && tray_mirror.is_none(),
                engine.is_ray(),
                remote_lane::remote_involved(
                    remote_active,
                    live_epoch.is_some(),
                    live_compute_target,
                ),
            );
            let relight_held = relight_possible && reshade_view.holds(pose_generation);
            let camera_moving = camera_moving && !relight_held;
            let (phase, phase_changed) = preview.select_keeping(
                scene_generation,
                camera_moving,
                remote_active,
                live_compute_target,
                !display.busy(),
                local_preview::AfterFinal::from_keep(relight_held),
            );
            let (width, height) =
                local_preview::phase_dimensions(width, height, local_preview_scale, phase);
            let trace_target = if phase.is_preview() {
                1
            } else {
                target_samples
            };
            // The phase boundary invalidates radiance even for identical dimensions.
            // Preview sample zero never consumes the final epoch's sample zero.
            let dirty = dirty || phase_changed || shadow_reset;

            // Captured before `update_accumulation_state` overwrites `last_width`/
            // `last_height` -- invalidates the denoise cadence cache immediately on any
            // reset rather than leaving a stale frame on screen for up to
            // `DENOISE_MIN_INTERVAL` longer. `dimensions_changed` is the narrower of
            // the two: `update_accumulation_state` only reallocates the first-hit guide
            // buffers on a dimension change (see its own doc comment), never on a plain
            // `dirty`, which only clears `accum`.
            let dimensions_changed = width != last_width || height != last_height;
            let accumulation_reset = dirty || dimensions_changed;

            update_accumulation_state(
                width,
                height,
                dirty,
                &mut AccumulationBuffers {
                    accum: &mut accum_buffer,
                    first_hit_depth: &mut first_hit_depth,
                    first_hit_normal: &mut first_hit_normal,
                    first_hit_facet_id: &mut first_hit_facet_id,
                },
                &mut accum_samples,
                &mut last_width,
                &mut last_height,
            );
            if dimensions_changed {
                // The guide buffers were just reallocated to fresh,
                // unrelated-to-any-key contents -- the GPU's cached `applied_guide_key`
                // must not be trusted to still describe what they hold.
                gpu_backend.invalidate_guide_cache();
            }

            if accumulation_reset {
                publication_pending = true;
                last_pass_elapsed = std::time::Duration::ZERO;
                pass_budget.reset();
                last_denoise_at = None;
                // A reset starts a new epoch (or none): its remote count starts from 0
                // again, so "remote advanced past what was shown" must too.
                last_displayed_remote_samples = 0;
                // Invalidates any display cycle already in flight/queued from before
                // this reset -- see `DisplayHandle::bump_generation`.
                display.bump_generation();
            }

            // Adaptive sampling of a still view (the preference; off keeps `adaptive_on`
            // false on every frame and the accumulation below untouched). Synced before the
            // suspension `continue`, so a reset that lands on a suspended frame still drops
            // the run -- `dirty` was consumed by the snapshot.
            #[cfg(feature = "adaptive-sampling")]
            let adaptive_on = adaptive_view.sync(Wanted {
                applies: adaptive_view::applies(
                    adaptive_viewport,
                    engine.is_ray(),
                    remote_lane::remote_involved(
                        remote_active,
                        live_epoch.is_some(),
                        live_compute_target,
                    ),
                    phase,
                    trace_target,
                ),
                reset: accumulation_reset,
                first_pass_done: accum_samples > 0,
                size: (width, height),
                cap: trace_target,
            });
            #[cfg(not(feature = "adaptive-sampling"))]
            let adaptive_on = false;
            // The path cache of a still view: wanted once the pose has refined for
            // `VIEWPORT_CACHE_AFTER_SPP` samples (an adaptive run's samples count, its
            // accumulation stays at the first pass), dropped by the same reset that restarts
            // the accumulation.
            let cache_wanted = viewport_cache.sync(CacheWanted {
                applies: viewport_cache::applies(
                    path_cache_viewport && !shadow_eligible && tray_mirror.is_none(),
                    engine.is_ray(),
                    phase,
                ),
                reset: accumulation_reset,
                samples: if adaptive_on {
                    adaptive_view.samples_shown()
                } else {
                    accum_samples
                },
                size: (width, height),
            });
            // The beam-traced picture of a still view: a background job that starts once the pose
            // has refined for `VIEWPORT_BEAMS_AFTER_SPP` samples and is cancelled by any change of
            // pose, size or engine, or by the view going away. Beams win over the path cache on
            // the stone they draw. With the preference off the state stays inert.
            #[cfg(feature = "beam")]
            let beam_step = still_beams.sync(BeamWanted {
                applies: still_beams::applies(
                    beam_viewport
                        && !beams_collect_refused
                        && !shadow_eligible
                        && tray_mirror.is_none(),
                    engine.is_ray(),
                    remote_lane::remote_involved(
                        remote_active,
                        live_epoch.is_some(),
                        live_compute_target,
                    ),
                    phase,
                ) && tab_visible
                    && !paused
                    && !export_active
                    && material_unresolved.is_none(),
                pose: pose_generation,
                scene: scene_generation,
                size: (width, height),
                samples: if adaptive_on {
                    adaptive_view.samples_shown()
                } else {
                    accum_samples
                },
                instant_lighting: beam_instant_lighting,
            });
            #[cfg(feature = "beam")]
            let cache_wanted = cache_wanted && !still_beams.owns_stone();
            #[cfg(feature = "beam")]
            if still_beams.take_news() {
                publication_pending = true;
            }
            // The stored light paths of a still Ray-Traced view: wanted once its taps are all in,
            // dropped by any change of pose (and by the preference or the phase going away); a
            // change of lighting alone keeps them.
            let relight_build_due = reshade_view.sync(ReshadeWanted {
                applies: relight_possible && reshade_view::in_final(phase),
                pose: pose_generation,
                converged: !accumulation_reset && accum_samples >= trace_target,
                size: (width, height),
            });
            // The beam-traced stone of a still Ray-Traced view: a background job once the
            // Ray-Traced picture of this lighting is finished (its sum of taps is the Whitted
            // background), cancelled by any change of pose, size or engine; a lighting change
            // keeps the leaves. With the preference off the state stays inert.
            #[cfg(feature = "beam")]
            let ray_beam_step = ray_beams.sync(RayBeamWanted {
                applies: ray_beams::applies(
                    // The switch is greyed out while Beam Tracing in Still Views is off.
                    beam_viewport
                        && beam_instant_lighting
                        && !shadow_eligible
                        && tray_mirror.is_none(),
                    engine.is_ray(),
                    remote_lane::remote_involved(
                        remote_active,
                        live_epoch.is_some(),
                        live_compute_target,
                    ),
                    phase,
                ) && tab_visible
                    && !paused
                    && !export_active
                    && material_unresolved.is_none(),
                pose: pose_generation,
                scene: scene_generation,
                size: (width, height),
                converged: !accumulation_reset
                    && accum_samples > 0
                    && accum_samples >= trace_target,
            });
            #[cfg(feature = "beam")]
            if ray_beams.take_news() {
                publication_pending = true;
            }
            // Whether the Ray-Traced beam job has nothing to do this frame; always so without the
            // `beam` feature (beam tracing is retired).
            #[cfg(feature = "beam")]
            let ray_beam_idle = ray_beam_step == RayBeamStep::Idle;
            #[cfg(not(feature = "beam"))]
            let ray_beam_idle = true;

            if redisplay_requested {
                publication_pending = true;
            }

            // Suspended by an explicit user pause, an invisible 3D tab, a remote worker
            // owning the displayed image (`remote_active`), or a running high-res export
            // (`export_active`). Skips raytracing entirely -- the accumulation buffer
            // stays in sync with `dirty`, so resuming continues converging. The sleep is
            // long enough to cost no CPU but short enough (~100ms) to feel responsive.
            // A harness run traces nothing: the view counts as an invisible tab (see
            // `harness_support::disable_viewport_rendering`).
            #[cfg(feature = "ui-harness")]
            let tab_visible = tab_visible && !crate::harness_support::viewport_rendering_disabled();
            let suspension = SuspensionFlags {
                paused,
                tab_visible,
                remote_suspends: remote_suspends_local(remote_active, live_compute_target),
                export_active,
                material_unresolved: material_unresolved.is_some(),
            };

            // Pushed unconditionally (not only in the suspended branch below) so the
            // banner clears the instant the design resolves again, on the very frame
            // tracing resumes -- not one suspended-branch iteration later. Compared as
            // `&str` so an unchanged refusal (or an unchanged healthy `None`) costs one
            // comparison, not a clone, on every iteration -- same convention as
            // `last_gpu_status`.
            let shadow_refusal = if !stone_shadow {
                None
            } else if env_map.is_some() {
                Some(
                    pleochroma::optics::raytracer::environment::tray_shadow::TrayShadowOff::Hdr
                        .message(),
                )
            } else {
                shadow_worker
                    .refusal
                    .map(tray_shadow_worker::Refusal::message)
            };
            let accelerator_refusal = if path_cache_viewport
                && !engine.is_ray()
                && pleochroma::optics::raytracer::default_estimator()
                    == pleochroma::optics::raytracer::Estimator::ExitCollection
            {
                Some("Path cache off: exit collection, the default estimator, does not use it.")
            } else if shadow_eligible && path_cache_viewport {
                Some(
                    "Path cache off: the stone shadow needs exit positions the cache does not keep.",
                )
            } else {
                None
            };
            // The beams' note comes first: with the preference on, beams would have taken the stone.
            #[cfg(feature = "beam")]
            let accelerator_refusal = if shadow_eligible && beam_viewport {
                Some("Beam tracing off: the stone shadow is traced without beams.")
            } else {
                accelerator_refusal
            };
            // The tray reflection declines the accelerators the same way; the shadow's note, when
            // both are on, already names the accelerator.
            let accelerator_refusal =
                if accelerator_refusal.is_none() && tray_mirror.is_some() && path_cache_viewport {
                    Some("Path cache off: the tray reflection needs paths the cache does not keep.")
                } else {
                    accelerator_refusal
                };
            #[cfg(feature = "beam")]
            let accelerator_refusal = if tray_mirror.is_some() && beam_viewport && !shadow_eligible
            {
                Some("Beam tracing off: the tray reflection is traced without beams.")
            } else {
                accelerator_refusal
            };
            // Under exit collection the Path-Traced view's beams are off whatever else applies;
            // the Ray-Traced view keeps them and shows no such note.
            #[cfg(feature = "beam")]
            let accelerator_refusal = if beams_collect_refused && !engine.is_ray() {
                Some("Beam tracing off: exit collection, the default estimator, does not use it.")
            } else {
                accelerator_refusal
            };
            // Only an untraceable design is a refusal (it dims the picture and shows the
            // banner); the shadow's and the accelerators' notes leave the picture valid and go
            // to their own line.
            let refusal = material_unresolved.as_deref();
            if refusal != last_trace_refusal.as_deref() {
                last_trace_refusal = refusal.map(str::to_owned);
                let text: slint::SharedString = last_trace_refusal.as_deref().unwrap_or("").into();
                let update_trace_refusal = update_trace_refusal.clone();
                let _ = ui_weak_metrics_only.upgrade_in_event_loop(move |ui| {
                    update_trace_refusal(&ui, text);
                });
            }
            // With both effects refused, both reasons show (the shadow's first); the joined text
            // is built only then.
            let both_refusals = shadow_refusal
                .zip(mirror_refusal)
                .map(|(shadow, mirror)| format!("{shadow} {mirror}"));
            let note = both_refusals
                .as_deref()
                .or(shadow_refusal)
                .or(mirror_refusal)
                .or(accelerator_refusal);
            if note != last_trace_note.as_deref() {
                last_trace_note = note.map(str::to_owned);
                let text: slint::SharedString = last_trace_note.as_deref().unwrap_or("").into();
                let update_trace_note = update_trace_note.clone();
                let _ = ui_weak_metrics_only.upgrade_in_event_loop(move |ui| {
                    update_trace_note(&ui, text);
                });
            }

            // The loaded map with its turn at this camera yaw: what the metrics are scored under.
            let metrics_env = env_map
                .as_ref()
                .map(|map| (map, hdr_orientation.rotation_for(yaw)));

            if suspension.tracing_suspended() {
                // An invisible tab alone must not also freeze the gemological HUD and tilt-dialog
                // metrics while the cutter keeps editing; only a hard suspend does
                // (see `SuspensionFlags::metrics_suspended`). The worker request is a
                // no-op whenever planes/material/pose haven't moved since the last one,
                // so this costs nothing extra in the common case. Pushed directly via
                // `push_metrics_to_ui`, bypassing `display` entirely, so nothing pays for
                // a denoise+tonemap cycle for an image nobody can see.
                if !suspension.metrics_suspended() {
                    let current_mat = resolve_frame_material(
                        &MaterialSources {
                            materials: &materials,
                            custom_materials: &custom_materials,
                            material_override: material_override.as_ref(),
                            material_name: &material_name,
                        },
                        &MaterialOverrides {
                            inclusion_sigma_s,
                            c_axis_override,
                            alpha_axis_override,
                            edge_rounding_radius,
                            stone_width_mm,
                        },
                        &active_planes,
                        &mut stone_width_cache,
                    );
                    // This branch never reaches the render's own finish list below, so it
                    // asks the (shared) cache itself.
                    let facet_finishes = frame_finishes(
                        &mut girdle_cache,
                        &active_planes,
                        girdle_frosted,
                        &active_frosted,
                    );
                    metrics_worker.request_with_head_shadow(
                        SharedGeometry {
                            planes: &active_planes,
                            tools: &active_tools,
                            finishes: facet_finishes,
                        },
                        &current_mat,
                        [yaw, pitch, light_yaw, light_pitch],
                        lighting_preset,
                        metrics_env,
                        head_shadow_deg,
                    );
                    // Only a newer evaluation or a moved pitch marker reaches the UI.
                    push_if_changed(
                        &metrics_worker,
                        &mut pushed_metrics_version,
                        &mut last_pushed_pitch_deg,
                        &ui_weak_metrics_only,
                        &update_metrics_metrics_only,
                        pitch.to_degrees(),
                    );
                }
                thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }

            if accumulation_reset || redisplay_requested {
                push_trace_progress(
                    &ui_weak_metrics_only,
                    Arc::clone(&cancel),
                    scene_generation,
                    FrameActivityFlags {
                        phase,
                        camera_moving,
                        converged: false,
                        samples: if phase.is_preview() {
                            0
                        } else {
                            accum_samples.saturating_add(remote_samples_done_now)
                        },
                        target_samples,
                        pass_elapsed: std::time::Duration::ZERO,
                    },
                );
            }

            let current_mat = resolve_frame_material(
                &MaterialSources {
                    materials: &materials,
                    custom_materials: &custom_materials,
                    material_override: material_override.as_ref(),
                    material_name: &material_name,
                },
                &MaterialOverrides {
                    inclusion_sigma_s,
                    c_axis_override,
                    alpha_axis_override,
                    edge_rounding_radius,
                    stone_width_mm,
                },
                &active_planes,
                &mut stone_width_cache,
            );

            // Optical metrics: analytical raytracing from the camera PoV accounting for
            // light direction. Expensive (single-threaded), and dependent only on (active_planes,
            // current_mat, yaw, pitch, light_yaw, light_pitch, lighting_preset, env_map),
            // so it runs on the metrics worker: this only queues a request when one of
            // those moved, and the frame shows the last published values until the
            // debounced evaluation lands. The finishes are part of that stone: a frosting edit
            // re-queues it, and the same list is the render's `facet_finishes` below.
            let facet_finishes = frame_finishes(
                &mut girdle_cache,
                &active_planes,
                girdle_frosted,
                &active_frosted,
            );
            metrics_worker.request_with_head_shadow(
                SharedGeometry {
                    planes: &active_planes,
                    tools: &active_tools,
                    finishes: facet_finishes,
                },
                &current_mat,
                [yaw, pitch, light_yaw, light_pitch],
                lighting_preset,
                metrics_env,
                head_shadow_deg,
            );
            let cam_pitch_deg = pitch.to_degrees();

            // Enough samples accumulated in still mode: sleep to conserve power.
            // Combined against remote's contribution too (`0` whenever not combining) --
            // once local plus remote reach the target, further local tracing is wasted.
            //
            // In `LiveComputeTarget::Both`, a converged local half must not
            // stall the display forever at whatever combined count it last actually
            // sent. `remote_samples_done_now` rising past `last_displayed_remote_samples`
            // also catches the remote lane finishing, with no separate check needed: it
            // never clears `remote_active` (`resolve_remote_ownership`'s doc comment),
            // so `should_combine_remote` keeps reading true, and the epoch's FINAL
            // remote count is itself a rise past whatever was last shown. So
            // whenever remote has moved on, fall through to send ONE more display cycle
            // below with no new local tracing, instead of sleeping on a stale image;
            // sleep only when local is converged AND remote has nothing new to show.
            let remote_advanced =
                live_epoch.is_some() && remote_samples_done_now > last_displayed_remote_samples;
            // An adaptive run is converged when every tile has stopped or the cap is reached.
            let local_converged = if adaptive_on {
                adaptive_view.is_finished()
            } else {
                accum_samples.saturating_add(remote_samples_done_now) >= trace_target
            };
            // `redisplay_requested` (the denoise toggle) is treated exactly like
            // `remote_advanced` here -- see `RenderContext::redisplay_requested`'s own
            // doc comment: it must fall through to a display cycle below even though
            // nothing new was traced, rather than taking this early sleep and never
            // re-tonemapping the already-converged buffer at all.
            if local_converged
                && !dirty
                && !remote_advanced
                && !publication_pending
                && !relight_build_due
                && ray_beam_idle
            {
                // No display cycle will carry a metrics evaluation that finished after
                // the converged picture was shown, so send it on its own.
                push_if_fresh(
                    &metrics_worker,
                    &mut pushed_metrics_version,
                    !display.busy(),
                    &ui_weak_metrics_only,
                    &update_metrics_metrics_only,
                    cam_pitch_deg,
                );
                thread::sleep(std::time::Duration::from_millis(60));
                continue;
            }
            // This frame's `(sample_offset, spp)`: local's own continuation, or a claim
            // from the live epoch's shared cursor while combining. `None` when there is
            // nothing left for LOCAL tracing to usefully add -- converged, or every
            // remaining index is claimed by an in-flight remote chunk.
            let spp = pass_budget.samples(
                camera_moving,
                accumulation_reset,
                trace_target.saturating_sub(accum_samples.saturating_add(remote_samples_done_now)),
            );
            let waiting_for_remote_shadow = remote_shadow
                && stone_shadow
                && env_map.is_none()
                && shadow_map.is_none()
                && shadow_worker.refusal.is_none();
            let local_claim = if (local_converged && !dirty) || waiting_for_remote_shadow {
                None
            } else {
                live_split::local_frame_claim(live_epoch.as_deref(), accum_samples, spp)
            };
            if local_claim.is_none()
                && !remote_advanced
                && !publication_pending
                && !accumulation_reset
                && !relight_build_due
                && ray_beam_idle
            {
                // Waiting on the remote chunk that holds the last unclaimed indices --
                // nothing new to trace or show yet.
                push_if_fresh(
                    &metrics_worker,
                    &mut pushed_metrics_version,
                    !display.busy(),
                    &ui_weak_metrics_only,
                    &update_metrics_metrics_only,
                    cam_pitch_deg,
                );
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
            let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG)
                .with_roll(camera_roll)
                .with_pan(camera_pan);

            // A loaded HDR panorama, lit at `exposure`, replaces the analytic studio rig as
            // this frame's environment source. The GPU megakernel has its own `env_mode` for
            // `HdrMap` and renders it directly; `accumulate_frame_samples` still falls
            // through to the CPU path (`render_frame_scanlines`) on a decline, but that
            // is the same generic per-frame fallback every other scene gets (no
            // adapter, device lost, `gpu` feature off, or a map past the adapter's
            // storage-buffer limit), not an HDR-specific one.
            let environment = env_map
                .as_deref()
                .map_or_else(
                    || {
                        lighting_preset
                            .studio(exposure, light_yaw, light_pitch)
                            .with_backdrop(backdrop.resolve(
                                lighting_preset,
                                light_yaw,
                                light_pitch,
                            ))
                            .with_surface_glare(surface_glare)
                            .with_head_shadow(head_shadow_deg)
                    },
                    |map| {
                        EnvironmentSource::hdr_at(map, exposure)
                            .with_hdr_rotation(hdr_orientation.rotation_for(yaw))
                    },
                )
                .with_tray_shadow(shadow_map.as_deref())
                .with_tray_mirror(tray_mirror);

            // The design's render physics: the silk and play-of-color on the traced
            // material, the facet surfaces and abraded edge as the surface input. The
            // metrics above score the plain material (with the frosted facets and girdle,
            // but not the silk, surfaces or rounding), and Ray-Traced ignores the lot.
            let no_effects = SceneEffects::default();
            let effects = if engine.is_ray() {
                &no_effects
            } else {
                &scene_effects
            };
            let traced_mat = effects.apply_to_material(&current_mat);
            // The stone as it stands on the tray (Live Render tab only): everything traced from
            // here on reads the posed planes, tools and material, and the scratch directions of
            // the surface finish turn with it. The metrics above keep the stone as designed.
            let posed = traced_pose.get(
                scene_generation,
                stone_pose,
                (&active_planes[..], &active_tools[..]),
                &traced_mat,
            );
            let (frame_planes, frame_tools, frame_material): (
                &[GpuFacetPlane],
                &[ToolPrimitive],
                &GemMaterial,
            ) = posed.as_deref().map_or_else(
                || (&active_planes[..], &active_tools[..], &*traced_mat),
                |stone| (&stone.planes[..], &stone.tools[..], &stone.material),
            );
            let facet_surfaces = effects.surfaces();
            let mut frame_surface = effects.surface_scene(&facet_surfaces);
            if let Some(stone) = posed.as_deref()
                && effects.has_surface()
            {
                frame_surface.scratch = ScratchBasis::new(stone.up, stone.side);
            }

            // The frame a Ray-Traced view traces; only the instant-lighting code reads it.
            let ray_view = RayView {
                width,
                height,
                yaw,
                pitch,
                distance,
                camera: &camera,
                planes: frame_planes,
                tools: frame_tools,
                fluorescence: fluorescence
                    .as_deref()
                    .unwrap_or_else(|| Fluorescence::none()),
                facet_finishes,
                material: posed
                    .as_deref()
                    .map_or(&current_mat, |stone| &stone.material),
                max_bounces,
                environment,
            };
            if relight_build_due {
                // Phase A of every tile of the finished still pose. The Ray-Traced picture stays
                // on screen meanwhile; a cancel (any edit) abandons the build.
                reshade_view.build(&ray_view, &cancel);
                continue;
            }
            #[cfg(feature = "beam")]
            if ray_beam_step != RayBeamStep::Idle {
                // The stone of the finished Ray-Traced picture from beams: the job starts in the
                // background and the picture stays on screen until it lands.
                let launch = RayLaunch {
                    hdr: env_map.as_ref(),
                    frame_cancel: &cancel,
                    accum: accum_buffer.as_slice(),
                    taps: accum_samples,
                };
                ray_view.with_frame((1, 0), |frame| {
                    ray_beams.launch(ray_beam_step, frame, &launch);
                });
                continue;
            }

            // Skipped when this iteration exists only to refresh the display with
            // remote's latest contribution -- see `local_claim`'s own comment above.
            // `accum_samples` was left un-incremented for exactly this case, so
            // `accum_buffer`'s sample count stays accurate.
            if let Some((sample_offset, frame_spp)) = local_claim {
                let mut claim = PassClaim::new(live_epoch.as_deref(), sample_offset, frame_spp);
                let pass_start = Instant::now();
                let backend_frame = BackendFrame {
                    width,
                    height,
                    yaw,
                    pitch,
                    distance,
                    camera: &camera,
                    planes: frame_planes,
                    tools: frame_tools,
                    fluorescence: fluorescence
                        .as_deref()
                        .unwrap_or_else(|| Fluorescence::none()),
                    facet_finishes,
                    surface: frame_surface,
                    material: frame_material,
                    max_bounces,
                    environment,
                    spp: frame_spp,
                    // The claimed range's first absolute index. While combining, the epoch's
                    // cursor never hands this range to the remote lane too.
                    sample_offset,
                };
                // The beam job of this still pose (or the re-light of its leaves) starts here, in
                // the background; the pass below goes on as usual.
                #[cfg(feature = "beam")]
                if beam_step != BeamStep::Idle {
                    still_beams.launch(
                        beam_step,
                        &backend_frame,
                        &Launch {
                            hdr: env_map.as_ref(),
                            cap: trace_target,
                            frame_cancel: &cancel,
                        },
                    );
                }
                // Instant lighting changes: the light paths of this very pose are held and only the
                // lighting moved, so the picture is drawn from them instead of traced.
                let relit = if relight_held && accumulation_reset && reshade_view::in_final(phase) {
                    let job = RelightJob {
                        engine,
                        generation: scene_generation,
                        shared_hdr: env_map.as_ref(),
                        taps: trace_target,
                    };
                    match reshade_view.relight(
                        &ray_view,
                        &mut engine_cache,
                        &job,
                        accum_buffer.as_mut_slice(),
                        &cancel,
                    ) {
                        Relight::Cancelled => continue,
                        Relight::Done => true,
                        Relight::Unavailable => false,
                    }
                } else {
                    false
                };
                // A still pose that has stood long enough gets its path cache here (phase A
                // of every tile, once); a cancel during the build abandons the frame.
                if cache_wanted
                    && !viewport_cache.ensure_built(&backend_frame, &cancel)
                    && cancel.load(Ordering::Acquire)
                {
                    continue;
                }
                // One ordinary-accumulation pass shaded from the cache, when there is one.
                let cached_pass = if !adaptive_on && viewport_cache.is_built() {
                    match viewport_cache.accumulate_on(
                        &mut gpu_backend,
                        phase.compute(local_compute_target),
                        &backend_frame,
                        (accum_buffer.as_mut_slice(), &mut cache_scratch),
                        &cancel,
                    ) {
                        CacheOutcome::Cancelled => continue,
                        CacheOutcome::Done => true,
                        CacheOutcome::Unavailable => false,
                    }
                } else {
                    false
                };
                if relit {
                    // The picture is in the accumulator already: the stone from the stored light
                    // paths, the rest from the Ray-Traced engine, summed over all the taps.
                    last_pass_elapsed = pass_start.elapsed();
                    publication_pending = true;
                    accum_samples = trace_target;
                    claim.commit();
                    tracing::debug!(
                        taps = trace_target,
                        width,
                        height,
                        elapsed_ms = pass_start.elapsed().as_millis(),
                        "instant lighting: re-lit from the stored light paths"
                    );
                } else if adaptive_on {
                    #[cfg(feature = "adaptive-sampling")]
                    {
                        // One adaptive pass over the live tiles, traced by one engine (the
                        // CPU+GPU split is not used here). The guides of the denoiser stay
                        // those of the ordinary first pass of this accumulation, and the
                        // ordinary sum and sample count are left alone: the display reads
                        // the run's per-pixel means instead. With a path cache the stone
                        // pixels shade from it.
                        let compute = phase.compute(local_compute_target);
                        let outcome = viewport_cache.with_pass_frame(&backend_frame, |cache| {
                            adaptive_view.trace_pass_cached(
                                &mut gpu_backend,
                                compute,
                                &backend_frame,
                                cache,
                                &cancel,
                            )
                        });
                        if outcome == PassOutcome::Cancelled {
                            continue;
                        }
                        last_pass_elapsed = pass_start.elapsed();
                        publication_pending = true;
                        claim.commit();
                        tracing::debug!(
                            samples = adaptive_view.samples_shown(),
                            width,
                            height,
                            elapsed_ms = pass_start.elapsed().as_millis(),
                            "adaptive live render pass completed"
                        );
                    }
                } else if cached_pass {
                    // The pass is in the accumulator already, the stone shaded from the path
                    // cache. The denoiser's guides stay those of the pass that built them.
                    last_pass_elapsed = pass_start.elapsed();
                    publication_pending = true;
                    accum_samples += frame_spp;
                    claim.commit();
                    tracing::debug!(
                        samples = frame_spp,
                        width,
                        height,
                        elapsed_ms = pass_start.elapsed().as_millis(),
                        "cached live render pass completed"
                    );
                } else {
                    // Told before the frame so the guide prepass skips the path-signature walk
                    // that `sync_path_sig` below would discard.
                    let path_sig_wanted = denoise_enabled && !phase.is_preview();
                    gpu_backend.set_path_sig_wanted(path_sig_wanted);
                    let completed = engine::accumulate_engine_samples(
                        &mut engine_cache,
                        engine,
                        scene_generation,
                        env_map.as_ref(),
                        &mut gpu_backend,
                        &backend_frame,
                        &mut pass_budget.begin(accum_buffer.len()),
                        &mut hybrid_pacing,
                        phase.compute(local_compute_target),
                        &cancel,
                    );
                    match completed {
                        Ok(true) if !cancel.load(Ordering::Acquire) => {}
                        Ok(_) => continue,
                        Err(reason) => {
                            tracing::warn!(
                                reason = reason.message(),
                                "image engine declined the scene"
                            );
                            continue;
                        }
                    }
                    // The signature costs a prepass per pose and material, so it is skipped
                    // (zeroed) while the denoiser is off or the camera is moving.
                    if !gpu_backend.sync_path_sig(
                        &backend_frame,
                        &mut first_hit_path_sig,
                        path_sig_wanted,
                        &cancel,
                    ) || cancel.load(Ordering::Acquire)
                    {
                        continue;
                    }
                    pass_budget.commit(
                        &mut FrameOutputs {
                            accum: &mut accum_buffer,
                            depth: &mut first_hit_depth,
                            normal: &mut first_hit_normal,
                            facet_id: &mut first_hit_facet_id,
                        },
                        frame_spp,
                        pass_start.elapsed(),
                    );
                    last_pass_elapsed = pass_start.elapsed();
                    publication_pending = true;
                    accum_samples += frame_spp;
                    claim.commit();
                    tracing::debug!(
                        samples = frame_spp,
                        width,
                        height,
                        elapsed_ms = pass_start.elapsed().as_millis(),
                        "live render pass completed"
                    );
                }
            }

            if cancel.load(Ordering::Acquire) {
                continue;
            }
            if accum_samples == 0 && remote_samples_done_now == 0 {
                // The last ranges can all belong to an in-flight remote chunk.
                // Keep the useful preview visible until there is real radiance.
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
            // The picture to show: an adaptive run's per-pixel means once it has traced a
            // pass, as a running sum of ONE sample (the display divides by its count, and
            // each mean already is its own pixel's); else the ordinary running sum.
            let (shown_accum, shown_count): (&[Vec3], u32) = if adaptive_on {
                adaptive_view
                    .means()
                    .map_or((accum_buffer.as_slice(), accum_samples), |means| (means, 1))
            } else {
                (accum_buffer.as_slice(), accum_samples)
            };
            // Beams draw their finished tiles over that picture (again a running sum of one
            // sample); without a beam job this is the picture above, untouched.
            #[cfg(feature = "beam")]
            let (display_accum, current_sample_count) =
                if still_beams.owns_stone() && still_beams.compose_into(shown_accum, shown_count) {
                    (still_beams.composite(), 1)
                } else if ray_beams.shows() && ray_beams.compose_into(shown_accum, shown_count) {
                    (ray_beams.composite(), 1)
                } else {
                    (shown_accum, shown_count)
                };
            #[cfg(not(feature = "beam"))]
            let (display_accum, current_sample_count) = (shown_accum, shown_count);
            // The samples per pixel the progress bar reports against the cap.
            let shown_samples = if adaptive_on {
                adaptive_view.samples_shown()
            } else {
                accum_samples.saturating_add(remote_samples_done_now)
            };

            // Push `ViewportGpu`'s current self-healing
            // status to the UI whenever it actually changed -- see
            // `ViewportGpu::status_message`'s own doc comment for when this is
            // `Some` (transiently re-acquiring, or permanently on CPU fallback) vs
            // `None` (healthy, or nothing traced this iteration to change it).
            // Compared as `&str` so an unchanged `Some("...")` costs one comparison,
            // not a clone, on every iteration.
            if gpu_backend.status_message() != last_gpu_status.as_deref() {
                last_gpu_status = gpu_backend.status_message().map(str::to_string);
                let text: slint::SharedString = last_gpu_status.as_deref().unwrap_or("").into();
                let update_gpu_status = update_gpu_status.clone();
                let _ = ui_weak_metrics_only.upgrade_in_event_loop(move |ui| {
                    update_gpu_status(&ui, text);
                });
            }

            // Denoise on the READBACK path only -- `accum_buffer` stays the raw running
            // sum (never overwritten), so the progressive estimator stays unbiased;
            // only the displayed tone-mapped image is filtered.
            //
            // `denoise_enabled` gates whether that filtering happens at all -- off is a
            // straight tone-map of the raw average, cheap enough to run every frame.
            //
            // While accumulating, the display cycle is rate-limited to at most once per
            // `DENOISE_MIN_INTERVAL`: tracing is unaffected, but the UI keeps showing
            // the last pushed frame in between (nothing resent, to avoid flickering
            // between filtered and unfiltered). `converged_now` (first iteration
            // reaching `target_samples`) and `accumulation_reset` always attempt a
            // cycle regardless of the gap, since each is the last send before settling
            // or the first after invalidation.
            let converged_now = !phase.is_preview()
                && if adaptive_on {
                    adaptive_view.is_finished()
                } else {
                    accum_samples.saturating_add(remote_samples_done_now) >= target_samples
                };
            let denoise_due = !denoise_enabled
                || accumulation_reset
                || redisplay_requested
                || phase.is_preview()
                || converged_now
                || last_denoise_at.is_none_or(|t| t.elapsed() >= DENOISE_MIN_INTERVAL);

            if denoise_due {
                // The render thread never blocks on a display cycle's cost -- it hands
                // the frame off to `display` and keeps tracing. At most one cycle may
                // be in flight, so a cycle is skipped (not queued) while the previous
                // one runs; `denoise_due` retries next iteration. Exception:
                // `converged_now` must always display exactly once, so that send waits
                // out any in-flight cycle instead of skipping.
                let can_send = if converged_now || phase == RenderPhase::Bootstrap {
                    // Bounded, not an unconditional spin -- a display
                    // thread that has stopped finishing cycles (e.g. it died mid-panic)
                    // must not hang this loop forever; give up on THIS cycle instead.
                    display.wait_until_ready(&cancel)
                } else {
                    !display.busy()
                };

                if can_send {
                    let (metrics_version, evaluated) = metrics_worker.latest();
                    let mut work = display.reclaim();
                    work.set_cancel(Arc::clone(&cancel));
                    work.fill(
                        display.current_generation(),
                        scene_generation,
                        local_preview::denoise_this_cycle(denoise_enabled, phase, accum_samples),
                        FirstHitSnapshot {
                            width,
                            height,
                            current_sample_count,
                            accum_buffer: display_accum,
                            first_hit_depth: &first_hit_depth,
                            first_hit_normal: &first_hit_normal,
                            first_hit_facet_id: &first_hit_facet_id,
                            first_hit_path_sig: &first_hit_path_sig,
                        },
                        &evaluated.snapshot(cam_pitch_deg),
                        // `push_frame_to_ui`'s own activity start/finish reads
                        // these straight off this cycle's already-computed
                        // `camera_moving`/`converged_now` -- see
                        // `frame_helpers::FrameActivityFlags`'s own doc comment.
                        FrameActivityFlags {
                            phase,
                            camera_moving,
                            converged: converged_now,
                            samples: if phase.is_preview() { 0 } else { shown_samples },
                            target_samples,
                            pass_elapsed: last_pass_elapsed,
                        },
                    );
                    // Fold the epoch's remote contribution (finished chunks plus the
                    // in-flight one) into THIS cycle's pooled copy only -- never into
                    // `accum_buffer` itself, and with no second full-frame copy: the sum
                    // is added straight into the buffer `fill` just wrote. This
                    // read-lock-and-add happens at the display cadence, not the hot
                    // per-sample trace path. The count comes from the SAME locked read as
                    // the radiance, so the tone mapper's divisor always matches the sum.
                    if let Some(epoch) = live_epoch.as_ref() {
                        // Records what THIS cycle actually displayed, so the "local
                        // converged" check above knows remote has nothing new the next
                        // time it reads a bigger count.
                        last_displayed_remote_samples =
                            work.add_remote(|dst| epoch.add_remote_into(dst));
                    }
                    display.send(work);
                    publication_pending = false;
                    if phase.is_preview() {
                        preview.published();
                    }
                    pushed_metrics_version = metrics_version;
                    last_pushed_pitch_deg = Some(cam_pitch_deg);
                    last_denoise_at = Some(Instant::now());
                }
            }

            // Pacing -- see `FRAME_PACE`: top up to the frame interval only while the
            // camera moves or a display cycle was due; otherwise go straight to the next
            // chunk.
            if let Some(rest) = pacing_sleep(
                camera_moving || denoise_due,
                iteration_start.elapsed(),
                FRAME_PACE,
            ) {
                thread::sleep(rest);
            }
        }
    });
}
