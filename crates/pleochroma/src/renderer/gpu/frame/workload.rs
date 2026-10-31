//! Cost identity and bounded growth for adaptive GPU dispatches.

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::{
    geometry::plane::GpuFacetPlane, optics::raytracer::EnvironmentSource,
    render_setup::hash_planes, renderer::buffers::GpuGemMaterial,
};

use super::{GpuPipelineKind, TurnRequest, renderer::GpuFrameRenderer};

/// Includes the data that changes transport cost, even with identical geometry.
///
/// Camera motion and sample offset deliberately reuse measurements; the bounded
/// growth and short target duration limit how much one cheap view can overpredict.
pub(super) fn workload_key(
    request: &TurnRequest<'_>,
    pipeline: GpuPipelineKind,
    packed: &[GpuFacetPlane],
    material: &GpuGemMaterial,
    finishes: &[u32],
) -> u64 {
    let scene = request.scene;
    let mut hash = DefaultHasher::new();
    (
        scene.width,
        scene.height,
        scene.max_bounces,
        request.pipeline_class,
    )
        .hash(&mut hash);
    std::mem::discriminant(&pipeline).hash(&mut hash);
    hash_planes(packed).hash(&mut hash);
    bytemuck::bytes_of(material).hash(&mut hash);
    finishes.hash(&mut hash);
    match scene.environment {
        EnvironmentSource::HdrMap(map) => {
            0u8.hash(&mut hash);
            // Immutable maps are shared between frames; no per-frame scan of their pixels.
            std::ptr::from_ref(map).hash(&mut hash);
        }
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            backdrop,
            surface_glare,
            head_shadow_deg,
        } => {
            1u8.hash(&mut hash);
            std::mem::discriminant(&preset).hash(&mut hash);
            for value in [
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
            ] {
                value.to_bits().hash(&mut hash);
            }
        }
    }
    hash.finish()
}

impl GpuFrameRenderer {
    /// Grow at most twice the last completed chunk's tuple count. A background-only
    /// first chunk must not immediately expand into a full-frame heavy stone dispatch.
    pub(super) fn growth_limited_pixels(&self, spp: u32, budget: usize) -> usize {
        growth_limited_pixels(self.measured_chunk_tuples, spp, budget)
    }
}

fn growth_limited_pixels(measured: Option<usize>, spp: u32, budget: usize) -> usize {
    measured.map_or(budget, |tuples| {
        budget.min((tuples.saturating_mul(2) / (spp as usize).max(1)).max(1))
    })
}

#[cfg(test)]
mod tests {
    use super::{growth_limited_pixels, workload_key};
    use crate::{
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, LightingPreset},
        },
        renderer::{
            buffers::GpuGemMaterial,
            gpu::frame::{GpuFrameScene, GpuPipelineKind, TurnRequest},
        },
    };

    fn key(
        scene: &GpuFrameScene<'_>,
        material: &GpuGemMaterial,
        finishes: &[u32],
        pipeline: GpuPipelineKind,
    ) -> u64 {
        let request = TurnRequest {
            scene,
            tools: &[],
            pipeline_class: 1,
            sample_offset: 0,
            spp: 1,
            cancel: None,
            max_chunks: 1,
            surface: None,
        };
        workload_key(&request, pipeline, scene.planes, material, finishes)
    }

    #[test]
    fn unchanged_geometry_does_not_hide_cost_changes() {
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let gem = GemMaterial::diamond();
        let encoded = GpuGemMaterial::encode(&gem);
        let mut scene = GpuFrameScene {
            camera: &camera,
            width: 64,
            height: 64,
            planes: &[],
            facet_finishes: &[],
            material: &gem,
            max_bounces: 8,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        };
        let baseline = key(&scene, &encoded, &[0], GpuPipelineKind::Megakernel);
        scene.max_bounces = 64;
        assert_ne!(
            baseline,
            key(&scene, &encoded, &[0], GpuPipelineKind::Megakernel)
        );
        scene.max_bounces = 8;
        let mut anisotropic = encoded;
        anisotropic.crystal_system = encoded.crystal_system + 1;
        assert_ne!(
            baseline,
            key(&scene, &anisotropic, &[0], GpuPipelineKind::Megakernel)
        );
        assert_ne!(
            baseline,
            key(&scene, &encoded, &[1], GpuPipelineKind::Megakernel)
        );
        assert_ne!(
            baseline,
            key(&scene, &encoded, &[0], GpuPipelineKind::Wavefront)
        );
        scene.environment = LightingPreset::Daylight.studio(1.0, 0.8, 0.35);
        assert_ne!(
            baseline,
            key(&scene, &encoded, &[0], GpuPipelineKind::Megakernel)
        );
    }

    #[test]
    fn growth_uses_completed_work_and_respects_smaller_budgets() {
        assert_eq!(growth_limited_pixels(None, 8, 100_000), 100_000);
        assert_eq!(growth_limited_pixels(Some(32_768), 8, 100_000), 8_192);
        assert_eq!(growth_limited_pixels(Some(32_768), 8, 100), 100);
        assert_eq!(growth_limited_pixels(Some(1), 8, 100), 1);
    }
}
