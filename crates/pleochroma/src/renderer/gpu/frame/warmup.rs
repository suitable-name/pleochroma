//! Background preparation uses exactly the production setup, without dispatching
//! transport or advancing an absolute sample index.
use super::{GpuFrameError, TurnRequest, renderer::GpuFrameRenderer};

impl GpuFrameRenderer {
    pub(crate) fn prepare_without_dispatch(
        &mut self,
        request: &TurnRequest<'_>,
    ) -> Result<(), GpuFrameError> {
        if self.poisoned || self.turn_in_progress {
            return Err(GpuFrameError::DeviceLost(
                "GPU preparation found poisoned renderer state".into(),
            ));
        }
        self.check_uncaptured_error()?;
        if !request.scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                request.scene.material.name.clone(),
            ));
        }
        self.turn_in_progress = true;
        let scope = self
            .ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.prepare_turn(request).map(|setup| {
            self.scene_buffers = Some(setup.frame_buffers);
        });
        let validation = pollster::block_on(scope.pop());
        self.turn_in_progress = false;
        if let Some(error) = validation {
            self.abandon_in_flight();
            return Err(GpuFrameError::DeviceLost(format!(
                "GPU preparation validation: {error}"
            )));
        }
        if matches!(&result, Err(GpuFrameError::DeviceLost(_))) {
            self.abandon_in_flight();
        }
        result?;
        self.check_uncaptured_error()
    }
}
