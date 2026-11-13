//! Hybrid CPU+GPU tracing for one `stream_emit::tracer::run_tracer` job.
//!
//! The GPU and CPU tracers run concurrently over disjoint sample sub-ranges of a batch, whose sums
//! are additive. This differs from [`super::trace_samples_with_gpu`], which picks one engine for
//! the whole batch.
//!
//! [`calibrate`] times one real sample per pixel on each engine to seed an initial GPU
//! throughput share. [`hybrid_trace`] re-measures its split on every call and blends the
//! result into the running estimate with a 0.7-old/0.3-new exponential moving average, so
//! the split tracks the machine's throughput over a long stream.
//!
//! # Caching the calibration decision
//!
//! [`calibrate`] costs two GPU dispatches and a CPU trace, and its result depends on the
//! resource profile captured by [`JobKey`], not on the request's material or geometry.
//! [`calibrate_cached`] therefore caches the DECISION per [`JobKey`], not the raw
//! measurement, so jobs with the same profile skip the probe. [`finalize_split_cache`]
//! writes each job's final blended `gpu_frac` back when the job ends, so the seed improves
//! across jobs as [`hybrid_trace`]'s EMA improves it within one job.

use super::{resolve_facet_finishes, scene_uses_gpu, trace_into};
use crate::{assets::TrayRef, cli::ComputeMode};
use glam::Vec3;
use pleochroma::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, Estimator},
    renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
};
use pleochroma_dispatch::SampleRange;
use pleochroma_net::SceneState;
use std::{
    collections::BTreeMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

mod split;
// Sub-batch splitting lives in `split.rs`; re-exported here.
#[cfg(test)]
pub use split::hybrid_trace;
pub use split::{HybridBuffers, hybrid_trace_into};

/// Below this many total samples in a `RenderRequest`, hybrid calibration (two extra,
/// otherwise-unnecessary dispatches) costs more than splitting could ever save. Also
/// acts as the floor [`calibrate`] itself needs (three real samples).
pub const HYBRID_MIN_SPP: u32 = 8;

/// How many CPU threads to trace with while a GPU dispatch runs concurrently: every
/// core except one, reserved for the thread that submits and polls the GPU dispatch
/// (see the call site). Never returns 0: a single-core machine still traces its share
/// with something, and the reservation is an optimisation, not a requirement.
fn cpu_threads_beside_gpu(threads: usize) -> usize {
    super::effective_thread_count(threads)
        .saturating_sub(1)
        .max(1)
}

/// Above this GPU throughput share, splitting work with the CPU costs more than it saves,
/// so the job goes GPU-only for the rest of its life.
///
/// Measured on a Vulkan adapter (800x600, 6 bounces) with the GPU about 10x faster than the
/// CPU, a split ran at 0.58x the GPU-only speed. Per-sub-batch synchronisation and the CPU
/// tracer competing with the GPU's submit thread cost more than the split gains. `0.7`
/// admits hybrid only when the CPU is within roughly 2.3x of the GPU, which leaves headroom
/// for that overhead.
const HYBRID_MAX_GPU_SHARE: f64 = 0.7;

/// Whether a measured GPU share still leaves the CPU enough of a contribution to be
/// worth the synchronisation -- see [`HYBRID_MAX_GPU_SHARE`].
fn cpu_is_worth_splitting_with(gpu_share_frac: f64) -> bool {
    gpu_share_frac < HYBRID_MAX_GPU_SHARE
}

/// What [`calibrate`] found. `GpuDeclined` means the GPU refused the probe, while
/// `GpuDominates` means the [`HYBRID_MAX_GPU_SHARE`] cutoff fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalibrationOutcome {
    /// The GPU offered a share worth splitting for: `(gpu_share_frac, samples_consumed)`.
    Split(f64, u32),
    /// The GPU declined the probe dispatch itself (no adapter, or an unsupported
    /// material) -- the caller falls back to [`super::trace_samples_with_gpu`]'s
    /// either/or behavior for the whole job.
    GpuDeclined,
    /// The GPU accepted the probe, but its measured throughput share meets or exceeds
    /// [`HYBRID_MAX_GPU_SHARE`]. Carries the measured share for logging; the job runs
    /// GPU-only.
    GpuDominates { gpu_share_frac: f64 },
    /// `cancel` was observed set during calibration. A scene can make the 3-sample probe
    /// slow, so it honours cancellation like every other sub-batch.
    ///
    /// `consumed` is how many of the 3 probe samples were traced and already folded into
    /// `out` (0, 1 or 2). The caller must count them, as with [`Self::Split`].
    Cancelled { consumed: u32 },
}

