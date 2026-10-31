//! Immediate axis projection over the Live Render image, independent of tracing.

use super::{Shared, crystal_axis_overlay::view_of};
use crate::{
    CrystalAxisModel, MainWindow, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext, gui::render::crystal_axis_ui,
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

/// The actual image's contain rectangle, including a reduced bootstrap's aspect.
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
    let camera = Camera::new(render.yaw, render.pitch, render.distance, DEFAULT_FOV_DEG)
        .with_roll(render.camera_roll);
    // A previous design's Solid frame may still supply the radius while this
    // scene traces. Keep the direction marker in front of the current camera.
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
}
