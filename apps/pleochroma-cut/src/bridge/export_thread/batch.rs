//! The batch render loop: hybrid CPU+GPU calibration and per-batch dispatch, plus the CPU
//! scanline batch tracer.
//!
//! The CPU share must not slow a render. Calibration is a short timing probe
//! (`calibrate_split`), and a GPU that does `HYBRID_MAX_GPU_SHARE` or more of the work runs
//! alone (`settled_gpu_share`).

use super::{cache_run::CacheRun, sample_cursor::SampleCursor, scene_snapshot::SceneSnapshot};
use glam::Vec3;
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::{adaptive::Pass, gpu_backend::GpuAccumulate};
use pleochroma::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{
        Camera, EnvironmentSource, FacetFinish, add_finite_sample, build_plane_soa,
        pixel_rotations, sample_draws,
        surface::{SurfaceScene, merged_facet_finishes},
        transport::{PreparedMaterial, trace_spectral_ray_prepared},
    },
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Below this many samples per pixel, calibration costs more than it saves, so a single engine
/// renders.
pub(super) const HYBRID_MIN_SPP: u32 = 8;

/// Above this GPU share the CPU share is dropped and the GPU runs alone. When the GPU is much
/// faster than the CPU, a split runs slower than the GPU alone, because the CPU's share then
/// dominates the batch time. Below the limit, a CPU within about 2.3 times of the GPU still
/// pays for the split.
const HYBRID_MAX_GPU_SHARE: f64 = 0.7;

/// The shortest time the CPU timing probe of [`calibrate_split`] may trace.
const CPU_PROBE_MIN: Duration = Duration::from_millis(20);

/// The longest time the CPU timing probe of [`calibrate_split`] may trace.
const CPU_PROBE_MAX: Duration = Duration::from_millis(500);

/// The most image rows the CPU timing probe traces.
const CPU_PROBE_ROWS: usize = 64;

/// The GPU share for a measured share `frac`: `frac` itself while the CPU is worth splitting
/// with, else `1.0` (see [`HYBRID_MAX_GPU_SHARE`]).
///
/// `1.0` is stable. A batch that runs one engine measures nothing, so the split is never
/// re-measured.
const fn settled_gpu_share(frac: f64) -> f64 {
    if frac < HYBRID_MAX_GPU_SHARE {
        frac
    } else {
        1.0
    }
}

/// Every core this process may use.
fn all_cpu_threads() -> usize {
    thread::available_parallelism().map_or(8, std::num::NonZero::get)
}

/// The CPU tracer's thread count while a GPU share runs beside it: every core but one.
///
/// One thread has to submit and poll the GPU's chunks. With every core tracing, that thread waits
/// for a core, the GPU idles meanwhile, and the measured GPU rate drops. Never zero.
const fn cpu_threads_beside_gpu(threads: usize) -> usize {
    if threads > 1 { threads - 1 } else { 1 }
}

/// The greatest common divisor of `a` and `b`.
const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let rest = a % b;
        a = b;
        b = rest;
    }
    a
}

/// A row step that spreads the first few claims of [`RowPlan::probe`] over the whole image:
/// about 0.618 of `height` and coprime to it, so claim `i` traces row `(i * step) % height` and
/// no row twice. `1` for an image under three rows.
const fn spread_step(height: usize) -> usize {
    if height < 3 {
        return 1;
    }
    let mut step = height * 618 / 1000;
    if step == 0 {
        step = 1;
    }
    while gcd(step, height) != 1 {
        step += 1;
    }
    step
}

/// Which image rows one CPU batch traces, how many, and for how long.
#[derive(Clone, Copy)]
struct RowPlan {
    /// Claim `i` traces image row `(i * step) % height` into row slot `i` of the buffer.
    step: usize,
    /// The most rows to trace.
    max_rows: usize,
    /// Stop claiming rows this long after the first claim; `None` never stops early.
    budget: Option<Duration>,
}

impl RowPlan {
    /// Every row, top to bottom: row slot `i` is image row `i`.
    const ALL: Self = Self {
        step: 1,
        max_rows: usize::MAX,
        budget: None,
    };

