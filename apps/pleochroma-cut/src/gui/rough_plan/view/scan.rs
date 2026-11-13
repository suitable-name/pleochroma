//! The scan in the planner's window: the card, the import report, the layers drawn for it,
//! the camera turn of a finding's Show, and the hover tooltip.
//!
//! The report and the geometry come from `mesh_io`, which builds them on its worker thread.
//! This module keeps them in the view's state for as long as the rough is that scan, pushes
//! them into the window, and draws them into every finished frame of the rough view like the
//! Crystal Axis does. The layers are the repaired triangles, the places where the mesh crosses
//! itself and the outline the grid uses. They are in the rough's frame at the size of the
//! import; Fit to Weight scales the rough about its origin, so they follow by one factor.

use super::{
    ViewState,
    interaction::Picked,
    render::{Pixels, camera_for},
    request_render,
    scene::{SceneKind, WorldFrame},
    showing_result,
};
use crate::{
    RoughFinding, RoughPlanModel,
    gui::{
        rough_colour::handles::{Marker, Overlay, Segment, ViewMap, draw_overlay as draw_handles},
        rough_plan::{
            format::{group_thousands, to_i32},
            host::{Host, on_host, on_idle_host},
            mesh_task,
            metrics::PALETTE,
        },
    },
    mesh_io::{
        ImportReport, LEVEL_OK, LEVEL_PROBLEM, LEVEL_REPAIRED, MeshUnit, ReadProgress, ScanLayers,
        ScanVerdict,
    },
};
use glam::{DVec3, Vec3};
use pleochroma::optics::raytracer::Camera;
use pleochroma_cut_core::rough_plan::{RoughBase, RoughLayout, shape::hull};
use pleochroma_solid::preview::CameraPose;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

/// How often the progress of a read is looked at.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

/// The color of the repaired triangles: amber.
const REPAIRED_COLOUR: [u8; 3] = [245, 158, 11];

/// The color of the places where the mesh crosses itself: red.
const CROSSING_COLOUR: [u8; 3] = [239, 68, 68];

/// The color of the outline ghost and of the ring a finding's Show leaves: the accent cyan.
const OUTLINE_COLOUR: [u8; 3] = [56, 189, 248];

/// How opaque the repaired triangles are drawn.
const REPAIRED_ALPHA: f32 = 0.7;

/// The most repaired triangles drawn.
const MAX_REPAIRED_DRAWN: usize = 5000;

/// The radius, in logical pixels, of the ring round a crossing.
const CROSSING_RING_PX: f32 = 12.0;

/// The radius, in logical pixels, of the ring a finding's Show leaves.
const FOCUS_RING_PX: f32 = 20.0;

/// How far from the pointer the tooltip's corner is, in logical pixels.
const TIP_OFFSET: (f32, f32) = (14.0, 18.0);

/// What the view knows about the scan the rough was imported from.
#[derive(Default)]
pub(super) struct ScanState {
    /// The file the scan was read from, for reading it again in another unit.
    pub(super) path: Option<PathBuf>,
    /// What the import said; `None` when the rough is not a scan.
    report: Option<ImportReport>,
    /// What the view draws.
    pub(super) layers: Option<ScanLayers>,
    /// The place a finding's Show turned to, in the rough's frame at the import's size.
    focus: Option<DVec3>,
    /// The rough the model had when a file was refused: the refusal stays until the rough is
    /// edited.
    refused_from: Option<RoughBase>,
    /// The progress of the read under way.
    progress: Option<Arc<ReadProgress>>,
    /// Looks at that progress while the read runs.
    timer: Timer,
}

/// The position of `unit` in the window's unit box.
#[must_use]
pub(in crate::gui::rough_plan) fn unit_position(unit: MeshUnit) -> i32 {
    MeshUnit::ALL
        .iter()
        .position(|&u| u == unit)
        .map_or(0, to_i32)
}

/// "48,212 triangles · 1 cavity"; cavities are told only when there are some, and nothing is
/// said of a rough with no triangles to count.
#[must_use]
fn stats_text(triangles: usize, cavities: usize) -> String {
    if triangles == 0 {
        return String::new();
    }
    let mut text = format!(
        "{} triangle{}",
        group_thousands(triangles),
        if triangles == 1 { "" } else { "s" }
    );
    if cavities > 0 {
        let _ = write!(
            text,
            " \u{b7} {cavities} cavit{}",
            if cavities == 1 { "y" } else { "ies" }
        );
    }
    text
}

