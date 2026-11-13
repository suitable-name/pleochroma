//! The phases of one [`super::render_accumulation`] call: resolving the remote lane
//! (availability, then calibration), running the local and remote lanes concurrently
//! over one shared claim point, and merging each engine's buffer exactly once.

use super::super::{
    taps::{LinearImageKind, SnapshotSchedule, snapshot_claim_cap, summed},
    types::{Accumulation, AccumulationCarry, AccumulationOutcome},
};
#[cfg(feature = "beam")]
use crate::bridge::export_thread::remote::BeamClientPool;
use crate::{
    bridge::{
        export_thread::{
            ExportProgress,
            batch::{
                ExportCtx, HYBRID_MIN_SPP, calibrate_split_confirmed, local_chunk_size,
                run_local_batches,
            },
            params::ComputeTarget,
            preview::PreviewThrottle,
            remote::{
                self, REMOTE_CALIBRATION_SAMPLES, REMOTE_MIN_SPP, RemoteCalibration,
                RemoteCapability, RemoteProgress, calibrate_remote_rate, exceeds_pixel_cap,
                probe_remote, run_remote_lane,
            },
            sample_cursor::SampleCursor,
            scene_snapshot::SceneSnapshot,
        },
        remote::{inclusions_refusal, remote_can_render, zoned_scene_refusal},
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use glam::Vec3;
use pleochroma_net::SceneState;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

/// Starting rate (samples/sec) for a remote lane with no calibration probe: the `RemoteOnly`
/// case where the whole budget is smaller than one probe. The value barely matters, since
/// `SampleCursor::claim` clamps the only chunk to the remainder. Not `0.0` or negative, which
/// `remote_chunk_samples` reads as "use the floor".
const DEFAULT_REMOTE_RATE_GUESS: f64 = 1000.0;

/// The absolute index of the sample that is `done` samples into a render whose first sample
/// is `offset`. With `offset` 0 it is `done`.
#[must_use]
const fn absolute_sample(offset: u32, done: u32) -> u32 {
    offset.saturating_add(done)
}

/// Resolves which remote worker (if any) this call's remote lane uses. Reuses
/// `carry.remote_capability` once a prior call on the same carry has probed (see
/// [`AccumulationCarry::remote_probed`]); otherwise probes now. `Err` is only returned for
/// `ComputeTarget::RemoteOnly`, which has no local fallback.
pub(super) fn resolve_remote_capability(
    carry: &mut AccumulationCarry,
    compute_target: ComputeTarget,
    worker: Option<&WorkerSettings>,
    scene: &SceneSnapshot,
    width: u32,
    height: u32,
    pending_note: &mut Option<String>,
) -> Result<Option<RemoteCapability>, String> {
    resolve_probe(
        (&mut carry.remote_probed, &mut carry.remote_capability),
        compute_target,
        worker,
        (scene, width, height),
        pending_note,
    )
}

/// What a beam render takes from the carry: the cached probe and the kept beam-tile connections.
///
/// A struct of three field borrows, so the render can hold it beside `&carry.taps`.
#[cfg(feature = "beam")]
pub(super) struct BeamLaneCache<'c> {
    /// [`AccumulationCarry::remote_probed`].
    pub(super) probed: &'c mut bool,
    /// [`AccumulationCarry::remote_capability`].
    pub(super) capability: &'c mut Option<RemoteCapability>,
    /// The beam-tile connections kept across frames.
    pub(super) clients: &'c mut BeamClientPool,
}

#[cfg(feature = "beam")]
impl BeamLaneCache<'_> {
    /// [`resolve_remote_capability`] over the cached probe: probes once per carry, like the
    /// ordinary lane.
    pub(super) fn resolve(
        &mut self,
        compute_target: ComputeTarget,
        worker: Option<&WorkerSettings>,
        frame: (&SceneSnapshot, u32, u32),
        pending_note: &mut Option<String>,
    ) -> Result<Option<RemoteCapability>, String> {
        resolve_probe(
            (&mut *self.probed, &mut *self.capability),
            compute_target,
            worker,
            frame,
            pending_note,
        )
    }
}

