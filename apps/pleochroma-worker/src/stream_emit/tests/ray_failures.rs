//! A declined deterministic range must fail instead of completing a partial picture.
use super::fixtures::tiny_scene;
use crate::{
    assets::TrayPin,
    cli::ComputeMode,
    stream_emit::{
        trace_range,
        tracer::{SharedState, TracerJob, run_tracer},
    },
};
use pleochroma::{
    optics::raytracer::environment::tray_shadow::TrayShadowOff,
    renderer::{engine::RenderEngine, gpu_backend::GpuBackend},
};
use pleochroma_net::messages::RequestIntent;
use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

#[test]
fn a_declined_ray_range_is_an_explicit_stream_and_own_lane_failure() {
    let mut scene = tiny_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    scene.engine.ray.aa_taps = 1;
    // Bypass ingress validation to exercise the renderer's defensive refusal.
    let job = TracerJob {
        scene: scene.clone(),
        first_sample: 1,
        samples: 1,
        threads: 1,
        compute_mode: ComputeMode::OnlyCpu,
        pin: Arc::new(TrayPin::new(None)),
    };
    let state = Arc::new(Mutex::new(SharedState::new(16)));
    let (tx, _rx) = mpsc::channel();
    let cancel = AtomicBool::new(false);
    let gpu = GpuBackend::disabled();
    run_tracer(&job, &gpu, &state, &cancel, &tx);
    let guard = state.lock().unwrap();
    assert!(guard.finished);
    assert!(!guard.panicked);
    assert_eq!(guard.samples_done, 0);
    assert!(guard.failed.is_some());
    drop(guard);
    let pin = Arc::new(TrayPin::new(None));
    let range = trace_range(&scene, (1, 1), 1, ComputeMode::OnlyCpu, &gpu, &cancel, &pin);
    assert_eq!(range.done, 0);
    assert!(range.failed.is_some());
    assert!(!range.panicked);
}

#[test]
fn cancelling_a_ray_range_does_not_report_a_renderer_failure() {
    let mut scene = tiny_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    let range = trace_range(
        &scene,
        (0, 1),
        1,
        ComputeMode::OnlyCpu,
        &GpuBackend::disabled(),
        &AtomicBool::new(true),
        &Arc::new(TrayPin::new(None)),
    );
    assert_eq!(range.done, 0);
    assert!(range.failed.is_none());
    assert!(!range.panicked);
}

/// A shadow whose map cannot be built (one plane bounds no stone) fails the range with the
/// message of `TrayShadowOff::DegenerateStone`: nothing is traced and nothing panics.
/// Cancellation during the same build is a plain cancel, not a failure.
#[test]
fn a_shadow_that_cannot_be_built_fails_the_range_with_its_message() {
    let mut scene = tiny_scene();
    scene.stone_shadow = true;
    scene.backdrop = 1.0;
    scene.planes.truncate(1);
    let gpu = GpuBackend::disabled();
    let pin = Arc::new(TrayPin::new(Some(RequestIntent::Interactive)));
    let range = trace_range(
        &scene,
        (0, 1),
        1,
        ComputeMode::OnlyCpu,
        &gpu,
        &AtomicBool::new(false),
        &pin,
    );
    assert_eq!(range.done, 0);
    assert!(!range.panicked);
    let failed = range.failed.expect("the request is refused");
    assert_eq!(
        failed.code,
        pleochroma_net::messages::error_codes::VALIDATION_FAILED
    );
    assert_eq!(failed.message, TrayShadowOff::DegenerateStone.message());

    let cancelled = trace_range(
        &scene,
        (0, 1),
        1,
        ComputeMode::OnlyCpu,
        &gpu,
        &AtomicBool::new(true),
        &pin,
    );
    assert_eq!(cancelled.done, 0);
    assert!(cancelled.failed.is_none());
}