/// "30.1 × 58.7 × 21.4 mm", the bounding box.
#[must_use]
fn size_text(extents: [f64; 3]) -> String {
    format!(
        "{:.1} \u{d7} {:.1} \u{d7} {:.1} mm",
        extents[0], extents[1], extents[2]
    )
}

/// The file name of `path`, as the card shows it.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Registers the callbacks of the scan card.
pub(super) fn wire(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_finding_show(|index| on_host(|host| finding_show(host, index)));
    model.on_rescan_units(|unit| {
        on_idle_host(move |host| crate::gui::rough_plan::obj_import::rescan(host, unit));
    });
}

/// A scan was imported: its report and layers are what the card and the view show.
///
/// Its file is remembered, and the layer switches take their defaults: Problems when something
/// was repaired or found, Outline when the convex outline stands in for the mesh.
pub(in crate::gui::rough_plan) fn scan_installed(
    host: &Host,
    path: PathBuf,
    unit: MeshUnit,
    report: ImportReport,
    layers: ScanLayers,
) {
    let problems = report.highest_level() >= LEVEL_REPAIRED;
    let outline = report.verdict == ScanVerdict::ConvexOutline;
    {
        let mut session = host.session.borrow_mut();
        let scan = &mut session.view.scan;
        scan.timer.stop();
        scan.progress = None;
        scan.path = Some(path);
        scan.report = Some(report);
        scan.layers = Some(layers);
        scan.focus = None;
        scan.refused_from = None;
    }
    let model = host.window.global::<RoughPlanModel>();
    model.set_show_problems(problems);
    model.set_show_outline(outline);
    model.set_mesh_unit_index(unit_position(unit));
}

/// A file was refused: the card says so, with the reason, and keeps the file so that another
/// unit can be tried. The rough itself is not changed.
pub(in crate::gui::rough_plan) fn scan_refused(
    host: &Host,
    path: PathBuf,
    unit: MeshUnit,
    message: &str,
) {
    {
        let mut session = host.session.borrow_mut();
        let base = session.model.base;
        let scan = &mut session.view.scan;
        scan.timer.stop();
        scan.progress = None;
        scan.path = Some(path);
        scan.report = Some(ImportReport::refused(message));
        scan.layers = None;
        scan.focus = None;
        scan.refused_from = Some(base);
    }
    host.window
        .global::<RoughPlanModel>()
        .set_mesh_unit_index(unit_position(unit));
    scan_push(host);
}

/// A read started: the triangles it has counted are looked at every moment until the mesh job is
/// over, and shown as the card's progress.
pub(in crate::gui::rough_plan) fn scan_read_started(host: &Host, progress: Arc<ReadProgress>) {
    host.window
        .global::<RoughPlanModel>()
        .set_scan_progress(-1.0);
    let mut session = host.session.borrow_mut();
    let scan = &mut session.view.scan;
    scan.progress = Some(progress);
    scan.timer
        .start(TimerMode::Repeated, PROGRESS_INTERVAL, || {
            on_host(poll_progress);
        });
}

/// Shows how far the read is, or stops looking when no mesh job runs any more.
fn poll_progress(host: &Rc<Host>) {
    let busy = mesh_task::is_busy(host);
    let fraction = host
        .session
        .borrow()
        .view
        .scan
        .progress
        .as_ref()
        .and_then(|progress| progress.fraction());
    if !busy {
        let mut session = host.session.borrow_mut();
        session.view.scan.timer.stop();
        session.view.scan.progress = None;
    }
    host.window
        .global::<RoughPlanModel>()
        .set_scan_progress(if busy { fraction.unwrap_or(-1.0) } else { -1.0 });
}

/// The weight check of the window as one more finding, when it says something.
fn weight_finding(model: &RoughPlanModel<'_>) -> Option<RoughFinding> {
    let text = model.get_weight_check_text();
    if text.is_empty() {
        return None;
    }
    let level = match model.get_weight_check_level() {
        1 => LEVEL_OK,
        2 => LEVEL_REPAIRED,
        3 => LEVEL_PROBLEM,
        _ => return None,
    };
    Some(RoughFinding {
        level,
        text,
        detail: model.get_model_text(),
        has_place: false,
    })
}