/// [`resolve_remote_capability`] over the two probe fields of a carry.
fn resolve_probe(
    (probed, cached): (&mut bool, &mut Option<RemoteCapability>),
    compute_target: ComputeTarget,
    worker: Option<&WorkerSettings>,
    (scene, width, height): (&SceneSnapshot, u32, u32),
    pending_note: &mut Option<String>,
) -> Result<Option<RemoteCapability>, String> {
    if matches!(compute_target, ComputeTarget::LocalOnly) {
        return Ok(None);
    }
    if *probed {
        return Ok(cached.clone());
    }
    // Re-probed here, not trusted from whatever the export dialog observed when it
    // opened, so a worker that went away or came up in the meantime is judged by
    // actual state at dispatch time.
    let resolved = match probe_remote(worker) {
        Ok(cap) => {
            if let Err(refusal) = remote_can_render(scene.env_map.as_ref(), cap.hdr) {
                // The one shared rule (`bridge::remote::guard`), per capability: an HDR
                // map the remote cannot render (or that cannot be sent) makes remote
                // compute UNSOUND, not merely slower -- the remote would light the stone
                // differently than the local half, and `render_accumulation` sums both
                // halves into one buffer. Falling back to local-only for the WHOLE
                // render keeps the image correct and says so plainly -- for the still
                // export and the tilt video alike. `RemoteOnly` is the exception: it was
                // chosen to keep this computer free, so tracing locally instead would
                // betray it; it fails with the same sentence. Cached like every other
                // outcome below, so the note is not repeated every frame.
                if matches!(compute_target, ComputeTarget::RemoteOnly) {
                    return Err(format!(
                        "{} Remote Only cannot render this scene; choose Local Only or \
                         Local + Remote.",
                        refusal.export_note()
                    ));
                }
                *pending_note = Some(refusal.export_note().to_string());
                *probed = true;
                *cached = None;
                return Ok(None);
            }
            // The same per-capability rule for colour zones. A remote
            // without the zoning capability would render only the base zone and
            // `render_accumulation` sums both halves into one buffer, so a zoned stone keeps
            // the WHOLE render local; `RemoteOnly` fails with the same sentence.
            if let Err(refusal) = zoned_scene_refusal(
                !scene.engine.is_ray() && scene.material.zoning.is_some(),
                cap.zoning,
            ) {
                if matches!(compute_target, ComputeTarget::RemoteOnly) {
                    return Err(format!(
                        "{} Remote Only cannot render this scene; choose Local Only or \
                         Local + Remote.",
                        refusal.export_note()
                    ));
                }
                *pending_note = Some(refusal.export_note().to_string());
                *probed = true;
                *cached = None;
                return Ok(None);
            }
            // And for oriented inclusions the wire record could not carry (a custom material's
            // silk): the remote would trace the stone without them.
            if let Err(refusal) =
                inclusions_refusal(scene.engine.is_ray(), &scene.material, &scene.effects)
            {
                if matches!(compute_target, ComputeTarget::RemoteOnly) {
                    return Err(format!(
                        "{} Remote Only cannot render this scene; choose Local Only or \
                         Local + Remote.",
                        refusal.export_note()
                    ));
                }
                *pending_note = Some(refusal.export_note().to_string());
                *probed = true;
                *cached = None;
                return Ok(None);
            }
            if exceeds_pixel_cap(width, height, &cap) {
                // Never silently shrink the export. `Both` falls back to local for the
                // WHOLE export with a status message; `RemoteOnly` has no local
                // fallback, so this fails outright instead.
                let pixels = u64::from(width) * u64::from(height);
                let message = format!(
                    "This export ({pixels} px) exceeds the remote worker's maximum of \
                     {} px.",
                    cap.max_pixels
                );
                if matches!(compute_target, ComputeTarget::RemoteOnly) {
                    return Err(message);
                }
                *pending_note = Some(format!("{message} Rendering locally only."));
                None
            } else {
                Some(cap)
            }
        }
        Err(reason) => {
            // Same `RemoteOnly`-has-no-fallback reasoning as above.
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(reason.message());
            }
            *pending_note = Some(format!("{} Rendering locally only.", reason.message()));
            None
        }
    };
    *probed = true;
    cached.clone_from(&resolved);
    Ok(resolved)
}

