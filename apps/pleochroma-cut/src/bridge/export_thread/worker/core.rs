//! [`render_accumulation`]: the shared local/remote render core -- calibrates and runs
//! the local/remote concurrent phase and merges every engine's contribution exactly
//! once into a linear accumulation buffer, without writing anything to disk.

#[cfg(feature = "adaptive-sampling")]
mod adaptive;
#[cfg(feature = "beam")]
mod beam;
mod lanes;

use super::{
    taps::RenderTaps,
    types::{AccumulationCarry, AccumulationOutcome},
};
use crate::{
    bridge::export_thread::{
        ExportProgress,
        batch::{
            ExportCtx, HYBRID_MIN_SPP, calibrate_split_confirmed, local_chunk_size,
            run_local_batches,
        },
        cache_run::{self, CacheRun},
        params::{ComputeTarget, ExportParams},
        sample_cursor::SampleCursor,
        scene_snapshot::SceneSnapshot,
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use glam::Vec3;
use lanes::{RenderJob, render_on_ctx};
use pleochroma::{
    geometry::tool::StoneGeometry,
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, EnvironmentSource, build_plane_soa,
            environment::tray_shadow::TrayShadowOff,
        },
    },
    renderer::{
        frame_scene::FrameScene,
        gpu_backend::{GpuBackend, GpuSceneRef, scene_routes_to_gpu_with_tools},
        whitted::WhittedPrep,
    },
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
};

/// The frame of `scene` seen through `camera` at `width x height`, with `material` as the
/// stone's material and `environment` as its lighting.
fn frame_scene_of<'a>(
    scene: &'a SceneSnapshot,
    camera: &'a Camera,
    (width, height): (u32, u32),
    material: &'a GemMaterial,
    environment: EnvironmentSource<'a>,
) -> FrameScene<'a> {
    FrameScene {
        camera,
        width,
        height,
        planes: &scene.active_planes,
        facet_finishes: &scene.facet_finishes,
        material,
        max_bounces: scene.max_bounces,
        environment,
    }
}

/// The Whitted preparation of `frame` when `scene` renders with the ray engine, `None` for every
/// other engine, or the reason the ray engine declines the scene.
fn whitted_prep_of<'a>(
    scene: &SceneSnapshot,
    frame: &FrameScene<'a>,
) -> Result<Option<WhittedPrep<'a>>, String> {
    if !scene.engine.is_ray() {
        return Ok(None);
    }
    WhittedPrep::new(frame, &scene.fluorescence, scene.engine.ray)
        .map(Some)
        .map_err(|reason| reason.to_string())
}

