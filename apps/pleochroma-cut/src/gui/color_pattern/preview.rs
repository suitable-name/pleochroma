//! The Preview area of the Color Pattern window.
//!
//! * Crystal and Cross-Section are window-free CPU rasters (`cross_section.rs`), drawn at once on
//!   the UI thread at a size that keeps a drag of a handle smooth.
//! * Current Design is a small Path-Traced render of the open design with the pattern applied, on
//!   the CPU tracer (the one renderer that always traces zones). It runs on a worker, waits
//!   [`DEBOUNCE`] after the last edit and is cancelled by the next one; `preview_busy` is on
//!   meanwhile. The Ray-Traced renderer has no zone path, so it is never used here.

use super::{
    cross_section::{Handle, View, handle_fractions, handles_for, render_rod, render_slice, scene},
    model::{f32_of, f64_of},
    session::{Host, PREVIEW_NOTE, Session},
};
use crate::{
    ColorPatternModel, ColorPatternWindow, PatternHandle,
    bridge::{pixel_buffer::rgba_image, preview_render::render_rgba_at_pose},
    gui::{
        latest_worker::LatestWorker,
        render::camera_lighting::fit_distance_for_radius,
        rough_colour::wizard::raster::Rgba,
        solid_preview::{
            cut_slider::cut_geometry,
            mesh_cache::{CachedMesh, MeshCache},
        },
    },
};
use pleochroma::{
    optics::{materials::GemMaterial, raytracer::DEFAULT_MAX_BOUNCES},
    render_setup::material_for_stone_oriented,
};
use pleochroma_cut_core::{
    Design,
    material::pattern::{ColorPattern, apply_color_pattern, follow_c_axis},
};
use pleochroma_editor::stone_orientation::orientation_override;
use slint::{ComponentHandle, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// How long the Current Design render waits after the last edit.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// The longest edge, in pixels, of a Crystal or Cross-Section picture.
const RASTER_EDGE: f32 = 360.0;
/// The longest edge, in pixels, of the Current Design render.
const DESIGN_EDGE: f32 = 256.0;
/// Samples per pixel of the Current Design render.
const DESIGN_SPP: u32 = 24;
/// The camera of the Current Design render: yaw and pitch in radians. Nearly from above, so the
/// table shows the colors the light brings out.
const DESIGN_YAW: f32 = 0.6;
const DESIGN_PITCH: f32 = 1.0;
/// The area's size, logical pixels, before the window has written its own.
const DEFAULT_AREA: (f32, f32) = (420.0, 300.0);

/// What the worker hands back: the picture, or the sentence for why there is none.
pub(super) type Rendered = Result<SharedPixelBuffer<Rgba8Pixel>, String>;

/// Posts a finished render to the UI thread (`ticket` 0: a failure of the worker itself).
pub(super) type Post = Arc<dyn Fn(u64, Rendered) + Send + Sync>;

/// One Current Design render to make.
struct Request {
    ticket: u64,
    design: Design,
    material: GemMaterial,
    /// Whether the pattern follows the Crystal Axis, so its zones turn with the axis the design
    /// gives the stone.
    follows_axis: bool,
    size: (u32, u32),
}

/// The thread that renders the Current Design preview.
pub(super) struct PreviewWorker {
    worker: LatestWorker<Request>,
    ticket: Arc<AtomicU64>,
}

impl PreviewWorker {
    /// Starts the thread. `post` is called ON THE WORKER THREAD with each finished render and
    /// must hop to the UI thread itself.
    pub(super) fn new(post: Post) -> Self {
        let ticket = Arc::new(AtomicU64::new(0));
        let latest = Arc::clone(&ticket);
        let on_failure = Arc::clone(&post);
        let worker = LatestWorker::spawn_with_panic_hook(
            "color-pattern-preview",
            move |request: Request| {
                std::thread::sleep(DEBOUNCE);
                if latest.load(Ordering::SeqCst) != request.ticket {
                    return;
                }
                let result = render_design(&request, &latest);
                if latest.load(Ordering::SeqCst) == request.ticket {
                    post(request.ticket, result);
                }
            },
            move || on_failure(0, Err("The preview could not be drawn.".to_owned())),
        );
        Self { worker, ticket }
    }

    /// Cancels the render in flight (it will not be posted) and returns the next ticket.
    pub(super) fn next_ticket(&self) -> u64 {
        self.ticket.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The newest ticket.
    pub(super) fn current(&self) -> u64 {
        self.ticket.load(Ordering::SeqCst)
    }

    fn submit(&self, request: Request) -> bool {
        self.worker.submit(request)
    }
}

/// The yaw, pitch and distance of the Current Design camera for a stone `radius` large.
fn design_pose(radius: f64) -> (f32, f32, f32) {
    (DESIGN_YAW, DESIGN_PITCH, fit_distance_for_radius(radius))
}

/// Renders the design with the pattern on the worker thread.
fn render_design(request: &Request, latest: &AtomicU64) -> Rendered {
    let stone = cut_geometry(&request.design, None, None);
    if stone.planes.is_empty() {
        return Err("The open design does not solve, so it cannot be drawn here.".to_owned());
    }
    if latest.load(Ordering::SeqCst) != request.ticket {
        return Err(String::new());
    }
    let radius = MeshCache::default()
        .get_or_build_geometry(&stone)
        .map_or(1.0, CachedMesh::bounding_radius);
    let girdle = request.design.girdle_diameter_mm.unwrap_or(0.0);
    // Drawn in the orientation the design gives the stone, so the pattern's colors are the ones
    // the Live Render shows.
    let orientation = orientation_override(&request.design.render_effects, &request.material);
    let mut material = material_for_stone_oriented(
        request.material.clone(),
        f32_of(girdle),
        &stone.planes,
        orientation,
    );
    // "Follows Crystal Axis": the zones turn to the axis the stone now has (the design's own, else
    // the material's default).
    if request.follows_axis {
        let c_axis = material.c_axis;
        if let Some(zoning) = material.zoning.as_mut() {
            follow_c_axis(zoning, c_axis);
        }
    }
    let rgba = render_rgba_at_pose(
        &stone.planes,
        &stone.tools,
        &stone.frosted,
        &material,
        {
            let (yaw, pitch, distance) = design_pose(radius);
            (yaw, pitch, distance, 0.0)
        },
        request.size,
        DESIGN_SPP,
        DEFAULT_MAX_BOUNCES,
    );
    let (width, height) = request.size;
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err("The preview could not be drawn.".to_owned());
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(&rgba);
    Ok(buffer)
}

/// The material the preview renders: the saved material with `pattern` applied (its zones
/// resolved at `width_mm`; a Uniform pattern is its base color alone; a pattern that cannot be
/// built yet leaves the material as it is).
pub(super) fn preview_material(
    base: &GemMaterial,
    pattern: &ColorPattern,
    width_mm: f64,
) -> GemMaterial {
    apply_color_pattern(base.clone(), pattern, width_mm).unwrap_or_else(|_| base.clone())
}

/// A picture size with the aspect of `area` and a longest edge of `edge` (or the area's own when
/// smaller), at least 16 pixels each.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the sizes are rounded and clamped to 16..=4096 before the cast"
)]
fn fit_size(area: (f32, f32), edge: f32) -> (usize, usize) {
    let (w, h) = (area.0.max(1.0), area.1.max(1.0));
    let scale = (edge / w.max(h)).min(1.0);
    let pixels = |v: f32| (v * scale).round().clamp(16.0, 4096.0) as usize;
    (pixels(w), pixels(h))
}

