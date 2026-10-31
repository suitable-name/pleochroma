//! Compile the actual material pipeline and upload the actual scene off the image
//! worker, retaining all preparation in the backend for the next real sample range.
use super::GpuBackend;
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    geometry::tool::GPU_MAX_TOOL_PRIMITIVES,
    renderer::{
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameScene,
            frame::{TurnRequest, classify_material},
        },
    },
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::surface::SurfaceScene,
    renderer::gpu_backend::{GpuAccumulate, GpuSceneRef},
};
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;

impl GpuBackend {
    /// Compiles the scene's selected pipeline and uploads its geometry, surfaces and
    /// environment without tracing or changing any accumulation/sample counter.
    ///
    /// Check cancellation before and after the FIFO turn and synchronous compilation.
    /// A preparation worker should retain this backend for subsequent image work.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn prepare_geom_surface_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if cancel.load(Ordering::Relaxed) {
            return GpuAccumulate::Cancelled;
        }
        let pixels = u64::from(scene.width) * u64::from(scene.height);
        if pixels == 0
            || pixels > u64::from(u32::MAX)
            || tools.len() > GPU_MAX_TOOL_PRIMITIVES
            || !self.work_admits(scene, 1)
        {
            return GpuAccumulate::Declined;
        }
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &self.renderer else {
            return GpuAccumulate::Declined;
        };
        let ticket = self.turnstile.take_ticket();
        let _turn = self.turnstile.wait_for_turn(ticket);
        if cancel.load(Ordering::Relaxed) {
            return GpuAccumulate::Cancelled;
        }
        if self.lost.load(Ordering::Acquire) {
            return GpuAccumulate::Declined;
        }
        let Some(mut renderer) = self.lock_renderer(mutex) else {
            return GpuAccumulate::Declined;
        };
        if let Some(limits) = self.configured_limits() {
            renderer.set_chunk_budget_bytes(limits.chunk_budget_bytes);
        }
        let gpu_scene = GpuFrameScene {
            camera: scene.camera,
            width: scene.width,
            height: scene.height,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            max_bounces: scene.max_bounces,
            environment: scene.environment,
        };
        let encoded_surface = GpuSurfaceScene::from_scene(surface);
        let request = TurnRequest {
            scene: &gpu_scene,
            tools,
            pipeline_class: classify_material(scene.material),
            sample_offset: 0,
            spp: 1,
            cancel: Some(cancel),
            max_chunks: 0,
            surface: Some(&encoded_surface),
        };
        let result = renderer.prepare_without_dispatch(&request);
        drop(renderer);
        if let Err(error) = result {
            self.note_error(error);
            return GpuAccumulate::Declined;
        }
        if cancel.load(Ordering::Relaxed) {
            GpuAccumulate::Cancelled
        } else {
            GpuAccumulate::Done
        }
    }
    /// Native background preparation is unavailable on the browser thread.
    #[cfg(target_arch = "wasm32")]
    pub const fn prepare_geom_surface_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }
}
