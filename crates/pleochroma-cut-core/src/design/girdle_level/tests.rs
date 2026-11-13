//! Tests for [`Design::girdle_level`], with reference heights from an independent calculation of
//! its line-corner rule on the same planes, for four stones (all fixtures this file controls):
//!
//! - Pentalume (`designs/Pentalume_Sapphire`, its tiers named by their codes): P3 raises the
//!   bottom line and C3 lowers the top line where G1 meets G2, so the girdle is not level;
//! - Cam's Portuguese (PC 45.140, by Greg Ninedorf, assigned to the public domain): a meet-point
//!   design whose girdle is level all round, set off-centre above `y = 0`;
//! - Iris (`designs/Iris_Sapphire`): its girdle-face corners and level edges stand at several
//!   heights, so it is not level;
//! - a hand-built brilliant (the masts of the Standard Round Brilliant template, written out
//!   here so a template change does not move this test): its break facets meet each girdle
//!   face in a V, the natural scallop, and its girdle is level at the line corners.

use super::{GIRDLE_LEVEL_TOLERANCE_FRACTION, runs_level};
use crate::{
    design::{ConstraintTier, Design, ScheduleMeta},
    preform::PreformSpec,
};
use glam::DVec3;
use pleochroma::geometry::meet_solver::MeetConstraint;

/// Pentalume, the published design, with each tier named by its code (`P1`, ..., `T`).
const PENTALUME: &str = include_str!("pentalume.asc");

/// Cam's Portuguese, verbatim.
const CAMS_PORTUGUESE: &str = include_str!("cams_portuguese.asc");

/// Iris, the published design, verbatim.
const IRIS: &str = include_str!("iris.asc");

/// The planes go through `f32`, so the corners stand a few `1e-7` from where exact planes put
/// them; this is far below the tolerance being judged (0.005 of these units).
const CLOSE: f64 = 2e-5;

/// `text` as a design, every mast pinned as the file records it, in a rough that does not touch
/// the stone.
fn design_of(text: &str) -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(text).expect("the fixture parses");
    Design::from_asc_schedule(PreformSpec::block(3.0, 1.0, 4.0), &schedule)
}

/// A tier at `angle_deg` pinned at `mast`.
fn pinned(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A 96-tooth schedule with `tiers`, in a rough that does not touch the stone.
fn on_96_teeth(tiers: Vec<ConstraintTier>) -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        tiers,
    )
}

/// A brilliant cut to a level girdle with the natural scallop: 16 girdle faces, the upper and
/// lower girdle facets in pairs either side of them (so no facet stands at a face's azimuth
/// except the mains, which touch the girdle in points), and the crown and pavilion mains.
fn scalloped_brilliant() -> Design {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAKS: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAINS: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STARS: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
    on_96_teeth(vec![
        pinned("Table", 0.0, &[], 0.32),
        pinned("Star", 15.0, &STARS, 0.45),
        pinned("Crown Main", 34.5, &MAINS, 0.59),
        pinned("Upper Girdle", 41.0, &BREAKS, 0.67),
        pinned("Girdle", 90.0, &GIRDLE, 1.0),
        pinned("Pavilion Main", -41.0, &MAINS, 0.67),
        pinned("Lower Girdle", -42.5, &BREAKS, 0.68),
        pinned("Culet", -0.0, &[], 0.88),
    ])
}

/// The position of the tier called `name`.
fn position(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier is called {name}"))
}

/// Asserts `actual` is within [`CLOSE`] of `expected`.
fn assert_close(what: &str, actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < CLOSE,
        "{what}: {actual}, expected {expected}"
    );
}

