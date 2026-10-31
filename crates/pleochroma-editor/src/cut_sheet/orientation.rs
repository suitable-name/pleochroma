//! The Orientation block of the printed cutting sheet: the lines of
//! [`SheetOrientation`] and a small top and side diagram of the crystal axis in the stone.
//!
//! The diagram is an inline SVG built from the angles alone, so it is deterministic and the
//! sheet stays one self-contained file. All styling is inline: the page's own stylesheet is
//! untouched, so a sheet without the block is byte for byte what it was.

use super::html::html_escape;
use pleochroma_cut_core::SheetOrientation;
use std::fmt::Write as _;

/// Ink of the stone outline and the labels.
const INK: &str = "#333333";
/// Ink of the crystal axis (it must read on a black and white print too, so it is also thick).
const AXIS_INK: &str = "#b91c1c";
/// The diagram's size in SVG units.
const WIDTH: f64 = 380.0;
/// See [`WIDTH`].
const HEIGHT: f64 = 190.0;

/// One number as the diagram writes it: a decimal with one place.
fn n(value: f64) -> String {
    format!("{value:.1}")
}

/// A line through `centre` along `direction`, reaching `reach` each way, in the diagram's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisLine {
    /// Where the line crosses the stone's middle.
    pub centre: (f64, f64),
    /// The unit direction (SVG axes: x to the right, y down).
    pub direction: (f64, f64),
    /// How far the line reaches either way from the centre.
    pub reach: f64,
}

impl AxisLine {
    /// The two ends: first the one at `-reach`, then the one at `+reach` (the dotted end).
    #[must_use]
    pub fn ends(&self) -> ((f64, f64), (f64, f64)) {
        let (cx, cy) = self.centre;
        let (dx, dy) = self.direction;
        (
            (dx.mul_add(-self.reach, cx), dy.mul_add(-self.reach, cy)),
            (dx.mul_add(self.reach, cx), dy.mul_add(self.reach, cy)),
        )
    }
}

/// The geometry of the crystal axis diagram, as data: the cutting sheet draws it as an SVG
/// ([`orientation_svg`]) and desktop Cutting Mode draws it with the same numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisDiagram {
    /// The picture's size.
    pub size: (f64, f64),
    /// The middle of the top view (the stone seen from above the table).
    pub top_centre: (f64, f64),
    /// The radius of the girdle outline in the top view.
    pub top_radius: f64,
    /// The axis projected onto the table; `None` for an axis through the table (it then points at
    /// the viewer).
    pub top_axis: Option<AxisLine>,
    /// The middle of the side view (the stone seen from the side, table up).
    pub side_centre: (f64, f64),
    /// The axis in the vertical plane of the axis, leaning right by its tilt.
    pub side_axis: AxisLine,
}

impl AxisDiagram {
    /// The corners of the side view's outline: crown, girdle and pavilion.
    #[must_use]
    pub fn side_outline(&self) -> [(f64, f64); 6] {
        let (x, y) = self.side_centre;
        [
            (x - 60.0, y),
            (x - 40.0, y - 28.0),
            (x + 40.0, y - 28.0),
            (x + 60.0, y),
            (x, y + 62.0),
            (x - 60.0, y),
        ]
    }
}

/// The diagram of `block`: the stone from above the table and from the side.
///
/// On the left the stone is seen from above the table (the axis projected onto the table, azimuth
/// running clockwise from index 0 at the right), on the right from the side, in the vertical plane
/// of the axis.
#[must_use]
pub fn axis_diagram(block: &SheetOrientation) -> AxisDiagram {
    let axis = &block.orientation.axis;
    let (sin_t, cos_t) = axis.tilt_deg().to_radians().sin_cos();
    let (sin_p, cos_p) = axis.azimuth_deg().to_radians().sin_cos();
    let top_centre = (95.0, 98.0);
    let top_radius = 70.0;
    AxisDiagram {
        size: (WIDTH, HEIGHT),
        top_centre,
        top_radius,
        top_axis: (axis.tilt_deg() > 0.5).then_some(AxisLine {
            centre: top_centre,
            direction: (sin_t * cos_p, sin_t * sin_p),
            reach: top_radius * 0.9,
        }),
        side_centre: (285.0, 98.0),
        side_axis: AxisLine {
            centre: (285.0, 98.0),
            direction: (sin_t, -cos_t),
            reach: 80.0,
        },
    }
}

