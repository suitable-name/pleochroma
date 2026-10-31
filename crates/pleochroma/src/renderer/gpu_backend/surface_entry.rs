//! The surface-finish entry points of [`GpuBackend`]: the `try_accumulate_geom*` family with the
//! stone's facet surfaces, faceting machine and abraded edge riding with the request.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{GpuAccumulate, GpuBackend, GpuSceneRef};
use crate::{
    geometry::tool::ToolPrimitive, optics::raytracer::surface::SurfaceScene,
    renderer::buffers::GpuSurfaceScene,
};

impl GpuBackend {
    /// [`Self::try_accumulate_geom`] with the stone's surface finish: the facet surfaces, the
    /// faceting machine and the abraded edge of `surface`. An ideal scene is exactly
    /// [`Self::try_accumulate_geom`]. The finish rides with the request, so callers sharing
    /// this backend never trace each other's surfaces.
    pub fn try_accumulate_geom_surface(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> bool {
        let never_cancel = AtomicBool::new(false);
        matches!(
            self.try_accumulate_geom_surface_cancellable(
                scene,
                tools,
                surface,
                sample_offset,
                spp,
                accum,
                &never_cancel
            ),
            GpuAccumulate::Done
        )
    }

    /// [`Self::try_accumulate_geom_cancellable`] with the stone's surface finish -- see
    /// [`Self::try_accumulate_geom_surface`].
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `scene.width * scene.height`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the cancellable geometry entry point's parameter list plus the surface scene"
    )]
    pub fn try_accumulate_geom_surface_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let surface = GpuSurfaceScene::from_scene(surface);
        self.accumulate_geom_surface(
            scene,
            tools,
            Some(&surface),
            sample_offset,
            spp,
            accum,
            cancel,
        )
    }
}
