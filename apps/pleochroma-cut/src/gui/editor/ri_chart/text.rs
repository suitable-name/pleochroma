//! The words of the RI chart: the lead sentence, the labels on the plot, the summary under it,
//! the "Works in" heading, the "Outside" line and the status line. Slint-free.

use super::layout::{BuiltIn, RI_MAX, RI_MIN, join_names, ri_text};
use pleochroma_editor::ri_range::{
    RiGridRow, RiRange, SMALL_TILT_DEG, THRESHOLD_VERSION, WINDOWING_LIMIT_PCT,
};

/// How many names the "Outside" line spells out before it says "and N more".
const OUTSIDE_NAME_CAP: usize = 6;

/// The sentence over the chart.
pub(super) fn intro() -> String {
    format!(
        "This design at its own angles, in a flat, colorless material of each RI: table up, and \
         tilted by {SMALL_TILT_DEG:.0}\u{b0} on four axes. Pleochroma estimate."
    )
}

/// The label of the low end of the range.
pub(super) fn low_label(low: f64) -> String {
    format!("Works from {low:.2}")
}

/// The label of the high end: "to 2.31", and "(uncalibrated)" while the high end is not
/// calibrated.
pub(super) fn high_label(high: f64, calibrated: bool) -> String {
    if calibrated {
        format!("to {high:.2}")
    } else {
        format!("to {high:.2} (uncalibrated)")
    }
}

/// The label on the design's own index: "Current: Sapphire", or "Current" for a design that
/// names no material.
pub(super) fn current_label(material: &str) -> String {
    if material.is_empty() {
        "Current".to_owned()
    } else {
        format!("Current: {material}")
    }
}

/// The label of the windowing limit line.
pub(super) fn limit_label() -> String {
    format!("Windowing limit {WINDOWING_LIMIT_PCT:.0} %")
}

/// What holds at the lowest index that works, read off the grid row nearest to it.
pub(super) fn summary(low: f64, row: &RiGridRow) -> String {
    format!(
        "RI {low:.2}, the lowest that works: windowing {:.0} % tilted by {SMALL_TILT_DEG:.0}\u{b0} \
         (limit {WINDOWING_LIMIT_PCT:.0} %), {:.0} % table up; brilliance {:.0} %, extinction {:.0} \
         % tilted by {SMALL_TILT_DEG:.0}\u{b0}.",
        row.windowing_tilt, row.windowing_table_up, row.brilliance_tilt, row.extinction_tilt,
    )
}

/// The heading over the chips: "Works in (Pleochroma estimate, RI 1.63 and up):".
pub(super) fn works_heading(statement_text: &str) -> String {
    format!("Works in (Pleochroma estimate, RI {statement_text}):")
}

/// One chip: "Sapphire 1.768".
pub(super) fn chip(material: &BuiltIn) -> String {
    format!("{} {}", material.name, ri_text(material.n))
}

/// The line for a list of materials that all work, shown when none does.
pub(super) fn none_work() -> String {
    "No built-in material works at this design's angles.".to_owned()
}

/// The hover of "and N more": the chips that did not fit.
pub(super) fn more_hint(rest: &[String]) -> String {
    format!("Also works in: {}.", rest.join(", "))
}

/// The "Outside" line: the materials below the low end (they window) and, when the high end is
/// calibrated, those above it (they lose brilliance).
pub(super) fn outside(
    below: &[String],
    above: &[String],
    range: &RiRange,
    calibrated: bool,
) -> String {
    let mut clauses = Vec::new();
    if let (false, Some(low)) = (below.is_empty(), range.n_low) {
        clauses.push(format!(
            "{} window below {low:.2}",
            join_names(below, OUTSIDE_NAME_CAP)
        ));
    }
    if let (true, false, Some(high)) = (calibrated, above.is_empty(), range.n_high) {
        clauses.push(format!(
            "{} lose brilliance above {high:.2}",
            join_names(above, OUTSIDE_NAME_CAP)
        ));
    }
    if clauses.is_empty() {
        "Outside: none of the built-in materials.".to_owned()
    } else {
        format!("Outside: {}.", clauses.join("; "))
    }
}

