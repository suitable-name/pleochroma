//! Scattering: Henyey-Greenstein phase/sample, and `maybe_scatter_or_extinguish`.

use crate::{
    optics::{
        polarization::StokesVector,
        raytracer::{
            henyey_greenstein_phase, maybe_scatter_or_extinguish,
            sample_henyey_greenstein_direction,
        },
    },
    renderer::gpu::compute,
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::henyey_greenstein_phase.
// Compared against the real CPU function.
// ---------------------------------------------------------------------------------

/// One input case for the hg phase check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HgPhaseCase {
    cos_theta: f32,
    g: f32,
    _pad0: f32,
    _pad1: f32,
}

/// ULP budget for every case outside the near-degenerate corner.
///
/// See [`is_hg_phase_near_degenerate`]. `pow(x, 1.5)` diverges across platforms more than the
/// `sqrt`/`exp` calls of the other budgets, so this leaves headroom above them. The NEE path
/// evaluates the phase directly as a MIS density, so the budget stays tight.
const HG_PHASE_ULP_BUDGET: u32 = 64;

/// ULP budget for the near-degenerate corner of [`is_hg_phase_near_degenerate`].
///
/// There `g` and `cos_theta` are both within 0.1 of an extreme (+1 or -1), the denominator
/// `1 + g^2 - 2*g*cos_theta` sits near its `1e-6` clamp, and `pow(x, 1.5)` amplifies the
/// rounding difference between CPU `mul_add` and GPU `fma`. The wide budget is confined to this
/// corner: the NEE path evaluates the phase directly for its MIS weight, so a real porting error
/// must not hide behind this headroom.
const HG_PHASE_NEAR_DEGENERATE_ULP_BUDGET: u32 = 16384;

/// Absolute floor for the phase comparison.
///
/// Near `cos_theta -> 1, g -> 1` the phase spikes as the denominator approaches its clamp, and
/// the spike value is numerically sensitive; the floor keeps that regime from dominating the ULP
/// budget. Both accumulators share it because it concerns the compared value, not the budget.
const HG_PHASE_ABS_FLOOR: f32 = 1e-3;

/// True when `g` and `cos_theta` are both at least 0.9 in absolute value.
///
/// This is the corner [`HG_PHASE_NEAR_DEGENERATE_ULP_BUDGET`] covers. The 0.9 threshold, rather
/// than 0.95, keeps bank entries such as `g = -0.95` inside it despite f32 rounding in `v + 1.0`.
fn is_hg_phase_near_degenerate(case: &HgPhaseCase) -> bool {
    fn near_an_extreme(v: f32) -> bool {
        v.abs() >= 0.9
    }
    near_an_extreme(case.g) && near_an_extreme(case.cos_theta)
}

/// Merges the near-degenerate accumulator's [`UlpCheckResult`] into the ordinary one, so
/// [`run_hg_phase`] still reports one result. `primary` supplies the label, budget and floor.
/// Counts are summed. The worst case comes from the group that failed its own budget if only
/// one did, else from the group with the larger `max_ulp`.
fn merge_hg_phase_results(
    mut primary: UlpCheckResult<HgPhaseCase>,
    secondary: UlpCheckResult<HgPhaseCase>,
) -> UlpCheckResult<HgPhaseCase> {
    // A failing group's `max_ulp` and `argmax` must win over a passing group's. Otherwise the
    // corner's within-budget maximum (budget 16384) could be reported as the argmax of a failure
    // in the ordinary group (budget 64). Decided before the counts are summed below.
    let secondary_wins = if primary.passed() == secondary.passed() {
        secondary.max_ulp > primary.max_ulp
    } else {
        !secondary.passed()
    };
    primary.total_comparisons += secondary.total_comparisons;
    primary.over_budget_count += secondary.over_budget_count;
    primary.exempted_count += secondary.exempted_count;
    primary.max_raw_ulp = primary.max_raw_ulp.max(secondary.max_raw_ulp);
    if secondary_wins {
        primary.max_ulp = secondary.max_ulp;
        primary.argmax = secondary.argmax;
    }
    primary
        .over_budget_samples
        .extend(secondary.over_budget_samples);
    primary.over_budget_samples.truncate(8);
    primary
}

