//! The Start and End pictures of the "Export Tilt Video" section: for every axis of the
//! video, the solid renderer draws the stone at the Start angle and at the End angle, with
//! the camera, framing and image roll the video's frames use, so the person sees what the
//! numbers mean before rendering anything.
//!
//! The drawing runs on its own thread behind a latest-request-wins mailbox
//! (`LatestWorker`): typing in the angle fields asks again and again, and only the newest
//! request is drawn. Each picture is a few milliseconds of software rasterizing.

use super::{frames::axis_roll, params};
use crate::{
    MainWindow, TiltPosePreview, TiltVideoExportModel,
    bridge::render_thread::RenderContext,
    gui::{
        latest_worker::LatestWorker,
        solid_preview::{
            mesh_cache::MeshCache,
            raster::{SolidRasterizer, SolidStyle},
            to_pixel_buffer,
        },
        tilt::tilt_hover_preview::camera_pose_for_axis_tilt,
    },
};
use pleochroma::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use pleochroma_solid::preview::StoneGeometryBuf;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The long edge of a picture in pixels. The section draws it about 120 logical pixels
/// wide, so this stays sharp on a 125 % display.
const LONG_EDGE: u32 = 160;

/// One request: everything the drawing needs, taken on the UI thread.
struct PoseRequest {
    stone: StoneGeometryBuf,
    /// The video's camera distance and the image roll of its first axis.
    distance: f32,
    base_roll: f32,
    axes: Vec<usize>,
    start_deg: f64,
    end_deg: f64,
    /// The picture size, with the video's aspect ratio.
    width: u32,
    height: u32,
}

/// One axis's two finished pictures, `Send` so they can cross back to the UI thread.
struct AxisPictures {
    axis_index: usize,
    start: SharedPixelBuffer<Rgba8Pixel>,
    end: SharedPixelBuffer<Rgba8Pixel>,
}

/// The picture size for a video of `width` x `height`: [`LONG_EDGE`] on the longer side,
/// the video's aspect ratio, at least 32 pixels on the shorter side.
fn picture_size(width: u32, height: u32) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    let short = |long_side: u32, short_side: u32| {
        (u64::from(LONG_EDGE) * u64::from(short_side) / u64::from(long_side)).max(32)
    };
    if width >= height {
        (
            LONG_EDGE,
            u32::try_from(short(width, height)).unwrap_or(LONG_EDGE),
        )
    } else {
        (
            u32::try_from(short(height, width)).unwrap_or(LONG_EDGE),
            LONG_EDGE,
        )
    }
}

/// Draws `stone` at the video's pose for `axis_index` tilted by `tilt_deg`.
fn draw(
    cache: &mut MeshCache,
    request: &PoseRequest,
    axis_index: usize,
    tilt_deg: f64,
) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let mesh = cache.get_or_build_geometry(&request.stone)?;
    let (yaw, pitch) = camera_pose_for_axis_tilt(axis_index, tilt_deg);
    let camera = Camera::new(yaw, pitch, request.distance, DEFAULT_FOV_DEG)
        .with_roll(request.base_roll + axis_roll(&request.axes, axis_index));
    let style = SolidStyle {
        show_orientation_marker: false,
        ..SolidStyle::default()
    };
    let mut raster = SolidRasterizer::new(request.width, request.height);
    raster.render_prepared(mesh, &camera, &style);
    Some(to_pixel_buffer(&raster))
}

/// Draws both pictures of every axis in `request`, or nothing when the stone does not
/// close into a solid.
fn draw_all(cache: &mut MeshCache, request: &PoseRequest) -> Vec<AxisPictures> {
    request
        .axes
        .iter()
        .filter_map(|&axis_index| {
            Some(AxisPictures {
                axis_index,
                start: draw(cache, request, axis_index, request.start_deg)?,
                end: draw(cache, request, axis_index, request.end_deg)?,
            })
        })
        .collect()
}

