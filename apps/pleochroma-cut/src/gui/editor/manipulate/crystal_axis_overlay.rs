//! The Crystal Axis drawn over the stone, with its drag handle(s) (`CrystalAxisModel`).
//!
//! The axis is a line through the stone's centre (the model origin), as long as 60 percent of
//! the shown stone's bounding radius on each side, with a handle at its upper end; a biaxial
//! stone also shows the Second Axis. It is drawn in the Solid, Path-Traced, Ray-Traced and Both
//! views (the solid raster's camera, which the path tracer shares) and on the Diagram's profile
//! panel. Dragging a handle edits the Stone-tab controls live (the same path as the sliders: one
//! debounced commit, one undo step) and Escape puts the axis back.
//!
//! All positions go to the model in the viewport's LOGICAL coordinates (UNZOOMED in the Diagram
//! view, like the facet handles).

use super::{Shared, handles::CAMERA_FOV_DEG, set_hint};
use crate::{
    CrystalAxisModel, MainWindow, ManipulateModel, RenderEffectsModel, SettingsModel, SilkLine,
    gui::{
        editor::callbacks::{map_to_pick_coordinates, pick_margin, pick_pixels_to_logical},
        optics::c_axis::{angles_to_c_axis, c_axis_to_angles},
        render::{
            crystal_axis_ui::{self, EffectiveAxes},
            physics_effects_ui,
        },
        solid_preview::{diagram2d::PanelKind, preview_state::FrameGeometry},
        tutorial_events::raise,
    },
};
use glam::Vec3;
use pleochroma::optics::{birefringence::alpha_axis_from_turn, raytracer::Camera};
use pleochroma_editor::{
    guide::viewing_events::{CRYSTAL_AXIS_DRAG_CANCELLED, CRYSTAL_AXIS_DRAGGED},
    manipulate::{HANDLE_HIT_RADIUS_PX, ScreenPoint, ScreenSize, project, unproject},
};
use slint::{ComponentHandle as _, ModelRc, VecModel};
use std::cell::RefCell;

/// Each axis line reaches this fraction of the bounding radius on both sides of the centre.
const LENGTH_FRACTION: f32 = 0.6;

/// A silk direction line is this fraction of the axis line's length.
const SILK_LENGTH_FRACTION: f32 = 0.7;

/// How one frame turns a model point into a pick-frame pixel (the Show Surface Finish overlay,
/// `surface_overlay`, places its marks with it too).
pub(super) enum View {
    /// A 3D view: the solid raster's camera.
    Camera(Camera, ScreenSize),
    /// The Diagram's profile panel: `x` to the right, `y` up, `scale` pixels per model unit.
    Profile { center: (f32, f32), scale: f32 },
}

impl View {
    pub(super) fn project(&self, p: Vec3) -> Option<(f32, f32)> {
        match self {
            Self::Camera(camera, size) => project(camera, p, *size).map(|s| (s.x, s.y)),
            Self::Profile { center, scale } => {
                Some((center.0 + p.x * scale, center.1 - p.y * scale))
            }
        }
    }
}

/// The view of `geometry` and the axis length in model units, or `None` while no frame (or no
/// profile panel) is there.
pub(super) fn view_of(geometry: &FrameGeometry) -> Option<(View, f32)> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the bounding radius is a small model-unit length; the camera math is f32"
    )]
    let length = LENGTH_FRACTION * geometry.bounding_radius as f32;
    if !(length.is_finite() && length > 0.0) {
        return None;
    }
    if let Some(layout) = geometry.diagram.as_deref() {
        let panel = layout.panel(PanelKind::Profile)?;
        return Some((
            View::Profile {
                center: (panel.center_x, panel.center_y),
                scale: panel.scale,
            },
            length,
        ));
    }
    let pose = geometry.camera;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a frame size in pixels is far below f32's exact-integer range"
    )]
    let size = ScreenSize::new(geometry.size.0 as f32, geometry.size.1 as f32);
    Some((
        View::Camera(
            Camera::new(pose.yaw, pose.pitch, pose.distance, CAMERA_FOV_DEG).with_roll(pose.roll),
            size,
        ),
        length,
    ))
}

/// A line and its handle in logical coordinates.
struct Placed {
    from: (f32, f32),
    to: (f32, f32),
    tip: (f32, f32),
}