fn build_hg_phase_cases() -> Vec<HgPhaseCase> {
    let mut cases = Vec::new();
    let cos_thetas: Vec<f32> = (0..=20).map(|i| (i as f32).mul_add(0.1, -1.0)).collect();
    let gs = [-0.95f32, -0.7, -0.4, -0.1, 0.0, 0.1, 0.4, 0.7, 0.95];
    for &ct in &cos_thetas {
        for &g in &gs {
            cases.push(HgPhaseCase {
                cos_theta: ct,
                g,
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }
    }
    // Adversarial: exact forward/backward directions, near-isotropic g, near-extreme g and
    // the exact g == 0 case. Pairs with `cos_theta` and `g` both at an extreme (e.g. 1.0 and
    // 0.999) drive the denominator `1 + g^2 - 2*g*cos_theta` down to its `1e-6` clamp. That is
    // a genuine near-singularity, and CPU and GPU rounding can land on either side of the floor.
    // `pow` then amplifies the difference. `g = 0.98` keeps the case strongly peaked without
    // sitting on the singularity.
    let adversarial = [
        (1.0f32, 0.98),
        (1.0, -0.98),
        (-1.0, 0.98),
        (-1.0, -0.98),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 0.0),
        (0.5, 1e-4),
        (0.5, -1e-4),
    ];
    for &(ct, g) in &adversarial {
        cases.push(HgPhaseCase {
            cos_theta: ct,
            g,
            _pad0: 0.0,
            _pad1: 0.0,
        });
    }
    cases
}

fn cpu_hg_phase(c: &HgPhaseCase) -> f32 {
    henyey_greenstein_phase(c.cos_theta, c.g)
}

/// Runs the hg phase check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_hg_phase(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HgPhaseCase> {
    let cases = build_hg_phase_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "hg phase in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hg phase out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "hg_phase_main", SHADER_SRC, "hg_phase_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hg phase bind group",
        &pipeline,
        &[(30, &in_buf), (31, &out_buf)],
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

    // Two accumulators, not one -- the near-degenerate corner
    // (`is_hg_phase_near_degenerate`) is checked against the wide budget, every other
    // case against the tight one, so a real porting error on the path that matters
    // (the NEE path, see `HG_PHASE_NEAR_DEGENERATE_ULP_BUDGET`'s doc comment) cannot
    // hide behind headroom the corner alone needs.
    let mut ordinary = UlpAccumulator::new(
        "henyey_greenstein_phase",
        HG_PHASE_ULP_BUDGET,
        HG_PHASE_ABS_FLOOR,
    );
    let mut near_degenerate = UlpAccumulator::new(
        "henyey_greenstein_phase (near g,cos_theta -> +-1)",
        HG_PHASE_NEAR_DEGENERATE_ULP_BUDGET,
        HG_PHASE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let acc = if is_hg_phase_near_degenerate(case) {
            &mut near_degenerate
        } else {
            &mut ordinary
        };
        acc.record(case, "phase", cpu_hg_phase(case), gpu_out[idx]);
    }
    merge_hg_phase_results(ordinary.finish(), near_degenerate.finish())
}

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::sample_henyey_greenstein_direction.
// Compared against the real CPU function.
// ---------------------------------------------------------------------------------

/// One input case for the hg sample check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HgSampleCase {
    u1: f32,
    u2: f32,
    g: f32,
    _pad0: f32,
    forward: [f32; 3],
    _pad1: f32,
}

/// Covers a CDF-inversion division, a `sqrt`, a `sin_cos` pair and an orthonormal-basis
/// construction: the cosine-hemisphere shape plus one more division.
const HG_SAMPLE_ULP_BUDGET: u32 = 128;
/// Absolute floor, wider than the cosine-hemisphere floor (1e-5).
///
/// With `u1`/`u2` near the `[0, 1)` edges, an output component can be a near-zero off-axis
/// residual of the CDF inversion, and its absolute difference then reaches about 7e-5. ULP is
/// the wrong metric at such a zero crossing; see `crate::renderer::gpu::ulp::within_tolerance`.
const HG_SAMPLE_ABS_FLOOR: f32 = 2e-4;

