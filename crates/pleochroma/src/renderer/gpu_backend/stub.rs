//! Stand-in for builds without the `gpu` feature. It has the same signatures as the
//! `gpu`-gated [`super::backend::GpuBackend`], so callers need no `#[cfg]`.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    GpuAccumulate, GpuBatchItem, GpuPathCache, GpuPipelineKind, GpuSceneRef, GpuWarmFamilies,
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::{materials::GemMaterial, raytracer::surface::SurfaceScene},
    renderer::adaptive::{LiveTiles, NoiseMetric, Pass, PendingCheck},
};

/// Always-declining `GpuBackend` for builds without the `gpu` feature.
pub struct GpuBackend;

/// Stand-in for the `gpu` build's GPU-resident adaptive run: never constructed, because
/// [`GpuBackend::try_adaptive_resident`] always declines without the `gpu` feature.
pub struct GpuResidentRun<'a> {
    never: std::convert::Infallible,
    marker: std::marker::PhantomData<&'a ()>,
}

impl GpuResidentRun<'_> {
    /// Unreachable: no value of this type exists.
    #[must_use]
    pub const fn pass(&mut self, _pass: &Pass, _cancel: &AtomicBool) -> GpuAccumulate {
        let _ = self.marker;
        GpuAccumulate::Declined
    }

    /// Unreachable: no value of this type exists.
    #[must_use]
    pub const fn tile_errors(
        &mut self,
        _check: &PendingCheck,
        _scale: f32,
        _metric: NoiseMetric,
    ) -> Option<Vec<f32>> {
        None
    }

    /// Unreachable: no value of this type exists.
    #[must_use]
    pub const fn finish(self) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
        match self.never {}
    }
}

impl GpuBackend {
    /// A build without GPU support sends every shadowed scene to the CPU.
    #[must_use]
    pub const fn supports_tray_shadow(&self) -> bool {
        false
    }

    /// A build without GPU support sends every mirrored scene to the CPU.
    #[must_use]
    pub const fn supports_tray_mirror(&self) -> bool {
        false
    }

    /// Creates the backend (a no-op stand-in without the `gpu` feature).
    #[must_use]
    pub const fn acquire() -> Self {
        Self
    }

    /// Creates a backend that never renders on the GPU.
    #[must_use]
    pub const fn disabled() -> Self {
        Self
    }

    /// Always `None`: with no `gpu` feature there is no adapter to name, so a caller
    /// advertising its backend correctly reports CPU.
    #[must_use]
    pub const fn adapter_label(&self) -> Option<String> {
        None
    }

    /// No-op: there is no renderer to select a kernel on. `_kind` is ignored only to match
    /// the `gpu`-gated [`super::backend::GpuBackend::set_pipeline_kind`].
    pub const fn set_pipeline_kind(&self, _kind: GpuPipelineKind) {}

    /// Always `false`: with no `gpu` feature there is no device to ever lose -- see the
    /// `gpu`-gated [`super::backend::GpuBackend::is_lost`].
    #[must_use]
    pub const fn is_lost(&self) -> bool {
        false
    }

    /// Always `true` ("not lost, nothing to recover"), since there is no device to lose.
    /// See the `gpu`-gated [`super::backend::GpuBackend::try_recover`].
    #[must_use]
    pub const fn try_recover(&self) -> bool {
        true
    }

    /// Always `None`, since [`Self::is_lost`] is always `false`.
    #[must_use]
    pub const fn last_lost_reason(&self) -> Option<String> {
        None
    }

