//! One adaptive pass over several engines: [`PassEngines`] splits the live tiles of a [`Pass`]
//! between this computer and the remote workers, traces the shares at the same time, and merges
//! them into the frame's two buffers.
//!
//! # Split
//!
//! By tile, through [`TileAssignment`]: sticky across passes (a tile keeps its engine while it
//! stays live), moved only when its engine fails.
//!
//! # Merge
//!
//! Every engine returns `(pass total, odd-sample total)` per live pixel, each formed from zero in
//! ascending sample order, and the pair is **added** to the frame's `sum` and `half_b` -- exactly
//! the step the local passes take. This computer adds its own tiles in place while the remote
//! replies are in flight; a reply is added when it arrives at the join. Each pixel belongs to one
//! tile and each tile to one engine, so the order of the additions does not matter and the frame
//! equals a local pass's. Convergence is decided afterwards, from the merged frame, by the caller
//! (`AdaptiveRun::finish_pass`), never on a worker.
//!
//! # A remote failure
//!
//! A worker that errors, goes silent, or returns pixels that do not fit what was asked is taken
//! out ([`TileAssignment::fail`]) and its tiles of **this** pass are traced here, so the pass
//! completes; its tiles then belong to the next living engine for the rest of the render. With no
//! engine of this computer to fall back on (`RemoteOnly`) the render fails instead.

use super::{
    super::super::types::AccumulationOutcome, assignment::TileAssignment, cpu_pass,
    remote_lane::RemoteLane,
};
use crate::bridge::export_thread::{batch::ExportCtx, remote::LivePassFailure};
use glam::Vec3;
use pleochroma::renderer::{
    adaptive::{LiveTiles, Pass},
    gpu_backend::GpuAccumulate,
};
use pleochroma_net::messages::scatter_live;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

/// Traces `pass` here: on the GPU while `gpu_usable`, otherwise (or once the GPU declines) on the
/// CPU. Returns `true` when `cancel` ended it.
pub(super) fn trace_here(
    ctx: &ExportCtx<'_>,
    gpu_usable: &mut bool,
    pass: &Pass,
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
    cancel: &AtomicBool,
) -> bool {
    if pass.live.is_empty() {
        return false;
    }
    if *gpu_usable && !ctx.gpu_retired.load(Ordering::Relaxed) {
        match ctx.try_gpu_live(pass, sum, half_b, cancel) {
            GpuAccumulate::Done => return false,
            GpuAccumulate::Cancelled => return true,
            GpuAccumulate::Declined => *gpu_usable = false,
        }
    }
    cpu_pass::trace_live_pass(ctx, pass, sum, half_b, cancel)
}

/// The engines of one adaptive render and who traces which tile.
pub(super) struct PassEngines<'a, 'c, 'e> {
    ctx: &'a ExportCtx<'c>,
    cancel: &'a AtomicBool,
    /// Whether this computer's GPU may still be offered a share.
    gpu_usable: bool,
    /// Whether this computer takes tiles (engine 0 when it does).
    local: bool,
    assignment: TileAssignment,
    lane: RemoteLane<'e>,
}

impl<'a, 'c, 'e> PassEngines<'a, 'c, 'e> {
    /// The engines for a frame of `tiles` tiles: this computer first (unless the lane says it
    /// sits out), then the lane's workers. `gpu_usable` is whether this computer's GPU is offered
    /// its shares.
    pub(super) fn new(
        ctx: &'a ExportCtx<'c>,
        cancel: &'a AtomicBool,
        gpu_usable: bool,
        lane: RemoteLane<'e>,
        tiles: usize,
    ) -> Self {
        let local = lane.local_participates();
        let engines = usize::from(local) + lane.engines.len();
        Self {
            ctx,
            cancel,
            gpu_usable,
            local,
            assignment: TileAssignment::new(tiles, engines),
            lane,
        }
    }

    /// The assignment as it stands (tests read it).
    #[cfg(test)]
    pub(super) const fn assignment(&self) -> &TileAssignment {
        &self.assignment
    }

