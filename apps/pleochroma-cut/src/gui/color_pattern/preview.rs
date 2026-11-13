//! The stage of the Color Pattern window: the main picture, the inset and the handles over the
//! slice.
//!
//! * Slice and Crystal are window-free CPU rasters (`cross_section.rs`), drawn at once on the UI
//!   thread at a size that keeps a drag of a handle smooth. The slice shows the stone's outline;
//!   the outline solves the design, so a worker makes it ([`OutlineWorker`]) and the slice draws
//!   the last one (or a dashed circle) until the new one arrives.
//! * Stone is a small Path-Traced render of the open design with the pattern applied, on the CPU
//!   tracer (the one renderer that always traces zones). It runs on a worker, waits [`DEBOUNCE`]
//!   after the last edit and is cancelled by the next one. It posts a quick frame, then a refined
//!   one: `render_state` is 2 (updating) until the first arrives, 1 (refining) until the second,
//!   then 0. The Ray-Traced renderer has no zone path, so it is never used here.
//! * The inset shows the other picture: the slice when the main view is Stone or Crystal, the
//!   last Stone render when it is Slice. It never starts a second trace: the one render serves
//!   both places, at the size of the place it is drawn in.
//! * Zones the cutter hid are left out of every picture (`Session::visible`); nothing is stored.

use super::{
    colors::SwatchStone,
    cross_section::{
        Handle, SliceScene, SliceStyle, View, handle_fractions, handle_label, handles_for,
        render_rod, render_slice_styled, scene, stone_outline_mm, zone_at,
    },
    model::{f32_of, f64_of},
    session::{Host, Session, VIEW_CRYSTAL, VIEW_SLICE, VIEW_STONE, with_session},
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
    material::pattern::{ColorPattern, PatternAxis, apply_color_pattern, follow_c_axis},
};
use pleochroma_editor::stone_orientation::orientation_override;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// How long the Current Design render waits after the last edit.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// The longest edge, in pixels, of a Slice or Crystal picture.
const RASTER_EDGE: f32 = 520.0;
/// The longest edge, in pixels, of the Stone render in the main view.
const DESIGN_EDGE: f32 = 320.0;
/// The longest edge, in pixels, of the Stone render when it only fills the inset.
const INSET_EDGE: f32 = 200.0;
/// Samples per pixel of the quick first frame of the Stone render.
const QUICK_SPP: u32 = 6;
/// Samples per pixel of the refined Stone render.
const DESIGN_SPP: u32 = 32;
/// The camera of the Current Design render: yaw and pitch in radians. Nearly from above, so the
/// table shows the colors the light brings out.
const DESIGN_YAW: f32 = 0.6;
const DESIGN_PITCH: f32 = 1.0;
/// The picture area's size, logical pixels, before the window has written its own.
const DEFAULT_AREA: (f32, f32) = (420.0, 300.0);
/// The inset's size, logical pixels, before the window has written its own.
const DEFAULT_INSET: (f32, f32) = (210.0, 180.0);

/// A Stone render: the picture, and whether it is the refined one (the last of its ticket).
pub(super) struct Frame {
    /// The picture.
    pub image: SharedPixelBuffer<Rgba8Pixel>,
    /// The refined frame; the quick one before it is not.
    pub done: bool,
}

/// What the worker hands back: a frame, or the sentence for why there is none.
pub(super) type Rendered = Result<Frame, String>;

/// Posts a finished render to the UI thread (`ticket` 0: a failure of the worker itself).
pub(super) type Post = Arc<dyn Fn(u64, Rendered) + Send + Sync>;

/// What an outline was made for: when one of these changes, the outline must be made again.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct OutlineKey {
    /// The bits of the slice position.
    slice_bits: u32,
    /// The pattern axis (the frame the outline is in).
    axis: PatternAxis,
    /// Whether the pattern has zones (a pattern without any has no frame of its own).
    framed: bool,
    /// The bits of the stone width.
    width_bits: u64,
}

