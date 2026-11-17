//! The strip's Slint-free helpers: the words and the cells.

use super::present::{age_text, cells_of, label_text, material_label};
use pleochroma::color::metrics::AsetAreaShares;
use pleochroma_cut_core::MaterialSelection;
use pleochroma_editor::{live_deltas::figure_deltas, sweep::RowMetrics};

fn metrics(brilliance_pct: f32, warning_count: usize) -> RowMetrics {
    RowMetrics {
        brilliance_pct,
        windowing_pct: 20.0,
        extinction_pct: 15.0,
        fire_index: 4.0,
        scintillation_pct: 30.0,
        yield_pct: Some(38.0),
        warning_count,
        has_girdle: true,
        aset: None,
        tilt: None,
    }
}

#[test]
fn an_age_reads_in_the_largest_whole_unit() {
    assert_eq!(age_text(0), "just now");
    assert_eq!(age_text(59), "just now");
    assert_eq!(age_text(60), "1 min ago");
    assert_eq!(age_text(3599), "59 min ago");
    assert_eq!(age_text(3600), "1 h ago");
    assert_eq!(age_text(7300), "2 h ago");
}

#[test]
fn the_label_names_how_the_baseline_came_to_be() {
    assert_eq!(label_text("opened", 240), "Baseline: opened 4 min ago");
    assert_eq!(
        label_text("after Optimize", 5),
        "Baseline: after Optimize just now"
    );
}

#[test]
fn a_material_is_named_by_its_pick_its_override_or_the_design() {
    let named = MaterialSelection {
        name: Some("Quartz".to_owned()),
        ..MaterialSelection::default()
    };
    assert_eq!(material_label(&named), "Quartz");
    let typed = MaterialSelection {
        refractive_index_override: Some(1.74),
        ..MaterialSelection::default()
    };
    assert_eq!(material_label(&typed), "Custom RI");
    assert_eq!(
        material_label(&MaterialSelection::default()),
        "the design's refractive index"
    );
}

#[test]
fn the_cells_follow_the_figures_with_their_tone() {
    let cells = cells_of(&figure_deltas(&metrics(20.0, 0), &metrics(28.0, 1)));
    let brilliance = cells.first().expect("brilliance is the first figure");
    assert_eq!(brilliance.short_label, "Bril.");
    assert_eq!(brilliance.tone, 1, "a large rise in brilliance is better");
    let warnings = cells
        .iter()
        .find(|cell| cell.short_label == "Warn.")
        .expect("the warning count is always shown");
    assert_eq!(warnings.tone, 2, "a facet that appears is worse");
}

fn with_contrast(contrast: f32) -> RowMetrics {
    RowMetrics {
        aset: Some(AsetAreaShares {
            red_pct: 40.0,
            green_pct: 20.0,
            blue_pct: 20.0,
            leak_pct: 10.0,
            lost_pct: 10.0,
            contrast,
        }),
        ..metrics(20.0, 0)
    }
}

#[test]
fn contrast_follows_extinction_and_precedes_yield_in_the_cells() {
    let cells = cells_of(&figure_deltas(&with_contrast(20.0), &with_contrast(32.0)));
    let names: Vec<&str> = cells.iter().map(|cell| cell.name.as_str()).collect();
    let extinction = names
        .iter()
        .position(|name| *name == "Extinction")
        .expect("extinction is shown");
    assert_eq!(names.get(extinction + 1), Some(&"Contrast"));
    assert_eq!(names.get(extinction + 2), Some(&"Yield"));
    let contrast = &cells[extinction + 1];
    assert_eq!(contrast.short_label, "Contr.");
    assert_eq!(contrast.tone, 1, "more contrast by 12 points is better");
    assert!(!contrast.flag);
}

#[test]
fn a_design_without_zone_figures_has_no_contrast_cell() {
    let cells = cells_of(&figure_deltas(&metrics(20.0, 0), &metrics(20.0, 0)));
    assert!(cells.iter().all(|cell| cell.name != "Contrast"));
}

#[test]
fn identical_figures_are_all_about_the_same() {
    let cells = cells_of(&figure_deltas(&metrics(20.0, 0), &metrics(20.0, 0)));
    assert_ne!(cells.len(), 0);
    assert!(cells.iter().all(|cell| cell.tone == 0));
}
