//! The strip's Slint-free helpers: the words and the cells.

use super::{
    present::{age_text, cells_of, label_text, material_label},
    state::{displayed_figures, feed_is_outdated},
};
use pleochroma::color::metrics::{AsetAreaShares, AxisProfile};
use pleochroma_cut_core::MaterialSelection;
use pleochroma_editor::{
    live_deltas::figure_deltas,
    metric_deltas::{average_axis_profiles, average_axis_profiles_within},
    sweep::{RowMetrics, TiltAverages},
};

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
fn a_late_solve_of_an_older_design_is_not_fed() {
    // Nothing fed yet, no design opened above generation 0: anything is taken.
    assert!(!feed_is_outdated(None, 0, 3));
    // The masts of generation 7 are held; a solve of generation 5 (an edit was undone while it
    // ran) is older, the same generation and a newer one are not.
    assert!(feed_is_outdated(Some(7), 0, 5));
    assert!(!feed_is_outdated(Some(7), 0, 7));
    assert!(!feed_is_outdated(Some(7), 0, 8));
    // A design opened at generation 10 drops the solves of the design before it.
    assert!(feed_is_outdated(None, 10, 9));
    assert!(!feed_is_outdated(None, 10, 10));
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

fn with_tilt(brilliance: f32) -> RowMetrics {
    RowMetrics {
        tilt: Some(TiltAverages {
            brilliance_pct: brilliance,
            windowing_pct: 12.0,
            extinction_pct: 25.0,
        }),
        ..with_contrast(20.0)
    }
}

/// The figures card puts the caption "Tilt average " + the range before the last three cells, so the
/// tilt averages must stay the last three, in this order.
#[test]
fn the_three_tilt_averages_are_the_last_cells() {
    let cells = cells_of(&figure_deltas(&with_tilt(44.0), &with_tilt(46.0)));
    let names: Vec<&str> = cells.iter().map(|cell| cell.name.as_str()).collect();
    assert!(
        names.ends_with(&["Tilt Brilliance", "Tilt Windowing", "Tilt Extinction"]),
        "{names:?}"
    );
    let without = cells_of(&figure_deltas(&with_contrast(20.0), &with_contrast(20.0)));
    assert!(
        without.iter().all(|cell| !cell.name.starts_with("Tilt")),
        "no tilt cells until both sides carry the averages"
    );
}

#[test]
fn identical_figures_are_all_about_the_same() {
    let cells = cells_of(&figure_deltas(&metrics(20.0, 0), &metrics(20.0, 0)));
    assert_ne!(cells.len(), 0);
    assert!(cells.iter().all(|cell| cell.tone == 0));
}

/// Four axes whose brilliance rises with the square of the tilt, so a narrower range reads lower.
fn sloped_profiles(base: f32) -> Vec<AxisProfile> {
    (0..4)
        .map(|_| AxisProfile {
            brilliance: std::array::from_fn(|index| {
                let tilt = index as f32 - 90.0;
                tilt.mul_add(tilt * 0.001, base)
            }),
            windowing: std::array::from_fn(|index| 10.0 + index as f32 * 0.01),
            extinction: std::array::from_fn(|index| 20.0 - index as f32 * 0.01),
        })
        .collect()
}

/// Figures whose stored tilt average is the all-tilts mean of `profiles`, as the worker files them.
fn scored_with(profiles: &[AxisProfile]) -> RowMetrics {
    RowMetrics {
        tilt: Some(average_axis_profiles(profiles)),
        ..with_contrast(20.0)
    }
}

#[test]
fn the_default_range_shows_the_all_tilts_mean_bit_for_bit() {
    let profiles = sloped_profiles(44.0);
    let stored = scored_with(&profiles);
    let shown = displayed_figures(&stored, Some(&profiles), 90);
    assert_eq!(shown, stored, "at 90 the shown figures are the scored ones");
    assert_eq!(shown.tilt, Some(average_axis_profiles(&profiles)));
}

#[test]
fn a_narrower_range_averages_the_kept_profiles_again_and_back_to_ninety_returns_the_first() {
    let profiles = sloped_profiles(44.0);
    let stored = scored_with(&profiles);
    let first = displayed_figures(&stored, Some(&profiles), 90);
    let narrow = displayed_figures(&stored, Some(&profiles), 30);
    assert_eq!(
        narrow.tilt,
        Some(average_axis_profiles_within(&profiles, 30.0))
    );
    assert_ne!(
        narrow.tilt, first.tilt,
        "a window of 30 degrees reads lower"
    );
    let brilliance = |figures: &RowMetrics| figures.tilt.map(|tilt| tilt.brilliance_pct);
    assert!(brilliance(&narrow) < brilliance(&first));
    assert_eq!(
        narrow.brilliance_pct.to_bits(),
        stored.brilliance_pct.to_bits(),
        "the table-up figures never follow the range"
    );
    assert_eq!(displayed_figures(&stored, Some(&profiles), 90), first);
}

#[test]
fn figures_without_profiles_or_without_a_tilt_average_are_shown_as_they_are() {
    let profiles = sloped_profiles(44.0);
    let plain = with_contrast(20.0);
    assert_eq!(displayed_figures(&plain, Some(&profiles), 30), plain);
    let stored = scored_with(&profiles);
    assert_eq!(displayed_figures(&stored, None, 30), stored);
}

#[test]
fn the_cells_change_with_the_range_and_no_measurement_is_needed() {
    let was = sloped_profiles(40.0);
    let now = sloped_profiles(43.0);
    let cells_at = |window_deg: u8| {
        cells_of(&figure_deltas(
            &displayed_figures(&scored_with(&was), Some(&was), window_deg),
            &displayed_figures(&scored_with(&now), Some(&now), window_deg),
        ))
    };
    let wide = cells_at(90);
    let narrow = cells_at(30);
    assert_eq!(wide.len(), narrow.len(), "the same cells at any range");
    let tilt_cell = |cells: &[super::present::CellView]| {
        cells
            .iter()
            .find(|cell| cell.name == "Tilt Brilliance")
            .map(|cell| (cell.value.clone(), cell.delta.clone()))
    };
    assert_ne!(tilt_cell(&wide), tilt_cell(&narrow));
    assert_eq!(
        tilt_cell(&cells_at(90)),
        tilt_cell(&wide),
        "back to 90: the first values"
    );
}
