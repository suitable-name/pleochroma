//! Device-backed twin of `TrayMirror::hit` (`tm_floor`) over deterministic random rays.

use super::UlpAccumulator;
use crate::{
    optics::raytracer::environment::{TrayMirror, tray_shadow::pack_gpu_words_with_mirror},
    renderer::gpu::{GpuContext, compute, frame::tray_shadow_source::TRAY_SHADOW_WGSL},
};
use glam::Vec3;

/// A ray origin and unit direction for the floor comparison.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TrayMirrorCase {
    origin: [f32; 4],
    direction: [f32; 4],
}

/// The deterministic rays of [`run_tray_mirror_floor`]: origins spread above and below the
/// plane, directions over the whole sphere (about half point downward), unit length.
fn build_tray_mirror_cases(count: usize) -> Vec<TrayMirrorCase> {
    let mut state = 0x2b7e_1516_u32;
    let mut random = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / 16_777_216.0
    };
    (0..count)
        .map(|_| {
            let origin = [
                8.0f32.mul_add(random(), -4.0),
                5.0f32.mul_add(random(), -1.5),
                8.0f32.mul_add(random(), -4.0),
                0.0,
            ];
            let raw = Vec3::new(random() - 0.5, random() - 0.5, random() - 0.5);
            let dir = if raw.length_squared() > 1e-6 {
                raw.normalize()
            } else {
                Vec3::NEG_Y
            };
            TrayMirrorCase {
                origin,
                direction: [dir.x, dir.y, dir.z, 0.0],
            }
        })
        .collect()
}

/// Compares 4096 deterministic rays of `tm_floor` against [`TrayMirror::hit`].
///
/// The budget is 64 ULP or 2e-6 absolute; a CPU/GPU disagreement about a floor hit counts as
/// one over-budget comparison. The buffer is the production header-only upload of a mirror-only
/// scene ([`pack_gpu_words_with_mirror`]).
///
/// The absolute floor matters for the hit coordinates `x` and `z`: the origins reach 4 in each
/// axis, so where `origin.x + dir.x * t` cancels to |x| below about 0.25 a ULP budget alone is
/// too tight, and the division (2.5 ULP on the GPU) plus a contracted multiply-add leave an
/// absolute error of about 1.4e-6. 2e-6 bounds that; for values of about 0.5 and more the 64 ULP
/// budget (about 4e-6) is the wider one and stays the rule.
///
/// # Panics
/// Panics on GPU API misuse.
#[must_use]
pub fn run_tray_mirror_floor(
    ctx: &GpuContext,
    mirror: TrayMirror,
) -> super::UlpCheckResult<TrayMirrorCase> {
    let cases = build_tray_mirror_cases(4096);
    // The lookup prefix of the tray unit holds `tm_floor` and the binding; the environment
    // wrapper after it needs the production parameters, which this test kernel does not bind.
    let lookup = TRAY_SHADOW_WGSL
        .split("fn ts_environment(")
        .next()
        .expect("lookup prefix");
    let source = format!(
        "{lookup}\n{}",
        include_str!("../../shaders/transport_functions/tray_mirror_check.wgsl")
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "tray mirror floor twin",
        &source,
        "tm_check_main",
    );
    let data = compute::upload(
        &ctx.device,
        "tray mirror twin header",
        &pack_gpu_words_with_mirror(None, Some(mirror)),
        wgpu::BufferUsages::STORAGE,
    );
    let input = compute::upload(
        &ctx.device,
        "tray mirror twin rays",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let output = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "tray mirror twin output",
        cases.len() * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let count = compute::upload(
        &ctx.device,
        "tray mirror twin count",
        &[4096_u32, 0, 0, 0],
        wgpu::BufferUsages::UNIFORM,
    );
    let bindings = compute::bind_buffers(
        &ctx.device,
        "tray mirror twin bindings",
        &pipeline,
        &[(190, &data), (191, &input), (192, &output), (193, &count)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bindings, (64, 1, 1));
    let results: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &output, cases.len() * 4);
    let mut comparison = UlpAccumulator::new("tray mirror floor", 64, 2e-6);
    for (case, result) in cases.iter().zip(results.as_chunks::<4>().0) {
        let origin = Vec3::new(case.origin[0], case.origin[1], case.origin[2]);
        let dir = Vec3::new(case.direction[0], case.direction[1], case.direction[2]);
        let hit = mirror.hit(origin, dir);
        // A floor-hit disagreement is one failed comparison (1 against 0), not a panic.
        comparison.record(case, "hit", f32::from(u8::from(hit.is_some())), result[3]);
        if let Some(hit) = hit {
            comparison.record(case, "x", hit.point.x, result[0]);
            comparison.record(case, "z", hit.point.z, result[1]);
            comparison.record(case, "fresnel", hit.fresnel, result[2]);
        }
    }
    comparison.finish()
}

#[test]
fn the_tray_mirror_floor_matches_the_cpu() {
    let Some(ctx) =
        crate::renderer::gpu::merge_tests::context_or_skip("the_tray_mirror_floor_matches_the_cpu")
    else {
        return;
    };
    for (plane_y, r0) in [(0.3_f32, 0.04_f32), (-0.9, 1.0), (0.0, 0.5)] {
        let mirror = TrayMirror::new(plane_y, r0).expect("valid mirror");
        let result = run_tray_mirror_floor(&ctx, mirror);
        assert!(result.passed(), "plane {plane_y}, R0 {r0}: {result:?}");
    }
}

#[test]
fn the_tray_mirror_cases_cover_hits_and_misses() {
    let mirror = TrayMirror::new(0.3, 0.04).expect("valid mirror");
    let cases = build_tray_mirror_cases(4096);
    let hits = cases
        .iter()
        .filter(|case| {
            let origin = Vec3::new(case.origin[0], case.origin[1], case.origin[2]);
            let dir = Vec3::new(case.direction[0], case.direction[1], case.direction[2]);
            mirror.hit(origin, dir).is_some()
        })
        .count();
    assert!(hits > 800 && hits < 3300, "{hits} of 4096 rays hit");
    assert!(cases.iter().all(|c| {
        (Vec3::new(c.direction[0], c.direction[1], c.direction[2]).length() - 1.0).abs() < 1e-5
    }));
}
