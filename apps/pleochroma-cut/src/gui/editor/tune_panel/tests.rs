//! Tests of the Tune view's pure parts: the tier list and its order, "Next Tier", the Baseline's
//! angles, the mini sweep's reading (best, plateau, Jump to Best, basis, cache) and the sentences.

use super::{
    metrics::{self, Visibility},
    mini_sweep::{self, Basis, Cache, FIGURES},
    order::{
        Side, TierInfo, apply_custom_order, baseline_angles, move_tier, next_untuned, side_of,
        tier_infos, tuning_order,
    },
    present::{ListInput, change_text, chart_marks, format_angle, readout, tier_rows, title_text},
    simple_mode,
};
use pleochroma_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use pleochroma_editor::sweep::{RowMetrics, SweepOutcome, SweepRow};
use std::collections::BTreeSet;

fn round_brilliant() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    design
}

fn info(index: usize, code: &str, name: &str, angle_deg: f64, side: Side) -> TierInfo {
    TierInfo {
        index,
        code: code.to_owned(),
        name: name.to_owned(),
        angle_deg,
        side,
        driven: false,
        relation: None,
    }
}

/// A design's tiers as the list sees them: a pavilion main that is not first, a crown main, two
/// other crown tiers (numbered out of order), a pavilion tier and a girdle tier.
fn sample_infos() -> Vec<TierInfo> {
    vec![
        info(0, "C3", "Upper Girdle", 41.0, Side::Crown),
        info(1, "G1", "Girdle", 90.0, Side::Girdle),
        info(2, "P2", "Lower Girdle", -42.5, Side::Pavilion),
        info(3, "C2", "Star", 15.0, Side::Crown),
        info(4, "C1", "Crown Main", 34.5, Side::Crown),
        info(5, "P1", "Pavilion Main", -41.0, Side::Pavilion),
    ]
}

#[test]
fn a_table_and_a_culet_have_no_side_and_a_girdle_tier_has_no_angle_to_tune() {
    assert_eq!(side_of(0.0), None);
    assert_eq!(side_of(-0.0), None);
    assert_eq!(side_of(34.5), Some(Side::Crown));
    assert_eq!(side_of(-41.0), Some(Side::Pavilion));
    assert_eq!(side_of(90.0), Some(Side::Girdle));
    assert!(!info(1, "G1", "Girdle", 90.0, Side::Girdle).tunable());
    let mut driven = info(2, "P2", "Lower Girdle", -42.5, Side::Pavilion);
    assert!(driven.tunable());
    driven.driven = true;
    assert!(!driven.tunable());
}

#[test]
fn the_list_leaves_out_a_table_and_a_culet() {
    let design = round_brilliant();
    let infos = tier_infos(&design);
    let listed: Vec<usize> = infos.iter().map(|info| info.index).collect();
    // Table (0) and Culet (7) are not listed; the Girdle (4) is.
    assert_eq!(listed, vec![1, 2, 3, 4, 5, 6]);
    assert!(infos.iter().all(|info| !info.code.is_empty()));
}

#[test]
fn the_tuning_order_starts_with_the_mains_and_ends_with_the_girdle() {
    let order = tuning_order(&sample_infos());
    // Pavilion main, crown main, the other crown tiers by code, the other pavilion tiers, the
    // girdle.
    assert_eq!(order, vec![5, 4, 3, 0, 2, 1]);
    let design = round_brilliant();
    let order = tuning_order(&tier_infos(&design));
    assert_eq!(order.len(), 6);
    assert_eq!(&order[..2], &[5, 2], "Pavilion Main, then Crown Main");
    assert_eq!(*order.last().expect("a girdle"), 4);
    let mut middle = order[2..5].to_vec();
    middle.sort_unstable();
    assert_eq!(middle, vec![1, 3, 6]);
    assert_eq!(order[4], 6, "the pavilion tier follows the crown tiers");
}