/// The footer line: how the chart was made.
pub(super) fn status(steps: usize, calibrated: bool) -> String {
    let mut text = format!(
        "{steps} steps from {RI_MIN:.2} to {RI_MAX:.2}, both ends to 0.01 \u{b7} thresholds v{THRESHOLD_VERSION}"
    );
    if !calibrated {
        text.push_str(" \u{b7} high end not calibrated");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(n_low: Option<f64>, n_high: Option<f64>) -> RiRange {
        let row = RiGridRow {
            n: 1.768,
            windowing_table_up: 6.0,
            windowing_tilt: 9.0,
            brilliance_tilt: 47.0,
            extinction_tilt: 15.0,
        };
        RiRange {
            n0: 1.768,
            own: row,
            rows: vec![row],
            n_low,
            n_high,
            threshold_version: THRESHOLD_VERSION,
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn the_small_tilt_is_ten_degrees() {
        // The legend in `ri_chart_plot.slint` spells the tilt out.
        assert!((SMALL_TILT_DEG - 10.0).abs() < f32::EPSILON);
        assert!(intro().contains("tilted by 10\u{b0}"));
        assert!(intro().ends_with("Pleochroma estimate."));
        assert!((WINDOWING_LIMIT_PCT - 15.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_labels_on_the_plot() {
        assert_eq!(low_label(1.63), "Works from 1.63");
        assert_eq!(high_label(2.31, true), "to 2.31");
        assert_eq!(high_label(2.31, false), "to 2.31 (uncalibrated)");
        assert_eq!(current_label("Sapphire"), "Current: Sapphire");
        assert_eq!(current_label(""), "Current");
        assert_eq!(limit_label(), "Windowing limit 15 %");
    }

    #[test]
    fn the_summary_reads_the_figures_at_the_low_end() {
        let row = RiGridRow {
            n: 1.63,
            windowing_table_up: 6.0,
            windowing_tilt: 15.0,
            brilliance_tilt: 47.0,
            extinction_tilt: 15.0,
        };
        assert_eq!(
            summary(1.63, &row),
            "RI 1.63, the lowest that works: windowing 15 % tilted by 10\u{b0} (limit 15 %), \
             6 % table up; brilliance 47 %, extinction 15 % tilted by 10\u{b0}."
        );
    }

    #[test]
    fn the_heading_and_chips() {
        assert_eq!(
            works_heading("1.63 and up"),
            "Works in (Pleochroma estimate, RI 1.63 and up):"
        );
        let sapphire = BuiltIn {
            name: "Sapphire".to_owned(),
            n: 1.768,
        };
        assert_eq!(chip(&sapphire), "Sapphire 1.768");
        assert_eq!(
            more_hint(&names(&["Zircon 1.925", "Diamond 2.417"])),
            "Also works in: Zircon 1.925, Diamond 2.417."
        );
    }

    #[test]
    fn outside_judges_by_the_low_end_while_the_high_end_is_not_calibrated() {
        let below = names(&["Quartz", "Aquamarine", "Tourmaline"]);
        let above = names(&["Diamond", "Rutile"]);
        let range = range(Some(1.63), Some(2.31));
        assert_eq!(
            outside(&below, &above, &range, false),
            "Outside: Quartz, Aquamarine and Tourmaline window below 1.63."
        );
        assert_eq!(
            outside(&below, &above, &range, true),
            "Outside: Quartz, Aquamarine and Tourmaline window below 1.63; Diamond and Rutile \
             lose brilliance above 2.31."
        );
        assert_eq!(
            outside(&[], &[], &range, true),
            "Outside: none of the built-in materials."
        );
    }

    #[test]
    fn the_status_says_when_the_high_end_is_not_calibrated() {
        assert_eq!(
            status(27, true),
            format!(
                "27 steps from 1.40 to 2.70, both ends to 0.01 \u{b7} thresholds v{THRESHOLD_VERSION}"
            )
        );
        assert!(status(27, false).ends_with("\u{b7} high end not calibrated"));
    }
}
