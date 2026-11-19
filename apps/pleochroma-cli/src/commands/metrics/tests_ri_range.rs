//! `metrics --ri-range`: the table and both bounds as text, JSON and CSV.
//!
//! The report functions are checked on a small synthetic range (exact text, and the "uncalibrated"
//! mark on the high bound while `HIGH_BOUND_CALIBRATED` is false); one end-to-end run per format
//! traces a real stone and checks the shape of what the owner's calibration reads.

use super::*;
use crate::{outcome::EXIT_OK, testing};
use pleochroma_editor::ri_range::{
    HIGH_BOUND_CALIBRATED, RiGridRow, THRESHOLD_VERSION, range_text,
};

fn row(n: f64, windowing: f32) -> RiGridRow {
    RiGridRow {
        n,
        windowing_table_up: windowing,
        windowing_tilt: windowing + 1.0,
        brilliance_tilt: 80.0,
        extinction_tilt: 5.0,
    }
}

fn synthetic(high: Option<f64>) -> RiRange {
    RiRange {
        n0: 1.76,
        own: row(1.76, 9.0),
        rows: vec![row(1.60, 30.0), row(1.65, 12.5), row(2.30, 4.0)],
        n_low: Some(1.63),
        n_high: high,
        threshold_version: THRESHOLD_VERSION,
    }
}

#[test]
fn the_text_block_prints_both_bounds_and_marks_the_high_one_while_uncalibrated() {
    let range = synthetic(Some(2.31));
    let text = ri_report::text(&range);
    let line = |label: &str| {
        text.lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no {label} line: {text}"))
    };
    assert!(line("Low bound:").ends_with("1.6300"), "{text}");
    let high = if HIGH_BOUND_CALIBRATED {
        "2.3100"
    } else {
        "2.3100 (uncalibrated)"
    };
    assert!(line("High bound:").ends_with(high), "{text}");
    let stated = range.statement().map(|statement| range_text(&statement));
    assert!(
        line("Stated as:").ends_with(stated.as_deref().unwrap_or("none")),
        "{text}"
    );
    assert!(line("Design's own index:").ends_with("1.7600"), "{text}");
    for header in [
        "Windowing table up %",
        "Windowing 10 deg %",
        "Brilliance 10 deg %",
        "Extinction 10 deg %",
    ] {
        assert!(text.contains(header), "{header}: {text}");
    }
    for cell in [
        "1.6000", "30.00", "31.00", "1.6500", "12.50", "2.3000", "4.00",
    ] {
        assert!(text.contains(cell), "{cell}: {text}");
    }
    assert!(text.contains("At the design's own index 1.7600: windowing table up 9.00 %"));
    assert!(text.lines().all(|line| line == line.trim_end()), "{text}");
}

#[test]
fn a_range_without_a_high_bound_reads_n_a() {
    let text = ri_report::text(&synthetic(None));
    let high = text
        .lines()
        .find(|line| line.trim_start().starts_with("High bound:"))
        .expect("a high bound line");
    assert!(high.contains("n/a"), "{high}");
}

#[test]
fn the_json_object_carries_the_rows_the_bounds_and_the_calibration_state() {
    let value = ri_report::json(&synthetic(Some(2.31)));
    assert_eq!(value["rows"].as_array().map(Vec::len), Some(3));
    assert_eq!(value["own"]["n"], 1.76);
    assert_eq!(value["low"], 1.63);
    assert_eq!(value["high"], 2.31);
    assert_eq!(value["high_bound_calibrated"], HIGH_BOUND_CALIBRATED);
    assert_eq!(
        value["threshold_version"].as_u64(),
        Some(u64::from(THRESHOLD_VERSION))
    );
    for key in [
        "n",
        "windowing_table_up_pct",
        "windowing_tilt_pct",
        "brilliance_tilt_pct",
        "extinction_tilt_pct",
    ] {
        assert!(value["rows"][0][key].is_number(), "{key}");
        assert!(value["own"][key].is_number(), "{key}");
    }
    assert_eq!(ri_report::json(&synthetic(None))["high"], Value::Null);
}