/// Builds the scene/camera/environment context ([`ExportCtx`]) that one render's batches
/// share, and calls `f` with it. [`render_local_share`] uses the same context, so the camera
/// pose, environment and `GpuSceneRef` wiring cannot differ between local and remote lanes.
#[expect(
    clippy::too_many_arguments,
    reason = "the scoped context borrows the frozen scene, pose and both independent GPU backends"
)]
fn with_export_ctx<R>(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    width: u32,
    height: u32,
    gpu: &GpuBackend,
    ray_gpu: Option<&pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend>,
    cancel: &AtomicBool,
    f: impl FnOnce(Result<&ExportCtx<'_>, String>) -> R,
) -> R {
    with_export_ctx_taps(
        scene,
        cam_yaw,
        cam_pitch,
        width,
        height,
        gpu,
        ray_gpu,
        cancel,
        None,
        true,
        &mut |_| {},
        f,
    )
}

/// The progress report of the stone-shadow build at `fraction`, in a render of `samples_total`.
fn shadow_progress_event(fraction: f32, samples_total: u32) -> ExportProgress {
    ExportProgress {
        fraction,
        samples_done: 0,
        samples_total,
        preview: None,
        note: Some("Computing the stone shadow…".to_owned()),
        #[cfg(feature = "beam")]
        beam: None,
    }
}

/// The Whitted GPU backend of a ray-engine render on this computer's GPU. It is acquired once
/// and kept on `carry` for the following frames. `None` for any other render.
fn ray_gpu_of(
    scene: &SceneSnapshot,
    local_compute: LocalComputeTarget,
    compute_target: ComputeTarget,
    carry: &mut AccumulationCarry,
) -> Option<std::sync::Arc<pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend>> {
    if scene.engine.is_ray()
        && local_compute != LocalComputeTarget::Cpu
        && compute_target != ComputeTarget::RemoteOnly
    {
        Some(std::sync::Arc::clone(carry.ray_gpu.get_or_insert_with(
            || {
                std::sync::Arc::new(
                    pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend::acquire(),
                )
            },
        )))
    } else {
        None
    }
}

/// The taps for the path cache's decision, cloned because the render borrows `carry` mutably.
///
/// `None` when the cache was not asked for, when an earlier frame of the sweep refused a
/// one-shot cache (`AccumulationCarry::one_shot_cache_refused`), or when only a remote worker
/// traces: the path cache belongs to the local lane.
fn cache_taps_of(carry: &AccumulationCarry, compute_target: ComputeTarget) -> Option<RenderTaps> {
    (carry.taps.path_cache.is_some()
        && !carry.one_shot_cache_refused
        && !matches!(compute_target, ComputeTarget::RemoteOnly))
    .then(|| carry.taps.clone())
}

/// Queues the decline `reason` (from `scene_decline_with_tray_shadow` or
/// `scene_decline_with_tray_mirror`; `None` when the GPU copes) ahead of any note on `ctx`.
/// The shadow or mirror stays on the scene and the render traces on the CPU.
fn queue_gpu_decline(
    ctx: &ExportCtx<'_>,
    reason: Option<pleochroma::renderer::gpu_backend::GpuRouteDecline>,
) {
    if let Some(reason) = reason {
        let mut note = ctx
            .cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *note = Some(note.take().map_or_else(
            || reason.to_string(),
            |existing| format!("{reason} {existing}"),
        ));
    }
}

/// Queues `BeamOff::Unavailable`'s note on `ctx`, ahead of any existing note, when `taps` ask
/// for beams. Without the `beam` feature such a render proceeds without them.
#[cfg(not(feature = "beam"))]
fn queue_beams_unavailable(ctx: &ExportCtx<'_>, taps: &RenderTaps) {
    if taps.beam.is_none() {
        return;
    }
    let reason = pleochroma::renderer::accel_beam::BeamOff::Unavailable.message();
    let mut note = ctx
        .cache_note
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *note = Some(note.take().map_or_else(
        || format!("{reason}."),
        |existing| format!("{reason}. {existing}"),
    ));
}

/// The note a build without the `adaptive-sampling` feature queues for a render that asks for
/// adaptive sampling.
#[cfg(not(feature = "adaptive-sampling"))]
const ADAPTIVE_UNAVAILABLE_NOTE: &str =
    "Adaptive sampling off: this build has no adaptive sampling.";

/// Queues [`ADAPTIVE_UNAVAILABLE_NOTE`] on `ctx`, ahead of any existing note, when `taps` ask
/// for adaptive sampling. Without the `adaptive-sampling` feature the job renders at its own
/// samples per pixel.
#[cfg(not(feature = "adaptive-sampling"))]
fn queue_adaptive_unavailable(ctx: &ExportCtx<'_>, taps: &RenderTaps) {
    if taps.adaptive.is_none() {
        return;
    }
    let mut note = ctx
        .cache_note
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *note = Some(note.take().map_or_else(
        || ADAPTIVE_UNAVAILABLE_NOTE.to_owned(),
        |existing| format!("{ADAPTIVE_UNAVAILABLE_NOTE} {existing}"),
    ));
}

/// The adaptive branch of [`render_accumulation`]. With `taps.adaptive` set, adaptive sampling
/// runs its own loop, and `Some` ends the render with its result (image, cancel or failure).
/// `None` lets the ordinary render follow, since no adaptive sampling was asked for.
#[cfg(feature = "adaptive-sampling")]
fn render_adaptive_first(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    taps: &RenderTaps,
    progress: &std::cell::RefCell<impl FnMut(ExportProgress)>,
) -> Option<Result<super::types::Accumulation, AccumulationOutcome>> {
    let settings = taps.adaptive?;
    Some(adaptive::render_adaptive_on_ctx(
        ctx,
        job,
        local_compute,
        settings,
        taps,
        &mut *progress.borrow_mut(),
    ))
}

/// The adaptive branch of [`render_accumulation`] without the `adaptive-sampling` feature: a
/// render that asks for adaptive sampling queues the note saying why, then the ordinary render
/// follows.
#[cfg(not(feature = "adaptive-sampling"))]
fn render_adaptive_first(
    ctx: &ExportCtx<'_>,
    _job: &RenderJob<'_>,
    _local_compute: LocalComputeTarget,
    taps: &RenderTaps,
    _progress: &std::cell::RefCell<impl FnMut(ExportProgress)>,
) -> Option<Result<super::types::Accumulation, AccumulationOutcome>> {
    queue_adaptive_unavailable(ctx, taps);
    None
}

/// The beam branch of [`render_accumulation`]. With `carry.taps.beam` set, beam tracing renders
/// an eligible stone and `Some` ends the render with its result. `None` lets the ordinary
/// render follow: no beams were asked for, or the stone cannot be beam-rendered (its note is
/// then queued on `ctx`).
#[cfg(feature = "beam")]
fn render_beams_first(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    progress: &std::cell::RefCell<impl FnMut(ExportProgress)>,
) -> Option<Result<super::types::Accumulation, AccumulationOutcome>> {
    carry.taps.beam?;
    // The probe and beam-tile connections are reused across the frames of a sweep.
    let cache = lanes::BeamLaneCache {
        probed: &mut carry.remote_probed,
        capability: &mut carry.remote_capability,
        clients: &mut carry.beam_clients,
    };
    match beam::render_beams_on_ctx(
        ctx,
        job,
        local_compute,
        &carry.taps,
        cache,
        &mut *progress.borrow_mut(),
    ) {
        beam::BeamAttempt::Done(image) => Some(Ok(image)),
        beam::BeamAttempt::Cancelled => Some(Err(AccumulationOutcome::Cancelled)),
        beam::BeamAttempt::Failed(message) => Some(Err(AccumulationOutcome::Failed(message))),
        beam::BeamAttempt::Off => None,
    }
}

/// `cache_taps` unless beams render the stone of `frame`. Beams and the path cache are
/// alternatives for the stone's pixels: where beams render the stone they win, and the cache
/// is neither chosen nor built (beam guide section 14.6). Otherwise the cache is kept.
#[cfg(feature = "beam")]
fn cache_taps_beside_beams<'t>(
    cache_taps: Option<&'t RenderTaps>,
    frame: &FrameScene<'_>,
    scene: &SceneSnapshot,
    surface: pleochroma::optics::raytracer::surface::SurfaceScene<'_>,
) -> Option<&'t RenderTaps> {
    cache_taps.filter(|taps| {
        !beam::renders_stone(
            taps.beam.as_ref(),
            frame,
            (&scene.tools, &scene.fluorescence, surface),
        )
    })
}