/// The picture area the window reported, in logical pixels.
fn area_of(window: &ColorPatternWindow) -> (f32, f32) {
    let model = window.global::<ColorPatternModel>();
    let (w, h) = (
        model.get_preview_area_width(),
        model.get_preview_area_height(),
    );
    if w > 1.0 && h > 1.0 {
        (w, h)
    } else {
        DEFAULT_AREA
    }
}

/// How the Cross-Section picture maps to millimeters, for the picture area as laid out now.
pub(super) fn area_view(window: &ColorPatternWindow, stone_width_mm: f64) -> View {
    let (w, h) = fit_size(area_of(window), 4096.0);
    View::new(w, h, stone_width_mm)
}

/// Shows `rgba` as the preview picture.
fn show_raster(model: &ColorPatternModel<'_>, rgba: &Rgba) {
    let (Ok(width), Ok(height)) = (u32::try_from(rgba.width), u32::try_from(rgba.height)) else {
        return;
    };
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(&rgba.data);
    model.set_preview_image(rgba_image(buffer));
}

/// Pushes the handles of `session` (and which one is under the pointer) to the window.
pub(super) fn push_handles(window: &ColorPatternWindow, session: &Session) {
    let view = area_view(window, session.stone_width_mm);
    let points: Vec<PatternHandle> = session
        .handles
        .iter()
        .enumerate()
        .map(|(index, handle)| {
            let (x, y) = handle_fractions(handle, view);
            PatternHandle {
                x,
                y,
                kind: handle.kind,
                hovered: session.hover == Some(index),
            }
        })
        .collect();
    window
        .global::<ColorPatternModel>()
        .set_handle_points(ModelRc::new(VecModel::from(points)));
}

