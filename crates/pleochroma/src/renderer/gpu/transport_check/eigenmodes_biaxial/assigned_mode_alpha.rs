//! Assigned-mode absorption, biaxial branch.
//!
//! Checks `optics::birefringence::{assigned_mode_alpha, BiaxialIndicatrix::assigned_mode_e_field}`
//! against `AbsorptionTensor3::biaxial`. This is the biaxial counterpart of
//! `absorption_pleochroism::run_assigned_mode_alpha_uniaxial`. `run_biaxial_pleochroic` stays a
//! separate check.
//!
//! `c_axis` is fed the same value as the indicatrix's gamma axis (column 2 of
//! `BiaxialIndicatrix::axes`). `GemMaterial::biaxial_indicatrix` builds the indicatrix with
//! `from_gamma_axis(.., self.c_axis)`, and the megakernel passes that same `material.c_axis`
//! as the tensor frame in `spectral_transport.wgsl`. Varying the two independently would
//! test a combination the pipeline never produces.

use glam::{Mat3, Vec3};

use crate::{
    optics::birefringence::{AbsorptionTensor3, BiaxialIndicatrix, assigned_mode_alpha},
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::{biaxial_test_directions, biaxial_test_indicatrices};

/// One input case for the assigned mode alpha biaxial check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AssignedModeAlphaBiaxialCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    is_extraordinary: u32,
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    ax0: [f32; 3],
    _pad1: f32,
    ax1: [f32; 3],
    _pad2: f32,
    ax2: [f32; 3],
    _pad3: f32,
    c_axis: [f32; 3],
    _pad4: f32,
    k: [f32; 3],
    _pad5: f32,
}

/// The GPU residual here is hardware `/` and `sqrt` precision, not a formula difference.
///
/// The eigenvector solve is near-degenerate for the built-in materials (their three principal
/// indices differ by only 0.1 to 0.3 %), and the anisotropic weights (`alpha_o`, `alpha_beta`,
/// `alpha_e`) amplify last-bit differences. The CPU and the GPU both stay close to the f64
/// truth, so the budget guards against real regressions (sign or axis errors land far above
/// it), not driver rounding. The worst measured case was about 1270 ULP on one driver.
const ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET: u32 = 1600;
const ASSIGNED_MODE_ALPHA_BIAXIAL_ABS_FLOOR: f32 = 1e-5;

fn build_assigned_mode_alpha_biaxial_cases() -> Vec<AssignedModeAlphaBiaxialCase> {
    let mut cases = Vec::new();
    let alpha_triples = [
        (0.0f32, 0.0f32, 0.0f32),
        (1.0, 1.0, 1.0),
        (0.5, 2.0, 3.5),
        (3.0, 0.2, 1.7),
    ];
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
        for k in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            for &(alpha_o, alpha_beta, alpha_e) in &alpha_triples {
                for is_extraordinary in [false, true] {
                    cases.push(AssignedModeAlphaBiaxialCase {
                        alpha_o,
                        alpha_beta,
                        alpha_e,
                        is_extraordinary: u32::from(is_extraordinary),
                        n_alpha,
                        n_beta,
                        n_gamma,
                        _pad0: 0.0,
                        ax0: ind.axes.x_axis.to_array(),
                        _pad1: 0.0,
                        ax1: ind.axes.y_axis.to_array(),
                        _pad2: 0.0,
                        ax2: ind.axes.z_axis.to_array(),
                        _pad3: 0.0,
                        c_axis: gamma_axis.to_array(),
                        _pad4: 0.0,
                        k: k.to_array(),
                        _pad5: 0.0,
                    });
                }
            }
        }
    }
    cases
}

/// Runs the assigned mode alpha biaxial check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_assigned_mode_alpha_biaxial(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<AssignedModeAlphaBiaxialCase> {
    let cases = build_assigned_mode_alpha_biaxial_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "assigned mode alpha biaxial in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "assigned mode alpha biaxial out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "assigned_mode_alpha_biaxial_main",
        SHADER_SRC,
        "assigned_mode_alpha_biaxial_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "assigned mode alpha biaxial bind group",
        &pipeline,
        &[(58, &in_buf), (59, &out_buf)],
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

    let mut acc = UlpAccumulator::new(
        "assigned_mode_alpha_biaxial",
        ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET,
        ASSIGNED_MODE_ALPHA_BIAXIAL_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let indicatrix = BiaxialIndicatrix::new(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Mat3::from_cols(
                Vec3::from_array(case.ax0),
                Vec3::from_array(case.ax1),
                Vec3::from_array(case.ax2),
            ),
        );
        let e_hat =
            indicatrix.assigned_mode_e_field(Vec3::from_array(case.k), case.is_extraordinary != 0);
        let tensor = AbsorptionTensor3::biaxial(
            case.alpha_o,
            case.alpha_beta,
            case.alpha_e,
            Vec3::from_array(case.c_axis),
        );
        let cpu = assigned_mode_alpha(&tensor, e_hat);
        acc.record(case, "alpha", cpu, gpu_out[idx]);
    }
    acc.finish()
}