/// The stone-shadow map of an export and the note that says why it has none.
///
/// `(build_shadow, cancel, progress)` are the build's switch, cancellation and progress sink. A
/// shadow that cannot apply (HDR, As Lit, ASET, a UV lamp) or cannot be built renders without it
/// and says why in the note; only a cancel is an error (its message), which stops the render.
fn resolve_stone_shadow(
    scene: &SceneSnapshot,
    environment: EnvironmentSource<'_>,
    material: &GemMaterial,
    (build_shadow, cancel, progress): (bool, &AtomicBool, &mut dyn FnMut(f32)),
) -> Result<
    (
        Option<std::sync::Arc<pleochroma::optics::raytracer::environment::TrayShadowMap>>,
        Option<&'static str>,
    ),
    String,
> {
    if !scene.stone_shadow {
        return Ok((None, None));
    }
    if !scene.stone_shadow_enabled() {
        return Ok((None, Some(TrayShadowOff::AsLit.message())));
    }
    if !scene.stone_closed {
        return Ok((None, Some(TrayShadowOff::DegenerateStone.message())));
    }
    match pleochroma::render_setup::stone_shadow_for(
        build_shadow,
        environment,
        StoneGeometry {
            planes: &scene.active_planes,
            tools: &scene.tools,
        },
        material,
        scene.max_bounces,
        pleochroma::optics::raytracer::environment::TrayShadowQuality::Final,
        cancel,
        progress,
    ) {
        Ok(map) => Ok((map, None)),
        Err(TrayShadowOff::Cancelled) => Err(TrayShadowOff::Cancelled.message().to_owned()),
        Err(reason) => Ok((None, Some(reason.message()))),
    }
}

/// The tray mirror of an export and the note that says why a requested reflection has none.
///
/// The plane is a few vertex reads, so it is resolved here for every render that asks for one,
/// local or remote (a remote-only render never uses it, the worker resolves its own). A
/// reflection that cannot apply (HDR, As Lit, ASET, a UV lamp, a degenerate stone) renders
/// without it and says why; a reflection of `0` is no request and no note.
fn resolve_tray_mirror(
    scene: &SceneSnapshot,
    environment: EnvironmentSource<'_>,
) -> (
    Option<pleochroma::optics::raytracer::environment::TrayMirror>,
    Option<&'static str>,
) {
    match pleochroma::render_setup::tray_mirror_for(
        scene.tray_reflection,
        environment,
        StoneGeometry {
            planes: &scene.active_planes,
            tools: &scene.tools,
        },
    ) {
        Ok(mirror) => (mirror, None),
        Err(reason) => (None, Some(reason.message())),
    }
}

/// The backend the export's GPU lanes use: `gpu` when the scene routes to the GPU kernels (and,
/// with a stone shadow or tray mirror, the backend can sample it). Otherwise `no_gpu`, whose
/// every attempt declines.
fn gpu_for_scene<'a>(
    gpu: &'a GpuBackend,
    no_gpu: &'a GpuBackend,
    scene: &SceneSnapshot,
    traced: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> &'a GpuBackend {
    if scene_routes_to_gpu_with_tools(
        traced,
        StoneGeometry {
            planes: &scene.active_planes,
            tools: &scene.tools,
        },
        &scene.fluorescence,
        environment.lighting_preset().unwrap_or_default(),
    ) && environment
        .tray_shadow()
        .is_none_or(|_| gpu.supports_tray_shadow())
        && environment
            .tray_mirror()
            .is_none_or(|_| gpu.supports_tray_mirror())
    {
        gpu
    } else {
        no_gpu
    }
}

/// The export context's first note: the refusal notes (the stone shadow's, the tray
/// reflection's), then the path-cache note; each sentence that exists, joined by a space.
fn join_notes(refusals: Option<String>, cache: Option<String>) -> Option<String> {
    match (refusals, cache) {
        (Some(refusals), Some(cache)) => Some(format!("{refusals} {cache}")),
        (refusals, None) => refusals,
        (None, cache) => cache,
    }
}

