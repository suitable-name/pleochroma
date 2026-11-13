//! Tests for [`super`]: the cut-so-far meets of two real designs and the two teaching templates,
//! the cutting sheet's text for them, and the snap.
//!
//! The expected figures come from an independent reference check and a port of this module's
//! rules (the same `f32` facet planes, preform and mesh tolerances), which agree with each
//! other to well under 1 %.

use crate::{
    cutting_sheet::CuttingSheet,
    design::{CutMeet, CutMeetAnchor, DEFAULT_SNAP_MAX_MOVE_FRACTION, Design},
    native::{DesignExtras, design_from_str, design_to_string},
    preform::PreformSpec,
    templates::TEMPLATES,
};
use pleochroma::geometry::meet_solver::MeetConstraint;

/// PC 45.140, Cam's Portuguese, by Greg Ninedorf (2014), assigned to the public domain by its
/// own header: a true meet-point design, cut girdle first. Inlined verbatim (the crate has no
/// fixtures folder).
pub(super) const CAMS_PORTUGUESE: &str = "\
GemCad 5.0_2017
g 96 0.0
y 12 y
I 2.16
H PC 45.140  Cam's Portuguese
H by Greg Ninedorf, 2014
H Copyright assigned to the public domain.
a -90.00000 1.07325 96 n 1 88 80 72 64 56 48 40 32 24 16 8 G Set the size and shape
a -61.71792 0.79638 96 n 2 88 80 72 64 56 48 40 32 24 16 8 G Set the girdle line
a -51.97569 0.68189 92 n 3 84 76 68 60 52 44 36 28 20 12 4 G Meet the girdle line
a -47.01841 0.65290 96 n 4 88 80 72 64 56 48 40 32 24 16 8 G Meet 2
a -42.00000 0.64937 92 n 5 84 76 68 60 52 44 36 28 20 12 4 G Meet 3
a 32.00000 0.88823 96 n a 8 16 24 32 40 48 56 64 72 80 88 G Set the girdle width
a 28.00000 0.85428 4 n b 12 20 28 36 44 52 60 68 76 84 92 G Meet the girdle
a 23.00000 0.83175 96 n c 8 16 24 32 40 48 56 64 72 80 88 G Meet a
a 0.00000 0.68812 96 n t G Meet b
F This design works for R.I. 1.76 - 2.16
";

/// The Pentalume Sapphire design (`designs/Pentalume_Sapphire/Pentalume_Sapphire.asc`), whose
/// depths an optimizer set freely, so a cutter finds some of its tiers float. Inlined verbatim,
/// line endings aside.
const PENTALUME: &str = "\
GemCad 5.0
g 80 0
y 5 y
I 1.7680773735046387
H Pentalume Sapphire
H Designed by Niklas Gussek
a -43 0.7130808923813132 0 n P1_Five_broad_mains 16 32 48 64
a -40.5 0.7661935481511087 8 n P2_Five_corner_guards 24 40 56 72
a -44 0.7955073176906 76 n P3_Ten_shoulder_splits 4 12 20 28 36 44 52 60 68
a 90 1 0 n G1_Five_long_sides 16 32 48 64 G Set stone size.
a 90 1.13 8 n G2_Five_clipping_flats 24 40 56 72
a 34 0.5944270003043362 0 n C1_Five_broad_bezels 16 32 48 64
a 30 0.6018060796608384 8 n C2_Five_corner_bezels 24 40 56 72
a 33 0.6346315320439263 76 n C3_Ten_shoulder_facets 4 12 20 28 36 44 52 60 68
a 0 0.35 n Table
";

/// Pentalume's tiers, stored in the order they are cut.
const P1: usize = 0;
const P2: usize = 1;
const P3: usize = 2;
const G1: usize = 3;
const G2: usize = 4;
const C1: usize = 5;
const C2: usize = 6;
const C3: usize = 7;
const TABLE: usize = 8;

/// Pentalume's girdle width in design units: the 10 mm the reference figures are given at.
const PENTALUME_WIDTH: f64 = 2.1029;

/// `mm` on Pentalume at a 10 mm width, in design units.
const fn units(mm: f64) -> f64 {
    mm / 10.0 * PENTALUME_WIDTH
}

/// A design read from `.asc` text the way the app opens a bare `.asc` file
/// (`pleochroma_editor::loading::design_from_asc_text`): `parse_asc`, the default preform the
/// editor builds for it (`default_preform_for_schedule`: a round cylinder with one side per
/// gear tooth, half-width 3 and depth 3), and [`Design::from_asc_schedule`], which pins every
/// tier to its recorded mast as an explicit depth (`MeetConstraint::ScaleReference`) and keeps
/// the recorded notes.
pub(super) fn design_from_asc(text: &str) -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(text).expect("the fixture parses");
    let preform = PreformSpec::cylinder_for_schedule(&schedule, 3.0, 1.0, 3.0);
    Design::from_asc_schedule(preform, &schedule)
}