/// The stone's outline in millimeters `(x, z)` of the cutting plane; `None` when the plane misses
/// the stone.
pub(super) type OutlineMm = Option<Vec<(f64, f64)>>;

/// Posts a finished outline to the UI thread.
pub(super) type PostOutline = Arc<dyn Fn(OutlineKey, OutlineMm) + Send + Sync>;

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

/// One stone outline to make: it needs the design and the frame of the pattern.
struct OutlineJob {
    key: OutlineKey,
    design: Design,
    pattern: ColorPattern,
    width_mm: f64,
    slice_pos: f64,
}

/// The threads that render the Current Design preview and work out the stone's outline.
pub(super) struct PreviewWorker {
    worker: LatestWorker<Request>,
    outlines: LatestWorker<OutlineJob>,
    ticket: Arc<AtomicU64>,
}

impl PreviewWorker {
    /// Starts the threads. `post` and `post_outline` are called ON A WORKER THREAD with each
    /// finished render or outline and must hop to the UI thread themselves.
    pub(super) fn new(post: Post, post_outline: PostOutline) -> Self {
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
                render_design(&request, &latest, &mut |result| {
                    post(request.ticket, result);
                });
            },
            move || on_failure(0, Err("The preview could not be drawn.".to_owned())),
        );
        let outlines = LatestWorker::spawn_with_panic_hook(
            "color-pattern-outline",
            move |job: OutlineJob| {
                let stone = SwatchStone {
                    path_mm: 1.0,
                    width_mm: job.width_mm,
                    host: None,
                };
                let scene = scene(&job.pattern, &stone);
                let outline = stone_outline_mm(&scene, &job.design, job.slice_pos);
                post_outline(job.key, outline);
            },
            || {},
        );
        Self {
            worker,
            outlines,
            ticket,
        }
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

    fn submit_outline(&self, job: OutlineJob) -> bool {
        self.outlines.submit(job)
    }
}

/// The yaw, pitch and distance of the Current Design camera for a stone `radius` large.
fn design_pose(radius: f64) -> (f32, f32, f32) {
    (DESIGN_YAW, DESIGN_PITCH, fit_distance_for_radius(radius))
}