#[test]
fn the_csv_has_the_own_row_first_then_one_line_per_step_with_the_bounds_on_each() {
    let text = ri_report::csv(
        "Round",
        "refractive index 1.7600",
        "grading",
        &synthetic(None),
    );
    let lines: Vec<Vec<&str>> = text.lines().map(|line| line.split(',').collect()).collect();
    assert_eq!(lines.len(), 5, "{text}");
    assert_eq!(lines[0], ri_report::CSV_COLUMNS, "{text}");
    assert!(
        lines
            .iter()
            .all(|line| line.len() == ri_report::CSV_COLUMNS.len())
    );
    assert_eq!(lines[1][7], "own");
    assert!(lines[2..].iter().all(|line| line[7] == "grid"));
    assert!(
        lines[1..]
            .iter()
            .all(|line| line[4] == "1.6300" && line[5].is_empty())
    );
    assert!(
        lines[1..]
            .iter()
            .all(|line| line[6] == HIGH_BOUND_CALIBRATED.to_string())
    );
    assert_eq!(lines[2][8], "1.6000");
    assert_eq!(lines[1][9], "9.00");
}

fn run_ri_range(extra: &str) -> crate::outcome::Outcome {
    let input = testing::write_design("metrics-e2e-ri-range.psdd", &testing::template());
    let mut argv = vec!["metrics", &input, "--ri", "1.76", "--ri-range"];
    if !extra.is_empty() {
        argv.push(extra);
    }
    testing::run(&argv)
}

#[test]
fn end_to_end_the_text_report_adds_the_ri_range_block_after_the_figures() {
    let outcome = run_ri_range("");
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let text = &outcome.stdout;
    let block = text.find("\nRI range (traced").expect("the range block");
    assert!(
        block > text.find("\nStone:\n").expect("the stone group"),
        "{text}"
    );
    assert!(text.contains("  Low bound:"), "{text}");
    assert!(text.contains("  High bound:"), "{text}");
    assert_eq!(
        text.contains("(uncalibrated)"),
        !HIGH_BOUND_CALIBRATED,
        "{text}"
    );
    assert!(text.contains("Windowing table up %"), "{text}");
}

#[test]
fn end_to_end_the_json_report_adds_an_ri_range_object_with_ascending_rows() {
    let outcome = run_ri_range("--json");
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    assert!(value["figures"]["table_up_windowing_pct"].is_number());
    let range = &value["ri_range"];
    assert_eq!(range["own"]["n"], 1.76);
    assert_eq!(range["high_bound_calibrated"], HIGH_BOUND_CALIBRATED);
    let rows = range["rows"].as_array().expect("rows");
    assert!(rows.len() >= 27, "{}", rows.len());
    let ns: Vec<f64> = rows.iter().filter_map(|row| row["n"].as_f64()).collect();
    assert_eq!(ns.len(), rows.len());
    assert!(ns.windows(2).all(|pair| pair[0] < pair[1]), "{ns:?}");
    if let (Some(low), Some(high)) = (range["low"].as_f64(), range["high"].as_f64()) {
        assert!(low <= high, "{low} {high}");
    }
    let plain = testing::run(&[
        "metrics",
        &testing::write_design("metrics-e2e-no-range.psdd", &testing::template()),
        "--ri",
        "1.76",
        "--json",
    ]);
    let plain: Value = serde_json::from_str(&plain.stdout).expect("valid JSON");
    assert!(plain.get("ri_range").is_none());
}

#[test]
fn end_to_end_the_csv_report_is_the_range_table() {
    let outcome = run_ri_range("--csv");
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let lines: Vec<Vec<&str>> = outcome
        .stdout
        .lines()
        .map(|line| line.split(',').collect())
        .collect();
    assert_eq!(lines[0], ri_report::CSV_COLUMNS, "{}", outcome.stdout);
    assert!(lines.len() >= 2 + 27, "{}", lines.len());
    assert!(lines.iter().all(|line| line.len() == lines[0].len()));
    assert_eq!(lines[1][7], "own");
    let grid: Vec<f64> = lines[2..]
        .iter()
        .map(|line| line[8].parse().expect("an index"))
        .collect();
    assert!(grid.windows(2).all(|pair| pair[0] < pair[1]), "{grid:?}");
}
