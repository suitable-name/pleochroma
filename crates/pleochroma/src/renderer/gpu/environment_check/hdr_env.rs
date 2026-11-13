//! `hdr_env_radiance_at` ULP-budget self-test, dispatched by [`run_hdr_env_radiance`].
//!
//! Exercises `shaders/environment.wgsl`'s standalone `hdr_env_radiance_main` kernel --
//! its own duplicate of `spectral_transport.wgsl`'s `hdr_env_radiance_at` (see that
//! kernel's own header comment for why this file keeps an independent copy rather than
//! sharing code across WGSL modules) -- against
//! `renderer::env_map::EnvironmentMap::radiance_at` directly, independent of the
//! megakernel's own Tier 3 image comparisons for a loaded HDR map. A second pass runs the same
//! cases with a map exposure and a map yaw against `radiance_rgb_rotated`.

use glam::Vec3;

use crate::{
    optics::raytracer::hdr_rgb_at_exposure,
    renderer::{
        env_map::{EnvRotation, EnvironmentMap, rgb_to_spectral_radiance},
        gpu::compute,
    },
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult, fibonacci_sphere};

/// One input case for the hdr env check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HdrEnvCase {
    dir: [f32; 3],
    lambda_nm: f32,
}

/// ULP budget for `hdr_env_radiance_at`.
///
/// Chains `acos`/`atan2`/`normalize` (direction to uv), a bilinear interpolation, and
/// `rgb_to_spectral_radiance`'s six asymmetric Gaussians (one `exp()` each, clamped at zero).
/// It has fewer transcendental calls than [`super::STUDIO_ENV_ULP_BUDGET`]'s ring-light loop,
/// so the budget sits below that one, and it is a conservative ceiling.
pub const HDR_ENV_ULP_BUDGET: u32 = 4096;

/// Absolute-difference floor for `hdr_env_radiance_at` comparisons.
///
/// Covers the equirectangular seam (`u` wrapping through `0.0`/`1.0`) and the poles (`v`
/// clamped to `0.0`/`1.0`), where a one-texel rounding difference between CPU and GPU changes
/// which neighbour `sample_bilinear` blends toward.
pub const HDR_ENV_ABS_FLOOR: f32 = 1e-4;

/// Map exposure of the second (rotated) pass of [`run_hdr_env_radiance`].
const ROTATED_PASS_EXPOSURE: f32 = 1.75;

/// Map yaw (radians) of the second (rotated) pass of [`run_hdr_env_radiance`].
const ROTATED_PASS_YAW: f32 = 0.7;

/// Mirror of `environment.wgsl`'s `HdrEnvCheckParams` uniform (binding 14).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct HdrEnvCheckParams {
    hdr_exposure: f32,
    hdr_rot_cos: f32,
    hdr_rot_sin: f32,
    _pad: f32,
}

impl HdrEnvCheckParams {
    const fn new(exposure: f32, rotation: EnvRotation) -> Self {
        let [hdr_rot_cos, hdr_rot_sin] = rotation.cos_sin();
        Self {
            hdr_exposure: exposure,
            hdr_rot_cos,
            hdr_rot_sin,
            _pad: 0.0,
        }
    }
}

/// A small, deliberately non-uniform synthetic HDR map: a smooth RGB gradient across
/// `(u, v)`, so `sample_bilinear`'s interpolation is genuinely exercised (a uniform map,
/// like [`crate::renderer::env_map::EnvironmentMap::uniform`], would return the same
/// value regardless of a wrong texel index or a wrong interpolation weight).
///
/// `pub(crate)`: `estimator_check::run_spectral_debug` reuses this map for its
/// `EnvironmentSource::HdrMap` case.
pub fn build_hdr_test_map() -> crate::renderer::env_map::EnvironmentMap {
    let width = 16usize;
    let height = 8usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let u = x as f32 / width as f32;
            let v = y as f32 / height as f32;
            pixels.push([
                0.8f32.mul_add(u, 0.2),
                0.6f32.mul_add(v, 0.1),
                0.9f32.mul_add(f32::midpoint(u, v), 0.05),
            ]);
        }
    }
    crate::renderer::env_map::EnvironmentMap::from_rgb(width, height, pixels)
        .expect("width/height/pixels are self-consistent by construction")
}