/// Resolves the remote lane's starting throughput estimate for this call: reuses
/// `carry.remote_rate` once a prior call on the same carry has measured one
/// (skipping the calibration probe entirely), or times a fresh
/// [`REMOTE_CALIBRATION_SAMPLES`]-sample probe. `Ok(None)` means "no remote lane this call"
/// (not worth it for the SPP remaining, cancelled, or the probe failed/ran short with
/// nothing to seed a lane from); `Err` is fatal only for `ComputeTarget::RemoteOnly`.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one calibration attempt's own \
              identity (the carry to seed from/into, compute target, worker/scene \
              state, resolution, the shared sample budget/accumulator, cancellation, \
              and the pending-note slot) -- bundling them into a struct would just \
              move the same count into field access, not reduce it"
)]
fn resolve_remote_rate(
    carry: &mut AccumulationCarry,
    compute_target: ComputeTarget,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    sample_offset: u32,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    cancel: &AtomicBool,
    pending_note: &mut Option<String>,
) -> Result<Option<f64>, String> {
    let remaining = samples_per_pixel - *samples_done;
    // `RemoteOnly` bypasses `REMOTE_MIN_SPP`: that threshold only avoids the overhead
    // of remote when local could handle a tiny remainder alone, which is irrelevant
    // once the user explicitly chose `RemoteOnly`.
    let worth_attempting = scene_state.engine.is_ray()
        || remaining >= REMOTE_MIN_SPP
        || matches!(compute_target, ComputeTarget::RemoteOnly);
    if !worth_attempting || remaining == 0 {
        return Ok(None);
    }
    if scene_state.engine.is_ray() {
        return Ok(Some(DEFAULT_REMOTE_RATE_GUESS));
    }
    if let Some(seeded) = carry.remote_rate {
        return Ok(Some(seeded));
    }

    // A failed or short probe's reason reaches the user through `RemoteCalibration`.
    // `Cancelled` gets no note: the caller's `cancel` check ends the render right after.
    let calibration = if remaining >= REMOTE_CALIBRATION_SAMPLES {
        // `samples_done` counts samples; the probe wants the absolute index it starts at.
        let mut absolute = absolute_sample(sample_offset, *samples_done);
        let calibration = calibrate_remote_rate(
            capability,
            scene_state,
            width,
            height,
            &mut absolute,
            accum,
            cancel,
        );
        *samples_done = absolute.saturating_sub(sample_offset);
        calibration
    } else {
        // Only reachable via the `RemoteOnly` bypass above, when the ENTIRE sample
        // budget is smaller than one calibration probe. Spending any of that tiny
        // budget on a throwaway timing probe would waste a disproportionate share of
        // it, and the guessed rate barely matters -- `SampleCursor::claim`'s clamp
        // sizes the first (only) chunk to the whole remainder regardless.
        RemoteCalibration::Ready(DEFAULT_REMOTE_RATE_GUESS)
    };
    match calibration {
        RemoteCalibration::Ready(rate) => {
            carry.remote_rate = Some(rate);
            Ok(Some(rate))
        }
        RemoteCalibration::Cancelled => Ok(None),
        RemoteCalibration::Failed(message) => {
            let note = format!("Remote worker rejected this export ({message}).");
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(note);
            }
            *pending_note = Some(format!("{note} Rendering locally only."));
            Ok(None)
        }
        RemoteCalibration::Short { done, expected } => {
            let note = format!(
                "Remote worker's calibration probe only completed {done} of {expected} \
                 samples before ending."
            );
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(note);
            }
            *pending_note = Some(format!("{note} Rendering locally only."));
            Ok(None)
        }
        RemoteCalibration::Partial {
            rate,
            done,
            expected,
        } => {
            // Enough completed to size a first chunk from; the lane's own failure
            // handling (pause-and-retry, never a permanent write-off) takes it from
            // here.
            carry.remote_rate = Some(rate);
            *pending_note = Some(format!(
                "Remote worker's calibration probe completed {done} of {expected} \
                 samples before ending early -- still using it, and retrying if it \
                 drops out again."
            ));
            Ok(Some(rate))
        }
    }
}

