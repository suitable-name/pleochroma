//! Hardware tests of the HDR map's yaw (`EnvironmentSource::HdrMap::rotation`) on the GPU
//! frame renderer: the megakernel and the wavefront pipeline read it from the transport
//! uniform (`GpuTransportParams::hdr_rot_cos`/`hdr_rot_sin`) at every lookup and every
//! importance draw. Every test acquires its own renderer through
//! [`renderer_or_skip`](crate::renderer::gpu::merge_tests::renderer_or_skip) (a clean skip
//! without an adapter, a failure with `PLEOCHROMA_REQUIRE_GPU=1`). Run with:
//!
//! ```text
//! cargo test -p pleochroma --features gpu -- hdr_rotation_hardware_tests
//! ```

use std::f32::consts::TAU;

use glam::Vec3;

use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, Estimator, FacetFinish, add_finite_sample, build_plane_soa,
            pixel_rotations, sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::{
        env_map::{EnvRotation, EnvironmentMap},
        gpu::{
            merge_tests::renderer_or_skip,
            z_stats::{Welford, batch_frames_agree, batch_mean_stats},
        },
    },
};

use super::{GpuFrameRenderer, GpuFrameScene, GpuPipelineKind};

/// Frame edge in pixels.
const SIZE: u32 = 16;

/// Samples per pixel of every frame.
const SPP: u32 = 4;

/// Batches of a frosted frame compared in expectation: 48 batch means per pixel keep the
/// z-test's Student tails close to the normal ones.
const FROSTED_BATCHES: u32 = 48;

/// Samples per pixel of one such batch.
const FROSTED_BATCH_SPP: u32 = 8;

/// A sky whose radiance changes with the azimuth everywhere, so any wrong turn shows.
fn sky_map() -> EnvironmentMap {
    let (width, height) = (32_usize, 16_usize);
    let texels = (0..width * height)
        .map(|i| {
            let u = (i % width) as f32 / width as f32;
            let v = (i / width) as f32 / height as f32;
            [0.2 + u, 0.5f32.mul_add(v, 0.3), u.mul_add(v, 0.1)]
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// `map` with every row moved `columns` texels toward larger `u` (wrapping).
fn shifted(map: &EnvironmentMap, columns: usize) -> EnvironmentMap {
    let (width, height) = (map.width(), map.height());
    let texels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (y, x)))
        .map(|(y, x)| map.texels()[y * width + (x + width - columns) % width])
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// The fixed scene pieces of a test: camera, stone and material.
struct Fixture {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
}

impl Fixture {
    fn new() -> Self {
        Self {
            camera: Camera::new(0.35, 0.28, 5.0, 18.0),
            planes: StandardGemCuts::standard_round_brilliant(),
            material: GemMaterial::diamond(),
        }
    }

    fn scene<'a>(
        &'a self,
        finishes: &'a [FacetFinish],
        environment: EnvironmentSource<'a>,
    ) -> GpuFrameScene<'a> {
        GpuFrameScene {
            camera: &self.camera,
            width: SIZE,
            height: SIZE,
            planes: &self.planes,
            facet_finishes: finishes,
            material: &self.material,
            max_bounces: 8,
            environment,
        }
    }

    /// The GPU frame of samples `0..SPP`.
    fn gpu_frame(
        &self,
        renderer: &mut GpuFrameRenderer,
        finishes: &[FacetFinish],
        environment: EnvironmentSource<'_>,
    ) -> Vec<Vec3> {
        let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
        renderer
            .accumulate(&self.scene(finishes, environment), 0, SPP, &mut accum)
            .expect("a small HDR frame renders");
        accum
    }

    /// Per-pixel batch statistics of samples `first..first + FROSTED_BATCHES *
    /// FROSTED_BATCH_SPP`, one `accumulate` call per batch.
    fn gpu_batch_stats(
        &self,
        renderer: &mut GpuFrameRenderer,
        finishes: &[FacetFinish],
        environment: EnvironmentSource<'_>,
        first: u32,
    ) -> Vec<[Welford; 3]> {
        let batches: Vec<Vec<Vec3>> = (0..FROSTED_BATCHES)
            .map(|k| {
                let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
                renderer
                    .accumulate(
                        &self.scene(finishes, environment),
                        first + k * FROSTED_BATCH_SPP,
                        FROSTED_BATCH_SPP,
                        &mut accum,
                    )
                    .expect("a small HDR frame renders");
                accum
            })
            .collect();
        batch_mean_stats(&batches, FROSTED_BATCH_SPP)
    }

    /// The CPU tracer's frame of samples `0..SPP`, traced as `renderer::cpu_frame` does.
    fn cpu_frame(&self, environment: EnvironmentSource<'_>) -> Vec<Vec3> {
        let plane_soa = build_plane_soa(&self.planes);
        let prepared = PreparedMaterial::new(&self.material).with_estimator(Estimator::Branching);
        (0..SIZE * SIZE)
            .map(|pixel| {
                let rot = pixel_rotations(pixel);
                let mut sum = Vec3::ZERO;
                for sample in 0..SPP {
                    let d = sample_draws(pixel, sample, &rot);
                    let ray = self.camera.generate_ray(
                        (pixel % SIZE) as f32,
                        (pixel / SIZE) as f32,
                        SIZE as f32,
                        SIZE as f32,
                        d.jitter_x,
                        d.jitter_y,
                    );
                    let xyz = trace_spectral_ray_prepared(
                        ray,
                        StoneGeometry {
                            planes: &self.planes,
                            tools: &[],
                        },
                        &plane_soa,
                        &[],
                        &prepared,
                        Fluorescence::none(),
                        8,
                        environment,
                        d.seed,
                        d.hero_rand,
                        None,
                        SurfaceScene::IDEAL,
                    );
                    add_finite_sample(&mut sum, xyz);
                }
                sum
            })
            .collect()
    }
}

