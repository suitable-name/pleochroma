//! The meet-point tests of the templates: which templates are meet-point designs, what each tier
//! meets when it is cut, the proportions of the round brilliants, and the level girdle.

use super::*;
use crate::{
    CutMeet, CutMeetAnchor,
    proportions_windows::{MaterialClass, Metric, ShapeClass, Verdict, verdict_for},
};
use pleochroma::geometry::meet_solver::SolvedTier;

/// The three round brilliants: the standard and its shallow and deep pavilion variants, one
/// meet-point crown and one tier order. The round-brilliant tests below filter on this family.
const ROUND_BRILLIANTS: [&str; 3] = [
    "Standard Round Brilliant",
    "Round Brilliant -- Shallow Pavilion",
    "Round Brilliant -- Deep Pavilion",
];

/// The templates that are meet-point designs: every tier, cut in order on the template's own
/// rough, fixes a dimension or reaches a point already on the stone, and the girdle is level.
/// All nine.
const MEET_POINT_TEMPLATES: [&str; 9] = [
    "Standard Round Brilliant",
    "Round Brilliant -- Shallow Pavilion",
    "Round Brilliant -- Deep Pavilion",
    "Simple Teaching Design",
    "Rich Teaching Design",
    "Oval Brilliant",
    "Cushion Brilliant",
    "Emerald Step Cut",
    "Princess Cut",
];

/// The gears a remap lands every index of the template on a whole tooth of: the emerald's
/// indices are multiples of 12, so its meets survive every gear it offers. The other shapes'
/// break facets sit one tooth off a main, so a remap moves them off their points.
const EXACT_ON_EVERY_GEAR: [&str; 1] = ["Emerald Step Cut"];

/// `spec` on its default preform, with its solve.
fn solved_on_its_rough(spec: &TemplateSpec) -> (Design, Vec<SolvedTier>) {
    let design = design_on(spec, spec.preform.clone());
    let solved = design
        .solve()
        .unwrap_or_else(|e| panic!("{:?}: failed to solve: {e:?}", spec.name));
    (design, solved)
}

/// The template called `name`.
fn template_named(name: &str) -> &'static TemplateSpec {
    TEMPLATES
        .iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("no template {name:?}"))
}

/// The meet of every tier of the template called `name`, cut in order on its own rough.
fn cut_meets_of(name: &str) -> Vec<CutMeet> {
    let (design, solved) = solved_on_its_rough(template_named(name));
    design
        .cut_meets(&solved)
        .unwrap_or_else(|| panic!("{name:?}: the stone closes"))
        .tiers
}

/// Every template is a meet-point design: the list holds every name in `TEMPLATES` once, so a
/// new template has to be added to it (and pass the tests below).
#[test]
fn every_template_is_listed_as_a_meet_point_design() {
    for spec in TEMPLATES {
        assert_eq!(
            MEET_POINT_TEMPLATES
                .iter()
                .filter(|name| **name == spec.name)
                .count(),
            1,
            "{:?} must be in the list once",
            spec.name
        );
    }
    assert_eq!(MEET_POINT_TEMPLATES.len(), TEMPLATES.len());
}