/// The built-in template called `name` on its own rough, as the New Design dialog creates it
/// (its masts as the template stores them).
fn template_design(name: &str) -> Design {
    let spec = TEMPLATES
        .iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("no template {name:?}"));
    Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers())
}

/// Each row's code and the instruction the sheet prints for it, in cutting order.
fn instructions(sheet: &CuttingSheet) -> Vec<(&str, &str)> {
    sheet
        .rows
        .iter()
        .map(|row| (row.code.as_str(), row.meet_instruction.as_str()))
        .collect()
}

/// The instruction the sheet prints for the row coded `code`, and that row's depth in
/// millimetres.
fn row_text(sheet: &CuttingSheet, code: &str) -> (String, Option<f64>) {
    let row = sheet
        .rows
        .iter()
        .find(|row| row.code == code)
        .unwrap_or_else(|| panic!("no row {code}"));
    (row.meet_instruction.clone(), row.depth_mm)
}

/// Cam's Portuguese is a true meet-point design: on the stone cut so far every tier but its
/// three anchors meets the points the tiers before it left, as its recorded notes say. Tier 1
/// (the girdle) sets the stone size; tier 2 sets the girdle line, since the girdle's vertical
/// facets form no point yet; tier a sets the girdle width. The twelve mains of tier 5 end in the
/// culet point, but they meet tiers 3 and 4 ("Meet 3"), so they are not "cut to centerpoint".
/// The sheet still prints the recorded notes verbatim.
#[test]
fn cams_portuguese_meets_on_the_stone_cut_so_far() {
    let design = design_from_asc(CAMS_PORTUGUESE);
    let solved = design.solve().expect("every imported tier is pinned");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    assert_eq!(
        meets.tiers,
        [
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleLine),
            CutMeet::Meets(vec![0, 1]),
            CutMeet::Meets(vec![1, 2]),
            CutMeet::Meets(vec![2, 3]),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
            CutMeet::Meets(vec![0, 5]),
            CutMeet::Meets(vec![5, 6]),
            CutMeet::Meets(vec![6, 7]),
        ]
    );
    assert_eq!(meets.floating(&design), [] as [(usize, f64); 0]);
    // The largest extent is the width across the twelve girdle flats.
    assert!((meets.size - 2.1465).abs() < 1e-3, "{}", meets.size);

    let sheet = design.cutting_sheet(&solved);
    let printed: Vec<&str> = sheet
        .rows
        .iter()
        .map(|row| row.meet_instruction.as_str())
        .collect();
    assert_eq!(
        printed,
        [
            "Set the size and shape",
            "Set the girdle line",
            "Meet the girdle line",
            "Meet 2",
            "Meet 3",
            "Set the girdle width",
            "Meet the girdle",
            "Meet a",
            "Meet b",
        ]
    );
}

/// Without its notes, Cam's Portuguese gets them back from the stone cut so far: the sheet's
/// computed instructions say what the designer wrote, by code (tier 1 is G1, tiers 2 to 5 are
/// P1 to P4, tiers a to c are C1 to C3, t is T).
#[test]
fn cams_portuguese_without_notes_prints_its_meets_by_code() {
    let mut design = design_from_asc(CAMS_PORTUGUESE);
    for tier in &mut design.tiers {
        tier.original_notes = None;
    }
    let solved = design.solve().expect("every imported tier is pinned");
    let sheet = design.cutting_sheet(&solved);
    let printed: Vec<(&str, &str)> = sheet
        .rows
        .iter()
        .map(|row| (row.code.as_str(), row.meet_instruction.as_str()))
        .collect();
    assert_eq!(
        printed,
        [
            ("G1", "Set stone size."),
            ("P1", "Set girdle line."),
            ("P2", "Meet G1, P1"),
            ("P3", "Meet P1, P2"),
            ("P4", "Meet P2, P3"),
            ("C1", "Set girdle width."),
            ("C2", "Meet G1, C1"),
            ("C3", "Meet C1, C2"),
            ("T", "Meet C2, C3"),
        ]
    );
}

/// The Simple Teaching Design (pavilion mains, girdle, table) is a meet-point design: the mains
/// end in one point, the girdle sets the stone size, and with no other flat crown tier the
/// table, which reaches the girdle, sets the girdle width.
#[test]
fn the_simple_teaching_design_sets_the_girdle_width_with_its_table() {
    let design = template_design("Simple Teaching Design");
    let solved = design.solve().expect("every template tier is pinned");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    assert_eq!(
        meets.tiers,
        [
            CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint),
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
        ]
    );
    let sheet = design.cutting_sheet(&solved);
    assert_eq!(
        instructions(&sheet),
        [
            ("P1", "Cut to centerpoint."),
            ("G1", "Set stone size."),
            ("T", "Set girdle width."),
        ]
    );
}

