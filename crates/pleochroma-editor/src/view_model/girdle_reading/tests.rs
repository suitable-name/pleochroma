//! Tests for [`GirdleReading`].
//!
//! They cover the girdle read at its line on the meet-point round brilliant
//! template, Pentalume (an unlevel line) and Cam's Portuguese (a level one), and the band a
//! design without a girdle line falls back to.

use super::GirdleReading;
use crate::{
    cut_sheet::{SizeData, StoneSummary},
    loading::design_from_asc_text,
    view_model::yield_report::{
        girdle_and_ratio_texts_from_solved, proportion_verdicts, proportion_verdicts_with_girdle,
    },
};
use pleochroma::geometry::stone_metrics::StoneProportions;
use pleochroma_cut_core::{Design, templates::TEMPLATES};

/// Pentalume, every mast as published, its tiers named `P1` ... `C3`, `T`.
const PENTALUME: &str =
    include_str!("../../../../pleochroma-cut-core/src/design/girdle_level/pentalume.asc");

/// Cam's Portuguese (PC 45.140, public domain), verbatim.
const CAMS_PORTUGUESE: &str =
    include_str!("../../../../pleochroma-cut-core/src/design/girdle_level/cams_portuguese.asc");

/// The design `text` describes, opened the way the editor opens a bare `.asc`.
fn opened(text: &str) -> Design {
    design_from_asc_text("fixture.asc", text, None)
        .expect("the fixture parses")
        .design
}

/// The Standard Round Brilliant template as the New Design dialog makes it, in its own material.
fn standard_round_brilliant() -> Design {
    let spec = &TEMPLATES[0];
    assert_eq!(spec.name, "Standard Round Brilliant", "the first template");
    let mut design = Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers());
    design.material = spec.default_material_selection();
    design
}

/// `design`'s girdle reading, solved.
fn reading_of(design: &Design) -> (GirdleReading, StoneProportions) {
    let solved = design.solve().expect("every tier is pinned");
    let proportions = design.stone_proportions(&solved).expect("the stone closes");
    let reading =
        GirdleReading::of(design, &solved, &proportions).expect("the design has a girdle");
    (reading, proportions)
}

/// The Stone summary's girdle line of `design`.
fn summary_girdle_row(design: &Design) -> Option<String> {
    let solved = design.solve().expect("every tier is pinned");
    StoneSummary::build(design, &solved, &SizeData::default(), 1.54, None)
        .rows()
        .into_iter()
        .find(|row| row.label == "Girdle thickness")
        .map(|row| row.value)
}

#[test]
fn the_meet_point_brilliants_girdle_line_is_within_its_window_where_its_band_is_not() {
    // Pinned to the template's masts: the girdle line is 2.75 % of the width thick, while the
    // band, V-scallops included, is some 8.5 %.
    let design = standard_round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let (reading, proportions) = reading_of(&design);
    assert!(reading.is_level(), "{reading:?}");
    let (thinnest, thickest) = reading.percents();
    assert!((thinnest - 2.75).abs() < 0.01, "{reading:?}");
    assert!((thickest - 2.75).abs() < 0.01, "{reading:?}");
    assert!(
        reading.width_text().ends_with("% of width") && !reading.width_text().contains("level"),
        "{}",
        reading.width_text()
    );

    let n_d = design.effective_refractive_index();
    let verdicts = proportion_verdicts(&design, &solved, &proportions, n_d);
    assert!(
        (0..=1).contains(&verdicts.girdle_pct.level),
        "the girdle line reads within or near: level {} ({})",
        verdicts.girdle_pct.level,
        verdicts.girdle_pct.reason
    );
    // The band, judged as a girdle window judges it, is outside.
    let band = proportions.girdle_to_width_percent.expect("a girdle band");
    let by_band = proportion_verdicts_with_girdle(
        &design,
        &proportions,
        Some(GirdleReading::Band {
            thickness: proportions.girdle_thickness.expect("a girdle band"),
            percent: band,
        }),
        n_d,
    );
    assert_eq!(by_band.girdle_pct.level, 2, "the band of {band:.2} %");
}

