//! Tests for [`crate::design::TierTarget`] resolution.
//!
//! `GirdleThicknessMm`, `TableWidthMm` and `DepthMm` each resolve to their requested
//! figure on a real design. An incremental resolve re-derives a target exactly as a full
//! solve does. The algorithm is described in `crate::design::targets`.

use crate::{
    design::{ConstraintTier, Design, ScheduleMeta, TierTarget},
    preform::PreformSpec,
};
use pleochroma::geometry::{
    meet_solver::MeetConstraint,
    stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
};

/// Pentalume (the girdle-level fixture, tiers named by their codes): its girdle line is not
/// level, thinnest where G1 meets G2 (P3 and C3 cut into it there).
const PENTALUME: &str = include_str!("../girdle_level/pentalume.asc");

/// Pentalume's G1, the tier that sets its stone size.
const PENTALUME_G1: usize = 3;

/// The girdle-thickness target's own tolerance: `1e-4` of the girdle diameter.
fn target_tolerance_mm(design: &Design) -> f64 {
    1e-4 * design
        .girdle_diameter_mm
        .expect("the fixture sets a girdle diameter")
}

/// The girdle of `design`, solved (its targets resolved), in millimetres: the thinnest and the
/// thickest thickness at its girdle line, and the band's full height, each through the solid's
/// own `width_axis`. `None` for the line when the design has none.
fn girdle_mm(design: &Design) -> (Option<(f64, f64)>, f64) {
    let solved = design
        .solve()
        .expect("design with a resolved target must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("must measure as a solid");
    let scale = crate::yield_metrics::mm_per_unit(
        design
            .girdle_diameter_mm
            .expect("fixture sets a girdle diameter"),
        metrics.width_axis,
    )
    .expect("must have a real width_axis to scale from");
    let line = design.girdle_level(&solved).map(|level| {
        let (thinnest, thickest) = level.thickness_range();
        (thinnest.max(0.0) * scale, thickest * scale)
    });
    let band = metrics
        .girdle_thickness
        .expect("the design has a girdle band")
        * scale;
    (line, band)
}

/// Tier indices in [`ConstraintTier::standard_round_brilliant`]'s own table.
const TABLE: usize = 0;
const GIRDLE: usize = 4;
const PAVILION_MAIN: usize = 5;

/// [`ConstraintTier::standard_round_brilliant`] over a block preform, with a
/// real 6.5 mm girdle diameter set -- the fixture every test below targets.
fn standard_round_brilliant_6_5mm_girdle() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.girdle_diameter_mm = Some(6.5);
    design
}

/// Solves `design` (through [`Design::solve`], which resolves any
/// [`TierTarget`] first) and returns `extract`'s measured mm figure for the
/// finished solid -- the same measurement `crate::design::targets::bisect_tier_mast`
/// itself checks against, built here from public API only.
fn measured_mm(design: &Design, extract: impl Fn(&StoneProportions, f64) -> Option<f64>) -> f64 {
    let solved = design
        .solve()
        .expect("design with a resolved target must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("must measure as a solid");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("must close into a real solid");
    };
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let scale = crate::yield_metrics::mm_per_unit(
        design
            .girdle_diameter_mm
            .expect("fixture sets a girdle diameter"),
        metrics.width_axis,
    )
    .expect("must have a real width_axis to scale from");
    extract(&proportions, metrics.width_axis)
        .expect("the requested figure must be measurable on this solid")
        * scale
}

/// `GirdleThicknessMm` aims at the girdle line, not the band: the round brilliant's
/// girdle faces carry the break facets' V-scallops, which the band's full height
/// includes and the line does not.
#[test]
fn girdle_thickness_mm_target_reaches_the_girdle_line_thickness() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(GIRDLE).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::GirdleThicknessMm(0.3));

    let (line, band) = girdle_mm(&design);
    let (thinnest, _) = line.expect("the brilliant has a girdle line");
    assert!(
        (thinnest - 0.3).abs() <= target_tolerance_mm(&design),
        "the girdle line is {thinnest:.5} mm thick, not the 0.3 mm target"
    );
    // The band, V-scallops and all, is much thicker: it is not what was targeted.
    assert!(band > 0.4, "band {band:.4} mm");
}