/// The Rich Teaching Design (pavilion mains, girdle, crown mains, table) is a meet-point design
/// too: the crown mains set the girdle width, and they meet only in their apex, on the axis, and
/// on the girdle, so the stone has no point a table could meet and the table sets the table
/// size.
#[test]
fn the_rich_teaching_design_sets_the_table_size_with_its_table() {
    let design = template_design("Rich Teaching Design");
    let solved = design.solve().expect("every template tier is pinned");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    assert_eq!(
        meets.tiers,
        [
            CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint),
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
            CutMeet::Anchor(CutMeetAnchor::SetTableSize),
        ]
    );
    assert_eq!(meets.floating(&design), [] as [(usize, f64); 0]);
    let sheet = design.cutting_sheet(&solved);
    assert_eq!(
        instructions(&sheet),
        [
            ("P1", "Cut to centerpoint."),
            ("G1", "Set stone size."),
            ("C1", "Set girdle width."),
            ("T", "Set table size."),
        ]
    );
}

/// Pentalume, cut in sheet order: P1, G1 and C1 are the anchors, C2 meets the
/// G1-G2-C1 corners, and P2, P3, G2, C3 and the table float by the misses the reference check
/// measured at a 10 mm width (0.118, 0.264, 0.169, 0.052 and 0.828 mm), each nearest a vertex of
/// the tiers listed.
#[test]
fn pentalume_floats_where_the_cutter_found_no_meet_point() {
    let design = design_from_asc(PENTALUME);
    let solved = design.solve().expect("every imported tier is pinned");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    assert_eq!(meets.tiers.len(), 9);
    assert_eq!(
        meets.tiers[P1],
        CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint)
    );
    assert_eq!(
        meets.tiers[G1],
        CutMeet::Anchor(CutMeetAnchor::SetStoneSize)
    );
    assert_eq!(
        meets.tiers[C1],
        CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth)
    );
    assert_eq!(meets.tiers[C2], CutMeet::Meets(vec![G1, G2, C1]));
    for (tier, miss_mm, nearest) in [
        // Only the culet is on the stone when P2 is cut.
        (P2, 0.118, vec![P1]),
        (P3, 0.264, vec![P1, P2]),
        (G2, 0.169, vec![P1, P3, G1]),
        (C3, 0.052, vec![G1, G2, C1, C2]),
        // Not "Set table size.": the crown's C1-C2-C3 corners lie off the axis, above the
        // girdle, so the table has a point it could meet, and misses it.
        (TABLE, 0.828, vec![C1, C2, C3]),
    ] {
        let CutMeet::Floats {
            miss,
            nearest: found,
        } = &meets.tiers[tier]
        else {
            panic!("tier {tier} should float: {:?}", meets.tiers[tier]);
        };
        let expected = units(miss_mm);
        assert!(
            (miss - expected).abs() <= 0.02 * expected,
            "tier {tier}: miss {miss}, expected {expected}"
        );
        assert_eq!(found, &nearest, "tier {tier}");
    }
    let floating: Vec<usize> = meets
        .floating(&design)
        .into_iter()
        .map(|(tier, _)| tier)
        .collect();
    assert_eq!(floating, [P2, P3, G2, C3, TABLE]);
    assert!((meets.size - 2.2396).abs() < 1e-3, "{}", meets.size);
}

/// The sheet prints a floating tier's depth alone, in millimetres once a girdle diameter gives
/// them, else as a mast; the editor's Checks, not the sheet, name the tiers with no meet point.
/// The anchors and the meeting C2 print their computed instructions; G1 prints its recorded note.
#[test]
fn pentalume_sheet_gives_the_floating_tiers_their_depth() {
    let mut design = design_from_asc(PENTALUME);
    let solved = design.solve().expect("every imported tier is pinned");
    let unanchored = design.cutting_sheet(&solved);
    assert_eq!(row_text(&unanchored, "P2").0, "Set to mast depth 0.7662");
    assert_eq!(row_text(&unanchored, "T").0, "Set to mast depth 0.3500");

    design.girdle_diameter_mm = Some(10.0);
    let sheet = design.cutting_sheet(&solved);
    for code in ["P2", "P3", "G2", "C3", "T"] {
        let (text, depth_mm) = row_text(&sheet, code);
        let mm = depth_mm.expect("a girdle diameter gives millimetres");
        assert_eq!(text, format!("Cut to depth {mm:.2} mm."), "{code}");
    }
    assert_eq!(row_text(&sheet, "P1").0, "Cut to centerpoint.");
    assert_eq!(row_text(&sheet, "G1").0, "Set stone size.");
    assert_eq!(row_text(&sheet, "C1").0, "Set girdle width.");
    assert_eq!(row_text(&sheet, "C2").0, "Meet G1, G2, C1");
}

