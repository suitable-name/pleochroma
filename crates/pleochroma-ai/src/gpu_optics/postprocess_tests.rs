//! Source-parity definitions only; no CUDA execution was performed while authoring.
use super::{
    diagnostics::Counters, fan::FanGeometry, lighting::ExitLighting, postprocess::Postprocess,
    prepare, reduce, tone_cuda::ToneCuda,
};
use cudarc::driver::CudaContext;
use glam::Vec3;
use pleochroma::{
    color::metrics::{ToneIlluminant, camera_view_basis},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_cut_core::optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW};
use std::sync::Arc;

fn records(modes: usize) -> Vec<f32> {
    let mut records = Vec::new();
    for ray in 0..96 {
        for mode in 0..modes {
            let angle = (ray as f32) * 0.37;
            let dir = Vec3::new(
                angle.sin(),
                0.15 + ((ray + mode) % 11) as f32 * 0.07,
                angle.cos(),
            )
            .normalize();
            let fate = if ray % 17 == 0 {
                0.0
            } else if (ray + mode) % 9 == 0 {
                2.0
            } else if (ray + mode) % 13 == 0 {
                3.0
            } else {
                4.0
            };
            let length = 0.2 + (ray % 17) as f32 * 0.31;
            records.extend([
                fate,
                0.65 + (ray % 7) as f32 * 0.04,
                length,
                length * 0.2,
                length * 0.3,
                length * 0.5,
                dir.x,
                dir.y,
                dir.z,
                2.0,
                0.8,
                2.0,
            ]);
        }
    }
    records
}

