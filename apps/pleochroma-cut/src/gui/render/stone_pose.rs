//! The Stone Pose controls: the Lighting tab's STONE POSE group, the Alt-drag and Ctrl+Shift-drag
//! gesture in the Live Render view, Face the Camera, Product Shot and the chip that stands the
//! stone as designed.
//!
//! Every change goes one way: the pose is written to `RenderContext::stone_pose`, the context is
//! marked dirty, the Solid raster is redrawn at the new pose ([`resubmit_at_current_pose`]), the
//! pose is stored in the settings (`AppSettings::stone_pose`) and the controls are refreshed from
//! the clamped pose. While a slider or the gesture holds the stone, `pose_drag_held` and
//! `camera_drag_held` are set, so the Stone Shadow waits for the release and the remote settle
//! does not dispatch mid-motion.

use super::camera_lighting::{pan_picture_size, resubmit_at_current_pose, shown_viewport};
use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{RenderContext, progress::set_camera_drag},
    gui::{
        show_toast,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
    settings::SettingsPersister,
};
use glam::Vec3;
use pleochroma::{
    geometry::{
        plane::GpuFacetPlane,
        stone_metrics::{SolidStatus, build_solid_mesh_geom},
        tool::ToolPrimitive,
    },
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
    render_setup::StonePose,
};
use pleochroma_solid::preview::camera::{
    fit_distance_for_view, orbit_distance_bounds, pan_pose, unroll_drag_delta,
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Degrees of tilt or turn per logical pixel of a pose drag: the orbit's 0.008 radians per pixel.
const DEGREES_PER_PIXEL: f32 = 0.458_366_24;

/// The camera elevation Product Shot sets, in degrees.
pub const PRODUCT_SHOT_PITCH_DEG: f32 = 15.0;

/// How long a slider may stand still before its hold on the shadow is let go. A slider released
/// with the mouse reports it; one moved with the keys does not, so the hold times out.
const SLIDER_HOLD_TIMEOUT: Duration = Duration::from_millis(400);

/// Passes of the Product Shot's pose and pan, which depend on each other through the eye the pan
/// moves: the pan places the contact point of the posed stone, the pose faces the panned eye.
const PRODUCT_SHOT_PASSES: usize = 4;

/// The pose after a drag of `(dx, dy)` logical pixels in the picture: vertical tilts the stone
/// (down leans the table towards the viewer), horizontal turns it (right turns the front of the
/// stone to the right), at the orbit's degrees per pixel. The deltas are first mapped through the
/// camera `roll`, as an orbit drag's are, so a rolled view drags along the picture. The tilt
/// stops at `0` and `180`, the turn wraps; the spin is kept. A non-finite delta changes nothing.
#[must_use]
pub fn pose_drag_step(pose: StonePose, roll: f32, dx: f32, dy: f32) -> StonePose {
    if !dx.is_finite() || !dy.is_finite() {
        return pose;
    }
    let (dx, dy) = unroll_drag_delta(roll, dx, dy);
    StonePose {
        tilt_deg: dy.mul_add(DEGREES_PER_PIXEL, pose.tilt_deg),
        turn_deg: dx.mul_add(DEGREES_PER_PIXEL, pose.turn_deg),
        spin_deg: pose.spin_deg,
    }
    .clamped()
}

/// The unit direction from the model origin to the eye of the camera `yaw`, `pitch` (radians),
/// `roll` and `pan`: the eye after the roll and the pan, not the orbit angles alone.
#[must_use]
pub fn eye_direction(yaw: f32, pitch: f32, roll: f32, pan: [f32; 2]) -> Vec3 {
    Camera::new(yaw, pitch, 1.0, DEFAULT_FOV_DEG)
        .with_roll(roll)
        .with_pan(pan)
        .origin
        .normalize_or_zero()
}

/// What Face the Camera sets: the pose that turns the table to the eye of the camera, with the
/// current `spin_deg` kept.
#[must_use]
pub fn face_camera_pose(
    yaw: f32,
    pitch: f32,
    roll: f32,
    pan: [f32; 2],
    spin_deg: f32,
) -> StonePose {
    StonePose::facing(eye_direction(yaw, pitch, roll, pan), spin_deg)
}

/// The text of the Live view's chip: `Stone Pose · tilted 75°`, or `Stone Pose · rotated` for a
/// stone that is turned or spun but not tilted. Empty for the stone as designed.
#[must_use]
pub fn pose_chip_text(pose: StonePose) -> String {
    if pose.is_identity() {
        String::new()
    } else if pose.tilt_deg >= 0.5 {
        format!("Stone Pose \u{b7} tilted {:.0}\u{b0}", pose.tilt_deg)
    } else {
        "Stone Pose \u{b7} rotated".to_owned()
    }
}

/// The export dialog's line, `Stone Pose: tilt 75°, turn 30°, spin 0°`. Empty for the stone as
/// designed.
#[must_use]
pub fn pose_export_text(pose: StonePose) -> String {
    let detail = crate::gui::render_jobs::convert::stone_pose_detail(pose);
    detail
        .strip_prefix(" \u{b7} ")
        .map_or_else(|| detail.clone(), str::to_owned)
}

/// What Product Shot sets: the camera and the pose of a stone standing on the tray with room for
/// its mirror image below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProductShot {
    /// The pose: the table faces the camera, the spin is kept.
    pub pose: StonePose,
    /// The camera yaw in radians: unchanged.
    pub yaw: f32,
    /// The camera elevation in radians: [`PRODUCT_SHOT_PITCH_DEG`].
    pub pitch: f32,
    /// The camera roll in radians: `0`.
    pub roll: f32,
    /// The orbit distance: Fit for a stone of twice the bounding radius.
    pub distance: f32,
    /// The pan that puts the contact point of the posed stone at the picture's centre.
    pub pan: [f32; 2],
}

