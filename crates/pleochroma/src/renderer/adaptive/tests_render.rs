//! Tests of the CPU adaptive render on a small real scene.

use glam::Vec3;

use super::{AdaptiveImage, AdaptiveSettings, LiveTiles, NoiseMetric};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, Camera, LightingPreset, build_plane_soa, surface::SurfaceScene,
        },
    },
    renderer::{
        cpu_frame::{
            render_adaptive_surface, trace_live_tiles_surface, trace_pixels_interleaved_surface,
        },
        frame_scene::FrameScene,
    },
    simd::PlanesSoA32,
};

const SIZE: u32 = 16;

/// Runs `f` on a 16 x 16 round brilliant in diamond under the light tent.
fn with_scene<R>(f: impl FnOnce(&FrameScene<'_>, &PlanesSoA32) -> R) -> R {
    let planes = StandardGemCuts::standard_round_brilliant();
    let soa = build_plane_soa(&planes);
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let scene = FrameScene {
        camera: &camera,
        width: SIZE,
        height: SIZE,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::LightTent
            .studio(1.0, 0.4, 0.35)
            .with_backdrop(BACKDROP_GREY),
    };
    f(&scene, &soa)
}

fn extras() -> (
    &'static [ToolPrimitive],
    &'static Fluorescence,
    SurfaceScene<'static>,
) {
    (&[], Fluorescence::none(), SurfaceScene::IDEAL)
}

fn bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
        .collect()
}

#[test]
fn live_tiles_all_trace_what_the_interleaved_tracer_traces() {
    with_scene(|scene, soa| {
        let pixels = (SIZE * SIZE) as usize;
        let (first, spp) = (3, 6);
        let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
        let live = LiveTiles::all(SIZE, SIZE, 8);
        trace_live_tiles_surface(scene, extras(), soa, &live, first, spp, &mut sum, &mut half);

        let reference = trace_pixels_interleaved_surface(scene, extras(), soa, 0, 1, first, spp);
        assert_eq!(bits(&sum), bits(&reference));

        // The odd-index half is a trace of only the odd samples (3 and 5), added in order.
        let mut odd = vec![Vec3::ZERO; pixels];
        for sample in (first..first + spp).filter(|s| !s.is_multiple_of(2)) {
            let one = trace_pixels_interleaved_surface(scene, extras(), soa, 0, 1, sample, 1);
            for (acc, value) in odd.iter_mut().zip(one) {
                *acc += value;
            }
        }
        assert_eq!(bits(&half), bits(&odd));
        assert!(sum.iter().any(|v| v.length_squared() > 0.0), "a real image");
    });
}

#[test]
fn pixels_outside_the_live_tiles_are_untouched() {
    with_scene(|scene, soa| {
        let pixels = (SIZE * SIZE) as usize;
        let (mut sum, mut half) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
        // Tiles 0 (top-left) and 3 (bottom-right) of 2 x 2 tiles.
        let live = LiveTiles::from_indices(SIZE, SIZE, 8, vec![0, 3]);
        trace_live_tiles_surface(scene, extras(), soa, &live, 0, 4, &mut sum, &mut half);
        let reference = trace_pixels_interleaved_surface(scene, extras(), soa, 0, 1, 0, 4);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let index = (y * SIZE + x) as usize;
                let is_live = (x < 8) == (y < 8);
                if is_live {
                    assert_eq!(bits(&sum[index..=index]), bits(&reference[index..=index]));
                } else {
                    assert_eq!(sum[index], Vec3::ZERO);
                    assert_eq!(half[index], Vec3::ZERO);
                }
            }
        }
    });
}

#[test]
fn buffers_of_the_wrong_size_are_left_alone() {
    with_scene(|scene, soa| {
        let live = LiveTiles::all(SIZE, SIZE, 8);
        let (mut sum, mut half) = (vec![Vec3::ZERO; 10], vec![Vec3::ZERO; 10]);
        trace_live_tiles_surface(scene, extras(), soa, &live, 0, 2, &mut sum, &mut half);
        assert!(sum.iter().chain(&half).all(|v| *v == Vec3::ZERO));
    });
}

