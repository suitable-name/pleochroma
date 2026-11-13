//! Exit-event spectral splitting: kernel-level equivalence checks.
//!
//! The checks cover the three per-channel helpers in
//! `renderer/shaders/transport_physics/08_exit_splitting.wgsl`: `compute_channel_transmission`,
//! `compute_uniaxial_exit_transmission` and `narrow_compat`.
//!
//! # Why the CPU side is a transcription
//!
//! The CPU originals cannot be called from the renderer tree. `compute_channel_transmission`
//! is `pub(super)` to `optics::raytracer::refraction`, `compute_uniaxial_exit_transmission`
//! is private to `refraction::uniaxial_internal`, and `narrow_compat` is `pub(super)` to
//! `refraction`. So the `cpu_*` functions below transcribe them, with the same operations
//! and `mul_add` sites. They are built on the real `MuellerMatrix`, `StokesVector` and `Cplx`
//! primitives, so only the glue arithmetic is duplicated.

use glam::Vec3;

use crate::{
    optics::{
        polarization::{MuellerMatrix, StokesVector},
        raytracer::{
            refraction::{R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN, directions_match},
            uniaxial_fresnel::Cplx,
        },
    },
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// compute_channel_transmission
// (optics::raytracer::refraction::exit_split)
// ---------------------------------------------------------------------------------

/// One input case for the channel transmission check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChannelTransmissionCase {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    entering_anisotropic: u32,
    azimuth_valid: u32,
    stokes_i: f32,
    stokes_q: f32,
    stokes_u: f32,
    stokes_v: f32,
}

const CHANNEL_TRANSMISSION_ULP_BUDGET: u32 = 64;
const CHANNEL_TRANSMISSION_ABS_FLOOR: f32 = 1e-6;

fn build_channel_transmission_cases() -> Vec<ChannelTransmissionCase> {
    let mut cases = Vec::new();
    let angles_deg = [10.0f32, 30.0, 50.0, 70.0, 80.0];
    let index_pairs = [
        (1.0f32, 1.5f32),
        (1.5, 1.0),
        (1.0, 2.616),
        (2.616, 1.0),
        (1.925, 1.0),
    ];
    let stokes_samples = [
        (1.0f32, 0.0f32, 0.0f32, 0.0f32),
        (1.0, 0.3, -0.4, 0.2),
        (1.0, 1.0, 0.0, 0.0),
    ];
    for &ang in &angles_deg {
        let cos_i = ang.to_radians().cos();
        for &(n1k, n2k) in &index_pairs {
            let eta = n1k / n2k;
            let sin2_t_k = eta * eta * cos_i.mul_add(-cos_i, 1.0);
            if sin2_t_k > 1.0 {
                continue;
            }
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            // `r_unpol` is the hero's own Fresnel reflectance at (n1k, n2k, cos_i), as in a
            // real bounce dispatch, so every case stays physically self-consistent. An
            // independent sweep could reach r_unpol near 0.02 or 0.98 at geometries no trace
            // produces. Those amplify `1 / (1 - r_unpol)` and the near-normal cancellation in
            // `MuellerMatrix::fresnel_transmission`.
            let r_s =
                f32::mul_add(n1k, -cos_i, n2k * cos_t_k) / f32::mul_add(n1k, cos_i, n2k * cos_t_k);
            let r_p =
                f32::mul_add(n2k, -cos_i, n1k * cos_t_k) / f32::mul_add(n2k, cos_i, n1k * cos_t_k);
            let r_unpol = (0.5 * f32::mul_add(r_s, r_s, r_p * r_p)).clamp(0.02, 0.98);
            for entering_anisotropic in [false, true] {
                for azimuth_valid in [false, true] {
                    for &(si, sq, su, sv) in &stokes_samples {
                        cases.push(ChannelTransmissionCase {
                            n1k,
                            n2k,
                            cos_i,
                            cos_t_k,
                            r_unpol,
                            cos_2psi_x: 0.6,
                            sin_2psi_x: 0.8,
                            entering_anisotropic: u32::from(entering_anisotropic),
                            azimuth_valid: u32::from(azimuth_valid),
                            stokes_i: si,
                            stokes_q: sq,
                            stokes_u: su,
                            stokes_v: sv,
                        });
                    }
                }
            }
        }
    }
    cases
}