/// [`with_export_ctx`] for a render that may use the path cache. With `cache_taps` set
/// (`RenderTaps::path_cache`, path-traced engine) the context carries the chosen cache, or the
/// note saying why there is none. A persisting render fills its cache folder on a background
/// thread beside `f`. With `None` this is [`with_export_ctx`].
///
/// `build_shadow` is false for a render no local lane traces (remote only). `shadow_progress`
/// receives the Final build's fraction. A declined or failed shadow renders without it and
/// queues its reason as the context's note.
#[expect(
    clippy::too_many_arguments,
    reason = "[`with_export_ctx`]'s arguments plus the taps that ask for the path cache"
)]
fn with_export_ctx_taps<R>(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    width: u32,
    height: u32,
    gpu: &GpuBackend,
    ray_gpu: Option<&pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend>,
    cancel: &AtomicBool,
    cache_taps: Option<&RenderTaps>,
    build_shadow: bool,
    shadow_progress: &mut dyn FnMut(f32),
    f: impl FnOnce(Result<&ExportCtx<'_>, String>) -> R,
) -> R {
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll)
        .with_pan(scene.camera_pan);

    // A loaded HDR panorama (`SceneSnapshot::env_map`) replaces the analytic studio rig as
    // this render's environment, as in the live render loop. The GPU traces it like any other
    // environment and falls back to the CPU only on the generic per-frame decline.
    // The map turns with this call's camera yaw, as the live view turns it.
    let environment = scene.environment_at(cam_yaw);
    // The silk and play-of-color ride on the traced material, which the GPU kernels read too.
    let traced = scene.traced_material();
    // A shadow that cannot apply (HDR, As Lit, ASET, a UV lamp) or cannot be built renders
    // without it and says why in the note. Only a cancel stops the render. A remote-only render
    // builds no local map: the worker builds its own.
    let (map, shadow_note) = match resolve_stone_shadow(
        scene,
        environment,
        &traced,
        (build_shadow, cancel, shadow_progress),
    ) {
        Ok(resolved) => resolved,
        Err(message) => return f(Err(message)),
    };
    // The tray reflection is resolved from the plain lighting and attached with the shadow map.
    // Its refusal shares the shadow's note slot, the shadow's reason first.
    let (mirror, mirror_note) = resolve_tray_mirror(scene, environment);
    let shadow_note = join_notes(
        shadow_note.map(str::to_owned),
        mirror_note.map(str::to_owned),
    );
    let environment = environment
        .with_tray_shadow(map.as_deref())
        .with_tray_mirror(mirror);
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &scene.active_planes,
        facet_finishes: &scene.facet_finishes,
        material: &traced,
        max_bounces: scene.max_bounces,
        environment,
        estimator: None,
    };
    // Shared by every batch of this call (via `ExportCtx::gpu_retired`), so a GPU-thread panic
    // retires the backend for the rest of the call (see `batch::hybrid_batch`). Per call, not
    // in `AccumulationCarry`, so each frame of a sweep retries the GPU.
    let gpu_retired = AtomicBool::new(false);
    // A scene the GPU cannot trace (fluorescence, a UV lamp, too many tools) is the CPU
    // tracer's alone. A backend with no device makes every GPU attempt decline, and each batch
    // then traces its full share on the CPU.
    let no_gpu = GpuBackend::disabled();
    // The facet surfaces ride to the GPU kernels with every request (`ExportCtx::try_gpu`)
    // and to the CPU tracer in `render_batch`.
    let facet_surfaces = scene.facet_surfaces();
    let surface = scene.surface_scene(&facet_surfaces);
    let gpu = gpu_for_scene(gpu, &no_gpu, scene, &traced, environment);
    let ray_frame = frame_scene_of(
        scene,
        &camera,
        (width, height),
        &scene.material,
        environment,
    );
    let ray_prep = match whitted_prep_of(scene, &ray_frame) {
        Ok(prep) => prep,
        Err(reason) => return f(Err(reason)),
    };
    let ray_soa = ray_prep
        .as_ref()
        .map(|_| build_plane_soa(&scene.active_planes));
    // The frozen frame owns these uploads until all local and remote claims finish. A decline
    // means CPU for this frame; the next frame may use the backend again.
    let ray_uploaded = ray_gpu.zip(ray_prep.as_ref()).and_then(|(gpu, prep)| {
        gpu.prepare_cancellable(&ray_frame, &scene.tools, prep, &|| {
            cancel.load(Ordering::Acquire)
        })
        .ok()
    });
    // The path cache (guide section 8) is built only for a path-traced render that asked for it.
    // It is built from the GPU scene's own fields, so it sees the same stone, material and light
    // as every batch.
    let cache_frame = frame_scene_of(scene, &camera, (width, height), &traced, environment);
    let cache_taps = cache_taps.filter(|taps| taps.path_cache.is_some() && !scene.engine.is_ray());
    // Where beams render the stone they win over the path cache (beam guide section 14.6).
    #[cfg(feature = "beam")]
    let cache_taps = cache_taps_beside_beams(cache_taps, &cache_frame, scene, surface);
    let cache_soa = cache_taps.map(|_| build_plane_soa(&scene.active_planes));
    let (chosen, cache_note, persist) = cache_run::choose(
        cache_taps.zip(cache_soa.as_ref()),
        &cache_frame,
        scene,
        surface,
    );
    let cache_note = join_notes(shadow_note, cache_note);
    let gpu_cache = chosen
        .as_ref()
        .and_then(|chosen| cache_run::gpu_cache(gpu, &chosen.frame, &chosen.placement));
    let path_cache = chosen
        .as_ref()
        .zip(cache_soa.as_ref())
        .map(|(chosen, soa)| CacheRun::new(&chosen.frame, gpu_cache, &cache_frame, soa));
    let ctx = ExportCtx {
        width,
        height,
        camera: &camera,
        scene,
        gpu,
        gpu_scene: &gpu_scene,
        surface,
        gpu_retired: &gpu_retired,
        ray: ray_prep.as_ref().zip(ray_soa.as_ref()),
        ray_gpu: ray_gpu.zip(ray_uploaded.as_ref()),
        path_cache,
        cache_note: Mutex::new(cache_note),
    };
    // A render that keeps its cache fills the folder on a background thread while it renders;
    // the scope joins it before the context goes away.
    thread::scope(|scope| {
        if let (Some(persist), Some(chosen)) = (persist, chosen.as_ref()) {
            let frame = &chosen.frame;
            scope.spawn(move || cache_run::write_cache(frame, persist, cancel));
        }
        f(Ok(&ctx))
    })
}