/// Rebuilds the per-call scene pieces that [`GpuSceneRef`] and `trace_into` need. This is
/// cheap, so nothing has to outlive the caller's loop.
fn scene_pieces(scene: &SceneState) -> (Camera, Vec<pleochroma::optics::raytracer::FacetFinish>) {
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    (camera, resolve_facet_finishes(scene))
}

/// Times one real sample per pixel on each engine (GPU, then CPU) and returns a
/// [`CalibrationOutcome`].
///
/// Spends exactly 3 real samples from the front of `[first_sample, first_sample +
/// samples)` and folds them into `out` like any sub-batch. Requires `samples >= 3`, which
/// callers gating on [`HYBRID_MIN_SPP`] always satisfy.
///
/// The GPU side dispatches two 1-spp samples. The first pays one-time adapter and pipeline
/// warm-up, so only the second is timed, which keeps the estimate a steady-state read.
///
/// `cancel` is checked before each of the 3 probe samples. See
/// [`CalibrationOutcome::Cancelled`] for what `consumed` means.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection. The probe samples
/// carry it like every other sub-batch.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the probe's range, engines' inputs, cancellation, output and the request's resolved tray are independent"
)]
pub fn calibrate(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    out: &mut [Vec3],
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> CalibrationOutcome {
    // A scene the GPU cannot trace (see `scene_uses_gpu`) is CPU-only: there is nothing to
    // calibrate a split against.
    if samples < 3 || !scene_uses_gpu(scene, gpu) {
        return CalibrationOutcome::GpuDeclined;
    }
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 0 };
    }
    let (camera, facet_finishes) = scene_pieces(scene);
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray);
    let traced_material = scene.traced_material();
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &traced_material,
        max_bounces: scene.max_bounces,
        environment,
        estimator: Some(scene.estimator),
    };

    let surfaces = scene.effects.surfaces();
    let surface = scene.effects.surface_scene(&surfaces);
    // Untimed warm-up sample: real and counted, just not the one whose wall-clock cost
    // feeds the split.
    match gpu.try_accumulate_geom_surface_cancellable(
        &gpu_scene,
        &scene.tools,
        &surface,
        first_sample,
        1,
        out,
        cancel,
    ) {
        GpuAccumulate::Done => {}
        GpuAccumulate::Declined => return CalibrationOutcome::GpuDeclined,
        // `out` is guaranteed untouched, so `consumed: 0` is exact.
        GpuAccumulate::Cancelled => return CalibrationOutcome::Cancelled { consumed: 0 },
    }
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 1 };
    }

    let start = Instant::now();
    match gpu.try_accumulate_geom_surface_cancellable(
        &gpu_scene,
        &scene.tools,
        &surface,
        first_sample + 1,
        1,
        out,
        cancel,
    ) {
        GpuAccumulate::Done => {}
        GpuAccumulate::Declined => return CalibrationOutcome::GpuDeclined,
        // The warm-up sample already folded into `out`, so `consumed: 1` is exact.
        GpuAccumulate::Cancelled => return CalibrationOutcome::Cancelled { consumed: 1 },
    }
    let gpu_time = start.elapsed().as_secs_f64().max(1e-9);
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 2 };
    }

    let start = Instant::now();
    // A `false` return (cancelled while queued for CPU permits) is not checked here: the
    // caller re-checks `cancel` before its next sub-batch.
    let _ = trace_into(
        scene,
        SampleRange::new(first_sample + 2, 1),
        threads,
        &camera,
        environment,
        out,
        cancel,
    );
    let cpu_time = start.elapsed().as_secs_f64().max(1e-9);

    // GPU share proportional to measured throughput (1/time per engine).
    let frac = (cpu_time / (gpu_time + cpu_time)).clamp(0.0, 1.0);
    // A dominant GPU runs GPU-only, with no split overhead. This returns GpuDominates rather
    // than Split(1.0, 3) so the caller takes the plain path.
    //
    // On this path the three probe samples in `out` are discarded: the caller re-traces the
    // full range from first_sample. That is three samples once per job.
    if !cpu_is_worth_splitting_with(frac) {
        return CalibrationOutcome::GpuDominates {
            gpu_share_frac: frac,
        };
    }
    CalibrationOutcome::Split(frac, 3)
}

