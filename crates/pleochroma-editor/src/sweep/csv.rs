//! A sweep as CSV text, for a spreadsheet.
//!
//! One header row and one row per finished angle. The angle column is the magnitude, like
//! the table the person saw: the tier's side of the girdle is not in the number. The
//! decimal separator is always a point, numbers carry four decimals, a missing figure is an
//! empty field, and a field holding a comma, a quote or a line break is quoted with its
//! quotes doubled.
//!
//! Contrast and the six Zones (ASET-style) shares are written after `notes`, so every
//! other column keeps its position.
//! Lines end with CRLF as RFC 4180 has it; spreadsheets read that and LF alike.

use super::{RowMetrics, SweepMetric, SweepOutcome, SweepRow};

use pleochroma::color::metrics::AsetAreaShares;

/// The line ending of the file.
pub(super) const LINE_END: &str = "\r\n";

/// `field` as one CSV field: quoted only when it has to be.
pub(super) fn escape(field: &str) -> String {
    if field.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// A figure with four decimals, or an empty field.
pub(super) fn number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format!("{value:.4}"))
}

/// `yes` or `no`.
pub(super) fn yes_no(flag: bool) -> String {
    (if flag { "yes" } else { "no" }).to_owned()
}

/// The columns written after `notes`: Contrast and the six Zones (ASET-style) shares.
pub(super) const APPENDED_COLUMNS: [&str; 7] = [
    "contrast",
    "aset_red_pct",
    "aset_green_pct",
    "aset_blue_pct",
    "aset_dark_pct",
    "aset_leak_pct",
    "aset_lost_pct",
];

/// The figures that take their own column in the metric loop: every figure that is not one of the
/// appended columns, without the tilt averages unless `tilt_average`.
pub(super) fn loop_metrics(tilt_average: bool) -> Vec<SweepMetric> {
    SweepMetric::ALL
        .into_iter()
        .filter(|metric| *metric != SweepMetric::Contrast && (tilt_average || !metric.is_tilt()))
        .collect()
}

/// The fields of the appended columns of one row, empty where the figures are missing.
pub(super) fn appended_fields(metrics: Option<&RowMetrics>) -> Vec<String> {
    let aset = metrics.and_then(|figures| figures.aset);
    let share = |pick: fn(&AsetAreaShares) -> f32| {
        number(aset.as_ref().map(|shares| f64::from(pick(shares))))
    };
    vec![
        number(SweepMetric::Contrast.value_of(metrics)),
        share(|shares| shares.red_pct),
        share(|shares| shares.green_pct),
        share(|shares| shares.blue_pct),
        share(AsetAreaShares::dark_pct),
        share(|shares| shares.leak_pct),
        share(|shares| shares.lost_pct),
    ]
}

/// The fields of one row, in the order of the header.
fn row_fields(outcome: &SweepOutcome, row: &SweepRow, metrics: &[SweepMetric]) -> Vec<String> {
    let mut fields = vec![
        escape(&outcome.tier_name),
        format!("{:.4}", row.angle_deg.abs()),
        yes_no(row.is_current),
        yes_no(row.is_valid()),
    ];
    fields.extend(metrics.iter().map(|metric| number(metric.value(row))));
    fields.push(
        row.metrics
            .map_or_else(String::new, |figures| figures.warning_count.to_string()),
    );
    fields.push(escape(&row.notes_text()));
    fields.extend(appended_fields(row.metrics.as_ref()));
    fields
}

/// The rows of `outcome` as CSV text: a header, then one line per finished angle in
/// ascending angle order. The tilt-average columns are there only when the sweep asked
/// for them.
#[must_use]
pub fn sweep_csv(outcome: &SweepOutcome) -> String {
    let metrics = loop_metrics(outcome.tilt_average);
    let mut header: Vec<&str> = vec!["tier", "angle_deg", "current", "valid"];
    header.extend(metrics.iter().map(|metric| metric.csv_name()));
    header.push("facet_warnings");
    header.push("notes");
    header.extend(APPENDED_COLUMNS);
    let mut text = header.join(",");
    text.push_str(LINE_END);
    for row in &outcome.rows {
        text.push_str(&row_fields(outcome, row, &metrics).join(","));
        text.push_str(LINE_END);
    }
    text
}
