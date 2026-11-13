//! The steady-state hybrid sub-batch.
//!
//! [`hybrid_trace_into`] runs the GPU and CPU tracers
//! concurrently over disjoint sample sub-ranges and sums them, re-measuring the split every
//! call. Calibration and its cache stay in `hybrid.rs`.

use super::{cpu_is_worth_splitting_with, cpu_threads_beside_gpu, scene_pieces};
use crate::{
    assets::TrayRef,
    render_core::{scene_uses_gpu, trace_into},
};
use glam::Vec3;
use pleochroma::{
    optics::raytracer::{Camera, EnvironmentSource, FacetFinish},
    renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
};
use pleochroma_dispatch::SampleRange;
use pleochroma_net::SceneState;
use std::{
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

/// One hybrid sub-batch: the GPU traces the lower `[first_sample, first_sample +
/// gpu_share)` sub-range on a scoped thread while every CPU core traces the upper
/// sub-range concurrently (via [`trace_into`]), summed into one buffer -- the same
/// return contract as [`crate::render_core::trace_samples_with_gpu`], so `run_tracer`
/// doesn't need to know which mode produced it.
///
/// `gpu_share` is `(samples as f64 * gpu_frac.unwrap_or(0.0)).round()`, clamped to
/// `samples`. `*gpu_frac` is updated afterward via the measured-throughput blend, unless
/// the GPU declines mid-job (e.g. device loss), in which case its share is retraced on
/// the CPU and `*gpu_frac` is cleared to `None` so later calls stop offering it work. A
/// batch exercising only one engine leaves `*gpu_frac` untouched: no second engine's
/// timing to compare against.
///
/// Returns `false` if the sub-batch was cancelled. The whole sub-batch is then discarded,
/// including the CPU's disjoint share. `samples_done` advances only by whole sub-batches
/// that cover every pixel, so a partial share would leave some pixels under-sampled.
/// The CPU share of the concurrent path is the exception: its `trace_into` call does not
/// observe cancellation separately, since the GPU thread beside it is under time pressure
/// and `run_tracer`'s between-sub-batch check bounds the CPU side's staleness.
///
/// # One core reserved for the GPU submitter
///
/// When `cpu_share > 0` both engines run concurrently, and the CPU tracer gets `threads`
/// minus one (see [`cpu_threads_beside_gpu`]). The submitter thread must poll the GPU. If
/// every core is tracing, it gets descheduled, and the EMA would read the wait as a slow
/// GPU and push more work onto the CPU. That loop would starve the GPU on many-core
/// machines. A GPU-only batch or a GPU-declined retrace has no concurrent dispatch to
/// protect, so it uses every core.
///
/// # Reused buffers
///
/// The summed radiance lands in `bufs.sum` and the GPU's share is staged in `bufs.gpu`.
/// Both are re-zeroed here, since the backends add into their buffer, but keep their
/// capacity. This avoids allocating two full-frame buffers (about 25 MB each at 1080p) on
/// every sub-batch.
/// Returns `false` for a discarded (cancelled) sub-batch; `bufs.sum` is then unspecified.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection. The caller holds it
/// for every sub-batch and it is never rebuilt here.
#[expect(
    clippy::too_many_arguments,
    reason = "the sub-batch's range, split state, cancellation, buffers and the request's resolved tray are independent"
)]
pub fn hybrid_trace_into(
    gpu: &GpuBackend,
    scene: &SceneState,
    (first_sample, samples): (u32, u32),
    threads: usize,
    gpu_frac: &mut Option<f64>,
    cancel: &AtomicBool,
    bufs: &mut HybridBuffers,
    tray: TrayRef<'_>,
) -> bool {
    let width = scene.width;
    let height = scene.height;
    let pixel_count = width as usize * height as usize;
    let HybridBuffers {
        sum: buffer,
        gpu: gpu_buf,
    } = bufs;
    buffer.clear();
    buffer.resize(pixel_count, Vec3::ZERO);
    if samples == 0 || width == 0 || height == 0 {
        return true;
    }

    // A cached `Split` decision is keyed by hardware, not geometry, so it can arrive here
    // for a scene the GPU cannot trace (see `scene_uses_gpu`): drop it and go CPU-only.
    if !scene_uses_gpu(scene, gpu) {
        *gpu_frac = None;
    }
    let frac = gpu_frac.unwrap_or(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the product is a non-negative sample count of at most `samples` for a fraction in 0..=1; the saturating cast and the min bound it anyway"
    )]
    let gpu_share = ((f64::from(samples) * frac).round() as u32).min(samples);
    let cpu_share = samples - gpu_share;

    let (camera, facet_finishes) = scene_pieces(scene);
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray);

    if gpu_share == 0 {
        return trace_into(
            scene,
            SampleRange::new(first_sample, samples),
            threads,
            &camera,
            environment,
            buffer.as_mut_slice(),
            cancel,
        );
    }

    let batch = SplitBatch {
        gpu,
        scene,
        camera: &camera,
        facet_finishes: &facet_finishes,
        environment,
        first_sample,
        threads,
        gpu_share,
        cpu_share,
        frac,
        cancel,
    };
    batch.run(buffer, gpu_buf, gpu_frac)
}

