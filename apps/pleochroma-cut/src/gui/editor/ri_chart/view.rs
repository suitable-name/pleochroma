//! What the RI chart shows, built from a traced range: the lines, the range marks, the material
//! ticks and chips and every sentence. Slint-free; `present.rs` copies the result into
//! `RiChartModel`.
//!
//! Both forms of the high end are built here. While the high end is not calibrated
//! (`calibrated == false`, the state of `HIGH_BOUND_CALIBRATED` at the start) the range is shaded
//! to the right edge, the high end is a dashed line marked "uncalibrated", and the material
//! lists judge by the low bound alone. Calibrated, the range ends in a solid edge and the lists
//! judge by both bounds.

use super::{
    dash::{DASH_PX, GAP_PX, dash_path},
    layout::{
        BuiltIn, RI_MAX, RI_MIN, built_in_materials, chip_width_px, label_rows, line_commands,
        nearest_row, wrap_rows, x_percent, y_percent,
    },
    text,
};
use pleochroma_editor::ri_range::{RiRange, WINDOWING_LIMIT_PCT, range_text};

/// How many chips the "Works in" line shows before "and N more".
const CHIP_CAP: usize = 12;

/// The series number of the tilted windowing line in `RiChartModel`.
const SERIES_WINDOWING_TILT: i32 = 0;

/// The series number of the table-up windowing line (dashed).
const SERIES_WINDOWING_TABLE_UP: i32 = 1;

/// The series number of the tilted brilliance line.
const SERIES_BRILLIANCE_TILT: i32 = 2;

/// The series number of the tilted extinction line.
const SERIES_EXTINCTION_TILT: i32 = 3;

/// The series number of the uncalibrated high end of the range (dashed).
const SERIES_HIGH_END: i32 = 4;

/// What a tick is: outside the range, inside it, or the design's own material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TickState {
    /// The range excludes this material.
    Outside,
    /// The range includes it.
    Inside,
    /// It is the design's own material.
    Current,
}

impl TickState {
    /// The number `RiChartModel` knows it by.
    pub(super) const fn number(self) -> i32 {
        match self {
            Self::Outside => 0,
            Self::Inside => 1,
            Self::Current => 2,
        }
    }
}

/// One built-in material on the index axis.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Tick {
    /// Position on the axis in percent.
    pub(super) x: f32,
    /// The material's name.
    pub(super) label: String,
    /// The label row, or `None` when the name is folded into the chips.
    pub(super) row: Option<usize>,
    /// Whether the material is outside, inside or the design's own.
    pub(super) state: TickState,
}

/// One "Works in" chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Chip {
    /// "Sapphire 1.768".
    pub(super) text: String,
    /// The design's own material.
    pub(super) current: bool,
}

/// A position on the plot: percent across (or down), and its label.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    /// Percent of the plot.
    pub(super) at: f32,
    /// The words beside it.
    pub(super) text: String,
}

/// Everything the dialog shows for a traced range.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct View {
    /// The lead sentence.
    pub(super) intro: String,
    /// `(series, path data)` of every line that has points.
    pub(super) lines: Vec<(i32, String)>,
    /// Where the shading starts and stops, in percent of the width.
    pub(super) band: (f32, f32),
    /// The low end of the range, labelled.
    pub(super) low: Mark,
    /// The high end of the range, when there is one.
    pub(super) high: Option<Mark>,
    /// The design's own index, when it is on the axis.
    pub(super) current: Option<Mark>,
    /// The windowing limit: its height in percent from the top.
    pub(super) limit: Mark,
    /// The left end of the axis.
    pub(super) axis_low: String,
    /// The right end of the axis.
    pub(super) axis_high: String,
    /// The built-in materials, by rising index.
    pub(super) ticks: Vec<Tick>,
    /// The figures at the lowest index that works.
    pub(super) summary: String,
    /// The heading over the chips.
    pub(super) works_heading: String,
    /// The chips, wrapped to rows.
    pub(super) chip_rows: Vec<Vec<Chip>>,
    /// "and 3 more", or the line that says no material works.
    pub(super) more_text: String,
    /// The hover of `more_text`.
    pub(super) more_hint: String,
    /// The "Outside" line.
    pub(super) outside: String,
    /// The footer line.
    pub(super) status: String,
}

/// What a view is built from.
#[derive(Debug, Clone, Copy)]
pub(super) struct Inputs<'a> {
    /// The traced range.
    pub(super) range: &'a RiRange,
    /// The design's material, "" when it names none.
    pub(super) material: &'a str,
    /// Whether the high bound is calibrated (`HIGH_BOUND_CALIBRATED`).
    pub(super) calibrated: bool,
    /// The plot's size in pixels (dash length, label room, chip rows).
    pub(super) plot_px: (f64, f64),
}

