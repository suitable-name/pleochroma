//! Oriented scattering: the per-channel scattering coefficient, the mixture phase of every
//! channel, the mixture direction sampler and `maybe_scatter_oriented`.
//!
//! The WGSL twin is `shaders/transport_physics/11_oriented_scattering.wgsl`, driven by
//! `shaders/transport_functions/09_oriented_scattering.wgsl`. Every expected value is computed
//! by the CPU functions of [`crate::optics::raytracer::scattering::oriented`] in
//! [`build_cases`] and [`cpu_outputs`], never hard-coded. The zoned estimator reads the zone
//! table of the material binding, which the standalone Tier 2 unit does not have, so it is
//! covered by the zone-table echo and the Tier 3 image comparisons instead.

use crate::{
    optics::{
        inclusions::{FamilyKind, InclusionSet, ResolvedFamily},
        polarization::StokesVector,
        raytracer::scattering::oriented::{OrientedMedium, maybe_scatter_oriented},
    },
    renderer::{buffers::family_kind, gpu::compute},
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// Floats the shader writes per case: `[0..8)` the coefficient per channel, `[8]` the hero's
/// mixture phase, `[9..17)` every channel's, `[17..20)` the sampled direction, `[20]`
/// scattered, `[21]` `t_free`, `[22..25)` the new direction, `[25..57)` the Stokes vectors,
/// `[57..65)` `path_pdf`.
const OUT_FLOATS: usize = 65;

/// One input case, byte for byte the shader's `OrientedCase` (432 bytes, no padding).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OrientedCase {
    /// `[family_count, rng_seed, bounce, 0]`.
    header: [u32; 4],
    /// `GpuInclusionBlock::fam_dir`: world direction and kind.
    fam_dir: [[f32; 4]; 4],
    /// `GpuInclusionBlock::fam_params`: `[sigma, p, lobe, rho]`.
    fam_params: [[f32; 4]; 4],
    /// `[sigma_hg, g, path_scale, hit_t]`.
    haze: [f32; 4],
    /// The incoming propagation direction.
    w_i: [f32; 4],
    /// The scattered direction the phases are evaluated at.
    w_o: [f32; 4],
    /// `[u_c, u1, u2, 0]` for the direction sampler.
    u: [f32; 4],
    lambdas: [f32; 8],
    alphas: [f32; 8],
    stokes_in: [[f32; 4]; 8],
    path_pdf_in: [f32; 8],
}

const _: () = assert!(size_of::<OrientedCase>() == 432);

/// The budget for every compared value. The chains are long (a `log`/`exp` pair per channel,
/// the cone's `tan`/`atan`/`sin`/`cos`, up to 64 `exp`s in a random-needle sum), so this sits
/// well above `SCATTER_ULP_BUDGET`; it is about 1.2e-4 relative. Provisional until the owner's
/// first harness run reports the measured maximum.
const ORIENTED_ULP_BUDGET: u32 = 2048;
const ORIENTED_ABS_FLOOR: f32 = 1e-4;

/// The wavelength comb every case uses, nanometres; channel 0 is the hero.
const LAMBDAS: [f32; 8] = [420.0, 465.0, 510.0, 550.0, 590.0, 630.0, 670.0, 710.0];

const fn family(
    kind: FamilyKind,
    dir: Vec3,
    sigma: f32,
    p: u8,
    lobe: f32,
    rho: f32,
) -> ResolvedFamily {
    ResolvedFamily {
        kind,
        dir,
        sigma,
        p,
        lobe,
        rho,
    }
}