/// The pan, in image half-heights, that puts the world point `point` at the centre of the picture
/// of the unpanned camera `yaw`, `pitch` at `distance` in a picture of `aspect` (width over
/// height), through the pan clamp of a drag (`pan_pose`): the stone's centre stays inside the
/// picture.
#[must_use]
pub fn pan_for_point(point: Vec3, yaw: f32, pitch: f32, distance: f32, aspect: f32) -> [f32; 2] {
    let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
    let scale = distance * camera.fov_tan;
    let x = -point.dot(camera.right) / scale;
    let y = -point.dot(camera.up) / scale;
    // `pan_pose` moves the pan by `2 * dx / height` and `-2 * dy / height`: a picture of height 1
    // turns the wanted pan into the drag that makes it, clamped as the drag is.
    pan_pose(
        CameraPose {
            roll: 0.0,
            yaw,
            pitch,
            distance,
            pan: [0.0, 0.0],
        },
        x / 2.0,
        -y / 2.0,
        aspect,
        1.0,
    )
    .pan
}

/// The lowest vertex of `vertices` after the pose `pose`: where the stone touches the tray.
fn contact_point(vertices: &[Vec3], pose: StonePose) -> Vec3 {
    let rotation = pose.rotation();
    vertices
        .iter()
        .map(|v| rotation * *v)
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap_or(Vec3::ZERO)
}