/// A percentage as the model's `float`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the Slint property precision; a chart percentage is 0..=100"
)]
const fn pct(value: f64) -> f32 {
    value as f32
}

/// The four lines (and the dashed high end while it is not calibrated).
fn lines_of(input: &Inputs<'_>) -> Vec<(i32, String)> {
    let rows = &input.range.rows;
    let dashed = |commands: &str| dash_path(commands, input.plot_px, DASH_PX, GAP_PX);
    let mut lines = vec![
        (
            SERIES_WINDOWING_TILT,
            line_commands(rows, |row| row.windowing_tilt),
        ),
        (
            SERIES_WINDOWING_TABLE_UP,
            dashed(&line_commands(rows, |row| row.windowing_table_up)),
        ),
        (
            SERIES_BRILLIANCE_TILT,
            line_commands(rows, |row| row.brilliance_tilt),
        ),
        (
            SERIES_EXTINCTION_TILT,
            line_commands(rows, |row| row.extinction_tilt),
        ),
    ];
    if let (false, Some(high)) = (input.calibrated, input.range.n_high) {
        let x = x_percent(high);
        lines.push((SERIES_HIGH_END, dashed(&format!("M {x:.2} 0 L {x:.2} 100"))));
    }
    lines.retain(|(_, commands)| !commands.is_empty());
    lines
}

/// The built-in materials as ticks: their state, and the label row each name gets.
fn ticks_of(input: &Inputs<'_>, materials: &[BuiltIn], current: Option<&str>) -> Vec<Tick> {
    let positions: Vec<(f64, usize)> = materials
        .iter()
        .map(|material| {
            (
                x_percent(material.n) / 100.0 * input.plot_px.0,
                material.name.chars().count(),
            )
        })
        .collect();
    let rows = label_rows(&positions);
    materials
        .iter()
        .zip(rows)
        .map(|(material, row)| {
            let state = if current == Some(material.name.as_str()) {
                TickState::Current
            } else if input.range.contains_as(material.n, input.calibrated) {
                TickState::Inside
            } else {
                TickState::Outside
            };
            Tick {
                x: pct(x_percent(material.n)),
                label: material.name.clone(),
                row,
                state,
            }
        })
        .collect()
}

/// The "Works in" chips, their overflow and the "Outside" line.
struct Lists {
    chip_rows: Vec<Vec<Chip>>,
    more_text: String,
    more_hint: String,
    outside: String,
}

/// Sorts the built-in materials into those that work (chips) and those that do not (outside).
fn lists_of(input: &Inputs<'_>, materials: &[BuiltIn], current: Option<&str>) -> Lists {
    let range = input.range;
    let (works, outside): (Vec<&BuiltIn>, Vec<&BuiltIn>) = materials
        .iter()
        .partition(|material| range.contains_as(material.n, input.calibrated));
    let name_of = |material: &&BuiltIn| material.name.clone();
    let below: Vec<String> = outside
        .iter()
        .filter(|material| material.n < range.n0)
        .map(name_of)
        .collect();
    let above: Vec<String> = outside
        .iter()
        .filter(|material| material.n >= range.n0)
        .map(name_of)
        .collect();
    let chips: Vec<Chip> = works
        .iter()
        .map(|material| Chip {
            text: text::chip(material),
            current: current == Some(material.name.as_str()),
        })
        .collect();
    let rest: Vec<String> = chips
        .iter()
        .skip(CHIP_CAP)
        .map(|chip| chip.text.clone())
        .collect();
    let shown: Vec<Chip> = chips.into_iter().take(CHIP_CAP).collect();
    let widths: Vec<f64> = shown.iter().map(|chip| chip_width_px(&chip.text)).collect();
    let mut chip_rows: Vec<Vec<Chip>> = Vec::new();
    for indices in wrap_rows(&widths, input.plot_px.0) {
        chip_rows.push(indices.into_iter().map(|i| shown[i].clone()).collect());
    }
    let (more_text, more_hint) = if shown.is_empty() {
        (text::none_work(), String::new())
    } else if rest.is_empty() {
        (String::new(), String::new())
    } else {
        (format!("and {} more", rest.len()), text::more_hint(&rest))
    };
    Lists {
        chip_rows,
        more_text,
        more_hint,
        outside: text::outside(&below, &above, range, input.calibrated),
    }
}

/// The range's marks: the shaded band, the low label and the high label.
fn range_marks(input: &Inputs<'_>, low: f64) -> ((f32, f32), Mark, Option<Mark>) {
    let range = input.range;
    let band_end = match (input.calibrated, range.n_high) {
        (true, Some(high)) => x_percent(high),
        _ => 100.0,
    };
    let low_mark = Mark {
        at: pct(x_percent(low)),
        text: text::low_label(low),
    };
    let high_mark = range.n_high.map(|high| Mark {
        at: pct(x_percent(high)),
        text: text::high_label(high, input.calibrated),
    });
    ((low_mark.at, pct(band_end)), low_mark, high_mark)
}