/// Pushes the scan card's fields into the window: the file, the figures, the verdict and the
/// findings (the weight check last).
pub(in crate::gui::rough_plan) fn scan_push(host: &Host) {
    let model = host.window.global::<RoughPlanModel>();
    let shown = {
        let session = host.session.borrow();
        let scan = &session.view.scan;
        scan.report.as_ref().map(|report| {
            let refused = report.verdict == ScanVerdict::Refused;
            let (stats, size) = if refused {
                (String::new(), String::new())
            } else {
                (
                    stats_text(report.triangles, report.cavities),
                    size_text(session.model.base.bounding_box_extents()),
                )
            };
            (
                scan.path.as_deref().map(file_name).unwrap_or_default(),
                stats,
                size,
                report.verdict.index(),
                report.verdict_text.clone(),
                report.findings.clone(),
            )
        })
    };
    let Some((file, stats, size, verdict, verdict_text, findings)) = shown else {
        model.set_scan_file(SharedString::default());
        model.set_scan_stats(SharedString::default());
        model.set_scan_size_text(SharedString::default());
        model.set_scan_verdict(-1);
        model.set_scan_verdict_text(SharedString::default());
        model.set_scan_findings(ModelRc::new(VecModel::from(Vec::<RoughFinding>::new())));
        return;
    };
    let mut rows: Vec<RoughFinding> = findings
        .into_iter()
        .map(|finding| RoughFinding {
            level: finding.level,
            text: finding.text.into(),
            detail: finding.detail.into(),
            has_place: finding.place.is_some(),
        })
        .collect();
    if verdict != ScanVerdict::Refused.index() {
        rows.extend(weight_finding(&model));
    }
    model.set_scan_file(file.into());
    model.set_scan_stats(stats.into());
    model.set_scan_size_text(size.into());
    model.set_scan_verdict(verdict);
    model.set_scan_verdict_text(verdict_text.into());
    model.set_scan_findings(ModelRc::new(VecModel::from(rows)));
}

/// The rough changed (the shape worker stored a new mesh): the scan state follows it.
///
/// A rough that is no scan has none. A scan the state does not know (a saved plan was opened,
/// or an undo went to another mesh) gets a report that says it came from a saved plan. The same
/// scan at another size (Fit to Weight) keeps its report. A refused file's card stays until the
/// rough is edited.
pub(super) fn sync(host: &Host) {
    let (base, kept, has_report, sticky_refusal) = {
        let session = host.session.borrow();
        let scan = &session.view.scan;
        let base = session.model.base;
        let refused = scan
            .report
            .as_ref()
            .is_some_and(|report| report.verdict == ScanVerdict::Refused);
        let kept = scan
            .layers
            .as_ref()
            .is_some_and(|layers| layers.scale_for(base.bounding_box_extents()).is_some());
        (
            base,
            kept,
            scan.report.is_some(),
            refused && scan.refused_from == Some(base),
        )
    };
    if sticky_refusal {
        return;
    }
    match base {
        RoughBase::Hull { id, .. } => {
            if !kept {
                synthesise(host, base, id);
            }
            scan_push(host);
        }
        _ => {
            if has_report {
                {
                    let mut session = host.session.borrow_mut();
                    let scan = &mut session.view.scan;
                    scan.report = None;
                    scan.layers = None;
                    scan.path = None;
                    scan.focus = None;
                    scan.refused_from = None;
                }
                let model = host.window.global::<RoughPlanModel>();
                model.set_show_problems(false);
                model.set_show_outline(false);
                scan_push(host);
            }
        }
    }
}

/// Gives a hull rough that no import report belongs to (it came from a saved plan) a report that
/// says so, with its figures read from the rough and nothing drawn but its outline.
fn synthesise(host: &Host, base: RoughBase, id: u64) {
    let mesh = hull::mesh(id);
    let planes = base.to_halfspaces(false).map_or(0, |planes| planes.len());
    let (triangles, cavities) = mesh.as_ref().map_or((0, 0), |mesh| {
        (
            mesh.triangles().len(),
            crate::mesh_io::count_cavities(mesh.vertices(), mesh.triangles()),
        )
    });
    let report = ImportReport::saved(mesh.is_some(), planes, triangles, cavities);
    let layers = ScanLayers::outline_only(&base);
    {
        let mut session = host.session.borrow_mut();
        let scan = &mut session.view.scan;
        scan.report = Some(report);
        scan.layers = Some(layers);
        scan.path = None;
        scan.focus = None;
        scan.refused_from = None;
    }
    let model = host.window.global::<RoughPlanModel>();
    model.set_show_problems(false);
    model.set_show_outline(false);
}

