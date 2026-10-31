//! The rough's Crystal Axis on the wizard's photo canvas.
//!
//! The planner's rough may have a Crystal Axis (a line through the rough). On the Fit step the
//! wizard draws that line over the transmittance photo with the zone outlines, as the locate
//! window does over its photos, so the cutter sees how the zones and the axis lie in the photo.

use super::context::AxisLine;
use glam::DVec2;
use pleochroma_cut_core::rough_plan::{
    locate::{RigProfile, Rigid},
    photometry::WorkingGrid,
};

/// The colour the axis line is drawn in.
pub const AXIS_COLOUR: [u8; 3] = [255, 255, 255];

/// The axis of the rough as a two-point polyline in the working pixels of one view.
///
/// The view is `view` of `rig` (`grid`: the working grid of that view's photo), the mesh moved
/// into the rig by `alignment`. `None` when the rig has no such view or an end of the line is behind a pinhole camera.
#[must_use]
pub fn axis_polyline(
    rig: &RigProfile,
    alignment: &Rigid,
    view: usize,
    axis: &AxisLine,
    grid: &WorkingGrid,
) -> Option<Vec<[f64; 2]>> {
    let pose = rig.views.get(view)?;
    let end = |sign: f64| {
        pose.project(alignment.to_rig(axis.centre + axis.direction * (sign * axis.half_mm)))
    };
    let working = |pixel: DVec2| {
        [
            (pixel.x - grid.origin[0]) / grid.scale,
            (pixel.y - grid.origin[1]) / grid.scale,
        ]
    };
    Some(vec![working(end(1.0)?), working(end(-1.0)?)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use pleochroma_cut_core::rough_plan::locate::{Projection, ViewPose};

    fn rig() -> RigProfile {
        let front = ViewPose::look_at(
            "front",
            DVec3::new(0.0, 0.0, 100.0),
            DVec3::ZERO,
            DVec3::Y,
            Projection::Orthographic { px_per_mm: 10.0 },
            [200, 200],
        );
        RigProfile::new("test", vec![front], 1.76)
    }

    fn axis() -> AxisLine {
        AxisLine {
            centre: DVec3::ZERO,
            direction: DVec3::X,
            half_mm: 5.0,
        }
    }

    #[test]
    fn the_axis_is_projected_into_the_working_pixels_of_the_view() {
        // A working grid that starts at full-resolution pixel (50, 40) and halves the photo.
        let grid = WorkingGrid {
            width: 50,
            height: 50,
            origin: [50.0, 40.0],
            scale: 2.0,
        };
        let line = axis_polyline(&rig(), &Rigid::IDENTITY, 0, &axis(), &grid).expect("projects");
        assert_eq!(line.len(), 2);
        // The camera looks along -z with y up: +x lies to the right of the image centre
        // (100, 100), 50 px away; in working pixels (x - 50) / 2, (y - 40) / 2.
        let expected_y = (100.0 - 40.0) / 2.0;
        let right = [(150.0 - 50.0) / 2.0, expected_y];
        let left = [(50.0 - 50.0) / 2.0, expected_y];
        let (a, b) = (line[0], line[1]);
        let matches =
            |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6;
        assert!(
            (matches(a, right) && matches(b, left)) || (matches(a, left) && matches(b, right)),
            "{line:?}"
        );
    }

    #[test]
    fn a_view_the_rig_does_not_have_draws_nothing() {
        let grid = WorkingGrid {
            width: 10,
            height: 10,
            origin: [0.0, 0.0],
            scale: 1.0,
        };
        assert!(axis_polyline(&rig(), &Rigid::IDENTITY, 3, &axis(), &grid).is_none());
    }
}