    /// The CPU timing probe: at most [`CPU_PROBE_ROWS`] rows spread over the image, traced for at
    /// most `budget` (a row that has started is finished).
    const fn probe(height: u32, budget: Duration) -> Self {
        Self {
            step: spread_step(height as usize),
            max_rows: CPU_PROBE_ROWS,
            budget: Some(budget),
        }
    }
}

/// The samples the calibration probe counts toward the render: its two GPU samples (the CPU
/// timing rows are discarded).
const CALIBRATION_SPP: u32 = 2;

/// The samples per pixel of the batch that confirms a fresh calibration
/// ([`calibrate_split_confirmed`]).
const HYBRID_CONFIRM_SPP: u32 = 16;

/// When one engine took more than this many times as long as the other for its share of a batch,
/// the split the batch started from was wrong, not drifting: the batch's own rates replace it.
const HYBRID_REBALANCE_RATIO: f64 = 2.0;

/// The log target of the hybrid split (`--verbose` prints it).
const HYBRID_LOG: &str = "pleochroma::hybrid";

/// The split after a batch that started from `frac` and measured `measured`, given the GPU's and
/// CPU's time for their shares.
///
/// A balanced batch moves the split by a 0.7 / 0.3 moving average. A lopsided one
/// ([`HYBRID_REBALANCE_RATIO`]) takes the measurement at once. A batch lasts as long as its
/// slower engine, and a moving average would take many batches to correct a bad seed.
const fn next_split(frac: f64, measured: f64, (gpu_secs, cpu_secs): (f64, f64)) -> f64 {
    let imbalance = gpu_secs.max(cpu_secs) / gpu_secs.min(cpu_secs).max(1e-9);
    let updated = if imbalance > HYBRID_REBALANCE_RATIO {
        measured
    } else {
        frac.mul_add(0.7, measured * 0.3)
    };
    updated.clamp(0.0, 1.0)
}

/// How many batches the local hybrid loop targets across the FULL export, not just
/// whatever share local ends up claiming (see [`local_chunk_size`]).
const TARGET_BATCHES: u32 = 40;

/// The local lane's claim size for [`SampleCursor::claim_local`]. It comes from the export's
/// total `samples_per_pixel`, so batch granularity and cancel latency do not depend on whether a
/// remote worker takes part.
pub(super) fn local_chunk_size(samples_per_pixel: u32) -> u32 {
    (samples_per_pixel / TARGET_BATCHES).max(1)
}

/// How long the local lane sleeps between re-checks of `SampleCursor::claim_local`
/// when it momentarily finds nothing to claim but the remote lane hasn't signalled
/// done yet. Export batches run for seconds at a time, so this poll interval only
/// matters in the closing moments.
const LOCAL_RETRY_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Everything one export batch needs, bundled to keep the helper signatures short.
pub(super) struct ExportCtx<'a> {
    /// Frozen GPU uploads for this frame, reused by all local claims.
    pub(super) ray_gpu: Option<(
        &'a pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend,
        &'a pleochroma::renderer::gpu_backend::whitted::WhittedGpuPrepared,
    )>,
    /// Preparation and plane arena shared by every absolute-tap claim in this frame.
    pub(super) ray: Option<(
        &'a pleochroma::renderer::whitted::WhittedPrep<'a>,
        &'a pleochroma::simd::PlanesSoA32,
    )>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) camera: &'a Camera,
    pub(super) scene: &'a SceneSnapshot,
    pub(super) gpu: &'a GpuBackend,
    pub(super) gpu_scene: &'a GpuSceneRef<'a>,
    /// The stone's facet surfaces, faceting machine and abraded edge
    /// (`SceneSnapshot::surface_scene`; `SurfaceScene::IDEAL` for a polished stone): the CPU
    /// tracer reads it in `render_batch`, the GPU through [`Self::try_gpu`].
    pub(super) surface: SurfaceScene<'a>,
    /// Set when a GPU thread panics in [`hybrid_batch`] and never cleared for this export. The
    /// backend's own `lost` flag does not catch a raw thread panic, so every GPU call checks this
    /// first, and the backend stays retired for later batches too.
    pub(super) gpu_retired: &'a AtomicBool,
    /// The path cache this render shades from (`RenderTaps::path_cache`), if one was chosen.
    pub(super) path_cache: Option<CacheRun<'a>>,
    /// The path-cache progress note (size line, or why it is off), taken by the first progress
    /// report.
    pub(super) cache_note: Mutex<Option<String>>,
}

