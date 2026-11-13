//! Tests of the re-shade: it is deterministic, it equals the mean of phase B under the same
//! lighting, and a lighting or exposure change keeps the cache and changes the image.

use std::sync::atomic::AtomicBool;

use glam::{Mat4, Vec3};

use super::{
    BuildSettings, COMBS, CacheKey, CacheOptions, PathCacheFrame, ReshadeError, Reshaded,
    comb_hero_rand, reshade,
    tests_shade::{MAX_BOUNCES, SIZE, STONE_PIXELS, assert_z_stats, smooth_environment},
    tests_support::{Rig, glass},
};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            BACKDROP_GREY, EnvironmentSource, LightingPreset, pixel_rotations,
            surface::SurfaceScene,
        },
    },
    renderer::frame_scene::FrameScene,
};

/// The scene of `rig` under `environment`.
fn scene_under<'a>(rig: &'a Rig, environment: EnvironmentSource<'a>) -> FrameScene<'a> {
    FrameScene {
        environment,
        ..rig.scene(SIZE, MAX_BOUNCES)
    }
}

/// The path cache of `scene` (an eligible plain stone).
fn cache_of<'s, 'a>(scene: &'s FrameScene<'a>, rig: &'s Rig) -> PathCacheFrame<'s, 'a> {
    PathCacheFrame::new(
        scene,
        &rig.soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        CacheOptions::default(),
    )
    .expect("a plain glass stone may use the cache")
}

