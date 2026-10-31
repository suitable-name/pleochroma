//! The Crystal Axis in the planner's 3D view: the axis drawn through the rough, picking a face or
//! an edge of the rough for it, turning it by dragging its tip, and turning a stone by hand
//! (Free Orientation).
//!
//! - **Drawing.** With the model shown, the rough's Crystal Axis is a line through the rough with a
//!   handle at its tip. With a result shown, a short line through every stone of the result shows
//!   the axis the stones were planned with. Both are drawn into every frame ([`draw_overlay`]).
//! - **Picking.** While Align to Face or Align to Edge waits for a click ([`PickMode`]), a click
//!   on the rough finds the face or the point under it (a ray against the rough's planes) and sets
//!   the axis ([`click`]). The click is used up: it does not add a cut.
//! - **Turning.** A drag that starts on the tip turns the axis, live, and counts as one undo step
//!   when the button goes up ([`drag_begin`], [`drag_move`], [`drag_end`]). While a stone is being
//!   turned by hand, a drag in the result view turns the stone instead of the camera.
//!
//! The logic of the orientation is in `gui::rough_plan::axis` and the core; this file holds the
//! view's geometry (rays and planes, the screen position of the tip) and its tests.

use super::{
    Hover, axis_view,
    render::{Pixels, camera_for},
    request_render,
    scene::{SceneKind, WorldFrame},
};
use crate::{
    RoughPlanModel,
    gui::{
        rough_colour::handles::{
            HIT_RADIUS_PX, Marker, Overlay, Segment, ViewMap, draw_overlay as draw_handles,
        },
        rough_plan::{
            axis::{self, PickMode},
            host::Host,
        },
    },
};
use glam::DVec3;
use pleochroma_cut_core::{
    crystal_axis::CrystalOrientation,
    rough_plan::{
        RoughBase,
        orientation::{align_to_edge, align_to_face},
    },
};
use pleochroma_solid::preview::CameraPose;
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

/// The color of the rough's axis: the accent cyan.
const AXIS_COLOUR: [u8; 3] = [56, 189, 248];

/// The color of the axis through the stones of a result: amber.
const STONE_AXIS_COLOUR: [u8; 3] = [245, 158, 11];

/// How much longer than the rough's half diagonal the axis line runs on each side of the centre.
const AXIS_HALF_LENGTH: f64 = 0.6;

/// How long the line through a stone is, as a multiple of the stone's longest side (half on each
/// side of its centre).
const STONE_LINE: f64 = 1.4;

/// The hint over the axis handle.
const HANDLE_HINT: &str = "Drag to turn the crystal axis";

/// What a drag started on.
#[derive(Debug, Clone, Copy)]
enum Drag {
    /// The tip of the rough's axis: where it was when the pointer went down, and how far the
    /// pointer has travelled since (logical pixels).
    Axis { tip: DVec3, travel: (f32, f32) },
    /// A stone being turned by hand.
    Stone,
}

/// Whether the pointer is over the handle, and the drag under way.
#[derive(Default)]
struct Link {
    hot: bool,
    drag: Option<Drag>,
}

thread_local! {
    static LINK: RefCell<Link> = RefCell::default();
}

/// What the camera, the frame and the size of the view are, and which scene is shown.
#[derive(Clone, Copy)]
struct View {
    pose: CameraPose,
    size: (f32, f32),
    frame: WorldFrame,
    /// The rough is shown (not a result).
    model_shown: bool,
}

impl View {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the view size is a positive logical pixel count (view_data rejects sizes <= 0); the saturating cast bounds it"
    )]
    const fn pixels(&self) -> (u32, u32) {
        (self.size.0.round() as u32, self.size.1.round() as u32)
    }
}

/// The base and orientation of the rough the shown scene is for: the model's own while the model
/// is shown, the plan's while a result is.
fn shown_rough(host: &Host, model_shown: bool) -> Option<(RoughBase, Option<CrystalOrientation>)> {
    let session = host.session.borrow();
    if model_shown {
        Some((session.model.base, session.model.orientation))
    } else {
        let planned = session.run.plan_model.as_ref()?;
        Some((planned.base, planned.orientation))
    }
}

