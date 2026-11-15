//! Smoke and equivalence tests of the hybrid CPU+GPU export path.

use super::{
    worker::{Accumulation, AccumulationOutcome, render_accumulation, render_local_share},
    *,
};
use crate::bridge::render_thread::RenderContext;
use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, LightingPreset},
    },
    renderer::gpu_backend::GpuBackend,
};
use std::sync::{Mutex, atomic::AtomicU32};

/// Renders a small frame through `run_export` (calibration, batching, merge, PNG write) and
/// checks that it completes with a non-empty file. Without a GPU adapter this runs CPU only.
/// Pixel values are not checked, because the sample split depends on calibration.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn export_completes_via_hybrid_or_cpu_path() {
    let scene = SceneSnapshot {
        camera_roll: 0.0,
        camera_pan: [0.0, 0.0],
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: 0.6,
        pitch: -0.4,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::by_name("Zircon").expect("built-in material"),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 12,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: std::sync::Arc::default(),
        facet_finishes: Vec::new(),
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        stone_pose: pleochroma::render_setup::StonePose::IDENTITY,
        surface_glare: 1.0,
        head_shadow_deg: 16.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
    };
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 48,
        height: 48,
        samples_per_pixel: 12,
        max_bounces: 12,
    };
    let out = std::env::temp_dir().join("pleochroma_hybrid_export_smoke.png");
    let cancel = AtomicBool::new(false);
    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &out,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );
    match outcome {
        ExportOutcome::Completed(path) => {
            let len = std::fs::metadata(&path).map_or(0, |m| m.len());
            assert!(len > 0, "export wrote an empty file");
            let _ = std::fs::remove_file(path);
        }
        ExportOutcome::Cancelled => panic!("export reported cancelled without a cancel"),
        ExportOutcome::Failed(e) => panic!("export failed: {e}"),
    }
}

// ---- `worker::render_local_share` ------------------------------------------------

/// The tail sum of `render_local_share`, added to a plain `render_batch` trace of the head
/// `[0, first)`, must match one `render_accumulation` over the whole range within a relative
/// tolerance. The two sides sum the same samples in a different order, and float addition is
/// not associative. CPU only (`GpuBackend::disabled()`).
#[test]
fn a_local_share_plus_its_complement_matches_render_accumulation() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let (width, height, spp) = (6u32, 6u32, 12u32);
    let (cam_yaw, cam_pitch) = (0.3f32, 0.2f32);
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width,
        height,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();

    // The reference: the whole range traced by `render_accumulation` (local CPU only).
    let mut carry = AccumulationCarry::default();
    let cancel = AtomicBool::new(false);
    let whole = match render_accumulation(
        &scene,
        cam_yaw,
        cam_pitch,
        params,
        ComputeTarget::LocalOnly,
        None,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut carry,
        &cancel,
        |_| {},
    ) {
        AccumulationOutcome::Completed(Accumulation { accum, .. }) => accum,
        AccumulationOutcome::Cancelled | AccumulationOutcome::Failed(_) => {
            panic!("expected a completed accumulation")
        }
    };

    // The same samples, split into a head `[0, first)` traced with `render_batch` and a tail
    // `[first, spp)` traced with `render_local_share`, as `final_picture` splits a contribution.
    let viewer = 5u32;
    let first = spp - viewer;
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll)
        .with_pan(scene.camera_pan);
    let mut combined = vec![Vec3::ZERO; (width * height) as usize];
    batch::render_batch(width, height, first, 0, &camera, &scene, &mut combined);

    let mut hybrid_frac: Option<f64> = None;
    let stop = AtomicBool::new(false);
    let done = AtomicU32::new(0);
    let tail = render_local_share(
        &scene,
        (cam_yaw, cam_pitch),
        params,
        first,
        viewer,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut hybrid_frac,
        &stop,
        &done,
    )
    .expect("not stopped");
    assert_eq!(
        done.load(Ordering::Relaxed),
        viewer,
        "must finish the whole tail"
    );

    for (dst, src) in combined.iter_mut().zip(&tail) {
        *dst += *src;
    }

    for (whole_px, combined_px) in whole.iter().zip(&combined) {
        let scale = whole_px.length().max(combined_px.length()).max(1e-6);
        let relative_error = (*whole_px - *combined_px).length() / scale;
        assert!(
            relative_error < 1e-3,
            "{whole_px:?} vs {combined_px:?} (relative error {relative_error})"
        );
    }
}

/// A `stop` raised before the first batch must end the render with `None`, never a partial
/// `Some`. `run_final_image_request` raises `stop` when the remote finishes first, so a cut-short
/// local render must never count as a finished contribution.
#[test]
fn render_local_share_returns_none_when_stopped() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 4,
        height: 4,
        samples_per_pixel: 8,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();
    let mut hybrid_frac: Option<f64> = None;
    let stop = AtomicBool::new(true);
    let done = AtomicU32::new(0);

    let result = render_local_share(
        &scene,
        (0.3, 0.2),
        params,
        0,
        8,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut hybrid_frac,
        &stop,
        &done,
    );
    assert!(result.is_none());
}

/// A carried split under the limit runs concurrent batches: the GPU thread beside the CPU tracer.
/// Calibration on a fast GPU settles on the GPU alone, so this test is the only one that reaches
/// that branch on such a machine.
///
/// The render must count every sample and produce a finite, lit image. The returned share must be
/// `None` (the GPU declined), below 0.7, or exactly `1.0`. `PLEOCHROMA_REQUIRE_GPU=1` makes a GPU
/// decline a failure.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-10); run on request: scripts/run_tests_on_request.sh"]
fn a_carried_split_runs_concurrent_batches_and_settles_on_a_valid_share() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    // 160 samples: batches of 4, so a share of 0.5 gives each engine 2 samples per batch.
    let (width, height, spp) = (48u32, 48u32, 160u32);
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width,
        height,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::acquire();
    let mut hybrid_frac = Some(0.5);
    let stop = AtomicBool::new(false);
    let done = AtomicU32::new(0);
    let sum = render_local_share(
        &scene,
        (0.3, 0.2),
        params,
        0,
        spp,
        &gpu,
        LocalComputeTarget::CpuGpu,
        &mut hybrid_frac,
        &stop,
        &done,
    )
    .expect("not stopped");

    assert_eq!(done.load(Ordering::Relaxed), spp, "every sample is counted");
    assert_eq!(sum.len(), (width * height) as usize);
    assert!(sum.iter().all(|px| px.is_finite()), "a non-finite pixel");
    assert!(sum.iter().any(|px| px.length() > 0.0), "a black image");
    assert!(
        hybrid_frac.is_none_or(|frac| frac < 0.7 || frac == 1.0),
        "the share must be under the limit or exactly 1.0, got {hybrid_frac:?}"
    );
    if std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|value| value == "1") {
        assert!(
            hybrid_frac.is_some(),
            "PLEOCHROMA_REQUIRE_GPU=1 but the GPU declined the render (no adapter, or the app was \
             built without the gpu feature)"
        );
    }
}