/// `direction` (either sign) as a line through the origin, its handle on the upper end.
fn place_line(ui: &MainWindow, view: &View, length: f32, direction: Vec3) -> Option<Placed> {
    let d = direction.try_normalize()?;
    let d = if d.y < 0.0 { -d } else { d };
    let scale = ui.window().scale_factor();
    let margin = pick_margin(ui);
    let logical = |p: Vec3| {
        view.project(p)
            .map(|px| pick_pixels_to_logical(px, scale, margin))
    };
    Some(Placed {
        from: logical(-d * length)?,
        to: logical(d * length)?,
        tip: logical(d * length)?,
    })
}

/// What a drag started from, to put back on Escape.
#[derive(Clone, Copy)]
struct DragStart {
    kind: i32,
    enabled: bool,
    tilt: f32,
    azimuth: f32,
    second_set: bool,
    second_deg: f32,
    /// The handle was on the near side of the stone at the press (3D views).
    near: bool,
}

thread_local! {
    static DRAG: RefCell<Option<DragStart>> = const { RefCell::new(None) };
}

/// The current view, axes and length, if the overlay applies.
fn current(ui: &MainWindow, ctx: &Shared) -> Option<(View, f32, EffectiveAxes)> {
    if !ui.global::<CrystalAxisModel>().get_show_axis() {
        return None;
    }
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let material = {
        let render = crate::bridge::render_thread::RenderContext::lock(&ctx.render_ctx);
        crystal_axis_ui::resolved_material(&render)?
    };
    let axes = crystal_axis_ui::effective_axes(ui, &material)?;
    let (view, length) = view_of(&geometry)?;
    Some((view, length, axes))
}

/// Recomputes the overlay and the Stone tab's derived state (a frame landed, or an input of the
/// Crystal Axis changed).
pub(super) fn refresh(ui: &MainWindow, ctx: &Shared) {
    super::live_crystal_axis::refresh(ui, ctx);
    crystal_axis_ui::refresh_section(ui, &ctx.render_ctx);
    physics_effects_ui::refresh(ui, &ctx.render_ctx);
    let model = ui.global::<CrystalAxisModel>();
    let placed = current(ui, ctx).and_then(|(view, length, axes)| {
        let axis = place_line(ui, &view, length, axes.gamma)?;
        let second = axes
            .alpha
            .and_then(|alpha| place_line(ui, &view, length * 0.8, alpha));
        // The silk directions ride this overlay (`RenderEffectsModel.show_silk_directions`).
        let silk: Vec<SilkLine> = physics_effects_ui::silk_directions(ui, &ctx.render_ctx)
            .into_iter()
            .filter_map(|dir| place_line(ui, &view, length * SILK_LENGTH_FRACTION, dir))
            .map(|line| SilkLine {
                x0: line.from.0,
                y0: line.from.1,
                x1: line.to.0,
                y1: line.to.1,
            })
            .collect();
        Some((axis, second, silk))
    });
    let Some((axis, second, silk)) = placed else {
        model.set_overlay_visible(false);
        model.set_second_visible(false);
        ui.global::<RenderEffectsModel>()
            .set_silk_lines(ModelRc::default());
        return;
    };
    ui.global::<RenderEffectsModel>()
        .set_silk_lines(ModelRc::new(VecModel::from(silk)));
    model.set_axis_x0(axis.from.0);
    model.set_axis_y0(axis.from.1);
    model.set_axis_x1(axis.to.0);
    model.set_axis_y1(axis.to.1);
    model.set_axis_tip_x(axis.tip.0);
    model.set_axis_tip_y(axis.tip.1);
    if let Some(second) = &second {
        model.set_second_x0(second.from.0);
        model.set_second_y0(second.from.1);
        model.set_second_x1(second.to.0);
        model.set_second_y1(second.to.1);
        model.set_second_tip_x(second.tip.0);
        model.set_second_tip_y(second.tip.1);
    }
    model.set_second_visible(second.is_some());
    model.set_overlay_visible(true);
}

/// The handle under the logical pointer `(x, y)`: 0 the Crystal Axis, 1 the Second Axis.
fn hit(ui: &MainWindow, x: f32, y: f32) -> i32 {
    let model = ui.global::<CrystalAxisModel>();
    if !model.get_overlay_visible() {
        return -1;
    }
    // The tips are logical coordinates and a pick-frame pixel is `scale` per logical pixel, so
    // the facet handles' radius is `HANDLE_HIT_RADIUS_PX * handle_scale` logical pixels.
    let radius = HANDLE_HIT_RADIUS_PX * ui.global::<ManipulateModel>().get_handle_scale();
    let near = |tx: f32, ty: f32| (tx - x).hypot(ty - y) <= radius;
    if near(model.get_axis_tip_x(), model.get_axis_tip_y()) {
        0
    } else if model.get_second_visible() && near(model.get_second_tip_x(), model.get_second_tip_y())
    {
        1
    } else {
        -1
    }
}