/// The camera pose that looks at the centre from the side of `direction` (a direction from the
/// centre of the scene), keeping the distance and the roll.
#[must_use]
fn pose_toward(pose: CameraPose, direction: Vec3) -> CameraPose {
    let length = direction.length();
    if length.is_nan() || length < 1e-6 {
        return pose;
    }
    let unit = direction / length;
    CameraPose {
        yaw: unit.x.atan2(unit.z),
        pitch: unit.y.clamp(-1.0, 1.0).asin(),
        ..pose
    }
}

/// Show on finding `index`: the rough is shown (not a result), the Problems layer is on, and the
/// camera turns to the finding's place, which gets a ring.
fn finding_show(host: &Rc<Host>, index: i32) {
    let place = {
        let session = host.session.borrow();
        let scan = &session.view.scan;
        usize::try_from(index)
            .ok()
            .and_then(|i| scan.report.as_ref()?.findings.get(i)?.place)
    };
    let Some(place) = place else {
        return;
    };
    if showing_result(host) {
        super::deselect_result(host);
    }
    host.window
        .global::<RoughPlanModel>()
        .set_show_problems(true);
    {
        let mut session = host.session.borrow_mut();
        let base = session.model.base;
        let factor = session
            .view
            .scan
            .layers
            .as_ref()
            .and_then(|layers| layers.scale_for(base.bounding_box_extents()))
            .unwrap_or(1.0);
        let at = DVec3::from_array(place);
        session.view.scan.focus = Some(at);
        let world = WorldFrame::for_base(&base).point(at * factor);
        session.view.pose = pose_toward(session.view.pose, world.as_vec3());
    }
    request_render(host);
}

/// The size of the view in logical pixels, rounded.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the view size is a positive logical pixel count; the saturating cast bounds it"
)]
const fn pixel_size(size: (f32, f32)) -> (u32, u32) {
    (size.0.round() as u32, size.1.round() as u32)
}

/// Adds a ring of radius `radius` round `at` to `overlay`.
fn add_ring(overlay: &mut Overlay, at: (f32, f32), radius: f32) {
    const SIDES: usize = 20;
    let point = |k: usize| {
        let angle = std::f32::consts::TAU * k as f32 / SIDES as f32;
        (
            radius.mul_add(angle.cos(), at.0),
            radius.mul_add(angle.sin(), at.1),
        )
    };
    for k in 0..SIDES {
        overlay.segments.push(Segment {
            from: point(k),
            to: point(k + 1),
        });
    }
}

/// The twice-the-area edge function of the point `q` against the edge `a`-`b`.
fn edge_function(a: (f32, f32), b: (f32, f32), q: (f32, f32)) -> f32 {
    (q.0 - a.0).mul_add(b.1 - a.1, -((q.1 - a.1) * (b.0 - a.0)))
}

/// Blends `colour` at `alpha` over the pixels of the triangle `p` (in pixels of the buffer).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the pixel bounds are clamped to the buffer before they become indices, and a blend of two bytes stays a byte"
)]
fn fill_triangle(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    p: [(f32, f32); 3],
    colour: [u8; 3],
    alpha: f32,
) {
    let low = |pick: fn(&(f32, f32)) -> f32| p.iter().map(pick).fold(f32::INFINITY, f32::min);
    let high = |pick: fn(&(f32, f32)) -> f32| p.iter().map(pick).fold(f32::NEG_INFINITY, f32::max);
    let min_x = low(|q| q.0).floor().max(0.0);
    let min_y = low(|q| q.1).floor().max(0.0);
    let max_x = high(|q| q.0).ceil().min(width as f32 - 1.0);
    let max_y = high(|q| q.1).ceil().min(height as f32 - 1.0);
    let area = edge_function(p[0], p[1], p[2]);
    if min_x > max_x || min_y > max_y || area.abs() < 1e-6 {
        return;
    }
    for y in (min_y as usize)..=(max_y as usize) {
        for x in (min_x as usize)..=(max_x as usize) {
            let q = (x as f32 + 0.5, y as f32 + 0.5);
            let w = [
                edge_function(p[1], p[2], q),
                edge_function(p[2], p[0], q),
                edge_function(p[0], p[1], q),
            ];
            let inside = if area > 0.0 {
                w.iter().all(|&v| v >= 0.0)
            } else {
                w.iter().all(|&v| v <= 0.0)
            };
            if !inside {
                continue;
            }
            let at = (y * width as usize + x) * 4;
            if let Some(pixel) = rgba.get_mut(at..at + 4) {
                for (byte, &c) in pixel.iter_mut().zip(&colour) {
                    *byte = f32::from(*byte)
                        .mul_add(1.0 - alpha, f32::from(c) * alpha)
                        .round() as u8;
                }
                pixel[3] = 0xff;
            }
        }
    }
}

