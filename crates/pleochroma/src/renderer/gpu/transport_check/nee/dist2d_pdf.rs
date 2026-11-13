//! Tier 2 check for `EnvironmentMap::pdf`'s piecewise-constant lookup (`dist2d_pdf`).

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

/// One input case for the dist2d pdf check: a world direction and the HDR map's yaw as
/// `(cos, sin, 0, 0)` (`(1, 0)` is the identity).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist2dPdfCase {
    dir: [f32; 3],
    _pad0: f32,
    rot: [f32; 4],
}

const _: () = assert!(size_of::<Dist2dPdfCase>() == 32);

/// Map yaw (radians) of the rotated half of the case table.
const ROTATED_HALF_YAW: f32 = 0.7;

const DIST2D_PDF_ULP_BUDGET: u32 = 32;
const DIST2D_PDF_ABS_FLOOR: f32 = 1e-5;

/// Every direction here must map to a `(u, v)` strictly inside a texel of the 8x4
/// [`super::synthetic_test_map`], never onto a bucket edge.
///
/// `EnvironmentMap::pdf` is piecewise constant and jumps at every texel edge, and neighbouring
/// texels differ by up to 10x in luminance. A direction on an edge can land in either texel,
/// depending on the last ULP of `atan2`/`acos`. The grid is therefore offset off the edges,
/// and the hand-picked directions avoid `phi` multiples of `PI/8` and `theta` multiples of
/// `PI/4`. The near-pole picks carry a small `x` so `phi` is neither `0` nor `PI`. The two
/// poles stay, since both sides return exactly `0.0` there.
fn build_dist2d_pdf_directions() -> Vec<Vec3> {
    let mut dirs = Vec::new();
    let hand_picked = [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::new(0.007, 0.9999, 0.01).normalize(),
        Vec3::new(0.007, -0.9999, 0.01).normalize(),
        Vec3::new(0.3, 0.4, 0.5).normalize(),
        Vec3::new(-1.0, 2.0, -0.5).normalize(),
        Vec3::new(0.9, -0.1, -0.2).normalize(),
        Vec3::new(-0.2, -0.7, 0.6).normalize(),
    ];
    dirs.extend_from_slice(&hand_picked);
    for theta_i in 0..10 {
        let theta = (theta_i as f32 + 0.37) * std::f32::consts::PI / 10.0;
        let (sin_t, cos_t) = theta.sin_cos();
        for phi_i in 0..16 {
            let phi = (phi_i as f32 + 0.29) * 2.0 * std::f32::consts::PI / 16.0;
            let (sin_p, cos_p) = phi.sin_cos();
            dirs.push(Vec3::new(sin_t * sin_p, cos_t, sin_t * cos_p));
        }
    }
    dirs
}

/// The case table: every direction of [`build_dist2d_pdf_directions`] unrotated, then every
/// direction again with a [`ROTATED_HALF_YAW`] map yaw. The pdf is looked up at `to_map(dir)`,
/// so the rotated half passes `to_world(d)` of each edge-avoiding map-frame direction `d`: its
/// `to_map` lands back on `d` (to rounding), never on a bucket edge.
fn build_dist2d_pdf_cases() -> Vec<Dist2dPdfCase> {
    let dirs = build_dist2d_pdf_directions();
    let mut cases = Vec::with_capacity(dirs.len() * 2);
    for rotation in [
        EnvRotation::IDENTITY,
        EnvRotation::from_yaw(ROTATED_HALF_YAW),
    ] {
        let [rot_cos, rot_sin] = rotation.cos_sin();
        for &d in &dirs {
            cases.push(Dist2dPdfCase {
                dir: rotation.to_world(d).to_array(),
                _pad0: 0.0,
                rot: [rot_cos, rot_sin, 0.0, 0.0],
            });
        }
    }
    cases
}

/// Runs the Tier 2 check for
/// [`EnvironmentMap::pdf`](crate::renderer::env_map::EnvironmentMap::pdf).
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_dist2d_pdf(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<Dist2dPdfCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_dist2d_pdf_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "dist2d pdf in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "dist2d pdf out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dist2d_pdf_main",
        SHADER_SRC,
        "dist2d_pdf_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dist2d pdf bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (64, &gpu_data.dist_dims),
            (71, &in_buf),
            (72, &out_buf),
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new("dist2d_pdf", DIST2D_PDF_ULP_BUDGET, DIST2D_PDF_ABS_FLOOR);
    let rotated_from = cases.len() / 2;
    let rotated = EnvRotation::from_yaw(ROTATED_HALF_YAW);
    for (idx, case) in cases.iter().enumerate() {
        let rotation = if idx < rotated_from {
            EnvRotation::IDENTITY
        } else {
            rotated
        };
        let cpu = map.pdf_rotated(Vec3::from_array(case.dir), rotation);
        acc.record(case, "pdf", cpu, gpu_out[idx]);
    }
    acc.finish()
}