/// Product Shot for a stone with the given `vertices` (the solid's corners as designed), the
/// camera's current `yaw`, the current `spin_deg` and a picture of `aspect` (width over height).
///
/// The camera drops to [`PRODUCT_SHOT_PITCH_DEG`] with no roll and its yaw kept; the stone faces
/// it (Face the Camera: the eye after the pan, so pose and pan are settled together over a few
/// passes); the distance is Fit for twice the bounding radius, which leaves room for the mirror
/// image; and the pan puts the contact point at the picture's centre, stone above, mirror image
/// below. `None` when there are no vertices.
#[must_use]
pub fn product_shot(
    vertices: &[Vec3],
    yaw: f32,
    spin_deg: f32,
    aspect: f32,
) -> Option<ProductShot> {
    let radius = vertices
        .iter()
        .copied()
        .map(Vec3::length)
        .fold(0.0_f32, f32::max);
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let pitch = PRODUCT_SHOT_PITCH_DEG.to_radians();
    let (min_distance, _) = orbit_distance_bounds(f64::from(radius));
    let distance = fit_distance_for_view(f64::from(2.0 * radius), aspect).max(min_distance);
    let mut pan = [0.0_f32, 0.0];
    let mut pose = StonePose::IDENTITY;
    for _ in 0..PRODUCT_SHOT_PASSES {
        pose = face_camera_pose(yaw, pitch, 0.0, pan, spin_deg);
        pan = pan_for_point(contact_point(vertices, pose), yaw, pitch, distance, aspect);
    }
    Some(ProductShot {
        pose,
        yaw,
        pitch,
        roll: 0.0,
        distance,
        pan,
    })
}

/// The toast Product Shot ends with. The Tray Reflection shows the mirror image only on a studio
/// light or lit model with a backdrop (`tray_in_scope`) and only above 0.
#[must_use]
pub const fn product_shot_toast(tray_reflection: f32, tray_in_scope: bool) -> &'static str {
    if !tray_in_scope {
        "Product Shot set the pose and the camera. The Tray Reflection needs a studio light with a backdrop."
    } else if tray_reflection <= 0.0 {
        "Product Shot set the pose and the camera. Turn on the Tray Reflection for the mirror image."
    } else {
        "Product Shot set the pose and the camera."
    }
}

/// The corners of the solid the planes and tools bound, as designed; empty when they close no
/// solid.
fn solid_vertices(planes: &[GpuFacetPlane], tools: &[ToolPrimitive]) -> Vec<Vec3> {
    let halfspaces: Vec<_> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    match build_solid_mesh_geom(&halfspaces, tools) {
        SolidStatus::Closed(mesh) => mesh.positions.iter().map(|p| p.as_vec3()).collect(),
        _ => Vec::new(),
    }
}

/// Stands the stone as designed in the render context: what the Lighting tab's "Reset this tab"
/// does to the pose (the settings are cleared by `AppSettings::reset_lighting_tab`'s own rule).
pub(in crate::gui) const fn stand_as_designed(ctx: &mut RenderContext) {
    ctx.stone_pose = StonePose::IDENTITY;
}

/// Sets the hold flags: while a slider or the gesture has the stone, the shadow waits for the
/// release and the remote settle does not dispatch.
fn set_holds(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, held: bool) {
    {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.pose_drag_held = held;
        ctx.camera_drag_held = held;
    }
    set_camera_drag(ui, held);
}

/// Shows `pose` in the controls: the chip and the export line always, the three angles unless
/// `keep_angles` (a slider the person is dragging must not be moved under the hand: a turn of 360
/// would read back as 0).
pub(in crate::gui) fn push_stone_pose_to_controls(
    ui: &MainWindow,
    pose: StonePose,
    keep_angles: bool,
) {
    let settings = ui.global::<SettingsModel>();
    if !keep_angles {
        settings.set_stone_tilt_deg(pose.tilt_deg);
        settings.set_stone_turn_deg(pose.turn_deg);
        settings.set_stone_spin_deg(pose.spin_deg);
    }
    settings.set_stone_pose_chip(pose_chip_text(pose).into());
    settings.set_stone_pose_detail(pose_export_text(pose).into());
}

