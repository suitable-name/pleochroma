//! `intersect_polyhedron` GPU self-test: a case-bank check, because the output is a discrete
//! `Option<HitRecord>` (a facet index), not a smooth scalar.
//!
//! Dispatches `shaders/intersect_polyhedron.wgsl` against the 57-facet Standard Round Brilliant
//! plane set, taken from `geometry::cuts::StandardGemCuts::standard_round_brilliant`. Rays come
//! from outside the stone, and separately from inside it (the exit branch), plus adversarial
//! cases: rays exactly on a facet plane, rays perpendicular to the girdle facets' normals, and
//! near-silhouette grazing rays.
//!
//! Grazing rays where two facets have the same `t` within rounding can legitimately pick
//! different facets on CPU and GPU. These are counted as whitelisted ties, not failures; see
//! [`CaseOutcome`].

use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::raytracer::{Ray, intersect_polyhedron},
    renderer::{
        buffers::{GpuHitRecord, GpuRay},
        gpu::{compute, ulp::ulp_distance},
    },
};
use glam::Vec3;

const SHADER_SRC: &str = include_str!("../shaders/intersect_polyhedron.wgsl");

/// A hit's `(t, facet_idx, normal)` summary, or `None` for a miss -- shared by
/// [`PolyhedronMismatch`]'s `cpu_hit`/`gpu_hit` fields and [`classify_case`]'s return.
type HitSummary = Option<(f32, i64, Vec3)>;

/// `t` difference below which two different facet picks count as a grazing tie, not a bug.
/// It is the same order as the `1e-4` that `intersect_polyhedron` uses for "effectively at
/// the surface" in its entry/exit decision, so such facets are indistinguishable by its own
/// standard.
const TIE_EPS: f32 = 1e-4;

/// ULP budget for `t` and `normal` components once facet identity is established (an
/// exact facet match, or a whitelisted tie).
///
/// CPU and GPU run the same dot-product and division sequence over up to 57 planes, so a
/// difference beyond this budget points at the arithmetic, not at which facet won a tie. The
/// budget absorbs the rounding accumulated over those planes, and stays far below the change a
/// wrong epsilon or a dropped branch makes (geometry-scale changes of `t`).
pub const HIT_ULP_BUDGET: u32 = 512;

/// One ray case: origin + direction, plus a human-readable label for diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct PolyhedronCase {
    /// Short name of the check.
    pub label: &'static str,
    /// Ray origin.
    pub origin: Vec3,
    /// Ray direction.
    pub dir: Vec3,
}

/// `n` well-spaced directions on the unit sphere (a Fibonacci-sphere lattice), used to build
/// both outside-looking-in and inside-looking-out rays.
fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32) / ((n - 1).max(1) as f32);
            let radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let theta = golden_angle * i as f32;
            Vec3::new(theta.cos() * radius, y, theta.sin() * radius)
        })
        .collect()
}