/// Snapping Pentalume with a 0.3 mm limit, in cutting order:
/// - P2 moves 0.12 mm deeper, through the culet, the one point on the stone when it is cut;
/// - that leaves P3 the culet alone to meet, 0.45 mm away, beyond the limit;
/// - G2 moves 0.18 mm in to the P1-P2-G1 corners, which moves C2 off its meet, so C2 moves
///   0.09 mm onto the new G1-G2-C1 corners;
/// - C3 then meets without moving;
/// - the table still floats, by more than the limit.
///
/// After a re-solve the walk agrees, and a second snap at the default limit moves nothing.
#[test]
fn snapping_pentalume_moves_the_tiers_a_point_is_within_reach_of() {
    let mut design = design_from_asc(PENTALUME);
    let solved = design.solve().expect("every imported tier is pinned");
    let max_move = units(0.3);
    let report = design
        .snap_depths_to_meets(&solved, max_move)
        .expect("the stone closes");
    let moved: Vec<usize> = report.moved.iter().map(|snap| snap.tier).collect();
    assert_eq!(moved, [P2, G2, C2]);
    assert_eq!(report.still_floating, [P3, TABLE]);
    let expected = [
        (P2, 0.741_407_27, vec![P1]),
        (G2, 1.091_834_99, vec![P1, P2, G1]),
        (C2, 0.582_723_53, vec![G1, G2, C1]),
    ];
    for (snap, (tier, new_mast, meets)) in report.moved.iter().zip(expected) {
        assert_eq!(snap.tier, tier);
        assert_eq!(snap.old_mast, solved[tier].mast, "tier {tier}");
        assert!(
            (snap.new_mast - new_mast).abs() < 1e-5,
            "tier {tier}: {} against {new_mast}",
            snap.new_mast
        );
        assert!(snap.new_mast < snap.old_mast, "tier {tier} is cut deeper");
        assert!((snap.new_mast - snap.old_mast).abs() <= max_move);
        assert_eq!(snap.meets, meets, "tier {tier}");
        assert_eq!(
            design.tiers[tier].constraint,
            MeetConstraint::ScaleReference(snap.new_mast)
        );
    }
    for tier in [P1, P3, G1, C1, C3, TABLE] {
        assert_eq!(
            design.tiers[tier].constraint,
            MeetConstraint::ScaleReference(solved[tier].mast),
            "tier {tier} keeps its depth"
        );
    }
    // The moved depths are saved to the design file and read back bit for bit.
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("the design saves");
    let reopened = design_from_str(&text).expect("the design opens").design;
    let constraints = |design: &Design| -> Vec<MeetConstraint> {
        design
            .tiers
            .iter()
            .map(|tier| tier.constraint.clone())
            .collect()
    };
    assert_eq!(constraints(&reopened), constraints(&design));

    let resolved = design.solve().expect("still pinned");
    let after = design.cut_meets(&resolved).expect("the stone closes");
    for tier in [P2, G2, C2, C3] {
        assert!(
            matches!(after.tiers[tier], CutMeet::Meets(_)),
            "tier {tier}: {:?}",
            after.tiers[tier]
        );
    }
    let floating = after.floating(&design);
    let tiers: Vec<usize> = floating.iter().map(|&(tier, _)| tier).collect();
    assert_eq!(tiers, [P3, TABLE]);
    // After the snap, P3's only point is the culet.
    assert!((floating[0].1 - 0.094_14).abs() < 1e-3, "{floating:?}");
    assert!(floating.iter().all(|&(_, miss)| miss > max_move));

    let again = design
        .snap_depths_to_meets(&resolved, DEFAULT_SNAP_MAX_MOVE_FRACTION * after.size)
        .expect("the stone closes");
    assert!(again.moved.is_empty(), "{again:?}");
    assert_eq!(again.still_floating, [P3, TABLE]);
}

/// A mast list that does not belong to the design gives no answer, and the snap moves nothing.
#[test]
fn a_misaligned_solve_gives_no_answer() {
    let mut design = design_from_asc(PENTALUME);
    let solved = design.solve().expect("every imported tier is pinned");
    assert_eq!(design.cut_meets(&solved[..3]), None);
    assert_eq!(design.snap_depths_to_meets(&solved[..3], 1.0), None);
    assert_eq!(design.tiers, design_from_asc(PENTALUME).tiers);
}