#[test]
#[ignore = "requires native CUDA/NVRTC; definitions only, no execution during implementation"]
fn native_lighting_counts_and_ordered_tone_compaction_match_source() {
    let context = CudaContext::new(0).unwrap_or_else(|e| panic!("{e}"));
    let stream = context.default_stream();
    let mut post = Postprocess::new(&context, Arc::clone(&stream), Arc::new(Counters::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    for preset in LightingPreset::ALL {
        for material in [
            GemMaterial::diamond().with_body_color([0.2, 0.4, 0.1]),
            GemMaterial::sapphire(),
        ] {
            let modes = if material.crystal_system
                != pleochroma::optics::materials::CrystalSystem::Cubic
                && material.birefringence_delta.abs() > 1e-4
            {
                2
            } else {
                1
            };
            let raw = records(modes);
            let fan = FanGeometry::from_half_width(1.0);
            let poses = prepare::pose_data(&[(0.0, 90.0f32.to_radians())], fan, true);
            let descriptor = [
                0,
                (raw.len() / 12) as u32,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                post.light_offset(preset).unwrap_or_else(|e| panic!("{e}")),
                modes as u32,
            ];
            let environment = preset.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
            let lighting = ExitLighting::new(environment);
            let reference = reduce::reduce(
                &raw,
                &material,
                -camera_view_basis(0.0, 90.0f32.to_radians()).0,
                &lighting,
                1.0,
                true,
                modes,
            )
            .unwrap_or_else(|e| panic!("{e}"));
            let mut device = stream.clone_htod(&raw).unwrap_or_else(|e| panic!("{e}"));
            let poses = stream.clone_htod(&poses).unwrap_or_else(|e| panic!("{e}"));
            let descriptors = stream
                .clone_htod(&descriptor)
                .unwrap_or_else(|e| panic!("{e}"));
            let actual = post
                .classify_reduce(
                    &mut device,
                    &poses,
                    &descriptors,
                    1,
                    (raw.len() / 12) as u32,
                )
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                actual[0].map(f32::to_bits),
                [
                    reference.metrics.brilliance_pct,
                    reference.metrics.extinction_pct,
                    reference.metrics.windowing_pct
                ]
                .map(f32::to_bits)
            );
            check_tone(
                &context,
                &stream,
                &mut post,
                &device,
                reference
                    .tone
                    .unwrap_or_else(|| panic!("reference tone missing")),
                &material,
                preset,
            );
        }
    }
}

#[test]
#[ignore = "requires native CUDA/NVRTC; invalid-ray propagation fixture, not executed during implementation"]
fn native_metric_reduction_reports_invalid_device_records() {
    let context = CudaContext::new(0).unwrap_or_else(|e| panic!("{e}"));
    let stream = context.default_stream();
    let mut post = Postprocess::new(&context, Arc::clone(&stream), Arc::new(Counters::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut raw = records(1);
    raw[12 + 7] = f32::NAN;
    let mut device = stream.clone_htod(&raw).unwrap_or_else(|e| panic!("{e}"));
    let poses = stream
        .clone_htod(&prepare::pose_data(
            &[(0.0, 90.0f32.to_radians())],
            FanGeometry::from_half_width(1.0),
            true,
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    let d = [
        0,
        (raw.len() / 12) as u32,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        post.light_offset(LightingPreset::LightTent)
            .unwrap_or_else(|e| panic!("{e}")),
        1,
    ];
    let descriptors = stream.clone_htod(&d).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        post.classify_reduce(
            &mut device,
            &poses,
            &descriptors,
            1,
            (raw.len() / 12) as u32
        )
        .is_err()
    );
}

fn check_tone(
    context: &Arc<CudaContext>,
    stream: &Arc<cudarc::driver::CudaStream>,
    post: &mut Postprocess,
    device: &cudarc::driver::CudaSlice<f32>,
    accumulator: super::tone::ToneAccumulator,
    material: &GemMaterial,
    preset: LightingPreset,
) {
    let modes = if material.crystal_system != pleochroma::optics::materials::CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        2
    } else {
        1
    };
    let mut input = accumulator.cuda_input(material);
    let config = [
        0.0,
        (device.len() / 12) as f64,
        modes as f64,
        f64::from(12.0f32 / 128.0),
        input.scale,
        if input.principal { 1.0 } else { 0.0 },
    ];
    let summary = post
        .compact_tones(&device, &[config])
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary[0][4] as usize, input.paths.len());
    assert_eq!(summary[0][5].to_bits(), input.denominator.to_bits());
    let compacted = stream
        .clone_dtoh(&post.paths().slice(..input.paths.len() * 5))
        .unwrap_or_else(|e| panic!("{e}"));
    for (actual, expected) in compacted.as_chunks::<5>().0.iter().zip(&input.paths) {
        assert_eq!(&actual[..4], expected);
        assert_eq!(actual[4], input.scale);
    }
    input.denominator = summary[0][5];
    let mut cuda =
        ToneCuda::new(Arc::clone(context), Arc::clone(stream)).unwrap_or_else(|e| panic!("{e}"));
    let expected = cuda
        .spectra(&[accumulator.cuda_input(material)])
        .unwrap_or_else(|e| panic!("{e}"));
    let inputs = [input];
    let actual = cuda
        .spectra_resident(&inputs, post.paths(), &[summary[0][4] as u32])
        .unwrap_or_else(|e| panic!("{e}"));
    for (a, b) in actual[0].iter().zip(expected[0]) {
        assert!((a - b).abs() <= 1e-12 + 1e-12 * b.abs());
    }
    let illuminant = ToneIlluminant::for_environment(preset.studio(
        1.0,
        CANONICAL_LIGHT_YAW,
        CANONICAL_LIGHT_PITCH,
    ));
    let expected = accumulator.finish(material, illuminant);
    assert_eq!(
        super::tone::ToneAccumulator::from_gpu_summary(&summary[0])
            .finish_from_spectrum(&actual[0], illuminant)
            .returned_fraction
            .to_bits(),
        expected.returned_fraction.to_bits()
    );
    let band_spectrum = cuda
        .spectra_materials(&inputs, post.paths(), &[summary[0][4] as u32], &[material])
        .unwrap_or_else(|e| panic!("{e}"));
    // f32 band evaluation has a separate narrow regression budget; this is not
    // a global error bound and cannot certify a discontinuous policy decision.
    for (a, b) in band_spectrum[0].iter().zip(actual[0]) {
        assert!((a - b).abs() <= 1e-5 + 1e-5 * b.abs());
    }
}