#[test]
fn a_main_is_found_by_its_code_when_no_name_says_main() {
    let infos = vec![
        info(0, "C2", "Break", 40.0, Side::Crown),
        info(1, "C1", "First", 35.0, Side::Crown),
        info(2, "P1", "Deep", -42.0, Side::Pavilion),
    ];
    assert_eq!(tuning_order(&infos), vec![2, 1, 0]);
}

#[test]
fn the_cutters_order_comes_first_and_survives_a_changed_design() {
    let order = vec![5, 4, 3, 0, 2, 1];
    assert_eq!(apply_custom_order(&order, &[]), order);
    assert_eq!(apply_custom_order(&order, &[3, 5]), vec![3, 5, 4, 0, 2, 1]);
    // A tier the design lost is skipped; one named twice counts once.
    assert_eq!(
        apply_custom_order(&order, &[9, 3, 3]),
        vec![3, 5, 4, 0, 2, 1]
    );
}

#[test]
fn a_tier_moves_one_place_and_stops_at_the_ends() {
    let mut order = vec![5, 4, 3];
    assert!(move_tier(&mut order, 4, true));
    assert_eq!(order, vec![4, 5, 3]);
    assert!(!move_tier(&mut order, 4, true), "already first");
    assert!(move_tier(&mut order, 4, false));
    assert_eq!(order, vec![5, 4, 3]);
    assert!(!move_tier(&mut order, 3, false), "already last");
    assert!(!move_tier(&mut order, 8, true), "not in the list");
}

#[test]
fn next_tier_skips_accepted_and_untunable_tiers_and_wraps_around() {
    let order = vec![5, 4, 3, 0, 2, 1];
    let tunable = |tier: usize| tier != 1;
    let mut accepted = BTreeSet::new();
    assert_eq!(next_untuned(&order, Some(5), &tunable, &accepted), Some(4));
    accepted.insert(4);
    assert_eq!(next_untuned(&order, Some(5), &tunable, &accepted), Some(3));
    // After the last tunable tier it wraps to the first one that is left.
    assert_eq!(next_untuned(&order, Some(2), &tunable, &accepted), Some(5));
    // Nothing is picked when every other tunable tier is accepted.
    accepted.extend([3, 0, 2]);
    assert_eq!(next_untuned(&order, Some(5), &tunable, &accepted), None);
    // With no tier picked the first one that is left is.
    assert_eq!(next_untuned(&order, None, &tunable, &accepted), Some(5));
    assert_eq!(next_untuned(&[], None, &tunable, &accepted), None);
}

#[test]
fn the_baselines_angle_of_a_tier_is_found_by_place_then_by_name() {
    let baseline = round_brilliant();
    let mut design = baseline.clone();
    design.tiers[5].angle_deg = -41.5;
    let angles = baseline_angles(&baseline, &design);
    assert_eq!(angles.len(), design.tiers.len());
    assert_eq!(angles[5], Some(-41.0), "the Baseline's, not the angle now");
    // A tier added in front shifts the places; the names still find the Baseline's tiers.
    let mut shifted = baseline.clone();
    let mut extra = shifted.tiers[2].clone();
    extra.name = "Extra".to_owned();
    shifted.tiers.insert(0, extra);
    let angles = baseline_angles(&baseline, &shifted);
    assert_eq!(angles[0], None, "the Baseline has no such tier");
    assert_eq!(angles[3], Some(34.5), "Crown Main");
}

