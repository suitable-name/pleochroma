//! Source-based capacity sizing, not measured occupancy or autotuning.
#[cfg(feature = "cuda")]
use super::OpticalError;
#[cfg(feature = "cuda")]
use cudarc::driver::{CudaContext, sys::CUdevice_attribute_enum as Attribute};

/// Device properties and calculated optical batching limits captured at service creation.
///
/// Limits are based on hardware and memory capacity, not measured throughput or occupancy.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct OpticalCapacity {
    /// Device name reported by CUDA.
    pub device_name: String,
    /// Zero-based CUDA device index shared with the neural training backend.
    pub device_ordinal: usize,
    /// Major component of the device's CUDA compute capability.
    pub compute_major: i32,
    /// Minor component of the device's CUDA compute capability.
    pub compute_minor: i32,
    /// Number of streaming multiprocessors reported by CUDA.
    pub multiprocessors: usize,
    /// Hardware maximum resident threads per streaming multiprocessor, not measured occupancy.
    pub threads_per_multiprocessor: usize,
    /// Free device memory in bytes at sizing time; not a reservation or a live reading.
    pub free_bytes_at_creation: usize,
    /// Total device memory in bytes reported by CUDA.
    pub total_bytes: usize,
    /// Budget used to size optical buffers, excluding Burn and CUDA driver/module overhead.
    pub scratch_budget_bytes: usize,
    /// Maximum Fast candidates combined into one native optical batch.
    pub max_fast_candidates: usize,
    /// Maximum Full-fidelity poses processed in one native ray-tracing tile.
    pub max_pose_tile: usize,
    /// Capacity of the bounded request channel, excluding work already held by the worker.
    pub queue_capacity: usize,
}
#[cfg(feature = "cuda")]
pub(super) fn size(
    context: &CudaContext,
    requested_poses: usize,
    cells: usize,
) -> Result<OpticalCapacity, OpticalError> {
    let error = |e| OpticalError::Unavailable(format!("CUDA capacity query: {e}"));
    let attribute = |name| context.attribute(name).map_err(error);
    let sm = attribute(Attribute::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT)? as usize;
    let threads =
        attribute(Attribute::CU_DEVICE_ATTRIBUTE_MAX_THREADS_PER_MULTIPROCESSOR)? as usize;
    let major = attribute(Attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR)?;
    let minor = attribute(Attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR)?;
    let (free, total) = context.mem_get_info().map_err(error)?;
    // Leave at least 7/8 of free VRAM for Burn and other CUDA services. Capacity growth
    // is bounded by this budget and a 512 MiB cap; this is sizing, not an occupancy claim.
    let budget = (free / 8).min(512 * 1024 * 1024);
    let fixed = 64 * 1024 * 1024;
    if budget < fixed + 4 * 1024 * 1024 {
        return Err(OpticalError::Unavailable(
            "insufficient CUDA headroom for bounded optical scratch".into(),
        ));
    }
    let maximum_pose_bytes = cells * 5 * 2 * 12 * std::mem::size_of::<f32>();
    let rays = sm.saturating_mul(threads).saturating_mul(3).max(1);
    let pose_capacity = rays.div_ceil(cells * 5 * 2).max(1);
    let memory_poses = (budget - fixed) / (maximum_pose_bytes * 2);
    let max_pose_tile = requested_poses
        .max(1)
        .min(512)
        .min(pose_capacity)
        .min(memory_poses)
        .max(1);
    // Geometric growth can double each explicit arena: ray records, resident paths,
    // planes, 128 tools, finishes and all three 4096-band absorption lists.
    let candidate_bytes = 2
        * (maximum_pose_bytes
            + 4096 * 5 * std::mem::size_of::<f64>()
            + 4096 * 4 * 4
            + 128 * 85 * 4
            + (4096 + 128) * 4
            + 3 * 4096 * 4 * 4);
    let max_fast_candidates = rays
        .div_ceil(cells * 5)
        .clamp(16, 128)
        .min((budget - fixed) / candidate_bytes)
        .max(1);
    Ok(OpticalCapacity {
        device_name: context.name().map_err(error)?,
        device_ordinal: context.ordinal(),
        compute_major: major,
        compute_minor: minor,
        multiprocessors: sm,
        threads_per_multiprocessor: threads,
        free_bytes_at_creation: free,
        total_bytes: total,
        scratch_budget_bytes: budget,
        max_fast_candidates,
        max_pose_tile,
        queue_capacity: (max_fast_candidates * 2).clamp(16, 256),
    })
}
