//! The beam-tracing fragment entry of [`GpuBackend`] (beam guide §14.7).
//!
//! It uploads a batch's leaves, shades every (leaf, pixel) fragment on the GPU, reduces the
//! fragments per pixel in fixed order and reads the sums back.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use glam::Vec3;

use super::GpuBackend;
use crate::renderer::{
    beam::{beam::PackedLeaves, primary::TileRect},
    gpu::{
        GpuContext, GpuFrameRenderer,
        beam::{
            buffers::{
                FragmentPlan, LeafBufferSet, PendingReduction, PixelCsr, ReduceScratch,
                busy_reason, leaf_binding_limit, reduction_fits, split_into_batches, sub_batch,
                submit_reduction,
            },
            engine::{DECLINE_REDUCTION_OVER_LIMIT, DECLINE_TILE_OVER_LIMIT, PixelSlots},
            fragments::{FragmentOptions, FragmentStop, dispatch_fragments_cancellable},
        },
    },
    gpu_backend::{BeamFragmentParams, GpuBeamFragments, GpuSceneRef},
};

/// A piece whose GPU work is submitted and whose readback is mapped but not yet waited for.
struct SubmittedPiece {
    csr: PixelCsr,
    reduction: PendingReduction,
}

/// Waits for `submitted`'s readback and returns its pixel sums as `(slot, XYZ)` pairs in the
/// CSR's pixel order. A pixel that no slot holds is dropped.
///
/// `width` is the frame width the CSR's pixel coordinates refer to. Takes only a cloned
/// context, never the renderer lock.
fn finish_piece(
    ctx: &GpuContext,
    (width, slots): (u32, &PixelSlots),
    submitted: SubmittedPiece,
) -> Result<Vec<(usize, Vec3)>, &'static str> {
    let SubmittedPiece { csr, reduction } = submitted;
    let sums = reduction.finish_noting(ctx).ok_or("reduction declined")?;
    if ctx.failure().is_some() || sums.len() != csr.pixel_count() {
        return Err("reduction declined");
    }
    let csr_width = csr.size.0 as usize;
    let mut pairs = Vec::new();
    for (pixel, sum) in sums.iter().enumerate() {
        if csr.pixel_slots(pixel).is_empty() {
            continue;
        }
        let x = csr.origin.0 as usize + pixel % csr_width;
        let y = csr.origin.1 as usize + pixel / csr_width;
        if let Some(slot) = slots.slot_of_index(width, y * width as usize + x) {
            pairs.push((slot, Vec3::new(sum[0], sum[1], sum[2])));
        }
    }
    Ok(pairs)
}

/// Shades one sub-batch and submits its reduction under the renderer lock, which the caller
/// holds. The error is the decline reason.
fn submit_piece(
    ctx: &GpuContext,
    scene: &GpuSceneRef<'_>,
    piece: &PackedLeaves,
    params: BeamFragmentParams,
    (scratch, cancel): (&mut ReduceScratch, &AtomicBool),
) -> Result<SubmittedPiece, FragmentStop> {
    // The plan is host-only: check the reduction's bounding-rectangle sizes before any host or
    // device allocation, so a sparse piece declines to the CPU.
    let plan = FragmentPlan::from_leaves(piece, (scene.width, scene.height));
    if !reduction_fits(&plan, leaf_binding_limit(ctx)) {
        return Err(FragmentStop::Declined(DECLINE_REDUCTION_OVER_LIMIT));
    }
    let set =
        LeafBufferSet::upload(ctx, piece).ok_or(FragmentStop::Declined("leaf upload declined"))?;
    let options = FragmentOptions {
        max_levels: params.max_levels,
        n_lambda: params.n_lambda,
        tolerance: params.tolerance,
    };
    let slots = dispatch_fragments_cancellable(ctx, scene, &set, &plan, &options, cancel)?;
    if slots.count != plan.total {
        return Err(FragmentStop::Declined(
            "fragment slot count differs from the plan",
        ));
    }
    let csr = PixelCsr::from_plan(&plan);
    let reduction = submit_reduction(ctx, scratch, &slots.slots, &csr)
        .ok_or(FragmentStop::Declined("reduction declined"))?;
    Ok(SubmittedPiece { csr, reduction })
}

