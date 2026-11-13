//! `blackbody_spectrum` ULP-budget self-test: [`build_blackbody_cases`]'s
//! temperature/wavelength sweep plus adversarial points, dispatched by [`run_blackbody`].

use crate::{optics::raytracer::blackbody_spectrum, renderer::gpu::compute};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// One input case for the blackbody check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlackbodyCase {
    lambda_nm: f32,
    temp_k: f32,
}

/// ULP budget for `blackbody_spectrum`.
///
/// The function chains two `exp()`s and a `powi_u(_, 5)` (exponentiation by squaring, not WGSL's
/// `pow()`; see `environment.wgsl`), so it needs a larger budget than the polynomial
/// `cie_1931_cmf`.
pub const BLACKBODY_ULP_BUDGET: u32 = 48;

/// Absolute-difference floor for `blackbody_spectrum` comparisons.
///
/// The function is clamped to `[0.01, 20.0]`, so `0.01` is its smallest value. The floor sits
/// far below that, so it absorbs rounding at the clamp boundary without masking a real
/// divergence between in-range values.
pub const BLACKBODY_ABS_FLOOR: f32 = 1e-5;

/// Builds the blackbody cases for the check.
#[must_use]
pub fn build_blackbody_cases() -> Vec<BlackbodyCase> {
    let mut cases = Vec::new();
    let temps = [
        1000.0f32, 1500.0, 2000.0, 3200.0, 5000.0, 6500.0, 6600.0, 8000.0, 10_000.0,
    ];
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the constant (780 - 380) / 15 = 26.67 is positive; truncating to 26 steps is the intent"
    )]
    let lambda_steps = ((780.0 - 380.0) / 15.0) as u32;
    for step in 0..=lambda_steps {
        let lambda = (step as f32).mul_add(15.0, 380.0);
        for &temp_k in &temps {
            cases.push(BlackbodyCase {
                lambda_nm: lambda,
                temp_k,
            });
        }
    }
    // Adversarial: below the 1000K clamp, exactly at 560nm (the normalization
    // wavelength), and low enough temperatures to approach the `min(80.0)` exponent
    // clamp.
    for &temp_k in &[100.0f32, 500.0, 999.0, 1000.0, 1000.1] {
        cases.push(BlackbodyCase {
            lambda_nm: 560.0,
            temp_k,
        });
        cases.push(BlackbodyCase {
            lambda_nm: 380.0,
            temp_k,
        });
        cases.push(BlackbodyCase {
            lambda_nm: 780.0,
            temp_k,
        });
    }
    cases
}

/// Runs the `blackbody_spectrum` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_blackbody(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<BlackbodyCase> {
    let cases = build_blackbody_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "blackbody cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "blackbody out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "blackbody_main",
        SHADER_SRC,
        "blackbody_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "blackbody bind group",
        &pipeline,
        &[(2, &in_buf), (3, &out_buf)],
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "blackbody_spectrum",
        BLACKBODY_ULP_BUDGET,
        BLACKBODY_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = blackbody_spectrum(case.lambda_nm, case.temp_k);
        acc.record(case, "value", cpu, gpu_out[idx]);
    }
    acc.finish()
}
