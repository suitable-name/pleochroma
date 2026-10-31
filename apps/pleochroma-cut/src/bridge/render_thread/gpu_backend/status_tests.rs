//! The viewport's GPU status pill and the workload identity the pacing state keys on.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{DEFAULT_FOV_DEG, LightingPreset},
};

const LOSS: &str = "CPU fallback (GPU lost: test)";

fn frame<'a>(
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
    size: u32,
    spp: u32,
) -> BackendFrame<'a> {
    BackendFrame {
        width: size,
        height: size,
        yaw: 0.0,
        pitch: 0.0,
        distance: 5.0,
        camera,
        planes,
        tools: &[],
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        surface: SurfaceScene::IDEAL,
        material,
        max_bounces: 1,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        spp,
        sample_offset: 0,
    }
}

/// Every `PassBudget` ramp step (1, 2, 4, 8, ...) is another `spp`; none of them is
/// another workload, or the measured CPU/GPU rates and the calibration would be dropped
/// on each doubling.
#[test]
fn the_workload_key_ignores_the_pass_size_but_not_the_resolution() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let key = |size, spp| dispatch::workload_key(&frame(&camera, &planes, &material, size, spp));
    assert_eq!(key(8, 1), key(8, 2));
    assert_eq!(key(8, 1), key(8, 64));
    assert_ne!(key(8, 1), key(16, 1));
}

#[test]
fn the_ramp_keeps_the_measured_rates() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let mut pacing = HybridPacing::new();
    pacing.prepare_frame(&frame(&camera, &planes, &material, 8, 1));
    pacing.observe_cpu_only(1, std::time::Duration::from_millis(10));
    assert!(!pacing.needs_cpu_timing());
    for spp in [2, 4, 8] {
        pacing.prepare_frame(&frame(&camera, &planes, &material, 8, spp));
        assert!(!pacing.needs_cpu_timing(), "spp={spp}");
    }
}

/// A CPU frame never polls the preparation worker, so a "Preparing GPU renderer" pill
/// left by an earlier GPU frame (or by Ray-Traced) would stay on screen for good. A
/// device-loss message still describes the GPU and stays.
#[test]
fn a_cpu_frame_drops_a_stale_preparing_pill_but_keeps_a_loss_message() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let frame = frame(&camera, &planes, &material, 2, 1);
    let mut backend = ViewportGpu::from_backend(GpuBackend::disabled(), true);
    let mut hybrid = HybridPacing::new();
    let mut run = |backend: &mut ViewportGpu| {
        let (mut accum, mut depth) = (vec![Vec3::ZERO; 4], vec![0.0; 4]);
        let (mut normal, mut facet_id) = (vec![Vec3::ZERO; 4], vec![0; 4]);
        accumulate_frame_samples(
            backend,
            &frame,
            &mut FrameOutputs {
                accum: &mut accum,
                depth: &mut depth,
                normal: &mut normal,
                facet_id: &mut facet_id,
            },
            &mut hybrid,
            LocalComputeTarget::Cpu,
        );
    };
    backend.status_message = Some(PREPARING_STATUS.to_owned());
    run(&mut backend);
    assert_eq!(backend.status_message(), None);
    backend.status_message = Some(LOSS.to_owned());
    run(&mut backend);
    assert_eq!(backend.status_message(), Some(LOSS));
}

/// Ray-Traced writes its own pill text; Path-Traced must not inherit it, nor lose the
/// text it wrote itself.
#[test]
fn a_ray_status_is_dropped_on_return_to_path_but_a_path_status_is_not() {
    let mut backend = ViewportGpu::from_backend(GpuBackend::disabled(), true);
    backend.set_ray_status(Some(PREPARING_STATUS));
    backend.clear_ray_status();
    assert_eq!(backend.status_message(), None);

    backend.status_message = Some(LOSS.to_owned());
    backend.clear_ray_status();
    assert_eq!(backend.status_message(), Some(LOSS));

    // A healthy Ray frame clears its own text, so nothing is left for the switch to drop.
    backend.set_ray_status(None);
    backend.clear_ray_status();
    assert_eq!(backend.status_message(), None);
}
