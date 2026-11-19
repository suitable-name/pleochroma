//! `metrics --ri-range`: the traced range of refractive indices as text, JSON or CSV.
//!
//! The range is `pleochroma_editor::ri_range::compute` on the stone `metrics` measures (concave
//! tools and frosted facets included). The report carries every row of the grid (the 0.05 steps
//! from 1.40 to 2.70 and the bisection points, ascending), the row at the design's own index and
//! both bounds, so the owner can calibrate the thresholds against known designs. The high bound
//! is marked "uncalibrated" while `HIGH_BOUND_CALIBRATED` is false: it is printed, but a sheet or
//! a chip does not state it.

use crate::{
    format::{
        Align, INDEX_DECIMALS, PERCENT_DECIMALS, csv_line, fixed, fixed_or_na, json_number,
        json_number32, json_optional, table,
    },
    materials::Scoring,
    outcome::CliError,
};
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{StoneGeometry, meet_solver::SolvedTier},
};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    ri_range::{self, HIGH_BOUND_CALIBRATED, RiGridRow, RiRange, SMALL_TILT_DEG, range_text},
    solve_policy::design_to_gpu_planes_from_solved,
};
use serde_json::{Value, json};
use std::{fmt::Write as _, sync::atomic::AtomicBool};

/// The columns of the CSV report, in order.
pub(super) const CSV_COLUMNS: [&str; 13] = [
    "design",
    "material",
    "refractive_index",
    "lighting",
    "ri_low",
    "ri_high",
    "high_bound_calibrated",
    "row",
    "n",
    "windowing_table_up_pct",
    "windowing_tilt_pct",
    "brilliance_tilt_pct",
    "extinction_tilt_pct",
];

/// Traces the range of refractive indices of the stone of `design`, whose solve is `solved`.
///
/// # Errors
///
/// [`CliError`] when the range cannot be traced (the trace only stops when it is cancelled, which
/// the command never does, so this is a safety net).
pub(super) fn trace(
    design: &Design,
    solved: &[SolvedTier],
    scoring: &Scoring,
) -> Result<RiRange, CliError> {
    let planes_gpu = design_to_gpu_planes_from_solved(design, solved);
    let planes = design.planes_from_solved(solved);
    let tools = design.tools_for_scoring(&planes);
    let finishes = design.facet_finishes_from_solved(solved, &planes_gpu);
    let stone = FinishedStone::new(
        StoneGeometry {
            planes: &planes_gpu,
            tools: &tools,
        },
        &finishes,
    );
    let cancel = AtomicBool::new(false);
    ri_range::compute(stone, scoring.n_d(), &cancel)
        .ok_or_else(|| CliError::design("the range of refractive indices could not be traced"))
}

/// The high bound as the text report prints it, marked while it is not calibrated.
fn high_bound_text(range: &RiRange) -> String {
    let high = fixed_or_na(range.n_high, INDEX_DECIMALS);
    if HIGH_BOUND_CALIBRATED {
        high
    } else {
        format!("{high} (uncalibrated)")
    }
}

/// The five figures of one row of the grid as text.
fn row_cells(row: &RiGridRow) -> Vec<String> {
    let percent = |value: f32| fixed(f64::from(value), PERCENT_DECIMALS);
    vec![
        fixed(row.n, INDEX_DECIMALS),
        percent(row.windowing_table_up),
        percent(row.windowing_tilt),
        percent(row.brilliance_tilt),
        percent(row.extinction_tilt),
    ]
}

/// The text block of the range: both bounds, the grid table and the design's own row.
pub(super) fn text(range: &RiRange) -> String {
    let tilt = format!("{SMALL_TILT_DEG} deg");
    let stated = range
        .statement()
        .map_or_else(|| "none".to_string(), |statement| range_text(&statement));
    let mut text = format!("RI range (traced; the tilted figures are the mean at {tilt}):\n");
    let pairs = [
        ("Low bound", fixed_or_na(range.n_low, INDEX_DECIMALS)),
        ("High bound", high_bound_text(range)),
        ("Stated as", stated),
        ("Design's own index", fixed(range.n0, INDEX_DECIMALS)),
        ("Threshold version", range.threshold_version.to_string()),
    ];
    let width = pairs
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in &pairs {
        let pad = width.saturating_sub(label.chars().count());
        let _ = writeln!(text, "  {label}:{}  {value}", " ".repeat(pad));
    }
    text.push('\n');
    let headers = [
        "n".to_string(),
        "Windowing table up %".to_string(),
        format!("Windowing {tilt} %"),
        format!("Brilliance {tilt} %"),
        format!("Extinction {tilt} %"),
    ];
    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    let rows: Vec<Vec<String>> = range.rows.iter().map(row_cells).collect();
    text.push_str(&table(&header_refs, &[Align::Right; 5], &rows));
    let own = row_cells(&range.own);
    let _ = write!(
        text,
        "\nAt the design's own index {}: windowing table up {} %, windowing {tilt} {} %, brilliance {tilt} {} %, extinction {tilt} {} %\n",
        own[0], own[1], own[2], own[3], own[4]
    );
    text
}

/// One row of the grid as JSON.
fn row_json(row: &RiGridRow) -> Value {
    let decimals = i32::try_from(PERCENT_DECIMALS).unwrap_or(2);
    json!({
        "n": json_number(row.n, i32::try_from(INDEX_DECIMALS).unwrap_or(4)),
        "windowing_table_up_pct": json_number32(row.windowing_table_up, decimals),
        "windowing_tilt_pct": json_number32(row.windowing_tilt, decimals),
        "brilliance_tilt_pct": json_number32(row.brilliance_tilt, decimals),
        "extinction_tilt_pct": json_number32(row.extinction_tilt, decimals),
    })
}

/// The range as the `ri_range` object of the JSON report.
pub(super) fn json(range: &RiRange) -> Value {
    let index_decimals = i32::try_from(INDEX_DECIMALS).unwrap_or(4);
    json!({
        "threshold_version": range.threshold_version,
        "refractive_index": json_number(range.n0, index_decimals),
        "tilt_deg": json_number32(SMALL_TILT_DEG, 1),
        "low": json_optional(range.n_low, index_decimals),
        "high": json_optional(range.n_high, index_decimals),
        "high_bound_calibrated": HIGH_BOUND_CALIBRATED,
        "stated": range.statement().map(|statement| range_text(&statement)),
        "own": row_json(&range.own),
        "rows": range.rows.iter().map(row_json).collect::<Vec<_>>(),
    })
}

/// The CSV report of the range: a header line, then the design's own row (`own`) and one line per
/// row of the grid (`grid`). The bounds repeat on every line, so each line stands alone.
pub(super) fn csv(name: &str, material: &str, lighting: &str, range: &RiRange) -> String {
    let mut text = csv_line(&CSV_COLUMNS);
    let low = range
        .n_low
        .map_or_else(String::new, |n| fixed(n, INDEX_DECIMALS));
    let high = range
        .n_high
        .map_or_else(String::new, |n| fixed(n, INDEX_DECIMALS));
    let lines =
        std::iter::once(("own", &range.own)).chain(range.rows.iter().map(|row| ("grid", row)));
    for (kind, row) in lines {
        let mut cells = vec![
            name.to_string(),
            material.to_string(),
            fixed(range.n0, INDEX_DECIMALS),
            lighting.to_string(),
            low.clone(),
            high.clone(),
            HIGH_BOUND_CALIBRATED.to_string(),
            kind.to_string(),
        ];
        cells.extend(row_cells(row));
        text.push_str(&csv_line(&cells));
    }
    text
}
