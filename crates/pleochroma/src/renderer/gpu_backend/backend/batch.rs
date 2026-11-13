//! [`GpuBackend::try_accumulate_batch_cancellable`]: many pictures through one renderer turn
//! stream.

use std::sync::atomic::{AtomicBool, Ordering};

use super::{GpuBackend, batch_output::BatchOutput};
use crate::{
    geometry::tool::GPU_MAX_TOOL_PRIMITIVES,
    renderer::{
        buffers::GpuSurfaceScene,
        gpu::frame::{TurnRequest, classify_material},
        gpu_backend::{GpuAccumulate, GpuBatchItem},
    },
};

impl GpuBackend {
    /// Traces every item of `items` through one stream of GPU chunks, keeping the queue fed
    /// across picture boundaries. Calls `on_done(index, outcome)` as each picture finishes,
    /// in input order.
    ///
    /// Each item's result is bit-identical to [`Self::try_accumulate_cancellable`] on that
    /// item alone: samples go into a zeroed scratch and are added into `out` only on
    /// [`GpuAccumulate::Done`]. Declines follow the single-picture rules per item: a device
    /// loss declines every unreported picture, and an unsupported material or environment
    /// declines only its own. `cancel` reports the first unfinished picture and all later
    /// ones as `Cancelled`; finished pictures stay `Done`. Afterwards the renderer is idle
    /// and reusable.
    ///
    /// The chunk stream is described in the parent module's "Batches" section. The batch
    /// yields the renderer turn after `CHUNKS_PER_TURN` chunks when another request is
    /// waiting. `on_done` runs while the turn is held, so it should be cheap.
    ///
    /// # Panics
    ///
    /// Panics if an item's `out.len()` is not `scene.width * scene.height`.
    pub fn try_accumulate_batch_cancellable(
        &self,
        items: &mut [GpuBatchItem<'_>],
        cancel: &AtomicBool,
        on_done: &mut dyn FnMut(usize, GpuAccumulate),
    ) {
        if self.configured_limits().is_some() {
            for index in 0..items.len() {
                on_done(index, GpuAccumulate::Declined);
            }
            return;
        }
        for item in items.iter() {
            assert_eq!(
                item.out.len(),
                item.scene.width as usize * item.scene.height as usize,
                "batch output buffer must have one entry per pixel"
            );
        }
        let count = items.len();
        // Defence in depth: callers route each picture first. A picture over the tool limit
        // declines the whole batch before any dispatch.
        if items
            .iter()
            .any(|item| item.tools.len() > GPU_MAX_TOOL_PRIMITIVES)
        {
            for index in 0..count {
                on_done(index, GpuAccumulate::Declined);
            }
            return;
        }
        let usable = (!self.lost.load(Ordering::Acquire) || self.try_recover())
            .then_some(self.renderer.as_ref())
            .flatten();
        let Some(mutex) = usable else {
            for index in 0..count {
                on_done(index, GpuAccumulate::Declined);
            }
            return;
        };

        let scenes: Vec<crate::renderer::gpu::GpuFrameScene<'_>> = items
            .iter()
            .map(|item| crate::renderer::gpu::GpuFrameScene {
                camera: item.scene.camera,
                width: item.scene.width,
                height: item.scene.height,
                planes: item.scene.planes,
                facet_finishes: item.scene.facet_finishes,
                material: item.scene.material,
                max_bounces: item.scene.max_bounces,
                environment: item.scene.environment,
            })
            .collect();
        // One owned copy per picture that has a surface finish; an ideal picture borrows none.
        let surfaces: Vec<Option<GpuSurfaceScene>> = items
            .iter()
            .map(|item| {
                (!item.surface.is_ideal()).then(|| GpuSurfaceScene::from_scene(&item.surface))
            })
            .collect();
        let requests: Vec<TurnRequest<'_>> = scenes
            .iter()
            .zip(items.iter())
            .zip(&surfaces)
            .map(|((scene, item), surface)| TurnRequest {
                scene,
                tools: item.tools,
                pipeline_class: classify_material(scene.material),
                sample_offset: item.first_sample,
                spp: item.samples,
                cancel: Some(cancel),
                max_chunks: usize::MAX,
                surface: surface.as_ref(),
                estimator: item.scene.estimator,
            })
            .collect();

        let mut output = BatchOutput {
            items,
            scratches: vec![Vec::new(); count],
            on_done,
            reported: 0,
        };
        self.run_batch(mutex, &requests, &mut output);
        // Normally every picture was reported by now; this only catches a path that ended
        // the stream early, so no `on_done` is ever missing.
        output.finish_rest(self, GpuAccumulate::Declined);
    }
}