/// Dense direction/wavelength sweep plus adversarial points: both poles (`v` clamped),
/// and directions straddling the `u == 0.0`/`1.0` equirectangular wrap seam.
#[must_use]
pub fn build_hdr_env_cases() -> Vec<HdrEnvCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(256);
    let lambdas = [400.0f32, 500.0, 560.0, 650.0, 700.0];
    for &dir in &directions {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    let adversarial_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1e-4, 0.0, -1.0).normalize(),
        Vec3::new(-1e-4, 0.0, -1.0).normalize(),
    ];
    for dir in adversarial_dirs {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    cases
}

/// Dispatches `hdr_env_radiance_main` once over `cases` with `params` and reads the results back.
fn dispatch_hdr_env(
    ctx: &crate::renderer::gpu::GpuContext,
    hdr_env: &crate::renderer::env_map_gpu::HdrEnvGpuData,
    cases: &[HdrEnvCase],
    params: HdrEnvCheckParams,
) -> Vec<f32> {
    let total = cases.len();
    let params_buf = compute::upload(
        &ctx.device,
        "hdr_env params",
        &[params],
        wgpu::BufferUsages::UNIFORM,
    );
    let in_buf = compute::upload(
        &ctx.device,
        "hdr_env cases",
        cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hdr_env out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "hdr_env_radiance_main",
        SHADER_SRC,
        "hdr_env_radiance_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hdr_env bind group",
        &pipeline,
        &[
            (8, &in_buf),
            (9, &out_buf),
            (10, &hdr_env.texels),
            (11, &hdr_env.dims),
            (14, &params_buf),
        ],
    );
    let workgroups = u32::try_from(total)
        .expect("a check case count fits u32")
        .div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    compute::readback(&ctx.device, &ctx.queue, &out_buf, total)
}

/// Runs the `hdr_env_radiance_at` ULP-budget self-test against a live GPU.
///
/// The unrotated, unexposed lookup against `EnvironmentMap::radiance_at`, then the same cases
/// with [`ROTATED_PASS_EXPOSURE`] and a [`ROTATED_PASS_YAW`] map yaw against
/// `radiance_rgb_rotated` + `hdr_rgb_at_exposure` + `rgb_to_spectral_radiance`.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_hdr_env_radiance(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HdrEnvCase> {
    let map: EnvironmentMap = build_hdr_test_map();
    let cases = build_hdr_env_cases();
    let hdr_env = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &map);

    let plain = dispatch_hdr_env(
        ctx,
        &hdr_env,
        &cases,
        HdrEnvCheckParams::new(1.0, EnvRotation::IDENTITY),
    );
    let rotation = EnvRotation::from_yaw(ROTATED_PASS_YAW);
    let turned = dispatch_hdr_env(
        ctx,
        &hdr_env,
        &cases,
        HdrEnvCheckParams::new(ROTATED_PASS_EXPOSURE, rotation),
    );

    let mut acc = UlpAccumulator::new("hdr_env_radiance_at", HDR_ENV_ULP_BUDGET, HDR_ENV_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        let dir = Vec3::from_array(case.dir);
        let cpu = map.radiance_at(dir, case.lambda_nm);
        acc.record(case, "radiance", cpu, plain[idx]);
        let lit = map.radiance_rgb_rotated(dir, rotation);
        let rgb = hdr_rgb_at_exposure(lit, ROTATED_PASS_EXPOSURE);
        let cpu_turned = rgb_to_spectral_radiance(rgb, case.lambda_nm);
        acc.record(case, "radiance rotated", cpu_turned, turned[idx]);
    }
    acc.finish()
}