/// Writes `pose` (clamped) to the render context and the settings, redraws the Solid raster and
/// refreshes the controls ([`push_stone_pose_to_controls`]). A pose the context already holds
/// changes nothing but the controls.
fn apply_pose(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &SettingsPersister,
    preview_state: &SolidPreviewState,
    pose: StonePose,
    keep_angles: bool,
) {
    let pose = pose.clamped();
    let changed = {
        let mut ctx = RenderContext::lock(render_ctx);
        if ctx.stone_pose == pose {
            false
        } else {
            ctx.stone_pose = pose;
            ctx.dirty = true;
            resubmit_at_current_pose(ui, &ctx, preview_state);
            drop(ctx);
            true
        }
    };
    if changed {
        settings_store.update(|s| s.settings.stone_pose = pose.to_array());
    }
    push_stone_pose_to_controls(ui, pose, keep_angles);
}

/// The hold of a slider on the shadow: set with each live step, released with the slider's
/// release or when the steps stop ([`SLIDER_HOLD_TIMEOUT`]).
struct SliderHold {
    timer: Timer,
}

impl SliderHold {
    fn engage(&self, ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
        set_holds(ui, render_ctx, true);
        let ui_weak = ui.as_weak();
        let render_ctx = Arc::clone(render_ctx);
        self.timer
            .start(TimerMode::SingleShot, SLIDER_HOLD_TIMEOUT, move || {
                if let Some(ui) = ui_weak.upgrade() {
                    set_holds(&ui, &render_ctx, false);
                }
            });
    }

    fn release(&self, ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
        if self.timer.running() {
            self.timer.stop();
            set_holds(ui, render_ctx, false);
        }
    }
}

/// Wires `SettingsModel.stone_pose_changed` (the sliders and fields), `stone_pose_reset`,
/// `stone_face_camera` and `stone_product_shot`, shows the stored pose in the controls, and wires
/// the viewport's pose drag (`ViewportModel.stone_pose_drag` with its button edges).
pub(in crate::gui) fn setup_stone_pose_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    setup_pose_changed(ui, render_ctx, settings_store, preview_state);
    setup_reset(ui, render_ctx, settings_store, preview_state);
    setup_face_camera(ui, render_ctx, settings_store, preview_state);
    setup_product_shot(ui, render_ctx, settings_store, preview_state);
    setup_pose_drag(ui, render_ctx, settings_store, preview_state);
    let pose = RenderContext::lock(render_ctx).stone_pose;
    push_stone_pose_to_controls(ui, pose, false);
}

/// The sliders (live, held) and the number fields (on Enter).
fn setup_pose_changed(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    let hold = SliderHold {
        timer: Timer::default(),
    };
    ui.global::<SettingsModel>()
        .on_stone_pose_changed(move |tilt, turn, spin, live| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if live {
                hold.engage(&ui, &render_ctx);
            }
            apply_pose(
                &ui,
                &render_ctx,
                &settings_store,
                &preview_state,
                StonePose::from_array([tilt, turn, spin]),
                live,
            );
            if !live {
                hold.release(&ui, &render_ctx);
            }
        });
}

/// The group's Reset and the Live view's chip: the stone as designed.
fn setup_reset(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    ui.global::<SettingsModel>().on_stone_pose_reset(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        apply_pose(
            &ui,
            &render_ctx,
            &settings_store,
            &preview_state,
            StonePose::IDENTITY,
            false,
        );
    });
}

/// Face the Camera: the table turns to the eye, the spin stays.
fn setup_face_camera(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    ui.global::<SettingsModel>().on_stone_face_camera(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let pose = {
            let ctx = RenderContext::lock(&render_ctx);
            face_camera_pose(
                ctx.yaw,
                ctx.pitch,
                ctx.camera_roll,
                ctx.camera_pan,
                ctx.stone_pose.spin_deg,
            )
        };
        apply_pose(
            &ui,
            &render_ctx,
            &settings_store,
            &preview_state,
            pose,
            false,
        );
    });
}