/// How many pixels of `a` differ from `b` by more than `relative` of the larger component.
fn differing(a: &[Vec3], b: &[Vec3], relative: f32) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| {
            let scale = y.abs().max_element().max(1e-3);
            (**x - **y).abs().max_element() > relative * scale
        })
        .count()
}

/// The median over the pixels of the relative difference of two frames' Y sums.
fn median_relative_difference(a: &[Vec3], b: &[Vec3]) -> f32 {
    let mut diffs: Vec<f32> = a
        .iter()
        .zip(b)
        .map(|(a, b)| (a.y - b.y).abs() / a.y.abs().max(b.y.abs()).max(1e-6))
        .collect();
    diffs.sort_by(f32::total_cmp);
    diffs[diffs.len() / 2]
}

/// The pipelines a test covers, each set on `renderer` and skipped when the device falls back.
fn pipelines(renderer: &mut GpuFrameRenderer, mut each: impl FnMut(&mut GpuFrameRenderer, &str)) {
    for (kind, label) in [
        (GpuPipelineKind::Megakernel, "megakernel"),
        (GpuPipelineKind::Wavefront, "wavefront"),
    ] {
        renderer.set_pipeline_kind(kind);
        if renderer.pipeline_kind() != kind {
            println!("skipping the {label} pipeline: this device falls back to the megakernel");
            continue;
        }
        each(renderer, label);
    }
}

/// A map turned by a whole number of texel columns is the map with its columns shifted, on both
/// pipelines.
///
/// A polished stone reads the map only where its paths escape, so the two frames match pixel
/// for pixel. A frosted stone also draws its exterior NEE directions out of the map's importance
/// distribution, and the shifted map's distribution hands the same random numbers other texels:
/// its two frames match in expectation only, which per-pixel z-tests over disjoint sample ranges
/// check (the turned map's draws and pdfs must turn with it, or the frames part).
#[test]
fn a_whole_column_turn_equals_shifted_columns_on_the_gpu() {
    let name = "a_whole_column_turn_equals_shifted_columns_on_the_gpu";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    let fixture = Fixture::new();
    let map = sky_map();
    let columns = 5;
    let yaw = TAU * columns as f32 / map.width() as f32;
    let shifted_map = shifted(&map, columns);
    let frosted = vec![FacetFinish::Frosted; fixture.planes.len()];
    let turned = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
    let moved = EnvironmentSource::hdr(&shifted_map);
    let unturned = EnvironmentSource::hdr(&map);
    pipelines(&mut renderer, |renderer, label| {
        let a = fixture.gpu_frame(renderer, &[], turned);
        let b = fixture.gpu_frame(renderer, &[], moved);
        let plain = fixture.gpu_frame(renderer, &[], unturned);
        let outliers = differing(&a, &b, 2e-3);
        assert!(
            outliers <= a.len() / 20,
            "{label} (polished): {outliers} of {} pixels differ between the turned map and the \
             shifted columns",
            a.len()
        );
        assert!(
            differing(&plain, &b, 1e-2) > a.len() / 10,
            "{label}: the fixture must show the turn: the unturned map renders differently"
        );

        let span = FROSTED_BATCHES * FROSTED_BATCH_SPP;
        let a = fixture.gpu_batch_stats(renderer, &frosted, turned, 0);
        let b = fixture.gpu_batch_stats(renderer, &frosted, moved, span);
        let plain = fixture.gpu_batch_stats(renderer, &frosted, unturned, 2 * span);
        let (agree, summary) = batch_frames_agree(&a, &b, SIZE, SIZE);
        assert!(
            agree,
            "{label} (frosted): the turned map and the shifted columns differ beyond noise: \
             {summary}"
        );
        let (alike, summary) = batch_frames_agree(&plain, &b, SIZE, SIZE);
        assert!(
            !alike,
            "{label} (frosted): the fixture must show the turn, yet the unturned map is within \
             noise: {summary}"
        );
    });
}