/// The three round brilliants, cut in order on their own rough, reach a point at every tier
/// that is not an anchor ([`Design::cut_meets`]): the pavilion mains come first, the girdle sets
/// the stone size, the lower girdles meet the mains and the girdle at the girdle-face corners,
/// the culet meets the mains' point; the upper girdles set the girdle width, the crown mains
/// meet them on the girdle, the stars meet the upper girdles and the crown mains, the table
/// meets the crown mains and the stars. Nothing floats.
#[test]
fn the_round_brilliants_reach_a_point_at_every_tier() {
    for spec in TEMPLATES
        .iter()
        .filter(|spec| ROUND_BRILLIANTS.contains(&spec.name))
    {
        let (design, solved) = solved_on_its_rough(spec);
        let names: Vec<&str> = design.tiers.iter().map(|tier| tier.name.as_str()).collect();
        assert_eq!(names, ROUND_BRILLIANT_ORDER, "{:?}", spec.name);
        let meets = design
            .cut_meets(&solved)
            .unwrap_or_else(|| panic!("{:?}: the stone closes", spec.name));
        let floating = meets.floating(&design);
        assert!(
            floating.is_empty(),
            "{:?}: tiers with no meet point (tier, miss): {floating:?}",
            spec.name
        );
        // The first tier is an anchor: nothing to meet yet, or cut to the centerpoint.
        assert!(
            matches!(
                meets.tiers[0],
                CutMeet::Anchor(CutMeetAnchor::NothingToMeet | CutMeetAnchor::CutToCenterpoint)
            ),
            "{:?}: {:?}",
            spec.name,
            meets.tiers[0]
        );
        let expected = [
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Meets(vec![0, 1]),
            CutMeet::Meets(vec![0]),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
            CutMeet::Meets(vec![1, 4]),
            CutMeet::Meets(vec![4, 5]),
            CutMeet::Meets(vec![5, 6]),
        ];
        assert_eq!(meets.tiers[1..], expected, "{:?}", spec.name);
    }
}

/// The round brilliant's cutting sheet names a point for every tier but the anchors.
#[test]
fn the_round_brilliants_sheet_names_a_meet_for_every_tier() {
    let (design, solved) = solved_on_its_rough(&TEMPLATES[0]);
    let sheet = design.cutting_sheet(&solved);
    let rows: Vec<(&str, &str)> = sheet
        .rows
        .iter()
        .map(|row| (row.code.as_str(), row.meet_instruction.as_str()))
        .collect();
    let (first, rest) = rows.split_first().expect("eight rows");
    assert_eq!(first.0, "P1");
    assert!(
        ["Set to mast depth 0.6700", "Cut to centerpoint."].contains(&first.1),
        "{first:?}"
    );
    assert_eq!(
        rest,
        [
            ("G1", "Set stone size."),
            ("P2", "Meet P1, G1"),
            ("Culet", "Meet P1"),
            ("C1", "Set girdle width."),
            ("C2", "Meet G1, C1"),
            ("C3", "Meet C1, C2"),
            ("T", "Meet C2, C3"),
        ]
    );
}

/// The culet is a pin-prick: a facet of its own, too small to polish, that meets the point where
/// the pavilion mains meet. No round brilliant facet vanishes, on the template's gear or on the
/// gears the dialog offers.
#[test]
fn the_round_brilliants_keep_every_facet_and_a_pin_prick_culet() {
    for spec in TEMPLATES
        .iter()
        .filter(|spec| ROUND_BRILLIANTS.contains(&spec.name))
    {
        for gear in spec.allowed_gears_among(&DIALOG_GEARS) {
            let label = format!("{} on gear {gear}", spec.name);
            let stone = inspect(&remapped(spec, gear), &label);
            assert_eq!(stone.vanishing(), Vec::<String>::new(), "{label}");
            assert_eq!(stone.undersized(), ["Culet"], "{label}");
        }
    }
}

