//! Turning the window-free drawing and report data of [`crate::locate_io`] into the Slint
//! models of the two windows, and showing one photo with its drawing on a canvas.

use super::{super::host::with_host, photo::Photo};
use crate::{
    CanvasMarker, CanvasPath, PhotoSlot, ReportRow,
    locate_io::{
        overlay::{Drawing, MarkerSpec, PATH_PREDICTED, PathSpec, path_commands},
        report::Row,
    },
};
use glam::DVec3;
use pleochroma_cut_core::rough_plan::{
    RoughBase,
    locate::{Rigid, ViewPose},
};
use slint::{Image, ModelRc, SharedString, VecModel};

/// A model of strings.
pub(super) fn strings_model(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// A model of report lines.
pub(super) fn rows_model(rows: Vec<Row>) -> ModelRc<ReportRow> {
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|row| ReportRow {
                text: row.text.into(),
                warn: row.warn,
            })
            .collect::<Vec<_>>(),
    ))
}

/// A model of photo slots.
pub(super) fn slots_model(slots: Vec<PhotoSlot>) -> ModelRc<PhotoSlot> {
    ModelRc::new(VecModel::from(slots))
}

fn marker(spec: MarkerSpec) -> CanvasMarker {
    CanvasMarker {
        fx: spec.fraction[0],
        fy: spec.fraction[1],
        kind: spec.kind,
        label: spec.label.into(),
    }
}

fn path(spec: PathSpec) -> CanvasPath {
    CanvasPath {
        commands: spec.commands.into(),
        kind: spec.kind,
    }
}

/// One slot of a window's photo list.
pub(super) fn slot(name: &str, photo: Option<&Photo>, status: String, active: bool) -> PhotoSlot {
    PhotoSlot {
        name: name.into(),
        file: photo.map(Photo::file_name).unwrap_or_default().into(),
        status: status.into(),
        active,
        loaded: photo.is_some(),
    }
}

/// The path of the rough's Crystal Axis in the photo of the camera `pose`: the line through
/// `centre` (the rough's centre, mesh frame, mm) along `direction`, `half_mm` to each side, moved
/// into the rig frame by `mesh_to_rig` and projected as the camera sees it, in the photo's own
/// pixels. `None` when an end is behind a pinhole camera.
#[must_use]
pub(super) fn axis_path(
    pose: &ViewPose,
    mesh_to_rig: &Rigid,
    centre: DVec3,
    direction: DVec3,
    half_mm: f64,
) -> Option<PathSpec> {
    let end = |sign: f64| pose.project(mesh_to_rig.to_rig(centre + direction * (sign * half_mm)));
    let (from, to) = (end(1.0)?, end(-1.0)?);
    Some(PathSpec {
        commands: path_commands(&[[from.x, from.y], [to.x, to.y]], false)?,
        kind: PATH_PREDICTED,
    })
}

/// `drawing` with the rough's Crystal Axis added when the planner's rough has one, the mesh was
/// aligned to the rig (`alignment`: the transform and the id of the mesh it was fitted to, which
/// must still be the planner's rough) and the camera of this view is known. Otherwise `drawing`
/// as it is.
#[must_use]
pub(super) fn with_axis_line(
    mut drawing: Drawing,
    pose: Option<&ViewPose>,
    alignment: Option<(Rigid, u64)>,
) -> Drawing {
    let (Some(pose), Some((transform, mesh_id))) = (pose, alignment) else {
        return drawing;
    };
    let rough = with_host(|planner| {
        // A planner busy with its own edit shows no axis for now.
        let session = planner.session.try_borrow().ok()?;
        let base = session.model.base;
        let (RoughBase::Hull { id, .. }, Some(orientation)) = (base, session.model.orientation)
        else {
            return None;
        };
        let diagonal = DVec3::from_array(base.bounding_box_extents()).length();
        Some((
            id,
            base.bounding_box_centre(),
            orientation.axis.direction(),
            diagonal,
        ))
    })
    .flatten();
    if let Some((id, centre, direction, diagonal)) = rough
        && id == mesh_id
        && let Some(path) = axis_path(pose, &transform, centre, direction, diagonal * 0.6)
    {
        drawing.1.push(path);
    }
    drawing
}

/// The properties of a canvas, set on whichever window shows it.
pub(super) struct CanvasView {
    /// The picture (empty without a photo).
    pub image: Image,
    /// The photo's own width, 1 without a photo.
    pub width: i32,
    /// The photo's own height, 1 without a photo.
    pub height: i32,
    /// Whether there is a photo.
    pub has_photo: bool,
    /// The markers over it.
    pub markers: ModelRc<CanvasMarker>,
    /// The paths over it.
    pub paths: ModelRc<CanvasPath>,
}

impl CanvasView {
    /// The canvas for `photo` (if any) with `drawing` over it.
    pub(super) fn new(photo: Option<&Photo>, drawing: Drawing) -> Self {
        let (markers, paths) = drawing;
        Self {
            image: photo.map(|p| p.image.clone()).unwrap_or_default(),
            width: photo.map_or(1, |p| i32::try_from(p.size[0]).unwrap_or(i32::MAX)),
            height: photo.map_or(1, |p| i32::try_from(p.size[1]).unwrap_or(i32::MAX)),
            has_photo: photo.is_some(),
            markers: ModelRc::new(VecModel::from(
                markers.into_iter().map(marker).collect::<Vec<_>>(),
            )),
            paths: ModelRc::new(VecModel::from(
                paths.into_iter().map(path).collect::<Vec<_>>(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::rough_plan::locate::Projection;

    fn camera() -> ViewPose {
        ViewPose::look_at(
            "front",
            DVec3::new(0.0, 0.0, 100.0),
            DVec3::ZERO,
            DVec3::Y,
            Projection::Orthographic { px_per_mm: 10.0 },
            [200, 200],
        )
    }

    #[test]
    fn the_axis_is_a_line_through_the_rough_as_the_camera_sees_it() {
        let path = axis_path(&camera(), &Rigid::IDENTITY, DVec3::ZERO, DVec3::X, 5.0)
            .expect("both ends are in view");
        assert_eq!(path.commands, "M 150.00 100.00 L 50.00 100.00");
        assert_eq!(path.kind, PATH_PREDICTED);
    }

    #[test]
    fn the_alignment_moves_the_rough_before_it_is_projected() {
        let shifted = Rigid {
            rotation: [0.0; 3],
            translation: [0.0, 2.0, 0.0],
        };
        let path = axis_path(&camera(), &shifted, DVec3::ZERO, DVec3::X, 5.0)
            .expect("both ends are in view");
        // 2 mm up is 20 pixels up the photo (y grows downwards).
        assert_eq!(path.commands, "M 150.00 80.00 L 50.00 80.00");
    }

    #[test]
    fn a_drawing_without_an_alignment_or_a_camera_is_left_as_it_is() {
        let drawing: Drawing = (Vec::new(), Vec::new());
        let same = with_axis_line(drawing.clone(), None, None);
        assert_eq!(same, drawing);
        let still = with_axis_line(drawing.clone(), Some(&camera()), None);
        assert_eq!(still, drawing);
    }
}
