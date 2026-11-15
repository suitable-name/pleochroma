//! Immediate axis projection over the Live Render image, independent of tracing.

use super::{Shared, crystal_axis_overlay::view_of};
use crate::{
    CrystalAxisModel, MainWindow, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        render::{crystal_axis_ui, pose_camera::inverse_pose_camera},
        solid_preview::preview_state::CameraPose,
    },
};
use glam::Vec3;
use pleochroma::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use pleochroma_editor::manipulate::{ScreenSize, project};
use slint::ComponentHandle;

fn line(
    camera: &Camera,
    size: ScreenSize,
    direction: Vec3,
    length: f32,
    offset: (f32, f32),
) -> Option<[(f32, f32); 2]> {
    let direction = direction.try_normalize()?;
    let direction = if direction.y < 0.0 {
        -direction
    } else {
        direction
    };
    let from = project(camera, -direction * length, size)?;
    let to = project(camera, direction * length, size)?;
    Some([
        (from.x + offset.0, from.y + offset.1),
        (to.x + offset.0, to.y + offset.1),
    ])
}

/// Size and offset of `image` fitted into `viewport` with its aspect ratio kept (letterboxed).
fn contain(viewport: (f32, f32), image: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let factor = (viewport.0 / image.0).min(viewport.1 / image.1);
    let size = (image.0 * factor, image.1 * factor);
    (
        size,
        ((viewport.0 - size.0) * 0.5, (viewport.1 - size.1) * 0.5),
    )
}

