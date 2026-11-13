//! Image-engine dispatch between the Path-Traced (spectral) and Ray-Traced (Whitted) frames.
use super::gpu_backend::{BackendFrame, FrameOutputs, HybridPacing, ViewportGpu};
use crate::settings::model::LocalComputeTarget;
use pleochroma::{
    optics::{fluorescence::Fluorescence, raytracer::LightingPreset},
    renderer::{
        engine::EngineSpec,
        frame_scene::FrameScene,
        gpu_backend::{
            GpuAccumulate,
            whitted::{WhittedGpuBackend, WhittedGpuPrepared},
        },
        whitted::{WhittedDecline, WhittedPrep},
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[path = "ray_gpu_warmup.rs"]
mod ray_gpu_warmup;
#[path = "ray_pacing.rs"]
mod ray_pacing;
use ray_pacing::RayPacing;

#[cfg(feature = "render-bench")]
#[path = "benchmark_observation.rs"]
pub(super) mod observation;
#[cfg(feature = "render-bench")]
use observation::{BenchmarkObservation, PhaseTimer};

/// Cached by content generation, effective dimensions and the active shadow map.
#[derive(Default)]
pub(super) struct EngineAccumulator {
    prep: Option<PreparedRay>,
    gpu: ray_gpu_warmup::RayGpu,
    pacing: RayPacing,
    #[cfg(feature = "render-bench")]
    observation: BenchmarkObservation,
}

#[cfg(feature = "render-bench")]
impl EngineAccumulator {
    /// Joins preparation only at benchmark sequence boundaries, outside case timing.
    pub(super) fn drain_background_preparation(&mut self) {
        self.gpu.drain_background_preparation();
    }

    pub(super) const fn benchmark_observation(&self) -> &BenchmarkObservation {
        &self.observation
    }
}

type RayPrepKey = (
    u64,
    u32,
    u32,
    Option<pleochroma::optics::raytracer::environment::tray_shadow::TrayShadowKey>,
);

fn ray_prep_key(generation: u64, frame: &BackendFrame<'_>) -> RayPrepKey {
    ray_prep_key_for(
        generation,
        frame.width,
        frame.height,
        frame
            .environment
            .tray_shadow()
            .map(pleochroma::optics::raytracer::environment::TrayShadowMap::key),
    )
}

const fn ray_prep_key_for(
    generation: u64,
    width: u32,
    height: u32,
    shadow: Option<pleochroma::optics::raytracer::environment::tray_shadow::TrayShadowKey>,
) -> RayPrepKey {
    (generation, width, height, shadow)
}

struct PreparedRay {
    key: RayPrepKey,
    prep: Arc<WhittedPrep<'static>>,
    planes: pleochroma::simd::PlanesSoA32,
    gpu: Option<WhittedGpuPrepared>,
    /// The upload was refused for this key; the CPU route serves it.
    gpu_declined: bool,
}

/// Remote workers and coordinators carry both image engines explicitly, so this is always `true`.
///
/// It is the single switch for allowing remote Ray-Traced. Its call sites consult it so that
/// changing it is a one-line edit.
#[must_use]
pub const fn remote_allowed(_engine: EngineSpec) -> bool {
    true
}

/// Refusals feed the existing banner before the frame claims a sample range.
pub(super) fn decline_message(
    engine: EngineSpec,
    preset: Option<LightingPreset>,
    fluorescence: Option<&Fluorescence>,
) -> Option<&'static str> {
    if !engine.is_ray() {
        None
    } else if fluorescence.is_some_and(|value| !value.is_empty()) {
        Some(WhittedDecline::Fluorescence.message())
    } else if preset.is_some_and(pleochroma::optics::LightingPreset::is_uv_lamp) {
        Some(WhittedDecline::UvLamp.message())
    } else {
        None
    }
}

pub(super) const fn frame_scene<'a>(frame: &BackendFrame<'a>) -> FrameScene<'a> {
    FrameScene {
        camera: frame.camera,
        width: frame.width,
        height: frame.height,
        planes: frame.planes,
        facet_finishes: frame.facet_finishes,
        material: frame.material,
        max_bounces: frame.max_bounces,
        environment: frame.environment,
    }
}

