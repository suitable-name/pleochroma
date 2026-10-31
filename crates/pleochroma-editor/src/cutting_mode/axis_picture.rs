//! The small top and side picture of the crystal axis on the first page of Cutting Mode.
//!
//! The geometry is the cutting sheet's own ([`crate::cut_sheet::axis_diagram`]: the stone seen from
//! above the table with index 0 at the right, and from the side with the table up); this only
//! writes it as the SVG path commands the desktop's picture draws, in the diagram's own
//! 380 x 190 units, so the picture scales with whatever room it is given.

use crate::cut_sheet::{AxisDiagram, AxisLine, axis_diagram};
use pleochroma_cut_core::SheetOrientation;
use std::fmt::Write as _;

/// What the desktop's axis picture draws.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisPicture {
    /// The picture's width in path units.
    pub width: f32,
    /// The picture's height in path units.
    pub height: f32,
    /// The outlines: the girdle in the top view, the index 0 mark, and the side view's outline.
    pub frame: String,
    /// The crystal axis in both views, with a dot at the leaning end (a ring and a dot where the
    /// axis points at the viewer).
    pub axis: String,
    /// What the picture shows, in words, for a screen reader.
    pub description: String,
}

/// A circle as path commands: two half arcs, so it closes without a seam.
fn circle(out: &mut String, (cx, cy): (f64, f64), radius: f64) {
    let _ = write!(
        out,
        "M {:.1} {:.1} A {r:.1} {r:.1} 0 1 1 {:.1} {:.1} A {r:.1} {r:.1} 0 1 1 {:.1} {:.1} Z ",
        cx - radius,
        cy,
        cx + radius,
        cy,
        cx - radius,
        cy,
        r = radius,
    );
}

/// The axis line with a dot at its positive end.
fn line_with_dot(out: &mut String, line: AxisLine) {
    let (from, to) = line.ends();
    let _ = write!(
        out,
        "M {:.1} {:.1} L {:.1} {:.1} ",
        from.0, from.1, to.0, to.1
    );
    circle(out, to, 4.0);
}

/// The path commands of `diagram`'s outlines.
fn frame_of(diagram: &AxisDiagram) -> String {
    let mut out = String::new();
    circle(&mut out, diagram.top_centre, diagram.top_radius);
    // The index 0 mark: from the middle to just past the girdle, at the right.
    let (cx, cy) = diagram.top_centre;
    let _ = write!(
        out,
        "M {cx:.1} {cy:.1} L {:.1} {cy:.1} ",
        cx + diagram.top_radius + 8.0
    );
    let outline = diagram.side_outline();
    for (index, (x, y)) in outline.iter().enumerate() {
        let command = if index == 0 { 'M' } else { 'L' };
        let _ = write!(out, "{command} {x:.1} {y:.1} ");
    }
    out.push('Z');
    out
}

/// The path commands of `diagram`'s axis in both views.
fn axis_of(diagram: &AxisDiagram) -> String {
    let mut out = String::new();
    if let Some(line) = diagram.top_axis {
        line_with_dot(&mut out, line);
    } else {
        // Through the table: the axis points at the viewer, a ring around a dot.
        circle(&mut out, diagram.top_centre, 6.0);
        circle(&mut out, diagram.top_centre, 2.0);
    }
    line_with_dot(&mut out, diagram.side_axis);
    out.trim_end().to_owned()
}

impl AxisPicture {
    /// The picture of the stone's crystal axis described by `block`.
    #[must_use]
    pub fn new(block: &SheetOrientation) -> Self {
        let diagram = axis_diagram(block);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the diagram is a few hundred units wide, far inside f32's range"
        )]
        let (width, height) = (diagram.size.0 as f32, diagram.size.1 as f32);
        Self {
            width,
            height,
            frame: frame_of(&diagram),
            axis: axis_of(&diagram),
            description: "The crystal axis in the stone, from above with index 0 at the right, \
                          and from the side with the table up."
                .to_owned(),
        }
    }
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
    fn the_picture_has_the_sheets_size_and_is_deterministic() {
        let picture = AxisPicture::new(&block(40.0, 90.0));
        assert!((picture.width - 380.0).abs() < f32::EPSILON);
        assert!((picture.height - 190.0).abs() < f32::EPSILON);
        assert_eq!(picture, AxisPicture::new(&block(40.0, 90.0)));
        assert_ne!(
            picture.axis,
            AxisPicture::new(&block(40.0, 10.0)).axis,
            "the azimuth moves the line in the top view"
        );
        assert!(
            picture.frame.starts_with("M 25.0 98.0 A 70.0 70.0"),
            "{}",
            picture.frame
        );
        assert!(picture.frame.ends_with('Z'));
    }

    #[test]
    fn an_axis_through_the_table_is_a_ring_and_a_dot_in_the_top_view() {
        let upright = AxisPicture::new(&block(0.0, 0.0));
        // Ring, dot, then the side view's line and its dot: two lines of commands in all
        // (the side view's), no top line.
        assert_eq!(upright.axis.matches('L').count(), 1, "{}", upright.axis);
        assert!(
            upright.axis.starts_with("M 89.0 98.0 A 6.0 6.0"),
            "{}",
            upright.axis
        );
        let leaning = AxisPicture::new(&block(40.0, 90.0));
        assert_eq!(leaning.axis.matches('L').count(), 2, "{}", leaning.axis);
    }

    #[test]
    fn the_axis_ends_are_the_sheets_ends() {
        let block = block(40.0, 90.0);
        let diagram = axis_diagram(&block);
        let (_, to) = diagram.side_axis.ends();
        let picture = AxisPicture::new(&block);
        assert!(
            picture.axis.contains(&format!("L {:.1} {:.1}", to.0, to.1)),
            "{}",
            picture.axis
        );
    }
}
