//! Per-device caches of the beam kernels, owned by the
//! [`GpuContext`](crate::renderer::gpu::GpuContext) they were compiled for.
//!
//! A process-wide static would keep every device alive: each `wgpu` pipeline and buffer holds the
//! device's error sink, which holds a clone of the context's failure flag, so the flag's count
//! never reaches zero. The driver's exit handlers then ran over live devices and crashed at exit.
//! Owned by the context, the caches drop with its last clone, panic or not.

use std::sync::{Arc, Mutex};

use crate::renderer::env_map_gpu::HdrEnvGpuData;

/// The compiled fragment and refine kernels of one device.
pub(crate) struct BeamFragmentPipelines {
    /// The first-level fragment kernel (`bm_fragments_main`).
    pub(crate) fragments: wgpu::ComputePipeline,
    /// The refine kernel (`bm_refine_main`).
    pub(crate) refine: wgpu::ComputePipeline,
}

/// What the beam path keeps on one device between batches.
#[derive(Default)]
pub(crate) struct DeviceCaches {
    /// The fragment and refine kernels, compiled once.
    pub(crate) fragment_pipelines: Mutex<Option<Arc<BeamFragmentPipelines>>>,
    /// The per-pixel reduction kernel, compiled once.
    pub(crate) reduce_pipeline: Mutex<Option<Arc<wgpu::ComputePipeline>>>,
    /// The last HDR environment uploaded for the beam kernels: the map's hash and its buffers.
    pub(crate) hdr: Mutex<Option<(u64, Arc<HdrEnvGpuData>)>>,
    /// The last beam submission whose wait timed out and may still occupy the device.
    pub(crate) abandoned: Mutex<Option<wgpu::SubmissionIndex>>,
}