#[test]
fn pentalumes_girdle_is_cut_into_by_p3_and_c3() {
    let design = design_of(PENTALUME);
    let solved = design.solve().expect("every tier is pinned");
    let level = design
        .girdle_level(&solved)
        .expect("the stone closes and has a girdle");

    // Reference: C1 and C2 run along G1 and G2 at +0.0425 and P1 and P2 at -0.0425 (level
    // edges); where G1 meets G2, C3 lowers the top line to +0.0295 and P3 raises the bottom line
    // to -0.0245.
    assert_close("lowest top line corner", level.top.0, 0.0295);
    assert_close("highest top line corner", level.top.1, 0.0425);
    assert_close("lowest bottom line corner", level.bottom.0, -0.0425);
    assert_close("highest bottom line corner", level.bottom.1, -0.0245);
    // The width the proportions use: the smaller axis-aligned extent, here along `x`.
    assert_close("width", level.width, 2.13);
    let (thinnest, thickest) = level.thickness_range();
    assert_close("thinnest", thinnest, 0.054);
    assert_close("thickest", thickest, 0.085);
    assert_close(
        "tolerance",
        level.tolerance(),
        GIRDLE_LEVEL_TOLERANCE_FRACTION * 2.13,
    );

    // P3 is cut before C3.
    assert_eq!(
        level.cut_in,
        vec![position(&design, "P3"), position(&design, "C3")]
    );
    assert!(!level.is_level());

    // At 10 mm across that width the band runs 0.25 to 0.40 mm thick. (At the narrower caliper
    // width of 2.1029 it reads 0.257 to 0.404 mm.)
    let mm = 10.0 / level.width;
    let (thinnest_mm, thickest_mm) = (thinnest * mm, thickest * mm);
    assert!((thinnest_mm - 0.2535).abs() < 1e-3, "{thinnest_mm}");
    assert!((thickest_mm - 0.3991).abs() < 1e-3, "{thickest_mm}");
}