/// Renders `scene` from the camera pose `(cam_yaw, cam_pitch)` into a linear accumulation buffer.
///
/// It traces `params.samples_per_pixel` samples per pixel in small batches, so progress is
/// reported and cancellation checked between batches. There is no tone-mapping or disk I/O: see
/// [`super::export::run_export_tapped`] for the still-image wrapper that adds both.
///
/// `cam_yaw`/`cam_pitch` are separate from `scene`'s stored pose, so a caller sweeping the
/// camera over many calls against one `scene` (the tilt video, one call per angle) passes each
/// call's own pose. This is the one place where the local `Camera` and the remote `SceneState`
/// get their pose, so local, GPU and remote engines always trace the same frame.
///
/// `gpu` and `carry` are owned by the caller, so a multi-frame caller reuses the same
/// [`GpuBackend`] and [`AccumulationCarry`] instead of re-acquiring the adapter and re-probing
/// the remote on every frame.
///
/// # Local and remote share one claim point
///
/// `compute_target` and `worker` add the remote endpoint next to the CPU/GPU hybrid split.
/// Remote samples are claimed from the same [`SampleCursor`] as the local lanes, for the whole
/// concurrent phase. A shared atomic claim point never leaves an engine with a fixed slice
/// that runs out early. `ComputeTarget::LocalOnly` skips every remote code path, so its output
/// is identical to a purely local render.
///
/// Remote is dispatched as a sequence of chunk requests sized to a target duration (see
/// `remote::remote_chunk_samples`). If a chunk's connection drops partway, the samples it
/// completed are its valid prefix. The unfinished remainder goes back to the shared cursor for
/// the local lanes (`ComputeTarget::Both`), or fails the render (`RemoteOnly`, which has no
/// local lane).
#[expect(
    clippy::too_many_arguments,
    reason = "this is the shared render worker's own top-level entry point -- \
              scene/pose/output params plus the compute-target/worker/carry choice -- \
              and its body is already split across `batch`/`remote`'s own helper \
              functions and this module's `prepare_remote_lane`/`run_lanes`; bundling \
              the arguments into a struct would just move the same count into field \
              access, not reduce it"
)]
pub fn render_accumulation(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    params: ExportParams,
    compute_target: ComputeTarget,
    worker: Option<&WorkerSettings>,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> AccumulationOutcome {
    // `max_bounces` is not used here: `scene.max_bounces` is already the resolved cap, set
    // right after `SceneSnapshot::capture`. `ExportParams::max_bounces` only validates the
    // dialog's choice with the other parameters.
    let ExportParams {
        width,
        height,
        samples_per_pixel,
        ..
    } = params;
    let job = RenderJob {
        scene,
        cam_yaw,
        cam_pitch,
        width,
        height,
        samples_per_pixel,
        sample_offset: carry.taps.sample_offset,
        compute_target,
        worker,
        cancel,
    };

    if scene.engine.is_ray() && cancel.load(Ordering::Acquire) {
        return AccumulationOutcome::Cancelled;
    }
    let ray_gpu = ray_gpu_of(scene, local_compute, compute_target, carry);
    let cache_taps = cache_taps_of(carry, compute_target);
    // The shadow build and the render share one progress sink; the build finishes before the
    // render's closure runs, so the cell is never borrowed twice at once.
    let progress = std::cell::RefCell::new(report_progress);
    // A remote-only render builds no local map: the worker builds its own.
    let build_shadow = compute_target != ComputeTarget::RemoteOnly;
    let shadow_step = |fraction: f32| {
        let mut sink = progress.borrow_mut();
        (*sink)(shadow_progress_event(fraction, samples_per_pixel));
    };
    if build_shadow && scene.stone_shadow_enabled() {
        shadow_step(0.0);
    }
    let rendered = with_export_ctx_taps(
        scene,
        cam_yaw,
        cam_pitch,
        width,
        height,
        gpu,
        ray_gpu.as_deref(),
        cancel,
        cache_taps.as_ref(),
        build_shadow,
        &mut |fraction| shadow_step(fraction),
        |ctx| {
            if (scene.engine.is_ray() || scene.stone_shadow_enabled())
                && cancel.load(Ordering::Acquire)
            {
                return Err(AccumulationOutcome::Cancelled);
            }
            let ctx = ctx.map_err(AccumulationOutcome::Failed)?;
            // A path cache asked for without being kept is refused with one note. Later frames
            // of the sweep skip the decision.
            if cache_run::refused_one_shot(ctx) {
                carry.one_shot_cache_refused = true;
            }
            // Keep the shadow visible when the requested GPU cannot sample it.
            if local_compute != LocalComputeTarget::Cpu
                && compute_target != ComputeTarget::RemoteOnly
                && !scene.engine.is_ray()
            {
                queue_gpu_decline(
                    ctx,
                    pleochroma::renderer::gpu_backend::scene_decline_with_tray_shadow(
                        ctx.gpu_scene.environment,
                        ctx.gpu.supports_tray_shadow(),
                    ),
                );
                queue_gpu_decline(
                    ctx,
                    pleochroma::renderer::gpu_backend::scene_decline_with_tray_mirror(
                        ctx.gpu_scene.environment,
                        ctx.gpu.supports_tray_mirror(),
                    ),
                );
            }
            // Beam tracing renders eligible stones on this computer and on a remote worker (whole
            // tiles). A stone it cannot render queues its note and falls through to the render.
            #[cfg(feature = "beam")]
            if let Some(early) = render_beams_first(ctx, &job, local_compute, carry, &progress) {
                return early;
            }
            // Without the `beam` feature, a render that asks for beams renders without them,
            // with the note saying why.
            #[cfg(not(feature = "beam"))]
            queue_beams_unavailable(ctx, &carry.taps);
            // Adaptive sampling is its own loop and never reaches the remote probing or the
            // shared claim cursor. Without the `adaptive-sampling` feature, a render that asks
            // for it renders at its own samples per pixel, with the note saying why.
            if let Some(early) =
                render_adaptive_first(ctx, &job, local_compute, &carry.taps, &progress)
            {
                return early;
            }
            render_on_ctx(ctx, &job, local_compute, carry, &mut *progress.borrow_mut())
        },
    );
    match rendered {
        Ok(done) => AccumulationOutcome::Completed(done),
        Err(early) => early,
    }
}

/// Traces absolute samples `[first, first + samples)` of `scene` with this computer's local
/// lanes only (CPU, or CPU+GPU under `LocalComputeTarget::CpuGpu`), and no remote lane. It is
/// the viewer's share of a final-picture export that reserved this tail. It runs beside the
/// server's `FinalImageRequest` (see `worker::final_picture::final_picture`).
///
/// Returns `None` if `stop` was raised before the range finished. Otherwise `Some` holds
/// exactly `samples` samples deep: a `width * height` radiance sum, not yet divided by
/// `samples`, ready to upload as one `CONTRIBUTION`.
///
/// `done` is updated after every batch (relative to `first`), so a polling caller can report
/// combined progress. `hybrid_frac` is read as a carried split estimate and written back.
///
/// Sample identity matches the server: `run_local_batches` and the worker's tracer derive each
/// sample's stratification from the absolute pixel and sample index. A tail traced on either
/// side gives the same bits.
#[expect(
    clippy::too_many_arguments,
    reason = "one render's own identity (scene, pose, output params, the absolute \
              sample range, GPU backend, local compute choice, the split-estimate \
              carry, cancellation and a progress counter) -- the same shape \
              `render_accumulation` itself takes, split differently because this is a \
              single-engine-choice, single-range, local-only render"
)]
pub(in crate::bridge::export_thread) fn render_local_share(
    scene: &SceneSnapshot,
    pose: (f32, f32),
    params: ExportParams,
    first: u32,
    samples: u32,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    hybrid_frac: &mut Option<f64>,
    stop: &AtomicBool,
    done: &AtomicU32,
) -> Option<Vec<Vec3>> {
    let (cam_yaw, cam_pitch) = pose;
    let ExportParams { width, height, .. } = params;

    if scene.engine.is_ray() && stop.load(Ordering::Acquire) {
        return None;
    }
    let ray_gpu = (scene.engine.is_ray() && local_compute != LocalComputeTarget::Cpu)
        .then(pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend::acquire);
    with_export_ctx(
        scene,
        cam_yaw,
        cam_pitch,
        width,
        height,
        gpu,
        ray_gpu.as_ref(),
        stop,
        |ctx| {
            if scene.engine.is_ray() && stop.load(Ordering::Acquire) {
                return None;
            }
            let ctx = ctx.ok()?;
            let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
            let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
            // Absolute cursor for the calibration probe, starting at the tail's first sample.
            let mut next = first;
            let mut gpu_frac = if !scene.engine.is_ray()
                && local_compute == LocalComputeTarget::CpuGpu
                && samples >= HYBRID_MIN_SPP
            {
                hybrid_frac.or_else(|| {
                    calibrate_split_confirmed(
                        ctx,
                        &mut next,
                        samples,
                        &mut accum,
                        &mut gpu_accum,
                        stop,
                    )
                })
            } else {
                None
            };

            let cursor = SampleCursor::new(next, first + samples);
            // No remote lane to wait on: a `true` "remote lane done" flag makes the stop
            // condition "stop once the cursor is empty".
            let no_remote = AtomicBool::new(true);
            let stopped = run_local_batches(
                ctx,
                &cursor,
                |_| local_chunk_size(samples),
                &no_remote,
                &mut gpu_frac,
                &mut accum,
                &mut gpu_accum,
                stop,
                |traced, _accum, _gpu_accum| {
                    done.store((next - first) + traced, Ordering::Relaxed);
                },
            );
            *hybrid_frac = gpu_frac;
            if stopped || stop.load(Ordering::Relaxed) {
                return None;
            }

            // Merge the GPU's accumulation once, as `render_accumulation` does at its end. A
            // pure-CPU share leaves `gpu_accum` zero, so this is a no-op.
            for (px, gpu_px) in accum.iter_mut().zip(&gpu_accum) {
                *px += *gpu_px;
            }
            Some(accum)
        },
    )
}

