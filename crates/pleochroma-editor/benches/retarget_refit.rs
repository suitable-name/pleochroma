//! P07 measurement: what one flat refit probe costs, and what a whole Retarget check costs
//! (`C:\temp\mcp2\review_briefs_2026-10-26\perf\FABLE_perf_verification.md`, item P07).
//!
//! What this bench times, through public API only:
//!
//! - `probe_*`: the body of the ORIGINAL `refit::area_at` reproduced step for step
//!   (`Design::planes_from_solved` with one mast changed, `build_solid_mesh`, the ring-area
//!   sum for the flat's plane). One bisection step of the full-mesh refit is exactly one such
//!   probe.
//! - `refit_full_mesh` / `refit_clipped`: `retarget::refit::refit_flats_by` over a candidate
//!   whose pinned flats are all 0.7 x lower (bigger) than the design's, against the design's own
//!   analysis, with `AreaMethod::FullMesh` (a mesh at every probe, the refit as it was) and
//!   `AreaMethod::Clipped` (one mesh per flat, the other probes cut the flat's facet out of its
//!   own plane; what `check_retarget` runs now). Their ratio is the gain; `main` also prints that
//!   both find the same masts.
//! - `check_*`: the whole public Retarget check (`retarget::check::check_retarget`) with a
//!   crown policy that does not follow the pavilion, so the flats change size and the refit
//!   runs. This is the denominator: the share of a Retarget that the refit can take.
//!
//! Probes per refitted flat, from `refit.rs`: 2 bracket probes plus the bisection. The bracket
//! is `[1e-4, 3.0] * scale`, the loop stops once the bracket is narrower than `1e-10 * scale`:
//! `log2(3 / 1e-10)` is 34.8, so 35 steps, so 37 probes per flat that gets refitted (a flat
//! whose bracket does not enclose the target stops after 2). A table plus a culet is at most
//! 74 probes per refit.
//!
//! Estimated refit cost = 37 x `probe_authored` x (number of refitted flats). `main` prints,
//! per fixture, the plane count, the flat tiers and how many masts the check refitted
//! (`anchors`), so the estimate can be formed from one run.
//!
//! Run: `cargo bench -p pleochroma-editor --bench retarget_refit`.

use divan::black_box;
use pleochroma::{
    geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::{LightingPreset, materials::GemMaterial},
};
use pleochroma_cut_core::{BuiltinMaterials, Design, MaterialSelection, templates::TEMPLATES};
use pleochroma_editor::{
    retarget::{
        CrownShift,
        anchors::scale_reference_mast,
        build_plan,
        check::{CheckInputs, check_retarget},
        refit::{AreaMethod, refit_flats_by},
        validity::{StoneAnalysis, analyze, polygon_area},
    },
    templates::gallery_design,
};

/// Probes in one fully bisected refit of one flat (2 bracket probes plus 35 steps).
const PROBES_PER_FLAT: u32 = 37;

// Template names; the lower-fold templates have the most planes, `main` prints the real counts.
const OVAL: &str = "Oval Brilliant";
const CUSHION: &str = "Cushion Brilliant";
const EMERALD: &str = "Emerald Step Cut";
const ROUND: &str = "Standard Round Brilliant";

fn main() {
    for name in [ROUND, OVAL, CUSHION, EMERALD] {
        let (design, analysis) = fixture(name);
        let flats: Vec<&str> = analysis
            .flats
            .iter()
            .map(|flat| design.tiers[flat.tier_index].name.as_str())
            .collect();
        let (plan_target, check) = run_check(&design);
        eprintln!(
            "[retarget_refit] {name}: {} planes, flats {flats:?}, target {plan_target}, \
             refitted masts (anchors) {}, probes per refitted flat {PROBES_PER_FLAT}",
            analysis.planes.len(),
            check.anchors.len(),
        );
        let (candidate_design, candidate, original) = refit_inputs(name);
        let run = |method| {
            refit_flats_by(
                method,
                &candidate_design,
                &candidate.solved,
                &candidate.flats,
                &original,
                &|| false,
            )
        };
        let (by_mesh, by_clip) = (run(AreaMethod::FullMesh), run(AreaMethod::Clipped));
        let largest_difference = by_mesh
            .iter()
            .zip(&by_clip)
            .map(|(mesh, clip)| (mesh.new_mast - clip.new_mast).abs())
            .fold(0.0_f64, f64::max);
        eprintln!(
            "[retarget_refit] {name}: refit_flats refits {} flats by mesh and {} by clip, \
             largest mast difference {largest_difference:e}",
            by_mesh.len(),
            by_clip.len(),
        );
    }
    divan::main();
}

/// A candidate whose pinned flats are all lower (bigger) than the design's, that candidate's
/// analysis, and the design's own analysis: the inputs of `refit_flats_by`.
fn refit_inputs(name: &str) -> (Design, StoneAnalysis, StoneAnalysis) {
    let (design, original) = fixture(name);
    let mut candidate_design = design.clone();
    for flat in &original.flats {
        if let Some(mast) = scale_reference_mast(&design, flat.tier_index) {
            candidate_design.tiers[flat.tier_index].constraint =
                MeetConstraint::ScaleReference(0.7 * mast);
        }
    }
    let candidate = analyze(&candidate_design, false)
        .expect("the design with its flats lowered must still solve and close");
    (candidate_design, candidate, original)
}