impl ExportCtx<'_> {
    /// Runs this CPU share on every core, against the same prepared environment as the GPU share.
    fn render_cpu(&self, spp: u32, first: u32, accum: &mut [Vec3]) {
        self.render_cpu_on(all_cpu_threads(), spp, first, accum);
    }

    /// [`Self::render_cpu`] on exactly `threads` threads.
    fn render_cpu_on(&self, threads: usize, spp: u32, first: u32, accum: &mut [Vec3]) {
        let _ = render_prepared_batch(
            (self.width, self.height),
            (spp, first),
            self.camera,
            self.scene,
            accum,
            (threads, RowPlan::ALL),
            self.gpu_scene.environment,
        );
    }

    /// Times the CPU tracer on a few rows spread over the frame, for at most `budget`: returns the
    /// rows it finished and the time they took. The rows are sample `first` of their pixels and
    /// are discarded; nothing of the export's buffers is touched.
    fn probe_cpu(&self, first: u32, budget: Duration) -> (usize, Duration) {
        if self.width == 0 || self.height == 0 {
            return (0, Duration::ZERO);
        }
        let rows = (self.height as usize).min(CPU_PROBE_ROWS);
        let mut scratch = vec![Vec3::ZERO; rows * self.width as usize];
        render_prepared_batch(
            (self.width, self.height),
            (1, first),
            self.camera,
            self.scene,
            &mut scratch,
            (all_cpu_threads(), RowPlan::probe(self.height, budget)),
            self.gpu_scene.environment,
        )
    }

    /// The path-cache note, once: the first progress report takes it.
    pub(super) fn take_cache_note(&self) -> Option<String> {
        self.cache_note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Offers `spp` samples from `first_sample` to the GPU with the scene's tools and surface
    /// finish; `false` when it declines.
    fn try_gpu(&self, first_sample: u32, spp: u32, accum: &mut [Vec3]) -> bool {
        self.gpu.try_accumulate_geom_surface(
            self.gpu_scene,
            &self.scene.tools,
            &self.surface,
            first_sample,
            spp,
            accum,
        )
    }

    /// Offers one adaptive pass to the GPU: the samples of `pass` for the pixels of its live
    /// tiles, added to `sum`, and the odd-index ones to `half_b`, with the scene's tools and
    /// surface finish. [`GpuAccumulate::Declined`] leaves both buffers untouched, and so does
    /// [`GpuAccumulate::Cancelled`].
    #[cfg(feature = "adaptive-sampling")]
    pub(super) fn try_gpu_live(
        &self,
        pass: &Pass,
        sum: &mut [Vec3],
        half_b: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if let Some(cache) = &self.path_cache {
            return cache.try_gpu_live(self, pass, sum, half_b, cancel);
        }
        self.gpu.try_accumulate_geom_surface_live_cancellable(
            self.gpu_scene,
            &self.scene.tools,
            &self.surface,
            pass.first_sample,
            pass.spp,
            &pass.live,
            sum,
            half_b,
            cancel,
        )
    }
}