    /// Traces every live tile of `pass` and adds the result to `sum` and `half_b`.
    ///
    /// Returns a note about a remote worker that was lost during the pass, if one was.
    ///
    /// # Errors
    ///
    /// `Cancelled` when `cancel` ended the pass; `Failed` when a remote worker failed and no
    /// engine of this computer can take over.
    pub(super) fn trace(
        &mut self,
        pass: &Pass,
        sum: &mut [Vec3],
        half_b: &mut [Vec3],
    ) -> Result<Option<String>, AccumulationOutcome> {
        let (width, height, tile) = (pass.live.width(), pass.live.height(), pass.live.tile());
        let share = |tiles: &[u32]| Pass {
            first_sample: pass.first_sample,
            spp: pass.spp,
            live: LiveTiles::from_indices(width, height, tile, tiles.to_vec()),
        };
        let parts = self.assignment.split(pass.live.indices());
        let first_remote = usize::from(self.local);
        let samples = (pass.first_sample, pass.spp);
        let (ctx, cancel, local) = (self.ctx, self.cancel, self.local);
        let scene = self.lane.scene.as_ref();
        let assignment = &self.assignment;
        let gpu_usable = &mut self.gpu_usable;
        let engines = &mut self.lane.engines;

        let mut local_cancelled = false;
        let mut replies = Vec::new();
        thread::scope(|scope| {
            let mut running = Vec::new();
            if let Some(scene) = scene {
                for (index, engine) in engines.iter_mut().enumerate() {
                    let slot = first_remote + index;
                    let tiles = parts.get(slot).map_or(&[][..], Vec::as_slice);
                    if tiles.is_empty() || !assignment.is_alive(slot) {
                        continue;
                    }
                    let live = share(tiles).live;
                    running.push((
                        slot,
                        scope.spawn(move || {
                            let result = engine.trace(scene, samples, &live, cancel);
                            (live, result)
                        }),
                    ));
                }
            }
            // This computer's share runs here while the workers trace theirs.
            if local && let Some(tiles) = parts.first().filter(|tiles| !tiles.is_empty()) {
                local_cancelled = trace_here(ctx, gpu_usable, &share(tiles), sum, half_b, cancel);
            }
            for (slot, handle) in running {
                let reply = handle.join().unwrap_or_else(|_| {
                    let tiles = parts.get(slot).map_or(&[][..], Vec::as_slice);
                    (
                        share(tiles).live,
                        Err(LivePassFailure::Broken(
                            "the remote lane stopped unexpectedly".to_owned(),
                        )),
                    )
                });
                replies.push((slot, reply));
            }
        });
        if local_cancelled {
            return Err(AccumulationOutcome::Cancelled);
        }

        let mut note: Option<String> = None;
        for (slot, (live, result)) in replies {
            let merged = result.and_then(|pixels| {
                if scatter_live(&live, &pixels, sum, half_b) {
                    Ok(())
                } else {
                    Err(LivePassFailure::Broken(
                        "the worker's pixels do not fit the tiles it was asked for".to_owned(),
                    ))
                }
            });
            match merged {
                Ok(()) => {}
                Err(LivePassFailure::Cancelled) => return Err(AccumulationOutcome::Cancelled),
                Err(failure) => {
                    if !self.local {
                        return Err(AccumulationOutcome::Failed(format!(
                            "The remote worker failed: {failure}"
                        )));
                    }
                    let heir = self.assignment.fail(slot);
                    let missing = Pass {
                        first_sample: pass.first_sample,
                        spp: pass.spp,
                        live,
                    };
                    if trace_here(ctx, &mut self.gpu_usable, &missing, sum, half_b, cancel) {
                        return Err(AccumulationOutcome::Cancelled);
                    }
                    let who = if heir == Some(0) {
                        "this computer traces its tiles from here on"
                    } else {
                        "its tiles move to the next worker"
                    };
                    let said = format!("The remote worker stopped ({failure}); {who}.");
                    note = Some(match note {
                        Some(earlier) => format!("{earlier} {said}"),
                        None => said,
                    });
                }
            }
        }
        Ok(note)
    }
}