fn daylight(exposure: f32) -> EnvironmentSource<'static> {
    LightingPreset::Daylight
        .studio(exposure, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

fn light_tent(exposure: f32) -> EnvironmentSource<'static> {
    LightingPreset::LightTent
        .studio(exposure, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

fn bits(image: &Reshaded) -> Vec<[u32; 3]> {
    image
        .xyz
        .iter()
        .map(|value| value.to_array().map(f32::to_bits))
        .collect()
}

#[test]
fn the_wavelength_set_is_six_combs_of_eight_a_slice_apart() {
    assert_eq!(COMBS, 6);
    let heroes: Vec<f32> = (0..COMBS).map(comb_hero_rand).collect();
    for pair in heroes.windows(2) {
        assert!((pair[1] - pair[0] - 1.0 / 48.0).abs() < 1e-6, "{heroes:?}");
    }
    assert!((heroes[0] - 0.5 / 48.0).abs() < 1e-6);
    assert!(heroes.iter().all(|&h| (0.0..1.0 / 8.0).contains(&h)));
}

#[test]
fn reshade_is_deterministic_whatever_the_thread_count() {
    let rig = Rig::brilliant(glass());
    let build = scene_under(&rig, light_tent(1.0));
    let frame = cache_of(&build, &rig);
    let scene = scene_under(&rig, daylight(1.0));
    let first = reshade(&frame, &scene, 1, &never()).expect("a re-shade");
    let again = reshade(&frame, &scene, 1, &never()).expect("a second re-shade");
    let threaded = reshade(&frame, &scene, 3, &never()).expect("a threaded re-shade");
    assert_eq!(bits(&first), bits(&again), "two calls, the same bytes");
    assert_eq!(
        bits(&first),
        bits(&threaded),
        "the thread count is invisible"
    );
    assert_eq!(first.covered, threaded.covered);
    assert_eq!(
        first.mean_residual.to_bits(),
        threaded.mean_residual.to_bits()
    );
    assert_eq!(first.xyz.len(), (SIZE * SIZE) as usize);
}

#[test]
fn the_stone_is_covered_and_the_background_is_not() {
    let rig = Rig::brilliant(glass());
    let build = scene_under(&rig, light_tent(1.0));
    let frame = cache_of(&build, &rig);
    let image = reshade(&frame, &build, 2, &never()).expect("a re-shade");
    let at = |x: u32, y: u32| (y * SIZE + x) as usize;
    assert!(image.covered[at(8, 8)], "the middle is stone");
    assert!(image.xyz[at(8, 8)].y > 0.0, "and it has light");
    assert!(
        !image.covered[at(0, 0)],
        "the corner sees only the background"
    );
    assert_eq!(image.xyz[at(0, 0)], Vec3::ZERO);
    assert!(image.xyz.iter().all(|value| value.is_finite()));
    // What the build dropped is a fraction of the light, not most of it.
    assert!(image.mean_residual >= 0.0 && image.mean_residual < 0.25);
}

/// The re-shade of a pixel is the mean of phase B's samples of it (without the residual
/// completion, which the re-shade leaves out): every pixel within a z score of the sample mean.
#[test]
fn reshade_equals_the_mean_of_phase_b_under_the_same_lighting() {
    let rig = Rig::brilliant(glass());
    let scene = scene_under(&rig, smooth_environment());
    let frame = cache_of(&scene, &rig);
    let image = reshade(&frame, &scene, 2, &never()).expect("a re-shade");
    let samples = 3000_u32;
    let mut zs = Vec::new();
    for (x, y) in STONE_PIXELS {
        let index = (y * SIZE + x) as usize;
        if !image.covered[index] {
            continue;
        }
        let rot = pixel_rotations(y * SIZE + x);
        let mut sum = [0.0_f64; 3];
        let mut squares = [0.0_f64; 3];
        for sample in 0..samples {
            let kept = frame
                .shade_sample((x, y), sample, &rot)
                .expect("a covered pixel shades")
                .kept;
            for (i, v) in kept.to_array().into_iter().enumerate() {
                let v = f64::from(v);
                sum[i] += v;
                squares[i] = v.mul_add(v, squares[i]);
            }
        }
        let n = f64::from(samples);
        let reshaded = image.xyz[index].to_array();
        for ((total, square), value) in sum.iter().zip(&squares).zip(reshaded) {
            let mean = total / n;
            let mean_of_squares = square / n;
            let variance = mean.mul_add(-mean, mean_of_squares).max(0.0) * n / (n - 1.0);
            let error = (variance / n).sqrt();
            if error > 0.0 {
                zs.push((mean - f64::from(value)) / error);
            }
        }
    }
    assert!(zs.len() >= 9, "only {} values compared", zs.len());
    assert_z_stats(&zs, 4.0, "reshade against phase B");
}

#[test]
fn changing_the_lighting_keeps_the_cache_key_and_changes_the_image() {
    let rig = Rig::brilliant(glass());
    let tent = scene_under(&rig, light_tent(1.0));
    let day = scene_under(&rig, daylight(1.0));
    let bright = scene_under(&rig, light_tent(2.0));
    let settings = BuildSettings::default();
    let key = CacheKey::from_scene(&tent, Mat4::IDENTITY, &settings);
    assert_eq!(key, CacheKey::from_scene(&day, Mat4::IDENTITY, &settings));
    assert_eq!(
        key,
        CacheKey::from_scene(&bright, Mat4::IDENTITY, &settings)
    );

    let frame = cache_of(&tent, &rig);
    let under_tent = reshade(&frame, &tent, 2, &never()).expect("a re-shade");
    let built = (frame.built_tiles(), frame.built_bytes());
    assert!(built.0 > 0, "the first re-shade built the tiles");
    let under_day = reshade(&frame, &day, 2, &never()).expect("a re-shade");
    let under_bright = reshade(&frame, &bright, 2, &never()).expect("a re-shade");
    assert_eq!(
        (frame.built_tiles(), frame.built_bytes()),
        built,
        "new lighting builds nothing"
    );

    let changed = |a: &Reshaded, b: &Reshaded| {
        a.xyz
            .iter()
            .zip(&b.xyz)
            .filter(|(p, q)| (p.y - q.y).abs() > 1.0e-3 * p.y.max(1.0e-6))
            .count()
    };
    assert!(changed(&under_tent, &under_day) > 10, "another preset");
    assert!(changed(&under_tent, &under_bright) > 10, "another exposure");
    let centre = (8 * SIZE + 8) as usize;
    assert!(
        under_bright.xyz[centre].y > under_tent.xyz[centre].y,
        "twice the exposure is brighter"
    );
}

#[test]
fn a_raised_cancel_and_a_wrong_size_are_refused() {
    let rig = Rig::brilliant(glass());
    let scene = scene_under(&rig, light_tent(1.0));
    let frame = cache_of(&scene, &rig);
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        reshade(&frame, &scene, 2, &cancelled),
        Err(ReshadeError::Cancelled)
    );
    let other = FrameScene {
        width: SIZE + 1,
        ..scene_under(&rig, light_tent(1.0))
    };
    assert_eq!(
        reshade(&frame, &other, 2, &never()),
        Err(ReshadeError::SizeMismatch)
    );
}