/// An unlevel girdle is targeted at its thinnest line point: where G1 meets G2 on
/// Pentalume, not its thicker line points or its band.
#[test]
fn an_unlevel_girdles_target_is_its_thinnest_line_point() {
    let schedule = pleochroma_formats::asc::parse_asc(PENTALUME).expect("the fixture parses");
    let mut design = Design::from_asc_schedule(PreformSpec::block(3.0, 1.0, 4.0), &schedule);
    design.girdle_diameter_mm = Some(10.0);
    let id = design.tier_id_at(PENTALUME_G1).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::GirdleThicknessMm(0.3));

    let (line, band) = girdle_mm(&design);
    let (thinnest, thickest) = line.expect("Pentalume has a girdle line");
    assert!(
        (thinnest - 0.3).abs() <= target_tolerance_mm(&design),
        "the thinnest line point is {thinnest:.5} mm, not the 0.3 mm target"
    );
    // The line stays unlevel, and the band stays thicker than the line.
    assert!(thickest > 0.4, "thickest line point {thickest:.4} mm");
    assert!(band > 0.45, "band {band:.4} mm");
}

/// Without a girdle line the target sets the band's full height. The one girdle face
/// shares no corner with another girdle face, and no facet meets it along a level edge.
#[test]
fn without_a_girdle_line_the_target_is_the_band() {
    const MAINS: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    let pinned = |name: &str, angle_deg: f64, indices: &[f64], mast: f64| ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![
            pinned("Table", 0.0, &[], 0.32),
            pinned("Crown Main", 34.5, &MAINS, 0.59),
            pinned("Girdle", 90.0, &[6.0], 1.0),
            pinned("Pavilion Main", -41.0, &MAINS, 0.67),
        ],
    );
    design.girdle_diameter_mm = Some(6.5);
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(design.girdle_level(&solved), None, "no girdle line");

    let id = design.tier_id_at(2).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::GirdleThicknessMm(0.6));
    let (line, band) = girdle_mm(&design);
    assert_eq!(line, None, "still no girdle line");
    assert!(
        (band - 0.6).abs() <= target_tolerance_mm(&design),
        "the band is {band:.5} mm, not the 0.6 mm target"
    );
}

/// `TableWidthMm` resolves to within 1% of the requested width.
#[test]
fn table_width_mm_target_resolves_within_one_percent() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(TABLE).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::TableWidthMm(3.5));

    let measured = measured_mm(&design, |p, width_axis| {
        p.table_percent.map(|pct| pct / 100.0 * width_axis)
    });
    assert!(
        (measured - 3.5).abs() / 3.5 <= 0.01,
        "measured table width {measured:.4} mm not within 1% of the 3.5 mm target"
    );
}

/// `DepthMm` is a fixed point: the target tier's resolved mast, converted back to mm
/// with the same solid's own `width_axis`, reproduces the requested depth within 1%.
#[test]
fn depth_mm_target_resolves_within_one_percent() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(PAVILION_MAIN).expect("tier must exist");
    design.tier_targets.insert(id, TierTarget::DepthMm(2.8));

    let solved = design
        .solve()
        .expect("design with a resolved target must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("must measure as a solid");
    let scale = crate::yield_metrics::mm_per_unit(
        design
            .girdle_diameter_mm
            .expect("fixture sets a girdle diameter"),
        metrics.width_axis,
    )
    .expect("must have a real width_axis to scale from");
    let measured = solved[PAVILION_MAIN].mast * scale;
    assert!(
        (measured - 2.8).abs() / 2.8 <= 0.01,
        "Pavilion Main's resolved mast measures {measured:.4} mm, not within 1% of the 2.8 mm \
         depth target -- the fixed-point iteration must converge, not stop after one \
         mast-0 bootstrap pass"
    );
}

/// `resolve_dirty` re-derives a `TierTarget`'s mast when something its conversion depends
/// on changes. Here that is the girdle's mast, which sets `width_axis` and so the
/// depth-to-mast scale. A target tier looks like an ordinary `ScaleReference` anchor to
/// `affected_tiers`, which would otherwise keep its pre-edit mast.
#[test]
fn resolve_dirty_reresolves_a_depth_mm_target_when_the_girdle_changes() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(PAVILION_MAIN).expect("tier must exist");
    design.tier_targets.insert(id, TierTarget::DepthMm(2.8));

    let baseline = design.solve().expect("must solve");

    let mut edited = design.clone();
    let MeetConstraint::ScaleReference(mast) = edited.tiers[GIRDLE].constraint else {
        panic!("Girdle must be a ScaleReference");
    };
    edited.tiers[GIRDLE].constraint = MeetConstraint::ScaleReference(mast * 1.2);

    let dirty = std::collections::BTreeSet::from([GIRDLE]);
    let incremental = edited
        .resolve_dirty(&baseline, &dirty)
        .expect("must resolve");
    let full = edited.solve().expect("must solve");

    assert_eq!(
        incremental[PAVILION_MAIN].mast.to_bits(),
        full[PAVILION_MAIN].mast.to_bits(),
        "an incremental resolve must re-derive the DepthMm target exactly like a full solve, \
         not keep serving the pre-edit mast"
    );
}