/// `CrystalAxisModel.hit_test`.
pub(super) fn hit_test(ui: &MainWindow, x: f32, y: f32) -> i32 {
    hit(ui, x, y)
}

/// `CrystalAxisModel.hover`.
pub(super) fn hover(ui: &MainWindow, x: f32, y: f32) {
    if ui.global::<CrystalAxisModel>().get_dragging() {
        return;
    }
    let kind = hit(ui, x, y);
    let model = ui.global::<CrystalAxisModel>();
    if model.get_hovered() != kind {
        model.set_hovered(kind);
        let text = match kind {
            0 => "Drag to turn the crystal axis. Escape puts it back.",
            1 => "Drag to turn the Second Axis around the crystal axis. Escape puts it back.",
            _ => "",
        };
        set_hint(ui, text);
    }
}

/// `CrystalAxisModel.drag_begin`.
pub(super) fn drag_begin(ui: &MainWindow, ctx: &Shared, kind: i32) {
    let Some((view, length, axes)) = current(ui, ctx) else {
        return;
    };
    let settings = ui.global::<SettingsModel>();
    let axis_model = ui.global::<CrystalAxisModel>();
    // The handle's side: nearer to the camera than the centre?
    let near = match &view {
        View::Camera(camera, _) => {
            let tip = if kind == 1 {
                axes.alpha.unwrap_or(axes.gamma)
            } else {
                axes.gamma
            };
            let tip = if tip.y < 0.0 { -tip } else { tip };
            (tip * length - camera.origin).dot(camera.forward)
                < (-camera.origin).dot(camera.forward)
        }
        View::Profile { .. } => true,
    };
    // Dragging either handle turns "Set for This Stone" on, seeded from what is shown. The axis
    // is a line, so its upper end gives the angles the controls hold (tilt is at most 90).
    let upper = if axes.gamma.y < 0.0 {
        -axes.gamma
    } else {
        axes.gamma
    };
    let (tilt, azimuth) = c_axis_to_angles(upper);
    let start = DragStart {
        kind,
        enabled: settings.get_c_axis_override_enabled(),
        tilt: settings.get_c_axis_tilt_deg(),
        azimuth: settings.get_c_axis_azimuth_deg(),
        second_set: axis_model.get_second_set(),
        second_deg: axis_model.get_second_deg(),
        near,
    };
    if !start.enabled {
        settings.set_c_axis_override_enabled(true);
        settings.set_c_axis_tilt_deg(tilt);
        settings.set_c_axis_azimuth_deg(azimuth);
        // The stone keeps looking as it did: a Second Axis the material has of its own (not
        // just the completion of the Crystal Axis) is copied into the turn control.
        let seeded = angles_to_c_axis(tilt.clamp(0.0, 90.0), azimuth.clamp(0.0, 360.0));
        if let Some(turn) = axes
            .alpha
            .and_then(|alpha| second_turn_deg(seeded, alpha))
            .filter(|turn| *turn > MIN_COPIED_TURN_DEG)
        {
            axis_model.set_second_set(true);
            axis_model.set_second_deg(turn);
        }
        // One commit, so one undo step, carries the switch-on and every move of the gesture
        // (a press without a move still leaves the controls and the render in step).
        crystal_axis_ui::apply_controls(ui, &ctx.render_ctx);
    }
    DRAG.with(|cell| *cell.borrow_mut() = Some(start));
    axis_model.set_hovered(kind);
    axis_model.set_dragging(true);
}

