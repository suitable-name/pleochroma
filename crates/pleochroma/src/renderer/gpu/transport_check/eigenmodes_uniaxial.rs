//! Birefringent eigenmodes: uniaxial. `ordinary_eigen_polarization` /
//! `extraordinary_eigen_polarization`, `theta_c_for_bounce`,
//! `extraordinary_poynting_dir` (walk-off), and `per_channel_uniaxial_indices`.

use crate::{
    optics::{
        birefringence::BirefringenceParams,
        dispersion::DispersionModel,
        materials::GemMaterial,
        raytracer::{RayMaterialContext, per_channel_uniaxial_indices, theta_c_for_bounce},
    },
    renderer::gpu::compute,
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// ordinary_eigen_polarization / extraordinary_eigen_polarization
// ---------------------------------------------------------------------------------

/// One input case for the eigen polarization check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EigenPolarizationCase {
    wave_normal: [f32; 3],
    _pad0: f32,
    c_axis: [f32; 3],
    _pad1: f32,
}

const EIGEN_POLARIZATION_ULP_BUDGET: u32 = 32;
const EIGEN_POLARIZATION_ABS_FLOOR: f32 = 1e-5;

fn build_eigen_polarization_cases() -> Vec<EigenPolarizationCase> {
    let mut cases = Vec::new();
    let dirs: Vec<Vec3> = (0..20)
        .map(|i| {
            let theta = (i as f32 / 20.0) * std::f32::consts::PI;
            let phi = (i as f32 * 1.834_952) % (2.0 * std::f32::consts::PI);
            Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            )
        })
        .collect();
    for &wave_normal in &dirs {
        for &c_axis in &[Vec3::Y, Vec3::X, Vec3::new(0.3, 0.9, 0.1).normalize()] {
            cases.push(EigenPolarizationCase {
                wave_normal: wave_normal.to_array(),
                _pad0: 0.0,
                c_axis: c_axis.to_array(),
                _pad1: 0.0,
            });
        }
    }
    // Adversarial: wave_normal parallel (and near-parallel) to c_axis -- the degenerate
    // cross-product fallback branch.
    for &c_axis in &[Vec3::Y, Vec3::X] {
        cases.push(EigenPolarizationCase {
            wave_normal: c_axis.to_array(),
            _pad0: 0.0,
            c_axis: c_axis.to_array(),
            _pad1: 0.0,
        });
        cases.push(EigenPolarizationCase {
            wave_normal: Vec3::new(c_axis.x, c_axis.y + 1e-6, c_axis.z)
                .normalize()
                .to_array(),
            _pad0: 0.0,
            c_axis: c_axis.to_array(),
            _pad1: 0.0,
        });
    }
    cases
}