/// Transcription of `refraction::compute_channel_transmission`; see the module doc for why.
/// Calls the real `MuellerMatrix::fresnel_transmission`, `StokesVector::apply_matrix` and
/// `scale`, so only the glue arithmetic is duplicated.
fn cpu_compute_channel_transmission(c: &ChannelTransmissionCase) -> [f32; 5] {
    let n1k = c.n1k;
    let n2k = c.n2k;
    let cos_i = c.cos_i;
    let cos_t_k = c.cos_t_k;
    let r_unpol = c.r_unpol;
    let entering_anisotropic = c.entering_anisotropic != 0;
    let azimuth = (c.azimuth_valid != 0).then_some((c.cos_2psi_x, c.sin_2psi_x));
    let incident_stokes_k = StokesVector::new(c.stokes_i, c.stokes_q, c.stokes_u, c.stokes_v);

    let t_perp_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_par_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_perp_k, t_par_k);
    let incident_k = if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = azimuth {
        let i_k = incident_stokes_k.i;
        StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
    } else {
        incident_stokes_k
    };
    let transmitted = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));
    let r_perp_k =
        f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_par_k =
        f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = (0.5 * r_par_k.mul_add(r_par_k, r_perp_k * r_perp_k))
        .clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    [
        transmitted.i,
        transmitted.q,
        transmitted.u,
        transmitted.v,
        r_unpol_k,
    ]
}

const CHANNEL_TRANSMISSION_COMPONENT_NAMES: [&str; 5] = [
    "transmitted.i",
    "transmitted.q",
    "transmitted.u",
    "transmitted.v",
    "r_unpol_k",
];