/// The view of the scene on screen, `None` while nothing is shown or the view has no size yet.
fn view_data(host: &Host) -> Option<(View, RoughBase, Option<CrystalOrientation>)> {
    let (pose, size, model_shown) = {
        let session = host.session.borrow();
        let view = &session.view;
        if view.view_size.0 <= 0.0 || view.view_size.1 <= 0.0 {
            return None;
        }
        let model_shown = match view.scene.as_deref().map(|scene| &scene.kind)? {
            SceneKind::Model(_) => true,
            SceneKind::Fit(_) => false,
        };
        (view.pose, view.view_size, model_shown)
    };
    let (base, orientation) = shown_rough(host, model_shown)?;
    Some((
        View {
            pose,
            size,
            frame: WorldFrame::for_base(&base),
            model_shown,
        },
        base,
        orientation,
    ))
}

/// The line the axis is drawn along in the rough frame: through the rough's centre, as long as the
/// rough, for the orientation `orientation`: its two ends, the upper one first.
#[must_use]
fn axis_ends(base: &RoughBase, orientation: &CrystalOrientation) -> (DVec3, DVec3) {
    let half = DVec3::from_array(base.bounding_box_extents()).length() * AXIS_HALF_LENGTH;
    let centre = base.bounding_box_centre();
    let direction = orientation.axis.direction();
    (centre + direction * half, centre - direction * half)
}

/// The axis direction after the tip, which was at `tip`, was dragged: from the centre `centre` to
/// the tip's new place, `None` when the map is degenerate or the tip lands on the centre.
#[must_use]
fn dragged_direction(
    map: &ViewMap<'_>,
    centre: DVec3,
    tip: DVec3,
    travel: (f32, f32),
) -> Option<DVec3> {
    let right = map.camera.right.as_dvec3();
    let up = map.camera.up.as_dvec3();
    let probe = (tip - centre).length().max(1e-3) * 0.1;
    let here = map.screen(tip)?;
    let along = map.screen(tip + right * probe)?;
    let pixels_per_mm = f64::from((along.0 - here.0).hypot(along.1 - here.1)) / probe;
    if pixels_per_mm < 1e-9 {
        return None;
    }
    // Screen y grows downwards, the camera's up vector upwards.
    let moved =
        right * (f64::from(travel.0) / pixels_per_mm) - up * (f64::from(travel.1) / pixels_per_mm);
    let direction = tip + moved - centre;
    (direction.length() > 1e-9).then_some(direction)
}

/// Where a ray `origin + t * dir` first enters the convex solid `n . p <= d` of `planes`, and the
/// outward normal of the plane it enters through. `None` when the ray misses the solid or leaves it
/// behind the origin.
#[must_use]
fn first_hit(planes: &[(DVec3, f64)], origin: DVec3, dir: DVec3) -> Option<(f64, DVec3)> {
    let mut enter = f64::NEG_INFINITY;
    let mut exit = f64::INFINITY;
    let mut normal = None;
    for &(n, d) in planes {
        let slope = n.dot(dir);
        let room = d - n.dot(origin);
        if slope.abs() < 1e-12 {
            // Parallel to the plane: outside it, the ray never gets in.
            if room < 0.0 {
                return None;
            }
            continue;
        }
        let t = room / slope;
        if slope < 0.0 {
            if t > enter {
                enter = t;
                normal = Some(n);
            }
        } else if t < exit {
            exit = t;
        }
    }
    (enter <= exit && exit >= 0.0)
        .then_some(normal)
        .flatten()
        .map(|n| (enter, n))
}

/// The point of the rough under `(x, y)` (logical pixels) and the normal of the face there, in the
/// rough frame. Only the model view picks.
fn pick_point(host: &Host, x: f32, y: f32) -> Option<(DVec3, DVec3)> {
    let ray = super::zoning_view::mesh_ray(host, x, y)?;
    let planes = host.session.borrow().model.halfspaces().ok()?;
    let (t, normal) = first_hit(&planes, ray.origin, ray.dir)?;
    Some((ray.origin + ray.dir * t, normal))
}