fn settings(target_noise: f32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 8,
        max_spp: 64,
        pass_spp: 8,
        tile: 8,
    }
}

/// Settings under which no tile can stop: the first check would be at the cap itself.
fn never_stops() -> AdaptiveSettings {
    AdaptiveSettings {
        min_spp: 64,
        ..settings(1e-9)
    }
}

#[test]
fn two_adaptive_renders_with_the_same_inputs_are_byte_identical() {
    with_scene(|scene, soa| {
        let render = || {
            render_adaptive_surface(scene, extras(), soa, &settings(0.35), 0, 1.0)
                .expect("valid settings")
        };
        let (first, second) = (render(), render());
        assert_eq!(bits(&first.sum), bits(&second.sum));
        assert_eq!(bits(&first.half_b), bits(&second.half_b));
        assert_eq!(bits(&first.pixels), bits(&second.pixels));
        assert_eq!(first.counts, second.counts);
        assert_eq!(first.samples_traced, second.samples_traced);
    });
}

#[test]
fn an_invalid_setting_is_an_error_not_a_render() {
    with_scene(|scene, soa| {
        let bad = AdaptiveSettings {
            tile: 12,
            ..settings(0.35)
        };
        assert!(render_adaptive_surface(scene, extras(), soa, &bad, 0, 1.0).is_err());
    });
}

#[test]
fn a_target_that_is_never_met_equals_the_fixed_render() {
    with_scene(|scene, soa| {
        let never = render_adaptive_surface(scene, extras(), soa, &never_stops(), 0, 1.0)
            .expect("valid settings");
        assert!(never.counts.iter().all(|&c| c == 64));
        let fixed = trace_pixels_interleaved_surface(scene, extras(), soa, 0, 1, 0, 64);
        // Eight passes of eight, added up, against one sum of 64: equal up to float rounding.
        for (adaptive, reference) in never.sum.iter().zip(&fixed) {
            let tolerance = 1e-4 * reference.abs().max_element().max(1.0);
            assert!((*adaptive - *reference).abs().max_element() <= tolerance);
        }
    });
}

/// Squared standard error of pixel `index`'s luminance mean, from its two halves.
///
/// `(A - B)^2 / 4` is unbiased for the variance of the full mean when the halves hold the even
/// and odd samples of an even-offset run.
fn variance_of_mean(image: &AdaptiveImage, index: usize) -> f64 {
    let n = f64::from(image.counts[index]);
    let n_b = (n / 2.0).floor();
    let b = f64::from(image.half_b[index].y) / n_b;
    let a = f64::from(image.sum[index].y - image.half_b[index].y) / (n - n_b);
    ((a - b) / 2.0).powi(2)
}

#[test]
fn adaptive_and_fixed_renders_agree_within_the_z_tolerance() {
    with_scene(|scene, soa| {
        let adaptive = render_adaptive_surface(scene, extras(), soa, &settings(0.35), 0, 1.0)
            .expect("valid settings");
        // A reference that never stops, on samples 64..128: independent of the adaptive one.
        let reference = render_adaptive_surface(scene, extras(), soa, &never_stops(), 64, 1.0)
            .expect("valid settings");
        let full = u64::from(SIZE * SIZE) * 64;
        assert!(adaptive.samples_traced <= full);

        let mut difference = 0.0_f64;
        let mut variance = 0.0_f64;
        for index in 0..(SIZE * SIZE) as usize {
            difference += f64::from(adaptive.pixels[index].y - reference.pixels[index].y);
            variance += variance_of_mean(&adaptive, index) + variance_of_mean(&reference, index);
        }
        assert!(variance > 0.0, "the scene has noise");
        let z = difference / variance.sqrt();
        assert!(
            z.abs() < 4.0,
            "z = {z} (difference {difference}, samples {} of {full})",
            adaptive.samples_traced
        );
    });
}