/// The round brilliants' table (55 to 56 % of the width) and total depth on their own rough,
/// judged against the reference window of the template's material as the verdict judges them.
/// The girdle is 2.75 % of the width thick at the corners where two girdle faces meet, where the
/// break facets meet the girdle line.
///
/// The girdle band the proportions report also spans the V's the break facets cut between those
/// corners, so that figure is not judged here.
#[test]
fn the_round_brilliants_have_a_classic_table_and_depth_and_a_girdle_line_of_2_75_percent() {
    for spec in TEMPLATES
        .iter()
        .filter(|spec| ROUND_BRILLIANTS.contains(&spec.name))
    {
        let (design, solved) = solved_on_its_rough(spec);
        let stone = inspect(&design, spec.name);
        let proportions = &stone.proportions;
        let table = proportions.table_percent.expect("a table facet");
        assert!(
            (55.0..=56.0).contains(&table),
            "{:?}: table {table:.2} %",
            spec.name
        );
        let depth = proportions.crown_to_width_percent.expect("a crown")
            + proportions.pavilion_to_width_percent.expect("a pavilion")
            + proportions.girdle_to_width_percent.expect("a girdle");
        let material = MaterialClass::from_ri(spec.design_ri);
        for (metric, value) in [
            (Metric::TablePercent, table),
            (Metric::TotalDepthPercent, depth),
        ] {
            let (verdict, window) = verdict_for(ShapeClass::Round, material, metric, value)
                .expect("a reference window");
            assert_eq!(
                verdict,
                Verdict::Within,
                "{:?}: {metric:?} {value:.2} is not in {}..{}",
                spec.name,
                window.min,
                window.max
            );
        }
        let level = design.girdle_level(&solved).expect("a girdle tier");
        let range: [f64; 2] = level.thickness_range().into();
        for thickness in range {
            let percent = 100.0 * thickness / level.width;
            assert!(
                (percent - 2.75).abs() < 0.01,
                "{:?}: the girdle is {percent:.3} % of the width at its corners",
                spec.name
            );
        }
    }
}

/// The teaching designs are anchored at every tier ([`Design::cut_meets`]): the pavilion mains
/// end in one point (cut to the centerpoint), the girdle sets the stone size; the Simple design's
/// table, its only crown tier, sets the girdle width; the Rich design's crown mains set it and
/// its table, with no crown point off the axis to meet, sets the table size.
#[test]
fn the_teaching_designs_are_anchored_at_every_tier() {
    let anchored = |name: &str| -> (Vec<CutMeet>, Vec<(String, String)>) {
        let spec = TEMPLATES
            .iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("no template {name:?}"));
        let (design, solved) = solved_on_its_rough(spec);
        let meets = design
            .cut_meets(&solved)
            .unwrap_or_else(|| panic!("{name:?}: the stone closes"));
        let sheet = design
            .cutting_sheet(&solved)
            .rows
            .into_iter()
            .map(|row| (row.code, row.meet_instruction))
            .collect();
        (meets.tiers, sheet)
    };
    let row = |code: &str, text: &str| (code.to_string(), text.to_string());

    let (meets, sheet) = anchored("Simple Teaching Design");
    assert_eq!(
        meets,
        [
            CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint),
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
        ]
    );
    assert_eq!(
        sheet,
        [
            row("P1", "Cut to centerpoint."),
            row("G1", "Set stone size."),
            row("T", "Set girdle width."),
        ]
    );

    let (meets, sheet) = anchored("Rich Teaching Design");
    assert_eq!(
        meets,
        [
            CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint),
            CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
            CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth),
            CutMeet::Anchor(CutMeetAnchor::SetTableSize),
        ]
    );
    assert_eq!(
        sheet,
        [
            row("P1", "Cut to centerpoint."),
            row("G1", "Set stone size."),
            row("C1", "Set girdle width."),
            row("T", "Set table size."),
        ]
    );
}

/// No tier of a meet-point template floats on the template's own rough.
#[test]
fn no_meet_point_template_has_a_tier_without_a_meet_point() {
    for spec in TEMPLATES
        .iter()
        .filter(|spec| MEET_POINT_TEMPLATES.contains(&spec.name))
    {
        let (design, solved) = solved_on_its_rough(spec);
        let meets = design
            .cut_meets(&solved)
            .unwrap_or_else(|| panic!("{:?}: the stone closes", spec.name));
        let floating = meets.floating(&design);
        assert!(
            floating.is_empty(),
            "{:?}: tiers with no meet point (tier, miss): {floating:?}",
            spec.name
        );
    }
}