/// Runs the channel transmission check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_channel_transmission(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<ChannelTransmissionCase> {
    let cases = build_channel_transmission_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "channel_transmission in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "channel_transmission out",
        total * 5,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "channel_transmission_main",
        SHADER_SRC,
        "channel_transmission_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "channel_transmission bind group",
        &pipeline,
        &[(50, &in_buf), (51, &out_buf)],
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 5);

    let mut acc = UlpAccumulator::new(
        "compute_channel_transmission",
        CHANNEL_TRANSMISSION_ULP_BUDGET,
        CHANNEL_TRANSMISSION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_compute_channel_transmission(case);
        for c_idx in 0..5usize {
            acc.record(
                case,
                CHANNEL_TRANSMISSION_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 5 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// compute_uniaxial_exit_transmission
// (optics::raytracer::refraction::uniaxial_internal)
// ---------------------------------------------------------------------------------

/// One input case for the uniaxial exit transmission check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UniaxialExitTransmissionCase {
    t_s_re: f32,
    t_s_im: f32,
    t_p_re: f32,
    t_p_im: f32,
    flux_ts: f32,
    flux_tp: f32,
    flux_inc: f32,
    r_branch: f32,
    incident_i: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

const UNIAXIAL_EXIT_TRANSMISSION_ULP_BUDGET: u32 = 64;
const UNIAXIAL_EXIT_TRANSMISSION_ABS_FLOOR: f32 = 1e-6;

fn build_uniaxial_exit_transmission_cases() -> Vec<UniaxialExitTransmissionCase> {
    let mut cases = Vec::new();
    let amps = [0.0f32, 0.3, 0.7, 1.0, -0.2];
    let fluxes = [0.2f32, 0.6, 1.0, 1.8];
    let r_branch_vals = [0.02f32, 0.3, 0.6, 0.98];
    let incident_i_vals = [0.0f32, 0.5, 1.0, 3.5];
    for &t_s_re in &amps {
        for &t_p_re in &amps {
            for &flux_ts in &fluxes {
                for &flux_inc in &fluxes {
                    for &r_branch in &r_branch_vals {
                        for &incident_i in &incident_i_vals {
                            cases.push(UniaxialExitTransmissionCase {
                                t_s_re,
                                t_s_im: 0.1 * t_p_re,
                                t_p_re,
                                t_p_im: -0.15 * t_s_re,
                                flux_ts,
                                flux_tp: flux_ts.mul_add(0.7, 0.1),
                                flux_inc,
                                r_branch,
                                incident_i,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            });
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Transcription of `compute_uniaxial_exit_transmission` (`refraction::uniaxial_internal`);
/// see the module doc for why. Uses the real `Cplx` `scale`, `norm_sqr`, `mul` and `conj`.
fn cpu_compute_uniaxial_exit_transmission(c: &UniaxialExitTransmissionCase) -> [f32; 5] {
    let t_s = Cplx {
        re: c.t_s_re,
        im: c.t_s_im,
    };
    let t_p = Cplx {
        re: c.t_p_re,
        im: c.t_p_im,
    };
    let flux_inc = c.flux_inc.max(1e-12);
    let t_perp_n = t_s.scale((c.flux_ts / flux_inc).sqrt());
    let t_par_n = t_p.scale((c.flux_tp / flux_inc).sqrt());
    let i_unit = t_perp_n.norm_sqr() + t_par_n.norm_sqr();
    let q_unit = t_perp_n.norm_sqr() - t_par_n.norm_sqr();
    let cross = t_perp_n.mul(t_par_n.conj());
    let u_unit = 2.0 * cross.re;
    let v_unit = -2.0 * cross.im;
    let incident_i = c.incident_i;
    let transmitted = StokesVector::new(
        incident_i * i_unit,
        incident_i * q_unit,
        incident_i * u_unit,
        incident_i * v_unit,
    )
    .scale(1.0 / (1.0 - c.r_branch));
    [
        transmitted.i,
        transmitted.q,
        transmitted.u,
        transmitted.v,
        i_unit,
    ]
}

const UNIAXIAL_EXIT_TRANSMISSION_COMPONENT_NAMES: [&str; 5] = [
    "transmitted.i",
    "transmitted.q",
    "transmitted.u",
    "transmitted.v",
    "i_unit",
];

/// Runs the uniaxial exit transmission check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_uniaxial_exit_transmission(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<UniaxialExitTransmissionCase> {
    let cases = build_uniaxial_exit_transmission_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "uniaxial_exit_transmission in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "uniaxial_exit_transmission out",
        total * 5,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "uniaxial_exit_transmission_main",
        SHADER_SRC,
        "uniaxial_exit_transmission_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "uniaxial_exit_transmission bind group",
        &pipeline,
        &[(52, &in_buf), (53, &out_buf)],
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 5);

    let mut acc = UlpAccumulator::new(
        "compute_uniaxial_exit_transmission",
        UNIAXIAL_EXIT_TRANSMISSION_ULP_BUDGET,
        UNIAXIAL_EXIT_TRANSMISSION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_compute_uniaxial_exit_transmission(case);
        for c_idx in 0..5usize {
            acc.record(
                case,
                UNIAXIAL_EXIT_TRANSMISSION_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 5 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// narrow_compat (optics::raytracer::refraction::context) -- exact u32 equality, not a ULP
// comparison, because the output is an integer bitmask. So it uses its own result type
// rather than `UlpCheckResult`.
// ---------------------------------------------------------------------------------

/// One input case for the narrow compat check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NarrowCompatCase {
    dirs: [[f32; 4]; 8],
    hero_match_mask: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

#[derive(Debug, Clone, Copy)]
struct NarrowCompatMismatch {
    case_index: usize,
    channel: usize,
    cpu: u32,
    gpu: u32,
}

/// Outcome of the narrow compat check.
#[derive(Debug, Clone)]
pub struct NarrowCompatResult {
    /// Total number of cases run.
    pub total_cases: usize,
    /// Mismatches found.
    pub mismatches: usize,
    first_mismatch: Option<NarrowCompatMismatch>,
}

impl NarrowCompatResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches == 0
    }

    /// Human-readable description of the first mismatch.
    #[must_use]
    pub fn describe_first_mismatch(&self) -> String {
        self.first_mismatch.map_or_else(
            || "none".to_string(),
            |m| {
                format!(
                    "case {} channel {}: cpu=0x{:02x} gpu=0x{:02x}",
                    m.case_index, m.channel, m.cpu, m.gpu
                )
            },
        )
    }
}

fn build_narrow_compat_cases() -> Vec<NarrowCompatCase> {
    let mut cases = Vec::new();
    // A grid of direction sets. The hero (channel 0) direction is +Z. Each other channel is
    // exactly +Z (a match), a small perturbation inside the match tolerance (a match) or a
    // large one (a mismatch). Channel 4 always has `.w` = 0, which exercises the CPU `None`
    // path. `hero_match_mask` bit `j` is set when `directions_match` holds between channel `j`
    // and the hero, as a bounce-dispatch call site derives it.
    let hero_dir = Vec3::new(0.0, 0.0, 1.0);
    // 1 mrad off the hero, well inside the match tolerance, so CPU and kernel agree on it.
    let close = Vec3::new(0.001, 0.0, 1.0).normalize();
    let far = Vec3::new(0.6, 0.4, 0.6).normalize();
    let variants = [hero_dir, close, far];
    for pattern in 0..3usize.pow(7) {
        let mut p = pattern;
        let mut dirs = [[0.0f32; 4]; 8];
        dirs[0] = [hero_dir.x, hero_dir.y, hero_dir.z, 1.0];
        let mut hero_match_mask = 0u32;
        for (ch, slot) in dirs.iter_mut().enumerate().skip(1) {
            let variant = p % 3;
            p /= 3;
            let valid = ch % 4 != 0; // channel 4 always invalid, exercising Option::None
            let d = variants[variant];
            *slot = [d.x, d.y, d.z, f32::from(u8::from(valid))];
            if directions_match(d, hero_dir) {
                hero_match_mask |= 1 << ch;
            }
        }
        cases.push(NarrowCompatCase {
            dirs,
            hero_match_mask,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        });
        // 3^7 = 2187 cases is already a thorough sweep; cap it so this bank stays fast.
        if cases.len() >= 512 {
            break;
        }
    }
    cases
}

/// Transcription of `refraction::narrow_compat`, specialized to hero index 0 as in the WGSL
/// mirror. See the module doc for why it is not a direct call.
fn cpu_narrow_compat(case: &NarrowCompatCase) -> [u32; 8] {
    let dirs: [Option<Vec3>; 8] = std::array::from_fn(|k| {
        let d = case.dirs[k];
        (d[3] > 0.5).then(|| Vec3::new(d[0], d[1], d[2]))
    });
    let hero_match: [bool; 8] = std::array::from_fn(|k| (case.hero_match_mask >> k) & 1 != 0);
    let mut compat = [0xFFu32; 8];
    for a in 0..8 {
        for b in (a + 1)..8 {
            let matches = if a == 0 {
                hero_match[b]
            } else if b == 0 {
                hero_match[a]
            } else if let (Some(da), Some(db)) = (dirs[a], dirs[b]) {
                directions_match(da, db)
            } else {
                true
            };
            if !matches {
                compat[a] &= !(1u32 << b);
                compat[b] &= !(1u32 << a);
            }
        }
    }
    compat
}

/// Runs the narrow compat check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_narrow_compat(ctx: &crate::renderer::gpu::GpuContext) -> NarrowCompatResult {
    let cases = build_narrow_compat_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "narrow_compat in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<u32>(
        &ctx.device,
        "narrow_compat out",
        total * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "narrow_compat_main",
        SHADER_SRC,
        "narrow_compat_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "narrow_compat bind group",
        &pipeline,
        &[(54, &in_buf), (55, &out_buf)],
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
    let gpu_out: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 8);

    let mut mismatches = 0usize;
    let mut first_mismatch = None;
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_narrow_compat(case);
        for ch in 0..8usize {
            let gpu = gpu_out[idx * 8 + ch];
            if cpu[ch] != gpu {
                mismatches += 1;
                if first_mismatch.is_none() {
                    first_mismatch = Some(NarrowCompatMismatch {
                        case_index: idx,
                        channel: ch,
                        cpu: cpu[ch],
                        gpu,
                    });
                }
            }
        }
    }

    NarrowCompatResult {
        total_cases: total,
        mismatches,
        first_mismatch,
    }
}