/// The tip of the rough's axis under the point `cursor`, with the data it was found with: the
/// model is shown, the axis is set, the pointer is over the tip and no pick waits for a click.
fn tip_under(host: &Host, cursor: (f32, f32)) -> Option<(View, DVec3, DVec3)> {
    if !matches!(axis::pick_mode(host), PickMode::Off) {
        return None;
    }
    let (view, base, orientation) = view_data(host)?;
    if !view.model_shown {
        return None;
    }
    let (tip, _) = axis_ends(&base, &orientation?);
    let camera = camera_for(view.pose);
    let map = ViewMap {
        camera: &camera,
        size: view.pixels(),
        centre: view.frame.centre,
        scale: view.frame.scale,
    };
    let at = map.screen(tip)?;
    let near = (at.0 - cursor.0).hypot(at.1 - cursor.1) <= HIT_RADIUS_PX;
    near.then_some((view, tip, base.bounding_box_centre()))
}

/// The pointer moved over the image. Lights the handle under it, words what a click does while a
/// pick waits, and returns whether the view leaves its own hover alone.
pub(in crate::gui::rough_plan) fn hover(host: &Rc<Host>, x: f32, y: f32) -> bool {
    if LINK.with(|link| link.borrow().drag.is_some()) {
        return true;
    }
    let model = host.window.global::<RoughPlanModel>();
    let pick = axis::pick_mode(host);
    if pick.is_on() && view_data(host).is_some_and(|(view, _, _)| view.model_shown) {
        let hint = match pick {
            PickMode::Face => "Click a face of the rough to set the crystal axis along it",
            PickMode::EdgeFirst => "Click the first end of the edge",
            _ => "Click the second end of the edge",
        };
        if model.get_view_hint().as_str() != hint {
            model.set_view_hint(hint.into());
        }
        let changed = {
            let mut session = host.session.borrow_mut();
            let changed = session.view.hover != Hover::None;
            session.view.hover = Hover::None;
            changed
        };
        if changed {
            request_render(host);
        }
        return true;
    }
    let over = (x >= 0.0 && y >= 0.0) && tip_under(host, (x, y)).is_some();
    let changed = LINK.with(|link| {
        let mut link = link.borrow_mut();
        let changed = link.hot != over;
        link.hot = over;
        changed
    });
    if over && model.get_view_hint().as_str() != HANDLE_HINT {
        model.set_view_hint(HANDLE_HINT.into());
    }
    if changed {
        request_render(host);
    }
    over
}

/// A click at `(x, y)`: used up when a pick waits for it (it sets the axis along the face or the
/// edge clicked, or says there is nothing there).
pub(in crate::gui::rough_plan) fn click(host: &Rc<Host>, x: f32, y: f32) -> bool {
    let pick = axis::pick_mode(host);
    if !pick.is_on() {
        return false;
    }
    if !view_data(host).is_some_and(|(view, _, _)| view.model_shown) {
        return false;
    }
    let second = host
        .session
        .borrow()
        .model
        .orientation
        .and_then(|orientation| orientation.second_turn_deg);
    let Some((point, normal)) = pick_point(host, x, y) else {
        host.session.borrow_mut().axis.note = Some((
            "The click missed the rough. Click on the rough itself.".to_string(),
            2,
        ));
        axis::set_pick(host, pick);
        return true;
    };
    match pick {
        PickMode::Face => match align_to_face(normal.to_array(), second) {
            Some(orientation) => axis::apply_orientation(
                host,
                Some(orientation),
                Some((
                    "The crystal axis follows the face you clicked.".to_string(),
                    1,
                )),
            ),
            None => axis::set_pick(host, PickMode::Face),
        },
        PickMode::EdgeFirst => {
            host.session.borrow_mut().axis.note =
                Some(("Click the second end of the edge.".to_string(), 0));
            axis::set_pick(host, PickMode::EdgeSecond(point.to_array()));
        }
        PickMode::EdgeSecond(first) => {
            if let Some(orientation) = align_to_edge(first, point.to_array(), second) {
                axis::apply_orientation(
                    host,
                    Some(orientation),
                    Some((
                        "The crystal axis follows the line you clicked.".to_string(),
                        1,
                    )),
                );
            } else {
                host.session.borrow_mut().axis.note = Some((
                    "The two ends are the same point. Click another point.".to_string(),
                    2,
                ));
                axis::set_pick(host, PickMode::EdgeSecond(first));
            }
        }
        PickMode::Off => {}
    }
    true
}

