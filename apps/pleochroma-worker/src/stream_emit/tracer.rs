//! The free-running tracer: [`run_tracer`] traces a sample range in adaptive sub-batches.
//!
//! Each sub-batch is folded into a shared buffer and never touches the socket; see
//! `crate::serve`'s module docs for why that separation matters.

use super::{
    emitter::PendingDelta,
    sizing::{
        TARGET_SUBBATCH_FINAL_ONLY, next_batch_size, remember_converged_subbatch, seeded_batch_size,
    },
};
use crate::{
    assets::{self, TrayRef, TrayStop},
    cli::ComputeMode,
    render_core,
    render_core::hybrid::{CachedCalibration, HybridBuffers, JobKey},
};
use glam::Vec3;
use pleochroma::renderer::gpu_backend::GpuBackend;
use pleochroma_dispatch::SampleRange;
use pleochroma_net::{
    SceneState,
    messages::{ErrorMsg, error_codes},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// State shared by `run_tracer` (the writer) and the emitter in `run_stream` (the reader),
/// behind a `Mutex`. The lock is held only for brief updates and drains.
///
/// # The double-buffer swap protocol
///
/// `pending_delta` is the only per-pixel buffer here. The emitter (`emitter::EmitterAccum`)
/// keeps the running total, so the tracer never touches it. `PendingDelta::swap_with`
/// exchanges buffers in O(1) under the lock; the frame build, the fold, downsampling and
/// all socket writes happen after the lock is released.
///
/// A buffer left behind by a swap holds stale data and is never re-zeroed: the next
/// [`PendingDelta::add`] overwrites it entirely rather than accumulating.
pub(super) struct SharedState {
    pub(super) pending_delta: PendingDelta,
    pub(super) samples_done: u32,
    pub(super) finished: bool,
    /// Set when tracing panics. Pathological geometry can get past validation, so tracing
    /// runs inside `catch_unwind`. `run_stream` reports [`StreamOutcome::TracePanicked`].
    pub(super) panicked: bool,
    /// Set when the producer ends early without completion or cancellation (see
    /// [`super::ProducerSink`]). `run_stream` sends it as an `ERROR` with no `DONE`, via
    /// [`super::StreamOutcome::Failed`].
    pub(super) failed: Option<ErrorMsg>,
    /// The producer's own final sum of the whole request, when it has a better one than
    /// the emitter's running total (the coordinator's deterministic chunk-order merge,
    /// see `pleochroma_dispatch::Merger`). Used for the final `FRAME` under
    /// `TransferMode::FinalOnly`, the final `DISPLAY_FRAME`, and `FINAL_IMAGE`; `None`
    /// falls back to the running total. Always `None` for [`run_tracer`].
    pub(super) final_total: Option<Vec<Vec3>>,
    /// Samples of a `FinalImageRequest`'s viewer-reserved range that the coordinator rendered
    /// itself, because the viewer's `CONTRIBUTION` was late or invalid. Always `0` for
    /// [`run_tracer`].
    pub(super) reclaimed_samples: u32,
}

impl SharedState {
    /// A fresh state for a `pixel_count`-pixel request: nothing traced, nothing pending.
    pub(super) fn new(pixel_count: usize) -> Self {
        Self {
            pending_delta: PendingDelta::new(pixel_count),
            samples_done: 0,
            finished: false,
            panicked: false,
            failed: None,
            final_total: None,
            reclaimed_samples: 0,
        }
    }
}

/// One tracer job's fixed inputs, bundled to keep [`run_tracer`] within clippy's argument
/// limit.
pub(super) struct TracerJob {
    pub(super) scene: SceneState,
    pub(super) first_sample: u32,
    pub(super) samples: u32,
    pub(super) threads: usize,
    /// `--only-gpu`, `--only-cpu` or the hybrid default. Only hybrid attempts a split
    /// calibration.
    pub(super) compute_mode: ComputeMode,
    /// The request's Stone Shadow and Tray Reflection, resolved by [`run_tracer`] before its
    /// first sub-batch and held for the whole run. A coordinator lane shares one pin across
    /// the chunks of a job, so the map and mirror are built once per job, not per chunk.
    pub(super) pin: Arc<assets::TrayPin>,
    /// The duration the sub-batches adapt toward: short for a streamed request, long for one
    /// that sends its radiance once (see `super::sizing`'s "Why two targets").
    pub(super) subbatch: Duration,
}

/// Traces `[job.first_sample, job.first_sample + job.samples)` in adaptive sub-batches (see
/// [`next_batch_size`]) and folds each into `state.pending_delta`. Never touches the socket.
///
/// `cancel` is checked between sub-batches and mid-dispatch, so cancellation latency is bounded
/// by the GPU's chunk size rather than a whole sub-batch. A cancelled dispatch folds nothing.
/// Ray-Traced scenes take the deterministic path in [`trace_all_batches`] instead.
///
/// # Hybrid CPU+GPU
///
/// Only `ComputeMode::Hybrid` with `samples >= render_core::hybrid::HYBRID_MIN_SPP` on a
/// scene the GPU can trace (`render_core::scene_uses_gpu`) attempts a split, through
/// [`render_core::hybrid::calibrate_cached`]. `OnlyGpu` and `OnlyCpu` use a single engine:
/// GPU-first with per-frame CPU fallback, or CPU only through [`GpuBackend::disabled`].
///
/// The calibration cache is keyed on the job's [`JobKey`], so later jobs with the same resource
/// profile reuse the decision instead of probing again. See [`CachedCalibration`] for the
/// outcomes. A job that ran a split writes its final blend back through
/// [`render_core::hybrid::finalize_split_cache`]. The sub-batch size is cached the same way, via
/// [`seeded_batch_size`] and [`remember_converged_subbatch`], for every compute mode.
///
/// Runs inside `catch_unwind`: a panic in the tracer sets `state.panicked` rather than
/// silently ending this thread.
///
/// # The Stone Shadow and the Tray Reflection
///
/// The map and the mirror are resolved through `job.pin` before the first sub-batch and held
/// until the run ends, so the build is not timed as part of a sub-batch. A map or mirror that
/// cannot be built ends the run with `state.failed` set to a `VALIDATION_FAILED` error carrying
/// `TrayShadowOff::message()` or `TrayMirrorOff::message()`. A cancellation during the build
/// ends it like any other cancellation. Nothing is traced without the shadow or the mirror the
/// scene asked for.
pub(super) fn run_tracer(
    job: &TracerJob,
    gpu: &GpuBackend,
    state: &Arc<Mutex<SharedState>>,
    cancel: &AtomicBool,
    progress_tx: &mpsc::Sender<()>,
) {
    let reporter = Reporter { state, progress_tx };
    // Computed once so the calibration and sub-batch sizing caches share one key (see
    // `render_core::hybrid::job_key`).
    let key = render_core::hybrid::job_key(gpu, &job.scene, job.threads, job.compute_mode);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match job.pin.resolve(&job.scene, cancel) {
            Ok(tray) => {
                render_core::note_tray_route(&job.scene, gpu);
                trace_all_batches(key, job, tray.view(), gpu, &reporter, cancel);
                None
            }
            Err(TrayStop::Cancelled) => None,
            Err(TrayStop::Refused(message)) => Some(message),
        }
    }));

    let mut guard = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.finished = true;
    guard.panicked = result.is_err();
    if let Ok(Some(message)) = &result {
        guard.failed = Some(ErrorMsg {
            code: error_codes::VALIDATION_FAILED,
            request_id: None,
            message: (*message).to_owned(),
        });
    }
    if job.scene.engine.is_ray()
        && guard.failed.is_none()
        && !guard.panicked
        && !cancel.load(Ordering::Acquire)
        && guard.samples_done != job.samples
    {
        guard.failed = Some(ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            request_id: None,
            message: "the renderer could not trace the complete requested sample range".to_owned(),
        });
    }
    drop(guard);
    let _ = progress_tx.send(());
}