pub(super) fn refresh(ui: &MainWindow, shared: &Shared) {
    let model = ui.global::<CrystalAxisModel>();
    model.set_live_overlay_visible(false);
    model.set_live_second_visible(false);
    if !model.get_show_axis() {
        return;
    }
    let preview = ui.global::<SolidPreviewModel>();
    let viewport = (
        preview.get_live_viewport_width(),
        preview.get_live_viewport_height(),
    );
    if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
        return;
    }
    let shown_length = shared
        .geometry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(view_of)
        .map(|(_, length)| length);
    let render = RenderContext::lock(&shared.render_ctx);
    if render.active_planes.is_empty() {
        return;
    }
    let Some(material) = crystal_axis_ui::resolved_material(&render) else {
        return;
    };
    let Some(axes) = crystal_axis_ui::effective_axes(ui, &material) else {
        return;
    };
    let view = ui.global::<ViewportModel>();
    let image = view.get_render_image().size();
    let traced = view.get_live_view_mode() != 0;
    let image = if traced && image.width > 0 && image.height > 0 {
        (image.width as f32, image.height as f32)
    } else if traced {
        (render.width as f32, render.height as f32)
    } else {
        viewport
    };
    if image.0 <= 0.0 || image.1 <= 0.0 {
        return;
    }
    let (size, offset) = contain(viewport, image);
    // The axes are in the stone's own frame and the marker lies over the picture the Live Render
    // tab shows. With a Stone Pose that picture is the stone turned in front of the camera, which
    // is the unrotated stone seen by the camera turned by the inverse pose.
    let pose = inverse_pose_camera(
        render.stone_pose,
        CameraPose {
            yaw: render.yaw,
            pitch: render.pitch,
            roll: render.camera_roll,
            distance: render.distance,
            pan: render.camera_pan,
        },
    );
    let camera = Camera::new(pose.yaw, pose.pitch, pose.distance, DEFAULT_FOV_DEG)
        .with_roll(pose.roll)
        .with_pan(pose.pan);
    // The shown radius can come from the last Solid frame, so cap the marker
    // length by the current camera distance.
    let length = shown_length
        .unwrap_or(render.distance * 0.15)
        .min(render.distance * 0.25);
    drop(render);
    let size = ScreenSize::new(size.0, size.1);
    let Some([from, to]) = line(&camera, size, axes.gamma, length, offset) else {
        return;
    };
    model.set_live_axis_x0(from.0);
    model.set_live_axis_y0(from.1);
    model.set_live_axis_x1(to.0);
    model.set_live_axis_y1(to.1);
    if let Some([from, to]) = axes
        .alpha
        .and_then(|axis| line(&camera, size, axis, length * 0.8, offset))
    {
        model.set_live_second_x0(from.0);
        model.set_live_second_y0(from.1);
        model.set_live_second_x1(to.0);
        model.set_live_second_y1(to.1);
        model.set_live_second_visible(true);
    }
    model.set_live_overlay_visible(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traced_marker_stays_inside_the_letterboxed_image_and_survives_end_on_projection() {
        assert_eq!(
            contain((900.0, 700.0), (1600.0, 900.0)),
            ((900.0, 506.25), (0.0, 96.875))
        );
        let camera = Camera::new(0.0, 0.0, 2.4, DEFAULT_FOV_DEG);
        let ends = line(
            &camera,
            ScreenSize::new(900.0, 506.25),
            Vec3::Z,
            0.5,
            (0.0, 96.875),
        )
        .expect("an axis along the view remains a visible labeled dot");
        assert_eq!(ends, [(450.0, 350.0), (450.0, 350.0)]);
    }

    #[test]
    fn marker_projection_turns_with_the_image_plane() {
        let size = ScreenSize::new(600.0, 600.0);
        let original = Camera::new(0.0, 0.0, 2.4, DEFAULT_FOV_DEG);
        let rolled =
            Camera::new(0.0, 0.0, 2.4, DEFAULT_FOV_DEG).with_roll(std::f32::consts::FRAC_PI_2);
        let upright = line(&original, size, Vec3::Y, 0.5, (0.0, 0.0)).unwrap();
        let turned = line(&rolled, size, Vec3::Y, 0.5, (0.0, 0.0)).unwrap();
        assert_eq!(upright[0].0, upright[1].0);
        assert!((turned[0].1 - turned[1].1).abs() < 1e-3);
        assert!(turned[1].0 > turned[0].0);
    }

    /// With a Stone Pose the marker lies on the posed stone: a point of the axis in the stone's
    /// frame, seen by the inverse-pose camera, lands where the pose carries it in the plain camera.
    #[test]
    fn marker_of_a_posed_stone_lies_on_the_posed_stone() {
        let size = ScreenSize::new(600.0, 500.0);
        let pose = pleochroma::render_setup::StonePose {
            tilt_deg: 70.0,
            turn_deg: 25.0,
            spin_deg: 10.0,
        };
        let live = CameraPose {
            yaw: 0.4,
            pitch: 0.3,
            roll: 0.2,
            distance: 2.4,
            pan: [0.1, 0.05],
        };
        let plain = Camera::new(live.yaw, live.pitch, live.distance, DEFAULT_FOV_DEG)
            .with_roll(live.roll)
            .with_pan(live.pan);
        let turned = inverse_pose_camera(pose, live);
        let marker = Camera::new(turned.yaw, turned.pitch, turned.distance, DEFAULT_FOV_DEG)
            .with_roll(turned.roll)
            .with_pan(turned.pan);
        let axis = Vec3::new(0.3, 0.8, -0.2).normalize() * 0.5;
        let on_stone = project(&plain, pose.rotation() * axis, size).unwrap();
        let drawn = project(&marker, axis, size).unwrap();
        assert!((on_stone.x - drawn.x).abs() < 0.5, "{on_stone:?} {drawn:?}");
        assert!((on_stone.y - drawn.y).abs() < 0.5, "{on_stone:?} {drawn:?}");
    }

    #[test]
    fn marker_projection_follows_the_pan() {
        // Half a picture height to the right and a quarter up: the stone's centre, and the
        // vertical axis line through it, move to (450, 225) in a 600x600 picture.
        let size = ScreenSize::new(600.0, 600.0);
        let panned = Camera::new(0.0, 0.0, 2.4, DEFAULT_FOV_DEG).with_pan([0.5, 0.25]);
        let [from, to] = line(&panned, size, Vec3::Y, 0.5, (0.0, 0.0)).unwrap();
        assert!((from.0 - 450.0).abs() < 1e-2 && (to.0 - 450.0).abs() < 1e-2);
        assert!((f32::midpoint(from.1, to.1) - 225.0).abs() < 1e-2);
    }
}