/// The pointer went down on the image: a drag that starts on the axis tip turns the axis, and while
/// a stone is being turned by hand a drag in the result view turns the stone.
pub(in crate::gui::rough_plan) fn drag_begin(host: &Rc<Host>) {
    let pointer = host.session.borrow().view.pointer;
    let start = pointer
        .and_then(|cursor| tip_under(host, cursor))
        .map(|(_, tip, _)| Drag::Axis {
            tip,
            travel: (0.0, 0.0),
        })
        .or_else(|| {
            let fit_shown = view_data(host).is_some_and(|(view, _, _)| !view.model_shown);
            (fit_shown && axis::is_turning(host)).then_some(Drag::Stone)
        });
    LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.hot = matches!(start, Some(Drag::Axis { .. }));
        link.drag = start;
    });
}

/// The pointer moved by `(dx, dy)` with the button held. Returns whether a drag of the axis or of
/// a stone used it (the camera then stays where it is).
pub(in crate::gui::rough_plan) fn drag_move(host: &Rc<Host>, dx: f32, dy: f32) -> bool {
    let step = LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.drag.as_mut().map(|drag| {
            if let Drag::Axis { travel, .. } = drag {
                travel.0 += dx;
                travel.1 += dy;
            }
            *drag
        })
    });
    let Some(step) = step else {
        return false;
    };
    let Some((view, base, orientation)) = view_data(host) else {
        return true;
    };
    let camera = camera_for(view.pose);
    match step {
        Drag::Stone => {
            let (right, up) = (camera.right.as_dvec3(), camera.up.as_dvec3());
            if let Some(result) = axis::turn(host, right, up, dx, dy) {
                axis_view::redraw_result(host, result);
            }
        }
        Drag::Axis { tip, travel } => {
            let map = ViewMap {
                camera: &camera,
                size: view.pixels(),
                centre: view.frame.centre,
                scale: view.frame.scale,
            };
            let second = orientation.and_then(|orientation| orientation.second_turn_deg);
            if let Some(direction) =
                dragged_direction(&map, base.bounding_box_centre(), tip, travel)
                && let Some(next) = align_to_face(direction.to_array(), second)
            {
                axis::set_live(host, next);
            }
        }
    }
    true
}

/// The button went up: a drag of the axis or of a stone ends and counts as one change.
pub(in crate::gui::rough_plan) fn drag_end(host: &Rc<Host>) {
    let ended = LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.hot = false;
        link.drag.take()
    });
    match ended {
        Some(Drag::Axis { .. }) => axis::finish_live(host),
        Some(Drag::Stone) => axis::finish_turn(host),
        None => return,
    }
    request_render(host);
}

/// The overlay of the line through `from` and `to` (rough frame) with an optional marker at `to`.
fn line_overlay(map: &ViewMap<'_>, from: DVec3, to: DVec3, marker: Option<bool>) -> Overlay {
    let mut overlay = Overlay::default();
    if let (Some(a), Some(b)) = (map.screen(from), map.screen(to)) {
        overlay.segments.push(Segment { from: a, to: b });
        if let Some(lit) = marker {
            overlay.markers.push(Marker { at: b, lit });
        }
    }
    overlay
}

/// Draws the Crystal Axis into a finished frame: the rough's axis with its handle while the model
/// is shown, a line through every stone while a result is.
pub(in crate::gui::rough_plan) fn draw_overlay(host: &Host, pixels: &mut Pixels) {
    let Some((view, base, Some(orientation))) = view_data(host) else {
        return;
    };
    let camera = camera_for(view.pose);
    let map = ViewMap {
        camera: &camera,
        size: view.pixels(),
        centre: view.frame.centre,
        scale: view.frame.scale,
    };
    let (width, height) = (pixels.width(), pixels.height());
    let scale = width as f32 / view.size.0;
    if view.model_shown {
        let (tip, tail) = axis_ends(&base, &orientation);
        let lit = LINK.with(|link| link.borrow().hot || link.borrow().drag.is_some());
        let overlay = line_overlay(&map, tail, tip, Some(lit));
        draw_handles(
            pixels.make_mut_bytes(),
            width,
            height,
            &overlay,
            AXIS_COLOUR,
            scale,
        );
        return;
    }
    let direction = orientation.axis.direction();
    let mut stones = Overlay::default();
    {
        let session = host.session.borrow();
        let selected =
            usize::try_from(host.window.global::<RoughPlanModel>().get_selected_result()).ok();
        let Some(layout) = selected.and_then(|index| session.run.layouts.get(index)) else {
            return;
        };
        for stone in &layout.stones {
            let half = stone.stone_size_mm.into_iter().fold(0.0_f64, f64::max) * STONE_LINE * 0.5;
            let centre = DVec3::from_array(stone.pose.center_mm);
            let line = line_overlay(
                &map,
                centre - direction * half,
                centre + direction * half,
                None,
            );
            stones.segments.extend(line.segments);
        }
    }
    draw_handles(
        pixels.make_mut_bytes(),
        width,
        height,
        &stones,
        STONE_AXIS_COLOUR,
        scale,
    );
}