/// The handles of the selected zone for the Cross-Section; none in the other views.
fn current_handles(session: &Session) -> Vec<Handle> {
    let selected = session.selected();
    if session.preview_mode != 2 || selected == 0 {
        return Vec::new();
    }
    session
        .pattern
        .zone(selected)
        .map_or_else(Vec::new, |zone| {
            handles_for(
                selected,
                zone,
                session.pattern.relative_to_stone,
                session.stone_width_mm,
            )
        })
}

/// Redraws the preview for the session's mode. Cancels a Current Design render that is still
/// running. Does nothing while the session is borrowed elsewhere (the caller redraws later).
pub(super) fn refresh(host: &Host) {
    let ticket = host.preview.next_ticket();
    let Ok(mut guard) = host.session.try_borrow_mut() else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    let model = host.window.global::<ColorPatternModel>();
    session.handles = current_handles(session);
    if session
        .hover
        .is_some_and(|index| index >= session.handles.len())
    {
        session.hover = None;
    }
    let scene_of = |session: &Session| scene(&session.pattern, &session.swatch_stone());
    match session.preview_mode {
        1 => {
            if let Some(design) = session.preview_design.clone() {
                let (w, h) = fit_size(area_of(&host.window), DESIGN_EDGE);
                let size = (u32::try_from(w).unwrap_or(1), u32::try_from(h).unwrap_or(1));
                let request = Request {
                    ticket,
                    design,
                    material: preview_material(
                        &session.base_material,
                        &session.pattern,
                        session.stone_width_mm,
                    ),
                    follows_axis: session.pattern.axis.follows_crystal_axis(),
                    size,
                };
                model.set_preview_busy(host.preview.submit(request));
            } else {
                // No design to draw: the Crystal view stands in, and the note says why.
                let (w, h) = fit_size(area_of(&host.window), RASTER_EDGE);
                show_raster(&model, &render_rod(&scene_of(session), w, h));
                model.set_preview_busy(false);
                model.set_preview_note(
                    format!("Open a design in the Edit tab to see it here. {PREVIEW_NOTE}").into(),
                );
            }
        }
        0 => {
            let (w, h) = fit_size(area_of(&host.window), RASTER_EDGE);
            show_raster(&model, &render_rod(&scene_of(session), w, h));
            model.set_preview_busy(false);
        }
        _ => {
            let (w, h) = fit_size(area_of(&host.window), RASTER_EDGE);
            let rgba = render_slice(&scene_of(session), f64_of(session.slice_pos), w, h);
            show_raster(&model, &rgba);
            model.set_preview_busy(false);
        }
    }
    push_handles(&host.window, session);
}

