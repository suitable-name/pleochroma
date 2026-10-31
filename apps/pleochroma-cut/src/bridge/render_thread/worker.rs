//! Progressive live worker; preview lifetimes never enter the final sample epoch.

use super::{
    FirstHitSnapshot, MaterialOverrides, MaterialSources, RenderContext, context, display_thread,
    engine, frame_helpers, gpu_backend, interactive, live_split, local_preview, metrics_worker,
    pacing,
};
use crate::bridge::{
    frame_cache::{girdle_finish::GirdleFinishCache, stone_width::StoneWidthCache},
    sample_cursor::LiveEpoch,
};
use context::{FrameInputs, resolve_frame_material, snapshot_frame_inputs};
use display_thread::spawn_display_thread;
use glam::Vec3;
use gpu_backend::{BackendFrame, FrameOutputs, HybridPacing, ViewportGpu};
use interactive::{PassBudget, PassClaim};
use local_preview::{DragGesture, PreviewState, RenderPhase};
use metrics_worker::{MetricsWorker, SharedGeometry, push_if_changed, push_if_fresh};
use pleochroma::{
    geometry::plane::GpuFacetPlane,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish},
    },
};
use pleochroma_net::scene::effects::SceneEffects;
use slint::Weak;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::Instant,
};

use pacing::{DENOISE_MIN_INTERVAL, FRAME_PACE};

use frame_helpers::{
    AccumulationBuffers, FrameActivityFlags, SuspensionFlags, TraceActivitySink, pacing_sleep,
    push_trace_progress, remote_suspends_local, resolve_remote_ownership, should_combine_remote,
    update_accumulation_state,
};

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