/// A resource profile: what a hybrid calibration decision ([`calibrate_cached`]) or a converged
/// sub-batch size (`stream_emit::sizing`) depends on.
///
/// See [`job_key`] for why material, geometry and lighting are left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobKey {
    /// [`ComputeMode`]'s discriminant, reduced to a small ordered tag (that type derives
    /// neither `Ord` nor `Hash`) -- see [`compute_mode_tag`].
    compute_mode: u8,
    /// The address of `gpu`. The backend is one shared `Arc` for the process lifetime, so
    /// this tells "no adapter" from a real one.
    gpu_id: usize,
    /// The realized thread count (`0` resolved through [`super::effective_thread_count`]),
    /// so `0` and the machine's actual core count share an entry.
    threads: usize,
    /// `scene.width * scene.height`, bucketed to its enclosing power of two.
    resolution_bucket: u32,
    /// The concave-tool count, bucketed by [`tool_bucket`]. `0` for a stone without tools.
    tool_bucket: u8,
    /// The scene's estimator as a small ordered tag (see [`estimator_tag`]). The two
    /// estimators reach different throughput on both engines.
    estimator: u8,
}

/// Reduces `estimator` to a small, totally-ordered tag for [`JobKey`], since the type derives
/// no `Ord`.
const fn estimator_tag(estimator: Estimator) -> u8 {
    match estimator {
        Estimator::Branching => 0,
        Estimator::ExitCollection => 1,
    }
}

/// The tool-count tag of [`JobKey`]: `0` for no tools, else `1 + floor(log2(count))` (1 tool:
/// 1; 2-3: 2; 4-7: 3; and so on). Stones with a similar tool cost share a calibration.
#[must_use]
pub fn tool_bucket(tool_count: usize) -> u8 {
    if tool_count == 0 {
        0
    } else {
        // `ilog2` of a nonzero usize is below 64, so the sum fits u8.
        u8::try_from(tool_count.ilog2() + 1).unwrap_or(u8::MAX)
    }
}

/// Reduces `mode` to a small, totally-ordered tag for [`JobKey`] -- [`ComputeMode`]
/// itself derives neither `Ord` nor `Hash`, so this is the one place that mapping lives.
const fn compute_mode_tag(mode: ComputeMode) -> u8 {
    match mode {
        ComputeMode::Hybrid => 0,
        ComputeMode::OnlyGpu => 1,
        ComputeMode::OnlyCpu => 2,
    }
}

/// Builds the [`JobKey`] for `gpu`, `scene`, `threads` and `compute_mode`.
///
/// It is keyed on six things:
/// - `compute_mode`: the three modes reach different throughput, and only `Hybrid`
///   calibrates a split at all.
/// - `gpu`'s identity: a different adapter, or none, has different throughput.
/// - the realized `threads` count.
/// - `scene`'s resolution, bucketed to the enclosing power of two. [`calibrate`] includes
///   fixed per-dispatch and per-call overhead, which a small preview amortizes far worse
///   than a full-size frame, so the GPU/CPU ratio shifts with resolution. Bucketing lets
///   nearby sizes share a decision.
/// - `scene`'s concave-tool count ([`tool_bucket`]). The tool carve costs the GPU kernels
///   and the CPU tracer differently, so a split measured on one stone must not seed another.
/// - `scene`'s estimator ([`estimator_tag`]). The two estimators cost the engines
///   differently.
///
/// Material, planes, lighting, environment and the sampled range are not keyed. They shift
/// both engines' per-sample cost together, not their relative split. [`calibrate`] and
/// `stream_emit`'s adaptive ramp re-measure per job anyway, so a wrong guess self-corrects
/// within one job.
#[must_use]
pub fn job_key(
    gpu: &GpuBackend,
    scene: &SceneState,
    threads: usize,
    compute_mode: ComputeMode,
) -> JobKey {
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    JobKey {
        compute_mode: compute_mode_tag(compute_mode),
        gpu_id: std::ptr::from_ref(gpu) as usize,
        threads: super::effective_thread_count(threads),
        resolution_bucket: pixels.max(1).ilog2(),
        tool_bucket: tool_bucket(scene.tools.len()),
        estimator: estimator_tag(scene.estimator),
    }
}