/// The axis line with a dot at the positive end.
fn axis_line(out: &mut String, line: AxisLine) {
    let (from, to) = line.ends();
    let _ = write!(
        out,
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{AXIS_INK}\" stroke-width=\"3\" \
         stroke-linecap=\"round\"/>\
         <circle cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{AXIS_INK}\"/>",
        n(from.0),
        n(from.1),
        n(to.0),
        n(to.1),
        n(to.0),
        n(to.1),
    );
}

/// A text label.
fn label(out: &mut String, at: (f64, f64), anchor: &str, text: &str) {
    let _ = write!(
        out,
        "<text x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" font-size=\"11\" fill=\"{INK}\" \
         font-family=\"Arial, Helvetica, sans-serif\">{}</text>",
        n(at.0),
        n(at.1),
        html_escape(text)
    );
}

/// The diagram of `block`: on the left the stone seen from above the table (the axis projected
/// onto the table, azimuth running clockwise from index 0 at the right), on the right the stone
/// seen from the side, in the vertical plane of the axis (the axis leaning toward the right by
/// its tilt).
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "the six corners of the side outline, used positionally"
)]
pub(super) fn orientation_svg(block: &SheetOrientation) -> String {
    let geometry = axis_diagram(block);
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" \
         role=\"img\" aria-label=\"The crystal axis in the stone, from above and from the side\">",
        n(geometry.size.0),
        n(geometry.size.1),
        n(geometry.size.0),
        n(geometry.size.1)
    );

    // Top view: the girdle outline, the index 0 mark, the axis projected onto the table.
    let top = geometry.top_centre;
    let radius = geometry.top_radius;
    let _ = write!(
        out,
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.5\"/>\
         <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1\" \
         stroke-dasharray=\"3 3\"/>",
        n(top.0),
        n(top.1),
        n(radius),
        n(top.0),
        n(top.1),
        n(top.0 + radius + 8.0),
        n(top.1),
    );
    label(&mut out, (top.0 + radius + 10.0, top.1 + 4.0), "start", "0");
    if let Some(line) = geometry.top_axis {
        axis_line(&mut out, line);
    } else {
        // Through the table: the axis points at the viewer.
        let _ = write!(
            out,
            "<circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"none\" stroke=\"{AXIS_INK}\" \
             stroke-width=\"3\"/><circle cx=\"{}\" cy=\"{}\" r=\"2\" fill=\"{AXIS_INK}\"/>",
            n(top.0),
            n(top.1),
            n(top.0),
            n(top.1)
        );
    }
    label(
        &mut out,
        (top.0, 184.0),
        "middle",
        "Top view (index 0 at the right)",
    );

    // Side view: crown, girdle and pavilion outline; the axis leans by its tilt.
    let side = geometry.side_centre;
    let [a, b, c, d, e, f] = geometry.side_outline();
    let _ = write!(
        out,
        "<polygon points=\"{},{} {},{} {},{} {},{} {},{} {},{}\" fill=\"none\" stroke=\"{INK}\" \
         stroke-width=\"1.5\" stroke-linejoin=\"round\"/>",
        n(a.0),
        n(a.1),
        n(b.0),
        n(b.1),
        n(c.0),
        n(c.1),
        n(d.0),
        n(d.1),
        n(e.0),
        n(e.1),
        n(f.0),
        n(f.1),
    );
    axis_line(&mut out, geometry.side_axis);
    label(&mut out, (side.0, 184.0), "middle", "Side view (table up)");
    out.push_str("</svg>");
    out
}