/// An identity yaw (0, -0, a whole turn, `EnvRotation::IDENTITY`) renders the default frame bit
/// for bit, and a real yaw does not.
#[test]
fn an_identity_yaw_renders_the_default_gpu_frame_bit_for_bit() {
    let name = "an_identity_yaw_renders_the_default_gpu_frame_bit_for_bit";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    let fixture = Fixture::new();
    let map = sky_map();
    let frosted = vec![FacetFinish::Frosted; fixture.planes.len()];
    pipelines(&mut renderer, |renderer, label| {
        for finishes in [&[][..], &frosted[..]] {
            let base = fixture.gpu_frame(renderer, finishes, EnvironmentSource::hdr(&map));
            for yaw in [0.0f32, -0.0, TAU] {
                let source =
                    EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
                let frame = fixture.gpu_frame(renderer, finishes, source);
                assert!(
                    frame
                        .iter()
                        .zip(&base)
                        .all(|(a, b)| a.to_array().map(f32::to_bits)
                            == b.to_array().map(f32::to_bits)),
                    "{label} ({} frosted facets): yaw {yaw} changed the frame",
                    finishes.len()
                );
            }
            let source = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::IDENTITY);
            let frame = fixture.gpu_frame(renderer, finishes, source);
            assert!(
                frame
                    .iter()
                    .zip(&base)
                    .all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)),
                "{label}: the identity rotation changed the frame"
            );
            let turned = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(0.9));
            let moved = fixture.gpu_frame(renderer, finishes, turned);
            assert!(
                differing(&moved, &base, 1e-3) > base.len() / 10,
                "{label} ({} frosted facets): a turned map must change the frame",
                finishes.len()
            );
        }
    });
}

/// The GPU follows the CPU tracer sample for sample under a turned map as closely as it does
/// under the unturned one: the median relative difference of a pixel is at most three times the
/// unturned twin's plus 1e-3.
#[test]
fn a_turned_hdr_gpu_frame_follows_the_cpu_frame() {
    let Some(mut renderer) = renderer_or_skip("a_turned_hdr_gpu_frame_follows_the_cpu_frame")
    else {
        return;
    };
    renderer.set_estimator(Some(Estimator::Branching));
    let fixture = Fixture::new();
    let map = sky_map();
    let plain_source = EnvironmentSource::hdr_at(&map, 1.5);
    let turned_source = plain_source.with_hdr_rotation(EnvRotation::from_yaw(1.3));
    let cpu_plain = fixture.cpu_frame(plain_source);
    let cpu_turned = fixture.cpu_frame(turned_source);
    pipelines(&mut renderer, |renderer, label| {
        let gpu_plain = fixture.gpu_frame(renderer, &[], plain_source);
        let gpu_turned = fixture.gpu_frame(renderer, &[], turned_source);
        let twin_plain = median_relative_difference(&gpu_plain, &cpu_plain);
        let twin_turned = median_relative_difference(&gpu_turned, &cpu_turned);
        let crossed = median_relative_difference(&gpu_turned, &cpu_plain);
        println!(
            "hdr rotation twin -- {label}: GPU against CPU, median relative difference of a \
             pixel: unturned {twin_plain:.3e}, turned {twin_turned:.3e}, turned GPU against \
             unturned CPU {crossed:.3e}"
        );
        assert!(
            twin_turned <= 3.0f32.mul_add(twin_plain, 1e-3),
            "{label}: under a turned map the GPU is {twin_turned:.3e} from the CPU, under the \
             unturned one {twin_plain:.3e}"
        );
        assert!(
            crossed > 10.0 * twin_turned.max(1e-4),
            "{label}: the fixture must show the turn ({crossed:.3e} against {twin_turned:.3e})"
        );
    });
}