/// Cuts the Cross-Section picture again after a drag moved a handle, without cancelling anything
/// else. Cheaper than [`refresh`]: no ticket, no Current Design.
pub(super) fn redraw_slice(host: &Host, session: &mut Session) {
    if session.preview_mode != 2 {
        return;
    }
    let (w, h) = fit_size(area_of(&host.window), RASTER_EDGE);
    let scene = scene(&session.pattern, &session.swatch_stone());
    let rgba = render_slice(&scene, f64_of(session.slice_pos), w, h);
    show_raster(&host.window.global::<ColorPatternModel>(), &rgba);
    session.handles = current_handles(session);
    push_handles(&host.window, session);
}

/// A finished Current Design render arrives on the UI thread.
pub(super) fn deliver(host: &Host, ticket: u64, result: Rendered) {
    if ticket != 0 && ticket != host.preview.current() {
        return;
    }
    let model = host.window.global::<ColorPatternModel>();
    model.set_preview_busy(false);
    match result {
        Ok(buffer) => model.set_preview_image(rgba_image(buffer)),
        Err(sentence) if sentence.is_empty() => {}
        Err(sentence) => model.set_preview_note(format!("{sentence} {PREVIEW_NOTE}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::materials::AbsorptionUnit;
    use pleochroma_cut_core::material::pattern::{
        PatternAxis, PatternColor, TemplateId, ZoneColor, template,
    };

    fn solved_watermelon() -> ColorPattern {
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        pattern.base.color = PatternColor::Single(ZoneColor::solved(
            [55.0, 35.0, 140.0],
            vec![[600.0, 40.0, 0.5]],
        ));
        pattern.zones[0].color = PatternColor::Single(ZoneColor::solved(
            [60.0, 35.0, 0.0],
            vec![[520.0, 40.0, 0.5]],
        ));
        pattern
    }

    #[test]
    fn the_preview_material_carries_the_zones_of_a_pattern() {
        let base = GemMaterial::diamond();
        let material = preview_material(&base, &solved_watermelon(), 7.0);
        let zoning = material.zoning.expect("a watermelon is zoned");
        assert_eq!(zoning.zones.len(), 1);
        assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
    }

    #[test]
    fn a_uniform_pattern_is_the_base_color_alone() {
        let mut pattern = template(TemplateId::Uniform, PatternAxis::TABLE_NORMAL);
        pattern.base.color = PatternColor::Single(ZoneColor::solved(
            [55.0, 35.0, 140.0],
            vec![[600.0, 40.0, 0.5]],
        ));
        let material = preview_material(&GemMaterial::diamond(), &pattern, 7.0);
        assert!(material.zoning.is_none());
        assert_eq!(material.absorption.o_ray.len(), 1);
    }

    #[test]
    fn a_pattern_with_unsolved_colors_leaves_the_material_alone() {
        let pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let base = GemMaterial::diamond();
        assert!(preview_material(&base, &pattern, 7.0).zoning.is_none());
    }

    #[test]
    fn an_unsolved_uniform_pattern_keeps_the_original_spectrum() {
        let pattern = template(TemplateId::Uniform, PatternAxis::TABLE_NORMAL);
        let base = GemMaterial::ametrine();
        assert_eq!(preview_material(&base, &pattern, 7.0), base);
    }

    #[test]
    fn a_picture_keeps_the_aspect_and_stays_small() {
        assert_eq!(fit_size((720.0, 360.0), 360.0), (360, 180));
        assert_eq!(fit_size((200.0, 100.0), 360.0), (200, 100));
        assert_eq!(fit_size((0.0, 0.0), 360.0), (16, 16));
    }
}
