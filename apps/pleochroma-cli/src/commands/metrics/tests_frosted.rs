//! `metrics` measures a frosted design on the frosted stone (frosted facets and a frosted
//! girdle scatter the traced light as they do in the render); a polished design is measured on
//! exactly the polished stone, which `tests_concave` pins bit for bit.

use super::*;
use crate::{args::MaterialArg, testing};
use pleochroma::{geometry::girdle_facet_finishes, optics::raytracer::FacetFinish};
use pleochroma_cut_core::{CANONICAL_LIGHTING_PRESET, Edit, TierFrosting};

/// Measures `design` under refractive index 1.76 at the canonical light, table up, and the tilt
/// averages too when `tilt` is set.
fn measured_with(design: &Design, tilt: bool) -> Measured {
    let scoring = resolve_scoring(design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
        .expect("scoring");
    let analysis = analyze(design);
    let solved = analysis.require_stone(design).expect("a usable stone");
    measure(
        design,
        &analysis,
        solved,
        &scoring,
        CANONICAL_LIGHTING_PRESET,
        tilt,
    )
    .expect("measures")
}

/// [`measured_with`] for the table-up figures only.
fn measured_of(design: &Design) -> Measured {
    measured_with(design, false)
}

/// The five table-up figures as exact bits.
fn table_bits(metrics: &GemOpticalMetrics) -> [u32; 5] {
    [
        metrics.brilliance_pct.to_bits(),
        metrics.fire_index.to_bits(),
        metrics.scintillation_pct.to_bits(),
        metrics.windowing_pct.to_bits(),
        metrics.extinction_pct.to_bits(),
    ]
}

/// The gallery template with every tier frosted.
fn frosted_template() -> Design {
    let mut design = testing::template();
    for index in 0..design.tiers.len() {
        design
            .apply_edit(Edit::SetTierFrosting {
                index,
                frosting: Some(TierFrosting::Tier),
            })
            .expect("the tier exists");
    }
    design
}

#[test]
fn a_frosted_design_is_measured_on_the_frosted_stone() {
    let polished = measured_of(&testing::template());
    let design = frosted_template();
    let frosted = measured_of(&design);
    assert_ne!(
        table_bits(&frosted.table_up),
        table_bits(&polished.table_up),
        "frosted facets scatter the traced light"
    );
    assert_eq!(
        table_bits(&frosted.table_up),
        table_bits(&measured_of(&design).table_up),
        "a frosted measurement repeats exactly"
    );

    // The figures are the finished-stone entry point's on the design's own finishes.
    let analysis = analyze(&design);
    let solved = analysis.require_stone(&design).expect("a usable stone");
    let planes = design_to_gpu_planes_from_solved(&design, solved);
    let finishes = design.facet_finishes_from_solved(solved, &planes);
    assert!(finishes.contains(&FacetFinish::Frosted));
    let scoring = resolve_scoring(&design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
        .expect("scoring");
    let gem = scoring.sized_gem(&design, &planes);
    let environment =
        CANONICAL_LIGHTING_PRESET.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let direct = evaluate_gem_optical_metrics_finished(
        FinishedStone::new(StoneGeometry::planes_only(&planes), &finishes),
        &gem,
        0.0,
        TABLE_UP_PITCH,
        environment,
    );
    assert_eq!(table_bits(&frosted.table_up), table_bits(&direct));
}

/// The three tilt averages as exact bits.
fn tilt_bits(tilt: &ObjectiveComponents) -> [u32; 3] {
    [
        tilt.tilt_brilliance_pct.to_bits(),
        tilt.windowing_pct.to_bits(),
        tilt.extinction_pct.to_bits(),
    ]
}

#[test]
fn a_frosted_girdle_moves_the_figures() {
    let mut design = testing::template();
    design.render_effects.girdle_frosted = true;

    // The finishes `metrics` measures on are exactly the girdle band: frosted girdle facets,
    // every other facet polished.
    let analysis = analyze(&design);
    let solved = analysis.require_stone(&design).expect("a usable stone");
    let planes = design_to_gpu_planes_from_solved(&design, solved);
    let finishes = design.facet_finishes_from_solved(solved, &planes);
    assert_eq!(finishes.len(), planes.len(), "one finish per plane");
    assert!(
        finishes.contains(&FacetFinish::Frosted),
        "the girdle frosts"
    );
    assert!(
        finishes.contains(&FacetFinish::Polished),
        "nothing else does"
    );
    assert_eq!(
        finishes,
        girdle_facet_finishes(&planes),
        "the girdle toggle frosts the girdle band and nothing more"
    );

    // The girdle is a thin band at the rim. The table-up rays of a well-cut stone go down the
    // pavilion mains and leave through the crown without reaching it, so that one pose can
    // read exactly as the polished stone's; the tilt sweep lights the girdle from the side,
    // and its averages are the figures a frosted girdle moves.
    let polished = measured_with(&testing::template(), true);
    let girdle = measured_with(&design, true);
    let (polished_tilt, girdle_tilt) = (
        polished.tilt.expect("the polished tilt is measured"),
        girdle.tilt.expect("the girdle tilt is measured"),
    );
    assert_ne!(
        tilt_bits(&girdle_tilt),
        tilt_bits(&polished_tilt),
        "a frosted girdle scatters light too"
    );
}