/// What [`calibrate_cached`]'s cache stores per [`JobKey`]: the DECISION a job starts from,
/// not the raw measurement. It never holds [`CalibrationOutcome::GpuDeclined`], since a
/// decline can depend on the request's material, or `Cancelled`, which is not a decision.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CachedDecision {
    /// [`CalibrationOutcome::GpuDominates`] fired: this resource profile's GPU share
    /// meets or exceeds [`HYBRID_MAX_GPU_SHARE`], so later jobs should skip straight to
    /// the single-engine path without probing again.
    GpuOnly,
    /// [`CalibrationOutcome::Split`] fired: later jobs should seed `gpu_frac` at this
    /// value rather than probing from scratch. Kept fresh by [`finalize_split_cache`] as
    /// each job's own EMA blend improves on it.
    Split {
        /// The seed a new job's `gpu_frac` should start from.
        gpu_frac: f64,
    },
}

/// The process-wide cache [`calibrate_cached`] reads and writes -- a `BTreeMap` (not a
/// hash map) purely to keep iteration/debug output deterministic; lookups are by exact
/// key equality either way, not by iteration order.
static CALIBRATION_CACHE: LazyLock<Mutex<BTreeMap<JobKey, CachedDecision>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// What [`calibrate_cached`] decided. Mirrors [`CalibrationOutcome`]'s payloads, but
/// `consumed` is always `0` on a cache hit: a hit skips [`calibrate`]'s 3-sample probe
/// entirely, so there is nothing to fold into `out`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CachedCalibration {
    /// Take the hybrid split path, seeded at `gpu_frac`. `consumed` probe samples (`3` on
    /// a fresh calibration, `0` on a cache hit) are already folded into `out`.
    Split {
        /// The initial `gpu_frac` the caller's sub-batch loop should start from.
        gpu_frac: f64,
        /// Real probe samples already folded into `out` -- fold this into the caller's
        /// own `produced` count exactly as [`CalibrationOutcome::Split`]'s `consumed` is.
        consumed: u32,
    },
    /// Take the single-engine (GPU-only) path for this whole job -- either a fresh
    /// [`CalibrationOutcome::GpuDominates`] or a cached [`CachedDecision::GpuOnly`].
    /// `consumed` is always `0` (see this enum's own doc comment).
    GpuOnly {
        /// Always `0` -- kept for symmetry with the other variants' fold-in contract.
        consumed: u32,
    },
    /// The GPU declined the probe itself, or there weren't enough samples to probe with.
    /// Never cached (see [`CachedDecision`]'s own doc comment), so this only ever comes
    /// from a fresh [`calibrate`] call.
    GpuDeclined,
    /// `cancel` fired mid-probe; `consumed` real samples (`0..=2`) already folded into
    /// `out`. Never produced on a cache hit -- a hit dispatches nothing, so nothing can
    /// cancel mid-probe.
    Cancelled {
        /// Real probe samples already folded into `out` before `cancel` fired.
        consumed: u32,
    },
}

