//! Legacy wavefront must cache the same material features as the other transport paths.

use glam::Vec3;

use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::gpu::merge_tests::renderer_or_skip,
};

use super::{GpuFrameScene, GpuPipelineKind, classify_material};

#[test]
fn legacy_wavefront_reuses_only_matching_dispersion_pipelines() {
    let Some(mut renderer) = renderer_or_skip("legacy_wavefront_dispersion_pipelines") else {
        return;
    };
    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    if renderer.pipeline_kind() != GpuPipelineKind::Wavefront {
        println!("adapter cannot bind the legacy wavefront pipeline; skipping");
        return;
    }
    assert!(
        renderer.wavefront_pipelines.is_empty(),
        "selection must not compile a generic kernel"
    );
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let zircon = GemMaterial::by_name("Zircon").expect("built-in Zircon");
    let rutile = GemMaterial::by_name("Rutile").expect("built-in Rutile");
    assert_ne!(classify_material(&zircon), classify_material(&rutile));
    let mut first = Vec::new();
    for (index, material) in [&zircon, &rutile, &zircon].into_iter().enumerate() {
        let scene = GpuFrameScene {
            camera: &camera,
            width: 8,
            height: 8,
            planes: &planes,
            facet_finishes: &[],
            material,
            max_bounces: 8,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        };
        let mut output = vec![Vec3::ZERO; 64];
        renderer
            .accumulate(&scene, 0, 2, &mut output)
            .expect("wavefront render");
        assert!(output.iter().all(|pixel| pixel.is_finite()));
        assert!(
            renderer
                .wavefront_pipelines
                .contains_key(&classify_material(material))
        );
        if index == 0 {
            first = output;
        } else if index == 2 {
            assert_eq!(
                first, output,
                "switching curve capabilities must not alter a cached render"
            );
        }
    }
    assert_eq!(renderer.wavefront_pipelines.len(), 2);
}