/// Every template has a level girdle ([`Design::girdle_level`]): no tier cuts into it at a corner.
/// The break facets (or, on the emerald, the first steps) meet the girdle line at the corners
/// where two girdle faces meet. On the four non-round featured shapes the girdle line is 2.70 to
/// 2.76 % of the width thick at every corner.
#[test]
fn every_template_has_a_level_girdle() {
    for spec in TEMPLATES {
        let (design, solved) = solved_on_its_rough(spec);
        let level = design
            .girdle_level(&solved)
            .unwrap_or_else(|| panic!("{:?}: every template has a girdle tier", spec.name));
        let names: Vec<&str> = level
            .cut_in
            .iter()
            .map(|&tier| design.tiers[tier].name.as_str())
            .collect();
        assert!(
            level.is_level(),
            "{:?}: {names:?} cut into the girdle ({:?} above, {:?} below)",
            spec.name,
            level.top,
            level.bottom
        );
        if is_strict(spec) {
            let range: [f64; 2] = level.thickness_range().into();
            for thickness in range {
                let percent = 100.0 * thickness / level.width;
                assert!(
                    (2.70..=2.76).contains(&percent),
                    "{:?}: the girdle line is {percent:.3} % of the width thick",
                    spec.name
                );
            }
        }
    }
}

/// An anchor, for the expected lists below.
const fn anchor(kind: CutMeetAnchor) -> CutMeet {
    CutMeet::Anchor(kind)
}

/// A meet of the tiers `tiers` (stored indices, which are the cutting order here).
fn meets(tiers: &[usize]) -> CutMeet {
    CutMeet::Meets(tiers.to_vec())
}

/// The oval cuts its girdle outline first (Girdle 1 sets the stone size, the other four are cut
/// to depth), then sets the girdle line with the diagonal pavilion mains on Girdle 3. Each lower
/// girdle meets a girdle corner the tiers before it left, the second of each pair dipping into a V
/// in the middle of Girdle 1 or Girdle 5, and the side and end mains meet those V's (the side
/// mains meet each other in a short keel). The crown: the diagonal crown mains set the girdle
/// width and meet in the crown's point, the side and end mains meet that point, each upper girdle
/// meets its main's girdle corner (the second of a pair also the point the first left on the
/// ridge between two mains), the stars meet their four-facet star points and the table the side
/// and end kite tops.
#[test]
fn the_oval_meets_a_point_at_every_tier() {
    use CutMeetAnchor::{NothingToMeet, SetGirdleLine, SetGirdleWidth, SetStoneSize};
    assert_eq!(
        cut_meets_of("Oval Brilliant"),
        [
            anchor(SetStoneSize),
            anchor(NothingToMeet),
            anchor(NothingToMeet),
            anchor(NothingToMeet),
            anchor(NothingToMeet),
            anchor(SetGirdleLine),
            meets(&[1, 2, 5]),
            meets(&[0, 1, 6]),
            meets(&[0, 7]),
            meets(&[2, 3, 5]),
            meets(&[3, 4, 9]),
            meets(&[4, 10]),
            anchor(SetGirdleWidth),
            meets(&[12]),
            meets(&[12, 13]),
            meets(&[0, 1, 13]),
            meets(&[1, 2, 12, 13, 15]),
            meets(&[2, 3, 12]),
            meets(&[3, 4, 12, 14, 17]),
            meets(&[12, 13, 15, 16]),
            meets(&[12, 14, 17, 18]),
            meets(&[13, 14, 19, 20]),
        ]
    );
}

/// The cushion cuts its girdle outline first; the first lower girdles set the girdle line at the
/// corners where the side face meets the rounding faces, and the side mains meet the V they cut
/// in the middle of the side face. The second lower girdles meet those corners and run level
/// along the rounding faces, and the corner mains meet the next corners and run level along the
/// corner faces. The crown repeats it, then the stars meet the star points and the table the
/// tops of the side kites. No tier meets a point a later tier cuts away.
#[test]
fn the_cushion_meets_a_point_at_every_tier() {
    use CutMeetAnchor::{NothingToMeet, SetGirdleLine, SetGirdleWidth, SetStoneSize};
    assert_eq!(
        cut_meets_of("Cushion Brilliant"),
        [
            anchor(SetStoneSize),
            anchor(NothingToMeet),
            anchor(NothingToMeet),
            anchor(SetGirdleLine),
            meets(&[0, 3]),
            meets(&[0, 1, 3]),
            meets(&[1, 2, 5]),
            anchor(SetGirdleWidth),
            meets(&[0, 7]),
            meets(&[0, 1, 7]),
            meets(&[1, 2, 9]),
            meets(&[7, 8, 10]),
            meets(&[8, 11]),
        ]
    );
}