/// Builds the full case bank: outside sweep, inside sweep, and hand-picked adversarial
/// cases. See the module doc comment for the categories.
#[must_use]
pub fn build_cases() -> Vec<PolyhedronCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(1024);

    // Outside, aimed dead at the center: the common case, exercises the entry branch
    // across a dense directional sweep.
    for &d in &directions {
        cases.push(PolyhedronCase {
            label: "outside_aimed_at_center",
            origin: d * 5.0,
            dir: -d,
        });
    }

    // Outside, aimed with a small lateral perturbation: pushes many rays toward the
    // silhouette edge where two facets' `t` values are nearly tied -- exactly the
    // scenario `TIE_EPS` exists to whitelist.
    for &d in &directions {
        let perturb = Vec3::new(d.z, d.x, -d.y) * 0.15;
        let dir = (-d + perturb).normalize();
        cases.push(PolyhedronCase {
            label: "outside_grazing_silhouette",
            origin: d * 5.0,
            dir,
        });
    }

    // Inside, dense directional sweep from the origin (which lies inside the polyhedron
    // for every built-in cut: every facet plane's `d < 0`, so `n.dot(0) + d < 0`,
    // i.e. the origin satisfies every half-space). This is the critical exit-branch
    // case.
    for &d in &directions {
        cases.push(PolyhedronCase {
            label: "inside_exit_sweep",
            origin: Vec3::ZERO,
            dir: d,
        });
    }

    // Adversarial: straight up/down from the origin -- perpendicular to every girdle
    // facet's normal (`denom == 0` for all 16 of them), exercising the `abs(denom) >
    // 1e-7` skip branch for a large contiguous block of planes at once.
    cases.push(PolyhedronCase {
        label: "adversarial_straight_up",
        origin: Vec3::ZERO,
        dir: Vec3::Y,
    });
    cases.push(PolyhedronCase {
        label: "adversarial_straight_down",
        origin: Vec3::ZERO,
        dir: -Vec3::Y,
    });

    // Adversarial: origin exactly on the table facet plane (y = 0.32), aimed straight
    // down into the stone (numer ~ 0 for that plane).
    cases.push(PolyhedronCase {
        label: "adversarial_on_table_plane_downward",
        origin: Vec3::new(0.0, 0.32, 0.0),
        dir: -Vec3::Y,
    });
    cases.push(PolyhedronCase {
        label: "adversarial_on_table_plane_upward",
        origin: Vec3::new(0.0, 0.32, 0.0),
        dir: Vec3::Y,
    });

    // Adversarial: origin exactly on the girdle cylinder, aimed tangentially (denom ~
    // 0 for that one girdle facet, non-zero for its neighbors).
    cases.push(PolyhedronCase {
        label: "adversarial_on_girdle_tangential",
        origin: Vec3::new(1.0, 0.0, 0.0),
        dir: Vec3::new(0.0, 0.0, 1.0),
    });

    // Adversarial: grazing incidence just above and just below the `abs(denom) > 1e-7`
    // threshold against the table facet's normal (0, 1, 0).
    for &eps in &[5e-7f32, 5e-8, -5e-7, -5e-8] {
        let dir = Vec3::new(1.0, eps, 0.0).normalize();
        cases.push(PolyhedronCase {
            label: "adversarial_near_denom_epsilon",
            origin: Vec3::new(-5.0, 0.32, 0.0),
            dir,
        });
    }

    // Adversarial: a ray that misses the stone entirely (aimed away from it).
    cases.push(PolyhedronCase {
        label: "adversarial_total_miss",
        origin: Vec3::new(5.0, 5.0, 5.0),
        dir: Vec3::new(1.0, 1.0, 1.0).normalize(),
    });

    cases
}

/// One case's classification after comparing CPU and GPU results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseOutcome {
    /// Both agreed: either both missed, or both hit the same facet within
    /// [`HIT_ULP_BUDGET`].
    Agree,
    /// Both hit, but a different facet, with `t` values within [`TIE_EPS`] of each
    /// other -- a legitimate grazing tie, not a bug.
    WhitelistedTie,
    /// A genuine disagreement: hit-status differed, or `t`/`normal` differed by more
    /// than [`HIT_ULP_BUDGET`], or facet indices differed with `t` values more than
    /// [`TIE_EPS`] apart.
    Mismatch,
}

/// One case's full comparison detail, kept for every non-`Agree` case (capped) so a
/// failure is diagnosable without re-running anything.
#[derive(Debug, Clone)]
pub struct PolyhedronMismatch {
    /// Index of the case in the generated list.
    pub case_index: usize,
    /// Input case that produced this result.
    pub case: PolyhedronCase,
    /// Outcome.
    pub outcome: CaseOutcome,
    /// Cpu hit.
    pub cpu_hit: HitSummary,
    /// Gpu hit.
    pub gpu_hit: HitSummary,
    /// Detail.
    pub detail: String,
}

/// Outcome of the `intersect_polyhedron` self-test.
#[derive(Debug, Clone)]
pub struct PolyhedronCheckResult {
    /// Total number of cases run.
    pub total_cases: usize,
    /// Whitelisted ties.
    pub whitelisted_ties: usize,
    /// True count of mismatching cases -- NOT capped, unlike `mismatches` below (which
    /// holds only the first 64 for diagnostics).
    pub total_mismatches: usize,
    /// Mismatches found.
    pub mismatches: Vec<PolyhedronMismatch>,
}

impl PolyhedronCheckResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Runs the `intersect_polyhedron` case-bank self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run(ctx: &crate::renderer::gpu::GpuContext) -> PolyhedronCheckResult {
    let planes: Vec<GpuFacetPlane> = StandardGemCuts::standard_round_brilliant();
    let cases = build_cases();
    let total = cases.len();