/// Renders the pixels of `frame` that `covered` leaves out with the Ray-Traced CPU tracer.
/// The `frame.spp` taps from `frame.sample_offset` are summed into `accum`. A re-shade of the
/// stone (`reshade_view`) calls this for the background and for any pixel the stored light
/// paths do not cover.
///
/// Prepares the engine's scene for its generation, frame size and effective shadow exactly as
/// [`accumulate_engine_samples`] does, so the next ordinary Ray-Traced frame of the same scene
/// reuses it. `Ok(false)` means the frame's cancellation fired.
///
/// # Errors
///
/// The reason the Ray-Traced engine declines the scene.
pub(super) fn accumulate_uncovered(
    cache: &mut EngineAccumulator,
    engine: EngineSpec,
    (generation, shared_hdr): (
        u64,
        Option<&Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    ),
    frame: &BackendFrame<'_>,
    (covered, accum): (&[bool], &mut [glam::Vec3]),
    cancel: &AtomicBool,
) -> Result<bool, WhittedDecline> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let key = ray_prep_key(generation, frame);
    if cache.prep.as_ref().is_none_or(|cached| cached.key != key) {
        let prep = WhittedPrep::new(&frame_scene(frame), frame.fluorescence, engine.ray)?
            .into_owned_with_hdr(shared_hdr.cloned());
        cache.prep = Some(PreparedRay {
            key,
            prep: Arc::new(prep),
            planes: pleochroma::optics::raytracer::build_plane_soa(frame.planes),
            gpu: None,
            gpu_declined: false,
        });
    }
    let Some(prepared) = cache.prep.as_ref() else {
        return Ok(false);
    };
    Ok(super::whitted_scanline::render_uncovered(
        frame,
        &prepared.prep,
        &prepared.planes,
        covered,
        accum,
        cancel,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "adapts the frame dispatcher and carries the current frame cancellation token"
)]
pub(super) fn accumulate_engine_samples(
    cache: &mut EngineAccumulator,
    engine: EngineSpec,
    generation: u64,
    shared_hdr: Option<&std::sync::Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    compute: LocalComputeTarget,
    cancel: &AtomicBool,
) -> Result<bool, WhittedDecline> {
    #[cfg(feature = "render-bench")]
    {
        cache.observation = BenchmarkObservation::default();
    }
    if cancel.load(Ordering::Relaxed) {
        #[cfg(feature = "render-bench")]
        {
            cache.observation.cancelled = true;
        }
        return Ok(false);
    }
    if !engine.is_ray() {
        cache.prep = None;
        // Back from Ray-Traced: its pill text does not describe the Path-Traced route.
        backend.clear_ray_status();
        backend.set_shared_hdr(shared_hdr.cloned());
        let completed = super::gpu_backend::accumulate_frame_samples_cancellable(
            backend, frame, outputs, hybrid, compute, cancel,
        );
        #[cfg(feature = "render-bench")]
        {
            cache.observation = backend.benchmark_observation().clone();
            cache.observation.cancelled = !completed;
        }
        return Ok(completed);
    }
    // Ray leaves guide scratch unused; force the next spectral pass to refresh it.
    backend.invalidate_guide_cache();
    // A camera-only change moves `generation`, which re-prepares the scene and the GPU
    // upload. The HDR environment buffer is not part of that cost: the prep owns its map
    // as `shared_hdr`, and the GPU renderer keeps the uploaded buffer for that same `Arc`
    // (`WhittedGpuRenderer::prepare`), so an orbit step re-uploads only the small tables.
    let key = ray_prep_key(generation, frame);
    if cache.prep.as_ref().is_none_or(|cached| cached.key != key) {
        #[cfg(feature = "render-bench")]
        let timer = PhaseTimer::start("whitted_prep", false);
        let prep = WhittedPrep::new(&frame_scene(frame), frame.fluorescence, engine.ray)
            .map(|prep| prep.into_owned_with_hdr(shared_hdr.cloned()));
        #[cfg(feature = "render-bench")]
        {
            timer.finish(&mut cache.observation);
            if let Err(reason) = &prep {
                cache.observation.reason = Some(reason.message().to_owned());
            }
        }
        let prep = prep?;
        cache.prep = Some(PreparedRay {
            key,
            prep: Arc::new(prep),
            planes: pleochroma::optics::raytracer::build_plane_soa(frame.planes),
            gpu: None,
            gpu_declined: false,
        });
    }
    let completed =
        accumulate_ray_frame(cache, backend, frame, outputs, compute, shared_hdr, cancel);
    #[cfg(feature = "render-bench")]
    {
        cache.observation.cancelled = !completed;
    }
    Ok(completed)
}

