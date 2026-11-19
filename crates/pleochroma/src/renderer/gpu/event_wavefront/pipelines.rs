//! Pipeline cache and compilation management for the event wavefront kernels.

use std::collections::HashMap;

use super::kernels::EwKernel;
use crate::renderer::gpu::frame::event_wavefront_source_for;

/// Standalone reduce kernel source: sums `out_xyz` per-sample values into per-pixel XYZ.
const REDUCE_SHADER_SRC: &str = include_str!("../../shaders/reduce_xyz.wgsl");

/// Lazy cache of `(EwKernel, class)` compute pipelines, plus the eagerly built reduce pipeline.
///
/// Every pipeline compiles through its own persistent cache shard
/// (`compute::create_persisted_compute_pipeline`), so a start with a cache folder loads the
/// driver's compiled code instead of building it again.
pub struct EwPipelines {
    pipelines: HashMap<(EwKernel, u32), wgpu::ComputePipeline>,
    reduce_pipeline: wgpu::ComputePipeline,
    /// The adapter the device came from: the shards are stored per adapter and driver.
    adapter_info: wgpu::AdapterInfo,
}

impl EwPipelines {
    /// Creates a new pipeline cache and compiles `reduce_xyz_main`.
    ///
    /// `adapter_info` is the device's adapter (`ctx.adapter.get_info()`).
    #[must_use]
    pub fn new(device: &wgpu::Device, adapter_info: wgpu::AdapterInfo) -> Self {
        let reduce_pipeline = crate::renderer::gpu::compute::create_persisted_compute_pipeline(
            device,
            &adapter_info,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
            &[],
        );
        Self {
            pipelines: HashMap::new(),
            reduce_pipeline,
            adapter_info,
        }
    }

    /// Ensures that the pipeline for `(kernel, class)` is compiled and returns a reference to it.
    ///
    /// A class carrying `material_class::OPTICS_TABLES` compiles the optics-table variant of the
    /// kernel; the class is part of the key, so the two never share a pipeline.
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        kernel: EwKernel,
        class: u32,
    ) -> &wgpu::ComputePipeline {
        let adapter_info = &self.adapter_info;
        self.pipelines.entry((kernel, class)).or_insert_with(|| {
            crate::renderer::gpu::compute::create_persisted_compute_pipeline(
                device,
                adapter_info,
                kernel.entry_point(),
                event_wavefront_source_for(kernel, class),
                kernel.entry_point(),
                &crate::renderer::gpu::frame::material_class::pipeline_constants(class),
            )
        })
    }

    /// Retrieves an already-compiled compute pipeline.
    ///
    /// # Panics
    /// Panics if the pipeline was not compiled by [`Self::ensure`] first.
    #[must_use]
    pub fn get(&self, kernel: EwKernel, class: u32) -> &wgpu::ComputePipeline {
        self.pipelines
            .get(&(kernel, class))
            .unwrap_or_else(|| panic!("pipeline {kernel:?} (class {class}) was not compiled"))
    }

    /// Whether the pipeline for `(kernel, class)` is built.
    #[must_use]
    pub fn contains(&self, kernel: EwKernel, class: u32) -> bool {
        self.pipelines.contains_key(&(kernel, class))
    }

    /// Accessor for the shared `reduce_xyz_main` compute pipeline.
    #[must_use]
    pub const fn reduce_pipeline(&self) -> &wgpu::ComputePipeline {
        &self.reduce_pipeline
    }

    /// Returns an iterator over all compiled `(EwKernel, class)` pairs.
    pub fn compiled(&self) -> impl Iterator<Item = (EwKernel, u32)> + '_ {
        self.pipelines.keys().copied()
    }
}
