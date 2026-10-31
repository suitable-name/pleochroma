//! Tier 2 check for the per-wavelength principal indices of a biaxial material
//! (`GemMaterial::biaxial_indicatrix` with `biaxial_principal_dispersion`).

use crate::{
    optics::{dispersion::DispersionModel, materials::GemMaterial},
    renderer::{
        buffers::GpuBiaxialCurves,
        gpu::{
            compute,
            transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
        },
    },
};

/// One input case for the biaxial principal-index check.
///
/// The curve block is the real [`GpuBiaxialCurves::encode`] output of the material under
/// test, so the Rust encoder and the WGSL reader are exercised together.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialCurvesCase {
    has_curves: u32,
    alpha_type: u32,
    gamma_type: u32,
    _pad0: u32,
    alpha_a: [f32; 4],
    alpha_b: [f32; 4],
    gamma_a: [f32; 4],
    gamma_b: [f32; 4],
    n_beta: f32,
    delta_beta_alpha: f32,
    birefringence_delta: f32,
    lambda_nm: f32,
}

/// The two curve evaluations dominate the error (the same Sellmeier arithmetic
/// `DispersionModel::evaluate` is checked with, budget 24); the sort and the achromatic
/// offsets are exact selections and f32 operations.
const BIAXIAL_CURVES_ULP_BUDGET: u32 = 32;
const BIAXIAL_CURVES_ABS_FLOOR: f32 = 1e-6;

/// Wavelengths covering the visible band and the Fraunhofer lines.
const LAMBDAS_NM: [f32; 9] = [
    380.0, 430.8, 486.1, 532.0, 589.3, 656.3, 700.0, 750.0, 780.0,
];

/// The materials under test: every built-in with per-axis curves, two built-ins without
/// (the achromatic branch), and two synthetic Topaz variants whose curves are reversed
/// against the base index and cross it inside the band (the sort).
fn curve_test_materials() -> Vec<GemMaterial> {
    let all = GemMaterial::all_materials();
    let mut out: Vec<GemMaterial> = all
        .iter()
        .filter(|m| m.biaxial_principal_dispersion.is_some())
        .cloned()
        .collect();
    for name in ["Topaz", "Tanzanite"] {
        if let Some(material) = all.iter().find(|m| m.name == name) {
            out.push(material.clone());
        }
    }
    if let Some(topaz) = all.iter().find(|m| m.name == "Topaz") {
        let mut reversed = topaz.clone();
        reversed.biaxial_principal_dispersion = Some([
            DispersionModel::Cauchy {
                a: 1.70,
                b: 0.0,
                c: 0.0,
            },
            DispersionModel::Cauchy {
                a: 1.50,
                b: 0.0,
                c: 0.0,
            },
        ]);
        out.push(reversed);
        let mut crossing = topaz.clone();
        crossing.biaxial_principal_dispersion = Some([
            DispersionModel::Cauchy {
                a: 1.55,
                b: 0.0125,
                c: 0.0,
            },
            DispersionModel::Cauchy {
                a: 1.66,
                b: 0.0,
                c: 0.0,
            },
        ]);
        out.push(crossing);
    }
    out
}

/// The cases, each with the CPU twin's `(n_alpha, n_beta, n_gamma)` at its wavelength.
fn build_biaxial_curves_cases() -> Vec<(BiaxialCurvesCase, [f32; 3])> {
    let mut cases = Vec::new();
    for material in curve_test_materials() {
        let curves = GpuBiaxialCurves::encode(&material);
        for lambda_nm in LAMBDAS_NM {
            // Expected values come from the CPU twin, never hard-coded.
            let Some(indicatrix) = material.biaxial_indicatrix(lambda_nm) else {
                continue;
            };
            cases.push((
                BiaxialCurvesCase {
                    has_curves: curves.types[2],
                    alpha_type: curves.types[0],
                    gamma_type: curves.types[1],
                    _pad0: 0,
                    alpha_a: curves.alpha_a,
                    alpha_b: curves.alpha_b,
                    gamma_a: curves.gamma_a,
                    gamma_b: curves.gamma_b,
                    n_beta: material.dispersion.evaluate(lambda_nm),
                    delta_beta_alpha: material.biaxial_delta_beta_alpha.unwrap_or(0.0),
                    birefringence_delta: material.birefringence_delta,
                    lambda_nm,
                },
                [indicatrix.n_alpha, indicatrix.n_beta, indicatrix.n_gamma],
            ));
        }
    }
    cases
}

/// Runs the biaxial principal-index check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_biaxial_curves(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialCurvesCase> {
    let built = build_biaxial_curves_cases();
    let cases: Vec<BiaxialCurvesCase> = built.iter().map(|(case, _)| *case).collect();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial curves in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial curves out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_curves_main",
        SHADER_SRC,
        "biaxial_curves_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial curves bind group",
        &pipeline,
        &[(100, &in_buf), (101, &out_buf)],
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "GemMaterial::biaxial_indicatrix (per-axis curves)",
        BIAXIAL_CURVES_ULP_BUDGET,
        BIAXIAL_CURVES_ABS_FLOOR,
    );
    for (idx, (case, expected)) in built.iter().enumerate() {
        acc.record(case, "n_alpha", expected[0], gpu_out[idx * 3]);
        acc.record(case, "n_beta", expected[1], gpu_out[idx * 3 + 1]);
        acc.record(case, "n_gamma", expected[2], gpu_out[idx * 3 + 2]);
    }
    acc.finish()
}
