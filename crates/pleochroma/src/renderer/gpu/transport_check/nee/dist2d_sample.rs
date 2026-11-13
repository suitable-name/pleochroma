//! Tier 2 check for `EnvironmentMap::sample`'s continuous 2D importance sampling
//! (`dist2d_sample`).

use glam::Vec3;

use crate::renderer::{
    env_map::EnvRotation,
    env_map_gpu::HdrEnvGpuData,
    gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::synthetic_test_map;

/// One input case for the dist2d sample check: the two uniform randoms and the HDR map's yaw as
/// `(cos, sin)` (`(1, 0)` is the identity).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist2dSampleCase {
    u0: f32,
    u1: f32,
    rot_cos: f32,
    rot_sin: f32,
}

const _: () = assert!(size_of::<Dist2dSampleCase>() == 16);

const DIST2D_SAMPLE_ULP_BUDGET: u32 = 48;
const DIST2D_SAMPLE_ABS_FLOOR: f32 = 1e-5;

/// Map yaw (radians) of the rotated half of the case table.
const ROTATED_HALF_YAW: f32 = 0.7;

/// The `(u0, u1)` pairs of the check: a dense grid plus adversarial points. The case table runs
/// them twice, unrotated and then with a [`ROTATED_HALF_YAW`] map yaw.
fn build_dist2d_sample_pairs() -> Vec<(f32, f32)> {
    let mut cases = Vec::new();
    let steps = 14;
    for i in 0..=steps {
        let u0 = (i as f32 / steps as f32).mul_add(0.998, 0.001);
        for j in 0..=steps {
            let u1 = (j as f32 / steps as f32).mul_add(0.998, 0.001);
            cases.push((u0, u1));
        }
    }
    let adversarial = [
        (0.0f32, 0.0f32),
        (0.0, 0.5),
        (0.5, 0.0),
        (0.999_999, 0.999_999),
        (1e-6, 1e-6),
        (0.5, 0.999_999),
        (0.999_999, 0.5),
        (0.25, 0.75),
        (0.75, 0.25),
    ];
    cases.extend_from_slice(&adversarial);
    cases
}

/// The case table: every pair of [`build_dist2d_sample_pairs`] unrotated, then every pair again
/// with a [`ROTATED_HALF_YAW`] map yaw.
fn build_dist2d_sample_cases() -> Vec<Dist2dSampleCase> {
    let pairs = build_dist2d_sample_pairs();
    let mut cases = Vec::with_capacity(pairs.len() * 2);
    for rotation in [
        EnvRotation::IDENTITY,
        EnvRotation::from_yaw(ROTATED_HALF_YAW),
    ] {
        let [rot_cos, rot_sin] = rotation.cos_sin();
        for &(u0, u1) in &pairs {
            cases.push(Dist2dSampleCase {
                u0,
                u1,
                rot_cos,
                rot_sin,
            });
        }
    }
    cases
}

/// `|dir.y|` above which [`run_dist2d_sample`] compares `pdf * sin(theta)` (each side's own
/// `sin(theta)`, read off its own returned direction) instead of the raw pdf.
///
/// The solid-angle pdf is `pdf_uv / (2 PI^2 sin(theta))`, so near either pole a small error in
/// `sin(theta)` is a large relative pdf error. Near the south pole the spacing of `v` (`6e-8`)
/// sets the error, and near the north pole WGSL's `sin` is bounded only absolutely. Both sides
/// are self-consistent, so the ULP comparison is meaningless there. `pdf * sin(theta)` cancels
/// the `1/sin(theta)` amplification and compares the well-conditioned `pdf_uv / (2 PI^2)`. The
/// band `|y| < 0.998` (`1 - v > 0.02`) keeps the south-pole error under the budget. The north
/// pole uses the same band.
const POLE_CONDITIONING_Y: f32 = 0.998;

/// `sin(theta)` of a unit direction, `|(x, z)|` -- the same expression
/// `EnvironmentMap::pdf` uses.
fn sin_theta_of(dir: Vec3) -> f32 {
    dir.x.hypot(dir.z)
}

/// Runs the Tier 2 check for
/// [`EnvironmentMap::sample`](crate::renderer::env_map::EnvironmentMap::sample).
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_dist2d_sample(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<Dist2dSampleCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_dist2d_sample_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "dist2d sample in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "dist2d sample out",
        total * 7,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dist2d_sample_main",
        SHADER_SRC,
        "dist2d_sample_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dist2d sample bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (63, &gpu_data.dist_cdf),
            (64, &gpu_data.dist_dims),
            (65, &gpu_data.texels),
            (66, &gpu_data.dims),
            (69, &in_buf),
            (70, &out_buf),
        ],
    );
    let workgroups = u32::try_from(total)
        .expect("the case count fits u32")
        .div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 7);

    let mut acc = UlpAccumulator::new(
        "dist2d_sample",
        DIST2D_SAMPLE_ULP_BUDGET,
        DIST2D_SAMPLE_ABS_FLOOR,
    );
    let rotated_from = cases.len() / 2;
    let rotated = EnvRotation::from_yaw(ROTATED_HALF_YAW);
    for (idx, case) in cases.iter().enumerate() {
        let rotation = if idx < rotated_from {
            EnvRotation::IDENTITY
        } else {
            rotated
        };
        let (cpu_dir, cpu_rgb, cpu_pdf) = map.sample_rotated(case.u0, case.u1, rotation);
        let base = idx * 7;
        for (c_idx, comp) in ["dir_x", "dir_y", "dir_z"].iter().enumerate() {
            acc.record(case, comp, cpu_dir[c_idx], gpu_out[base + c_idx]);
        }
        for (c_idx, comp) in ["rgb_r", "rgb_g", "rgb_b"].iter().enumerate() {
            acc.record(case, comp, cpu_rgb[c_idx], gpu_out[base + 3 + c_idx]);
        }
        let gpu_pdf = gpu_out[base + 6];
        if cpu_dir.y.abs() > POLE_CONDITIONING_Y {
            let gpu_dir = Vec3::new(gpu_out[base], gpu_out[base + 1], gpu_out[base + 2]);
            acc.record(
                case,
                "pdf*sin_theta (pole, see POLE_CONDITIONING_Y)",
                cpu_pdf * sin_theta_of(cpu_dir),
                gpu_pdf * sin_theta_of(gpu_dir),
            );
        } else {
            acc.record(case, "pdf", cpu_pdf, gpu_pdf);
        }
    }
    acc.finish()
}