/// Measures both engines and returns the GPU's share of a hybrid batch, or `None` when the GPU
/// declines (no adapter, no `gpu` feature, or a scene it cannot trace).
///
/// The GPU takes two real 1-spp samples, both counted toward the export. The first includes
/// buffer allocation and driver warm-up, so only the second is timed.
///
/// The CPU side is a probe, not a sample: a few rows, timed for at most the GPU sample's time
/// and discarded. A whole CPU sample would cost as much as dozens of GPU samples, so the probe
/// is scaled up to a full sample's time.
///
/// Returns `1.0` (the GPU alone) when the share reaches [`HYBRID_MAX_GPU_SHARE`]. Checks
/// `cancel` between the GPU and CPU measurements.
pub(super) fn calibrate_split(
    ctx: &ExportCtx<'_>,
    samples_done: &mut u32,
    gpu_accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> Option<f64> {
    // A retired backend declines, without touching the possibly corrupted renderer.
    if ctx.scene.engine.is_ray() || ctx.gpu_retired.load(Ordering::Relaxed) {
        return None;
    }
    // Untimed warm-up sample: a real export sample (counted below), just not the one
    // whose wall-clock cost feeds the split.
    if !ctx.try_gpu(*samples_done, 1, gpu_accum) {
        return None;
    }
    *samples_done += 1;

    let start = Instant::now();
    if !ctx.try_gpu(*samples_done, 1, gpu_accum) {
        return None;
    }
    let gpu_elapsed = start.elapsed();
    *samples_done += 1;

    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let (rows, cpu_elapsed) = ctx.probe_cpu(
        *samples_done,
        gpu_elapsed.clamp(CPU_PROBE_MIN, CPU_PROBE_MAX),
    );
    let gpu_time = gpu_elapsed.as_secs_f64().max(1e-9);
    // The time of a whole CPU sample, scaled from the probe's rows. A probe that finished no row
    // within its budget is a CPU far slower than the GPU.
    let cpu_time = (rows > 0).then(|| {
        let probed = f64::from(u32::try_from(rows).unwrap_or(u32::MAX));
        (cpu_elapsed.as_secs_f64() * f64::from(ctx.height) / probed).max(1e-9)
    });
    // GPU share proportional to measured throughput (1/time per engine).
    let frac = cpu_time.map_or(1.0, |cpu_time| {
        (cpu_time / (gpu_time + cpu_time)).clamp(0.0, 1.0)
    });
    let settled = settled_gpu_share(frac);
    tracing::debug!(
        target: HYBRID_LOG,
        gpu_secs = gpu_time,
        cpu_secs = cpu_time.unwrap_or(f64::INFINITY),
        split = frac,
        settled,
        "hybrid calibration"
    );
    tracing::info!(
        target: "pleochroma_cut::render_bench",
        gpu_share_pct = frac * 100.0,
        gpu_only = settled >= 1.0,
        probe_rows = rows,
        "hybrid split: calibrated"
    );
    Some(settled)
}

/// [`calibrate_split`], then, when the render splits, one small real batch that checks the split.
///
/// One timing is noisy, so the seed can be far off. The confirmation batch
/// ([`HYBRID_CONFIRM_SPP`] samples) runs both engines on real shares, and [`hybrid_batch`]
/// corrects a lopsided split ([`next_split`]) before the first full batch. A GPU-only split needs
/// no confirmation. `available` is the samples left from `samples_done`. If there are too few for
/// the probe, the confirmation and a batch after it, the seed is used as is. `available` 0 skips
/// the confirmation, which renders with snapshots need.
pub(super) fn calibrate_split_confirmed(
    ctx: &ExportCtx<'_>,
    samples_done: &mut u32,
    available: u32,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> Option<f64> {
    let mut frac = calibrate_split(ctx, samples_done, gpu_accum, cancel);
    let splits = frac.is_some_and(|share| share < 1.0);
    let room = available >= CALIBRATION_SPP + HYBRID_CONFIRM_SPP + HYBRID_MIN_SPP;
    if splits && room && !cancel.load(Ordering::Relaxed) {
        hybrid_batch(
            ctx,
            *samples_done,
            HYBRID_CONFIRM_SPP,
            &mut frac,
            accum,
            gpu_accum,
        );
        *samples_done += HYBRID_CONFIRM_SPP;
    }
    frac
}

/// One hybrid batch: the GPU traces the lower sample range into `gpu_accum` on a scoped thread
/// while the CPU traces the upper range into `accum`. The ranges are disjoint, so each engine's
/// stratification stays consistent. If the GPU declines (e.g. device loss), its share is retraced
/// on the CPU so the sample count stays exact, and `gpu_frac` is cleared.
///
/// # Adapting the split
///
/// `calibrate_split` measures only one dispatch pair, and a fixed split would ignore drift over a
/// long export. A batch that measures both engines updates `gpu_frac` through [`next_split`]. A
/// batch that runs one engine leaves it unchanged, since there is no second timing to compare.
///
/// # When the CPU share is dropped
///
/// A share that reaches [`HYBRID_MAX_GPU_SHARE`] is stored as `1.0` ([`settled_gpu_share`]), and
/// later batches run on the GPU alone. While both engines run, the CPU leaves one core to the
/// thread that feeds the GPU ([`cpu_threads_beside_gpu`]).
pub(super) fn hybrid_batch(
    ctx: &ExportCtx<'_>,
    samples_done: u32,
    this_batch: u32,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
) {
    // A retired backend forces this batch onto the CPU and ends any GPU share.
    let frac = if ctx.gpu_retired.load(Ordering::Relaxed) {
        *gpu_frac = None;
        0.0
    } else {
        gpu_frac.unwrap_or(0.0)
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "frac is in 0..=1 so the rounded share is within 0..=this_batch"
    )]
    let gpu_share = (f64::from(this_batch) * frac).round() as u32;
    let gpu_share = gpu_share.min(this_batch);
    let cpu_share = this_batch - gpu_share;

    if gpu_share == 0 {
        ctx.render_cpu(this_batch, samples_done, accum);
        return;
    }

    let (gpu_ok, gpu_elapsed, cpu_elapsed) = if cpu_share == 0 {
        let start = std::time::Instant::now();
        let ok = ctx.try_gpu(samples_done, gpu_share, gpu_accum);
        (ok, start.elapsed(), std::time::Duration::ZERO)
    } else {
        thread::scope(|s| {
            let gpu_task = s.spawn(|| {
                let start = std::time::Instant::now();
                let ok = ctx.try_gpu(samples_done, gpu_share, gpu_accum);
                (ok, start.elapsed())
            });
            let cpu_start = std::time::Instant::now();
            ctx.render_cpu_on(
                cpu_threads_beside_gpu(all_cpu_threads()),
                cpu_share,
                samples_done + gpu_share,
                accum,
            );
            let cpu_elapsed = cpu_start.elapsed();
            if let Ok((gpu_ok, gpu_elapsed)) = gpu_task.join() {
                (gpu_ok, gpu_elapsed, cpu_elapsed)
            } else {
                // A panic, unlike a decline: retire the backend for the rest of the export.
                tracing::error!(
                    "GPU export thread panicked mid-batch; retiring the GPU \
                     backend for the rest of this export"
                );
                ctx.gpu_retired.store(true, Ordering::Relaxed);
                (false, std::time::Duration::ZERO, cpu_elapsed)
            }
        })
    };

    if !gpu_ok {
        ctx.render_cpu(gpu_share, samples_done, accum);
        *gpu_frac = None;
        return;
    }

    if cpu_share > 0 {
        let gpu_secs = gpu_elapsed.as_secs_f64().max(1e-9);
        let cpu_secs = cpu_elapsed.as_secs_f64().max(1e-9);
        let gpu_rate = f64::from(gpu_share) / gpu_secs;
        let cpu_rate = f64::from(cpu_share) / cpu_secs;
        let measured_frac = gpu_rate / (gpu_rate + cpu_rate);
        let updated = next_split(frac, measured_frac, (gpu_secs, cpu_secs));
        let settled = settled_gpu_share(updated);
        tracing::debug!(
            target: HYBRID_LOG,
            gpu_share,
            cpu_share,
            gpu_secs,
            cpu_secs,
            split = frac,
            measured = measured_frac,
            next = settled,
            "hybrid batch"
        );
        if settled > updated {
            tracing::info!(
                target: "pleochroma_cut::render_bench",
                gpu_share_pct = updated * 100.0,
                "hybrid split: the GPU share reached the limit, the rest of this render runs on the GPU alone"
            );
        }
        *gpu_frac = Some(settled);
    }
}

/// Runs the local hybrid loop. It claims ranges from the shared `cursor`, so local takes as much
/// of the budget as remains. It runs on one thread of a `thread::scope` while the remote lane
/// (`export_thread::remote::run_remote_lane`) runs on another.
///
/// `claim_size` maps the samples traced so far to the next claim's size. Callers normally pass
/// [`local_chunk_size`]. A render with snapshots caps it, so each claim ends on a snapshot point.
///
/// # Knowing when to stop
///
/// An empty `claim_local` does not mean done: the remote lane may still return a failed chunk's
/// remainder to the retry pile (`SampleCursor::return_to_local`, `Both` only). The loop stops
/// only when the shared pool and retry pile are empty and `remote_lane_done` is set. Callers with
/// no remote lane pass `true`, so the loop stops when the cursor is empty.
///
/// Returns `true` if cancellation was observed. A cancelled export must not be reported done
/// just because the local half stopped.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of the local hybrid loop's own state \
              (scene/GPU context, the shared cursor and its claim size, the CPU/GPU \
              split estimate, the two accumulation buffers, remote-lane-done \
              signalling, cancellation, and the progress callback) -- bundling them \
              into a struct would just move the same count into field access, not \
              reduce it"
)]
pub(super) fn run_local_batches(
    ctx: &ExportCtx<'_>,
    cursor: &SampleCursor,
    claim_size: impl Fn(u32) -> u32,
    remote_lane_done: &AtomicBool,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(u32, &[Vec3], &[Vec3]),
) -> bool {
    let mut local_traced = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }

        let Some((start, count)) = cursor.claim_local(claim_size(local_traced)) else {
            if remote_lane_done.load(Ordering::Acquire) {
                return false;
            }
            // The remote lane may still return a failed chunk's remainder.
            thread::sleep(LOCAL_RETRY_POLL_INTERVAL);
            continue;
        };

        if ctx.ray.is_some() {
            if super::worker::export_ray::accumulate(ctx, start, count, accum, cancel) {
                return true;
            }
        } else if let Some(cache) = &ctx.path_cache {
            // One engine per batch: the cached GPU entry, else the cached CPU tracer.
            if cache.accumulate(ctx, start, count, accum, cancel) {
                return true;
            }
        } else if gpu_frac.is_some() {
            hybrid_batch(ctx, start, count, gpu_frac, accum, gpu_accum);
        } else if ctx.gpu_retired.load(Ordering::Relaxed) || !ctx.try_gpu(start, count, gpu_accum) {
            // A retired GPU is not called again, since a panic may have corrupted its state.
            ctx.render_cpu(count, start, accum);
        }
        local_traced += count;
        on_batch(local_traced, accum, gpu_accum);
    }
}

