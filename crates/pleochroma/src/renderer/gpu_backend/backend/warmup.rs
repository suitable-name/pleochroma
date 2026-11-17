//! Compile the actual material pipeline and upload the actual scene off the image
//! worker, retaining all preparation in the backend for the next real sample range.
//!
//! A shader warm-up ([`GpuBackend::warm_pipelines_cancellable`] over
//! [`GpuBackend::warm_up_materials`]) does the same for one stand-in scene per material class,
//! before any real scene asks.
use super::GpuBackend;
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    geometry::tool::GPU_MAX_TOOL_PRIMITIVES,
    renderer::{
        buffers::GpuSurfaceScene,
        gpu::{GpuFrameScene, frame::TurnRequest},
    },
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::{materials::GemMaterial, raytracer::surface::SurfaceScene},
    renderer::{
        gpu::frame::{classify_material, material_class},
        gpu_backend::{GpuAccumulate, GpuSceneRef, GpuWarmFamilies},
    },
};
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{PoisonError, atomic::Ordering};

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
        self.warm_pipelines_cancellable(scene, tools, surface, GpuWarmFamilies::default(), cancel)
    }

    /// [`Self::prepare_geom_surface_cancellable`] plus the pipelines of `families` for the
    /// scene's material class, so a later live or cached pass of a scene of that class compiles
    /// nothing. The scene's uploads stay until the next turn replaces them.
    ///
    /// Takes one FIFO turn like any request, so a real request waits for at most one warm-up
    /// step. Cancellation is checked before and after the turn.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn warm_pipelines_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        families: GpuWarmFamilies,
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
            estimator: None,
        };
        let result = renderer.warm_pipelines(&request, families);
        // Beam kernels are not compiled here: they compile on the first beam batch
        // (`gpu::beam::fragments`).
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

    /// Whether this backend's device keeps compiled pipelines between runs: wgpu's pipeline
    /// cache (Vulkan) with the application's cache folder set
    /// (`gpu::pipeline_cache::set_pipeline_cache_dir`). Without it every start compiles again.
    /// `false` for a disabled or lost backend.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn persists_pipelines(&self) -> bool {
        if self.lost.load(Ordering::Acquire) {
            return false;
        }
        self.renderer.as_ref().is_some_and(|mutex| {
            mutex
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .persists_pipelines()
        })
    }

    /// How many pipelines this process compiled from source instead of loading them from the
    /// persistent cache, since it started
    /// ([`crate::renderer::gpu::pipeline_cache::fresh_compiles`]). A shader warm-up compares the
    /// number before and after a step: a step that raises it built something fresh. Stays `0`
    /// on a device that keeps no driver data, where the caller treats every step as fresh
    /// ([`Self::persists_pipelines`] is `false`).
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn fresh_pipeline_compiles() -> u64 {
        crate::renderer::gpu::pipeline_cache::fresh_compiles()
    }

    /// The part of [`Self::fresh_pipeline_compiles`] counted on the calling thread
    /// ([`crate::renderer::gpu::pipeline_cache::fresh_compiles_on_this_thread`]): a warm-up step
    /// compares it before and after, exactly even while other threads compile.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn fresh_pipeline_compiles_on_this_thread() -> u64 {
        crate::renderer::gpu::pipeline_cache::fresh_compiles_on_this_thread()
    }

    /// One material per Path-Traced pipeline class, for a shader warm-up: `first` when the GPU
    /// traces it, then the first built-in material of every other class, in catalogue order.
    ///
    /// The class is the material's ([`classify_material`], with the process's optics-table
    /// choice) folded like the pipeline caches fold it ([`material_class::pipeline_key`]); the
    /// tray variants come from the scene, not the material.
    #[must_use]
    pub fn warm_up_materials(first: Option<&GemMaterial>) -> Vec<GemMaterial> {
        let catalogue: &[GemMaterial] = GemMaterial::built_in();
        let mut keys = Vec::new();
        let mut materials = Vec::new();
        for material in first.into_iter().chain(catalogue) {
            if !material.gpu_supported() {
                continue;
            }
            let key = material_class::pipeline_key(classify_material(material));
            if !keys.contains(&key) {
                keys.push(key);
                materials.push(material.clone());
            }
        }
        materials
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

    /// Native background preparation is unavailable on the browser thread.
    #[cfg(target_arch = "wasm32")]
    pub const fn warm_pipelines_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _families: GpuWarmFamilies,
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// The browser compiles every pipeline: nothing is counted.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub const fn fresh_pipeline_compiles() -> u64 {
        0
    }

    /// The browser keeps no pipelines between page loads.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub const fn persists_pipelines(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warm_up_list_has_one_gpu_material_per_class_with_the_first_one_first() {
        let first = GemMaterial::built_in()
            .iter()
            .rev()
            .find(|material| material.gpu_supported())
            .expect("a built-in material the GPU traces")
            .clone();
        let list = GpuBackend::warm_up_materials(Some(&first));
        assert_eq!(
            list.first().map(|m| m.name.as_str()),
            Some(first.name.as_str())
        );
        let mut keys: Vec<u32> = list
            .iter()
            .map(|m| material_class::pipeline_key(classify_material(m)))
            .collect();
        assert!(list.iter().all(GemMaterial::gpu_supported));
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "one material per class");
        // Every class of the catalogue is covered.
        for material in GemMaterial::built_in().iter().filter(|m| m.gpu_supported()) {
            let key = material_class::pipeline_key(classify_material(material));
            assert!(keys.contains(&key), "{} has no stand-in", material.name);
        }
    }

    #[test]
    fn without_a_first_material_the_list_starts_with_the_catalogue() {
        let list = GpuBackend::warm_up_materials(None);
        let first = GemMaterial::built_in()
            .iter()
            .find(|material| material.gpu_supported())
            .expect("a built-in material the GPU traces");
        assert_eq!(
            list.first().map(|m| m.name.as_str()),
            Some(first.name.as_str())
        );
    }
}