/// The unit direction the pointer (logical `(x, y)`) asks for, on the sphere the handle moves on.
#[expect(
    clippy::many_single_char_names,
    reason = "pointer and sphere coordinates use their geometric names"
)]
fn direction_at(
    ui: &MainWindow,
    view: &View,
    length: f32,
    start: &DragStart,
    current_gamma: Vec3,
    x: f32,
    y: f32,
) -> Option<Vec3> {
    let (px, py) = map_to_pick_coordinates(ui, x, y);
    match view {
        View::Camera(camera, size) => {
            let ray = unproject(camera, ScreenPoint::new(px, py), *size);
            // Ray against the sphere of radius `length` about the origin.
            let b = ray.origin.dot(ray.dir);
            let c = length.mul_add(-length, ray.origin.length_squared());
            let disc = b.mul_add(b, -c);
            if disc < 0.0 {
                // Missed: the point of the ray nearest the centre, scaled onto the sphere.
                let nearest = ray.origin - ray.dir * b;
                return nearest.try_normalize();
            }
            let root = disc.sqrt();
            let hits = [(-b - root, true), (-b + root, false)];
            let points: Vec<(Vec3, bool)> = hits
                .iter()
                .map(|(t, is_near)| ((ray.origin + ray.dir * *t) / length, *is_near))
                .collect();
            // The handle is the upper end of a line: prefer a hit with y >= 0, and on the side
            // it started on.
            let upper: Vec<&(Vec3, bool)> = points.iter().filter(|(p, _)| p.y >= 0.0).collect();
            let pool: Vec<&(Vec3, bool)> = if upper.is_empty() {
                points.iter().collect()
            } else {
                upper
            };
            pool.iter()
                .find(|(_, is_near)| *is_near == start.near)
                .or_else(|| pool.first())
                .map(|(p, _)| *p)
        }
        View::Profile {
            center,
            scale: panel_scale,
        } => {
            let u = (px - center.0) / (panel_scale * length);
            let v = (center.1 - py) / (panel_scale * length);
            let flat = u.mul_add(u, v * v).min(1.0);
            let z = (1.0 - flat).sqrt() * if current_gamma.z < 0.0 { -1.0 } else { 1.0 };
            let planar = Vec3::new(u, v, z);
            planar.try_normalize()
        }
    }
}

/// A copied Second Axis turn at or below this many degrees (or within it of a half turn) is the
/// material's plain completion, which the controls' "not set" already stands for.
const MIN_COPIED_TURN_DEG: f32 = 0.01;

/// The Second Axis turn, degrees in `[0, 180)`, that brings the completion of `gamma` (turn
/// zero) onto `direction`, seen in the plane perpendicular to `gamma`; `None` when `direction`
/// lies along `gamma` and names no direction in that plane. A turn within
/// [`MIN_COPIED_TURN_DEG`] of a half turn reads as zero (the second axis is a line).
fn second_turn_deg(gamma: Vec3, direction: Vec3) -> Option<f32> {
    let reference = alpha_axis_from_turn(gamma, 0.0);
    // A pointer (nearly) along the Crystal Axis names no direction in the plane: the rounding
    // residue of removing the axis component must not count as one.
    let in_plane = direction - gamma * direction.dot(gamma);
    if in_plane.length_squared() <= 1e-8 * direction.length_squared() {
        return None;
    }
    let v = in_plane.try_normalize()?;
    let turn = gamma
        .dot(reference.cross(v))
        .atan2(reference.dot(v))
        .to_degrees()
        .rem_euclid(180.0);
    Some(if turn >= 180.0 - MIN_COPIED_TURN_DEG {
        0.0
    } else {
        turn
    })
}

/// `CrystalAxisModel.drag_move`.
pub(super) fn drag_move(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) {
    let Some(start) = DRAG.with(|cell| *cell.borrow()) else {
        return;
    };
    let Some((view, length, axes)) = current(ui, ctx) else {
        return;
    };
    let Some(dir) = direction_at(ui, &view, length, &start, axes.gamma, x, y) else {
        return;
    };
    let settings = ui.global::<SettingsModel>();
    let axis_model = ui.global::<CrystalAxisModel>();
    if start.kind == 0 {
        let (tilt, azimuth) = c_axis_to_angles(if dir.y < 0.0 { -dir } else { dir });
        settings.set_c_axis_tilt_deg(tilt.clamp(0.0, 90.0));
        settings.set_c_axis_azimuth_deg(azimuth.clamp(0.0, 360.0));
    } else {
        // The Second Axis: the turn that brings the material's completion onto the pointer's
        // direction, seen in the plane perpendicular to the Crystal Axis.
        let Some(turn) = second_turn_deg(axes.gamma, dir) else {
            return;
        };
        axis_model.set_second_set(true);
        axis_model.set_second_deg(turn);
    }
    crystal_axis_ui::apply_controls(ui, &ctx.render_ctx);
    refresh(ui, ctx);
}