/// Draws the repaired triangles that face the camera, as amber fills with their edges on top.
///
/// `factor` carries the layers to the rough's present size, `scale` the view's logical pixels
/// to the frame's physical ones.
fn draw_repaired(
    pixels: &mut Pixels,
    map: &ViewMap,
    camera: &Camera,
    layers: &ScanLayers,
    factor: f64,
    scale: f32,
) {
    let (width, height) = (pixels.width(), pixels.height());
    let mut edges = Overlay::default();
    for &[a, b, c] in layers.repaired.iter().take(MAX_REPAIRED_DRAWN) {
        let corners = [a, b, c].map(|p| p * factor);
        let world = corners.map(|p| map.world(p));
        let normal = (world[1] - world[0]).cross(world[2] - world[0]);
        if normal.dot(camera.origin - world[0]) <= 0.0 {
            continue;
        }
        let [Some(sa), Some(sb), Some(sc)] = corners.map(|p| map.screen(p)) else {
            continue;
        };
        let physical = [sa, sb, sc].map(|(x, y)| (x * scale, y * scale));
        fill_triangle(
            pixels.make_mut_bytes(),
            width,
            height,
            physical,
            REPAIRED_COLOUR,
            REPAIRED_ALPHA,
        );
        for (from, to) in [(sa, sb), (sb, sc), (sc, sa)] {
            edges.segments.push(Segment { from, to });
        }
    }
    draw_handles(
        pixels.make_mut_bytes(),
        width,
        height,
        &edges,
        REPAIRED_COLOUR,
        scale,
    );
}

/// Draws the scan's layers into a finished frame of the rough view.
///
/// With Outline on, the outline the grid uses is drawn as a cyan ghost. With Problems on, the
/// repaired triangles are drawn in amber, with a red ring at every crossing and the ring a
/// finding's Show left. Nothing is drawn over a result, or for a rough that is no scan.
pub(super) fn draw_overlay(host: &Host, pixels: &mut Pixels) {
    let model = host.window.global::<RoughPlanModel>();
    let (problems, outline) = (model.get_show_problems(), model.get_show_outline());
    if !problems && !outline {
        return;
    }
    let session = host.session.borrow();
    let view: &ViewState = &session.view;
    if !matches!(
        view.scene.as_deref().map(|scene| &scene.kind),
        Some(SceneKind::Model(_))
    ) || view.view_size.0 <= 0.0
        || view.view_size.1 <= 0.0
    {
        return;
    }
    let Some(layers) = view.scan.layers.as_ref() else {
        return;
    };
    let base = session.model.base;
    let Some(factor) = layers.scale_for(base.bounding_box_extents()) else {
        return;
    };
    let frame = WorldFrame::for_base(&base);
    let camera = camera_for(view.pose);
    let map = ViewMap {
        camera: &camera,
        size: pixel_size(view.view_size),
        centre: frame.centre,
        scale: frame.scale,
    };
    let (width, height) = (pixels.width(), pixels.height());
    let scale = width as f32 / view.view_size.0;
    if outline {
        let mut ghost = Overlay::default();
        for &[a, b] in &layers.outline {
            if let (Some(from), Some(to)) = (map.screen(a * factor), map.screen(b * factor)) {
                ghost.segments.push(Segment { from, to });
            }
        }
        draw_handles(
            pixels.make_mut_bytes(),
            width,
            height,
            &ghost,
            OUTLINE_COLOUR,
            scale,
        );
    }
    if !problems {
        return;
    }
    draw_repaired(pixels, &map, &camera, layers, factor, scale);
    let mut crossings = Overlay::default();
    for &place in &layers.crossings {
        if let Some(at) = map.screen(place * factor) {
            add_ring(&mut crossings, at, CROSSING_RING_PX);
            crossings.markers.push(Marker { at, lit: true });
        }
    }
    draw_handles(
        pixels.make_mut_bytes(),
        width,
        height,
        &crossings,
        CROSSING_COLOUR,
        scale,
    );
    if let Some(at) = view.scan.focus.and_then(|place| map.screen(place * factor)) {
        let mut ring = Overlay::default();
        add_ring(&mut ring, at, FOCUS_RING_PX);
        draw_handles(
            pixels.make_mut_bytes(),
            width,
            height,
            &ring,
            OUTLINE_COLOUR,
            scale,
        );
    }
}

