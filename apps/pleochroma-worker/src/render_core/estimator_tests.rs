//! The worker traces with the estimator the scene carries (`scene.estimator`), never its process
//! default.
//!
//! A collect scene equals the CPU reference built with exit collection and differs from the
//! branching picture. An HDR scene asked to collect is the branching picture. Jobs that differ only
//! by estimator never share a calibration.

use super::{tests::tiny_scene, *};
use crate::{assets::TrayRef, cli::ComputeMode};
use pleochroma::optics::raytracer::Estimator;
use pleochroma_net::scene::{HdrEnvironment, SceneEnvironment};

/// A frame with enough stone pixels for the two estimators to differ.
fn frame(estimator: Estimator) -> SceneState {
    SceneState {
        width: 24,
        height: 24,
        estimator,
        ..tiny_scene()
    }
}

/// The CPU reference: the worker's per-pixel recipe, single-threaded, over a material prepared
/// for exit collection explicitly.
fn collect_reference(scene: &SceneState, first_sample: u32, samples: u32) -> Vec<Vec3> {
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll)
        .with_pan(scene.camera_pan);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), TrayRef::default());
    let geom = stone_geometry(scene);
    let surfaces = scene.effects.surfaces();
    let resolved = resolve_facet_finishes(scene);
    let finishes = merged_facet_finishes(&resolved, &surfaces);
    let material = scene.traced_material();
    let prepared = PreparedMaterial::new(&material).with_estimator(Estimator::ExitCollection);
    let plane_soa = build_plane_soa(geom.planes);
    let width = scene.width as usize;
    let mut out = vec![Vec3::ZERO; width * scene.height as usize];
    for (index, pixel) in out.iter_mut().enumerate() {
        let (x, y) = (index % width, index / width);
        let global = u32::try_from(index).expect("a pixel index fits u32");
        let rot = pixel_rotations(global);
        let mut sum = Vec3::ZERO;
        for offset in 0..samples {
            let draws = sample_draws(global, first_sample.wrapping_add(offset), &rot);
            let ray = camera.generate_ray(
                x as f32,
                y as f32,
                scene.width as f32,
                scene.height as f32,
                draws.jitter_x,
                draws.jitter_y,
            );
            let sample = trace_spectral_ray_prepared(
                ray,
                geom,
                &plane_soa,
                &finishes,
                &prepared,
                &scene.fluorescence,
                scene.max_bounces,
                environment,
                draws.seed,
                draws.hero_rand,
                None,
                scene.effects.surface_scene(&surfaces),
            );
            add_finite_sample(&mut sum, sample);
        }
        *pixel = sum;
    }
    out
}

/// A scene that asks for exit collection is traced with it, on the one-shot entry and on the
/// GPU entry's CPU fallback. The picture matches the reference bit for bit and not the
/// branching one.
#[test]
fn a_collect_scene_is_traced_with_exit_collection() {
    let collect = frame(Estimator::ExitCollection);
    let branching = frame(Estimator::Branching);
    let reference = collect_reference(&collect, 0, 6);
    let direct = trace_samples(&collect, 0, 6, 2);
    assert_eq!(direct, reference, "trace_samples follows scene.estimator");
    let via_gpu_entry = trace_samples_with_gpu(&GpuBackend::disabled(), &collect, 0, 6, 2);
    assert_eq!(via_gpu_entry, reference, "the GPU entry's CPU fallback too");
    assert_ne!(
        direct,
        trace_samples(&branching, 0, 6, 2),
        "exit collection is a different estimator from branching"
    );
}

/// An HDR scene asked to collect is traced as branching (the library falls back for a map).
#[test]
fn an_hdr_scene_asked_to_collect_equals_branching() {
    let pixels: Vec<image::Rgb<f32>> = (0..32 * 16)
        .map(|i| {
            let t = i as f32 / 512.0;
            image::Rgb([t.mul_add(5.0, 0.2), 0.5, (t * 13.0).cos().abs()])
        })
        .collect();
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes)
        .encode(&pixels, 32, 16)
        .unwrap();
    let key = || HdrEnvironment {
        content_hash: [0x7A; 32],
        width: 32,
        height: 16,
    };
    let _pin = crate::assets::decode_and_register(&key(), &bytes).unwrap();
    let hdr = |estimator| SceneState {
        environment: SceneEnvironment::Hdr(key()),
        ..frame(estimator)
    };
    let collect = trace_samples(&hdr(Estimator::ExitCollection), 0, 4, 2);
    let branching = trace_samples(&hdr(Estimator::Branching), 0, 4, 2);
    assert_eq!(collect, branching);
}

/// The calibration and sub-batch key carries the estimator.
#[test]
fn the_job_key_differs_by_estimator() {
    let gpu = GpuBackend::disabled();
    let key = |estimator| hybrid::job_key(&gpu, &frame(estimator), 2, ComputeMode::Hybrid);
    assert_ne!(key(Estimator::Branching), key(Estimator::ExitCollection));
    assert_eq!(key(Estimator::Branching), key(Estimator::Branching));
    assert_eq!(
        key(Estimator::ExitCollection),
        key(Estimator::ExitCollection)
    );
}