fn accumulate_ray_frame(
    cache: &mut EngineAccumulator,
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    compute: LocalComputeTarget,
    shared_hdr: Option<&Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    cancel: &AtomicBool,
) -> bool {
    if let Some(prepared) = cache.prep.as_mut() {
        cache.pacing.prepare(frame, &prepared.prep);
        let mut ready = compute == LocalComputeTarget::Cpu;
        if compute != LocalComputeTarget::Cpu {
            ready = cache.gpu.poll(prepared, frame, shared_hdr);
            while !ready && compute == LocalComputeTarget::Gpu {
                if cancel.load(Ordering::Acquire) {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
                ready = cache.gpu.poll(prepared, frame, shared_hdr);
            }
        }
        let route = if compute == LocalComputeTarget::Cpu
            || !ready
            || (compute == LocalComputeTarget::CpuGpu && cache.pacing.cpu_wanted())
        {
            LocalComputeTarget::Cpu
        } else {
            LocalComputeTarget::Gpu
        };
        backend.set_ray_status(if compute == LocalComputeTarget::Cpu {
            // Nothing prepares or fails on a GPU this frame never asked for.
            None
        } else if !ready {
            Some(super::gpu_backend::PREPARING_STATUS)
        } else if cache.gpu.retired() {
            Some("CPU fallback (GPU preparation panicked)")
        } else if cache.gpu.backend().is_some_and(WhittedGpuBackend::is_lost) {
            Some("CPU fallback (GPU lost)")
        } else {
            None
        });
        accumulate_prepared_ray(
            prepared,
            frame,
            outputs,
            RayPass {
                gpu: cache.gpu.backend(),
                compute: route,
                pacing: &mut cache.pacing,
                cancel,
                #[cfg(feature = "render-bench")]
                observation: &mut cache.observation,
            },
        )
    } else {
        false
    }
}

struct RayPass<'a> {
    gpu: Option<&'a pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend>,
    compute: LocalComputeTarget,
    pacing: &'a mut RayPacing,
    cancel: &'a AtomicBool,
    #[cfg(feature = "render-bench")]
    observation: &'a mut BenchmarkObservation,
}