#[cfg(test)]
mod prepared_ray_tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use pleochroma::renderer::{engine::RenderEngine, gpu_backend::whitted::WhittedGpuBackend};
    use std::sync::Mutex;

    #[test]
    fn a_declined_frame_upload_preserves_cpu_claims_and_cancelled_output() {
        let mut scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        scene.engine.engine = RenderEngine::RayTraced;
        scene.engine.ray.aa_taps = 4;
        let gpu = GpuBackend::disabled();
        let ray_gpu = WhittedGpuBackend::disabled();
        let cancel = AtomicBool::new(false);
        let mut cpu = vec![Vec3::ZERO; 64];
        with_export_ctx(
            &scene,
            scene.yaw,
            scene.pitch,
            8,
            8,
            &gpu,
            None,
            &cancel,
            |ctx| {
                assert!(!super::super::export_ray::accumulate(
                    ctx.unwrap(),
                    0,
                    4,
                    &mut cpu,
                    &cancel
                ));
            },
        );
        with_export_ctx(
            &scene,
            scene.yaw,
            scene.pitch,
            8,
            8,
            &gpu,
            Some(&ray_gpu),
            &cancel,
            |ctx| {
                let ctx = ctx.unwrap();
                assert!(
                    ctx.ray_gpu.is_none(),
                    "a disabled adapter declines the frame upload"
                );
                let mut actual = vec![Vec3::ZERO; 64];
                assert!(!super::super::export_ray::accumulate(
                    ctx,
                    0,
                    4,
                    &mut actual,
                    &cancel
                ));
                assert_eq!(
                    actual, cpu,
                    "upload decline retries the complete absolute tap claim"
                );
                cancel.store(true, Ordering::Release);
                let before = actual.clone();
                assert!(super::super::export_ray::accumulate(
                    ctx,
                    0,
                    1,
                    &mut actual,
                    &cancel
                ));
                assert_eq!(
                    actual, before,
                    "cancellation commits no CPU fallback output"
                );
            },
        );
    }
}