/// Traces the whole job against the resolved `tray`: a hybrid split when the job qualifies,
/// then the adaptive sub-batch loop, folding each result in through `reporter`.
///
/// The caches are written back only on normal completion. A panic unwinds past that step, so
/// a partial job never stores a bogus value.
fn trace_all_batches(
    key: JobKey,
    job: &TracerJob,
    tray: TrayRef<'_>,
    gpu: &GpuBackend,
    reporter: &Reporter<'_>,
    cancel: &AtomicBool,
) {
    let mut produced: u32 = 0;
    if job.scene.engine.is_ray() {
        // No spectral probes consume or repeat these absolute deterministic taps.
        while produced < job.samples && !cancel.load(Ordering::Acquire) {
            let count = if produced == 0 {
                1
            } else {
                (job.samples - produced).min(4)
            };
            let first = job.first_sample + produced;
            let mut part = vec![Vec3::ZERO; job.scene.width as usize * job.scene.height as usize];
            if !render_core::whitted::trace(
                &job.scene,
                tray,
                first,
                count,
                job.threads,
                job.compute_mode != ComputeMode::OnlyCpu,
                cancel,
                &mut part,
            ) {
                break;
            }
            reporter.fold_and_notify(first, count, &part, &mut produced);
        }
        return;
    }
    let mut batch_size: u32 = seeded_batch_size(key, job.subbatch);

    // Only `Hybrid` attempts a split. A scene the GPU cannot trace is CPU-only and never
    // calibrates one either.
    let mut gpu_frac: Option<f64> = if matches!(job.compute_mode, ComputeMode::Hybrid)
        && job.samples >= render_core::hybrid::HYBRID_MIN_SPP
        && render_core::scene_uses_gpu(&job.scene, gpu)
    {
        calibrate_hybrid_split(key, gpu, (job, tray), reporter, cancel, &mut produced)
    } else {
        None
    };
    // Only a job that started on the split path finalizes the split cache, so it never
    // evicts an entry it did not create.
    let started_on_split_path = gpu_frac.is_some();

    let pixel_count = job.scene.width as usize * job.scene.height as usize;
    let mut scratch: Vec<Vec3> = Vec::new();
    // Kept across sub-batches; `hybrid_trace_into` re-zeroes them.
    let mut hybrid_bufs = HybridBuffers::default();
    while produced < job.samples {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let this_batch = batch_size.min(job.samples - produced);
        let batch_first = job.first_sample + produced;

        let start = Instant::now();
        let dispatch_outcome: Option<&[Vec3]> = if gpu_frac.is_some() {
            render_core::hybrid::hybrid_trace_into(
                gpu,
                &job.scene,
                (batch_first, this_batch),
                job.threads,
                &mut gpu_frac,
                cancel,
                &mut hybrid_bufs,
                tray,
            )
            .then_some(hybrid_bufs.sum.as_slice())
        } else {
            // Reused across sub-batches and re-zeroed here, since both backends add into
            // it. `fold_and_notify` copies out of it under the lock.
            scratch.clear();
            scratch.resize(pixel_count, Vec3::ZERO);
            render_core::trace_samples_with_gpu_into(
                gpu,
                &job.scene,
                (batch_first, this_batch),
                job.threads,
                cancel,
                &mut scratch,
                tray,
            )
            .then_some(scratch.as_slice())
        };
        // `None` means `cancel` was seen mid-dispatch. Nothing was added for this
        // sub-batch, so the job stops without touching `produced` or `state`.
        let Some(buf) = dispatch_outcome else {
            break;
        };
        let elapsed = start.elapsed();

        reporter.fold_and_notify(batch_first, this_batch, buf, &mut produced);

        batch_size = next_batch_size(batch_size, elapsed, job.subbatch);
    }

    // Store the converged state for later jobs with the same resource profile and target.
    remember_converged_subbatch(key, job.subbatch, batch_size);
    if started_on_split_path {
        render_core::hybrid::finalize_split_cache(key, gpu_frac);
    }
}

