//! The adaptive-sampling render of the export worker: [`render_adaptive_on_ctx`] runs an
//! [`AdaptiveRun`] over one frame instead of the sample-cursor loop.
//!
//! # What it does
//!
//! Each pass traces the tiles that are still live, with a sample count that grows (see
//! `AdaptiveRun::with_growing_passes`). The pass goes into two full-frame buffers: the sum of
//! every sample, and the sum of the odd-index samples (half-buffer B). Between passes the run
//! compares the two halves of each live tile and stops the tiles that are clean enough. The
//! finished frame is each pixel's sum divided by its own sample count, then goes through the
//! usual tone map, PNG and ICC path. [`Accumulation::accum`] holds means with a count of 1,
//! so `tonemap_accumulation` divides by one.
//!
//! # One engine per tile
//!
//! On this computer a share of a pass runs on the GPU when it is available
//! ([`ExportCtx::try_gpu_live`]), otherwise on the CPU ([`cpu_pass::trace_live_pass`], threaded
//! over the live tiles). The two do not split a share, and the hybrid CPU+GPU split of the
//! ordinary export is not used. When the GPU declines a pass (no adapter, a scene it does not
//! trace, a lost device, a retired backend) the render stays on the CPU for the rest of the
//! frame. The next frame of a video tries the GPU again.
//!
//! # A remote worker
//!
//! With `Local + Remote` or `Remote Only` and a worker that can take part
//! ([`remote_lane::open`]), every pass's live tiles are split **by tile** between this computer
//! and the worker ([`assignment::TileAssignment`], sticky across passes). The shares are traced
//! at the same time, and the worker's pixels are added into the same two buffers
//! ([`split_pass::PassEngines`]). Convergence is still decided here, between passes, from the
//! merged frame. A worker that fails hands its tiles back: they are traced here and the worker
//! gets no more. A scene with an HDR environment stays on this computer, since a live pass does
//! not carry the map.
//!
//! # GPU-resident convergence (opt in)
//!
//! With `RenderTaps::gpu_convergence` (`--gpu-convergence`) a render that stays on this
//! computer's GPU keeps the frame's sums on the GPU for the whole run. It reads back one value
//! per live tile per pass, and the image once ([`resident_run`]). Remote and hybrid passes keep
//! the host path, and so does any render the GPU declines, at the start or midway. The frame is
//! then rendered again from sample zero.
//!
//! # Progress
//!
//! Progress is `samples_done / max_spp`, the worst case. A frame whose tiles all converge early
//! ends below that, and its last report jumps to 1.
//!
//! # Scale of the convergence check
//!
//! The check tone-maps the means at scale 1. The renderer's values already hold the job's
//! exposure (it sits in the lighting rig), and the export tone map applies no exposure of its
//! own: `tonemap_accumulation` only divides by the sample count.

pub(super) mod assignment;
mod cpu_pass;
mod remote_lane;
mod resident_run;
#[cfg(test)]
mod resident_tests;
mod split_pass;
#[cfg(test)]
mod tests;

use super::{
    super::{
        taps::{LinearImageKind, RenderTaps, SnapshotSchedule},
        types::{Accumulation, AccumulationOutcome, AdaptiveStats},
    },
    lanes::RenderJob,
};
use crate::{
    bridge::export_thread::{ExportProgress, batch::ExportCtx},
    settings::LocalComputeTarget,
};
use glam::Vec3;
use pleochroma::renderer::adaptive::{AdaptiveRun, AdaptiveSettings};
use remote_lane::RemoteLane;
use resident_run::ResidentOutcome;
use split_pass::PassEngines;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

/// The scale of the convergence check's tone map: 1, see the module doc.
const CHECK_SCALE: f32 = 1.0;

/// Renders `job`'s frame with adaptive sampling, on this computer and, when the job asks for
/// one, a remote worker.
///
/// `requested` are the settings asked for. The job's samples per pixel (`job.samples_per_pixel`)
/// is the cap, so `requested.max_spp` is replaced by it. `taps` supplies the sinks for
/// `--raw-out` and `--progress-raw`; the absolute sample offset is `job.sample_offset`.
///
/// `Err` is an early end: cancelled, or failed (settings that break a rule, the Ray Traced
/// engine, or a `RemoteOnly` render whose worker is missing or fails).
pub(super) fn render_adaptive_on_ctx(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    requested: AdaptiveSettings,
    taps: &RenderTaps,
    report_progress: impl FnMut(ExportProgress),
) -> Result<Accumulation, AccumulationOutcome> {
    refuse_unsupported(ctx)?;
    // A worker builds a path cache only for a kept cache; neither side uses a one-shot cache.
    let lane = remote_lane::open(job, taps.path_cache.is_some_and(|cache| cache.persist))?;
    render_adaptive_with_lane(
        ctx,
        job,
        local_compute,
        requested,
        taps,
        lane,
        report_progress,
    )
}