fn build_hg_sample_cases() -> Vec<HgSampleCase> {
    let mut cases = Vec::new();
    let forwards = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.95, 0.1, 0.05).normalize(),
    ];
    let gs = [-0.9f32, -0.5, -0.1, 0.0, 0.1, 0.5, 0.9];
    let steps = 8;
    for &forward in &forwards {
        for &g in &gs {
            for i in 0..=steps {
                let u1 = (i as f32 / steps as f32).mul_add(0.998, 0.001);
                for j in 0..=steps {
                    let u2 = j as f32 / steps as f32;
                    cases.push(HgSampleCase {
                        u1,
                        u2,
                        g,
                        _pad0: 0.0,
                        forward: forward.to_array(),
                        _pad1: 0.0,
                    });
                }
            }
        }
    }
    // Adversarial: u1/u2 at the boundary, g straddling the isotropic-branch threshold
    // (+-1e-3), g near +-1 (the CDF inversion's own near-singular regime).
    let adversarial_u_g = [
        (0.0f32, 0.0f32, 0.5f32),
        (1.0 - 1e-6, 0.999_999, 0.5),
        (0.5, 0.0, 1e-3 - 1e-6),
        (0.5, 0.0, -(1e-3 - 1e-6)),
        (0.5, 0.0, 1e-3 + 1e-6),
        (0.5, 0.0, 0.999),
        (0.5, 0.0, -0.999),
        (1e-7, 0.25, 0.7),
        (0.999_999, 0.75, -0.7),
    ];
    for &forward in &forwards {
        for &(u1, u2, g) in &adversarial_u_g {
            cases.push(HgSampleCase {
                u1,
                u2,
                g,
                _pad0: 0.0,
                forward: forward.to_array(),
                _pad1: 0.0,
            });
        }
    }
    cases
}

fn cpu_hg_sample(c: &HgSampleCase) -> [f32; 3] {
    sample_henyey_greenstein_direction(c.u1, c.u2, c.g, Vec3::from_array(c.forward)).to_array()
}

/// Runs the hg sample check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_hg_sample(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HgSampleCase> {
    let cases = build_hg_sample_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "hg sample in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hg sample out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "hg_sample_main",
        SHADER_SRC,
        "hg_sample_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hg sample bind group",
        &pipeline,
        &[(32, &in_buf), (33, &out_buf)],
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
        "sample_henyey_greenstein_direction",
        HG_SAMPLE_ULP_BUDGET,
        HG_SAMPLE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_hg_sample(case);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, cpu[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::maybe_scatter_or_extinguish, the homogeneous-medium free-path
// sampler and per-channel extinction/scatter weighting. Compared against the real CPU function.
// See its doc comment for the estimator (hazards 1-5).
// ---------------------------------------------------------------------------------

/// One input case for the scatter or extinguish check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ScatterOrExtinguishCase {
    sigma_s: f32,
    g: f32,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    /// Path-length scale passed to the estimator; 1.0 is the default.
    path_scale: f32,
    _pad1: u32,
    _pad2: u32,
    ray_dir: [f32; 3],
    _pad3: f32,
    alphas: [f32; 8],
    stokes_in: [[f32; 4]; 8],
    path_pdf_in: [f32; 8],
}

const _: () = assert!(size_of::<ScatterOrExtinguishCase>() == 240);

/// Covers the distance-sampling chain, up to 8 channel `exp` calls and the HG direction sample.
///
/// The chain is the `log`/`exp`/division of hazard 2 in `maybe_scatter_or_extinguish`, the
/// sample is [`HG_SAMPLE_ULP_BUDGET`] on the scatter branch. The budget sits above that sum
/// rather than at it, since rounding does not compose exactly across a composite's parts.
const SCATTER_ULP_BUDGET: u32 = 512;
const SCATTER_ABS_FLOOR: f32 = 1e-5;

/// Distinct per-channel `alphas`, Stokes and `path_pdf` values, so a cross-channel indexing bug
/// cannot hide behind identical inputs. The frosted-bounce check uses the same approach.
fn scatter_varied_inputs() -> ([f32; 8], [[f32; 4]; 8], [f32; 8]) {
    let varied_alphas: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.02));
    let varied_stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            0.05 * k as f32,
            -0.03 * k as f32,
            0.02 * k as f32,
        ]
    });
    let varied_pdf: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.5));
    (varied_alphas, varied_stokes, varied_pdf)
}