/// The two full-frame buffers one hybrid sub-batch needs, kept across sub-batches by
/// [`hybrid_trace_into`]'s caller so they are allocated once per job, not once per ~100 ms.
#[derive(Default)]
pub struct HybridBuffers {
    /// The sub-batch's summed radiance (CPU share plus GPU share) after a successful call.
    pub sum: Vec<Vec3>,
    /// Staging for the GPU's share before it is added into `sum`.
    gpu: Vec<Vec3>,
}

/// [`hybrid_trace_into`] with freshly allocated buffers: returns the summed radiance, or
/// `None` for a discarded (cancelled) sub-batch. The streaming tracer uses the `_into`
/// form with one reused [`HybridBuffers`]; this is the convenient one-shot form.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the one-shot form; the streaming tracer uses `hybrid_trace_into`, the tests this"
    )
)]
///
/// Traces a scene without the Stone Shadow or the Tray Reflection. It passes neither.
pub fn hybrid_trace(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    gpu_frac: &mut Option<f64>,
    cancel: &AtomicBool,
) -> Option<Vec<Vec3>> {
    let mut bufs = HybridBuffers::default();
    hybrid_trace_into(
        gpu,
        scene,
        (first_sample, samples),
        threads,
        gpu_frac,
        cancel,
        &mut bufs,
        TrayRef::default(),
    )
    .then_some(bufs.sum)
}

/// The fixed inputs of one sub-batch that really splits across both engines
/// (`gpu_share > 0`), bundled so [`Self::run`] stays within the argument-count limit.
struct SplitBatch<'a> {
    gpu: &'a GpuBackend,
    scene: &'a SceneState,
    camera: &'a Camera,
    facet_finishes: &'a [FacetFinish],
    environment: EnvironmentSource<'a>,
    first_sample: u32,
    threads: usize,
    gpu_share: u32,
    cpu_share: u32,
    /// The `gpu_frac` this batch's shares were cut from, blended with the measurement.
    frac: f64,
    cancel: &'a AtomicBool,
}

impl SplitBatch<'_> {
    /// Dispatches both shares, adds the GPU's into `buffer` and blends the measured split
    /// into `*gpu_frac`; a mid-dispatch GPU decline retraces its share on the CPU and clears
    /// `*gpu_frac`, a cancellation discards the batch (`false`).
    fn run(
        &self,
        buffer: &mut [Vec3],
        gpu_buf: &mut Vec<Vec3>,
        gpu_frac: &mut Option<f64>,
    ) -> bool {
        let Self {
            gpu,
            scene,
            camera,
            environment,
            first_sample,
            threads,
            gpu_share,
            cpu_share,
            cancel,
            ..
        } = *self;
        let pixel_count = scene.width as usize * scene.height as usize;
        let traced_material = scene.traced_material();
        let gpu_scene = GpuSceneRef {
            camera,
            width: scene.width,
            height: scene.height,
            planes: &scene.planes,
            facet_finishes: self.facet_finishes,
            material: &traced_material,
            max_bounces: scene.max_bounces,
            environment,
            estimator: Some(scene.estimator),
        };
        gpu_buf.clear();
        gpu_buf.resize(pixel_count, Vec3::ZERO);
        let surfaces = scene.effects.surfaces();
        let surface = scene.effects.surface_scene(&surfaces);

        let (gpu_outcome, gpu_elapsed, cpu_elapsed) = if cpu_share == 0 {
            let start = Instant::now();
            let outcome = gpu.try_accumulate_geom_surface_cancellable(
                &gpu_scene,
                &scene.tools,
                &surface,
                first_sample,
                gpu_share,
                gpu_buf.as_mut_slice(),
                cancel,
            );
            (outcome, start.elapsed(), Duration::ZERO)
        } else {
            // See "One core reserved for the GPU submitter" on `hybrid_trace_into`.
            let cpu = CpuShare {
                scene,
                first_sample: first_sample + gpu_share,
                samples: cpu_share,
                threads: cpu_threads_beside_gpu(threads),
                buffer: &mut *buffer,
            };
            dispatch_concurrently(
                gpu,
                &gpu_scene,
                first_sample,
                gpu_share,
                gpu_buf.as_mut_slice(),
                cancel,
                cpu,
            )
        };

        match gpu_outcome {
            GpuAccumulate::Cancelled => return false,
            GpuAccumulate::Declined => {
                // Retrace the GPU's share on the CPU so the sample count stays exact, and
                // stop offering it work. A `false` return means `cancel` fired while queued
                // for CPU permits: discard the sub-batch like a GPU cancellation, rather than
                // report a buffer that lacks the retraced share.
                if !trace_into(
                    scene,
                    SampleRange::new(first_sample, gpu_share),
                    threads,
                    camera,
                    environment,
                    &mut *buffer,
                    cancel,
                ) {
                    return false;
                }
                *gpu_frac = None;
                return true;
            }
            GpuAccumulate::Done => {}
        }

        for (b, g) in buffer.iter_mut().zip(gpu_buf.iter()) {
            *b += *g;
        }

        if cpu_share > 0 {
            *gpu_frac = Some(blend_gpu_frac(
                self.frac,
                gpu_share,
                cpu_share,
                gpu_elapsed,
                cpu_elapsed,
            ));
        }

        true
    }
}