#[test]
fn rows_carry_the_change_the_accepted_mark_and_the_note_of_a_driven_tier() {
    let mut infos = sample_infos();
    infos[2].driven = true;
    infos[2].relation = Some("P1 - 2".to_owned());
    let order = tuning_order(&infos);
    let baseline = vec![None, None, None, None, Some(34.0), Some(-40.75)];
    let accepted: BTreeSet<usize> = BTreeSet::from([4]);
    let rows = tier_rows(&ListInput {
        infos: &infos,
        order: &order,
        baseline: &baseline,
        accepted: &accepted,
        selected: Some(5),
        reorderable: true,
    });
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0].tier, 5);
    assert_eq!(rows[0].change.as_str(), "40.75 \u{2192} 41.00");
    assert!(rows[0].selected && rows[0].selectable && !rows[0].can_up && rows[0].can_down);
    assert!(rows[1].accepted, "Crown Main was accepted");
    assert_eq!(rows[1].change.as_str(), "34.00 \u{2192} 34.50");
    let driven = rows
        .iter()
        .find(|row| row.tier == 2)
        .expect("the driven tier");
    assert!(!driven.selectable);
    assert_eq!(driven.note.as_str(), "follows P1 - 2");
    assert_eq!(driven.change.as_str(), "");
    let girdle = rows
        .iter()
        .find(|row| row.tier == 1)
        .expect("the girdle tier");
    assert!(!girdle.selectable && !girdle.note.is_empty() && !girdle.can_down);
    // Without the Advanced interface nothing can move.
    let plain = tier_rows(&ListInput {
        infos: &infos,
        order: &order,
        baseline: &baseline,
        accepted: &accepted,
        selected: None,
        reorderable: false,
    });
    assert!(plain.iter().all(|row| !row.can_up && !row.can_down));
}

#[test]
fn angles_and_titles_read_like_the_tier_table() {
    assert_eq!(format_angle(-41.25), "41.25\u{b0}");
    assert_eq!(change_text(Some(-40.75), -41.25), "40.75 \u{2192} 41.25");
    assert_eq!(
        change_text(Some(-41.0), -41.001),
        "",
        "under half a hundredth is no change"
    );
    assert_eq!(change_text(None, -41.0), "");
    let main = info(5, "P1", "Pavilion Main", -41.25, Side::Pavilion);
    assert_eq!(title_text(&main), "PAVILION MAIN (P1) 41.25\u{b0}");
    let bare = info(6, "P2", "P2", -42.0, Side::Pavilion);
    assert_eq!(title_text(&bare), "P2 42.00\u{b0}");
}

fn figures(brilliance_pct: f32, windowing_pct: f32) -> RowMetrics {
    RowMetrics {
        brilliance_pct,
        windowing_pct,
        extinction_pct: 5.0,
        fire_index: 1.0,
        scintillation_pct: 20.0,
        yield_pct: Some(30.0),
        warning_count: 0,
        has_girdle: true,
        tilt: None,
    }
}

fn row(angle_deg: f64, is_current: bool, metrics: Option<RowMetrics>) -> SweepRow {
    SweepRow {
        angle_deg,
        is_current,
        metrics,
        notes: Vec::new(),
    }
}

/// A pavilion sweep: brilliance 70, 76, 77, 78, 71 over 40.0 to 42.0 degrees; the row at 41.0
/// (77) is the current one and 41.5 (78) is the best.
fn sample_rows() -> Vec<SweepRow> {
    vec![
        row(-40.0, false, Some(figures(70.0, 10.0))),
        row(-40.5, false, Some(figures(76.0, 9.0))),
        row(-41.0, true, Some(figures(77.0, 8.0))),
        row(-41.5, false, Some(figures(78.0, 7.0))),
        row(-42.0, false, Some(figures(71.0, 12.0))),
    ]
}

#[test]
fn the_best_row_is_the_highest_brilliance_and_the_lowest_windowing() {
    let rows = sample_rows();
    assert_eq!(mini_sweep::best_row(&rows, 0), Some(3));
    assert_eq!(
        mini_sweep::best_row(&rows, 1),
        Some(3),
        "windowing: lower is better"
    );
    assert_eq!(mini_sweep::best_row(&rows, 9), None, "no such figure");
    let mut gaps = sample_rows();
    gaps[3].metrics = None;
    assert_eq!(mini_sweep::best_row(&gaps, 0), Some(2));
    assert_eq!(mini_sweep::best_row(&[], 0), None);
}

#[test]
fn the_plateau_is_every_row_within_the_noise_of_the_best() {
    // Brilliance noise is 2 points: 77 and 76 are within it of 78, 71 and 70 are not.
    assert_eq!(mini_sweep::plateau_rows(&sample_rows(), 0), vec![1, 2, 3]);
    assert_eq!(mini_sweep::plateau_rows(&[], 0).len(), 0);
}