/// Product Shot: pose and camera in one step, lighting untouched. The camera is written like a
/// pose pill's (`setup_set_view_callback`): the context, a redraw, then the settings.
fn setup_product_shot(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_stone_product_shot(move |tray_in_scope| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (planes, tools, yaw, spin, render_size) = {
                let ctx = RenderContext::lock(&render_ctx);
                (
                    Arc::clone(&ctx.active_planes),
                    Arc::clone(&ctx.active_tools),
                    ctx.yaw,
                    ctx.stone_pose.spin_deg,
                    (ctx.width, ctx.height),
                )
            };
            let (viewport, traced) = shown_viewport(&ui);
            let (width, height) = pan_picture_size(traced, viewport, render_size);
            let aspect = if width > 0.0 && height > 0.0 {
                width / height
            } else {
                1.0
            };
            // Measured outside the context lock, which the render thread shares.
            let vertices = solid_vertices(&planes, &tools);
            let Some(shot) = product_shot(&vertices, yaw, spin, aspect) else {
                show_toast(&ui, "Product Shot needs a solved stone.", "info");
                return;
            };
            let tray_reflection = {
                let mut ctx = RenderContext::lock(&render_ctx);
                ctx.stone_pose = shot.pose;
                ctx.yaw = shot.yaw;
                ctx.pitch = shot.pitch;
                ctx.camera_roll = shot.roll;
                ctx.distance = shot.distance;
                ctx.camera_pan = shot.pan;
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state);
                ctx.tray_reflection
            };
            settings_store.update(|s| {
                s.settings.stone_pose = shot.pose.to_array();
                s.settings.camera_yaw = shot.yaw;
                s.settings.camera_pitch = shot.pitch;
                s.settings.camera_roll = shot.roll;
                s.settings.camera_distance = shot.distance;
                s.settings.camera_pan = shot.pan;
            });
            push_stone_pose_to_controls(&ui, shot.pose, false);
            show_toast(
                &ui,
                product_shot_toast(tray_reflection, tray_in_scope),
                "info",
            );
        });
}

