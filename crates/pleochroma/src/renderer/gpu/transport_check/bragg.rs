//! Bragg domains: the domain map, the reflection coefficient, the delta tracker, the mosaic
//! mirror and `maybe_scatter_oriented_bragg`.
//!
//! The WGSL twin is `shaders/transport_physics/12_bragg_domains.wgsl`, driven by
//! `shaders/transport_functions/10_bragg.wgsl`. Every expected value is computed by the CPU
//! functions of [`crate::optics::inclusions::domains`], [`crate::optics::inclusions::bragg`] and
//! [`crate::optics::raytracer::scattering::delta_tracking`] in [`cpu_outputs`], never hard-coded.
//! The structure is encoded by [`structure_rows`], the very function the scene upload uses, so
//! the check covers the encoding too.
//!
//! The decisions inside the tracker (which domain is nearest, whether the hero accepts) are
//! discrete, so a one-ULP difference of an input could in principle flip one; with the case table
//! below that has not been seen to matter, and a failure of the `reflect` slot alone points there.

use crate::{
    optics::{
        inclusions::{
            InclusionSet,
            bragg::{BraggStructure, bragg_reflect},
            domains::domain_at,
        },
        polarization::StokesVector,
        raytracer::{
            Ray,
            scattering::{
                delta_tracking::{
                    BRAGG_MOSAIC_U_STREAM, BRAGG_MOSAIC_V_STREAM, BraggEvent, TrackSegment,
                    bragg_reflect_weights, draw, maybe_scatter_oriented_bragg, track_bragg,
                },
                oriented::OrientedMedium,
            },
        },
    },
    renderer::{buffers::structure_rows, gpu::compute},
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// Floats the shader writes per case: `[0]` domain on, `[1]` its `lambda_b0`, `[2..5)` its
/// normal, `[5..13)` the coefficient per channel along `dir` in that domain, `[13]` reflect,
/// `[14]` `t_model`, `[15..18)` the tracked domain's normal, `[18..26)` the channel weights,
/// `[26..29)` the mirrored direction, `[29]` scattered, `[30]` bragg, `[31]` `t_free`,
/// `[32..35)` the new direction, `[35..67)` the Stokes vectors, `[67..75)` `path_pdf`,
/// `[75..83)` the zoned reflection weights (`bragg_reflect_weights` of `depth_in`).
const OUT_FLOATS: usize = 83;

/// One input case, byte for byte the shader's `BraggCase` (384 bytes, no padding).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BraggCase {
    /// `[domain_seed, rng_seed, bounce, 0]`.
    header: [u32; 4],
    /// `structure_rows`, row 0: `[cell_size, lambda_low, lambda_high, purity]`.
    row0: [f32; 4],
    /// Row 1: `[coverage, mosaic, kappa, majorant]`.
    row1: [f32; 4],
    /// Row 2: the shared lattice normal, or zero.
    row2: [f32; 4],
    /// The point whose domain is queried.
    p: [f32; 4],
    /// The segment origin (xyz) and the estimator's facet distance (w).
    origin: [f32; 4],
    /// The unit direction (xyz) and the tracked length (w).
    dir: [f32; 4],
    /// `[sigma_hg, g, path_scale, 0]`.
    haze: [f32; 4],
    lambdas: [f32; 8],
    alphas: [f32; 8],
    stokes_in: [[f32; 4]; 8],
    path_pdf_in: [f32; 8],
    /// Synthetic zoned optical depths per channel at the tracked reflection, for
    /// `bragg_reflect_weights`.
    depth_in: [f32; 8],
}

const _: () = assert!(size_of::<BraggCase>() == 384);

/// The budget for every compared value. The chains are long (a hash-driven domain, a Box-Muller
/// draw, `exp` of a squared offset, a product of up to dozens of ratios); the same provisional
/// 2048 ULP (about 1.2e-4 relative) as the oriented check, until the owner's first harness run
/// reports the measured maximum.
const BRAGG_ULP_BUDGET: u32 = 2048;
const BRAGG_ABS_FLOOR: f32 = 1e-4;

/// The wavelength comb every case uses, nanometres; channel 0 is the hero, in the middle of the
/// first-order bands of the structures below so that it accepts reflections now and then.
const LAMBDAS: [f32; 8] = [560.0, 440.0, 480.0, 520.0, 600.0, 640.0, 680.0, 720.0];

/// The structures under test: an opal-like one (random normals), a labradorite-like one (one
/// shared normal), and a wide, dense one that reaches the second order and has no mosaic.
fn structures() -> Vec<BraggStructure> {
    vec![
        BraggStructure::new(12345, 0.2, (440.0, 680.0), 0.05, 0.8, 0.6, 0.03, Vec3::ZERO),
        BraggStructure::new(
            777,
            0.5,
            (470.0, 740.0),
            0.06,
            0.9,
            0.5,
            0.10,
            Vec3::new(0.3, 0.5, 0.8),
        ),
        BraggStructure::new(99, 0.05, (400.0, 850.0), 0.1, 1.0, 0.9, 0.0, Vec3::ZERO),
    ]
}

