//! Hardware tests of exit collection on the megakernel: the GPU collects the leaves the CPU
//! tracer collects. Every test acquires its own renderer through
//! [`renderer_or_skip`](crate::renderer::gpu::merge_tests::renderer_or_skip) (a clean skip without
//! an adapter, a failure with `PLEOCHROMA_REQUIRE_GPU=1`).

use glam::Vec3;

use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, Estimator, LightingPreset, add_finite_sample, build_plane_soa,
            intersect_polyhedron, pixel_rotations, sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::gpu::merge_tests::renderer_or_skip,
};

use super::GpuFrameScene;

/// Frame edge in pixels.
const SIZE: u32 = 32;
/// Samples per pixel of every frame.
const SPP: u32 = 64;

/// The CPU tracer's frame of `scene` (per-pixel sums of samples `0..SPP`) with `estimator`,
/// traced through the plain entry exactly as `renderer::cpu_frame` traces a pixel.
fn cpu_frame(scene: &GpuFrameScene<'_>, estimator: Estimator) -> Vec<Vec3> {
    let plane_soa = build_plane_soa(scene.planes);
    let prepared = PreparedMaterial::new(scene.material).with_estimator(estimator);
    (0..scene.width * scene.height)
        .map(|pixel| {
            let rot = pixel_rotations(pixel);
            let mut sum = Vec3::ZERO;
            for sample in 0..SPP {
                let d = sample_draws(pixel, sample, &rot);
                let ray = scene.camera.generate_ray(
                    (pixel % scene.width) as f32,
                    (pixel / scene.width) as f32,
                    scene.width as f32,
                    scene.height as f32,
                    d.jitter_x,
                    d.jitter_y,
                );
                let xyz = trace_spectral_ray_prepared(
                    ray,
                    StoneGeometry {
                        planes: scene.planes,
                        tools: &[],
                    },
                    &plane_soa,
                    &[],
                    &prepared,
                    Fluorescence::none(),
                    scene.max_bounces,
                    scene.environment,
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

/// The median, over the stone pixels, of the relative difference of two frames' Y sums.
fn median_relative_difference(a: &[Vec3], b: &[Vec3], stone: &[bool]) -> f32 {
    let mut diffs: Vec<f32> = a
        .iter()
        .zip(b)
        .zip(stone)
        .filter(|&(_, &on)| on)
        .map(|((a, b), _)| (a.y - b.y).abs() / a.y.abs().max(b.y.abs()).max(1e-6))
        .collect();
    diffs.sort_by(f32::total_cmp);
    diffs[diffs.len() / 2]
}

/// The sum over the stone pixels of the difference of two frames' Y, and the root of the sum of
/// its squares.
///
/// The pixels are independent, and for two unbiased frames of one scene every pixel's difference
/// has mean zero, so the second number estimates the standard deviation of the first whatever
/// the picture looks like.
fn paired_light_difference(a: &[Vec3], b: &[Vec3], stone: &[bool]) -> (f64, f64) {
    let (mut sum, mut squares) = (0.0f64, 0.0f64);
    for ((a, b), _) in a.iter().zip(b).zip(stone).filter(|&(_, &on)| on) {
        let difference = f64::from(a.y) - f64::from(b.y);
        sum += difference;
        squares = difference.mul_add(difference, squares);
    }
    (sum, squares.sqrt())
}

/// With exit collection the megakernel follows the CPU tracer sample for sample, as it does
/// with the branching estimator, and it stays unbiased.
///
/// For an isotropic stone (Diamond: the generic arm's entry and interior events) and a uniaxial
/// one (Zircon: the closed-form entry and internal arms, the mode split and the mode coupling)
/// the same samples are traced on the GPU and on the CPU, branching and collected. The branching
/// pair is the yardstick of how closely the two engines agree per pixel; the collected pair must
/// agree as closely (three times the yardstick plus 1e-3). When the branching pair agrees sample
/// for sample (its median difference is under a quarter of the difference between a collected
/// and a branching frame, which is the branching estimator's noise), the collected pair must
/// too: a GPU that lost a leaf, a weight or the glare scale would differ on every stone pixel.
/// The switch must change the GPU frame, and the light of the two GPU frames on the stone must
/// differ by at most four standard deviations of the paired per-pixel differences (both are
/// unbiased).
#[test]
fn the_megakernel_collects_the_cpu_leaves() {
    let Some(mut renderer) = renderer_or_skip("the_megakernel_collects_the_cpu_leaves") else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let stone: Vec<bool> = (0..SIZE * SIZE)
        .map(|pixel| {
            let ray = camera.generate_ray(
                (pixel % SIZE) as f32,
                (pixel / SIZE) as f32,
                SIZE as f32,
                SIZE as f32,
                0.0,
                0.0,
            );
            intersect_polyhedron(ray, &planes).is_some()
        })
        .collect();
    assert!(
        stone.iter().filter(|&&on| on).count() >= 64,
        "the stone covers too few pixels of the test frame"
    );
    for name in ["Diamond", "Zircon"] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let scene = GpuFrameScene {
            camera: &camera,
            width: SIZE,
            height: SIZE,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
            environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        };
        let mut gpu_frame = |estimator: Estimator| {
            renderer.set_estimator(Some(estimator));
            let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
            renderer
                .accumulate(&scene, 0, SPP, &mut accum)
                .expect("a small frame renders");
            accum
        };
        let gpu_branching = gpu_frame(Estimator::Branching);
        let gpu_collect = gpu_frame(Estimator::ExitCollection);
        assert!(
            gpu_branching.iter().zip(&gpu_collect).any(|(a, b)| a != b),
            "{name}: exit collection changed no pixel of the GPU frame: the switch is ignored"
        );
        let (light_gap, light_noise) =
            paired_light_difference(&gpu_collect, &gpu_branching, &stone);
        assert!(
            light_gap.abs() <= 4.0 * light_noise,
            "{name}: the collected GPU frame holds {light_gap:.4e} more light on the stone than \
             the branching one, against a noise of {light_noise:.4e}: exit collection is biased \
             on the GPU"
        );
        let cpu_branching = cpu_frame(&scene, Estimator::Branching);
        let cpu_collect = cpu_frame(&scene, Estimator::ExitCollection);
        let twin_branching = median_relative_difference(&gpu_branching, &cpu_branching, &stone);
        let twin_collect = median_relative_difference(&gpu_collect, &cpu_collect, &stone);
        let crossed = median_relative_difference(&gpu_collect, &cpu_branching, &stone);
        println!(
            "collect twin -- {name}: GPU against CPU, median relative difference of a stone \
             pixel: branching {twin_branching:.3e}, collected {twin_collect:.3e}, collected \
             against branching {crossed:.3e}; light collected minus branching {light_gap:.3e} \
             against a noise of {light_noise:.3e}"
        );
        assert!(
            twin_collect <= 3.0f32.mul_add(twin_branching, 1e-3),
            "{name}: with exit collection the GPU is {twin_collect:.3e} from the CPU, with the \
             branching estimator {twin_branching:.3e}: the GPU does not collect the CPU's leaves"
        );
        if twin_branching < 0.25 * crossed {
            assert!(
                twin_collect < 0.25 * crossed,
                "{name}: the branching frames agree sample for sample ({twin_branching:.3e} \
                 against a noise of {crossed:.3e}) but the collected frames do not \
                 ({twin_collect:.3e})"
            );
        } else {
            println!(
                "collect twin -- {name}: the branching GPU and CPU frames differ by Monte Carlo \
                 noise already; the sample-for-sample check is skipped"
            );
        }
    }
}