/// A `render_accumulation` call's fixed inputs, shared by its phases.
pub(super) struct RenderJob<'a> {
    /// The scene being rendered.
    pub(super) scene: &'a SceneSnapshot,
    /// The camera yaw this call renders from.
    pub(super) cam_yaw: f32,
    /// The camera pitch this call renders from.
    pub(super) cam_pitch: f32,
    /// Output width in pixels.
    pub(super) width: u32,
    /// Output height in pixels.
    pub(super) height: u32,
    /// Samples per pixel to accumulate.
    pub(super) samples_per_pixel: u32,
    /// The absolute index of the first sample (0 for an ordinary render): the render traces
    /// absolute samples `sample_offset..sample_offset + samples_per_pixel`.
    pub(super) sample_offset: u32,
    /// Which engines may render.
    pub(super) compute_target: ComputeTarget,
    /// The configured remote worker, if any.
    pub(super) worker: Option<&'a WorkerSettings>,
    /// Raised to abandon the render.
    pub(super) cancel: &'a AtomicBool,
}

/// What the remote lane needs for one call: the worker's capability, the scene as it is
/// sent and the starting throughput estimate. All `None` when no remote lane runs.
struct RemoteLanePlan {
    /// The worker's capability, once it is in play.
    capability: Option<RemoteCapability>,
    /// The scene state sent to the worker.
    scene_state: Option<SceneState>,
    /// `Some(rate)` once a lane is worth running this call -- either freshly
    /// calibrated, or carried forward from a prior call on the same carry.
    rate: Option<f64>,
}

impl RemoteLanePlan {
    /// The borrowed handles the concurrent phase runs the remote lane with; `None` when
    /// no remote lane is in play.
    fn lane<'a>(&'a self, progress: Option<&'a RemoteProgress>) -> Option<RemoteLane<'a>> {
        let rate = self.rate?;
        Some(RemoteLane {
            rate,
            capability: self
                .capability
                .as_ref()
                .expect("remote_rate implies remote_capability"),
            scene_state: self
                .scene_state
                .as_ref()
                .expect("remote_rate implies scene_state"),
            progress: progress.expect("remote_rate implies remote_progress"),
        })
    }
}

/// The remote lane's borrowed inputs for the concurrent phase.
#[derive(Clone, Copy)]
struct RemoteLane<'a> {
    /// The lane's starting throughput estimate (samples/sec).
    rate: f64,
    /// The worker's capability.
    capability: &'a RemoteCapability,
    /// The scene state sent to the worker.
    scene_state: &'a SceneState,
    /// Where the lane merges its finished chunks.
    progress: &'a RemoteProgress,
}

/// What the concurrent phase's local and remote lanes share.
struct LaneSetup<'a, 'b> {
    /// The scene/camera/environment context the local batches render with.
    ctx: &'a ExportCtx<'b>,
    /// The claim point both lanes draw samples from.
    cursor: SampleCursor,
    /// How many samples the local lane claims at a time.
    local_claim_size: u32,
    /// `--progress-raw` without a remote lane: the samples per pixel done before the
    /// local lane starts. Each claim is then capped to end on the next snapshot point.
    snapshot_base: Option<u32>,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// Raised to abandon the render.
    cancel: &'a AtomicBool,
    /// `true` only for `ComputeTarget::Both` -- gates whether a remote chunk's
    /// unfinished remainder is handed to the cursor for local to pick up, or fails the
    /// render outright. `RemoteOnly` never falls back: no local lane runs under it.
    fallback_to_local: bool,
    /// `false` only for `RemoteOnly` -- local claims nothing and the calling thread
    /// just polls for progress while remote's lane runs.
    run_local: bool,
}

impl LaneSetup<'_, '_> {
    /// The local lane's next claim, `traced` samples per pixel into its own share: the
    /// chunk size, capped at the next snapshot point when the render takes snapshots.
    fn claim_size(&self, traced: u32) -> u32 {
        self.snapshot_base.map_or(self.local_claim_size, |base| {
            self.local_claim_size
                .min(snapshot_claim_cap(base.saturating_add(traced)))
        })
    }
}

/// `Err(Cancelled)` once `cancel` is set.
fn check_not_cancelled(cancel: &AtomicBool) -> Result<(), AccumulationOutcome> {
    if cancel.load(Ordering::Relaxed) {
        Err(AccumulationOutcome::Cancelled)
    } else {
        Ok(())
    }
}