/// The media under test: the resolved set, the haze `sigma_s` and the haze asymmetry.
fn media() -> Vec<(InclusionSet, f32, f32)> {
    let needle_axis = Vec3::new(0.2, 0.3, 0.93).normalize();
    let needle = family(FamilyKind::Needle, needle_axis, 0.8, 2, 0.05, 0.5);
    let ideal_cone = family(FamilyKind::Needle, needle_axis, 0.8, 0, 0.0, 0.0);
    let second_needle = family(
        FamilyKind::Needle,
        Vec3::new(-0.7, 0.1, 0.7).normalize(),
        0.5,
        0,
        0.12,
        0.2,
    );
    let platelet = family(
        FamilyKind::Platelet,
        Vec3::new(0.0, 0.6, 0.8),
        0.7,
        1,
        0.25,
        0.0,
    );
    let random = family(FamilyKind::RandomNeedle, Vec3::ZERO, 0.6, 0, 0.08, 0.3);
    let random_blue = family(FamilyKind::RandomNeedle, Vec3::ZERO, 0.6, 4, 0.08, 0.3);
    vec![
        (InclusionSet::new(&[needle]), 0.0, 0.0),
        (InclusionSet::new(&[ideal_cone]), 0.0, 0.0),
        (InclusionSet::new(&[platelet]), 0.0, 0.0),
        (InclusionSet::new(&[random]), 0.0, 0.0),
        (
            InclusionSet::new(&[needle, platelet, random_blue]),
            0.3,
            0.4,
        ),
        (InclusionSet::new(&[needle, second_needle]), 0.2, -0.3),
        (InclusionSet::EMPTY, 0.5, 0.5),
    ]
}

/// The GPU encoding of one family (the same slots `GpuInclusionBlock::encode` fills).
fn encode_family(f: &ResolvedFamily) -> ([f32; 4], [f32; 4]) {
    let kind = match f.kind {
        FamilyKind::Needle => family_kind::NEEDLE,
        FamilyKind::Platelet => family_kind::PLATELET,
        FamilyKind::RandomNeedle => family_kind::RANDOM_NEEDLE,
    };
    (
        [f.dir.x, f.dir.y, f.dir.z, kind],
        [f.sigma, f32::from(f.p), f.lobe, f.rho],
    )
}

const fn medium_of(
    set: &InclusionSet,
    sigma_hg: f32,
    g: f32,
    path_scale: f32,
) -> OrientedMedium<'_> {
    OrientedMedium {
        set,
        sigma_hg_model: sigma_hg,
        g,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale,
    }
}