fn accumulate_prepared_ray(
    prepared: &mut PreparedRay,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    pass: RayPass<'_>,
) -> bool {
    let RayPass {
        gpu: cached_gpu,
        compute,
        pacing,
        cancel,
        #[cfg(feature = "render-bench")]
        observation,
    } = pass;
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    #[cfg(feature = "render-bench")]
    {
        observation.adapter_label = cached_gpu.and_then(WhittedGpuBackend::adapter_label);
    }
    if matches!(
        compute,
        LocalComputeTarget::Gpu | LocalComputeTarget::CpuGpu
    ) && let Some(gpu) = cached_gpu
    {
        if let Some(upload) = prepared.gpu.as_ref() {
            #[cfg(feature = "render-bench")]
            let timer = PhaseTimer::start("whitted_gpu_radiance_inclusive", false);
            #[cfg(feature = "render-bench")]
            {
                observation.route = "gpu";
            }
            let started = std::time::Instant::now();
            let result = gpu.try_accumulate_prepared_cancellable(
                upload,
                frame.sample_offset,
                frame.spp,
                outputs.accum,
                &|| cancel.load(Ordering::Relaxed),
            );
            #[cfg(feature = "render-bench")]
            timer.finish(observation);
            match result {
                GpuAccumulate::Cancelled => return false,
                GpuAccumulate::Done => {
                    pacing.observe_gpu(frame.spp, started.elapsed());
                    // Ray output has no display guide consumer. Picking intersects
                    // geometry directly; benchmark hit counts use diagnostic_hit_pixels.
                    return !cancel.load(Ordering::Relaxed);
                }
                GpuAccumulate::Declined => {
                    prepared.gpu = None;
                    // A lost device is retried through `RayGpu::poll`'s deadline; any
                    // other decline is final for this key, or the next frame would
                    // upload the same scene again and flicker the pill.
                    prepared.gpu_declined = !gpu.is_lost();
                }
            }
        }
        #[cfg(feature = "render-bench")]
        observation.fallback(if gpu.is_lost() {
            "Whitted GPU device lost"
        } else if gpu.adapter_label().is_none() {
            "Whitted GPU unavailable or disabled"
        } else {
            "Whitted GPU declined preparation or radiance; detailed reason unavailable"
        });
    }
    #[cfg(feature = "render-bench")]
    let timer = PhaseTimer::start("whitted_cpu_radiance", false);
    #[cfg(feature = "render-bench")]
    {
        observation.route = "cpu";
    }
    let started = std::time::Instant::now();
    let completed = super::whitted_scanline::render_frame(
        frame,
        &prepared.prep,
        &prepared.planes,
        outputs,
        cancel,
    );
    if completed {
        pacing.observe_cpu(frame.spp, started.elapsed());
    }
    #[cfg(feature = "render-bench")]
    timer.finish(observation);
    completed
}

/// Explicit diagnostic pass, deliberately separate from timed production radiance.
#[cfg(feature = "render-bench")]
pub(super) fn diagnostic_hit_pixels(frame: &BackendFrame<'_>, cancel: &AtomicBool) -> usize {
    use pleochroma::renderer::whitted::whitted_primary_hit;
    let scene = frame_scene(frame);
    let planes = pleochroma::optics::raytracer::build_plane_soa(frame.planes);
    (0..frame.height)
        .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter(|&(x, y)| {
            let ray = frame.camera.generate_ray(
                x as f32,
                y as f32,
                frame.width as f32,
                frame.height as f32,
                0.0,
                0.0,
            );
            whitted_primary_hit(&scene, frame.tools, &planes, ray).is_some()
        })
        .count()
}

#[cfg(test)]
mod shadow_key_tests {
    use super::ray_prep_key_for;
    use pleochroma::optics::raytracer::environment::tray_shadow::TrayShadowKey;

    /// A stone shadow map arriving, or being replaced by another, changes the ray-preparation key,
    /// so the Ray-Traced engine rebuilds its prepared scene; the same map keeps the key.
    #[test]
    fn shadow_arrival_and_replacement_invalidate_ray_preparation() {
        let before = ray_prep_key_for(7, 24, 16, None);
        let arrival = ray_prep_key_for(7, 24, 16, Some(TrayShadowKey(1)));
        let replacement = ray_prep_key_for(7, 24, 16, Some(TrayShadowKey(2)));
        assert_ne!(before, arrival);
        assert_ne!(arrival, replacement);
        assert_eq!(arrival, ray_prep_key_for(7, 24, 16, Some(TrayShadowKey(1))));
    }
}