/// Wires `TiltVideoExportModel.request_pose_previews`: the section calls it whenever the
/// axes, the angles or the picture size change, with the chart's axis for the case that
/// no axis chip is chosen.
pub(super) fn setup(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let ui_weak = ui.as_weak();
    let worker = LatestWorker::spawn_with_panic_hook(
        "tilt-pose-previews",
        move |request: PoseRequest| {
            let mut cache = MeshCache::default();
            let pictures = draw_all(&mut cache, &request);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| show(&ui, pictures));
        },
        || {},
    );
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    ui.global::<TiltVideoExportModel>()
        .on_request_pose_previews(move |fallback_axis| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<TiltVideoExportModel>();
            let (video_width, video_height) = params::resolve_resolution(
                model.get_resolution_preset(),
                model.get_custom_width(),
                model.get_custom_height(),
            );
            let (width, height) = picture_size(video_width, video_height);
            // The stone the viewport holds; the video renders the finished stone, which is
            // the same one unless the Cut slider is pulled back.
            // "Turn Stone 90°" turns the pictures like the frames (`build_request`).
            let quarter_turn = if model.get_quarter_turn() {
                std::f32::consts::FRAC_PI_2
            } else {
                0.0
            };
            let (stone, distance, base_roll) = {
                let guard = RenderContext::lock(&render_ctx);
                (
                    StoneGeometryBuf {
                        planes: guard.active_planes.to_vec(),
                        tools: guard.active_tools.to_vec(),
                        // Bookkeeping for the facet map and the finish; the solid picture
                        // needs neither.
                        placements: Vec::new(),
                        frosted: Vec::new(),
                    },
                    guard.distance,
                    guard.camera_roll + quarter_turn,
                )
            };
            if stone.planes.is_empty() {
                show(&ui, Vec::new());
                return;
            }
            worker.submit(PoseRequest {
                stone,
                distance,
                base_roll,
                axes: super::selected_axes(&ui, fallback_axis),
                start_deg: params::clamp_angle_deg(f64::from(model.get_start_angle_deg())),
                end_deg: params::clamp_angle_deg(f64::from(model.get_end_angle_deg())),
                width,
                height,
            });
        });
}

/// Puts the finished pictures on `TiltVideoExportModel.pose_previews`.
fn show(ui: &MainWindow, pictures: Vec<AxisPictures>) {
    let rows: Vec<TiltPosePreview> = pictures
        .into_iter()
        .map(|axis| TiltPosePreview {
            axis_index: i32::try_from(axis.axis_index).unwrap_or(0),
            start: slint::Image::from_rgba8(axis.start),
            end: slint::Image::from_rgba8(axis.end),
        })
        .collect();
    ui.global::<TiltVideoExportModel>()
        .set_pose_previews(Rc::new(VecModel::from(rows)).into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use pleochroma::geometry::plane::GpuFacetPlane;

    /// A unit cube's six faces.
    fn cube() -> StoneGeometryBuf {
        StoneGeometryBuf {
            planes: [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ]
            .into_iter()
            .map(|n| GpuFacetPlane::new(n, -0.5))
            .collect(),
            ..StoneGeometryBuf::default()
        }
    }

    #[test]
    fn the_pictures_keep_the_videos_aspect_ratio() {
        assert_eq!(picture_size(1920, 1080), (160, 90));
        assert_eq!(picture_size(1024, 1024), (160, 160));
        assert_eq!(picture_size(720, 1280), (90, 160));
        assert_eq!(
            picture_size(8192, 128),
            (160, 32),
            "never thinner than 32 pixels"
        );
    }

    #[test]
    fn every_axis_gets_a_start_and_an_end_picture_of_the_requested_size() {
        let request = PoseRequest {
            stone: cube(),
            distance: 4.0,
            base_roll: 0.0,
            axes: vec![0, 2],
            start_deg: -30.0,
            end_deg: 30.0,
            width: 48,
            height: 32,
        };
        let pictures = draw_all(&mut MeshCache::default(), &request);
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[1].axis_index, 2);
        for axis in &pictures {
            for picture in [&axis.start, &axis.end] {
                assert_eq!((picture.width(), picture.height()), (48, 32));
            }
        }
    }

    #[test]
    fn an_open_stone_draws_no_pictures() {
        let mut stone = cube();
        stone.planes.truncate(3);
        let request = PoseRequest {
            stone,
            distance: 4.0,
            base_roll: 0.0,
            axes: vec![0],
            start_deg: 0.0,
            end_deg: 10.0,
            width: 32,
            height: 32,
        };
        assert!(draw_all(&mut MeshCache::default(), &request).is_empty());
    }
}