/// Renders the design with the pattern on the worker thread: a quick frame, then a refined one,
/// each handed to `emit` unless a newer ticket made it stale (then nothing is emitted).
fn render_design(request: &Request, latest: &AtomicU64, emit: &mut dyn FnMut(Rendered)) {
    let current = || latest.load(Ordering::SeqCst) == request.ticket;
    let stone = cut_geometry(&request.design, None, None);
    if stone.planes.is_empty() {
        emit(Err(
            "The open design does not solve, so it cannot be drawn here.".to_owned(),
        ));
        return;
    }
    if !current() {
        return;
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
    let (width, height) = request.size;
    let expected = (width as usize) * (height as usize) * 4;
    for (spp, done) in [(QUICK_SPP, false), (DESIGN_SPP, true)] {
        if !current() {
            return;
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
            spp,
            DEFAULT_MAX_BOUNCES,
        );
        if rgba.len() != expected {
            emit(Err("The preview could not be drawn.".to_owned()));
            return;
        }
        let mut image = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        image.make_mut_bytes().copy_from_slice(&rgba);
        if !current() {
            return;
        }
        emit(Ok(Frame { image, done }));
    }
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

/// The main picture area the window reported, in logical pixels.
pub(super) fn area_of(window: &ColorPatternWindow) -> (f32, f32) {
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

/// The inset's area the window reported, in logical pixels.
fn inset_area_of(window: &ColorPatternWindow) -> (f32, f32) {
    let model = window.global::<ColorPatternModel>();
    let (w, h) = (model.get_inset_area_width(), model.get_inset_area_height());
    if w > 1.0 && h > 1.0 {
        (w, h)
    } else {
        DEFAULT_INSET
    }
}

/// How the Slice picture maps to millimeters, for the picture area as laid out now.
pub(super) fn area_view(window: &ColorPatternWindow, stone_width_mm: f64) -> View {
    let (w, h) = fit_size(area_of(window), 4096.0);
    View::new(w, h, stone_width_mm)
}

/// `rgba` as a window image; `None` for a picture too large for one.
pub(super) fn raster_image(rgba: &Rgba) -> Option<Image> {
    let (width, height) = (
        u32::try_from(rgba.width).ok()?,
        u32::try_from(rgba.height).ok()?,
    );
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(&rgba.data);
    Some(rgba_image(buffer))
}

/// Shows `rgba` as the main picture.
fn show_main(model: &ColorPatternModel<'_>, rgba: &Rgba) {
    if let Some(image) = raster_image(rgba) {
        model.set_preview_image(image);
    }
}

/// Shows `rgba` in the inset.
fn show_inset(model: &ColorPatternModel<'_>, rgba: &Rgba) {
    if let Some(image) = raster_image(rgba) {
        model.set_inset_image(image);
    }
}

/// The key the outline of the slice as it is now would have.
const fn outline_key(session: &Session, visible: &ColorPattern) -> OutlineKey {
    let (axis, framed) = Session::outline_axis(visible);
    OutlineKey {
        slice_bits: session.slice_pos.to_bits(),
        axis,
        framed,
        width_bits: session.stone_width_mm.to_bits(),
    }
}

/// Asks the outline worker for the stone's outline at the slice, unless the one held (or the one
/// being made) is for it. Without a design there is no outline: the slice draws a circle.
fn request_outline(host: &Host, session: &mut Session, visible: &ColorPattern) {
    let Some(design) = session.preview_design.clone() else {
        return;
    };
    let key = outline_key(session, visible);
    let held = session
        .outline
        .as_ref()
        .is_some_and(|(held, _)| *held == key);
    if held || session.outline_pending == Some(key) {
        return;
    }
    session.outline_pending = Some(key);
    host.preview.submit_outline(OutlineJob {
        key,
        design,
        pattern: visible.clone(),
        width_mm: session.stone_width_mm,
        slice_pos: f64::from(session.slice_pos),
    });
}

/// A finished outline arrives on the UI thread: it is kept, and the slice is drawn with it.
pub(super) fn deliver_outline(host: &Host, key: OutlineKey, outline: OutlineMm) {
    with_session(host, |session| {
        if session.outline_pending == Some(key) {
            session.outline_pending = None;
        }
        session.outline = Some((key, outline));
        redraw_slice(host, session);
    });
}

/// The slice picture of `session` at `width` x `height` pixels: the stone's outline (the last one
/// delivered, or a dashed circle), the selected zone's edge and the other zones dimmed.
fn slice_picture(
    session: &Session,
    scene: &SliceScene,
    kept: &[usize],
    size: (usize, usize),
) -> Rgba {
    let selected = kept.iter().position(|stored| *stored == session.selected());
    let outline = session
        .outline
        .as_ref()
        .and_then(|(_, outline)| outline.as_deref());
    render_slice_styled(
        scene,
        f64_of(session.slice_pos),
        size.0,
        size.1,
        &SliceStyle { selected, outline },
    )
}

/// The stored zone at the picture fractions `(fx, fy)` of the main Slice picture (a press that
/// misses every handle selects it); `None` outside the picture or over a hidden zone's place that
/// no visible zone holds.
pub(super) fn zone_under(session: &Session, view: View, fx: f64, fy: f64) -> Option<usize> {
    let (visible, kept) = session.visible();
    let scene = scene(&visible, &session.swatch_stone());
    let (width, height) = view.size();
    zone_at(
        &scene,
        &view,
        f64_of(session.slice_pos),
        fx * width,
        fy * height,
    )
    .and_then(|at| kept.get(at).copied())
}

/// Pushes the handles of `session` (and which one is under the pointer) to the window, with their
/// labels and the place of the selected zone's name chip.
pub(super) fn push_handles(window: &ColorPatternWindow, session: &Session) {
    let view = area_view(window, session.stone_width_mm);
    let relative = session.pattern.relative_to_stone;
    let points: Vec<PatternHandle> = session
        .handles
        .iter()
        .enumerate()
        .map(|(index, handle)| {
            let (x, y) = handle_fractions(handle, view);
            let label = session
                .pattern
                .zone(handle.zone)
                .map_or_else(String::new, |zone| {
                    handle_label(handle, zone, relative, session.stone_width_mm)
                });
            PatternHandle {
                x,
                y,
                kind: handle.kind,
                hovered: session.hover == Some(index),
                label: label.into(),
            }
        })
        .collect();
    let model = window.global::<ColorPatternModel>();
    // The name chip sits under the first handle of the selected zone; no handle, no chip.
    let (chip_x, chip_y) = session
        .handles
        .first()
        .map_or((-1.0, -1.0), |handle| handle_fractions(handle, view));
    model.set_selected_label_x(chip_x);
    model.set_selected_label_y(chip_y);
    model.set_handle_points(ModelRc::new(VecModel::from(points)));
}

/// The handles of the selected zone for the Slice view; none in the other views.
fn current_handles(session: &Session) -> Vec<Handle> {
    let selected = session.selected();
    if session.view != VIEW_SLICE || selected == 0 {
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

/// The inset's caption for the picture it shows: the Slice, the Stone, or else the Crystal rod.
fn inset_caption(session: &Session, inset_is_slice: bool, inset_is_stone: bool) -> String {
    if inset_is_slice {
        format!(
            "Slice {:.0} % \u{b7} drag to edit",
            f64::from(session.slice_pos) * 100.0
        )
    } else if inset_is_stone {
        "Stone".to_owned()
    } else {
        "Crystal".to_owned()
    }
}

/// Redraws the stage for the session's view. Cancels a Current Design render that is still
/// running and starts the one the stage needs. Does nothing while the session is borrowed
/// elsewhere (the caller redraws later).
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
    let (area, inset_area) = (area_of(&host.window), inset_area_of(&host.window));
    session.drawn_area = area;
    let (visible, kept) = session.visible();
    let scene = scene(&visible, &session.swatch_stone());
    request_outline(host, session, &visible);
    let has_design = session.preview_design.is_some();
    // The Stone view needs a design; without one the Slice stands in.
    let view = if session.view == VIEW_STONE && !has_design {
        VIEW_SLICE
    } else {
        session.view
    };
    match view {
        VIEW_STONE => {
            if let Some(image) = &session.design_image {
                model.set_preview_image(rgba_image(image.clone()));
            }
            show_inset(
                &model,
                &slice_picture(session, &scene, &kept, fit_size(inset_area, RASTER_EDGE)),
            );
            model.set_inset_caption(inset_caption(session, true, false).into());
            session.render_state = 2;
            submit_design(host, session, &visible, ticket, fit_size(area, DESIGN_EDGE));
        }
        VIEW_CRYSTAL => {
            let (w, h) = fit_size(area, RASTER_EDGE);
            show_main(&model, &render_rod(&scene, w, h));
            show_inset(
                &model,
                &slice_picture(session, &scene, &kept, fit_size(inset_area, RASTER_EDGE)),
            );
            model.set_inset_caption(inset_caption(session, true, false).into());
            session.render_state = 0;
        }
        _ => {
            show_main(
                &model,
                &slice_picture(session, &scene, &kept, fit_size(area, RASTER_EDGE)),
            );
            if has_design {
                // The inset is the last Stone render; the render that keeps it current is the
                // only trace, drawn at the inset's own size.
                if let Some(image) = &session.design_image {
                    model.set_inset_image(rgba_image(image.clone()));
                }
                model.set_inset_caption(inset_caption(session, false, true).into());
                submit_design(
                    host,
                    session,
                    &visible,
                    ticket,
                    fit_size(inset_area, INSET_EDGE),
                );
            } else {
                let (w, h) = fit_size(inset_area, RASTER_EDGE);
                show_inset(&model, &render_rod(&scene, w, h));
                model.set_inset_caption(inset_caption(session, false, false).into());
            }
            session.render_state = 0;
        }
    }
    model.set_render_state(session.render_state);
    push_handles(&host.window, session);
}

/// Queues the Current Design render of the visible pattern at `size` pixels.
fn submit_design(
    host: &Host,
    session: &mut Session,
    visible: &ColorPattern,
    ticket: u64,
    size: (usize, usize),
) {
    let Some(design) = session.preview_design.clone() else {
        return;
    };
    let request = Request {
        ticket,
        design,
        material: preview_material(&session.base_material, visible, session.stone_width_mm),
        follows_axis: visible.axis.follows_crystal_axis(),
        size: (
            u32::try_from(size.0).unwrap_or(1),
            u32::try_from(size.1).unwrap_or(1),
        ),
    };
    if !host.preview.submit(request) {
        session.render_state = 0;
    }
}

/// Cuts the slice picture again after a drag moved a handle (or a new outline arrived), without
/// cancelling anything else. Cheaper than [`refresh`]: no ticket, no Current Design. The slice is
/// the main picture in the Slice view and the inset in the others.
pub(super) fn redraw_slice(host: &Host, session: &mut Session) {
    let model = host.window.global::<ColorPatternModel>();
    let (visible, kept) = session.visible();
    let scene = scene(&visible, &session.swatch_stone());
    request_outline(host, session, &visible);
    let slice_is_main = session.view == VIEW_SLICE
        || (session.preview_design.is_none() && session.view == VIEW_STONE);
    if slice_is_main {
        let size = fit_size(area_of(&host.window), RASTER_EDGE);
        show_main(&model, &slice_picture(session, &scene, &kept, size));
        session.handles = current_handles(session);
        push_handles(&host.window, session);
    } else {
        let size = fit_size(inset_area_of(&host.window), RASTER_EDGE);
        show_inset(&model, &slice_picture(session, &scene, &kept, size));
        model.set_inset_caption(inset_caption(session, true, false).into());
    }
}

/// A Current Design frame arrives on the UI thread.
pub(super) fn deliver(host: &Host, ticket: u64, result: Rendered) {
    if ticket != 0 && ticket != host.preview.current() {
        return;
    }
    let model = host.window.global::<ColorPatternModel>();
    with_session(host, |session| {
        match result {
            Ok(frame) => {
                session.render_note = None;
                session.render_state = i32::from(session.view == VIEW_STONE && !frame.done);
                let image = rgba_image(frame.image.clone());
                session.design_image = Some(frame.image);
                match session.view {
                    VIEW_STONE => model.set_preview_image(image),
                    VIEW_SLICE => model.set_inset_image(image),
                    _ => {}
                }
            }
            Err(sentence) => {
                session.render_note = Some(sentence);
                session.render_state = 0;
            }
        }
        model.set_render_state(session.render_state);
        model.set_render_hint(session.render_hint().into());
    });
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
    fn a_pattern_with_its_zones_hidden_is_the_base_color_alone() {
        let mut session = Session::blank(
            solved_watermelon(),
            GemMaterial::diamond(),
            pleochroma_cut_core::material::pattern::OwnColor::single([50.0, 30.0, 100.0]),
        );
        session.hidden.toggle(1);
        let (visible, kept) = session.visible();
        assert_eq!(kept, [0]);
        let material = preview_material(&GemMaterial::diamond(), &visible, 7.0);
        assert!(material.zoning.is_none());
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

    #[test]
    fn the_outline_is_made_again_when_the_slice_or_the_axis_changes() {
        let mut session = Session::blank(
            solved_watermelon(),
            GemMaterial::diamond(),
            pleochroma_cut_core::material::pattern::OwnColor::single([50.0, 30.0, 100.0]),
        );
        let (visible, _) = session.visible();
        let first = outline_key(&session, &visible);
        assert!(first == outline_key(&session, &visible));
        session.slice_pos = 0.25;
        assert!(first != outline_key(&session, &visible));
        session.slice_pos = 0.5;
        let across = ColorPattern {
            axis: PatternAxis::ACROSS,
            ..visible
        };
        assert!(first != outline_key(&session, &across));
    }
}