#[test]
fn a_meet_point_designs_girdle_is_level_even_off_centre() {
    let design = design_of(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every tier is pinned");
    let level = design
        .girdle_level(&solved)
        .expect("the stone closes and has a girdle");
    // Every crown facet that reaches the girdle (tier `a`) meets it at one height, and every
    // pavilion facet (tier `2`) at another: the girdle sits above `y = 0`, level all round. The
    // file's five-decimal masts leave the corners up to 3e-5 apart (0.0014 % of the width, far
    // inside the tolerance), so each line has a lowest and a highest corner of its own.
    assert_close("lowest top line corner", level.top.0, 0.376_740_6);
    assert_close("highest top line corner", level.top.1, 0.376_770_5);
    assert_close("lowest bottom line corner", level.bottom.0, 0.313_929_6);
    assert_close("highest bottom line corner", level.bottom.1, 0.313_945_8);
    // The width is the computed one (2.146 525 3), not the four-decimal 2.1465: the rounding
    // would put it 2.5e-5 away, past CLOSE.
    assert_close("width", level.width, 2.146_525_3);
    let (thinnest, thickest) = level.thickness_range();
    assert_close("thinnest", thinnest, 0.062_794_8);
    assert_close("thickest", thickest, 0.062_840_9);
    assert!(level.cut_in.is_empty(), "{:?}", level.cut_in);
    assert!(level.is_level());
}

#[test]
fn a_brilliants_v_scallop_is_not_a_cut_into_its_girdle() {
    // Between its corners each girdle face carries a V of two break facets whose apex stands off
    // the girdle line (up to +0.0646 on the crown side and -0.0546 on the pavilion side, against
    // the line's +0.0090 and +0.0040). Only the corners where two girdle faces meet are line
    // corners here, and they all stand at one height above and one below.
    let design = scalloped_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let level = design
        .girdle_level(&solved)
        .expect("the stone closes and has a girdle");
    assert_close("width", level.width, 2.0);
    assert_close("lowest top line corner", level.top.0, 0.009_024_2);
    assert_close("highest top line corner", level.top.1, 0.009_024_2);
    assert_close("lowest bottom line corner", level.bottom.0, 0.003_977_9);
    assert_close("highest bottom line corner", level.bottom.1, 0.003_977_9);
    let (thinnest, thickest) = level.thickness_range();
    assert_close("thinnest", thinnest, 0.005_046_3);
    assert_close("thickest", thickest, 0.005_046_3);
    assert!(level.cut_in.is_empty(), "{:?}", level.cut_in);
    assert!(level.is_level());
}

#[test]
fn iris_girdle_lines_stand_at_several_heights() {
    // The corners where two of its 24 girdle faces meet run from +0.0310 to +0.0407 above
    // (0.155 to 0.203 mm at 10 mm) and from -0.0460 to -0.0330 below; the culet mains run along
    // their girdle faces at -0.0735, the lowest bottom line.
    let design = design_of(IRIS);
    let solved = design.solve().expect("every tier is pinned");
    let level = design
        .girdle_level(&solved)
        .expect("the stone closes and has a girdle");
    assert_close("width", level.width, 2.0);
    assert_close("lowest top line corner", level.top.0, 0.031_024_3);
    assert_close("highest top line corner", level.top.1, 0.040_681_6);
    assert_close("lowest bottom line corner", level.bottom.0, -0.073_498_3);
    assert_close("highest bottom line corner", level.bottom.1, -0.033_004_8);
    let (thinnest, thickest) = level.thickness_range();
    assert_close("thinnest", thinnest, 0.064_029_1);
    assert_close("thickest", thickest, 0.114_179_9);
    assert!(!level.is_level());
    let expected: Vec<usize> = [
        "Trailing_shoulders",
        "Leading_shoulders",
        "Outer_mains",
        "Leading_corner_facets",
        "Trailing_corner_facets",
        "Leading_girdle_breaks",
        "Trailing_girdle_breaks",
        "Leading_upper_shoulders",
        "Trailing_upper_shoulders",
        "Leading_corner_uppers",
        "Trailing_corner_uppers",
    ]
    .iter()
    .map(|name| position(&design, name))
    .collect();
    assert_eq!(level.cut_in, expected);
}

#[test]
fn a_facet_runs_level_along_a_girdle_face_only_at_its_azimuth() {
    let wall = (DVec3::X, 1.0);
    let facet_at = |angle_deg: f64, azimuth_deg: f64| {
        let (sin_t, cos_t) = angle_deg.to_radians().sin_cos();
        let (sin_p, cos_p) = azimuth_deg.to_radians().sin_cos();
        (DVec3::new(sin_t * cos_p, cos_t, sin_t * sin_p), 0.6)
    };
    // At the face's own azimuth the shared line is horizontal, on either side of the girdle.
    assert!(runs_level(facet_at(34.0, 0.0), wall));
    assert!(runs_level(facet_at(180.0 - 41.0, 0.0), wall));
    // One tooth of a 96-tooth wheel off, a 41-degree break rises 5.7 % of its run.
    assert!(!runs_level(facet_at(41.0, 3.75), wall));
    // A table meeting the face runs level; a facet parallel to the face has no line to judge.
    assert!(runs_level((DVec3::Y, 0.3), wall));
    assert!(!runs_level((DVec3::X, 0.9), wall));
}

#[test]
fn there_is_no_level_without_a_girdle_an_aligned_solve_or_an_edge() {
    // No tiers at all: no girdle tier.
    let empty = Design::fresh(PreformSpec::block(2.0, 1.0, 4.0), 96, 4, 1.54);
    assert_eq!(empty.girdle_level(&[]), None);

    // A mast list that is not the design's.
    let design = design_of(PENTALUME);
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(design.girdle_level(&solved[..solved.len() - 1]), None);

    // A girdle alone in a block: its faces end on the rough, not on crown or pavilion facets,
    // so there is no girdle line to judge.
    let lone = on_96_teeth(vec![pinned("G1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0)]);
    let solved = lone.solve().expect("the girdle is pinned");
    assert_eq!(lone.girdle_level(&solved), None);
}