fn build_scatter_cases() -> Vec<ScatterOrExtinguishCase> {
    let mut cases = Vec::new();
    let (varied_alphas, varied_stokes, varied_pdf) = scatter_varied_inputs();
    let uniform_zero_alphas = [0.0f32; 8];

    let ray_dirs = [
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.6, 0.2, 0.77).normalize(),
    ];
    let sigma_gs = [(0.3f32, 0.0f32), (0.8, 0.4), (1.5, -0.6), (0.05, 0.9)];
    // hit_t spans both sides of the free-path sample for the sigma_t values above. Small hit_t
    // favors survival and large hit_t favors scatter, so both branches are covered.
    let hit_ts = [0.05f32, 0.3, 1.0, 3.0];
    let seeds = [0u32, 1, 7, 42, 1000, 0xDEAD_BEEF, 0xC0FF_EE00];
    let bounces = [0u32, 3, 5, 9];
    // 1.0 (the default) plus a larger and a smaller value, so both branches are covered at each.
    let path_scales = [1.0f32, 3.0, 0.4];

    for &alphas in &[uniform_zero_alphas, varied_alphas] {
        for &ray_dir in &ray_dirs {
            for &(sigma_s, g) in &sigma_gs {
                for &hit_t in &hit_ts {
                    for &seed in &seeds {
                        for &bounce in &bounces {
                            for &path_scale in &path_scales {
                                cases.push(ScatterOrExtinguishCase {
                                    sigma_s,
                                    g,
                                    hit_t,
                                    rng_seed: seed,
                                    bounce,
                                    path_scale,
                                    _pad1: 0,
                                    _pad2: 0,
                                    ray_dir: ray_dir.to_array(),
                                    _pad3: 0.0,
                                    alphas,
                                    stokes_in: varied_stokes,
                                    path_pdf_in: varied_pdf,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Returns `(scattered, t_free, new_dir, stokes_out, path_pdf_out)`.
fn cpu_scatter_or_extinguish(
    c: &ScatterOrExtinguishCase,
) -> (bool, f32, [f32; 3], [[f32; 4]; 8], [f32; 8]) {
    let mut stokes: [StokesVector; 8] = std::array::from_fn(|k| {
        StokesVector::new(
            c.stokes_in[k][0],
            c.stokes_in[k][1],
            c.stokes_in[k][2],
            c.stokes_in[k][3],
        )
    });
    let mut path_pdf = c.path_pdf_in;
    let outcome = maybe_scatter_or_extinguish(
        &c.alphas,
        c.sigma_s,
        c.g,
        0,
        Vec3::from_array(c.ray_dir),
        c.hit_t,
        c.path_scale,
        c.rng_seed,
        c.bounce,
        &mut stokes,
        &mut path_pdf,
    );
    let (scattered, t_free, new_dir) = match outcome {
        Some((t, d)) => (true, t, d.to_array()),
        None => (false, 0.0, [0.0; 3]),
    };
    let stokes_out: [[f32; 4]; 8] =
        std::array::from_fn(|k| [stokes[k].i, stokes[k].q, stokes[k].u, stokes[k].v]);
    (scattered, t_free, new_dir, stokes_out, path_pdf)
}

/// Runs the scatter or extinguish check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_scatter_or_extinguish(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<ScatterOrExtinguishCase> {
    let cases = build_scatter_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "scatter in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "scatter out",
        total * 45,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "scatter_or_extinguish_main",
        SHADER_SRC,
        "scatter_or_extinguish_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "scatter bind group",
        &pipeline,
        &[(34, &in_buf), (35, &out_buf)],
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 45);

    let mut acc = UlpAccumulator::new(
        "maybe_scatter_or_extinguish",
        SCATTER_ULP_BUDGET,
        SCATTER_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let (cpu_scattered, cpu_t_free, cpu_new_dir, cpu_stokes, cpu_pdf) =
            cpu_scatter_or_extinguish(case);
        let base = idx * 45;
        acc.record(
            case,
            "scattered",
            u32::from(cpu_scattered) as f32,
            gpu_out[base],
        );
        acc.record(case, "t_free", cpu_t_free, gpu_out[base + 1]);
        for (c_idx, comp) in ["dir_x", "dir_y", "dir_z"].iter().enumerate() {
            acc.record(case, comp, cpu_new_dir[c_idx], gpu_out[base + 2 + c_idx]);
        }
        for k in 0..8 {
            for (c_idx, comp) in ["i", "q", "u", "v"].iter().enumerate() {
                acc.record(
                    case,
                    comp,
                    cpu_stokes[k][c_idx],
                    gpu_out[base + 5 + k * 4 + c_idx],
                );
            }
        }
        for k in 0..8 {
            acc.record(case, "path_pdf", cpu_pdf[k], gpu_out[base + 37 + k]);
        }
    }
    acc.finish()
}