/// What [`trace_range`] traced: the summed radiance of a PREFIX of the range.
pub struct TracedRange {
    /// Summed radiance of the first `done` samples, `width * height` long.
    pub sum: Vec<Vec3>,
    /// Samples traced (a prefix: tracing stops between sub-batches on cancel).
    pub done: u32,
    /// The tracer panicked (`sum`/`done` still hold what came before it).
    pub panicked: bool,
    /// Tracing stopped short without cancellation or a panic.
    pub failed: Option<ErrorMsg>,
}

/// Traces `[first_sample, first_sample + samples)` of `scene` synchronously on this thread, with
/// the same tracer a streamed request uses ([`run_tracer`]).
///
/// It stops between sub-batches once `cancel` is set. A coordinator lane runs this per chunk.
///
/// `pin` is the chunk's job's Stone Shadow and Tray Reflection pin ([`TracerJob::pin`]).
/// A lane keeps one per job, so every chunk reads the map and mirror the first chunk
/// resolved. A map or mirror that cannot be built comes back as `failed`, as in [`run_tracer`].
pub fn trace_range(
    scene: &SceneState,
    (first_sample, samples): (u32, u32),
    threads: usize,
    compute_mode: ComputeMode,
    gpu: &GpuBackend,
    cancel: &AtomicBool,
    pin: &Arc<assets::TrayPin>,
) -> TracedRange {
    let pixel_count = scene.width as usize * scene.height as usize;
    let state = Arc::new(Mutex::new(SharedState::new(pixel_count)));
    let (progress_tx, _progress_rx) = mpsc::channel();
    let job = TracerJob {
        // The job's scene is held as received (a joined worker poses its own copy), so each
        // chunk poses it here.
        scene: render_core::pose::posed_scene(scene).into_owned(),
        first_sample,
        samples,
        threads,
        compute_mode,
        pin: Arc::clone(pin),
        // The chunk's sum is returned once, at the end.
        subbatch: TARGET_SUBBATCH_FINAL_ONLY,
    };
    run_tracer(&job, gpu, &state, cancel, &progress_tx);
    let mut guard = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut sum = vec![Vec3::ZERO; pixel_count];
    let range = guard.pending_delta.swap_with(&mut sum);
    let panicked = guard.panicked;
    let failed = guard.failed.take();
    drop(guard);
    // Nothing folded in leaves `sum` as the untouched all-zero initial buffer.
    TracedRange {
        sum,
        done: range.map_or(0, |(_, n)| n),
        panicked,
        failed,
    }
}