/// Ends the gesture; `keep` is `false` for Escape, which puts the controls back.
fn finish(ui: &MainWindow, ctx: &Shared, keep: bool) {
    let Some(start) = DRAG.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    let axis_model = ui.global::<CrystalAxisModel>();
    axis_model.set_dragging(false);
    axis_model.set_hovered(-1);
    if keep {
        // A tutorial step may wait for a turn that was kept.
        raise(ui, CRYSTAL_AXIS_DRAGGED);
        return;
    }
    let settings = ui.global::<SettingsModel>();
    settings.set_c_axis_override_enabled(start.enabled);
    settings.set_c_axis_tilt_deg(start.tilt);
    settings.set_c_axis_azimuth_deg(start.azimuth);
    axis_model.set_second_set(start.second_set);
    axis_model.set_second_deg(start.second_deg);
    crystal_axis_ui::apply_controls(ui, &ctx.render_ctx);
    refresh(ui, ctx);
    raise(ui, CRYSTAL_AXIS_DRAG_CANCELLED);
}

/// `CrystalAxisModel.drag_end`.
pub(super) fn drag_end(ui: &MainWindow, ctx: &Shared) {
    finish(ui, ctx, true);
}

/// `CrystalAxisModel.drag_cancel`.
pub(super) fn drag_cancel(ui: &MainWindow, ctx: &Shared) {
    finish(ui, ctx, false);
}

/// Wires the overlay's `CrystalAxisModel` callbacks. Called from `setup_manipulate_callbacks`.
pub(super) fn setup(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<CrystalAxisModel>();
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_live_overlay_changed(move || {
        if let Some(ui) = weak.upgrade() {
            super::live_crystal_axis::refresh(&ui, &shared);
        }
    });
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_overlay_changed(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &shared);
        }
    });
    let weak = ui.as_weak();
    model.on_hit_test(move |x, y| weak.upgrade().map_or(-1, |ui| hit_test(&ui, x, y)));
    let weak = ui.as_weak();
    model.on_hover(move |x, y| {
        if let Some(ui) = weak.upgrade() {
            hover(&ui, x, y);
        }
    });
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_drag_begin(move |kind, _x, _y| {
        if let Some(ui) = weak.upgrade() {
            drag_begin(&ui, &shared, kind);
        }
    });
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_drag_move(move |x, y| {
        if let Some(ui) = weak.upgrade() {
            drag_move(&ui, &shared, x, y);
        }
    });
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_drag_end(move || {
        if let Some(ui) = weak.upgrade() {
            drag_end(&ui, &shared);
        }
    });
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    model.on_drag_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            drag_cancel(&ui, &shared);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_view_puts_up_above_the_centre() {
        let view = View::Profile {
            center: (100.0, 80.0),
            scale: 10.0,
        };
        assert_eq!(view.project(Vec3::new(1.0, 2.0, 5.0)), Some((110.0, 60.0)));
        assert_eq!(view.project(Vec3::ZERO), Some((100.0, 80.0)));
    }

    /// The turn a pointer direction asks for is the right-handed turn about the Crystal Axis from
    /// the material's completion, and `alpha_axis_from_turn` puts the Second Axis back on it.
    #[test]
    fn the_second_turn_of_a_direction_is_the_turn_that_reaches_it() {
        for gamma in [
            Vec3::Y,
            Vec3::new(0.3, 0.8, -0.2).normalize(),
            Vec3::new(1.0, 0.1, 0.0).normalize(),
        ] {
            let reference = alpha_axis_from_turn(gamma, 0.0);
            assert!(
                second_turn_deg(gamma, reference).is_some_and(|turn| turn.abs() < 1e-2),
                "{gamma:?}"
            );
            for turn in [10.0_f32, 40.0, 90.0, 150.0] {
                let wanted = alpha_axis_from_turn(gamma, turn);
                let found = second_turn_deg(gamma, wanted).expect("a direction in the plane");
                assert!((found - turn).abs() < 1e-2, "{gamma:?}: {turn} vs {found}");
                // The Second Axis is a line: the other end of it asks for the same turn.
                let flipped = second_turn_deg(gamma, -wanted).expect("a direction in the plane");
                assert!(
                    (flipped - turn).abs() < 1e-2,
                    "{gamma:?}: {turn} vs {flipped}"
                );
            }
            // A pointer along the Crystal Axis names no direction in the plane.
            assert_eq!(second_turn_deg(gamma, gamma), None);
        }
    }

    #[test]
    fn the_axis_line_is_folded_to_the_upper_end() {
        // Either sign of the direction gives the same handle: the upper end.
        let view = View::Profile {
            center: (0.0, 0.0),
            scale: 1.0,
        };
        let up = Vec3::new(0.0, 1.0, 0.0);
        let a = view.project(up * 2.0);
        let flipped = if (-up).y < 0.0 { up } else { -up };
        assert_eq!(view.project(flipped * 2.0), a);
    }
}