#[test]
fn jump_to_best_names_an_angle_unless_the_tier_is_there() {
    let rows = sample_rows();
    assert_eq!(mini_sweep::jump_target(&rows, 0, -41.0), Some(-41.5));
    assert_eq!(mini_sweep::jump_target(&rows, 0, -41.5), None);
    // After a nudge the tier's angle is not the one the sweep marked as current.
    assert_eq!(mini_sweep::jump_target(&rows, 0, -41.4), Some(-41.5));
}

#[test]
fn the_chart_marks_land_on_the_line() {
    let rows = sample_rows();
    let marks = chart_marks(&rows, 0, -41.0);
    assert!(marks.commands.starts_with("M "));
    let (best_x, best_y) = marks.best.expect("a best angle");
    let (current_x, current_y) = marks.current.expect("the tier's angle");
    // The best angle is the highest point: the smallest y. Both are inside the plot.
    assert!(best_y < current_y);
    assert!(current_x > 2.0 && best_x > current_x && best_x < 98.0);
    // A tier angle outside the swept range has no mark.
    assert!(chart_marks(&rows, 0, -45.0).current.is_none());
    // One band for the three neighbouring rows, inside the plot.
    assert_eq!(marks.plateau.len(), 1);
    let (left, width) = marks.plateau[0];
    assert!(left >= 0.0 && left + width <= 100.0 && width > 0.0);
    assert_eq!(marks.axis_low, "40.00\u{b0}");
    assert_eq!(marks.axis_high, "42.00\u{b0}");
    assert_eq!(marks.range_text, "Brilliance 70.00 to 78.00 %");
    assert_eq!(
        chart_marks(&rows, 99, -41.0),
        super::present::ChartMarks::default()
    );
}

#[test]
fn the_readout_follows_the_pointer_then_names_the_best_angle() {
    let rows = sample_rows();
    let best = readout(&rows, 0, None, Visibility::default());
    assert!(best.starts_with("Best (Brilliance): 41.50\u{b0}"), "{best}");
    let hovered = readout(&rows, 0, Some(2), Visibility::default());
    assert!(hovered.starts_with("41.00\u{b0} (current)"), "{hovered}");
    assert_eq!(readout(&[], 0, None, Visibility::default()), "");
    assert_eq!(readout(&rows, 99, None, Visibility::default()), "");
}

#[test]
fn visibility_changes_curves_and_values_without_changing_the_objectives_best_angle() {
    let rows = sample_rows();
    let before = chart_marks(&rows, 0, -41.0);
    let mut visible = Visibility::default();
    assert!(visible.toggle(0));
    assert!(visible.toggle(1));
    let curves = metrics::rows(&rows, visible);
    assert!(!curves[0].shown);
    assert!(!curves[1].shown);
    assert!(curves[2].shown);
    assert_eq!(curves[0].commands.as_str(), before.commands);
    let text = readout(&rows, 0, None, visible);
    assert!(text.starts_with("Best (Brilliance): 41.50°"), "{text}");
    assert!(text.contains("Extinction 5.00 %"));
    assert!(!text.contains("Brilliance 78.00"));
    assert!(!text.contains("Windowing"));
    assert_eq!(chart_marks(&rows, 0, -41.0), before);
    assert_eq!(mini_sweep::jump_target(&rows, 0, -41.0), Some(-41.5));
}

#[test]
fn multi_metric_legend_keeps_real_units_flat_curves_and_invalid_gaps() {
    let mut measured = sample_rows();
    measured[2].metrics = None;
    let curves = metrics::rows(&measured, Visibility::default());
    assert_eq!(curves.len(), FIGURES.len());
    assert!(curves.iter().all(|curve| curve.shown));
    assert_eq!(curves[0].range_text.as_str(), "Brilliance 70.00 to 78.00 %");
    assert_eq!(curves[3].range_text.as_str(), "Fire 1.00 to 1.00");
    assert_eq!(
        curves[4].range_text.as_str(),
        "Scintillation 20.00 to 20.00 %"
    );
    for curve in &curves {
        assert_eq!(curve.commands.matches('M').count(), 2, "{}", curve.label);
    }
    assert!(
        curves[3].commands.contains("50.00"),
        "a flat metric stays centered"
    );
    assert!(
        metrics::rows(&[], Visibility::default())
            .iter()
            .all(|curve| curve.commands.is_empty())
    );
}