/// Bundles the shared-state plumbing a calibration/sub-batch result needs to fold in and
/// announce, so [`calibrate_hybrid_split`] stays within clippy's argument-count limit.
struct Reporter<'a> {
    state: &'a Arc<Mutex<SharedState>>,
    progress_tx: &'a mpsc::Sender<()>,
}

impl Reporter<'_> {
    /// Folds `contribution` (the `consumed` samples starting at `first_sample`) into
    /// `state.pending_delta`, advances `*produced` and wakes the emitter.
    ///
    /// The running total is not kept here; the emitter builds it (see [`SharedState`]).
    fn fold_and_notify(
        &self,
        first_sample: u32,
        consumed: u32,
        contribution: &[Vec3],
        produced: &mut u32,
    ) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .pending_delta
            .add(first_sample, consumed, contribution);
        *produced += consumed;
        guard.samples_done = *produced;
        drop(guard);
        let _ = self.progress_tx.send(());
    }
}

/// Calibrates a hybrid split for a job that already qualifies for one (see [`run_tracer`]).
///
/// Folds the samples the probe produced (`0` on a cache hit; see [`CachedCalibration`]) and
/// returns the initial `gpu_frac` for the sub-batch loop. Only `Split` returns `Some`; every
/// other outcome leaves the job on the single-engine path.
fn calibrate_hybrid_split(
    key: JobKey,
    gpu: &GpuBackend,
    (job, tray): (&TracerJob, TrayRef<'_>),
    reporter: &Reporter<'_>,
    cancel: &AtomicBool,
    produced: &mut u32,
) -> Option<f64> {
    let pixel_count = job.scene.width as usize * job.scene.height as usize;
    let mut calib_buf = vec![Vec3::ZERO; pixel_count];
    match render_core::hybrid::calibrate_cached(
        key,
        gpu,
        &job.scene,
        SampleRange::new(job.first_sample, job.samples),
        job.threads,
        &mut calib_buf,
        cancel,
        tray,
    ) {
        CachedCalibration::Split { gpu_frac, consumed } => {
            if consumed > 0 {
                reporter.fold_and_notify(job.first_sample, consumed, &calib_buf, produced);
            }
            Some(gpu_frac)
        }
        // Single-engine path. `calibrate_cached` logs the decision itself.
        CachedCalibration::GpuOnly { consumed } => {
            if consumed > 0 {
                reporter.fold_and_notify(job.first_sample, consumed, &calib_buf, produced);
            }
            None
        }
        CachedCalibration::GpuDeclined => None,
        CachedCalibration::Cancelled { consumed } => {
            // Fold consumed samples as for `Split`, so progress is not lost. `None` is
            // fine: the sub-batch loop sees the same flag and stops.
            if consumed > 0 {
                reporter.fold_and_notify(job.first_sample, consumed, &calib_buf, produced);
            }
            None
        }
    }
}
