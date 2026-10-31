//! Surface finish: the GGX lobe, total integrated scatter, the packed word with the scratch
//! direction, the ground-arm energy compensation and the ground event.
//!
//! The WGSL twin is `shaders/transport_physics/13_surface_ggx.wgsl`, driven by
//! `shaders/transport_functions/11_surface.wgsl`. Every expected value is computed by the CPU
//! functions of [`crate::optics::raytracer::surface`] in the `cpu_*` functions below, never
//! hard-coded. The words and flags (kind, packed halves, branch choices) are compared exactly: a
//! mismatch is recorded as `0` against `1`, which no ULP budget can hide.
//!
//! The compensation table is bound at 116 from [`compensation::table_values`], the same values the
//! shipped kernels read from the tail of the `dist_func` binding.

use crate::{
    optics::raytracer::surface::{
        FacetSurface, FacetingMachine, GgxLobe, GroundCtx, GroundInterface, GroundParams,
        SweepRange, compensation,
        facet_surface::{SurfaceKind, halo_alpha},
        micro::fresnel_unpolarized,
        sweep::{scratch_angle_deg, scratch_direction},
        tis::{tis_reflection, tis_transmission},
    },
    renderer::{buffers::machine_sweep, gpu::compute},
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// The budget for the closed-form pieces (lobe, TIS, albedo lookups, decoders).
const SURFACE_ULP_BUDGET: u32 = 1024;
/// The budget for the ground event: a micro-normal sample, a refraction and a table lookup chain.
const GROUND_ULP_BUDGET: u32 = 2048;
const SURFACE_ABS_FLOOR: f32 = 1e-5;
/// The direction across the scratches is built from WGSL `sin`/`cos` of the scratch angle,
/// which WGSL bounds only absolutely (2^-11); a near-zero component then counts thousands of
/// ULPs for a 2e-5 difference (AMD RADV, 2026-10-22). 1e-4 is a 0.006 degree tilt.
const DIRECTION_ABS_FLOOR: f32 = 1e-4;
/// The sweep's scratch angle comes from WGSL inverse trigonometry, likewise bounded only
/// absolutely: 0.0022 degrees apart near 30 degrees was 1174 ULPs on AMD RADV (2026-10-22).
/// 0.01 degree is far below any setting a faceting machine has.
const ANGLE_ABS_FLOOR_DEG: f32 = 1e-2;
/// TIS of a fine polish is of order 1e-6; the floor stays far below it.
const TIS_ABS_FLOOR: f32 = 1e-9;

/// Dispatches `entry` over `cases` and reads `out_floats` floats per case back.
fn dispatch<C: bytemuck::Pod>(
    ctx: &crate::renderer::gpu::GpuContext,
    entry: &str,
    cases: &[C],
    bindings: (u32, u32),
    out_floats: usize,
    with_table: bool,
) -> Vec<f32> {
    let total = cases.len();
    let in_buf = compute::upload(&ctx.device, entry, cases, wgpu::BufferUsages::STORAGE);
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        entry,
        total * out_floats,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let table_buf = compute::upload(
        &ctx.device,
        "surface compensation table",
        compensation::table_values(),
        wgpu::BufferUsages::STORAGE,
    );
    let pipeline = compute::create_compute_pipeline(&ctx.device, entry, SHADER_SRC, entry);
    let mut entries = vec![(bindings.0, &in_buf), (bindings.1, &out_buf)];
    if with_table {
        entries.push((116, &table_buf));
    }
    let bind_group = compute::bind_buffers(&ctx.device, entry, &pipeline, &entries);
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
    compute::readback(&ctx.device, &ctx.queue, &out_buf, total * out_floats)
}

/// Records an exact (integer or flag) comparison as `0` against `0` or `1`.
fn record_exact<C: Clone>(
    acc: &mut UlpAccumulator<C>,
    case: &C,
    component: &'static str,
    cpu: f32,
    gpu: f32,
) {
    let mismatch = if (cpu - gpu).abs() < 0.5 { 0.0 } else { 1.0 };
    acc.record(case, component, 0.0, mismatch);
}

const fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

fn unit(x: f32, y: f32, z: f32) -> [f32; 3] {
    Vec3::new(x, y, z).normalize().to_array()
}

// ---------------------------------------------------------------------------------
// GGX lobe
// ---------------------------------------------------------------------------------

/// One lobe case, byte for byte the shader's `SurfLobeCase` (64 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SurfLobeCase {
    alpha_x: f32,
    alpha_y: f32,
    u1: f32,
    u2: f32,
    wi: [f32; 3],
    pad0: f32,
    wo: [f32; 3],
    pad1: f32,
    m: [f32; 3],
    pad2: f32,
}