#[test]
fn pentalumes_unlevel_line_reads_as_a_range_everywhere() {
    let mut design = opened(PENTALUME);
    let (reading, _) = reading_of(&design);
    assert!(!reading.is_level());
    // 0.054 to 0.085 units across a width of 2.13.
    let (thinnest, thickest) = reading.percents();
    assert!((thinnest - 2.535).abs() < 0.01, "{reading:?}");
    assert!((thickest - 3.991).abs() < 0.01, "{reading:?}");
    assert!((reading.judged_percent() - thinnest).abs() < 1e-12);
    assert_eq!(reading.percent_text(), "2.5\u{2013}4.0%");
    assert_eq!(reading.width_text(), "2.5\u{2013}4.0% of width (not level)");
    assert_eq!(
        reading.thickness_text(Some(10.0 / 2.13)),
        "0.254\u{2013}0.399 mm (not level)"
    );
    assert_eq!(
        summary_girdle_row(&design).as_deref(),
        Some("2.5\u{2013}4.0% of width (not level)")
    );

    // The panel's readouts say the same, at 10 mm across.
    design.girdle_diameter_mm = Some(10.0);
    let solved = design.solve().expect("every tier is pinned");
    let (thickness, _, _, of_width) = girdle_and_ratio_texts_from_solved(&design, &solved);
    assert_eq!(thickness, "0.254\u{2013}0.399 mm (not level)");
    assert_eq!(of_width, "2.5\u{2013}4.0%");
}

#[test]
fn cams_portuguese_level_line_reads_as_one_figure() {
    let design = opened(CAMS_PORTUGUESE);
    let (reading, proportions) = reading_of(&design);
    assert!(reading.is_level());
    // 0.0628 units across a width of 2.1465: the line and the band agree on a girdle cut level
    // with no scallops between its corners. The file's five-decimal masts leave the corners up to
    // 5e-5 units apart (0.002 % of the width), so the two ends differ in the last digits only.
    let (thinnest, thickest) = reading.percents();
    assert!((thinnest - 2.9255).abs() < 0.01, "{reading:?}");
    assert!((thickest - thinnest).abs() < 0.005, "{reading:?}");
    assert_eq!(reading.width_text(), "2.9% of width");
    assert_eq!(reading.thickness_text(None), "0.063");
    assert_eq!(
        summary_girdle_row(&design).as_deref(),
        Some("2.9% of width")
    );
    let solved = design.solve().expect("every tier is pinned");
    let (thickness, _, _, of_width) = girdle_and_ratio_texts_from_solved(&design, &solved);
    assert_eq!((thickness.as_str(), of_width.as_str()), ("0.063", "2.9%"));
    let band = proportions.girdle_to_width_percent.expect("a girdle band");
    assert!(
        (band - thinnest).abs() < 0.05,
        "band {band}, line {thinnest}"
    );
}

#[test]
fn without_a_girdle_line_the_band_is_read() {
    let proportions = StoneProportions {
        table_percent: None,
        crown_height: None,
        pavilion_depth: None,
        girdle_thickness: Some(0.1),
        total_depth: 1.0,
        length_to_width: Some(1.0),
        crown_to_width_percent: None,
        pavilion_to_width_percent: None,
        girdle_to_width_percent: Some(5.0),
    };
    let reading = GirdleReading::from_level(None, &proportions).expect("a band");
    assert_eq!(
        reading,
        GirdleReading::Band {
            thickness: 0.1,
            percent: 5.0
        }
    );
    assert!(reading.is_level());
    assert!((reading.judged_percent() - 5.0).abs() < 1e-12);
    assert_eq!(reading.width_text(), "5.0% of width");
    assert_eq!(reading.thickness_text(Some(2.0)), "0.200 mm");

    // No band either: nothing to read.
    let none = StoneProportions {
        girdle_thickness: None,
        girdle_to_width_percent: None,
        ..proportions
    };
    assert_eq!(GirdleReading::from_level(None, &none), None);
}