fn build_cases() -> Vec<(BraggCase, BraggStructure)> {
    let points = [
        Vec3::new(0.13, -0.27, 0.41),
        Vec3::new(-1.02, 0.55, 0.07),
        Vec3::new(0.9, 0.9, -0.9),
        Vec3::new(-0.31, -0.62, -1.4),
        Vec3::new(2.2, -1.7, 0.3),
        Vec3::new(0.0, 0.0, 0.0),
    ];
    let dirs = [
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.6, 0.2, 0.77).normalize(),
        Vec3::new(0.8, -0.1, 0.2).normalize(),
    ];
    // (rng_seed, bounce).
    let draws = [(0u32, 0u32), (7, 3), (0xDEAD_BEEF, 5)];
    // (sigma_hg, g, path_scale).
    let hazes = [(0.0f32, 0.0f32, 1.0f32), (0.3, 0.4, 2.5)];
    let alphas: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.02));
    let stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            0.05 * k as f32,
            -0.03 * k as f32,
            0.02 * k as f32,
        ]
    });
    let pdf: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.5));

    let mut cases = Vec::new();
    for structure in structures() {
        let rows = structure_rows(&structure);
        for (pi, &p) in points.iter().enumerate() {
            for &dir in &dirs {
                for &(seed, bounce) in &draws {
                    for &(sigma_hg, g, path_scale) in &hazes {
                        // Two depth sets: one with the hero (channel 0) the LEAST absorbed, so
                        // every `depth_k - depth_hero` is positive and `bragg_reflect_weights4`
                        // sees exp of a negative argument, and one with the hero the DEEPEST, so
                        // it sees exp of a positive argument (a channel less absorbed than the
                        // hero, the case the zoned route feeds when the hero sits in an
                        // absorbing band).
                        for hero_deepest in [false, true] {
                            let origin = points[(pi + 2) % points.len()];
                            cases.push((
                                BraggCase {
                                    header: [structure.seed, seed, bounce, 0],
                                    row0: rows[0],
                                    row1: rows[1],
                                    row2: rows[2],
                                    p: [p.x, p.y, p.z, 0.0],
                                    origin: [origin.x, origin.y, origin.z, 1.5],
                                    dir: [dir.x, dir.y, dir.z, 1.2],
                                    haze: [sigma_hg, g, path_scale, 0.0],
                                    lambdas: LAMBDAS,
                                    alphas,
                                    stokes_in: stokes,
                                    path_pdf_in: pdf,
                                    depth_in: std::array::from_fn(|k| {
                                        // A second synthetic coefficient set, so the channel
                                        // differences are not those of `alphas` alone.
                                        let rank = if hero_deepest { 7 - k } else { k };
                                        0.07f32.mul_add(rank as f32, alphas[k]) * 1.2
                                    }),
                                },
                                structure,
                            ));
                        }
                    }
                }
            }
        }
    }
    cases
}

/// The CPU's answer for `case` on `structure`, in the shader's output layout.
fn cpu_outputs(structure: &BraggStructure, case: &BraggCase) -> [f32; OUT_FLOATS] {
    let p = Vec3::new(case.p[0], case.p[1], case.p[2]);
    let origin = Vec3::new(case.origin[0], case.origin[1], case.origin[2]);
    let dir = Vec3::new(case.dir[0], case.dir[1], case.dir[2]);
    let (rng_seed, bounce) = (case.header[1], case.header[2]);

    let mut out = [0.0f32; OUT_FLOATS];
    let domain = domain_at(structure, p);
    out[0] = f32::from(u8::from(domain.active));
    out[1] = domain.lambda_b0_nm;
    out[2..5].copy_from_slice(&domain.normal.to_array());
    let cos_theta = dir.dot(domain.normal).abs();
    for (k, slot) in out[5..13].iter_mut().enumerate() {
        *slot = structure.sigma(&domain, cos_theta, LAMBDAS[k]);
    }

    let track = track_bragg(
        structure,
        &TrackSegment {
            origin,
            dir,
            t_max_model: case.dir[3],
            lambdas: &LAMBDAS,
            hero_idx: 0,
            rng_seed,
            bounce,
        },
    );
    if let BraggEvent::Reflect { t_model, domain } = track.event {
        out[13] = 1.0;
        out[14] = t_model;
        out[15..18].copy_from_slice(&domain.normal.to_array());
    }
    out[18..26].copy_from_slice(&track.null_weight);

    let u1 = draw(rng_seed, bounce, BRAGG_MOSAIC_U_STREAM, 0);
    let u2 = draw(rng_seed, bounce, BRAGG_MOSAIC_V_STREAM, 0);
    let mut mirrored = bragg_reflect(dir, domain.normal, structure.mosaic, u1, u2);
    if mirrored == Vec3::ZERO {
        mirrored = dir;
    }
    out[26..29].copy_from_slice(&mirrored.to_array());

    let set = InclusionSet::EMPTY;
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: case.haze[0],
        g: case.haze[1],
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: case.haze[2],
    };
    let mut stokes: [StokesVector; 8] = std::array::from_fn(|k| {
        StokesVector::new(
            case.stokes_in[k][0],
            case.stokes_in[k][1],
            case.stokes_in[k][2],
            case.stokes_in[k][3],
        )
    });
    let mut path_pdf = case.path_pdf_in;
    let event = maybe_scatter_oriented_bragg(
        &medium,
        structure,
        &case.alphas,
        Ray { origin, dir },
        case.origin[3],
        rng_seed,
        bounce,
        &mut stokes,
        &mut path_pdf,
    );
    if let Some(found) = event {
        out[29] = 1.0;
        out[30] = f32::from(u8::from(found.bragg));
        out[31] = found.scatter.t_free_model;
        out[32..35].copy_from_slice(&found.scatter.new_dir.to_array());
    }
    for (k, s) in stokes.iter().enumerate() {
        out[35 + k * 4..39 + k * 4].copy_from_slice(&[s.i, s.q, s.u, s.v]);
    }
    out[67..75].copy_from_slice(&path_pdf);
    out[75..83].copy_from_slice(&bragg_reflect_weights(
        &case.depth_in,
        0,
        &track.null_weight,
    ));
    out
}

