//! The sweep scores a frosted design on the frosted stone, and a polished one exactly as it
//! always did.

use super::*;
use pleochroma::{
    color::metrics::evaluate_gem_optical_metrics,
    optics::{
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
};
use pleochroma_cut_core::{ConstraintTier, Edit, PreformSpec, ScheduleMeta, TierFrosting};

/// The Pavilion Main of the standard round brilliant.
const PAVILION_MAIN: usize = 5;

fn rbc() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    design
}

fn frosted_rbc() -> Design {
    let mut design = rbc();
    design
        .apply_edit(Edit::SetTierFrosting {
            index: PAVILION_MAIN,
            frosting: Some(TierFrosting::Tier),
        })
        .expect("the tier exists");
    design
}

fn scene(material: &GemMaterial) -> SweepScene<'_> {
    SweepScene {
        material,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    }
}

fn fast_options() -> SweepOptions {
    SweepOptions {
        tilt_average: false,
        workers: 1,
    }
}

fn score(design: &Design, material: &GemMaterial) -> RowMetrics {
    score_design(
        design,
        None,
        &scene(material),
        &fast_options(),
        &AtomicBool::new(false),
    )
    .expect("not cancelled")
    .expect("the template scores")
}

#[test]
fn a_frosted_design_scores_differently_from_the_polished_one_and_repeatably() {
    let material = GemMaterial::diamond();
    let polished = score(&rbc(), &material);
    let frosted = score(&frosted_rbc(), &material);
    assert_ne!(
        (
            polished.brilliance_pct.to_bits(),
            polished.windowing_pct.to_bits(),
            polished.extinction_pct.to_bits()
        ),
        (
            frosted.brilliance_pct.to_bits(),
            frosted.windowing_pct.to_bits(),
            frosted.extinction_pct.to_bits()
        ),
        "frosted facets scatter the traced light"
    );
    assert_eq!(frosted, score(&frosted_rbc(), &material));
}

#[test]
fn the_score_equals_the_direct_finished_stone_call() {
    let material = GemMaterial::diamond();
    let design = frosted_rbc();
    let scored = score(&design, &material);

    let solved = design.solve().expect("the template solves");
    let planes = design.planes_from_solved(&solved);
    let gpu = design_to_gpu_planes_from_solved(&design, &solved);
    let tools = design.tools_for_scoring(&planes);
    let finishes = design.facet_finishes_from_solved(&solved, &gpu);
    assert!(finishes.contains(&FacetFinish::Frosted));
    let direct = evaluate_gem_optical_metrics_finished(
        FinishedStone::new(
            StoneGeometry {
                planes: &gpu,
                tools: &tools,
            },
            &finishes,
        ),
        &material,
        0.0,
        TABLE_UP_PITCH_RAD,
        scene(&material).environment,
    );
    assert_eq!(
        scored.brilliance_pct.to_bits(),
        direct.brilliance_pct.to_bits()
    );
    assert_eq!(
        scored.windowing_pct.to_bits(),
        direct.windowing_pct.to_bits()
    );
    assert_eq!(
        scored.extinction_pct.to_bits(),
        direct.extinction_pct.to_bits()
    );
    assert_eq!(scored.fire_index.to_bits(), direct.fire_index.to_bits());
    assert_eq!(
        scored.scintillation_pct.to_bits(),
        direct.scintillation_pct.to_bits()
    );
}

#[test]
fn a_polished_design_scores_the_plain_call_bit_for_bit() {
    let material = GemMaterial::diamond();
    let design = rbc();
    let scored = score(&design, &material);
    let solved = design.solve().expect("the template solves");
    let gpu = design_to_gpu_planes_from_solved(&design, &solved);
    let direct = evaluate_gem_optical_metrics(
        &gpu,
        &material,
        0.0,
        TABLE_UP_PITCH_RAD,
        scene(&material).environment,
    );
    assert_eq!(
        scored.brilliance_pct.to_bits(),
        direct.brilliance_pct.to_bits()
    );
    assert_eq!(scored.fire_index.to_bits(), direct.fire_index.to_bits());
    assert_eq!(
        scored.scintillation_pct.to_bits(),
        direct.scintillation_pct.to_bits()
    );
}

/// The tilt averages of `design` as the sweep scores them, as exact bits.
fn tilt_bits(design: &Design, material: &GemMaterial) -> [u32; 3] {
    let options = SweepOptions {
        tilt_average: true,
        workers: 1,
    };
    let scored = score_design(
        design,
        None,
        &scene(material),
        &options,
        &AtomicBool::new(false),
    )
    .expect("not cancelled")
    .expect("the template scores");
    let tilt = scored.tilt.expect("the sweep asked for the tilt average");
    [
        tilt.brilliance_pct.to_bits(),
        tilt.windowing_pct.to_bits(),
        tilt.extinction_pct.to_bits(),
    ]
}

/// A frosted girdle is a thin rim band. The single table-up pose lights the pavilion mains and
/// leaves through the crown, so no table-up ray reaches the band and those five figures may
/// legitimately match the polished stone's. The tilt average does light the band from the side,
/// so that is where the flag has to show; and the finishes the sweep scores on must carry it.
#[test]
fn the_frosted_girdle_flag_moves_the_score_too() {
    let material = GemMaterial::diamond();
    let polished = rbc();
    let mut girdle = rbc();
    girdle.render_effects.girdle_frosted = true;

    // The finishes the sweep scores on hold the girdle band as frosted, the rest polished; a
    // polished design has none (the documented all-polished form).
    let solved = girdle.solve().expect("the template solves");
    let gpu = design_to_gpu_planes_from_solved(&girdle, &solved);
    let finishes = girdle.facet_finishes_from_solved(&solved, &gpu);
    assert_eq!(finishes.len(), gpu.len(), "one finish per plane");
    assert!(
        finishes.contains(&FacetFinish::Frosted),
        "the girdle band frosts"
    );
    assert!(
        finishes.contains(&FacetFinish::Polished),
        "only the girdle band frosts"
    );
    let polished_solved = polished.solve().expect("the template solves");
    let polished_gpu = design_to_gpu_planes_from_solved(&polished, &polished_solved);
    assert!(
        !polished
            .facet_finishes_from_solved(&polished_solved, &polished_gpu)
            .contains(&FacetFinish::Frosted)
    );

    assert_ne!(
        tilt_bits(&polished, &material),
        tilt_bits(&girdle, &material),
        "a frosted girdle scatters the side-lit tilt average"
    );
}
