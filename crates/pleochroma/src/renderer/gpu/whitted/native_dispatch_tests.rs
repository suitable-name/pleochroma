use super::{WhittedGpuRenderer, WhittedGpuTurnLimits};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource},
    },
    renderer::{
        engine::RayOptions, env_map::EnvironmentMap, frame_scene::FrameScene,
        gpu::test_support::require_gpu, whitted::WhittedPrep,
    },
};

fn prepared_test_scenes(renderer: &WhittedGpuRenderer) -> Vec<super::WhittedGpuScene> {
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let gradient = (0u32..512)
        .map(|pixel| [(pixel % 32) as f32 / 32.0, (pixel / 32) as f32 / 16.0, 0.5])
        .collect();
    let environments = [
        EnvironmentMap::from_rgb(32, 16, gradient).unwrap(),
        EnvironmentMap::uniform(4, 2, [0.8, 0.6, 0.2]),
    ];
    let mut uploads = Vec::new();
    for environment in &environments {
        let scene = FrameScene {
            camera: &camera,
            width: 19,
            height: 3,
            planes: &[],
            facet_finishes: &[],
            material: &material,
            max_bounces: 16,
            environment: EnvironmentSource::HdrMap(environment),
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            RayOptions {
                aa_taps: 4,
                fire: true,
            },
        )
        .unwrap();
        uploads.push(renderer.prepare(&scene, &[], &prep).unwrap());
    }
    uploads
}

use super::{WhittedGpuRange, split_turn};

#[test]
fn turns_partition_absolute_indices_without_changing_taps_or_crossing_frame_end() {
    let mut first = 0;
    let total = 2_073_601;
    while first < total {
        let ranges = split_turn(
            WhittedGpuRange {
                first_pixel: first,
                pixels: total - first,
                sample_offset: 7,
                spp: 3,
            },
            65_536,
            2,
        );
        assert!(!ranges.is_empty());
        assert!(ranges.len() <= 2);
        for range in ranges {
            assert_eq!(range.first_pixel, first);
            assert_eq!((range.sample_offset, range.spp), (7, 3));
            assert!(range.pixels > 0 && range.pixels <= 65_536);
            assert!(range.admitted(total, 16));
            first += range.pixels;
        }
    }
    assert_eq!(first, total);
}

#[test]
fn range_admission_rejects_overflow_and_empty_work() {
    let range = WhittedGpuRange {
        first_pixel: u32::MAX - 4,
        pixels: 4,
        sample_offset: 15,
        spp: 1,
    };
    assert!(range.admitted(u32::MAX, 16));
    assert!(!WhittedGpuRange { pixels: 5, ..range }.admitted(u32::MAX, 16));
    assert!(
        !WhittedGpuRange {
            spp: u32::MAX,
            ..range
        }
        .admitted(u32::MAX, 16)
    );
    assert!(!WhittedGpuRange { pixels: 0, ..range }.admitted(u32::MAX, 16));
    assert!(!WhittedGpuRange { spp: 0, ..range }.admitted(u32::MAX, 16));
    assert_eq!(split_turn(range, 1, 1).len(), 1);
    assert_eq!(split_turn(range, 0, usize::MAX).len(), 2);
}

#[test]
fn paired_chunks_reuse_slots_and_rebind_frozen_scenes_without_changing_samples() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context)).unwrap();
    let uploads = prepared_test_scenes(&renderer);
    let range = WhittedGpuRange {
        first_pixel: 9,
        pixels: 43,
        sample_offset: 1,
        spp: 2,
    };
    let references: Vec<_> = uploads
        .iter()
        .map(|scene| {
            renderer
                .trace_chunk(
                    scene,
                    range.first_pixel,
                    range.pixels,
                    range.sample_offset,
                    range.spp,
                )
                .unwrap()
        })
        .collect();
    assert_ne!(references[0], references[1]);
    assert!(
        references[0]
            .windows(2)
            .any(|pixels| pixels[0] != pixels[1]),
        "nonuniform lighting must exercise absolute pixel/jitter indexing"
    );
    for index in [0, 1, 0, 1] {
        let actual = renderer
            .trace_turn(
                &uploads[index],
                range,
                WhittedGpuTurnLimits {
                    max_chunk_pixels: 23,
                    max_chunks: 2,
                },
                &|| false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(actual, references[index]);
    }
    // Cancel after the first slot was submitted/mapped, then prove that slot can
    // be reused on the next request without stale staging data or accumulation.
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let cancelled = renderer
        .trace_turn(
            &uploads[0],
            range,
            WhittedGpuTurnLimits {
                max_chunk_pixels: 23,
                max_chunks: 2,
            },
            &|| checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1,
        )
        .unwrap();
    assert!(cancelled.is_none());
    let actual = renderer
        .trace_turn(
            &uploads[0],
            range,
            WhittedGpuTurnLimits {
                max_chunk_pixels: 23,
                max_chunks: 2,
            },
            &|| false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(actual, references[0]);
}