/// Builds everything the chart shows, or `None` when the range has no low bound (a stone with
/// no geometry).
pub(super) fn build(input: &Inputs<'_>) -> Option<View> {
    let range = input.range;
    let low = range.n_low?;
    let statement = range.statement_as(input.calibrated)?;
    let materials = built_in_materials();
    let current = materials
        .iter()
        .find(|material| material.name.eq_ignore_ascii_case(input.material))
        .map(|material| material.name.as_str());
    let (band, low_mark, high_mark) = range_marks(input, low);
    let lists = lists_of(input, &materials, current);
    let summary =
        nearest_row(&range.rows, low).map_or_else(String::new, |row| text::summary(low, row));
    Some(View {
        intro: text::intro(),
        lines: lines_of(input),
        band,
        low: low_mark,
        high: high_mark,
        current: (RI_MIN..=RI_MAX).contains(&range.n0).then(|| Mark {
            at: pct(x_percent(range.n0)),
            text: text::current_label(input.material),
        }),
        limit: Mark {
            at: pct(y_percent(f64::from(WINDOWING_LIMIT_PCT))),
            text: text::limit_label(),
        },
        axis_low: format!("{RI_MIN:.2}"),
        axis_high: format!("{RI_MAX:.2}"),
        ticks: ticks_of(input, &materials, current),
        summary,
        works_heading: text::works_heading(&range_text(&statement)),
        chip_rows: lists.chip_rows,
        more_text: lists.more_text,
        more_hint: lists.more_hint,
        outside: lists.outside,
        status: text::status(range.rows.len(), input.calibrated),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::ri_range::{RiGridRow, THRESHOLD_VERSION};

    /// A made-up trace: windowing falls and extinction rises with the index. The numbers are
    /// invented for the layout, not a result.
    fn range(n_low: Option<f64>, n_high: Option<f64>) -> RiRange {
        let rows: Vec<RiGridRow> = (0..=26_u8)
            .map(|step| {
                let n = 0.05_f64.mul_add(f64::from(step), 1.40);
                let t = f32::from(step) * 0.05;
                RiGridRow {
                    n,
                    windowing_table_up: 40.0 - 30.0 * t,
                    windowing_tilt: 60.0 - 45.0 * t,
                    brilliance_tilt: 30.0 + 20.0 * t,
                    extinction_tilt: 10.0 + 25.0 * t,
                }
            })
            .collect();
        RiRange {
            n0: 1.768,
            own: rows[7],
            rows,
            n_low,
            n_high,
            threshold_version: THRESHOLD_VERSION,
        }
    }

    fn inputs<'a>(range: &'a RiRange, material: &'a str, calibrated: bool) -> Inputs<'a> {
        Inputs {
            range,
            material,
            calibrated,
            plot_px: (900.0, 170.0),
        }
    }

    fn tick<'a>(view: &'a View, name: &str) -> &'a Tick {
        view.ticks
            .iter()
            .find(|tick| tick.label == name)
            .expect("the tick")
    }

    #[test]
    fn a_range_without_a_low_bound_has_no_view() {
        let range = range(None, None);
        assert!(build(&inputs(&range, "Sapphire", false)).is_none());
    }

    #[test]
    fn uncalibrated_the_range_is_shaded_to_the_edge_and_the_high_end_is_dashed() {
        let range = range(Some(1.63), Some(2.31));
        let view = build(&inputs(&range, "Sapphire", false)).expect("a view");
        assert!((view.band.0 - 17.69).abs() < 0.01 && (view.band.1 - 100.0).abs() < 1e-3);
        let high = view.high.as_ref().expect("a high mark");
        assert_eq!(high.text, "to 2.31 (uncalibrated)");
        let dashed_end = view
            .lines
            .iter()
            .find(|(series, _)| *series == SERIES_HIGH_END)
            .expect("the dashed high end");
        assert!(
            dashed_end.1.starts_with("M 70.00 0.00 L 70.00"),
            "{}",
            dashed_end.1
        );
        assert!(
            view.works_heading.ends_with("RI 1.63 and up):"),
            "{}",
            view.works_heading
        );
        assert!(view.status.ends_with("high end not calibrated"));
    }

    #[test]
    fn calibrated_the_range_ends_in_a_solid_edge() {
        let range = range(Some(1.63), Some(2.31));
        let view = build(&inputs(&range, "Sapphire", true)).expect("a view");
        assert!((view.band.1 - pct(x_percent(2.31))).abs() < 1e-3);
        assert_eq!(view.high.as_ref().expect("a high mark").text, "to 2.31");
        assert!(
            view.lines
                .iter()
                .all(|(series, _)| *series != SERIES_HIGH_END)
        );
        assert!(
            view.works_heading.ends_with("RI 1.63 to 2.31):"),
            "{}",
            view.works_heading
        );
    }

    #[test]
    fn the_lists_follow_the_calibration() {
        let range = range(Some(1.63), Some(2.31));
        let low_only = build(&inputs(&range, "Sapphire", false)).expect("a view");
        let both = build(&inputs(&range, "Sapphire", true)).expect("a view");
        // Diamond (2.417) works while only the low bound judges, and does not with both.
        assert_eq!(tick(&low_only, "Diamond").state, TickState::Inside);
        assert_eq!(tick(&both, "Diamond").state, TickState::Outside);
        // Quartz (1.544) is below the low bound either way.
        assert_eq!(tick(&low_only, "Quartz").state, TickState::Outside);
        assert!(low_only.outside.contains("window below 1.63"));
        assert!(!low_only.outside.contains("lose brilliance"));
        assert!(both.outside.contains("lose brilliance above 2.31"));
    }

    #[test]
    fn the_design_own_material_is_marked_and_a_custom_one_has_no_tick() {
        let range = range(Some(1.63), None);
        let view = build(&inputs(&range, "sapphire", false)).expect("a view");
        assert_eq!(tick(&view, "Sapphire").state, TickState::Current);
        assert_eq!(
            view.current.as_ref().expect("current").text,
            "Current: sapphire"
        );
        assert!(
            view.chip_rows
                .iter()
                .flatten()
                .any(|chip| chip.current && chip.text.starts_with("Sapphire "))
        );
        // A custom material is on the chart as the current line, but not among the ticks.
        let custom = build(&inputs(&range, "My Glass", false)).expect("a view");
        assert!(custom.ticks.iter().all(|tick| tick.label != "My Glass"));
        assert!(
            custom
                .ticks
                .iter()
                .all(|tick| tick.state != TickState::Current)
        );
        assert_eq!(
            custom.current.as_ref().expect("current").text,
            "Current: My Glass"
        );
    }

    #[test]
    fn an_open_ended_range_shades_to_the_edge_in_both_forms() {
        let range = range(Some(1.63), None);
        for calibrated in [false, true] {
            let view = build(&inputs(&range, "", calibrated)).expect("a view");
            assert!((view.band.1 - 100.0).abs() < 1e-3);
            assert!(view.high.is_none());
            assert_eq!(view.current.as_ref().expect("current").text, "Current");
        }
    }

    #[test]
    fn the_chips_are_capped_and_the_rest_is_in_the_hover() {
        let range = range(Some(1.40), None);
        let view = build(&inputs(&range, "", false)).expect("a view");
        let shown: usize = view.chip_rows.iter().map(Vec::len).sum();
        assert_eq!(shown, CHIP_CAP);
        assert!(view.more_text.starts_with("and "), "{}", view.more_text);
        assert!(view.more_hint.starts_with("Also works in: "));
        assert!(view.outside.contains("none of the built-in materials"));
    }

    #[test]
    fn the_four_lines_share_one_percent_scale_and_the_limit_is_at_85() {
        let range = range(Some(1.63), None);
        let view = build(&inputs(&range, "", false)).expect("a view");
        let series: Vec<i32> = view.lines.iter().map(|(series, _)| *series).collect();
        assert_eq!(series, vec![0, 1, 2, 3]);
        assert!((view.limit.at - 85.0).abs() < 1e-3);
        assert_eq!(view.limit.text, "Windowing limit 15 %");
        // Brilliance at 1.40 is 30 %: 70 from the top.
        let brilliance = &view.lines[2].1;
        assert!(brilliance.starts_with("M 0.00 70.00"), "{brilliance}");
        assert_eq!(
            (view.axis_low.as_str(), view.axis_high.as_str()),
            ("1.40", "2.70")
        );
    }

    #[test]
    fn the_ticks_are_ordered_and_crowded_names_are_folded() {
        let range = range(Some(1.63), None);
        let view = build(&inputs(&range, "", false)).expect("a view");
        assert!(view.ticks.windows(2).all(|pair| pair[0].x <= pair[1].x));
        assert!(
            view.ticks.iter().any(|tick| tick.row.is_none()),
            "some names fold"
        );
        assert!(view.ticks.iter().any(|tick| tick.row == Some(0)));
        assert!(
            view.ticks
                .iter()
                .all(|tick| tick.row.is_none_or(|row| row < 4))
        );
    }
}