/// Wraps [`calibrate`] with a process-wide cache of the calibration DECISION per [`JobKey`].
///
/// A cache hit skips the 3-sample probe and returns the cached split or GPU-only choice, so jobs
/// with the same resource profile pay the probe once.
///
/// `key` must be the result of [`job_key`] for the same `gpu`, `scene`, `threads` and
/// `compute_mode`. It is passed in so the caller builds it once and reuses it for
/// [`finalize_split_cache`].
///
/// `range` bundles `first_sample` and `samples`. Only `range.samples` (`>= 3`) matters to
/// the probe.
///
/// The decision is logged at `info` when a [`JobKey`] is calibrated, and at `debug` on
/// each later cache hit.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection, passed to [`calibrate`].
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the cache key, engines' inputs, range, cancellation, output and the request's resolved tray are independent"
)]
pub fn calibrate_cached(
    key: JobKey,
    gpu: &GpuBackend,
    scene: &SceneState,
    range: SampleRange,
    threads: usize,
    out: &mut [Vec3],
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> CachedCalibration {
    let cached = CALIBRATION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied();
    if let Some(decision) = cached {
        tracing::debug!(
            ?key,
            ?decision,
            "hybrid split: cache hit, reusing calibration decision"
        );
        return match decision {
            CachedDecision::GpuOnly => CachedCalibration::GpuOnly { consumed: 0 },
            CachedDecision::Split { gpu_frac } => CachedCalibration::Split {
                gpu_frac,
                consumed: 0,
            },
        };
    }

    let SampleRange {
        first_sample,
        samples,
    } = range;
    match calibrate(
        gpu,
        scene,
        first_sample,
        samples,
        threads,
        out,
        cancel,
        tray,
    ) {
        CalibrationOutcome::Split(gpu_frac, consumed) => {
            CALIBRATION_CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, CachedDecision::Split { gpu_frac });
            tracing::info!(
                ?key,
                gpu_frac,
                "hybrid split: calibrated and cached a new decision"
            );
            CachedCalibration::Split { gpu_frac, consumed }
        }
        CalibrationOutcome::GpuDominates { gpu_share_frac } => {
            CALIBRATION_CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, CachedDecision::GpuOnly);
            tracing::info!(
                ?key,
                gpu_share_pct = gpu_share_frac * 100.0,
                "hybrid CPU+GPU split declined: measured GPU share exceeds the hybrid \
                 cutoff, so this job runs GPU-only (splitting measured slower on this \
                 hardware) -- cached for this resource profile"
            );
            CachedCalibration::GpuOnly { consumed: 0 }
        }
        CalibrationOutcome::GpuDeclined => CachedCalibration::GpuDeclined,
        CalibrationOutcome::Cancelled { consumed } => CachedCalibration::Cancelled { consumed },
    }
}

/// Writes back what a job that ran on the hybrid split path converged to, when the job ends.
///
/// Callers invoke it only after [`calibrate_cached`] returned `Split` for the job, not for
/// `GpuOnly`, `GpuDeclined`, `Cancelled`, or the `OnlyGpu` and `OnlyCpu` modes. See
/// `stream_emit::tracer::run_tracer`.
///
/// `Some(frac)`: the job ended still on a live split. The cache gets this job's final
/// EMA-blended fraction (see [`hybrid_trace`]), so the next job seeds from it.
///
/// `None`: the GPU declined mid-job (device loss, where [`hybrid_trace`] clears `gpu_frac`
/// on [`GpuAccumulate::Declined`]), and the job fell back to CPU-only. The cached decision
/// is evicted rather than overwritten with a stale split, so the next job recalibrates.
pub fn finalize_split_cache(key: JobKey, gpu_frac: Option<f64>) {
    let mut cache = CALIBRATION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match gpu_frac {
        Some(frac) => {
            cache.insert(key, CachedDecision::Split { gpu_frac: frac });
        }
        None => {
            cache.remove(&key);
        }
    }
}

/// Pre-warms [`calibrate_cached`]'s cache for `gpu`, `scene`, `threads` and `compute_mode`
/// before any request needs it. `serve` and `join` call it at start-up with a representative
/// scene.
///
/// The probe's contribution goes to a throwaway buffer, and cancellation is never requested.
/// Run it at a quiet moment, not mid-request.
///
/// If the resource profile is already cached this does nothing. Otherwise it runs the same
/// 3-sample probe that a first [`calibrate_cached`] call would.
pub fn calibrate_now(
    gpu: &GpuBackend,
    scene: &SceneState,
    threads: usize,
    compute_mode: ComputeMode,
) {
    let key = job_key(gpu, scene, threads, compute_mode);
    let pixel_count = scene.width as usize * scene.height as usize;
    let mut throwaway = vec![Vec3::ZERO; pixel_count];
    let never_cancel = AtomicBool::new(false);
    let _ = calibrate_cached(
        key,
        gpu,
        scene,
        SampleRange::new(0, HYBRID_MIN_SPP),
        threads,
        &mut throwaway,
        &never_cancel,
        // The start-up probe scene has no Stone Shadow and no Tray Reflection.
        TrayRef::default(),
    );
}

#[cfg(test)]
mod tests;
