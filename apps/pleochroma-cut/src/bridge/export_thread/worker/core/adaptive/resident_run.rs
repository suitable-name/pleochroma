//! The GPU-resident convergence path of an adaptive render: [`render_resident`].
//!
//! The GPU-resident convergence path of an adaptive render, opted into with
//! `RenderTaps::gpu_convergence` (`--gpu-convergence`).
//!
//! The frame's summed XYZ and half-buffer B stay on this computer's GPU for the whole run
//! ([`pleochroma::renderer::gpu_backend::GpuBackend::try_adaptive_resident`]). A pass reads
//! back nothing. The convergence check runs on the GPU and reads back one `f32` per live tile,
//! and the image is read back once at the end. Dilation and the stop decision stay on the host
//! in [`AdaptiveRun::finish_pass_with_tile_errors`], which shares its decision code with
//! `finish_pass`.
//!
//! # When it is used
//!
//! Only when every pass is on this computer's GPU: no remote worker (its pixels are merged on
//! the host), no `--progress-raw` snapshots (they need the sums mid-run), and a GPU that is
//! allowed and not retired. When the GPU declines, at the start or midway, the caller renders
//! the frame again on the host path from sample zero. The sums lived on the GPU, so nothing of
//! the failed attempt is reused.

use super::{
    super::{super::types::Accumulation, lanes::RenderJob},
    CHECK_SCALE, stats_of,
};
use crate::bridge::export_thread::{ExportProgress, batch::ExportCtx};
use pleochroma::renderer::{
    adaptive::{AdaptiveRun, AdaptiveSettings},
    gpu_backend::GpuAccumulate,
};
use std::sync::atomic::Ordering;

/// How a resident attempt ended.
pub(super) enum ResidentOutcome {
    /// The render finished on the GPU-resident path.
    Done(Accumulation),
    /// The render was cancelled.
    Cancelled,
    /// The GPU could not carry the run; render again on the host path. Carries why, for a note.
    Declined(&'static str),
}

/// Why a resident attempt that did not start or stopped midway fell back to the host path.
const DECLINED: &str = "The GPU-resident convergence check is not available for this render \
                        here; using the standard adaptive path.";

/// Renders `job`'s frame with its sums resident on the GPU. See the module doc.
///
/// `settings` are validated and carry the render's cap as `max_spp`. No snapshots are
/// published: the caller does not take this path when they are wanted.
pub(super) fn render_resident(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    settings: AdaptiveSettings,
    report_progress: &mut impl FnMut(ExportProgress),
) -> ResidentOutcome {
    let (width, height, cancel) = (job.width, job.height, job.cancel);
    let max_spp = settings.max_spp;
    let Ok(run) = AdaptiveRun::new(settings, width, height) else {
        return ResidentOutcome::Declined(DECLINED);
    };
    let mut run = run
        .with_sample_offset(job.sample_offset)
        .with_growing_passes();
    let Some(mut gpu) = ctx.gpu.try_adaptive_resident(
        ctx.gpu_scene,
        &ctx.scene.tools,
        &ctx.surface,
        run.largest_pass(),
    ) else {
        return ResidentOutcome::Declined(DECLINED);
    };

    while let Some(pass) = run.next_pass() {
        if cancel.load(Ordering::Relaxed) {
            return ResidentOutcome::Cancelled;
        }
        match gpu.pass(&pass, cancel) {
            GpuAccumulate::Done => {}
            GpuAccumulate::Cancelled => return ResidentOutcome::Cancelled,
            GpuAccumulate::Declined => return ResidentOutcome::Declined(DECLINED),
        }
        // One f32 per live tile crosses to the host, and only when a check is due.
        let noises = match run.pending_check() {
            Some(check) => match gpu.tile_errors(&check, CHECK_SCALE, settings.metric) {
                Some(noises) => noises,
                None => return ResidentOutcome::Declined(DECLINED),
            },
            None => Vec::new(),
        };
        run.finish_pass_with_tile_errors(&noises);

        let done = run.samples_done();
        report_progress(ExportProgress {
            fraction: done as f32 / max_spp as f32,
            samples_done: done,
            samples_total: max_spp,
            preview: None,
            note: None,
            #[cfg(feature = "beam")]
            beam: None,
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return ResidentOutcome::Cancelled;
    }

    // The one full-image readback.
    let Some((sum, _half_b)) = gpu.finish() else {
        return ResidentOutcome::Declined(DECLINED);
    };
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
    ResidentOutcome::Done(Accumulation {
        accum: run.resolve(&sum),
        // The buffer holds means: the tone map divides by one.
        samples_per_pixel: 1,
        adaptive: Some(stats),
    })
}