/// Renders one batch of `batch_spp` samples per pixel into `accum`, on all available cores.
///
/// A non-finite sample is dropped but still counted (`add_finite_sample`), the rule every
/// backend follows, so the buffers merge by plain addition.
///
/// Rows are claimed from a shared atomic counter rather than split into fixed bands, because
/// rows through the stone cost far more than background rows. Each row goes to exactly one
/// thread, so each pixel's sum is the same however the rows are shared out.
// `pub`, not `pub(super)`: `bridge::preview_render` reuses this tracer for its CPU fallback,
// so the sampling exists in one place only.
pub fn render_batch(
    width: u32,
    height: u32,
    batch_spp: u32,
    samples_already_done: u32,
    camera: &Camera,
    scene: &SceneSnapshot,
    accum: &mut [Vec3],
) {
    let num_threads = thread::available_parallelism().map_or(8, std::num::NonZero::get);
    render_batch_with_threads(
        (width, height),
        batch_spp,
        samples_already_done,
        camera,
        scene,
        accum,
        num_threads,
    );
}

/// [`render_batch`] on exactly `num_threads` threads (at least one).
///
/// Preview lanes pass their share of the process-wide permit pool
/// (`preview_render::cpu_permits`), so concurrent CPU fallbacks share the cores. Each pixel's
/// sum depends only on its index, so the result is the same for any `num_threads`.
pub fn render_batch_with_threads(
    (width, height): (u32, u32),
    batch_spp: u32,
    samples_already_done: u32,
    camera: &Camera,
    scene: &SceneSnapshot,
    accum: &mut [Vec3],
    num_threads: usize,
) {
    let _ = render_prepared_batch(
        (width, height),
        (batch_spp, samples_already_done),
        camera,
        scene,
        accum,
        (num_threads, RowPlan::ALL),
        scene.environment(),
    );
}

