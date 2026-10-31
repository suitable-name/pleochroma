//! Preserve the first device error through submission and readback failures.

use super::{GpuFrameError, renderer::GpuFrameRenderer};

impl GpuFrameRenderer {
    /// Rejects a failed context before another GPU operation is submitted.
    pub(super) fn check_device_error(&self) -> Result<(), GpuFrameError> {
        self.ctx
            .failure()
            .map_or(Ok(()), |detail| Err(GpuFrameError::DeviceLost(detail)))
    }

    /// Failure is sticky: discard staging state and never reuse this renderer.
    pub(super) fn check_uncaptured_error(&mut self) -> Result<(), GpuFrameError> {
        let result = self.check_device_error();
        if result.is_err() {
            self.abandon_in_flight();
        }
        result
    }
}