/// Resolves the remote lane's availability and starting throughput estimate for this
/// call (remote availability, then remote calibration), spending any calibration
/// samples from `job`'s budget into `accum`/`samples_done`. `Err` carries the message
/// that fails the whole render (`RemoteOnly` only).
fn prepare_remote_lane(
    job: &RenderJob<'_>,
    carry: &mut AccumulationCarry,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    pending_note: &mut Option<String>,
) -> Result<RemoteLanePlan, String> {
    // ---- Remote availability --------------------------------------------------------
    let capability = resolve_remote_capability(
        carry,
        job.compute_target,
        job.worker,
        job.scene,
        job.width,
        job.height,
        pending_note,
    )?;
    let scene_state = capability.as_ref().map(|_| {
        remote::scene_state_from_snapshot(
            job.scene,
            job.width,
            job.height,
            job.cam_yaw,
            job.cam_pitch,
        )
    });

    // ---- Remote calibration: an initial throughput estimate, not a fixed split ----
    // `Some(rate)` once a lane is worth running this call -- either freshly
    // calibrated, or carried forward from a prior call on the same `carry`. `None`
    // means no remote lane this call: declined outright (handled above), too little
    // budget left, or a fresh probe failed/was cancelled/ran short.
    let rate: Option<f64> = if let (Some(cap), Some(state)) = (&capability, &scene_state) {
        resolve_remote_rate(
            carry,
            job.compute_target,
            cap,
            state,
            job.width,
            job.height,
            job.samples_per_pixel,
            job.sample_offset,
            samples_done,
            accum,
            job.cancel,
            pending_note,
        )?
    } else {
        None
    };
    Ok(RemoteLanePlan {
        capability,
        scene_state,
        rate,
    })
}