#[test]
fn all_metrics_can_be_hidden_and_reenabled_and_invalid_callbacks_are_ignored() {
    let mut visible = Visibility::default();
    assert!(!visible.toggle(-1));
    assert!(!visible.toggle(i32::MAX));
    for index in 0..i32::try_from(FIGURES.len()).unwrap() {
        assert!(visible.toggle(index));
    }
    assert!(visible.selected().is_empty());
    assert!(
        metrics::rows(&sample_rows(), visible)
            .iter()
            .all(|curve| !curve.shown)
    );
    assert!(visible.toggle(3));
    assert_eq!(
        visible.selected(),
        vec![pleochroma_editor::sweep::SweepMetric::Fire]
    );
}

#[test]
fn a_sweep_is_a_sweep_of_the_design_without_the_tiers_own_angle() {
    let design = round_brilliant();
    let basis = Basis::of(&design, 3, 5);
    // A nudge of the swept tier itself keeps the basis.
    let mut nudged = design.clone();
    nudged.tiers[5].angle_deg = -41.1;
    assert_eq!(Basis::of(&nudged, 3, 5), basis);
    // Another tier's angle, another design or another tier is another basis.
    let mut other = design.clone();
    other.tiers[2].angle_deg = 35.0;
    assert_ne!(Basis::of(&other, 3, 5), basis);
    assert_ne!(Basis::of(&design, 4, 5), basis);
    assert_ne!(Basis::of(&design, 3, 2), basis);
}

#[test]
fn the_mini_sweep_plans_two_degrees_each_side_in_quarter_degree_steps() {
    let design = round_brilliant();
    let plan = mini_sweep::plan(&design, 5).expect("the pavilion main can be swept");
    assert_eq!(plan.tier, 5);
    assert_eq!(plan.angles.len(), 17);
    assert!(
        (plan.angles[0] + 39.0).abs() < 1e-9,
        "the flattest is 39 degrees"
    );
    assert!((plan.angles[16] + 43.0).abs() < 1e-9);
    assert!((plan.angles[plan.current_row] + 41.0).abs() < 1e-9);
    // The girdle and the table cannot be swept.
    assert!(mini_sweep::plan(&design, 4).is_err());
    assert!(mini_sweep::plan(&design, 0).is_err());
}

#[test]
fn the_cache_keeps_a_sweep_per_basis_and_drops_the_oldest() {
    let design = round_brilliant();
    let outcome = |tier: usize| SweepOutcome {
        tier,
        tier_name: "x".to_owned(),
        current_deg: -41.0,
        requested: 5,
        cancelled: false,
        tilt_average: false,
        rows: sample_rows(),
    };
    let mut cache = Cache::default();
    let first = Basis::of(&design, 1, 5);
    cache.put(first.clone(), outcome(5));
    assert!(cache.get(&first).is_some());
    for epoch in 2..20 {
        cache.put(Basis::of(&design, epoch, 5), outcome(5));
    }
    assert!(cache.get(&first).is_none(), "the oldest is dropped");
    assert!(cache.get(&Basis::of(&design, 19, 5)).is_some());
    cache.clear();
    assert!(cache.get(&Basis::of(&design, 19, 5)).is_none());
}

#[test]
fn the_figure_list_matches_the_strips_figures_and_the_simple_interface_says_when_it_hides_a_setting()
 {
    assert_eq!(FIGURES.len(), 5);
    assert_eq!(super::present::figure_names().len(), FIGURES.len());
    assert!(!simple_mode::settings_in_use(
        false,
        simple_mode::DEFAULT_FIGURE
    ));
    assert!(simple_mode::settings_in_use(
        true,
        simple_mode::DEFAULT_FIGURE
    ));
    assert!(simple_mode::settings_in_use(false, 2));
}