/// `refit_flats_by` over the lowered-flats candidate.
fn refit(bencher: divan::Bencher, name: &str, method: AreaMethod) {
    let (design, candidate, original) = refit_inputs(name);
    bencher.bench_local(|| {
        black_box(refit_flats_by(
            method,
            black_box(&design),
            &candidate.solved,
            &candidate.flats,
            black_box(&original),
            &|| false,
        ))
    });
}

/// The template's design and its solved analysis (no meet baseline needed).
fn fixture(name: &str) -> (Design, StoneAnalysis) {
    let position = TEMPLATES
        .iter()
        .position(|template| template.name == name)
        .unwrap_or_else(|| panic!("template {name:?} is not in TEMPLATES"));
    let index = i32::try_from(position + 1).expect("template index fits i32");
    let design = gallery_design(index);
    let analysis = analyze(&design, false).expect("the template must solve and close");
    (design, analysis)
}

/// A target material other than the template's own, so the angles move.
fn target_selection(design: &Design) -> MaterialSelection {
    let current = design.material.name.as_deref().unwrap_or("Diamond");
    let name = if current == "Sapphire" {
        "Quartz"
    } else {
        "Sapphire"
    };
    MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    }
}

/// One whole check, crown NOT following the pavilion (so the flats change size).
fn run_check(design: &Design) -> (String, pleochroma_editor::retarget::check::RetargetCheck) {
    let selection = target_selection(design);
    let label = selection.name.clone().unwrap_or_default();
    let target = selection.resolve(&BuiltinMaterials);
    let plan = build_plan(design, &target, CrownShift::fixed(), &[]);
    let current = GemMaterial::diamond();
    let inputs = CheckInputs {
        girdle: None,
        design,
        plan: &plan,
        current_gem: Some(&current),
        lighting: LightingPreset::RingLights,
    };
    (label, check_retarget(&inputs))
}

/// Which mast one probe evaluates.
#[derive(Clone, Copy)]
enum Mast {
    /// The authored mast: where the bisection ends up for an unchanged flat.
    Authored,
    /// The bracket's low end, `1e-4 * scale`: a flat hugging the girdle plane.
    BracketLow,
    /// The bracket's high end, `3 * scale`: a flat far above the stone.
    BracketHigh,
}

/// `refit::area_at`, reproduced: the area of the facet of tier `tier_index` at mast `mast`.
fn probe_once(
    design: &Design,
    analysis: &StoneAnalysis,
    work: &mut [pleochroma::geometry::meet_solver::SolvedTier],
    tier_index: usize,
    mast: f64,
) -> f64 {
    let plane_index = analysis.ranges[tier_index].start;
    let previous = work[tier_index].mast;
    work[tier_index].mast = mast;
    let planes = design.planes_from_solved(work);
    work[tier_index].mast = previous;
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh
            .rings
            .iter()
            .filter(|(plane, ring)| *plane == plane_index && ring.len() >= 3)
            .map(|(_, ring)| polygon_area(ring))
            .sum(),
        _ => 0.0,
    }
}

fn probe(bencher: divan::Bencher, name: &str, which: Mast) {
    let (design, analysis) = fixture(name);
    let flat = analysis
        .flats
        .first()
        .copied()
        .expect("the template has a flat tier");
    let scale = analysis
        .solved
        .iter()
        .map(|tier| tier.mast.abs())
        .fold(0.0_f64, f64::max);
    let mast = match which {
        Mast::Authored => analysis.solved[flat.tier_index].mast,
        Mast::BracketLow => 1e-4 * scale,
        Mast::BracketHigh => 3.0 * scale,
    };
    let mut work = analysis.solved.clone();
    bencher.bench_local(|| {
        black_box(probe_once(
            black_box(&design),
            black_box(&analysis),
            &mut work,
            flat.tier_index,
            black_box(mast),
        ))
    });
}

fn check(bencher: divan::Bencher, name: &str) {
    let (design, _) = fixture(name);
    bencher.bench_local(|| black_box(run_check(black_box(&design))));
}

macro_rules! fixture_benches {
    ($module:ident, $name:expr) => {
        mod $module {
            use super::*;

            /// One refit probe at the authored mast.
            #[divan::bench(sample_count = 50)]
            fn probe_authored(bencher: divan::Bencher) {
                probe(bencher, $name, Mast::Authored);
            }

            /// One refit probe at the bracket's low end.
            #[divan::bench(sample_count = 50)]
            fn probe_bracket_low(bencher: divan::Bencher) {
                probe(bencher, $name, Mast::BracketLow);
            }

            /// One refit probe at the bracket's high end.
            #[divan::bench(sample_count = 50)]
            fn probe_bracket_high(bencher: divan::Bencher) {
                probe(bencher, $name, Mast::BracketHigh);
            }

            /// The whole flat refit with a mesh at every probe (the refit as it was).
            #[divan::bench(sample_count = 5, sample_size = 1)]
            fn refit_full_mesh(bencher: divan::Bencher) {
                refit(bencher, $name, AreaMethod::FullMesh);
            }

            /// The whole flat refit with the clipped probes (the refit as it is).
            #[divan::bench(sample_count = 20, sample_size = 1)]
            fn refit_clipped(bencher: divan::Bencher) {
                refit(bencher, $name, AreaMethod::Clipped);
            }

            /// The whole Retarget check (includes metrics and any refit).
            #[divan::bench(sample_count = 5, sample_size = 1)]
            fn check_fixed_crown(bencher: divan::Bencher) {
                check(bencher, $name);
            }
        }
    };
}

fixture_benches!(round, ROUND);
fixture_benches!(oval, OVAL);
fixture_benches!(cushion, CUSHION);
fixture_benches!(emerald, EMERALD);