#[cfg(all(test, not(feature = "beam")))]
mod beam_off_tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use pleochroma::renderer::accel_beam::BeamSettings;

    /// The note a build without the `beam` feature queues for a render that asks for beams.
    const NOTE: &str = "Beam tracing off: this build has no beam tracer.";

    /// The note on `ctx` after `queue_beams_unavailable(ctx, taps)` ran on a context that held
    /// `before`.
    fn note_after(ctx: &ExportCtx<'_>, taps: &RenderTaps, before: Option<&str>) -> Option<String> {
        *ctx.cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = before.map(str::to_owned);
        queue_beams_unavailable(ctx, taps);
        ctx.cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Guards the beam gating in a build without the `beam` feature: the decision and the note.
    ///
    /// In a build without the `beam` feature a render whose taps ask for beams (a job file's
    /// `accel.beam`) queues the note that says why it renders without them, ahead of any note
    /// already there; a render that does not ask queues nothing and leaves the note as it was.
    #[test]
    fn a_render_that_asks_for_beams_queues_the_beam_tracing_off_note() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        let gpu = GpuBackend::disabled();
        let cancel = AtomicBool::new(false);
        let asks = RenderTaps {
            beam: Some(BeamSettings::default()),
            ..RenderTaps::default()
        };
        let silent = RenderTaps::default();
        with_export_ctx(
            &scene,
            scene.yaw,
            scene.pitch,
            8,
            8,
            &gpu,
            None,
            &cancel,
            |ctx| {
                let ctx = ctx.unwrap();
                assert_eq!(note_after(ctx, &asks, None).as_deref(), Some(NOTE));
                assert_eq!(
                    note_after(ctx, &asks, Some("The path cache is off.")).as_deref(),
                    Some("Beam tracing off: this build has no beam tracer. The path cache is off.")
                );
                assert_eq!(note_after(ctx, &silent, None), None);
                assert_eq!(
                    note_after(ctx, &silent, Some("The path cache is off.")).as_deref(),
                    Some("The path cache is off.")
                );
            },
        );
    }

    /// Guards the beam gating on the render path, the call in `render_accumulation`.
    ///
    /// In a build without the `beam` feature a render whose taps ask for beams completes as the
    /// plain render, and its progress reports carry the note exactly once (CPU only, 8 x 8 pixels,
    /// no stone shadow, so no other note).
    #[test]
    fn a_render_that_asks_for_beams_renders_without_them_and_reports_the_note() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        let params = ExportParams {
            engine: scene.engine,
            width: 8,
            height: 8,
            samples_per_pixel: 4,
            max_bounces: scene.max_bounces,
        };
        let mut carry = AccumulationCarry::with_taps(RenderTaps {
            beam: Some(BeamSettings::default()),
            ..RenderTaps::default()
        });
        let mut notes: Vec<String> = Vec::new();
        let outcome = render_accumulation(
            &scene,
            scene.yaw,
            scene.pitch,
            params,
            ComputeTarget::LocalOnly,
            None,
            &GpuBackend::disabled(),
            LocalComputeTarget::Cpu,
            &mut carry,
            &AtomicBool::new(false),
            |progress| notes.extend(progress.note),
        );
        let done = match outcome {
            AccumulationOutcome::Completed(done) => done,
            AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
            AccumulationOutcome::Failed(message) => panic!("the render failed: {message}"),
        };
        assert_eq!(done.samples_per_pixel, 4);
        assert_eq!(notes, [NOTE], "the note is reported once");
    }
}