#[cfg(test)]
mod tests {
    use super::{super::render::reset_pose, *};

    fn unit_box() -> Vec<(DVec3, f64)> {
        vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 1.0),
            (DVec3::NEG_Y, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ]
    }

    #[test]
    fn a_ray_enters_a_box_through_the_face_it_meets_first() {
        let (t, normal) =
            first_hit(&unit_box(), DVec3::new(5.0, 0.2, 0.3), DVec3::NEG_X).expect("a hit");
        assert!((t - 4.0).abs() < 1e-12);
        assert_eq!(normal, DVec3::X);
        let (t, normal) =
            first_hit(&unit_box(), DVec3::new(0.0, 7.0, 0.0), DVec3::NEG_Y).expect("a hit");
        assert!((t - 6.0).abs() < 1e-12);
        assert_eq!(normal, DVec3::Y);
    }

    #[test]
    fn a_ray_that_misses_the_box_or_points_away_hits_nothing() {
        assert!(first_hit(&unit_box(), DVec3::new(5.0, 3.0, 0.0), DVec3::NEG_X).is_none());
        assert!(first_hit(&unit_box(), DVec3::new(5.0, 0.0, 0.0), DVec3::X).is_none());
        // Parallel to a face and outside it.
        assert!(first_hit(&unit_box(), DVec3::new(5.0, 2.0, 0.0), DVec3::NEG_X).is_none());
    }

    #[test]
    fn a_slanted_ray_enters_through_the_face_it_reaches_first() {
        let origin = DVec3::new(4.0, 0.5, 0.5);
        let dir = DVec3::new(-1.0, -0.05, 0.0).normalize();
        let (t, normal) = first_hit(&unit_box(), origin, dir).expect("a hit");
        assert_eq!(normal, DVec3::X);
        let point = origin + dir * t;
        assert!((point.x - 1.0).abs() < 1e-9, "{point:?}");
    }

    #[test]
    fn the_axis_runs_through_the_centre_of_the_rough_along_its_direction() {
        let base = RoughBase::Block {
            x_mm: 10.0,
            y_mm: 6.0,
            z_mm: 8.0,
        };
        let orientation = CrystalOrientation::from_angles(90.0, 0.0, None);
        let (tip, tail) = axis_ends(&base, &orientation);
        let centre = base.bounding_box_centre();
        assert!(((tip + tail) / 2.0 - centre).length() < 1e-9);
        assert!((tip - tail).normalize().dot(DVec3::X).abs() > 1.0 - 1e-9);
        // 0.6 of the half diagonal each side.
        let diagonal = DVec3::new(10.0, 6.0, 8.0).length();
        assert!(diagonal.mul_add(-1.2, (tip - tail).length()).abs() < 1e-9);
    }

    #[test]
    fn dragging_the_tip_to_the_right_turns_the_axis_toward_the_cameras_right() {
        let camera = camera_for(reset_pose(1.5));
        let map = ViewMap {
            camera: &camera,
            size: (800, 600),
            centre: DVec3::ZERO,
            scale: 0.1,
        };
        let centre = DVec3::ZERO;
        let tip = DVec3::new(0.0, 8.0, 0.0);
        let right = camera.right.as_dvec3();
        let same = dragged_direction(&map, centre, tip, (0.0, 0.0)).expect("a direction");
        assert!((same.normalize() - DVec3::Y).length() < 1e-6, "{same:?}");
        let moved = dragged_direction(&map, centre, tip, (80.0, 0.0)).expect("a direction");
        assert!(moved.normalize().dot(right) > 0.0, "{moved:?}");
        let other = dragged_direction(&map, centre, tip, (-80.0, 0.0)).expect("a direction");
        assert!(other.normalize().dot(right) < 0.0, "{other:?}");
    }
}
