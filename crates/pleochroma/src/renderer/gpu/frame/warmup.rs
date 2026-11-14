//! Background preparation uses exactly the production setup, without dispatching
//! transport or advancing an absolute sample index.
use super::{GpuFrameError, TurnRequest, renderer::GpuFrameRenderer};
use crate::renderer::gpu_backend::GpuWarmFamilies;

impl GpuFrameRenderer {
    /// [`Self::prepare_class_without_dispatch`] plus the pipelines of `families` for the
    /// scene's class: what the first live or cached pass of such a scene would otherwise compile.
    /// A family this renderer cannot run is left out ([`Self::warm_live_pipelines`],
    /// [`Self::warm_cached_pipelines`]): its passes decline on this device anyway.
    ///
    /// # Errors
    ///
    /// Those of [`Self::prepare_class_without_dispatch`], and [`GpuFrameError::DeviceLost`] when
    /// the device rejects a family's pipeline.
    pub(crate) fn warm_pipelines(
        &mut self,
        request: &TurnRequest<'_>,
        families: GpuWarmFamilies,
    ) -> Result<(), GpuFrameError> {
        let class = self.prepare_class_without_dispatch(request)?;
        if families == GpuWarmFamilies::default() {
            return Ok(());
        }
        self.turn_in_progress = true;
        let scope = self
            .ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let built = if families.live {
            self.warm_live_pipelines(class)
        } else {
            Ok(())
        };
        if families.path_cache && built.is_ok() {
            self.warm_cached_pipelines(class);
        }
        let validation = pollster::block_on(scope.pop());
        self.turn_in_progress = false;
        if let Some(error) = validation {
            self.abandon_in_flight();
            return Err(GpuFrameError::DeviceLost(format!(
                "GPU preparation validation: {error}"
            )));
        }
        built?;
        self.check_uncaptured_error()
    }

    /// Whether this device keeps compiled pipelines between runs: it offers wgpu's pipeline
    /// cache (Vulkan) and the application set a cache folder
    /// ([`crate::renderer::gpu::pipeline_cache::set_pipeline_cache_dir`]).
    pub(crate) fn persists_pipelines(&self) -> bool {
        self.ctx
            .device
            .features()
            .contains(wgpu::Features::PIPELINE_CACHE)
            && crate::renderer::gpu::pipeline_cache::configured()
    }

    /// Compiles the scene's pipeline and its chunk engine's, and uploads the scene, without
    /// dispatching; returns the pipeline class it prepared.
    fn prepare_class_without_dispatch(
        &mut self,
        request: &TurnRequest<'_>,
    ) -> Result<u32, GpuFrameError> {
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
            // The chunk engine's pipelines as well (the sorted wavefront's or the staged
            // megakernel's while their switches are on), so the first chunk of the render does
            // not build them.
            self.ensure_chunk_engine(&setup.state, 1);
            let class = setup.state.pipeline_class;
            self.scene_buffers = Some(setup.frame_buffers);
            class
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
        let class = result?;
        self.check_uncaptured_error()?;
        Ok(class)
    }
}