/// The CPU half of one concurrent hybrid sub-batch -- the upper, disjoint sample
/// sub-range [`hybrid_trace_into`] hands to [`dispatch_concurrently`] while the GPU traces
/// the lower one. Bundled so that function stays under clippy's argument limit.
struct CpuShare<'a> {
    scene: &'a SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    buffer: &'a mut [Vec3],
}

/// Runs the GPU's `[first_sample, first_sample + gpu_share)` share on a scoped thread
/// while the calling thread traces `cpu`'s own share via [`trace_into`], then joins.
/// Returns the GPU outcome plus each engine's wall time, measured independently so
/// [`blend_gpu_frac`] compares real throughputs rather than the join's slower half.
///
/// A panic on the GPU thread is re-raised here rather than swallowed: it means a bug,
/// not a device loss (which surfaces as [`GpuAccumulate::Declined`]).
#[expect(
    clippy::needless_pass_by_value,
    reason = "CpuShare::buffer is an exclusive &mut [Vec3] that must be MOVED into \
              trace_into below; taking `&CpuShare` instead would make that reborrow \
              impossible from behind a shared reference, so by-value is the only option"
)]
fn dispatch_concurrently(
    gpu: &GpuBackend,
    gpu_scene: &GpuSceneRef<'_>,
    first_sample: u32,
    gpu_share: u32,
    gpu_buf: &mut [Vec3],
    cancel: &AtomicBool,
    cpu: CpuShare<'_>,
) -> (GpuAccumulate, Duration, Duration) {
    // The stone's tools ride with the GPU half of the split, the CPU half reads them from
    // `cpu.scene` itself.
    let tools = &cpu.scene.tools;
    let surfaces = cpu.scene.effects.surfaces();
    let surface = cpu.scene.effects.surface_scene(&surfaces);
    thread::scope(|scope| {
        let gpu_handle = scope.spawn(move || {
            let start = Instant::now();
            let outcome = gpu.try_accumulate_geom_surface_cancellable(
                gpu_scene,
                tools,
                &surface,
                first_sample,
                gpu_share,
                gpu_buf,
                cancel,
            );
            (outcome, start.elapsed())
        });
        let cpu_start = Instant::now();
        // `cancel` is checked only between sub-batches (see `hybrid_trace_into`). A `false`
        // return means CPU permits never freed before cancellation; the joined GPU outcome
        // below decides the result.
        let _ = trace_into(
            cpu.scene,
            SampleRange::new(cpu.first_sample, cpu.samples),
            cpu.threads,
            gpu_scene.camera,
            gpu_scene.environment,
            cpu.buffer,
            cancel,
        );
        let cpu_elapsed = cpu_start.elapsed();
        let (outcome, gpu_elapsed) = gpu_handle
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
        (outcome, gpu_elapsed, cpu_elapsed)
    })
}

/// Blends the just-measured throughput split into the running `gpu_frac` estimate with the
/// 0.7-old/0.3-new EMA.
///
/// Returns `1.0` once the GPU outclasses the CPU so much that splitting no longer pays (see
/// `HYBRID_MAX_GPU_SHARE`). That keeps the decision stable: later calls send everything to
/// the GPU and skip this blend.
fn blend_gpu_frac(
    frac: f64,
    gpu_share: u32,
    cpu_share: u32,
    gpu_elapsed: Duration,
    cpu_elapsed: Duration,
) -> f64 {
    let gpu_rate = f64::from(gpu_share) / gpu_elapsed.as_secs_f64().max(1e-9);
    let cpu_rate = f64::from(cpu_share) / cpu_elapsed.as_secs_f64().max(1e-9);
    let measured_frac = gpu_rate / (gpu_rate + cpu_rate);
    let updated = frac.mul_add(0.7, measured_frac * 0.3).clamp(0.0, 1.0);
    if cpu_is_worth_splitting_with(updated) {
        updated
    } else {
        1.0
    }
}