/// Component names for the output slots, for the failure report.
const fn slot_name(slot: usize) -> &'static str {
    match slot {
        0 => "domain_on",
        1 => "domain_lambda_b0",
        2..=4 => "domain_normal",
        5..=12 => "sigma",
        13 => "reflect",
        14 => "t_model",
        15..=17 => "track_normal",
        18..=25 => "channel_weight",
        26..=28 => "mirrored_dir",
        29 => "scattered",
        30 => "bragg",
        31 => "t_free",
        32..=34 => "new_dir",
        35..=66 => "stokes",
        67..=74 => "path_pdf",
        75..=82 => "zoned_reflect_weight",
        _ => "out_of_range",
    }
}

/// Runs the Bragg-domain check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
#[must_use]
pub fn run_bragg(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<BraggCase> {
    let built = build_cases();
    let cases: Vec<BraggCase> = built.iter().map(|(case, _)| *case).collect();
    let total = cases.len();
    let in_buf = compute::upload(&ctx.device, "bragg in", &cases, wgpu::BufferUsages::STORAGE);
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "bragg out",
        total * OUT_FLOATS,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "bragg_main", SHADER_SRC, "bragg_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "bragg bind group",
        &pipeline,
        &[(130, &in_buf), (131, &out_buf)],
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
        "bragg domains (domain map, sigma, delta tracker, mosaic mirror, maybe_scatter_oriented_bragg)",
        BRAGG_ULP_BUDGET,
        BRAGG_ABS_FLOOR,
    );
    for (idx, (case, structure)) in built.iter().enumerate() {
        let cpu = cpu_outputs(structure, case);
        let base = idx * OUT_FLOATS;
        for (slot, &expected) in cpu.iter().enumerate() {
            acc.record(case, slot_name(slot), expected, gpu_out[base + slot]);
        }
    }
    acc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{optics::materials::GemMaterial, renderer::buffers::GpuInclusionBlock};

    /// A material with a structure encodes its flag, seed and rows; one without stays all zero.
    #[test]
    fn the_scene_upload_encodes_the_structure_rows() {
        for structure in structures() {
            let material = GemMaterial::diamond().with_structure(structure);
            let block = GpuInclusionBlock::encode(&material);
            assert_eq!(block.header, [0, 1, structure.seed, 0]);
            assert_eq!(block.structure, structure_rows(&structure));
            assert_eq!(block.structure[0][0], structure.cell_size);
            assert_eq!(block.structure[1][2], structure.kappa());
            assert_eq!(block.structure[1][3], structure.majorant());
            assert_eq!(
                Vec3::new(
                    block.structure[2][0],
                    block.structure[2][1],
                    block.structure[2][2]
                ),
                structure.normal
            );
        }
        let plain = GpuInclusionBlock::encode(&GemMaterial::diamond());
        assert_eq!(plain, GpuInclusionBlock::default());
    }

    /// The case table reaches every branch of the estimator: reflections and passes, with and
    /// without the second order.
    #[test]
    fn the_case_table_covers_reflections_and_passes() {
        let (mut reflected, mut passed) = (0, 0);
        for (case, structure) in build_cases() {
            let out = cpu_outputs(&structure, &case);
            if out[30] == 1.0 {
                reflected += 1;
            } else {
                passed += 1;
            }
        }
        assert!(reflected > 0, "no Bragg reflection in the case table");
        assert!(passed > 0, "no pass in the case table");
    }
}