const _: () = assert!(size_of::<SurfLobeCase>() == 64);

fn lobe_cases() -> Vec<SurfLobeCase> {
    let lobes = [
        (0.45, 0.45),
        (0.1, 0.1),
        (0.05, 0.05),
        (0.8, 0.8),
        (0.2, 0.6),
        (0.6, 0.15),
    ];
    let wis = [
        unit(0.0, 0.0, 1.0),
        unit(0.4, 0.2, 0.9),
        unit(-0.7, 0.3, 0.5),
        unit(0.2, -0.9, 0.15),
    ];
    let wos = [
        unit(0.3, 0.1, 0.8),
        unit(-0.5, 0.6, 0.3),
        unit(0.9, 0.0, 0.2),
    ];
    let ms = [
        unit(0.0, 0.0, 1.0),
        unit(0.1, 0.05, 0.99),
        unit(-0.3, 0.2, 0.7),
    ];
    let draws = [(0.13, 0.37), (0.5, 0.9), (0.81, 0.04), (0.99, 0.5)];
    let mut cases = Vec::new();
    for (l, &(alpha_x, alpha_y)) in lobes.iter().enumerate() {
        for (w, &wi) in wis.iter().enumerate() {
            let (u1, u2) = draws[(l + w) % draws.len()];
            cases.push(SurfLobeCase {
                alpha_x,
                alpha_y,
                u1,
                u2,
                wi,
                pad0: 0.0,
                wo: wos[(l + w) % wos.len()],
                pad1: 0.0,
                m: ms[(l * 2 + w) % ms.len()],
                pad2: 0.0,
            });
        }
    }
    cases
}