/// What one piece of a batch needs to be shaded.
struct PieceStep<'a, 'b> {
    scene: &'a GpuSceneRef<'b>,
    piece: &'a PackedLeaves,
    params: BeamFragmentParams,
    slots: &'a PixelSlots,
}

impl GpuBackend {
    /// Shades and reduces one piece: records and submits under the renderer lock, releases it,
    /// then waits for the readback on a cloned context.
    ///
    /// Waiting off the lock is safe: the wait polls the device for one named submission and
    /// reads buffers this call owns. Queue order keeps the next piece's scratch reuse behind
    /// this piece's copy. A device replaced meanwhile makes the scratch drop its buffers
    /// (`ReduceScratch::adopt`) and the failure check decline. The caller commits results only
    /// after every piece succeeds.
    fn run_piece(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        step: &PieceStep<'_, '_>,
        scratch: &mut ReduceScratch,
        observe_wait: &mut dyn FnMut(bool),
        cancel: &AtomicBool,
    ) -> Result<Vec<(usize, Vec3)>, FragmentStop> {
        let Some(renderer) = self.lock_renderer(mutex) else {
            return Err(FragmentStop::Declined("GPU renderer unavailable"));
        };
        let ctx = renderer.context();
        if self.lost.load(Ordering::Acquire) || ctx.failure().is_some() {
            return Err(FragmentStop::Declined("beam device failed"));
        }
        if let Some(reason) = busy_reason(ctx) {
            return Err(FragmentStop::Declined(reason));
        }
        let submitted = submit_piece(ctx, step.scene, step.piece, step.params, (scratch, cancel));
        let ctx = ctx.clone();
        drop(renderer);
        let submitted = submitted?;
        observe_wait(mutex.try_lock().is_ok());
        finish_piece(&ctx, (step.scene.width, step.slots), submitted)
            .map_err(FragmentStop::Declined)
    }

