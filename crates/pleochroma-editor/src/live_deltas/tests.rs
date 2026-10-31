use super::*;
use crate::{
    metric_deltas::{BRILLIANCE_NOISE_POINTS, TILT_NOISE_POINTS},
    sweep::TiltAverages,
};

fn metrics() -> RowMetrics {
    RowMetrics {
        brilliance_pct: 20.0,
        windowing_pct: 60.0,
        extinction_pct: 20.0,
        fire_index: 10.0,
        scintillation_pct: 30.0,
        yield_pct: Some(40.0),
        warning_count: 0,
        has_girdle: true,
        tilt: None,
    }
}

fn tilt(brilliance: f32) -> TiltAverages {
    TiltAverages {
        brilliance_pct: brilliance,
        windowing_pct: 50.0,
        extinction_pct: 20.0,
    }
}

fn row(rows: &[FigureDelta], figure: DeltaFigure) -> Option<&FigureDelta> {
    rows.iter().find(|row| row.figure == figure)
}

fn tone(rows: &[FigureDelta], figure: DeltaFigure) -> DeltaTone {
    row(rows, figure).expect("the figure has a row").tone
}

#[test]
fn identical_metrics_are_all_the_same() {
    let rows = figure_deltas(&metrics(), &metrics());
    assert_eq!(rows.len(), 7);
    assert!(rows.iter().all(|row| row.tone == DeltaTone::Same));
    assert!(rows.iter().all(|row| row.change == 0.0));
}

#[test]
fn a_change_under_each_threshold_is_the_same() {
    let mut now = metrics();
    now.brilliance_pct += BRILLIANCE_NOISE_POINTS - 0.5;
    now.windowing_pct -= 1.5;
    now.extinction_pct += 1.5;
    now.scintillation_pct += 2.5;
    now.fire_index += 0.4;
    now.yield_pct = Some(YIELD_NOISE_POINTS.mul_add(0.5, 40.0));
    let rows = figure_deltas(&metrics(), &now);
    for figure in [
        DeltaFigure::Brilliance,
        DeltaFigure::Windowing,
        DeltaFigure::Extinction,
        DeltaFigure::Fire,
        DeltaFigure::Scintillation,
        DeltaFigure::Yield,
    ] {
        assert_eq!(tone(&rows, figure), DeltaTone::Same, "{figure:?}");
    }
    let brilliance = row(&rows, DeltaFigure::Brilliance).expect("row");
    assert_eq!(
        brilliance.tooltip,
        "Within the noise threshold, about the same."
    );
}

#[test]
fn a_change_over_the_threshold_goes_the_right_way_per_figure() {
    let mut now = metrics();
    now.brilliance_pct += 3.0; // higher is better
    now.windowing_pct += 3.0; // lower is better
    now.extinction_pct -= 3.0; // lower is better
    now.fire_index += 2.0; // higher is better
    now.scintillation_pct -= 4.0; // higher is better
    now.yield_pct = Some(41.0); // higher is better
    let rows = figure_deltas(&metrics(), &now);
    assert_eq!(tone(&rows, DeltaFigure::Brilliance), DeltaTone::Better);
    assert_eq!(tone(&rows, DeltaFigure::Windowing), DeltaTone::Worse);
    assert_eq!(tone(&rows, DeltaFigure::Extinction), DeltaTone::Better);
    assert_eq!(tone(&rows, DeltaFigure::Fire), DeltaTone::Better);
    assert_eq!(tone(&rows, DeltaFigure::Scintillation), DeltaTone::Worse);
    assert_eq!(tone(&rows, DeltaFigure::Yield), DeltaTone::Better);
    let mut worse_yield = metrics();
    worse_yield.yield_pct = Some(39.0);
    let rows = figure_deltas(&metrics(), &worse_yield);
    assert_eq!(tone(&rows, DeltaFigure::Yield), DeltaTone::Worse);
}

#[test]
fn the_warning_count_is_always_reported_and_any_change_counts() {
    let rows = figure_deltas(&metrics(), &metrics());
    let same = row(&rows, DeltaFigure::Warnings).expect("warnings row");
    assert_eq!(same.tone, DeltaTone::Same);

    let mut more = metrics();
    more.warning_count = 1;
    let rows = figure_deltas(&metrics(), &more);
    assert_eq!(tone(&rows, DeltaFigure::Warnings), DeltaTone::Worse);
    assert_eq!(row(&rows, DeltaFigure::Warnings).expect("row").text, "1 +1");

    let rows = figure_deltas(&more, &metrics());
    assert_eq!(tone(&rows, DeltaFigure::Warnings), DeltaTone::Better);
    assert_eq!(
        row(&rows, DeltaFigure::Warnings).expect("row").text,
        "0 \u{2212}1"
    );
}

#[test]
fn tilt_rows_need_both_sides_and_use_the_tilt_threshold() {
    let mut with_tilt = metrics();
    with_tilt.tilt = Some(tilt(30.0));
    // One side only: no tilt rows.
    let rows = figure_deltas(&metrics(), &with_tilt);
    assert!(rows.iter().all(|row| !row.figure.is_tilt()));
    let rows = figure_deltas(&with_tilt, &metrics());
    assert!(rows.iter().all(|row| !row.figure.is_tilt()));

    // Under the tilt threshold: the same; over it: better.
    let mut under = metrics();
    under.tilt = Some(tilt(30.0 + TILT_NOISE_POINTS - 0.5));
    let rows = figure_deltas(&with_tilt, &under);
    assert_eq!(rows.len(), 10);
    assert_eq!(tone(&rows, DeltaFigure::TiltBrilliance), DeltaTone::Same);
    let mut over = metrics();
    over.tilt = Some(tilt(30.0 + TILT_NOISE_POINTS + 0.5));
    let rows = figure_deltas(&with_tilt, &over);
    assert_eq!(tone(&rows, DeltaFigure::TiltBrilliance), DeltaTone::Better);
}

#[test]
fn a_missing_yield_leaves_the_yield_row_out() {
    let mut none = metrics();
    none.yield_pct = None;
    let rows = figure_deltas(&none, &metrics());
    assert!(row(&rows, DeltaFigure::Yield).is_none());
    let rows = figure_deltas(&metrics(), &none);
    assert!(row(&rows, DeltaFigure::Yield).is_none());
    assert_eq!(rows.len(), 6);
}

#[test]
fn the_text_carries_the_value_and_the_signed_change() {
    let mut now = metrics();
    now.brilliance_pct = 21.8;
    now.windowing_pct = 57.0;
    now.fire_index = 10.2;
    let rows = figure_deltas(&metrics(), &now);
    assert_eq!(
        row(&rows, DeltaFigure::Brilliance).expect("row").text,
        "21.8 % +1.8"
    );
    assert_eq!(
        row(&rows, DeltaFigure::Windowing).expect("row").text,
        "57.0 % \u{2212}3.0"
    );
    assert_eq!(
        row(&rows, DeltaFigure::Fire).expect("row").text,
        "10.2 +0.2"
    );
    let windowing = row(&rows, DeltaFigure::Windowing).expect("row");
    assert_eq!(windowing.tooltip, "Windowing: Baseline 60.0 %, now 57.0 %.");
}

#[test]
fn a_zero_change_carries_no_sign() {
    assert_eq!(signed(0.0, 1), "0.0");
    assert_eq!(signed(-0.01, 1), "0.0");
    assert_eq!(signed(1.84, 1), "+1.8");
    assert_eq!(signed(-3.0, 1), "\u{2212}3.0");
}