/// The emerald cuts its girdle first, then leads each half with its end steps: the first end step
/// sets the girdle line (girdle width on the crown), the third meets where the first crosses the
/// edge between the side and corner girdle faces, the first corner and side steps carry the line
/// round, the second end step goes in between, and every other step meets the three-tier corner
/// the steps before it left. The table meets the tops of the third corner steps.
#[test]
fn the_emerald_meets_a_point_at_every_tier() {
    use CutMeetAnchor::{NothingToMeet, SetGirdleLine, SetGirdleWidth, SetStoneSize};
    assert_eq!(
        cut_meets_of("Emerald Step Cut"),
        [
            anchor(SetStoneSize),
            anchor(NothingToMeet),
            anchor(NothingToMeet),
            anchor(SetGirdleLine),
            meets(&[0, 2, 3]),
            meets(&[1, 2, 3]),
            meets(&[0, 2, 5]),
            meets(&[4, 5, 6]),
            meets(&[3, 5, 7]),
            meets(&[4, 7, 8]),
            meets(&[6, 8, 9]),
            meets(&[5, 6, 8]),
            anchor(SetGirdleWidth),
            meets(&[0, 2, 12]),
            meets(&[1, 2, 12]),
            meets(&[0, 2, 14]),
            meets(&[13, 14, 15]),
            meets(&[12, 14, 16]),
            meets(&[13, 16, 17]),
            meets(&[15, 17, 18]),
            meets(&[14, 15, 17]),
            meets(&[13, 18, 19]),
        ]
    );
}

/// The princess cuts its girdle first (the corners cut to depth), then the lower girdle sets the
/// girdle line at the eight girdle corners; the side mains meet those corners and end in the
/// culet point, the corner mains meet the point of each corner V. The crown repeats it, the stars
/// meet the points above the girdle corners and the table the tops of the side mains.
#[test]
fn the_princess_meets_a_point_at_every_tier() {
    use CutMeetAnchor::{NothingToMeet, SetGirdleLine, SetGirdleWidth, SetStoneSize};
    assert_eq!(
        cut_meets_of("Princess Cut"),
        [
            anchor(SetStoneSize),
            anchor(NothingToMeet),
            anchor(SetGirdleLine),
            meets(&[0, 1, 2]),
            meets(&[1, 2]),
            anchor(SetGirdleWidth),
            meets(&[0, 1, 5]),
            meets(&[1, 5]),
            meets(&[5, 6, 7]),
            meets(&[6, 8]),
        ]
    );
}

/// What the cutting sheet prints for `meet`, the tier codes being `codes`: an anchor's sentence,
/// or "Meet" and the codes of the tiers met. `None` for a tier cut to its depth (the text holds
/// the mast).
fn sheet_text(meet: &CutMeet, codes: &[String]) -> Option<String> {
    match meet {
        CutMeet::Anchor(CutMeetAnchor::SetStoneSize) => Some("Set stone size.".to_string()),
        CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint) => Some("Cut to centerpoint.".to_string()),
        CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth) => Some("Set girdle width.".to_string()),
        CutMeet::Anchor(CutMeetAnchor::SetGirdleLine) => Some("Set girdle line.".to_string()),
        CutMeet::Anchor(CutMeetAnchor::SetTableSize) => Some("Set table size.".to_string()),
        CutMeet::Anchor(CutMeetAnchor::NothingToMeet) | CutMeet::Floats { .. } => None,
        CutMeet::Meets(tiers) => {
            let named: Vec<&str> = tiers.iter().map(|&tier| codes[tier].as_str()).collect();
            Some(format!("Meet {}", named.join(", ")))
        }
    }
}

