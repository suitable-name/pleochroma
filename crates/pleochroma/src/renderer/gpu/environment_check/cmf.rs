//! `cie_1931_cmf` ULP-budget self-test: [`build_cmf_lambdas`]'s dense wavelength sweep
//! plus adversarial points, dispatched by [`run_cmf`].

use crate::{color::cie1931::cie_1931_cmf, renderer::gpu::compute};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// ULP budget for `cie_1931_cmf`.
///
/// `cie_1931_cmf` is a 5 nm table lookup with linear interpolation (`lo + (hi - lo) * t`), one
/// multiply-add chain per component. The budget is a conservative ceiling for that short chain.
/// See [`build_cmf_lambdas`] for the cases.
pub const CMF_ULP_BUDGET: u32 = 32;

/// Absolute-difference floor exempting near-zero comparisons from [`CMF_ULP_BUDGET`]
/// entirely.
///
/// [`crate::color::cie1931::cie_1931_cmf`] returns exactly zero outside 380-780 nm, so an
/// in-range/out-of-range disagreement is a large absolute difference that this floor cannot
/// hide. The floor only absorbs rounding-direction noise near zero, such as `z_bar` at the
/// table's zero entries (630 nm and above).
pub const CMF_ABS_FLOOR: f32 = 1e-6;

/// Dense 0.5 nm sweep plus the table nodes, their midpoints and the range edges.
///
/// Nodes are exact lookups (`t == 0`) and midpoints the worst case of the interpolation
/// (`t == 0.5`). The edge and range probes exercise the `>=`/`<=` test and the `floor` index
/// of a WGSL port, the points most likely to fail there.
#[must_use]
pub fn build_cmf_lambdas() -> Vec<f32> {
    let mut lambdas = Vec::new();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the constant (850 - 300) / 0.5 = 1100 is positive and far below u32::MAX"
    )]
    let steps = ((850.0 - 300.0) / 0.5) as u32;
    for step in 0..=steps {
        lambdas.push((step as f32).mul_add(0.5, 300.0));
    }
    lambdas.push(380.0);
    lambdas.push(780.0);
    // Every table node (380, 385, ..., 780 nm) and its interpolation midpoint.
    for node in 0..81u32 {
        let lambda = (node as f32).mul_add(5.0, 380.0);
        lambdas.push(lambda);
        if node < 80 {
            lambdas.push(lambda + 2.5);
        }
    }
    // Epsilon-scale probes just inside/outside the tabulated 380-780nm range, plus
    // points far outside it.
    for &lambda in &[379.999f32, 380.001, 779.999, 780.001, -100.0, 2000.0] {
        lambdas.push(lambda);
    }
    lambdas
}

/// Runs the `cie_1931_cmf` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_cmf(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<f32> {
    let lambdas = build_cmf_lambdas();
    let total = lambdas.len();

    let in_buf = compute::upload(
        &ctx.device,
        "cmf lambdas",
        &lambdas,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "cmf out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "cmf_main", SHADER_SRC, "cmf_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "cmf bind group",
        &pipeline,
        &[(0, &in_buf), (1, &out_buf)],
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
    let gpu_xyz: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new("cie_1931_cmf", CMF_ULP_BUDGET, CMF_ABS_FLOOR);
    for (idx, &lambda) in lambdas.iter().enumerate() {
        let cpu = cie_1931_cmf(lambda);
        acc.record(&lambda, "x", cpu[0], gpu_xyz[idx * 3]);
        acc.record(&lambda, "y", cpu[1], gpu_xyz[idx * 3 + 1]);
        acc.record(&lambda, "z", cpu[2], gpu_xyz[idx * 3 + 2]);
    }
    acc.finish()
}