fn build_cases() -> Vec<OrientedCase> {
    let mut cases = Vec::new();
    let w_is = [
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.6, 0.2, 0.77).normalize(),
        Vec3::new(0.8, -0.1, 0.2).normalize(),
        // Along the first needle's axis: the azimuth origin is undefined there.
        Vec3::new(0.2, 0.3, 0.93).normalize(),
    ];
    // (u_c, u1, u2) paired with (rng_seed, bounce).
    let draws: [([f32; 3], (u32, u32)); 5] = [
        ([0.13, 0.37, 0.61], (0, 0)),
        ([0.5, 0.05, 0.9], (7, 3)),
        ([0.71, 0.83, 0.22], (0xDEAD_BEEF, 5)),
        ([0.93, 0.5, 0.5], (42, 9)),
        ([0.31, 0.66, 0.12], (1000, 4)),
    ];
    let hit_ts = [0.4f32, 3.0];
    let path_scales = [1.0f32, 2.5];
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

    for (set, sigma_hg, g) in media() {
        let mut fam_dir = [[0.0f32; 4]; 4];
        let mut fam_params = [[0.0f32; 4]; 4];
        for (slot, f) in set.families().iter().enumerate() {
            (fam_dir[slot], fam_params[slot]) = encode_family(f);
        }
        let n_fam = u32::try_from(set.families().len()).expect("at most four families");
        for &w_i in &w_is {
            for &(u, (seed, bounce)) in &draws {
                // A scattered direction on the medium's own mixture, so the phases are
                // evaluated where they are non-trivial.
                let medium = medium_of(&set, sigma_hg, g, 1.0);
                let w_o = medium.sample_direction(w_i, 0.37, 0.61, 0.23);
                for &hit_t in &hit_ts {
                    for &path_scale in &path_scales {
                        for alphas in [[0.0f32; 8], varied_alphas] {
                            cases.push(OrientedCase {
                                header: [n_fam, seed, bounce, 0],
                                fam_dir,
                                fam_params,
                                haze: [sigma_hg, g, path_scale, hit_t],
                                w_i: [w_i.x, w_i.y, w_i.z, 0.0],
                                w_o: [w_o.x, w_o.y, w_o.z, 0.0],
                                u: [u[0], u[1], u[2], 0.0],
                                lambdas: LAMBDAS,
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
    cases
}

/// The CPU's answer for `case`, in the shader's output layout.
fn cpu_outputs(case: &OrientedCase) -> [f32; OUT_FLOATS] {
    // Rebuild the set from the case's own encoding: the families are already sanitised.
    let families: Vec<ResolvedFamily> = (0..case.header[0] as usize)
        .map(|i| {
            // The kind travels as 0.0 / 1.0 / 2.0.
            let kind = if case.fam_dir[i][3] < 0.5 {
                FamilyKind::Needle
            } else if case.fam_dir[i][3] < 1.5 {
                FamilyKind::Platelet
            } else {
                FamilyKind::RandomNeedle
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the exponent is one of 0, 1, 2, 4, stored as a float"
            )]
            let p = case.fam_params[i][1] as u8;
            ResolvedFamily {
                kind,
                dir: Vec3::new(case.fam_dir[i][0], case.fam_dir[i][1], case.fam_dir[i][2]),
                sigma: case.fam_params[i][0],
                p,
                lobe: case.fam_params[i][2],
                rho: case.fam_params[i][3],
            }
        })
        .collect();
    let set = InclusionSet::new(&families);
    let medium = medium_of(&set, case.haze[0], case.haze[1], case.haze[2]);
    let w_i = Vec3::new(case.w_i[0], case.w_i[1], case.w_i[2]);
    let w_o = Vec3::new(case.w_o[0], case.w_o[1], case.w_o[2]);

    let mut out = [0.0f32; OUT_FLOATS];
    for (k, slot) in out.iter_mut().take(8).enumerate() {
        *slot = medium.sigma_s(w_i, LAMBDAS[k]);
    }
    let (hero_phase, per_channel) = medium.channel_phases(w_i, w_o);
    out[8] = hero_phase;
    out[9..17].copy_from_slice(&per_channel);
    let sampled = medium.sample_direction(w_i, case.u[0], case.u[1], case.u[2]);
    out[17..20].copy_from_slice(&sampled.to_array());

    let mut stokes: [StokesVector; 8] = std::array::from_fn(|k| {
        StokesVector::new(
            case.stokes_in[k][0],
            case.stokes_in[k][1],
            case.stokes_in[k][2],
            case.stokes_in[k][3],
        )
    });
    let mut path_pdf = case.path_pdf_in;
    let event = maybe_scatter_oriented(
        &medium,
        &case.alphas,
        w_i,
        case.haze[3],
        case.header[1],
        case.header[2],
        &mut stokes,
        &mut path_pdf,
    );
    if let Some(scatter) = event {
        out[20] = 1.0;
        out[21] = scatter.t_free_model;
        out[22..25].copy_from_slice(&scatter.new_dir.to_array());
    }
    for (k, s) in stokes.iter().enumerate() {
        out[25 + k * 4..29 + k * 4].copy_from_slice(&[s.i, s.q, s.u, s.v]);
    }
    out[57..65].copy_from_slice(&path_pdf);
    out
}

/// Component names for the output slots, for the failure report.
const fn slot_name(slot: usize) -> &'static str {
    match slot {
        0..=7 => "sigma_s",
        8 => "hero_phase",
        9..=16 => "channel_phase",
        17 => "sample_x",
        18 => "sample_y",
        19 => "sample_z",
        20 => "scattered",
        21 => "t_free",
        22 => "dir_x",
        23 => "dir_y",
        24 => "dir_z",
        25..=56 => "stokes",
        _ => "path_pdf",
    }
}

/// Runs the oriented-scattering check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_oriented(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<OrientedCase> {
    let cases = build_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "oriented in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "oriented out",
        total * OUT_FLOATS,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "oriented_main", SHADER_SRC, "oriented_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "oriented bind group",
        &pipeline,
        &[(110, &in_buf), (111, &out_buf)],
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
    let gpu_out: Vec<f32> =
        compute::readback(&ctx.device, &ctx.queue, &out_buf, total * OUT_FLOATS);

    let mut acc = UlpAccumulator::new(
        "oriented scattering (sigma_s, phases, sampler, maybe_scatter_oriented)",
        ORIENTED_ULP_BUDGET,
        ORIENTED_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_outputs(case);
        let base = idx * OUT_FLOATS;
        for (slot, &expected) in cpu.iter().enumerate() {
            acc.record(case, slot_name(slot), expected, gpu_out[base + slot]);
        }
    }
    acc.finish()
}