    let gpu_rays: Vec<GpuRay> = cases
        .iter()
        .map(|c| GpuRay::new(c.origin.to_array(), c.dir.to_array()))
        .collect();

    let planes_buf = compute::upload(
        &ctx.device,
        "intersect_polyhedron planes",
        &planes,
        wgpu::BufferUsages::STORAGE,
    );
    let rays_buf = compute::upload(
        &ctx.device,
        "intersect_polyhedron rays",
        &gpu_rays,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<GpuHitRecord>(
        &ctx.device,
        "intersect_polyhedron output",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );

    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "intersect_polyhedron", SHADER_SRC, "main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "intersect_polyhedron bind group",
        &pipeline,
        &[(0, &planes_buf), (1, &rays_buf), (2, &out_buf)],
    );

    let workgroups = u32::try_from(total).expect("a few cases").div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );

    let gpu_hits: Vec<GpuHitRecord> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut whitelisted_ties = 0usize;
    let mut total_mismatches = 0usize;
    let mut mismatches = Vec::new();

    for (idx, (case, gpu_hit)) in cases.iter().zip(gpu_hits.iter()).enumerate() {
        let ray = Ray {
            origin: case.origin,
            dir: case.dir,
        };
        let cpu_hit = intersect_polyhedron(ray, &planes);
        let (outcome, cpu_summary, gpu_summary) = classify_case(cpu_hit, gpu_hit);

        match outcome {
            CaseOutcome::Agree => {}
            CaseOutcome::WhitelistedTie => whitelisted_ties += 1,
            CaseOutcome::Mismatch => {
                total_mismatches += 1;
                if mismatches.len() < 64 {
                    let detail = format!(
                        "cpu={cpu_summary:?} gpu={gpu_summary:?} origin={:?} dir={:?}",
                        case.origin, case.dir
                    );
                    mismatches.push(PolyhedronMismatch {
                        case_index: idx,
                        case: *case,
                        outcome,
                        cpu_hit: cpu_summary,
                        gpu_hit: gpu_summary,
                        detail,
                    });
                }
            }
        }
    }

    PolyhedronCheckResult {
        total_cases: total,
        whitelisted_ties,
        total_mismatches,
        mismatches,
    }
}

/// One case's CPU-vs-GPU classification, pulled out of [`run`] to keep that function's
/// line count down. See [`CaseOutcome`]'s doc comment for what each variant means.
fn classify_case(
    cpu_hit: Option<crate::optics::raytracer::HitRecord>,
    gpu_hit: &GpuHitRecord,
) -> (CaseOutcome, HitSummary, HitSummary) {
    let cpu_summary = cpu_hit.map(|h| {
        (
            h.t,
            i64::try_from(h.facet_idx).expect("a facet index fits i64"),
            h.normal,
        )
    });
    let gpu_summary = (gpu_hit.hit != 0).then(|| {
        (
            gpu_hit.t,
            i64::from(gpu_hit.facet_idx),
            Vec3::from_array(gpu_hit.normal),
        )
    });

    let outcome = match (cpu_hit, gpu_hit.hit != 0) {
        (None, false) => CaseOutcome::Agree,
        (None, true) | (Some(_), false) => CaseOutcome::Mismatch,
        (Some(cpu), true) => {
            if i64::try_from(cpu.facet_idx).expect("a facet index fits i64")
                == i64::from(gpu_hit.facet_idx)
            {
                let t_ulp = ulp_distance(cpu.t, gpu_hit.t);
                let n_ulp = ulp_distance(cpu.normal.x, gpu_hit.normal[0])
                    .max(ulp_distance(cpu.normal.y, gpu_hit.normal[1]))
                    .max(ulp_distance(cpu.normal.z, gpu_hit.normal[2]));
                if t_ulp <= HIT_ULP_BUDGET && n_ulp <= HIT_ULP_BUDGET {
                    CaseOutcome::Agree
                } else {
                    CaseOutcome::Mismatch
                }
            } else if (cpu.t - gpu_hit.t).abs() < TIE_EPS {
                CaseOutcome::WhitelistedTie
            } else {
                CaseOutcome::Mismatch
            }
        }
    };

    (outcome, cpu_summary, gpu_summary)
}