    /// Shades a batch of beam leaves on the GPU and ADDS each covered pixel's fragment sum into
    /// `out`.
    ///
    /// `out` is the whole image, row-major, `scene.width * scene.height` pixels (any other
    /// length declines). Only pixels covered by a leaf's rectangle change, by `+=` of their
    /// fragments' XYZ sum in leaf order, then the leaf's row-major pixel order. The batch is
    /// halved by tiles until every sub-batch's buffers fit the device's storage binding
    /// (`FragmentPlan`, `split_into_batches`); a single tile that still does not fit declines
    /// the whole call. `out` is written only on [`GpuBeamFragments::Done`]. `cancel` is polled
    /// before every sub-batch and after the last one. `params` gives the refine depth, `N_λ`
    /// and `τ_L` of the fragment kernel.
    ///
    /// This is [`Self::try_beam_fragments_compact`] with the whole frame as the one tile.
    pub fn try_beam_fragments(
        &self,
        scene: &GpuSceneRef<'_>,
        packed: &PackedLeaves,
        params: BeamFragmentParams,
        out: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuBeamFragments {
        self.try_beam_fragments_observed(scene, packed, params, out, cancel, &mut |_| {})
    }

    /// [`Self::try_beam_fragments`], calling `observe_wait(lock_free)` just before every
    /// readback wait with whether the renderer lock was free at that moment.
    ///
    /// A test seam: the readback wait must not hold the renderer lock.
    pub fn try_beam_fragments_observed(
        &self,
        scene: &GpuSceneRef<'_>,
        packed: &PackedLeaves,
        params: BeamFragmentParams,
        out: &mut [Vec3],
        cancel: &AtomicBool,
        observe_wait: &mut dyn FnMut(bool),
    ) -> GpuBeamFragments {
        let frame = TileRect {
            x0: 0,
            y0: 0,
            x1: scene.width,
            y1: scene.height,
        };
        let Some(slots) = PixelSlots::new(&[frame]) else {
            return GpuBeamFragments::Declined("malformed frame slots");
        };
        self.shade_into_slots((scene, packed, params), (&slots, out), cancel, observe_wait)
    }

    /// [`Self::try_beam_fragments`] into the compact pixels of a batch: `target` is the batch's
    /// [`PixelSlots`] and a slice of [`PixelSlots::len`] pixels (any other length declines).
    /// Each covered pixel's sum is ADDED at its slot instead of its place in a whole frame.
    ///
    /// A pixel without a slot (a leaf reaching past the batch's tiles) is dropped. For every
    /// pixel with a slot, the sum is bit for bit the whole-frame entry's sum.
    pub fn try_beam_fragments_compact(
        &self,
        scene: &GpuSceneRef<'_>,
        packed: &PackedLeaves,
        params: BeamFragmentParams,
        target: (&PixelSlots, &mut [Vec3]),
        cancel: &AtomicBool,
    ) -> GpuBeamFragments {
        self.shade_into_slots((scene, packed, params), target, cancel, &mut |_| {})
    }

    /// The body of the fragment entries: shades `packed` and adds each pixel's sum at its slot.
    fn shade_into_slots(
        &self,
        (scene, packed, params): (&GpuSceneRef<'_>, &PackedLeaves, BeamFragmentParams),
        (slots, out): (&PixelSlots, &mut [Vec3]),
        cancel: &AtomicBool,
        observe_wait: &mut dyn FnMut(bool),
    ) -> GpuBeamFragments {
        if cancel.load(Ordering::Relaxed) {
            return GpuBeamFragments::Cancelled;
        }
        if packed.headers.is_empty() {
            return GpuBeamFragments::Done;
        }
        let Some(mutex) = &self.renderer else {
            return GpuBeamFragments::Declined("no GPU adapter");
        };
        if self.lost.load(Ordering::Acquire) {
            return GpuBeamFragments::Declined("GPU device lost");
        }
        if out.len() != slots.len() {
            return GpuBeamFragments::Declined("output length differs from the pixel slots");
        }
        // The beam kernels run on the renderer's own device; recording and submitting happen
        // under its lock like every pass, the readback wait does not (see `run_piece`).
        let limit = {
            let Some(renderer) = self.lock_renderer(mutex) else {
                return GpuBeamFragments::Declined("GPU renderer unavailable");
            };
            let ctx = renderer.context();
            if ctx.failure().is_some() {
                return GpuBeamFragments::Declined("beam device failed");
            }
            leaf_binding_limit(ctx)
        };
        let whole = FragmentPlan::from_leaves(packed, (scene.width, scene.height));
        let Some(ranges) = split_into_batches(packed, &whole, limit) else {
            return GpuBeamFragments::Declined(DECLINE_TILE_OVER_LIMIT);
        };
        // One grow-only reduction buffer set serves every piece of the batch.
        let mut scratch = ReduceScratch::default();
        let mut pending: Vec<(usize, Vec3)> = Vec::new();
        for range in &ranges {
            if cancel.load(Ordering::Relaxed) {
                return GpuBeamFragments::Cancelled;
            }
            let sub;
            let piece = if ranges.len() == 1 {
                packed
            } else if let Some(found) = sub_batch(packed, range.clone()) {
                sub = found;
                &sub
            } else {
                return GpuBeamFragments::Declined("malformed tile tables");
            };
            let step = PieceStep {
                scene,
                piece,
                params,
                slots,
            };
            match self.run_piece(mutex, &step, &mut scratch, observe_wait, cancel) {
                Ok(pairs) => pending.extend(pairs),
                Err(FragmentStop::Cancelled) => return GpuBeamFragments::Cancelled,
                Err(FragmentStop::Declined(reason)) => return GpuBeamFragments::Declined(reason),
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return GpuBeamFragments::Cancelled;
        }
        for (index, sum) in pending {
            if let Some(pixel) = out.get_mut(index) {
                *pixel += sum;
            }
        }
        GpuBeamFragments::Done
    }
}