/// The viewport gesture: Alt or Ctrl and Shift with the left button, vertical tilts, horizontal
/// turns. The button edges hold the shadow like a slider does.
fn setup_pose_drag(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let model = ui.global::<ViewportModel>();

    let begin_ctx = Arc::clone(render_ctx);
    let begin_weak: Weak<MainWindow> = ui.as_weak();
    model.on_stone_pose_drag_begin(move || {
        if let Some(ui) = begin_weak.upgrade() {
            set_holds(&ui, &begin_ctx, true);
        }
    });

    let end_ctx = Arc::clone(render_ctx);
    let end_weak = ui.as_weak();
    model.on_stone_pose_drag_end(move || {
        if let Some(ui) = end_weak.upgrade() {
            set_holds(&ui, &end_ctx, false);
        }
    });

    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    model.on_stone_pose_drag(move |dx, dy| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let pose = {
            let ctx = RenderContext::lock(&render_ctx);
            pose_drag_step(ctx.stone_pose, ctx.camera_roll, dx, dy)
        };
        apply_pose(
            &ui,
            &render_ctx,
            &settings_store,
            &preview_state,
            pose,
            false,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::AppSettings;
    use std::f32::consts::PI;

    /// A small stone: a girdle ring of four corners, a crown point and a deeper culet.
    fn stone() -> Vec<Vec3> {
        vec![
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.7),
            Vec3::new(0.0, 0.0, -0.7),
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(0.0, -0.8, 0.0),
        ]
    }

    #[test]
    fn a_pose_drag_down_tilts_and_right_turns_at_the_orbit_rate() {
        let pose = pose_drag_step(StonePose::IDENTITY, 0.0, 0.0, 100.0);
        assert!((pose.tilt_deg - 45.836_624).abs() < 1e-3);
        assert!(pose.turn_deg.abs() < 1e-6);
        let pose = pose_drag_step(StonePose::IDENTITY, 0.0, 100.0, 0.0);
        assert!((pose.turn_deg - 45.836_624).abs() < 1e-3);
        assert!(pose.tilt_deg.abs() < 1e-6);
    }

    #[test]
    fn a_pose_drag_clamps_the_tilt_wraps_the_turn_and_keeps_the_spin() {
        let posed = StonePose {
            tilt_deg: 170.0,
            turn_deg: 350.0,
            spin_deg: 33.0,
        };
        let pose = pose_drag_step(posed, 0.0, 100.0, 100.0);
        assert!((pose.tilt_deg - 180.0).abs() < 1e-6, "stops at table down");
        assert!(pose.turn_deg < 40.0, "350 + 45.8 wraps into the next turn");
        assert!((pose.spin_deg - 33.0).abs() < 1e-6);
        let pose = pose_drag_step(posed, 0.0, 0.0, -1000.0);
        assert!(pose.tilt_deg.abs() < 1e-6, "stops at table up");
    }

    #[test]
    fn a_pose_drag_follows_a_rolled_picture_and_ignores_non_finite_deltas() {
        // A quarter roll sends a horizontal drag along the stone's vertical axis.
        let pose = pose_drag_step(
            StonePose {
                tilt_deg: 90.0,
                turn_deg: 0.0,
                spin_deg: 0.0,
            },
            PI / 2.0,
            100.0,
            0.0,
        );
        assert!((pose.tilt_deg - (90.0 - 45.836_624)).abs() < 1e-2);
        let posed = StonePose {
            tilt_deg: 10.0,
            turn_deg: 20.0,
            spin_deg: 30.0,
        };
        assert_eq!(pose_drag_step(posed, 0.0, f32::NAN, 1.0), posed);
    }

    #[test]
    fn face_the_camera_turns_to_the_eye_and_keeps_the_spin() {
        // The default camera: yaw 0, pitch 0.45 rad (25.8 degrees) leaves 64.2 degrees of tilt.
        let pose = face_camera_pose(0.0, 0.45, 0.0, [0.0, 0.0], 40.0);
        assert!(pose.turn_deg.abs() < 1e-3 || (pose.turn_deg - 360.0).abs() < 1e-3);
        assert!((pose.tilt_deg - (90.0 - 0.45_f32.to_degrees())).abs() < 1e-3);
        assert!((pose.spin_deg - 40.0).abs() < 1e-6);
        // Yaw 90 degrees looks from +X.
        let pose = face_camera_pose(PI / 2.0, 0.0, 0.0, [0.0, 0.0], 0.0);
        assert!((pose.turn_deg - 90.0).abs() < 1e-3);
        assert!((pose.tilt_deg - 90.0).abs() < 1e-3);
    }

    #[test]
    fn product_shot_sets_the_camera_and_faces_it() {
        let shot = product_shot(&stone(), 0.6, 25.0, 16.0 / 9.0).expect("a stone with corners");
        assert!((shot.pitch - 15.0_f32.to_radians()).abs() < 1e-6);
        assert!(shot.roll.abs() < f32::EPSILON);
        assert!((shot.yaw - 0.6).abs() < f32::EPSILON);
        assert!((shot.pose.spin_deg - 25.0).abs() < 1e-4);
        // The table faces the eye: about 75 degrees of tilt, turned towards the yaw (the pan
        // moves the eye a little off the unpanned direction).
        assert!(shot.pose.tilt_deg > 60.0 && shot.pose.tilt_deg < 90.0);
        assert!((shot.pose.turn_deg - 0.6_f32.to_degrees()).abs() < 12.0);
    }

    #[test]
    fn product_shot_fits_twice_the_bounding_radius() {
        let shot = product_shot(&stone(), 0.0, 0.0, 1.5).expect("a stone with corners");
        // The farthest corner is 1.0 from the origin.
        let wanted = fit_distance_for_view(2.0, 1.5);
        assert!((shot.distance - wanted).abs() < 1e-4);
        assert!(shot.distance > fit_distance_for_view(1.0, 1.5));
    }

    #[test]
    fn product_shot_puts_the_contact_point_at_the_picture_centre() {
        let vertices = stone();
        let shot = product_shot(&vertices, 0.6, 0.0, 16.0 / 9.0).expect("a stone with corners");
        let contact = contact_point(&vertices, shot.pose);
        let camera = Camera::new(shot.yaw, shot.pitch, shot.distance, DEFAULT_FOV_DEG)
            .with_roll(shot.roll)
            .with_pan(shot.pan);
        let ray = contact - camera.origin;
        let depth = ray.dot(camera.forward);
        let x = ray.dot(camera.right) / (depth * camera.fov_tan);
        let y = ray.dot(camera.up) / (depth * camera.fov_tan);
        assert!(x.abs() < 2e-2 && y.abs() < 2e-2, "contact at ({x}, {y})");
    }

    #[test]
    fn the_contact_point_is_the_lowest_posed_vertex() {
        let vertices = stone();
        assert_eq!(
            contact_point(&vertices, StonePose::IDENTITY),
            Vec3::new(0.0, -0.8, 0.0)
        );
        // Table down: the crown point is the lowest.
        let lying = contact_point(
            &vertices,
            StonePose {
                tilt_deg: 180.0,
                turn_deg: 0.0,
                spin_deg: 0.0,
            },
        );
        assert!((lying.y + 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_point_the_pan_clamp_cannot_reach_stays_in_the_picture() {
        // A point far to the right of a square picture: the pan stops at the picture's edge.
        let pan = pan_for_point(Vec3::new(-100.0, 0.0, 0.0), 0.0, 0.0, 3.0, 1.0);
        assert!((pan[0] - 1.0).abs() < 1e-6);
        assert!(pan[1].abs() < 1e-6);
    }

    #[test]
    fn product_shot_needs_a_stone() {
        assert!(product_shot(&[], 0.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn the_product_shot_toast_names_the_missing_reflection_or_scope() {
        assert!(product_shot_toast(0.0, true).contains("Turn on the Tray Reflection"));
        assert_eq!(
            product_shot_toast(0.3, true),
            "Product Shot set the pose and the camera."
        );
        assert!(product_shot_toast(0.3, false).contains("needs a studio light with a backdrop"));
        assert!(product_shot_toast(0.0, false).contains("needs a studio light with a backdrop"));
    }

    #[test]
    fn resetting_the_tab_stands_the_stone_as_designed_in_context_and_settings() {
        let posed = StonePose {
            tilt_deg: 75.0,
            turn_deg: 30.0,
            spin_deg: 10.0,
        };
        let mut ctx = RenderContext {
            stone_pose: posed,
            ..RenderContext::default()
        };
        let mut settings = AppSettings {
            stone_pose: posed.to_array(),
            ..AppSettings::default()
        };
        stand_as_designed(&mut ctx);
        settings.reset_lighting_tab();
        assert!(ctx.stone_pose.is_identity());
        assert_eq!(settings.stone_pose, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_chip_names_the_tilt_and_is_empty_for_the_stone_as_designed() {
        assert_eq!(pose_chip_text(StonePose::IDENTITY), "");
        assert_eq!(
            pose_chip_text(StonePose {
                tilt_deg: 74.6,
                turn_deg: 10.0,
                spin_deg: 0.0,
            }),
            "Stone Pose \u{b7} tilted 75\u{b0}"
        );
        assert_eq!(
            pose_chip_text(StonePose {
                tilt_deg: 0.0,
                turn_deg: 10.0,
                spin_deg: 0.0,
            }),
            "Stone Pose \u{b7} rotated"
        );
    }

    #[test]
    fn the_export_line_drops_the_queue_separator() {
        assert_eq!(pose_export_text(StonePose::IDENTITY), "");
        assert_eq!(
            pose_export_text(StonePose {
                tilt_deg: 90.0,
                turn_deg: 30.0,
                spin_deg: 0.0,
            }),
            "Stone Pose: tilt 90\u{b0}, turn 30\u{b0}, spin 0\u{b0}"
        );
    }
}