/// Runs the GGX lobe check (`d`, `lambda`, `g1`, `g2`, `visible_density`, `sample_visible`).
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_surface_lobe(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<SurfLobeCase> {
    let cases = lobe_cases();
    let gpu = dispatch(ctx, "surf_lobe_main", &cases, (110, 111), 8, false);
    let mut acc = UlpAccumulator::new(
        "surface GGX lobe (d, lambda, g1, g2, visible density, VNDF sample)",
        SURFACE_ULP_BUDGET,
        SURFACE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let lobe = GgxLobe {
            alpha_x: case.alpha_x,
            alpha_y: case.alpha_y,
        };
        let wi = Vec3::from_array(case.wi);
        let wo = Vec3::from_array(case.wo);
        let m = Vec3::from_array(case.m);
        let sampled = lobe.sample_visible(wi, case.u1, case.u2);
        let cpu = [
            ("d", lobe.d(m)),
            ("lambda", lobe.lambda(wi)),
            ("g1", lobe.g1(wi)),
            ("g2", lobe.g2(wi, wo)),
            ("visible_density", lobe.visible_density(wi, m)),
            ("sample.x", sampled.x),
            ("sample.y", sampled.y),
            ("sample.z", sampled.z),
        ];
        for (slot, (name, expected)) in cpu.into_iter().enumerate() {
            acc.record(case, name, expected, gpu[idx * 8 + slot]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// Total integrated scatter and the halo width
// ---------------------------------------------------------------------------------

/// One TIS case, byte for byte the shader's `SurfTisCase` (32 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SurfTisCase {
    rq_nm: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    lambda_nm: f32,
    pad: [f32; 3],
}

const _: () = assert!(size_of::<SurfTisCase>() == 32);

fn tis_cases() -> Vec<SurfTisCase> {
    let mut cases = Vec::new();
    for &rq_nm in &[0.5f32, 2.0, 5.0, 20.0, 80.0, 300.0] {
        for &(n1, n2) in &[(1.0f32, 1.76f32), (1.76, 1.0), (1.54, 1.0), (1.0, 1.54)] {
            for (k, &cos_i) in [0.2f32, 0.6, 0.95, 1.0].iter().enumerate() {
                cases.push(SurfTisCase {
                    rq_nm,
                    n1,
                    n2,
                    cos_i,
                    lambda_nm: if k % 2 == 0 { 550.0 } else { 450.0 },
                    pad: [0.0; 3],
                });
            }
        }
    }
    cases
}

/// Runs the TIS check (`tis_reflection`, `tis_transmission`, `halo_alpha`, the unpolarized
/// Fresnel reflectance).
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_surface_tis(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<SurfTisCase> {
    let cases = tis_cases();
    let gpu = dispatch(ctx, "surf_tis_main", &cases, (112, 113), 4, false);
    let mut acc = UlpAccumulator::new(
        "surface TIS (reflection, transmission, halo alpha, Fresnel)",
        SURFACE_ULP_BUDGET,
        TIS_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = [
            (
                "tis_reflection",
                tis_reflection(case.rq_nm, case.n1, case.cos_i, case.lambda_nm),
            ),
            (
                "tis_transmission",
                tis_transmission(case.rq_nm, case.n1, case.n2, case.cos_i, case.lambda_nm),
            ),
            ("halo_alpha", halo_alpha(case.rq_nm)),
            ("fresnel", fresnel_unpolarized(case.n1, case.n2, case.cos_i)),
        ];
        for (slot, (name, expected)) in cpu.into_iter().enumerate() {
            acc.record(case, name, expected, gpu[idx * 4 + slot]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// The packed word, the sweep and the scratch direction
// ---------------------------------------------------------------------------------

/// One word case, byte for byte the shader's `SurfWordCase` (48 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SurfWordCase {
    word: u32,
    rq_floor: f32,
    sweep_u: f32,
    pad0: f32,
    /// `(arm_offset_deg, a, b)` of `GpuSurfaceScene`.
    machine: [f32; 3],
    pad1: f32,
    normal: [f32; 3],
    pad2: f32,
}

const _: () = assert!(size_of::<SurfWordCase>() == 48);

const fn machines() -> [FacetingMachine; 3] {
    [
        FacetingMachine {
            arm_offset_deg: 7.5,
            lap_radius_mm: 100.0,
            mast_distance_mm: 30.0,
            sweep: SweepRange {
                inner_mm: 40.0,
                outer_mm: 80.0,
            },
        },
        FacetingMachine {
            arm_offset_deg: -12.0,
            lap_radius_mm: 100.0,
            mast_distance_mm: -25.0,
            sweep: SweepRange {
                inner_mm: 55.0,
                outer_mm: 95.0,
            },
        },
        FacetingMachine {
            arm_offset_deg: 3.0,
            lap_radius_mm: 100.0,
            mast_distance_mm: 0.0,
            sweep: SweepRange {
                inner_mm: 40.0,
                outer_mm: 80.0,
            },
        },
    ]
}

fn machine_slots(machine: &FacetingMachine) -> [f32; 3] {
    let (a, b) = machine_sweep(machine);
    [machine.arm_offset_deg, a, b]
}

fn word_surfaces() -> Vec<FacetSurface> {
    vec![
        FacetSurface::IDEAL,
        FacetSurface::frosted(),
        FacetSurface::polished(2.5, 2.0),
        FacetSurface::polished(0.8, 1.0),
        FacetSurface::polished(6.0, 4.0).with_custom_angle(33.0),
        FacetSurface::polished(2.5, 2.0).with_no_sweep(),
        FacetSurface::ground(0.3, 1.0),
        FacetSurface::ground(0.45, 3.0),
        FacetSurface::ground(0.12, 2.0).with_custom_angle(120.0),
        FacetSurface::ground(0.6, 1.5).with_no_sweep(),
    ]
}

fn word_cases() -> Vec<SurfWordCase> {
    let normals = [
        unit(0.3, 0.8, 0.52),
        // A table facet: the girdle direction falls back to model +X.
        unit(0.0, 1.0, 0.0),
        unit(0.9, 0.1, -0.4),
    ];
    let floors = [0.0f32, 3.0, 40.0];
    let us = [0.0f32, 0.37, 0.99];
    let mut cases = Vec::new();
    for (s, surface) in word_surfaces().into_iter().enumerate() {
        for (m, machine) in machines().iter().enumerate() {
            for (n, &normal) in normals.iter().enumerate() {
                cases.push(SurfWordCase {
                    word: surface.packed(),
                    rq_floor: floors[(s + m + n) % floors.len()],
                    sweep_u: us[(s + n) % us.len()],
                    pad0: 0.0,
                    machine: machine_slots(machine),
                    pad1: 0.0,
                    normal,
                    pad2: 0.0,
                });
            }
        }
    }
    cases
}

/// The machine a word case was built from (the slots only hold its shader form).
fn machine_of(case: &SurfWordCase) -> FacetingMachine {
    machines()
        .into_iter()
        .find(|machine| machine_slots(machine) == case.machine)
        .expect("a word case always carries the slots of one of the test machines")
}

/// Runs the packed-word check: the decoders, the raised roughness floor, the scratch angle of the
/// sweep and the direction across the scratches.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_surface_word(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<SurfWordCase> {
    let cases = word_cases();
    let gpu = dispatch(ctx, "surf_word_main", &cases, (114, 115), 12, false);
    let mut acc = UlpAccumulator::new(
        "surface packed word (decoders, floor, sweep angle, scratch direction)",
        SURFACE_ULP_BUDGET,
        SURFACE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let surface = FacetSurface::from_packed(case.word);
        let machine = machine_of(case);
        let normal = Vec3::from_array(case.normal);
        let angle = scratch_angle_deg(&surface, &machine, case.sweep_u);
        let across = normal.cross(scratch_direction(normal, angle));
        let raised = surface.with_rq_at_least(case.rq_floor).packed();
        let kind = match surface.kind() {
            SurfaceKind::Ideal => 0.0,
            SurfaceKind::Frosted => 1.0,
            SurfaceKind::Polished => 2.0,
            SurfaceKind::Ground => 3.0,
        };
        let out = &gpu[idx * 12..idx * 12 + 12];
        record_exact(&mut acc, case, "kind", kind, out[0]);
        acc.record(case, "rq_nm", surface.rq_nm(), out[1]);
        acc.record(case, "ground_alpha", surface.ground_alpha(), out[2]);
        acc.record(case, "anisotropy", surface.anisotropy(), out[3]);
        if let Some(custom) = surface.custom_angle_deg() {
            acc.record(case, "custom_angle", custom, out[4]);
        }
        acc.record_with_floor(case, "scratch_angle", angle, out[5], ANGLE_ABS_FLOOR_DEG);
        acc.record_with_floor(case, "across.x", across.x, out[6], DIRECTION_ABS_FLOOR);
        acc.record_with_floor(case, "across.y", across.y, out[7], DIRECTION_ABS_FLOOR);
        acc.record_with_floor(case, "across.z", across.z, out[8], DIRECTION_ABS_FLOOR);
        record_exact(
            &mut acc,
            case,
            "raised_low",
            (raised & 0xffff) as f32,
            out[9],
        );
        record_exact(
            &mut acc,
            case,
            "raised_high",
            (raised >> 16) as f32,
            out[10],
        );
        record_exact(
            &mut acc,
            case,
            "has_roughness",
            flag(surface.has_roughness()),
            out[11],
        );
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// The energy compensation
// ---------------------------------------------------------------------------------

/// One albedo case, byte for byte the shader's `SurfAlbedoCase` (16 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SurfAlbedoCase {
    alpha: f32,
    cos_i: f32,
    eta_rel: f32,
    pad: f32,
}

const _: () = assert!(size_of::<SurfAlbedoCase>() == 16);

fn albedo_cases() -> Vec<SurfAlbedoCase> {
    let mut cases = Vec::new();
    for &alpha in &[0.03f32, 0.05, 0.12, 0.3, 0.45, 0.7, 1.0] {
        for &cos_i in &[0.05f32, 0.3, 0.7, 1.0] {
            for &eta_rel in &[0.57f32, 0.65, 1.0, 1.54, 1.76] {
                cases.push(SurfAlbedoCase {
                    alpha,
                    cos_i,
                    eta_rel,
                    pad: 0.0,
                });
            }
        }
    }
    cases
}

/// Runs the directional-albedo lookup check against `compensation::ground_albedo`.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_surface_albedo(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<SurfAlbedoCase> {
    let cases = albedo_cases();
    let gpu = dispatch(ctx, "surf_albedo_main", &cases, (117, 118), 1, true);
    let mut acc = UlpAccumulator::new(
        "surface ground albedo (trilinear table lookup)",
        SURFACE_ULP_BUDGET,
        SURFACE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let expected = compensation::ground_albedo(case.alpha, case.cos_i, case.eta_rel);
        acc.record(case, "albedo", expected, gpu[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// The ground event
// ---------------------------------------------------------------------------------

/// One ground-event case, byte for byte the shader's `SurfGroundCase` (96 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SurfGroundCase {
    normal: [f32; 3],
    alpha_x: f32,
    k_in: [f32; 3],
    alpha_y: f32,
    across: [f32; 3],
    has_across: u32,
    o_dir: [f32; 3],
    reflect_eval: u32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    sin2_t: f32,
    u1: f32,
    u2: f32,
    u_branch: f32,
    pad: f32,
}

const _: () = assert!(size_of::<SurfGroundCase>() == 96);

#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
fn ground_cases() -> Vec<SurfGroundCase> {
    let normal = Vec3::new(0.2, 0.9, 0.3).normalize();
    let tangent = normal.cross(Vec3::X).normalize();
    // (alpha_x, alpha_y, has_across)
    let lobes = [
        (0.45f32, 0.45f32, false),
        (0.3, 0.6, true),
        (0.1, 0.1, false),
        (0.8, 0.8, false),
    ];
    // (n1, n2)
    let interfaces = [(1.0f32, 1.76f32), (1.76, 1.0), (1.54, 1.0)];
    // The incident direction against the macro normal: steep and shallow.
    let incidences = [0.9f32, 0.45];
    // (u1, u2, u_branch): a reflection draw and a transmission draw clear of every clamp.
    let draws = [
        (0.13f32, 0.37f32, 0.001f32),
        (0.62, 0.81, 0.999),
        (0.9, 0.2, 0.001),
    ];
    let mut cases = Vec::new();
    for (l, &(alpha_x, alpha_y, has_across)) in lobes.iter().enumerate() {
        for &(n1, n2) in &interfaces {
            for &cos_i in &incidences {
                let sin_i = (1.0 - cos_i * cos_i).sqrt();
                let k_in = (tangent * sin_i - normal * cos_i).normalize();
                let sin2_t = (n1 / n2) * (n1 / n2) * (1.0 - cos_i * cos_i);
                let mirror = k_in - 2.0 * k_in.dot(normal) * normal;
                for (d, &(u1, u2, u_branch)) in draws.iter().enumerate() {
                    let reflect_eval = (l + d) % 2 == 0;
                    let o_dir = if reflect_eval {
                        (mirror + 0.15 * tangent).normalize()
                    } else {
                        (k_in * 0.9 + 0.1 * tangent).normalize()
                    };
                    let across = (tangent * 0.8 + normal.cross(tangent) * 0.6).normalize();
                    cases.push(SurfGroundCase {
                        normal: normal.to_array(),
                        alpha_x,
                        k_in: k_in.to_array(),
                        alpha_y,
                        across: across.to_array(),
                        has_across: u32::from(has_across),
                        o_dir: o_dir.to_array(),
                        reflect_eval: u32::from(reflect_eval),
                        n1,
                        n2,
                        cos_i,
                        sin2_t,
                        u1,
                        u2,
                        u_branch,
                        pad: 0.0,
                    });
                }
            }
        }
    }
    cases
}

/// Runs the ground-event check: the sampled direction, weight and densities, and the evaluation of
/// a given outgoing direction, against [`GroundCtx`].
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_surface_ground(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<SurfGroundCase> {
    let cases = ground_cases();
    let gpu = dispatch(ctx, "surf_ground_main", &cases, (119, 120), 12, true);
    let mut acc = UlpAccumulator::new(
        "surface ground event (VNDF sample, weight, pdf, eval)",
        GROUND_ULP_BUDGET,
        SURFACE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let params = GroundParams {
            lobe: GgxLobe {
                alpha_x: case.alpha_x,
                alpha_y: case.alpha_y,
            },
            across: (case.has_across != 0).then(|| Vec3::from_array(case.across)),
        };
        let iface = GroundInterface {
            n1: case.n1,
            n2: case.n2,
            cos_i: case.cos_i,
            sin2_t: case.sin2_t,
        };
        let event = GroundCtx::new(
            Vec3::from_array(case.normal),
            Vec3::from_array(case.k_in),
            params,
            iface,
        );
        let sample = event.sample(case.u1, case.u2, case.u_branch);
        let eval = event.eval(Vec3::from_array(case.o_dir), case.reflect_eval != 0);
        let out = &gpu[idx * 12..idx * 12 + 12];
        acc.record(case, "dir.x", sample.dir.x, out[0]);
        acc.record(case, "dir.y", sample.dir.y, out[1]);
        acc.record(case, "dir.z", sample.dir.z, out[2]);
        record_exact(&mut acc, case, "crossed", flag(sample.crossed), out[3]);
        record_exact(
            &mut acc,
            case,
            "reflect_branch",
            flag(sample.reflect_branch),
            out[4],
        );
        acc.record(case, "weight", sample.weight, out[5]);
        acc.record(case, "branch_prob", sample.branch_prob, out[6]);
        acc.record(case, "pdf", sample.pdf, out[7]);
        record_exact(&mut acc, case, "valid", flag(sample.valid), out[8]);
        record_exact(&mut acc, case, "eval_ok", flag(eval.is_some()), out[9]);
        if let Some(eval) = eval {
            acc.record(case, "eval_weight", eval.weight, out[10]);
            acc.record(case, "eval_pdf", eval.pdf, out[11]);
        }
    }
    acc.finish()
}