/// Runs the concurrent phase: the local lane (unless `RemoteOnly`) and, if `remote` is
/// in play, the remote lane on its own thread, both claiming from `setup.cursor`.
/// Returns what the remote lane ended with (nothing without one).
fn run_lanes(
    setup: &LaneSetup<'_, '_>,
    remote_lane: Option<RemoteLane<'_>>,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    mut on_local_batch: impl FnMut(u32, &[Vec3], &[Vec3]),
) -> remote::RemoteLaneOutcome {
    let cursor = &setup.cursor;
    let cancel = setup.cancel;
    let Some(RemoteLane {
        rate,
        capability,
        scene_state,
        progress,
    }) = remote_lane
    else {
        // No remote lane in play at all -- an already-`true` "remote lane done" flag
        // collapses `run_local_batches`' stop condition to "stop once the cursor is
        // empty", with no separate single-engine code path to keep in sync.
        let no_remote_lane = AtomicBool::new(true);
        run_local_batches(
            setup.ctx,
            cursor,
            |traced| setup.claim_size(traced),
            &no_remote_lane,
            gpu_frac,
            accum,
            gpu_accum,
            cancel,
            on_local_batch,
        );
        return remote::RemoteLaneOutcome {
            fatal: None,
            final_rate: None,
        };
    };
    let (width, height, fallback_to_local) = (setup.width, setup.height, setup.fallback_to_local);
    // Set by `run_remote_lane` as the LAST thing it does, once its claim loop has
    // permanently ended -- `run_local_batches` depends on that ordering to know
    // it's safe to stop waiting for a possible late `return_to_local`.
    let remote_lane_done = AtomicBool::new(false);

    thread::scope(|s| {
        let remote_thread = s.spawn(|| {
            run_remote_lane(
                cursor,
                capability,
                scene_state,
                width,
                height,
                rate,
                progress,
                fallback_to_local,
                cancel,
                &remote_lane_done,
            )
        });

        if setup.run_local {
            run_local_batches(
                setup.ctx,
                cursor,
                |traced| setup.claim_size(traced),
                &remote_lane_done,
                gpu_frac,
                accum,
                gpu_accum,
                cancel,
                on_local_batch,
            );
        } else {
            // `ComputeTarget::RemoteOnly` -- nothing for local to claim, so poll
            // for progress on a light timer instead of leaving the progress bar
            // frozen until remote's lane finishes.
            while !remote_thread.is_finished() {
                on_local_batch(0, accum, gpu_accum);
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }

        remote_thread
            .join()
            .unwrap_or_else(|_| remote::RemoteLaneOutcome {
                fatal: Some("remote render worker thread panicked".to_string()),
                final_rate: None,
            })
    })
}

/// Merges remote's and the GPU's separate accumulations into `accum`, each exactly once,
/// now that the concurrent phase has ended.
fn merge_lane_buffers(accum: &mut [Vec3], remote: Option<RemoteProgress>, gpu_accum: &[Vec3]) {
    // Merge remote's full contribution exactly once, now that the concurrent phase has
    // ended -- merging earlier would race local's CPU threads writing the same pixel
    // indices. Mirrors the GPU buffer's own single end-of-render merge just below.
    if let Some(progress) = remote {
        let remote_buf = progress.into_buffer();
        for (dst, src) in accum.iter_mut().zip(&remote_buf) {
            *dst += *src;
        }
    }

    // Merge the GPU's separate accumulation exactly once. A pure-CPU render leaves
    // `gpu_accum` all zero, making this a no-op.
    for (px, gpu_px) in accum.iter_mut().zip(gpu_accum) {
        *px += *gpu_px;
    }
}

/// `pending` with the path cache's one progress note (its size line, or why the render has none)
/// in front, when the context has one left.
fn with_cache_note(ctx: &ExportCtx<'_>, pending: Option<String>) -> Option<String> {
    match (ctx.take_cache_note(), pending) {
        (Some(cache_note), Some(earlier)) => Some(format!("{cache_note} {earlier}")),
        (cache_note, earlier) => cache_note.or(earlier),
    }
}

/// The hybrid CPU+GPU split the concurrent phase starts from.
///
/// That is the split carried from a prior call (`carried`), or else a fresh timing probe with its
/// confirmation batch. The probe's samples are real: `samples_done` moves past them, into
/// `buffers` (`(accum, gpu_accum)`). A render that takes snapshots (`snapshots`) gets the probe
/// alone: its first claims end on the snapshot points, and a confirmation batch would run past
/// the first one. `None` is one engine per batch: the Ray-Traced engine, a render with a path
/// cache (`CacheRun::accumulate` runs one engine per batch), any compute choice but `CpuGpu`, or
/// too few samples left to split. `Some(1.0)` is the GPU alone, chosen because the GPU does
/// nearly all of the work (`batch::calibrate_split`).
fn starting_split(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    carried: Option<f64>,
    snapshots: bool,
    samples_done: &mut u32,
    buffers: (&mut [Vec3], &mut [Vec3]),
) -> Option<f64> {
    if ctx.scene.engine.is_ray()
        || ctx.path_cache.is_some()
        || local_compute != LocalComputeTarget::CpuGpu
        || job.samples_per_pixel - *samples_done < HYBRID_MIN_SPP
    {
        return None;
    }
    let (accum, gpu_accum) = buffers;
    carried.or_else(|| {
        // `samples_done` counts samples; the probe wants the absolute index it starts at.
        let mut absolute = absolute_sample(job.sample_offset, *samples_done);
        let available = if snapshots {
            0
        } else {
            job.samples_per_pixel - *samples_done
        };
        let frac =
            calibrate_split_confirmed(ctx, &mut absolute, available, accum, gpu_accum, job.cancel);
        *samples_done = absolute.saturating_sub(job.sample_offset);
        frac
    })
}

/// The progress report of one tick of the ordinary render: `total_done` of `samples_per_pixel`
/// samples, with the tick's thumbnail and note.
const fn tick_report(
    total_done: u32,
    samples_per_pixel: u32,
    preview: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    note: Option<String>,
) -> ExportProgress {
    ExportProgress {
        fraction: total_done as f32 / samples_per_pixel as f32,
        samples_done: total_done,
        samples_total: samples_per_pixel,
        preview,
        note,
        #[cfg(feature = "beam")]
        beam: None,
    }
}

/// The body of [`super::render_accumulation`], run once [`super::with_export_ctx`] built `ctx`.
/// `Err` is an early end of the render (cancelled or failed); `Ok` the finished buffer.
pub(super) fn render_on_ctx(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    mut report_progress: impl FnMut(ExportProgress),
) -> Result<Accumulation, AccumulationOutcome> {
    let (width, height, samples_per_pixel) = (job.width, job.height, job.samples_per_pixel);
    let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
    let mut samples_done = 0u32;
    let mut pending_note: Option<String> = None;
    let remote_plan =
        prepare_remote_lane(job, carry, &mut samples_done, &mut accum, &mut pending_note)
            .map_err(AccumulationOutcome::Failed)?;
    // `gpu_accum` is declared here because the concurrent phase below needs it
    // regardless of whether remote ends up in play.
    let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
    check_not_cancelled(job.cancel)?;
    // The path cache's note leads the first progress tick.
    pending_note = with_cache_note(ctx, pending_note);

    // Hybrid CPU+GPU export: the GPU and all CPU cores trace DISJOINT sample ranges of
    // the same frame concurrently, and radiance sums merge -- the same
    // disjoint-sample-range convention `pleochroma::renderer::gpu::hybrid` uses. The
    // GPU owns its own accumulation buffer, summed into `accum` once at the end, so
    // the two engines never write one buffer concurrently. `gpu_frac == None` reads as
    // "single engine, no split": under `Gpu` the backend accepts every batch; under
    // `Cpu` the disabled backend declines every batch and the CPU tracer carries it.
    // Only `CpuGpu` measures a split and hands the two engines disjoint ranges. A
    // seed already carried from a prior call skips the timing probe entirely and
    // starts the concurrent phase from it directly -- `hybrid_batch`'s own per-batch
    // EMA keeps adapting it exactly as if it had just been measured.
    let mut gpu_frac = starting_split(
        ctx,
        job,
        local_compute,
        carry.hybrid_frac,
        carry.taps.wants_snapshots(),
        &mut samples_done,
        (&mut accum, &mut gpu_accum),
    );
    check_not_cancelled(job.cancel)?;

    // ---- The concurrent phase: local and (if in play) remote share one cursor -----
    // Both calibration probes above are real, already-counted work -- `samples_done`
    // marks where the shared claim point for the REST of the budget begins. A single
    // atomic claim point both lanes draw from concurrently avoids handing a lane a
    // fixed slice that finishes early with nothing further to claim.
    let setup = LaneSetup {
        ctx,
        cursor: SampleCursor::new(
            absolute_sample(job.sample_offset, samples_done),
            absolute_sample(job.sample_offset, samples_per_pixel),
        ),
        local_claim_size: local_chunk_size(samples_per_pixel),
        // Snapshots come from the local buffers only, so a remote lane takes none.
        snapshot_base: (carry.taps.wants_snapshots() && remote_plan.rate.is_none())
            .then_some(samples_done),
        width,
        height,
        cancel: job.cancel,
        fallback_to_local: matches!(job.compute_target, ComputeTarget::Both),
        run_local: !matches!(job.compute_target, ComputeTarget::RemoteOnly),
    };
    let mut preview_throttle = PreviewThrottle::new();
    let remote_progress = remote_plan.rate.map(|_| RemoteProgress::new(accum.len()));
    // `--progress-raw`: snapshots of the linear image at chunk boundaries. Taken from the
    // local buffers only, so a render with a remote lane in play takes none (the command
    // line refuses the combination before it starts).
    let taps = carry.taps.clone();
    let mut snapshots = SnapshotSchedule::new();
    // The samples traced before the lanes start (the hybrid probe and its confirmation batch, a
    // remote calibration) are in the buffers already: they count toward the progress, the
    // preview's sample count and the snapshot points.
    let lanes_start = samples_done;

    // Reports one progress tick. `local_done` is the LOCAL lane's own running total
    // (not a cursor position, which can run ahead of completed work). Adding remote's
    // independently-tracked total and `lanes_start` gives the render's true combined
    // completion, correct by construction since the shared cursor can never double-count a
    // sample. Without a remote lane (`remote_progress` is `None`) it is the local count alone.
    let mut on_local_batch = |local_done: u32, local_accum: &[Vec3], local_gpu_accum: &[Vec3]| {
        let (remote_done_so_far, remote_preview) = remote_progress
            .as_ref()
            .map_or((0, None), |p| (p.samples_done(), Some(p.preview_buffer())));
        let total_done = (lanes_start + local_done + remote_done_so_far).min(samples_per_pixel);
        let preview = preview_throttle.maybe_generate(
            width,
            height,
            local_accum,
            local_gpu_accum,
            remote_preview.as_deref(),
            total_done,
        );
        // A pre-dispatch decline/calibration note takes priority on the first tick;
        // once drained, later ticks surface whatever `run_remote_lane` itself queued
        // (a recovered mid-export chunk failure, or "giving up on remote").
        let note = pending_note
            .take()
            .or_else(|| remote_progress.as_ref().and_then(RemoteProgress::take_note));
        report_progress(tick_report(total_done, samples_per_pixel, preview, note));
        if local_done > 0 && remote_progress.is_none() && taps.wants_snapshots() {
            let done = lanes_start.saturating_add(local_done);
            if snapshots.due(done) {
                taps.publish(
                    LinearImageKind::Snapshot,
                    (width, height),
                    done,
                    &summed(local_accum, local_gpu_accum),
                );
            }
        }
    };

    // `outcome.fatal` is `Some` only when `run_remote_lane` ended in a state that must
    // fail the whole render (`RemoteOnly` only).
    let outcome = run_lanes(
        &setup,
        remote_plan.lane(remote_progress.as_ref()),
        &mut gpu_frac,
        &mut accum,
        &mut gpu_accum,
        &mut on_local_batch,
    );

    // Carries this call's final calibration state forward for the NEXT call on the
    // same `carry` (a no-op for `run_export`'s single-call `AccumulationCarry`, which
    // is dropped right after). `gpu_frac` is written back even when it ended `None`
    // (a declined/failed GPU): see `AccumulationCarry::hybrid_frac`'s own doc comment
    // on why that means "retry next frame", not "give up for the sweep". The remote
    // rate starts at `remote_plan.rate` (the seed this call started from) so a call that
    // never ran a remote lane still leaves `carry.remote_rate` unchanged rather than
    // clobbering it with `None`.
    carry.hybrid_frac = gpu_frac;
    carry.remote_rate = outcome
        .final_rate
        .or(remote_plan.rate)
        .or(carry.remote_rate);

    // A user-initiated cancel discards the whole render, remote's partial contribution
    // included. `run_remote_batch` already sent `CANCEL` to the worker for whatever
    // chunk was in flight once it observed `cancel`, so this is just bookkeeping
    // catching up.
    check_not_cancelled(job.cancel)?;

    // `RemoteOnly` only: fails the whole render outright rather than returning the
    // partial `accum`, since no local lane ever ran to pick up a failed chunk's
    // remainder.
    if let Some(message) = outcome.fatal {
        return Err(AccumulationOutcome::Failed(message));
    }

    merge_lane_buffers(&mut accum, remote_progress, &gpu_accum);
    check_not_cancelled(job.cancel)?;

    Ok(Accumulation {
        accum,
        samples_per_pixel,
        #[cfg(feature = "adaptive-sampling")]
        adaptive: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no offset the absolute index is the count. An offset shifts every index that reaches
    /// the accumulators (the cursor, the calibration probes) alike, and a sum past the largest
    /// index saturates instead of wrapping.
    #[test]
    fn the_absolute_sample_index_is_the_count_without_an_offset() {
        for done in [0, 1, 3, 4096, u32::MAX] {
            assert_eq!(absolute_sample(0, done), done);
        }
        assert_eq!(absolute_sample(16_384, 0), 16_384);
        assert_eq!(absolute_sample(16_384, 5), 16_389);
        assert_eq!(absolute_sample(u32::MAX, 5), u32::MAX);
    }

    /// A calibration probe that advances the absolute index by `n` advances the sample count
    /// by `n`, whatever the offset.
    #[test]
    fn a_probe_advances_the_count_by_the_samples_it_traced() {
        for offset in [0, 7, 16_384] {
            let before = 2;
            let mut absolute = absolute_sample(offset, before);
            absolute += 3;
            assert_eq!(absolute.saturating_sub(offset), before + 3);
        }
    }
}