/// Appends the Orientation block of `block` to the page: the heading, the lines and the diagram.
pub(super) fn push_orientation(out: &mut String, block: &SheetOrientation) {
    out.push_str(
        "<section class=\"orientation\" style=\"break-inside:avoid;page-break-inside:avoid;\">\n\
         <h2>Orientation</h2>\n\
         <div style=\"display:flex;gap:16px;align-items:flex-start;\">\n\
         <ul style=\"font-size:13px;margin:0;padding-left:18px;flex:1;\">\n",
    );
    // The first line is the text sheet's heading; the page has its own.
    for line in block.lines().iter().skip(1) {
        let _ = writeln!(out, "<li>{}</li>", html_escape(line));
    }
    out.push_str("</ul>\n");
    out.push_str(&orientation_svg(block));
    out.push_str("\n</div>\n</section>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::CrystalOrientation;

    fn block(tilt: f64, azimuth: f64) -> SheetOrientation {
        SheetOrientation::new(
            CrystalOrientation::from_angles(tilt, azimuth, None),
            false,
            96,
            0.0,
        )
    }

    #[test]
    fn the_diagram_is_deterministic() {
        assert_eq!(
            orientation_svg(&block(40.0, 90.0)),
            orientation_svg(&block(40.0, 90.0))
        );
        assert_ne!(
            orientation_svg(&block(40.0, 90.0)),
            orientation_svg(&block(40.0, 10.0))
        );
    }

    #[test]
    fn an_axis_through_the_table_is_a_dot_in_the_top_view() {
        let svg = orientation_svg(&block(0.0, 0.0));
        assert!(svg.contains("Top view"));
        assert!(svg.contains("Side view"));
        // No line in the top view: only the dashed index mark and the side view's axis.
        assert_eq!(svg.matches("<line ").count(), 2);
    }

    #[test]
    fn a_leaning_axis_is_a_line_in_each_view() {
        let svg = orientation_svg(&block(40.0, 90.0));
        assert_eq!(svg.matches("<line ").count(), 3);
    }

    #[test]
    fn the_diagram_data_is_what_the_svg_draws() {
        let leaning = axis_diagram(&block(40.0, 90.0));
        let top = leaning
            .top_axis
            .expect("a leaning axis shows in the top view");
        // Azimuth 90 runs straight down the page (SVG y grows downward) in the top view.
        assert!(top.direction.0.abs() < 1e-9 && top.direction.1 > 0.0);
        let (from, to) = top.ends();
        assert!((from.1 - leaning.top_centre.1).abs() > 1.0);
        assert!(
            (-2.0f64).mul_add(leaning.top_centre.1, to.1 + from.1).abs() < 1e-9,
            "centred"
        );
        let svg = orientation_svg(&block(40.0, 90.0));
        assert!(svg.contains(&format!("x2=\"{}\"", n(to.0))));
        assert!(svg.contains(&format!("y2=\"{}\"", n(to.1))));
        // The side view leans right by the tilt; the outline closes on its first corner.
        assert!(leaning.side_axis.direction.0 > 0.0);
        let outline = leaning.side_outline();
        assert_eq!(outline[0], outline[5]);
        // Through the table the top view has no line, only the dot.
        assert!(axis_diagram(&block(0.0, 0.0)).top_axis.is_none());
    }

    #[test]
    fn the_block_lists_every_line_but_the_heading() {
        let block = block(40.0, 90.0);
        let mut page = String::new();
        push_orientation(&mut page, &block);
        assert!(page.contains("<h2>Orientation</h2>"));
        assert!(page.contains("Orient the rough so the crystal axis (c-axis) leans 40"));
        assert!(page.contains("(index 24)"));
        assert!(page.contains("<svg "));
        assert_eq!(
            page.matches("<li>").count(),
            block.lines().len() - 1,
            "one item per line, without the heading"
        );
    }
}
