//! Persistent geometric-growth CUDA buffers, bounded by validated request/tile limits.
use super::OpticalError;
use cudarc::driver::{CudaSlice, CudaStream, DeviceRepr, ValidAsZeroBits};
use std::sync::Arc;

pub(super) struct Buffers {
    pub planes: CudaSlice<f32>,
    pub tools: CudaSlice<f32>,
    pub finishes: CudaSlice<u32>,
    pub material: CudaSlice<f32>,
    pub poses: CudaSlice<f32>,
    pub descriptors: CudaSlice<u32>,
    pub output: CudaSlice<f32>,
}
impl Buffers {
    pub fn new(stream: &Arc<CudaStream>) -> Result<Self, OpticalError> {
        Ok(Self {
            planes: stream.alloc_zeros(1).map_err(error)?,
            tools: stream.alloc_zeros(1).map_err(error)?,
            finishes: stream.alloc_zeros(1).map_err(error)?,
            material: stream.alloc_zeros(1).map_err(error)?,
            poses: stream.alloc_zeros(1).map_err(error)?,
            descriptors: stream.alloc_zeros(1).map_err(error)?,
            output: stream.alloc_zeros(1).map_err(error)?,
        })
    }
}
fn error(e: impl std::fmt::Display) -> OpticalError {
    OpticalError::Runtime(e.to_string())
}

pub(super) fn ensure<T: DeviceRepr + ValidAsZeroBits>(
    stream: &Arc<CudaStream>,
    buffer: &mut CudaSlice<T>,
    len: usize,
) -> Result<(), OpticalError> {
    if buffer.len() < len {
        *buffer = stream.alloc_zeros(len.next_power_of_two()).map_err(error)?;
    }
    Ok(())
}
pub(super) fn upload<T: DeviceRepr + ValidAsZeroBits>(
    stream: &Arc<CudaStream>,
    buffer: &mut CudaSlice<T>,
    values: &[T],
) -> Result<(), OpticalError> {
    ensure(stream, buffer, values.len())?;
    stream.memcpy_htod(values, buffer).map_err(error)
}