/// Every template's cutting sheet, row by row in cutting order (which is the stored order), gives
/// each tier its code and the instruction its meet calls for: a tier cut to depth (nothing to
/// meet yet) prints its mast, every other tier an anchor's sentence or the points it meets.
#[test]
fn every_template_sheet_prints_the_meet_of_every_tier() {
    for spec in TEMPLATES {
        let (design, solved) = solved_on_its_rough(spec);
        let meets = design
            .cut_meets(&solved)
            .unwrap_or_else(|| panic!("{:?}: the stone closes", spec.name));
        let codes: Vec<String> = crate::compute_tier_labels(&design.tiers)
            .into_iter()
            .map(|label| label.code)
            .collect();
        let sheet = design.cutting_sheet(&solved);
        assert_eq!(sheet.rows.len(), design.tiers.len(), "{:?}", spec.name);
        for (tier, row) in sheet.rows.iter().enumerate() {
            assert_eq!(row.code, codes[tier], "{:?} row {tier}", spec.name);
            match sheet_text(&meets.tiers[tier], &codes) {
                Some(text) => {
                    assert_eq!(row.meet_instruction, text, "{:?} {}", spec.name, row.code);
                }
                None => assert!(
                    row.meet_instruction.starts_with("Set to mast depth "),
                    "{:?} {}: {:?}",
                    spec.name,
                    row.code,
                    row.meet_instruction
                ),
            }
        }
    }
}

/// The emerald's indices are multiples of 12, so a remap to any gear it offers lands every facet
/// on its own azimuth: every tier meets the same points as on the template's own gear, and the
/// girdle stays level.
#[test]
fn the_emerald_keeps_every_meet_on_every_gear_it_offers() {
    for name in EXACT_ON_EVERY_GEAR {
        let spec = template_named(name);
        let own = cut_meets_of(name);
        for gear in spec.allowed_gears_among(&DIALOG_GEARS) {
            let design = remapped(spec, gear);
            let solved = design.solve().expect("the remapped design solves");
            let meets = design
                .cut_meets(&solved)
                .unwrap_or_else(|| panic!("{name} on gear {gear}: the stone closes"));
            assert_eq!(meets.tiers, own, "{name} on gear {gear}");
            let level = design.girdle_level(&solved).expect("a girdle tier");
            assert!(
                level.is_level(),
                "{name} on gear {gear}: {:?}",
                level.cut_in
            );
        }
    }
}

/// The oval's, the cushion's and the princess's break facets sit one tooth off a main, so a
/// remap to another gear turns them (and the tiers that meet them) off their points: the stones
/// keep every facet (`every_allowed_gear_keeps_the_stone_intact`), but they are not meet-point
/// designs there. Every floating tier misses its nearest point by no more than the default snap
/// reach ([`crate::DEFAULT_SNAP_MAX_MOVE_FRACTION`] of the stone's size).
#[test]
fn the_other_shapes_float_off_their_points_on_other_gears_within_snap_reach() {
    for spec in TEMPLATES
        .iter()
        .filter(|spec| is_strict(spec) && !EXACT_ON_EVERY_GEAR.contains(&spec.name))
    {
        for gear in spec
            .allowed_gears_among(&DIALOG_GEARS)
            .into_iter()
            .filter(|&gear| gear != spec.gear_teeth)
        {
            let label = format!("{} on gear {gear}", spec.name);
            let design = remapped(spec, gear);
            let solved = design.solve().expect("the remapped design solves");
            let meets = design
                .cut_meets(&solved)
                .unwrap_or_else(|| panic!("{label}: the stone closes"));
            let floating = meets.floating(&design);
            assert!(
                !floating.is_empty(),
                "{label}: every tier meets, so the template belongs in EXACT_ON_EVERY_GEAR"
            );
            let reach = crate::DEFAULT_SNAP_MAX_MOVE_FRACTION * meets.size;
            for (tier, miss) in floating {
                assert!(
                    miss <= reach,
                    "{label}: {} misses its point by {miss:.5}, beyond the snap's {reach:.5}",
                    design.tiers[tier].name
                );
            }
        }
    }
}
