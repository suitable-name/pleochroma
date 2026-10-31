//! Event-wavefront image statistics against the CPU tracer, independent of the
//! megakernel. Zircon exercises uniaxial dispersion without a separate
//! extraordinary curve; Quartz and Rutile exercise genuine extraordinary curves.
//!
//! Each backend traces 512 samples per pixel, grouped into 32 disjoint batches of
//! 16. The statistical observations are batch means, so the result's sample-count
//! fields report 32, not 512. CPU and GPU use disjoint global sample ranges as well.
//! These retain the renderer's stratified sampling, rather than claiming every
//! underlying sample is an independent identically distributed random draw.
//! Diagnostic readback rejects nonfinite finalized GPU XYZ before the production
//! reducer can drop those samples; it does not inspect intermediate shader state.

use glam::Vec3;

use crate::{
    optics::raytracer::LightingPreset,
    renderer::{
        frame_scene::FrameScene,
        gpu::{GpuContext, event_wavefront::EventWavefrontRenderer},
    },
};

use super::{
    CpuScene, ImageComparisonResult, cpu_sample_xyz, cpu_samples,
    image_comparison::compute_image_comparison_result_channel, quartz_material,
    round_brilliant_planes, rutile_material, test_camera, zircon_material,
};

const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const BATCHES: u32 = 32;
const SAMPLES_PER_BATCH: u32 = 16;
const TOTAL_SAMPLES: u32 = BATCHES * SAMPLES_PER_BATCH;

/// Compares event-wavefront rendering directly with the CPU for three uniaxial
/// materials under Daylight, including both extraordinary-dispersion paths.
///
/// Each result compares 32 batch means per pixel (512 actual samples per backend) of ONE XYZ
/// component; every material yields an X, a Y and a Z result, all of which must pass.
/// Only 32 GPU accumulation calls are needed per material; each call traces 16
/// samples together using the production event-wavefront renderer.
///
/// # Panics
///
/// Panics with the material and batch if GPU rendering fails.
#[must_use]
pub fn run_event_wavefront_cpu_comparisons(
    ctx: &GpuContext,
) -> Vec<(String, ImageComparisonResult)> {
    let camera = test_camera();
    let planes = round_brilliant_planes();
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
    let mut renderer = EventWavefrontRenderer::new_diagnostic(ctx.clone());
    let mut results = Vec::with_capacity(9);

    for (label, material) in [
        ("Zircon (Daylight, 32 x 16 spp)", zircon_material()),
        ("Quartz (Daylight, 32 x 16 spp)", quartz_material()),
        ("Rutile (Daylight, 32 x 16 spp)", rutile_material()),
    ] {
        let cpu_scene = CpuScene {
            camera: &camera,
            width: WIDTH,
            height: HEIGHT,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
        };
        // Both the CPU trace and the production GPU accumulation apply the
        // environment's white balance internally. Do not apply it a second time.
        let cpu_flat = cpu_samples(WIDTH, HEIGHT, TOTAL_SAMPLES, 0, |pixel, sample| {
            cpu_sample_xyz(&cpu_scene, pixel, sample, environment)
        });
        // cpu_samples is pixel-major, so consecutive groups remain pixel-major.
        let cpu_means = batch_means(&cpu_flat, SAMPLES_PER_BATCH);
        let scene = FrameScene {
            camera: &camera,
            width: WIDTH,
            height: HEIGHT,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
            environment,
        };
        let pixels = (WIDTH * HEIGHT) as usize;
        let mut gpu_batch_major = Vec::with_capacity(pixels * BATCHES as usize);
        for batch in 0..BATCHES {
            let mut sums = vec![Vec3::ZERO; pixels];
            // CPU: [0, 512); GPU: [512, 1024), with non-overlapping GPU batches.
            let offset = TOTAL_SAMPLES + batch * SAMPLES_PER_BATCH;
            pollster::block_on(renderer.accumulate(&scene, offset, SAMPLES_PER_BATCH, &mut sums))
                .unwrap_or_else(|error| {
                    panic!("event-wavefront CPU comparison {label}, batch {batch}: {error}")
                });
            // accumulate adds sample SUMS, not means.
            gpu_batch_major.extend(sums.into_iter().map(|sum| sum / SAMPLES_PER_BATCH as f32));
        }
        let gpu_xyz = pixel_major_xyz(&gpu_batch_major, pixels);
        // X, Y and Z each get the full z-test: an index error that only moves the hue
        // leaves the luminance (Y) alone and would pass a Y-only comparison.
        for (component, channel) in [("X", 0usize), ("Y", 1), ("Z", 2)] {
            results.push((
                format!("{label} [{component}]"),
                compute_image_comparison_result_channel(
                    WIDTH,
                    HEIGHT,
                    (BATCHES, BATCHES),
                    (&cpu_means, &gpu_xyz),
                    channel,
                ),
            ));
        }
    }
    results
}

fn batch_means(pixel_major_samples: &[Vec3], samples_per_batch: u32) -> Vec<Vec3> {
    assert_eq!(pixel_major_samples.len() % samples_per_batch as usize, 0);
    pixel_major_samples
        .chunks_exact(samples_per_batch as usize)
        .map(|samples| samples.iter().copied().sum::<Vec3>() / samples_per_batch as f32)
        .collect()
}

fn pixel_major_xyz(batch_major_means: &[Vec3], pixels: usize) -> Vec<f32> {
    assert_eq!(batch_major_means.len() % pixels, 0);
    let batches = batch_major_means.len() / pixels;
    let mut xyz = Vec::with_capacity(batch_major_means.len() * 3);
    for pixel in 0..pixels {
        for batch in 0..batches {
            xyz.extend_from_slice(&batch_major_means[batch * pixels + pixel].to_array());
        }
    }
    xyz
}

#[cfg(test)]
mod tests {
    use super::{batch_means, pixel_major_xyz};
    use glam::Vec3;

    #[test]
    fn cpu_batch_means_preserve_pixel_and_batch_order() {
        let samples = [2.0, 4.0, 6.0, 8.0, 20.0, 40.0, 60.0, 80.0].map(Vec3::splat);
        assert_eq!(
            batch_means(&samples, 2),
            [3.0, 7.0, 30.0, 70.0].map(Vec3::splat),
        );
    }

    #[test]
    fn gpu_batches_are_transposed_without_mixing_xyz_channels() {
        let batches = [
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(10.0, 20.0, 30.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(40.0, 50.0, 60.0),
        ];
        assert_eq!(
            pixel_major_xyz(&batches, 2),
            [
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0
            ],
        );
    }
}