    /// Always reports the GPU as unavailable, leaving `accum` untouched. Ignores every
    /// argument so the signature matches the `gpu` build.
    #[allow(
        clippy::unused_self,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate"
    )]
    pub const fn try_accumulate(
        &self,
        _scene: &GpuSceneRef<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
    ) -> bool {
        false
    }

    /// Always declines, leaving `accum` untouched. Arguments are ignored, as in
    /// [`Self::try_accumulate`]. See the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_cancellable`].
    pub const fn try_accumulate_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Always reports the GPU as unavailable -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_geom`] for what it stands in for.
    pub const fn try_accumulate_geom(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
    ) -> bool {
        false
    }

    /// Always declines, leaving `accum` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_geom_cancellable`].
    pub const fn try_accumulate_geom_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Always reports the GPU as unavailable -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_geom_surface`].
    pub const fn try_accumulate_geom_surface(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
    ) -> bool {
        false
    }

    /// Always declines, leaving `accum` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_geom_surface_cancellable`].
    #[expect(
        clippy::too_many_arguments,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_geom_surface_cancellable"
    )]
    pub const fn try_accumulate_geom_surface_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Always declines, leaving `accum` and `half_b` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_geom_surface_live_cancellable`].
    #[expect(
        clippy::too_many_arguments,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_geom_surface_live_cancellable"
    )]
    pub const fn try_accumulate_geom_surface_live_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _sample_offset: u32,
        _spp: u32,
        _live: &LiveTiles,
        _accum: &mut [Vec3],
        _half_b: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Always declines, leaving `accum` and `half_b` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_cached_live_cancellable`].
    #[expect(
        clippy::too_many_arguments,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_cached_live_cancellable"
    )]
    pub const fn try_accumulate_cached_live_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _sample_offset: u32,
        _spp: u32,
        _live: &LiveTiles,
        _cache: &GpuPathCache<'_, '_, '_>,
        _accum: &mut [Vec3],
        _half_b: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// No-op: with no `gpu` feature a cache never reaches the GPU -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::release_path_cache`].
    pub const fn release_path_cache(&self, _cache: &GpuPathCache<'_, '_, '_>) {}

    /// Always `None`: there is no device to bind a slab on -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::path_cache_slab_limit`].
    #[must_use]
    pub const fn path_cache_slab_limit(&self) -> Option<u64> {
        None
    }

    /// Always `None` (take the host path) -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_adaptive_resident`].
    #[must_use]
    pub const fn try_adaptive_resident<'a>(
        &'a self,
        _scene: &GpuSceneRef<'a>,
        _tools: &'a [ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _pass_spp: u32,
    ) -> Option<GpuResidentRun<'a>> {
        None
    }

    /// Always declines background preparation when the GPU feature is disabled.
    pub const fn prepare_geom_surface_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Always declines a shader warm-up when the GPU feature is disabled.
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

    /// Never: a build without GPU support keeps no pipelines.
    #[must_use]
    pub const fn persists_pipelines(&self) -> bool {
        false
    }

    /// Empty: a build without GPU support compiles no pipelines.
    #[must_use]
    pub const fn warm_up_materials(_first: Option<&GemMaterial>) -> Vec<GemMaterial> {
        Vec::new()
    }

    /// Always declines, leaving `out` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_beam_fragments`].
    #[cfg(feature = "beam")]
    pub const fn try_beam_fragments(
        &self,
        _scene: &GpuSceneRef<'_>,
        _packed: &crate::renderer::beam::beam::PackedLeaves,
        _params: super::BeamFragmentParams,
        _out: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> super::GpuBeamFragments {
        super::GpuBeamFragments::Declined("built without the gpu feature")
    }

    /// Always declines, leaving `accum` and `half_b` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_reference_live_cancellable`].
    #[cfg(feature = "beam")]
    #[expect(
        clippy::too_many_arguments,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_reference_live_cancellable"
    )]
    pub const fn try_accumulate_reference_live_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _tools: &[ToolPrimitive],
        _surface: &SurfaceScene<'_>,
        _sample_offset: u32,
        _spp: u32,
        _live: &LiveTiles,
        _kept: &crate::renderer::beam::beam::PackedKept,
        _n_lambda: u32,
        _accum: &mut [Vec3],
        _half_b: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Declines every item through `on_done`, leaving every `out` untouched -- see the
    /// `gpu`-gated [`super::backend::GpuBackend::try_accumulate_batch_cancellable`] for
    /// what it stands in for.
    #[allow(
        clippy::unused_self,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_batch_cancellable"
    )]
    pub fn try_accumulate_batch_cancellable(
        &self,
        items: &mut [GpuBatchItem<'_>],
        _cancel: &AtomicBool,
        on_done: &mut dyn FnMut(usize, GpuAccumulate),
    ) {
        for index in 0..items.len() {
            on_done(index, GpuAccumulate::Declined);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use glam::Vec3;

    use super::{GpuAccumulate, GpuBackend, GpuSceneRef, LiveTiles, SurfaceScene};
    use crate::{
        geometry::cuts::StandardGemCuts,
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, LightingPreset},
        },
    };

    /// The stand-in declines an adaptive pass and leaves both buffers untouched, exactly like
    /// the `gpu` build's declines.
    #[test]
    fn the_stub_declines_a_live_pass_and_touches_nothing() {
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let scene = GpuSceneRef {
            camera: &camera,
            width: 16,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
            estimator: None,
        };
        let live = LiveTiles::all(16, 16, 8);
        let sentinel = Vec3::new(1.5, -2.5, 3.5);
        let mut accum = vec![sentinel; 256];
        let mut half_b = vec![sentinel; 256];
        let never_cancel = AtomicBool::new(false);
        let outcome = GpuBackend::acquire().try_accumulate_geom_surface_live_cancellable(
            &scene,
            &[],
            &SurfaceScene::IDEAL,
            0,
            4,
            &live,
            &mut accum,
            &mut half_b,
            &never_cancel,
        );
        assert_eq!(outcome, GpuAccumulate::Declined);
        assert!(accum.iter().chain(&half_b).all(|p| *p == sentinel));
    }

    /// The stand-in never starts a GPU-resident run, so callers take the host path.
    #[test]
    fn the_stub_declines_a_resident_run() {
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let scene = GpuSceneRef {
            camera: &camera,
            width: 16,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
            estimator: None,
        };
        let backend = GpuBackend::acquire();
        let run = backend.try_adaptive_resident(&scene, &[], &SurfaceScene::IDEAL, 16);
        assert!(run.is_none());
    }
}