/// Runs the eigen polarization check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_eigen_polarization(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<EigenPolarizationCase> {
    let cases = build_eigen_polarization_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "eigen polarization in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "eigen polarization out",
        total * 6,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "eigen_polarization_main",
        SHADER_SRC,
        "eigen_polarization_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "eigen polarization bind group",
        &pipeline,
        &[(18, &in_buf), (19, &out_buf)],
    );
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
        .div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 6);

    let mut acc = UlpAccumulator::new(
        "eigen_polarization",
        EIGEN_POLARIZATION_ULP_BUDGET,
        EIGEN_POLARIZATION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let wave_normal = Vec3::from_array(case.wave_normal);
        let c_axis = Vec3::from_array(case.c_axis);
        let o_hat = BirefringenceParams::ordinary_eigen_polarization(wave_normal, c_axis);
        let e_hat = BirefringenceParams::extraordinary_eigen_polarization(wave_normal, c_axis);
        for (c_idx, comp) in ["ox", "oy", "oz"].iter().enumerate() {
            acc.record(
                case,
                comp,
                o_hat.to_array()[c_idx],
                gpu_out[idx * 6 + c_idx],
            );
        }
        for (c_idx, comp) in ["ex", "ey", "ez"].iter().enumerate() {
            acc.record(
                case,
                comp,
                e_hat.to_array()[c_idx],
                gpu_out[idx * 6 + 3 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// optics::raytracer::theta_c_for_bounce (the theta_c fixed-point iteration for the
// extraordinary index). Sweeps incidence angle and optic-axis orientation, including
// adversarial near-parallel and near-perpendicular wave-normal/c-axis pairs, at both of
// the built-in materials the Tier 3 statistical check also uses: Zircon
// (birefringence_delta = +0.0590, the largest in the material set) and Tourmaline
// (-0.0210, strongly negative).
// ---------------------------------------------------------------------------------

/// One input case for the theta c check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ThetaCCase {
    normal: [f32; 3],
    _pad0: f32,
    ray_dir: [f32; 3],
    _pad1: f32,
    c_axis: [f32; 3],
    _pad2: f32,
    cos_i: f32,
    inside_gem: u32,
    is_anisotropic: u32,
    n_o_hero_seed: f32,
    birefringence_delta: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

/// ULP budget for the two-iteration `theta_c` fixed point.
///
/// CPU and GPU converge to the same iterate (measured worst case 0 ULP). The budget is
/// headroom for compiler and driver float-codegen differences, not the measured maximum.
/// Near-parallel inputs have `theta_c` near 0, where a sub-ULP input difference is a large
/// relative ULP distance but stays below `THETA_C_ABS_FLOOR`, so the abs-floor exemption
/// covers them (see `ulp::within_tolerance`). A genuinely different iterate would show up as
/// a large, non-noise ULP spike in the reported argmax.
const THETA_C_ULP_BUDGET: u32 = 96;
const THETA_C_ABS_FLOOR: f32 = 1e-5;

fn build_theta_c_cases() -> Vec<ThetaCCase> {
    let mut cases = Vec::new();
    let dirs: Vec<Vec3> = (0..10)
        .map(|i| {
            let theta = (i as f32 / 10.0) * std::f32::consts::PI;
            let phi = (i as f32 * 2.399_963) % (2.0 * std::f32::consts::PI);
            Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            )
        })
        .collect();
    let c_axes = [Vec3::Y, Vec3::X, Vec3::new(0.3, 0.9, 0.1).normalize()];
    let n_seeds = [1.925f32, 1.624]; // Zircon's and Tourmaline's own n_o(D) magnitude.
    let deltas = [0.0590f32, -0.0210]; // Zircon, Tourmaline.

    for &ray_dir in &dirs {
        for &normal in &dirs {
            let cos_i = (-ray_dir).dot(normal).clamp(0.0, 1.0);
            if cos_i < 1e-3 {
                continue;
            }
            for &c_axis in &c_axes {
                for &n_o_hero_seed in &n_seeds {
                    for &birefringence_delta in &deltas {
                        for inside_gem in [false, true] {
                            for is_anisotropic in [true, false] {
                                cases.push(ThetaCCase {
                                    normal: normal.to_array(),
                                    _pad0: 0.0,
                                    ray_dir: ray_dir.to_array(),
                                    _pad1: 0.0,
                                    c_axis: c_axis.to_array(),
                                    _pad2: 0.0,
                                    cos_i,
                                    inside_gem: u32::from(inside_gem),
                                    is_anisotropic: u32::from(is_anisotropic),
                                    n_o_hero_seed,
                                    birefringence_delta,
                                    _pad3: 0.0,
                                    _pad4: 0.0,
                                    _pad5: 0.0,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Adversarial: wave normal near-parallel to c_axis (theta_c near 0, where the
    // extraordinary index is nearly degenerate with the ordinary one) and near
    // perpendicular (theta_c near pi/2, the maximum-birefringence direction). These are
    // the iteration's near-degenerate ends.
    for &c_axis in &[Vec3::Y, Vec3::X] {
        let near_parallel = Vec3::new(c_axis.x + 1e-4, c_axis.y + 1e-4, c_axis.z).normalize();
        let near_perp = c_axis.cross(Vec3::new(0.3, 0.4, 0.5)).normalize_or_zero();
        for &ray_dir in &[near_parallel, near_perp, -c_axis, -near_parallel] {
            let normal = Vec3::new(0.1, 0.95, 0.05).normalize();
            let cos_i = (-ray_dir).dot(normal).clamp(0.0, 1.0);
            if cos_i < 1e-3 {
                continue;
            }
            for &n_o_hero_seed in &n_seeds {
                for &birefringence_delta in &deltas {
                    cases.push(ThetaCCase {
                        normal: normal.to_array(),
                        _pad0: 0.0,
                        ray_dir: ray_dir.to_array(),
                        _pad1: 0.0,
                        c_axis: c_axis.to_array(),
                        _pad2: 0.0,
                        cos_i,
                        inside_gem: 0,
                        is_anisotropic: 1,
                        n_o_hero_seed,
                        birefringence_delta,
                        _pad3: 0.0,
                        _pad4: 0.0,
                        _pad5: 0.0,
                    });
                }
            }
        }
    }
    cases
}

fn cpu_theta_c(c: &ThetaCCase) -> f32 {
    let material =
        GemMaterial::new_custom("theta_c test", 1.8, 0.02, c.birefringence_delta, [0.0; 3]);
    let mat_ctx = RayMaterialContext {
        material: &material,
        lambdas: [550.0; 8],
        hero_idx: 0,
        c_axis: Vec3::from_array(c.c_axis),
        is_anisotropic: c.is_anisotropic != 0,
        enable_internal_mode_coupling: true,
    };
    theta_c_for_bounce(
        &mat_ctx,
        Vec3::from_array(c.normal),
        Vec3::from_array(c.ray_dir),
        c.cos_i,
        c.inside_gem != 0,
        false, // is_biaxial: always false -- biaxial materials never reach the GPU.
        c.n_o_hero_seed,
    )
}

/// Runs the theta c check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_theta_c(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<ThetaCCase> {
    let cases = build_theta_c_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "theta_c in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "theta_c out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "theta_c_main", SHADER_SRC, "theta_c_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "theta_c bind group",
        &pipeline,
        &[(20, &in_buf), (21, &out_buf)],
    );
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
        .div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new("theta_c_for_bounce", THETA_C_ULP_BUDGET, THETA_C_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        acc.record(case, "theta_c", cpu_theta_c(case), gpu_out[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// optics::birefringence::BirefringenceParams::extraordinary_poynting_dir (the
// extraordinary ray's walk-off direction, where the Poynting vector diverges from the
// wave normal).
// ---------------------------------------------------------------------------------

/// One input case for the walk off check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WalkOffCase {
    wave_normal: [f32; 3],
    _pad0: f32,
    c_axis: [f32; 3],
    _pad1: f32,
    n_o: f32,
    n_e: f32,
    _pad2: f32,
    _pad3: f32,
}

/// Walk-off is a few trig calls and a `normalize`, so it gets the same budget as
/// `EIGEN_POLARIZATION_ULP_BUDGET`. The measured worst case is 0 ULP.
const WALK_OFF_ULP_BUDGET: u32 = 32;
const WALK_OFF_ABS_FLOOR: f32 = 1e-5;

fn build_walk_off_cases() -> Vec<WalkOffCase> {
    let mut cases = Vec::new();
    let dirs: Vec<Vec3> = (0..16)
        .map(|i| {
            let theta = (i as f32 / 16.0) * std::f32::consts::PI;
            let phi = (i as f32 * 1.734_9) % (2.0 * std::f32::consts::PI);
            Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            )
        })
        .collect();
    let c_axes = [Vec3::Y, Vec3::X, Vec3::new(0.2, 0.6, 0.77).normalize()];
    // (n_o, n_e) pairs: Zircon (n_o(D)=1.925, delta=+0.0590 -> n_e=1.984) and Tourmaline
    // (n_o(D)=1.624, delta=-0.0210 -> n_e=1.603), plus a strongly dichroic synthetic pair
    // for a wide walk-off-angle sweep.
    let pairs = [
        (1.925f32, 1.925 + 0.0590),
        (1.624f32, 1.624 - 0.0210),
        (1.5f32, 1.7f32),
    ];

    for &wave_normal in &dirs {
        for &c_axis in &c_axes {
            for &(n_o, n_e) in &pairs {
                cases.push(WalkOffCase {
                    wave_normal: wave_normal.to_array(),
                    _pad0: 0.0,
                    c_axis: c_axis.to_array(),
                    _pad1: 0.0,
                    n_o,
                    n_e,
                    _pad2: 0.0,
                    _pad3: 0.0,
                });
            }
        }
    }

    // Adversarial: wave normal exactly along / near-parallel to c_axis (walk-off should
    // collapse to exactly zero -- the `delta.abs() < 1e-5` early-out) and roughly the
    // angle of maximum walk-off for these indices.
    for &c_axis in &[Vec3::Y, Vec3::X] {
        let near_parallel = Vec3::new(c_axis.x + 1e-5, c_axis.y, c_axis.z).normalize();
        let off_axis =
            (c_axis + c_axis.cross(Vec3::new(0.4, 0.3, 0.6)).normalize_or_zero()).normalize();
        for &wave_normal in &[c_axis, near_parallel, off_axis, -c_axis] {
            for &(n_o, n_e) in &pairs {
                cases.push(WalkOffCase {
                    wave_normal: wave_normal.to_array(),
                    _pad0: 0.0,
                    c_axis: c_axis.to_array(),
                    _pad1: 0.0,
                    n_o,
                    n_e,
                    _pad2: 0.0,
                    _pad3: 0.0,
                });
            }
        }
    }
    cases
}

fn cpu_walk_off(c: &WalkOffCase) -> Vec3 {
    BirefringenceParams::extraordinary_poynting_dir(
        Vec3::from_array(c.wave_normal),
        Vec3::from_array(c.c_axis),
        c.n_o,
        c.n_e,
    )
}

/// Runs the walk off check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_walk_off(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<WalkOffCase> {
    let cases = build_walk_off_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "walk off in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "walk off out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "walk_off_main", SHADER_SRC, "walk_off_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "walk off bind group",
        &pipeline,
        &[(22, &in_buf), (23, &out_buf)],
    );
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
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
        "extraordinary_poynting_dir",
        WALK_OFF_ULP_BUDGET,
        WALK_OFF_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_walk_off(case);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, cpu.to_array()[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// optics::raytracer::per_channel_uniaxial_indices (per-mode index evaluation -- the
// ordinary and effective-extraordinary index pair one channel's wavelength resolves to,
// feeding the existing refraction/transport machinery). Uses Zircon's and Tourmaline's
// own real Cauchy dispersion fits, not a synthetic curve.
// ---------------------------------------------------------------------------------

/// One input case for the per mode index check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PerModeIndexCase {
    model_type: u32,
    is_anisotropic: u32,
    _pad0: u32,
    _pad1: u32,
    param_a: [f32; 4],
    param_b: [f32; 4],
    lambda_nm: f32,
    birefringence_delta: f32,
    theta_c: f32,
    _pad2: f32,
}

/// Wraps `DispersionModel::evaluate` (budget 24) and adds one `effective_extraordinary_index`
/// call, so it gets a slightly larger budget. The measured worst case is 0 ULP.
const PER_MODE_INDEX_ULP_BUDGET: u32 = 32;
const PER_MODE_INDEX_ABS_FLOOR: f32 = 1e-6;

fn build_per_mode_index_cases() -> Vec<(PerModeIndexCase, DispersionModel)> {
    // Zircon (largest birefringence in the material set) and Tourmaline (strongly
    // negative), the same two the Tier 3 checks use.
    let materials = [
        (
            DispersionModel::Cauchy {
                a: 1.890_963,
                b: 0.011_820,
                c: 0.0,
            },
            0.0590f32,
        ),
        (
            DispersionModel::Cauchy {
                a: 1.624_481,
                b: 0.005_183,
                c: 0.0,
            },
            -0.0210f32,
        ),
    ];
    let lambdas: Vec<f32> = (0..=20)
        .map(|i| (i as f32 / 20.0).mul_add(400.0, 380.0))
        .collect();
    let theta_cs: Vec<f32> = (0..=8)
        .map(|i| (i as f32 / 8.0) * std::f32::consts::FRAC_PI_2)
        .collect();

    let mut out = Vec::new();
    for &(model, delta) in &materials {
        let (model_type, param_a, param_b) = match model {
            DispersionModel::Sellmeier1 { b1, c1 } => {
                (0u32, [b1, 0.0, 0.0, 0.0], [c1, 0.0, 0.0, 0.0])
            }
            DispersionModel::Sellmeier3 { b, c } => {
                (1u32, [b[0], b[1], b[2], 0.0], [c[0], c[1], c[2], 0.0])
            }
            DispersionModel::Cauchy { a, b, c } => (2u32, [a, b, c, 0.0], [0.0; 4]),
        };
        for &lambda_nm in &lambdas {
            for &theta_c in &theta_cs {
                for is_anisotropic in [true, false] {
                    out.push((
                        PerModeIndexCase {
                            model_type,
                            is_anisotropic: u32::from(is_anisotropic),
                            _pad0: 0,
                            _pad1: 0,
                            param_a,
                            param_b,
                            lambda_nm,
                            birefringence_delta: delta,
                            theta_c,
                            _pad2: 0.0,
                        },
                        model,
                    ));
                }
            }
        }
    }
    out
}

fn cpu_per_mode_index(case: &PerModeIndexCase, model: DispersionModel) -> (f32, f32) {
    let base = GemMaterial::new_custom(
        "per_mode_index test",
        1.8,
        0.02,
        case.birefringence_delta,
        [0.0; 3],
    );
    let material = GemMaterial {
        dispersion: model,
        ..base
    };
    let mat_ctx = RayMaterialContext {
        material: &material,
        lambdas: [case.lambda_nm; 8],
        hero_idx: 0,
        c_axis: Vec3::Y,
        is_anisotropic: case.is_anisotropic != 0,
        enable_internal_mode_coupling: true,
    };
    let (n_o_ch, n_eff_ch) = per_channel_uniaxial_indices(&mat_ctx, case.theta_c);
    (n_o_ch[0], n_eff_ch[0])
}

/// Runs the per mode index check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_per_mode_index(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<PerModeIndexCase> {
    let with_models = build_per_mode_index_cases();
    let cases: Vec<PerModeIndexCase> = with_models.iter().map(|(c, _)| *c).collect();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "per mode index in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "per mode index out",
        total * 2,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "per_mode_index_main",
        SHADER_SRC,
        "per_mode_index_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "per mode index bind group",
        &pipeline,
        &[(24, &in_buf), (25, &out_buf)],
    );
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
        .div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 2);

    let mut acc = UlpAccumulator::new(
        "per_channel_uniaxial_indices",
        PER_MODE_INDEX_ULP_BUDGET,
        PER_MODE_INDEX_ABS_FLOOR,
    );
    for (idx, (case, model)) in with_models.iter().enumerate() {
        let (cpu_n_o, cpu_n_eff) = cpu_per_mode_index(case, *model);
        acc.record(case, "n_o", cpu_n_o, gpu_out[idx * 2]);
        acc.record(case, "n_eff", cpu_n_eff, gpu_out[idx * 2 + 1]);
    }
    acc.finish()
}
