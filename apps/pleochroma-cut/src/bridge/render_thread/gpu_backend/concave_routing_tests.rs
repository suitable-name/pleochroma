//! The routing of a concave stone (tools within the GPU limit go to the GPU, beyond it stay
//! on the CPU) and the CPU fallback that carries them without an adapter.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset},
};

const W: u32 = 24;
const H: u32 = 24;

/// Accumulates two samples of the round brilliant, with or without a ball cut out of the
/// table, through `accumulate_frame_samples` and returns the radiance buffer.
fn accumulate(tools: &[ToolPrimitive], target: LocalComputeTarget) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(
        DEFAULT_POSE.yaw,
        DEFAULT_POSE.pitch,
        DEFAULT_POSE.distance,
        DEFAULT_FOV_DEG,
    );
    let frame = BackendFrame {
        width: W,
        height: H,
        yaw: DEFAULT_POSE.yaw,
        pitch: DEFAULT_POSE.pitch,
        distance: DEFAULT_POSE.distance,
        camera: &camera,
        planes: &planes,
        tools,
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        surface: SurfaceScene::IDEAL,
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        spp: 2,
        sample_offset: 0,
    };
    let pixels = (W * H) as usize;
    let mut accum = vec![Vec3::ZERO; pixels];
    let mut depth = vec![0.0; pixels];
    let mut normal = vec![Vec3::ZERO; pixels];
    let mut facet_id = vec![0; pixels];
    accumulate_frame_samples(
        &mut ViewportGpu::acquire(),
        &frame,
        &mut FrameOutputs {
            accum: &mut accum,
            depth: &mut depth,
            normal: &mut normal,
            facet_id: &mut facet_id,
        },
        &mut HybridPacing::new(),
        target,
    );
    accum
}

fn frame_with<'a>(
    camera: &'a Camera,
    planes: &'a [pleochroma::geometry::plane::GpuFacetPlane],
    material: &'a GemMaterial,
    tools: &'a [ToolPrimitive],
) -> BackendFrame<'a> {
    BackendFrame {
        width: 1,
        height: 1,
        yaw: 0.0,
        pitch: 0.0,
        distance: 5.0,
        camera,
        planes,
        tools,
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        surface: SurfaceScene::IDEAL,
        material,
        max_bounces: 1,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        spp: 1,
        sample_offset: 0,
    }
}

#[test]
fn a_cancelled_cpu_frame_does_not_add_samples_or_replace_guides() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let frame = frame_with(&camera, &planes, &material, &[]);
    let mut accum = [Vec3::ONE];
    let mut depth = [42.0];
    let mut normal = [Vec3::Y];
    let mut facet = [17];
    let completed = super::super::scanline::render_frame_scanlines_cancellable(
        &frame,
        1,
        1,
        &mut FrameOutputs {
            accum: &mut accum,
            depth: &mut depth,
            normal: &mut normal,
            facet_id: &mut facet,
        },
        &AtomicBool::new(true),
    );
    assert!(!completed);
    assert_eq!(accum, [Vec3::ONE]);
    assert_eq!(depth, [42.0]);
    assert_eq!(normal, [Vec3::Y]);
    assert_eq!(facet, [17]);
}

/// A concave frame routes to the GPU up to the tool limit. Only a stone beyond the limit
/// stays on the CPU.
#[test]
fn a_concave_frame_routes_to_the_gpu_up_to_the_tool_limit() {
    use pleochroma::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let ball = ToolPrimitive::ball(Vec3::new(0.0, 0.3, 0.0), 0.2);
    let within = vec![ball; GPU_MAX_TOOL_PRIMITIVES];
    let beyond = vec![ball; GPU_MAX_TOOL_PRIMITIVES + 1];

    assert!(frame_with(&camera, &planes, &material, &[]).routes_to_gpu());
    let concave = frame_with(&camera, &planes, &material, &within);
    assert!(!concave.stone().is_convex());
    assert!(concave.routes_to_gpu());
    assert!(!frame_with(&camera, &planes, &material, &beyond).routes_to_gpu());
}

/// A concave frame is traced with its tools on every compute target. The CPU tracer is the
/// reference, so without an adapter the GPU targets match it bit for bit. With an adapter
/// the GPU kernels are compared only by "the tools were applied" here. The crate's own
/// tests pin their equivalence.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn a_concave_frame_is_traced_with_its_tools_on_every_compute_target() {
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.35, 0.0), 0.3)];
    let flat = accumulate(&[], LocalComputeTarget::Cpu);
    let cut = accumulate(&tools, LocalComputeTarget::Cpu);
    assert_ne!(flat, cut, "the ball must change the picture");
    let has_adapter = ViewportGpu::acquire().gpu.adapter_label().is_some();
    for target in [LocalComputeTarget::CpuGpu, LocalComputeTarget::Gpu] {
        let traced = accumulate(&tools, target);
        assert_ne!(traced, flat, "{target:?} must apply the tools");
        if !has_adapter {
            assert_eq!(traced, cut, "{target:?}");
        }
    }
}

/// The GPU tool limit equals the CPU's (`GPU_MAX_TOOL_PRIMITIVES == MAX_TOOL_PRIMITIVES`),
/// so no valid stone exceeds it. Routing beyond it is pinned by
/// `a_concave_frame_routes_to_the_gpu_up_to_the_tool_limit`. A stone with the full list is
/// traced with its tools on every target, as in the test above.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn a_frame_at_the_tool_limit_is_traced_with_its_tools_on_every_target() {
    use pleochroma::geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, MAX_TOOL_PRIMITIVES};
    assert_eq!(GPU_MAX_TOOL_PRIMITIVES, MAX_TOOL_PRIMITIVES);
    let tools: Vec<ToolPrimitive> = (0..GPU_MAX_TOOL_PRIMITIVES)
        .map(|i| {
            let t = i as f32 / 100.0;
            ToolPrimitive::ball(Vec3::new(0.0, 0.002_f32.mul_add(t, 0.3), 0.0), 0.05)
        })
        .collect();
    let flat = accumulate(&[], LocalComputeTarget::Cpu);
    let cut = accumulate(&tools, LocalComputeTarget::Cpu);
    assert_ne!(flat, cut, "the tools must change the picture");
    let has_adapter = ViewportGpu::acquire().gpu.adapter_label().is_some();
    for target in [LocalComputeTarget::CpuGpu, LocalComputeTarget::Gpu] {
        let traced = accumulate(&tools, target);
        assert_ne!(traced, flat, "{target:?} must apply the tools");
        if !has_adapter {
            assert_eq!(traced, cut, "{target:?}");
        }
    }
}

/// A frame with a surface finish routes to the GPU like a polished one. The surface scene is
/// sent with the request (`GpuBackend::try_accumulate_geom_surface_cancellable`).
#[test]
fn a_frame_with_a_surface_finish_routes_to_the_gpu() {
    use pleochroma::optics::raytracer::surface::FacetSurface;
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let ground = [FacetSurface::ground(0.45, 1.0)];
    let mut frame = frame_with(&camera, &planes, &material, &[]);
    frame.surface = SurfaceScene {
        facet_surfaces: &ground,
        ..SurfaceScene::IDEAL
    };
    assert!(!frame.surface.is_ideal());
    assert!(frame.routes_to_gpu());
}
