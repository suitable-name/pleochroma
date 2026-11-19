//! Cuts chart paths into dashes. Slint 1.18.1 has no dash attribute on `Path`, so a dashed line
//! is drawn as a row of short sub-paths; this is the same cut Angle Sweep's chart makes
//! (`gui::editor::sweep::present::dash_path`), with the same pixel lengths.

use std::fmt::Write as _;

/// The length of one dash, in screen pixels.
pub(super) const DASH_PX: f64 = 6.0;

/// The length of the gap between two dashes, in screen pixels.
pub(super) const GAP_PX: f64 = 4.0;

/// The plot's size in pixels before the view has reported it; only the dash length depends on it.
pub(super) const DEFAULT_PLOT_PX: (f64, f64) = (900.0, 170.0);

/// Reads an SVG path of `M x y L x y ...` into its sub-paths.
fn subpaths_of(commands: &str) -> Vec<Vec<(f64, f64)>> {
    let mut subpaths: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut tokens = commands.split_whitespace();
    while let Some(token) = tokens.next() {
        let mut number = || tokens.next().and_then(|text| text.parse::<f64>().ok());
        let (Some(x), Some(y)) = (number(), number()) else {
            break;
        };
        match token {
            "M" => subpaths.push(vec![(x, y)]),
            _ => {
                if let Some(points) = subpaths.last_mut() {
                    points.push((x, y));
                }
            }
        }
    }
    subpaths
}

/// Cuts a chart path (`M x y L x y ...` in the 0..100 viewbox) into dashes of `dash_px` with gaps
/// of `gap_px` screen pixels, for a plot of `width_px` by `height_px`.
///
/// Lengths are measured in pixels (the viewbox is stretched unevenly), the dash phase carries
/// across the data points of a sub-path, and a new `M` starts the pattern afresh. A sub-path of
/// fewer than two points draws nothing. A size, dash or gap that is not positive returns
/// `commands` unchanged.
pub(super) fn dash_path(
    commands: &str,
    (width_px, height_px): (f64, f64),
    dash_px: f64,
    gap_px: f64,
) -> String {
    if !(width_px > 0.0 && height_px > 0.0 && dash_px > 0.0 && gap_px > 0.0) {
        return commands.to_owned();
    }
    let (kx, ky) = (width_px / 100.0, height_px / 100.0);
    let cycle = dash_px + gap_px;
    let mut out = String::new();
    let mut put = |command: char, x: f64, y: f64| {
        if !out.is_empty() {
            out.push(' ');
        }
        let _ = write!(out, "{command} {x:.2} {y:.2}");
    };
    for points in &subpaths_of(commands) {
        // `phase` is the distance into the dash-and-gap cycle; the pattern starts on a dash.
        let mut phase = 0.0_f64;
        let mut open = false;
        for pair in points.windows(2) {
            let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
            let length = ((bx - ax) * kx).hypot((by - ay) * ky);
            let mut done = 0.0_f64;
            while done < length {
                let on = phase < dash_px;
                let remaining = (if on { dash_px } else { cycle }) - phase;
                let step = remaining.min(length - done);
                if on && !open {
                    let t = done / length;
                    put('M', (bx - ax).mul_add(t, ax), (by - ay).mul_add(t, ay));
                    open = true;
                }
                done += step;
                if on {
                    let t = done / length;
                    put('L', (bx - ax).mul_add(t, ax), (by - ay).mul_add(t, ay));
                }
                if step >= remaining {
                    phase = if on { dash_px } else { 0.0 };
                    if on {
                        open = false;
                    }
                } else {
                    phase += step;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_of(text: &str, command: &str) -> usize {
        text.split_whitespace()
            .filter(|token| *token == command)
            .count()
    }

    #[test]
    fn a_dashed_line_has_one_segment_per_dash_of_its_pixel_length() {
        // 100 px long: 8 px dashes with 4 px gaps start every 12 px, the last one cut short.
        let dashed = dash_path("M 0 0 L 100 0", (100.0, 100.0), 8.0, 4.0);
        assert_eq!(count_of(&dashed, "M"), 9);
        assert!(dashed.starts_with("M 0.00 0.00 L 8.00 0.00 M 12.00 0.00"));
        assert!(dashed.ends_with("M 96.00 0.00 L 100.00 0.00"), "{dashed}");
        // The length is in pixels: a plot twice as wide has twice the dashes.
        let wide = dash_path("M 0 0 L 100 0", (200.0, 100.0), 8.0, 4.0);
        assert_eq!(count_of(&wide, "M"), 17);
    }

    #[test]
    fn the_pattern_carries_across_data_points() {
        let dashed = dash_path("M 0 0 L 50 0 L 100 0", (100.0, 100.0), 8.0, 4.0);
        assert!(
            dashed.contains("M 48.00 0.00 L 50.00 0.00 L 56.00 0.00"),
            "{dashed}"
        );
        let two = dash_path("M 0 0 L 10 0 M 0 50 L 10 50", (100.0, 100.0), 8.0, 4.0);
        assert!(two.contains("M 0.00 50.00 L 8.00 50.00"), "{two}");
    }

    #[test]
    fn a_vertical_line_is_cut_by_the_plot_height() {
        // 170 px tall: dashes of 6 px with gaps of 4 px start every 10 px (17 of them).
        let dashed = dash_path("M 50 0 L 50 100", (900.0, 170.0), DASH_PX, GAP_PX);
        assert_eq!(count_of(&dashed, "M"), 17);
        assert!(dashed.starts_with("M 50.00 0.00 L 50.00 3.53"), "{dashed}");
    }

    #[test]
    fn empty_one_point_and_unsized_lines_draw_no_dashes() {
        assert_eq!(dash_path("", (100.0, 100.0), 8.0, 4.0), "");
        assert_eq!(dash_path("M 5 5", (100.0, 100.0), 8.0, 4.0), "");
        assert_eq!(
            dash_path("M 0 0 L 9 9", (0.0, 100.0), 8.0, 4.0),
            "M 0 0 L 9 9"
        );
    }
}