/// What the hover tooltip of the view says.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Tip {
    /// The bold first line.
    title: String,
    /// The lines under it.
    lines: Vec<String>,
    /// The dot's color: the design's in a result, the accent otherwise.
    color: [u8; 3],
    /// The stone the tip is about, whose axis line is read from the window's stone rows.
    stone: Option<usize>,
}

/// "Click to cut the edge Top-Right" for the hint "Click to cut edge Top-Right".
#[must_use]
fn box_tip_title(hint: &str) -> String {
    hint.replacen("cut edge ", "cut the edge ", 1)
        .replacen("cut corner ", "cut the corner ", 1)
}

/// The tooltip for what is under the pointer: a stone of the result shown (its design, weight,
/// size and place in the cut plan), or the hint of an edge, a corner, a face or a cut.
#[must_use]
pub(super) fn tip_for(
    view: &ViewState,
    layouts: &[RoughLayout],
    picked: Picked,
    hint: &str,
) -> Option<Tip> {
    let accent = OUTLINE_COLOUR;
    match picked {
        Picked::Nothing => None,
        Picked::Stone(stone) => {
            let SceneKind::Fit(fit) = &view.scene.as_deref()?.kind else {
                return None;
            };
            let info = fit.info.get(stone)?;
            let mut lines = Vec::new();
            let size = view
                .fit_result
                .and_then(|result| layouts.get(result))
                .and_then(|layout| layout.stones.get(stone))
                .map(|placed| placed.stone_size_mm);
            lines.push(size.map_or_else(
                || format!("{:.2} ct", info.carat),
                |s| {
                    format!(
                        "{:.2} ct \u{b7} {:.1} \u{d7} {:.1} \u{d7} {:.1} mm",
                        info.carat, s[0], s[1], s[2]
                    )
                },
            ));
            if let Some(position) = info.position {
                lines.push(format!(
                    "slab {}, bar {}, piece {}",
                    position.slab, position.bar, position.piece
                ));
            }
            if !info.note.is_empty() {
                lines.push(info.note.clone());
            }
            let group = fit.stone_group.get(stone).copied().unwrap_or(0);
            Some(Tip {
                title: format!("Stone {} \u{b7} {}", stone + 1, info.title),
                lines,
                color: PALETTE[group % PALETTE.len()],
                stone: Some(stone),
            })
        }
        Picked::Box(_) => (!hint.is_empty()).then(|| Tip {
            title: box_tip_title(hint),
            lines: Vec::new(),
            color: accent,
            stone: None,
        }),
        Picked::Facet(_) | Picked::Cut(_) => (!hint.is_empty()).then(|| Tip {
            title: hint.to_string(),
            lines: Vec::new(),
            color: accent,
            stone: None,
        }),
    }
}