/// Spawns the live render thread and returns its handle.
#[expect(
    clippy::too_many_lines,
    reason = "the render worker thread's full loop (resize/dirty handling, sampling, \
              progressive accumulation, UI callbacks); splitting it apart risks changing \
              GUI render-thread behaviour verifiable only by launching the app"
)]
pub fn spawn_render_thread<T, F, M, S, R>(
    ui_weak: Weak<T>,
    ctx: Arc<Mutex<RenderContext>>,
    update_image: F,
    update_metrics: M,
    update_gpu_status: S,
    update_trace_refusal: R,
) where
    // `TraceActivitySink` lets this function stay generic over `T`.
    T: TraceActivitySink + 'static,
    F: Fn(&T, slint::SharedPixelBuffer<slint::Rgba8Pixel>, u64) + Send + 'static + Clone,
    M: Fn(&T, f32, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
    // Callback to push GPU status messages to the UI. `slint::SharedString` must
    // outlive the `upgrade_in_event_loop` closure.
    S: Fn(&T, slint::SharedString) + Send + 'static + Clone,
    // Callback to push `RenderContext::material_unresolved`'s cutter-facing reason (or
    // an empty string once resolved) to `ViewportModel.trace_refusal` -- see this
    // function's own `last_trace_refusal` doc comment for when it fires.
    R: Fn(&T, slint::SharedString) + Send + 'static + Clone,
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
        // Publish each metrics version once, including while idle.
        let mut pushed_metrics_version: u64 = 0;
        let mut last_pushed_pitch_deg: Option<f32> = None;
        // Acquire once; material pipelines are compiled lazily and cached.
        let mut gpu_backend = ViewportGpu::acquire();
        // Last GPU status message pushed to UI, cached to avoid redundant writes.
        let mut last_gpu_status: Option<String> = None;
        // None ensures the initial refusal status is published once.
        let mut last_trace_refusal: Option<String> = None;
        let mut hybrid_pacing = HybridPacing::new();
        let mut engine_cache = engine::EngineAccumulator::default();
        let mut pass_budget = PassBudget::default();
        // Recomputed only when the active design's geometry actually changes.
        let mut girdle_cache = GirdleFinishCache::new();
        let mut stone_width_cache = StoneWidthCache::new();
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
                camera_moving,
                camera_drag_held,
                env_map,
                scene_generation,
            } = snapshot_frame_inputs(&ctx);
            let camera_moving =
                drag_gesture.moving(camera_moving, camera_drag_held, scene_generation);

            if !running {
                break;
            }

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

            let (phase, phase_changed) = preview.select(
                scene_generation,
                camera_moving,
                remote_active,
                live_compute_target,
                !display.busy(),
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
            let dirty = dirty || phase_changed;

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
            // `last_gpu_status` just above.
            if material_unresolved.as_deref() != last_trace_refusal.as_deref() {
                last_trace_refusal.clone_from(&material_unresolved);
                let text: slint::SharedString = last_trace_refusal.as_deref().unwrap_or("").into();
                let update_trace_refusal = update_trace_refusal.clone();
                let _ = ui_weak_metrics_only.upgrade_in_event_loop(move |ui| {
                    update_trace_refusal(&ui, text);
                });
            }

            if suspension.tracing_suspended() {
                // An invisible tab alone (the Edit tab's default
                // Solid view mode) must not also freeze the gemological HUD/tilt-dialog
                // metrics while the cutter keeps editing -- only a hard suspend does
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
                        env_map.as_ref(),
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
            // light direction. Expensive (single-threaded, twenty evaluations after a
            // light/cut/material change), and dependent only on (active_planes,
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
                env_map.as_ref(),
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
            let local_converged =
                accum_samples.saturating_add(remote_samples_done_now) >= trace_target;
            // `redisplay_requested` (the denoise toggle) is treated exactly like
            // `remote_advanced` here -- see `RenderContext::redisplay_requested`'s own
            // doc comment: it must fall through to a display cycle below even though
            // nothing new was traced, rather than taking this early sleep and never
            // re-tonemapping the already-converged buffer at all.
            if local_converged && !dirty && !remote_advanced && !publication_pending {
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
            let local_claim = if local_converged && !dirty {
                None
            } else {
                live_split::local_frame_claim(live_epoch.as_deref(), accum_samples, spp)
            };
            if local_claim.is_none()
                && !remote_advanced
                && !publication_pending
                && !accumulation_reset
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
            let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG).with_roll(camera_roll);

            // A loaded HDR panorama replaces the analytic studio rig as this frame's
            // environment source. The GPU megakernel has its own `env_mode` for
            // `HdrMap` and renders it directly; `accumulate_frame_samples` still falls
            // through to the CPU path (`render_frame_scanlines`) on a decline, but that
            // is the same generic per-frame fallback every other scene gets (no
            // adapter, device lost, `gpu` feature off, or a map past the adapter's
            // storage-buffer limit), not an HDR-specific one.
            let environment = env_map.as_deref().map_or_else(
                || {
                    lighting_preset
                        .studio(exposure, light_yaw, light_pitch)
                        .with_backdrop(backdrop.resolve(lighting_preset, light_yaw, light_pitch))
                        .with_surface_glare(surface_glare)
                        .with_head_shadow(head_shadow_deg)
                },
                EnvironmentSource::HdrMap,
            );

            // Skipped when this iteration exists only to refresh the display with
            // remote's latest contribution -- see `local_claim`'s own comment above.
            // `accum_samples` was left un-incremented for exactly this case, so
            // `accum_buffer`'s sample count stays accurate.
            if let Some((sample_offset, frame_spp)) = local_claim {
                let mut claim = PassClaim::new(live_epoch.as_deref(), sample_offset, frame_spp);
                let pass_start = Instant::now();
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
                let facet_surfaces = effects.surfaces();
                let backend_frame = BackendFrame {
                    width,
                    height,
                    yaw,
                    pitch,
                    distance,
                    camera: &camera,
                    planes: &active_planes,
                    tools: &active_tools,
                    fluorescence: fluorescence
                        .as_deref()
                        .unwrap_or_else(|| Fluorescence::none()),
                    facet_finishes,
                    surface: effects.surface_scene(&facet_surfaces),
                    material: &traced_mat,
                    max_bounces,
                    environment,
                    spp: frame_spp,
                    // The claimed range's first absolute index -- the disjointness
                    // guarantee: while combining, the epoch's cursor never hands
                    // this range to the remote lane too. See this module's doc
                    // comment.
                    sample_offset,
                };
                // Told before the frame so the guide prepass skips the signature walk
                // that `sync_path_sig` below would discard (UI-1 a).
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

            if cancel.load(Ordering::Acquire) {
                continue;
            }
            if accum_samples == 0 && remote_samples_done_now == 0 {
                // The last ranges can all belong to an in-flight remote chunk.
                // Keep the useful preview visible until there is real radiance.
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
            let current_sample_count = accum_samples;

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
                && accum_samples.saturating_add(remote_samples_done_now) >= target_samples;
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
                            accum_buffer: &accum_buffer,
                            first_hit_depth: &first_hit_depth,
                            first_hit_normal: &first_hit_normal,
                            first_hit_facet_id: &first_hit_facet_id,
                            first_hit_path_sig: &first_hit_path_sig,
                        },
                        evaluated.snapshot(cam_pitch_deg),
                        // `push_frame_to_ui`'s own activity start/finish reads
                        // these straight off this cycle's already-computed
                        // `camera_moving`/`converged_now` -- see
                        // `frame_helpers::FrameActivityFlags`'s own doc comment.
                        FrameActivityFlags {
                            phase,
                            camera_moving,
                            converged: converged_now,
                            samples: if phase.is_preview() {
                                0
                            } else {
                                accum_samples.saturating_add(remote_samples_done_now)
                            },
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
