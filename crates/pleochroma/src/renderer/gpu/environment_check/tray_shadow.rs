//! Device-backed interpolation twin over deterministic random rays and wavelengths.

use super::UlpAccumulator;
use crate::{
    optics::raytracer::environment::tray_shadow::{TrayShadowMap, pack_gpu_words},
    renderer::gpu::{GpuContext, compute, frame::tray_shadow_source::TRAY_SHADOW_WGSL},
};
use glam::Vec3;

/// A ray origin, direction and wavelength for the lookup comparison.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TrayShadowCase {
    origin: [f32; 4],
    direction_lambda: [f32; 4],
}

/// Builds a small nonuniform spectral map for the lookup and frame comparisons.
///
/// Its plane sits just below the culet (the lowest vertex) of the standard round brilliant the
/// frame comparisons render, so the shadow is physical: the stone floats above the tray.
///
/// # Panics
/// Panics if the standard round brilliant has no valid hull.
#[cfg(any(test, feature = "stone-shadow"))]
#[must_use]
pub fn build_tray_shadow_check_map() -> TrayShadowMap {
    let culet_y = crate::geometry::brep::GemPolyhedron::from_planes(
        crate::geometry::cuts::StandardGemCuts::standard_round_brilliant(),
    )
    .expect("the standard round brilliant is a valid hull")
    .vertices
    .iter()
    .map(|vertex| vertex.y)
    .fold(f32::INFINITY, f32::min);
    let mut state = 0x7213_5917_u32;
    let factors = (0..8 * 8 * 20)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            3.0f32.mul_add((state >> 8) as f32 / 16_777_216.0, 0.05)
        })
        .collect();
    TrayShadowMap::from_parts(
        culet_y - 1e-4,
        [0.1, -0.2],
        2.5,
        8,
        factors,
        vec![[1.0; 3]; 64],
        crate::optics::raytracer::environment::TrayShadowKey(91),
    )
}

/// Absolute difference below which a lookup comparison is exempt from the ULP budget.
///
/// WGSL cannot forbid FMA contraction, so the GPU may fuse the plane hit's `origin + dir * t` and
/// the lerps where the CPU does not. One ULP of the hit (about 5e-7 at |x| = 4) becomes up to
/// 2e-7 in map coordinates, times 4 in texels (an 8-texel map), times the factor's slope across a
/// texel (up to 3 in the check map): about 2.4e-6. The floor leaves room for that and nothing
/// near the size of a wrong texel or band.
const TRAY_SHADOW_ABS_FLOOR: f32 = 1e-5;

/// Compares 4096 deterministic rays using a 64 ULP or [`TRAY_SHADOW_ABS_FLOOR`] budget; a
/// CPU/GPU disagreement about a plane hit counts as one over-budget comparison.
///
/// # Panics
/// Panics on GPU API misuse.
#[must_use]
pub fn run_tray_shadow_lookup(
    ctx: &GpuContext,
    map: &TrayShadowMap,
) -> super::UlpCheckResult<TrayShadowCase> {
    let mut state = 0x59af_1287_u32;
    let mut random = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / 16_777_216.0
    };
    let cases: Vec<TrayShadowCase> = (0..4096)
        .map(|_| TrayShadowCase {
            origin: [
                8.0f32.mul_add(random(), -4.0),
                4.0f32.mul_add(random(), -0.5),
                8.0f32.mul_add(random(), -4.0),
                0.0,
            ],
            direction_lambda: [
                random() - 0.5,
                random() - 0.8,
                random() - 0.5,
                440.0f32.mul_add(random(), 360.0),
            ],
        })
        .collect();
    // The lookup unit's environment wrapper requires the production parameters. Only the
    // independent lookup functions are reached by this test kernel, so omit that wrapper.
    let lookup = TRAY_SHADOW_WGSL
        .split("fn ts_environment(")
        .next()
        .expect("lookup prefix");
    let source = format!(
        "{lookup}\n{}",
        include_str!("../../shaders/transport_functions/tray_shadow_check.wgsl")
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "tray shadow lookup twin",
        &source,
        "ts_check_main",
    );
    let data = compute::upload(
        &ctx.device,
        "tray shadow twin map",
        &pack_gpu_words(map),
        wgpu::BufferUsages::STORAGE,
    );
    let input = compute::upload(
        &ctx.device,
        "tray shadow twin rays",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let output = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "tray shadow twin output",
        cases.len() * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let count = compute::upload(
        &ctx.device,
        "tray shadow twin count",
        &[4096_u32, 0, 0, 0],
        wgpu::BufferUsages::UNIFORM,
    );
    let bindings = compute::bind_buffers(
        &ctx.device,
        "tray shadow twin bindings",
        &pipeline,
        &[(190, &data), (191, &input), (192, &output), (193, &count)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bindings, (64, 1, 1));
    let results: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &output, cases.len() * 4);
    let mut comparison = UlpAccumulator::new("tray shadow lookup", 64, TRAY_SHADOW_ABS_FLOOR);
    for (case, result) in cases.iter().zip(results.as_chunks::<4>().0) {
        let origin = Vec3::new(case.origin[0], case.origin[1], case.origin[2]);
        let dir = Vec3::new(
            case.direction_lambda[0],
            case.direction_lambda[1],
            case.direction_lambda[2],
        );
        let hit = map.plane_uv(origin, dir);
        // A plane-hit disagreement is one failed comparison (1 against 0), not a panic.
        comparison.record(
            case,
            "hit",
            f32::from(u8::from(hit.is_some())),
            f32::from(u8::from(result[2] != 0.0)),
        );
        if let Some(uv) = hit {
            comparison.record(case, "u", uv[0], result[0]);
            comparison.record(case, "v", uv[1], result[1]);
        }
        let factor = hit.map_or(1.0, |uv| map.factor_at(uv, case.direction_lambda[3]));
        comparison.record(case, "factor", factor, result[3]);
    }
    comparison.finish()
}

#[test]
fn the_tray_shadow_lookup_matches_the_cpu() {
    let Some(ctx) = crate::renderer::gpu::merge_tests::context_or_skip(
        "the_tray_shadow_lookup_matches_the_cpu",
    ) else {
        return;
    };
    let result = run_tray_shadow_lookup(&ctx, &build_tray_shadow_check_map());
    assert!(result.passed(), "{result:?}");
}