/// Shows `tip` beside the pointer at `at` (view coordinates), or hides the tooltip. A stone's tip
/// gets the axis line of its row in the result card ("Axis 12° from the table normal · 15° off").
pub(super) fn show_tip(host: &Host, tip: Option<&Tip>, at: (f32, f32)) {
    let model = host.window.global::<RoughPlanModel>();
    let Some(tip) = tip else {
        if model.get_hover_tip_visible() {
            model.set_hover_tip_visible(false);
        }
        return;
    };
    if model.get_hover_tip_title().as_str() != tip.title || !model.get_hover_tip_visible() {
        let mut lines = tip.lines.clone();
        if let Some(stone) = tip.stone
            && let Some(row) = model
                .get_stone_rows()
                .iter()
                .find(|row| usize::try_from(row.stone).is_ok_and(|s| s == stone))
        {
            let mut axis = row.angle_text.to_string();
            if !row.status.is_empty() {
                let _ = write!(axis, " \u{b7} {}", row.status);
            }
            if !axis.is_empty() {
                lines.push(axis);
            }
        }
        let rows: Vec<SharedString> = lines.into_iter().map(SharedString::from).collect();
        model.set_hover_tip_title(tip.title.clone().into());
        model.set_hover_tip_lines(ModelRc::new(VecModel::from(rows)));
        model.set_hover_tip_color(slint::Color::from_rgb_u8(
            tip.color[0],
            tip.color[1],
            tip.color[2],
        ));
    }
    model.set_hover_tip_x(at.0 + TIP_OFFSET.0);
    model.set_hover_tip_y(at.1 + TIP_OFFSET.1);
    model.set_hover_tip_visible(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stats_line_counts_triangles_and_names_cavities_only_when_there_are_some() {
        assert_eq!(stats_text(48_212, 1), "48,212 triangles \u{b7} 1 cavity");
        assert_eq!(stats_text(12, 0), "12 triangles");
        assert_eq!(stats_text(5, 2), "5 triangles \u{b7} 2 cavities");
        assert_eq!(stats_text(1, 0), "1 triangle");
        assert_eq!(
            stats_text(0, 0),
            "",
            "a rough with no triangles says nothing"
        );
    }

    #[test]
    fn the_size_line_is_the_bounding_box_in_millimetres() {
        assert_eq!(
            size_text([30.12, 58.7, 21.4]),
            "30.1 \u{d7} 58.7 \u{d7} 21.4 mm"
        );
    }

    #[test]
    fn the_unit_position_is_the_place_in_the_unit_box() {
        assert_eq!(unit_position(MeshUnit::Millimetre), 0);
        assert_eq!(unit_position(MeshUnit::Inch), 3);
        for unit in MeshUnit::ALL {
            assert_eq!(MeshUnit::from_index(unit_position(unit)), unit);
        }
    }

    #[test]
    fn the_camera_turns_to_the_side_a_place_is_on() {
        let pose = CameraPose {
            yaw: 0.6,
            pitch: 0.45,
            roll: 0.0,
            distance: 3.0,
        };
        let front = pose_toward(pose, Vec3::new(0.0, 0.0, 2.0));
        assert!(front.yaw.abs() < 1e-6 && front.pitch.abs() < 1e-6);
        assert!((front.distance - 3.0).abs() < 1e-9, "the distance is kept");
        let right = pose_toward(pose, Vec3::X);
        assert!((right.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let top = pose_toward(pose, Vec3::Y);
        assert!((top.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let centre = pose_toward(pose, Vec3::ZERO);
        assert_eq!((centre.yaw, centre.pitch), (pose.yaw, pose.pitch));
    }

    #[test]
    fn a_box_hint_becomes_the_tooltip_sentence() {
        assert_eq!(
            box_tip_title("Click to cut edge Top-Right"),
            "Click to cut the edge Top-Right"
        );
        assert_eq!(
            box_tip_title("Click to cut corner Top-Front-Left"),
            "Click to cut the corner Top-Front-Left"
        );
    }

    #[test]
    fn a_triangle_is_filled_in_amber_inside_and_not_outside() {
        let mut pixels = vec![0_u8; 10 * 10 * 4];
        fill_triangle(
            &mut pixels,
            10,
            10,
            [(1.0, 1.0), (9.0, 1.0), (1.0, 9.0)],
            [200, 100, 0],
            1.0,
        );
        let at = |x: usize, y: usize| &pixels[(y * 10 + x) * 4..(y * 10 + x) * 4 + 4];
        assert_eq!(at(2, 2), [200, 100, 0, 255]);
        assert_eq!(at(8, 8), [0, 0, 0, 0], "outside the hypotenuse");
        // The same triangle wound the other way fills the same pixels.
        let mut turned = vec![0_u8; 10 * 10 * 4];
        fill_triangle(
            &mut turned,
            10,
            10,
            [(1.0, 1.0), (1.0, 9.0), (9.0, 1.0)],
            [200, 100, 0],
            1.0,
        );
        assert_eq!(pixels, turned);
    }
}