/// Traces a CPU batch into `accum`.
///
/// `plan` picks the rows. [`RowPlan::ALL`] traces every image row. [`RowPlan::probe`] traces
/// a few spread rows, stopping when its budget runs out. Returns the rows finished and the time
/// the row loop took, excluding preparation.
fn render_prepared_batch(
    (width, height): (u32, u32),
    (batch_spp, samples_already_done): (u32, u32),
    camera: &Camera,
    scene: &SceneSnapshot,
    accum: &mut [Vec3],
    (num_threads, plan): (usize, RowPlan),
    environment: EnvironmentSource<'_>,
) -> (usize, Duration) {
    let num_threads = num_threads.max(1);
    let width_usize = width as usize;
    let height_usize = height as usize;

    let rows: Vec<Option<&mut [Vec3]>> = accum.chunks_mut(width_usize).map(Some).collect();
    // Row slots this batch fills: every image row for a plain batch, the probe's few otherwise.
    let claims = height_usize.min(rows.len()).min(plan.max_rows);
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);
    let rows_done = AtomicUsize::new(0);

    // Built once per batch: the plane arena depends on `scene.active_planes` alone (the
    // tools have none), and the kernel's output is the same as building it per sample.
    let plane_soa = build_plane_soa(&scene.active_planes);
    let stone = StoneGeometry {
        planes: &scene.active_planes,
        tools: &scene.tools,
    };
    // Render physics of the design (silk, play-of-color, facet surfaces, abraded edge). Empty
    // for a design without them, so the tracer is the plain one.
    let material = scene.traced_material();
    // Built once per batch and shared by every sample.
    let prepared = PreparedMaterial::new(&material);
    let facet_surfaces = scene.facet_surfaces();
    let surface = scene.surface_scene(&facet_surfaces);
    let finishes = merged_facet_finishes(&scene.facet_finishes, surface.facet_surfaces);

    let began = Instant::now();
    let deadline = plan.budget.and_then(|budget| began.checked_add(budget));

    thread::scope(|s| {
        for _ in 0..num_threads {
            let rows = &rows;
            let next_row = &next_row;
            let rows_done = &rows_done;
            let plane_soa = &plane_soa;
            let prepared = &prepared;
            let finishes: &[FacetFinish] = &finishes;

            s.spawn(move || {
                loop {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        break;
                    }
                    let claim = next_row.fetch_add(1, Ordering::Relaxed);
                    if claim >= claims {
                        break;
                    }
                    // This claim traces image row `y` into row slot `claim`. A plain batch
                    // has step 1, so the two are equal.
                    let y = claim * plan.step % height_usize;

                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[claim]
                        .take()
                        .expect("each row slot is claimed by exactly one thread via fetch_add");

                    for (x, pixel) in row.iter_mut().enumerate() {
                        let global_pixel_idx = u32::try_from(y * width_usize + x)
                            .expect("pixel index fits u32: export dimensions are capped");
                        let mut sample_sum = Vec3::ZERO;

                        // Per-pixel rotations for the jitter and hero-wavelength draws. Must
                        // match `trace_into` in `apps/pleochroma-worker/src/render_core/mod.rs`;
                        // both use the shared `sampling` functions.
                        let rot = pixel_rotations(global_pixel_idx);

                        for s_idx in 0..batch_spp {
                            let global_sample = samples_already_done + s_idx;
                            let draws = sample_draws(global_pixel_idx, global_sample, &rot);

                            let ray = camera.generate_ray(
                                x as f32,
                                y as f32,
                                width as f32,
                                height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );

                            // Empty `facet_finishes` (girdle toggle off) makes this equal to
                            // `trace_spectral_ray`.
                            let sample = trace_spectral_ray_prepared(
                                ray,
                                stone,
                                plane_soa,
                                finishes,
                                prepared,
                                &scene.fluorescence,
                                scene.max_bounces,
                                environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                                surface,
                            );
                            // Non-finite samples are dropped but still counted.
                            add_finite_sample(&mut sample_sum, sample);
                        }

                        *pixel += sample_sum;
                    }
                    rows_done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    (rows_done.into_inner(), began.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use pleochroma::optics::raytracer::DEFAULT_FOV_DEG;

    #[test]
    fn a_dominant_gpu_share_settles_on_the_gpu_alone() {
        assert_eq!(settled_gpu_share(0.0), 0.0);
        assert_eq!(settled_gpu_share(0.42), 0.42);
        assert_eq!(settled_gpu_share(0.699), 0.699);
        assert_eq!(settled_gpu_share(HYBRID_MAX_GPU_SHARE), 1.0);
        assert_eq!(settled_gpu_share(0.98), 1.0);
        assert_eq!(settled_gpu_share(1.0), 1.0);
        assert_eq!(settled_gpu_share(f64::NAN), 1.0);
    }

    #[test]
    fn one_core_is_left_to_the_gpu_feeder_and_never_all() {
        assert_eq!(cpu_threads_beside_gpu(0), 1);
        assert_eq!(cpu_threads_beside_gpu(1), 1);
        assert_eq!(cpu_threads_beside_gpu(2), 1);
        assert_eq!(cpu_threads_beside_gpu(12), 11);
    }

    #[test]
    fn the_probe_step_visits_every_row_once() {
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(7, 0), 7);
        for height in (1usize..=70).chain([512, 1080, 2160, 4320]) {
            let step = spread_step(height);
            assert_eq!(gcd(step, height), 1, "height {height}");
            let mut seen = vec![false; height];
            for claim in 0..height {
                let row = claim * step % height;
                assert!(!seen[row], "height {height}: row {row} twice");
                seen[row] = true;
            }
        }
        // The first claims of a tall image are spread, not the top rows.
        assert_eq!(spread_step(512), 317);
    }

    #[test]
    fn a_probe_with_time_traces_the_same_rows_as_a_plain_batch() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let (width, height) = (6u32, 7u32);
        let camera =
            Camera::new(0.3, 0.2, scene.distance, DEFAULT_FOV_DEG).with_roll(scene.camera_roll);
        let mut direct = vec![Vec3::ZERO; (width * height) as usize];
        let (rows, _) = render_prepared_batch(
            (width, height),
            (2, 5),
            &camera,
            &scene,
            &mut direct,
            (3, RowPlan::ALL),
            scene.environment(),
        );
        assert_eq!(rows, height as usize);

        let plan = RowPlan::probe(height, Duration::from_secs(3600));
        assert_eq!(plan.step, 4, "7 rows: step 4");
        let mut probed = vec![Vec3::ZERO; direct.len()];
        let (rows, _) = render_prepared_batch(
            (width, height),
            (2, 5),
            &camera,
            &scene,
            &mut probed,
            (3, plan),
            scene.environment(),
        );
        assert_eq!(rows, height as usize);
        let w = width as usize;
        for claim in 0..height as usize {
            let y = claim * plan.step % height as usize;
            assert_eq!(
                probed[claim * w..(claim + 1) * w],
                direct[y * w..(y + 1) * w],
                "row slot {claim} must hold image row {y}"
            );
        }
    }

    #[test]
    fn a_probe_without_time_traces_nothing() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let (width, height) = (6u32, 7u32);
        let camera =
            Camera::new(0.3, 0.2, scene.distance, DEFAULT_FOV_DEG).with_roll(scene.camera_roll);
        let mut probed = vec![Vec3::ZERO; (width * height) as usize];
        let (rows, _) = render_prepared_batch(
            (width, height),
            (1, 0),
            &camera,
            &scene,
            &mut probed,
            (3, RowPlan::probe(height, Duration::ZERO)),
            scene.environment(),
        );
        assert_eq!(rows, 0);
        assert!(probed.iter().all(|px| *px == Vec3::ZERO));
    }

    #[test]
    fn a_balanced_batch_blends_and_a_lopsided_one_takes_the_measurement() {
        // Both engines took about as long: the 0.7 / 0.3 blend.
        let blended = next_split(0.9, 0.8, (1.0, 1.2));
        assert!((blended - 0.87).abs() < 1e-12, "{blended}");
        // The CPU took six times as long as the GPU: the measurement replaces the split.
        let cpu_slow = next_split(0.87, 0.977, (1.3, 7.8));
        assert!((cpu_slow - 0.977).abs() < 1e-12, "{cpu_slow}");
        // The same when the GPU share is the slow one.
        let gpu_slow = next_split(0.99, 0.6, (5.0, 1.0));
        assert!((gpu_slow - 0.6).abs() < 1e-12, "{gpu_slow}");
        // Exactly at the ratio it still blends, and the result stays a fraction.
        let at_ratio = next_split(0.5, 1.0, (1.0, HYBRID_REBALANCE_RATIO));
        assert!((at_ratio - 0.65).abs() < 1e-12, "{at_ratio}");
        let clamped = next_split(1.0, 1.5, (1.0, 1.0));
        assert!((clamped - 1.0).abs() < 1e-12, "{clamped}");
    }
}