#[cfg(all(test, not(feature = "adaptive-sampling")))]
mod adaptive_off_tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use pleochroma::renderer::{accel_beam::BeamSettings, adaptive::AdaptiveSettings};

    /// The note a build without the `adaptive-sampling` feature queues for a render that asks for
    /// adaptive sampling.
    const NOTE: &str = "Adaptive sampling off: this build has no adaptive sampling.";

    /// The taps of a job file that asks for adaptive sampling.
    fn asks_for_adaptive() -> RenderTaps {
        RenderTaps {
            adaptive: Some(AdaptiveSettings::provisional(8)),
            ..RenderTaps::default()
        }
    }

    /// The note on `ctx` after `queue_adaptive_unavailable(ctx, taps)` ran on a context that held
    /// `before`.
    fn note_after(ctx: &ExportCtx<'_>, taps: &RenderTaps, before: Option<&str>) -> Option<String> {
        *ctx.cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = before.map(str::to_owned);
        queue_adaptive_unavailable(ctx, taps);
        ctx.cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Guards the adaptive sampling gating in a build without the feature: the decision and note.
    ///
    /// In a build without the `adaptive-sampling` feature a render whose taps ask for adaptive
    /// sampling (a job file's `accel.adaptive`) queues the note saying why it renders at its own
    /// samples per pixel, ahead of any note already there. A render that does not ask queues
    /// nothing.
    #[test]
    fn a_render_that_asks_for_adaptive_sampling_queues_the_adaptive_sampling_off_note() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        let gpu = GpuBackend::disabled();
        let cancel = AtomicBool::new(false);
        let asks = asks_for_adaptive();
        let silent = RenderTaps::default();
        with_export_ctx(
            &scene,
            scene.yaw,
            scene.pitch,
            8,
            8,
            &gpu,
            None,
            &cancel,
            |ctx| {
                let ctx = ctx.unwrap();
                assert_eq!(note_after(ctx, &asks, None).as_deref(), Some(NOTE));
                assert_eq!(
                    note_after(ctx, &asks, Some("The path cache is off.")).as_deref(),
                    Some(
                        "Adaptive sampling off: this build has no adaptive sampling. \
                         The path cache is off."
                    )
                );
                assert_eq!(note_after(ctx, &silent, None), None);
                assert_eq!(
                    note_after(ctx, &silent, Some("The path cache is off.")).as_deref(),
                    Some("The path cache is off.")
                );
            },
        );
    }

    /// Guards the adaptive sampling gating on the render path, the call in `render_accumulation`.
    ///
    /// In a build without the `adaptive-sampling` feature a render whose taps ask for adaptive
    /// sampling completes as the plain render at its own samples per pixel (no adaptive
    /// figures). Its progress reports carry the note exactly once (CPU only, 8 x 8 pixels, no
    /// stone shadow, so no other note).
    #[test]
    fn a_render_that_asks_for_adaptive_sampling_renders_plain_and_reports_the_note() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        let params = ExportParams {
            engine: scene.engine,
            width: 8,
            height: 8,
            samples_per_pixel: 4,
            max_bounces: scene.max_bounces,
        };
        let mut carry = AccumulationCarry::with_taps(asks_for_adaptive());
        let mut notes: Vec<String> = Vec::new();
        let outcome = render_accumulation(
            &scene,
            scene.yaw,
            scene.pitch,
            params,
            ComputeTarget::LocalOnly,
            None,
            &GpuBackend::disabled(),
            LocalComputeTarget::Cpu,
            &mut carry,
            &AtomicBool::new(false),
            |progress| notes.extend(progress.note),
        );
        let done = match outcome {
            AccumulationOutcome::Completed(done) => done,
            AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
            AccumulationOutcome::Failed(message) => panic!("the render failed: {message}"),
        };
        assert_eq!(done.samples_per_pixel, 4);
        assert_eq!(notes, [NOTE], "the note is reported once");
    }

    /// A job file that asks for beams and for adaptive sampling, in a build with neither feature,
    /// gets both notes once each. The adaptive note is queued after the beam note, so it leads.
    #[test]
    fn a_render_that_asks_for_beams_and_adaptive_sampling_reports_both_notes_once() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap();
        let params = ExportParams {
            engine: scene.engine,
            width: 8,
            height: 8,
            samples_per_pixel: 4,
            max_bounces: scene.max_bounces,
        };
        let mut carry = AccumulationCarry::with_taps(RenderTaps {
            beam: Some(BeamSettings::default()),
            ..asks_for_adaptive()
        });
        let mut notes: Vec<String> = Vec::new();
        let outcome = render_accumulation(
            &scene,
            scene.yaw,
            scene.pitch,
            params,
            ComputeTarget::LocalOnly,
            None,
            &GpuBackend::disabled(),
            LocalComputeTarget::Cpu,
            &mut carry,
            &AtomicBool::new(false),
            |progress| notes.extend(progress.note),
        );
        assert!(matches!(outcome, AccumulationOutcome::Completed(_)));
        assert_eq!(
            notes,
            [format!(
                "{NOTE} Beam tracing off: this build has no beam tracer."
            )],
            "one report with both notes"
        );
    }
}