/// [`render_adaptive_on_ctx`] with its remote lane already opened (the seam the tests use to put
/// a scripted worker in place of a socket).
fn render_adaptive_with_lane(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    requested: AdaptiveSettings,
    taps: &RenderTaps,
    mut lane: RemoteLane<'_>,
    mut report_progress: impl FnMut(ExportProgress),
) -> Result<Accumulation, AccumulationOutcome> {
    let (width, height, cancel) = (job.width, job.height, job.cancel);
    let settings = AdaptiveSettings {
        max_spp: job.samples_per_pixel,
        ..requested
    };
    let max_spp = settings.max_spp;
    let mut run = AdaptiveRun::new(settings, width, height)
        .map_err(|e| AccumulationOutcome::Failed(format!("Adaptive sampling: {e}.")))?
        .with_sample_offset(job.sample_offset)
        .with_growing_passes();

    // The opt-in GPU-resident path (`--gpu-convergence`). If it declines, the frame is rendered
    // again below from sample zero.
    let mut resident_note = None;
    if resident_wanted(ctx, taps, local_compute, &lane) {
        match resident_run::render_resident(ctx, job, settings, &mut report_progress) {
            ResidentOutcome::Done(image) => return Ok(image),
            ResidentOutcome::Cancelled => return Err(AccumulationOutcome::Cancelled),
            ResidentOutcome::Declined(why) => resident_note = Some(why.to_owned()),
        }
    }

    let pixels = width as usize * height as usize;
    let mut sum = vec![Vec3::ZERO; pixels];
    let mut half_b = vec![Vec3::ZERO; pixels];
    let mut snapshots = SnapshotSchedule::new();
    let first_note = opening_note(ctx.take_cache_note(), lane.note.take(), resident_note);
    // `Cpu` never offers the GPU a pass; otherwise the first decline ends the offers.
    let mut engines = PassEngines::new(
        ctx,
        cancel,
        local_compute != LocalComputeTarget::Cpu,
        lane,
        run.map().states().len(),
    );
    if first_note.is_some() {
        report_progress(ExportProgress {
            fraction: 0.0,
            samples_done: 0,
            samples_total: max_spp,
            preview: None,
            note: first_note,
            #[cfg(feature = "beam")]
            beam: None,
        });
    }

    let (mut trace_time, mut check_time, mut passes) = (Duration::ZERO, Duration::ZERO, 0_u32);
    while let Some(pass) = run.next_pass() {
        if cancel.load(Ordering::Relaxed) {
            return Err(AccumulationOutcome::Cancelled);
        }
        // The live tiles are split between the engines, traced at the same time and merged into
        // `sum` and `half_b`; only then is convergence decided, here and nowhere else.
        let traced = Instant::now();
        let lost_worker = engines.trace(&pass, &mut sum, &mut half_b)?;
        let checked = Instant::now();
        run.finish_pass(&sum, &half_b, CHECK_SCALE);
        trace_time += checked - traced;
        check_time += checked.elapsed();
        passes += 1;

        let done = run.samples_done();
        report_progress(ExportProgress {
            fraction: done as f32 / max_spp as f32,
            samples_done: done,
            samples_total: max_spp,
            preview: None,
            note: lost_worker,
            #[cfg(feature = "beam")]
            beam: None,
        });
        // A snapshot after the pass that reaches each power of two of `samples_done`; the
        // counts of tiles that stopped stay at their final value.
        if taps.wants_snapshots() && snapshots.due(done) {
            taps.publish_counted(
                LinearImageKind::Snapshot,
                (width, height),
                done,
                &run.resolve(&sum),
                &run.pixel_counts(),
            );
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(AccumulationOutcome::Cancelled);
    }
    tracing::debug!(
        target: "pleochroma::adaptive",
        passes,
        trace_seconds = trace_time.as_secs_f64(),
        check_seconds = check_time.as_secs_f64(),
        "adaptive passes finished"
    );

    let stats = stats_of(&run, max_spp);
    report_progress(ExportProgress {
        fraction: 1.0,
        samples_done: stats.samples_done,
        samples_total: max_spp,
        preview: None,
        note: Some(stats.note(width, height)),
        #[cfg(feature = "beam")]
        beam: None,
    });
    Ok(Accumulation {
        accum: run.resolve(&sum),
        // The buffer holds means: the tone map divides by one.
        samples_per_pixel: 1,
        adaptive: Some(stats),
    })
}

/// The note shown before the first pass: the path cache's line, why there is no remote lane
/// although one was asked for, and why the resident path was declined. Parts are joined with
/// spaces. `None` when there is none of the three.
fn opening_note(
    cache: Option<String>,
    lane: Option<String>,
    resident: Option<String>,
) -> Option<String> {
    let parts: Vec<String> = [cache, lane, resident].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Whether the GPU-resident convergence path is asked for and possible for this render.
///
/// Possible only when every pass runs on this computer's GPU: the GPU is allowed and not
/// retired, there is no remote worker (its pixels are merged on the host), and no snapshot is
/// wanted (it needs the sums mid-run).
fn resident_wanted(
    ctx: &ExportCtx<'_>,
    taps: &RenderTaps,
    local_compute: LocalComputeTarget,
    lane: &RemoteLane<'_>,
) -> bool {
    taps.gpu_convergence
        // The resident run has no cached pass: a render with a path cache keeps the host loop.
        && ctx.path_cache.is_none()
        && !taps.wants_snapshots()
        && local_compute != LocalComputeTarget::Cpu
        && lane.engines.is_empty()
        && !ctx.gpu_retired.load(Ordering::Relaxed)
}

/// Why this render cannot be adaptive, if it cannot: the Ray Traced engine has no noise to
/// converge.
fn refuse_unsupported(ctx: &ExportCtx<'_>) -> Result<(), AccumulationOutcome> {
    if ctx.scene.engine.is_ray() {
        return Err(AccumulationOutcome::Failed(
            "Adaptive sampling is for the path-traced renderer: the Ray Traced engine's taps are \
             a fixed set, with no noise to converge."
                .to_string(),
        ));
    }
    Ok(())
}

/// The counts and figures of a finished run.
fn stats_of(run: &AdaptiveRun, max_spp: u32) -> AdaptiveStats {
    let states = run.map().states();
    AdaptiveStats {
        counts: run.pixel_counts(),
        samples_done: run.samples_done(),
        samples_traced: run.samples_traced(),
        max_spp,
        tiles_total: states.len(),
        tiles_converged: states.iter().filter(|tile| !tile.live).count(),
    }
}
