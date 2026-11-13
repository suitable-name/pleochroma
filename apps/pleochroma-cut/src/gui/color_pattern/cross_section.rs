//! The window-free pictures of a pattern and the handles over them.
//!
//! * [`render_slice_styled`]: a flat cut through the stone, perpendicular to the pattern axis.
//!   Every pixel asks the zone kernel which zone holds that point (`ZoneKernel::zone_index_at`)
//!   and takes the zone's swatch; the stone's outline is drawn over it, the selected zone's edge
//!   in cyan and the other zones dimmed.
//! * [`render_rod`]: the Crystal view, a stack of such cuts drawn one above the other like a rod.
//! * [`stone_outline_mm`] makes that outline from the design's facet planes, and [`zone_at`]
//!   answers which zone a picture point lies in.
//! * [`handles_for`], [`dragged_value`], [`handle_label`]: the in-plane handles of the selected
//!   zone (the outer radius of a rod, the edges of a plane or a stripe, the edges of a wedge),
//!   what a drag of one of them sets and the words the window shows while it is dragged.
//!
//! Everything is in the pattern frame, where the pattern axis is `+Y`: the cut is the plane
//! `y = const`, a picture pixel is `(x, z)`, `x` to the right and `z` down. The angle of a
//! rod's or wedge's reference direction runs from `+X`, counter-clockwise as seen on the picture
//! when the zone's axis is `+Y` (clockwise for `-Y`), as `ZoneShape` defines it.

use super::{
    colors::{Rgb, SwatchStone, pattern_swatches},
    model::{f32_of, to_display},
};
use crate::gui::{rough_colour::wizard::raster::Rgba, solid_preview::cut_slider::cut_geometry};
use glam::DVec3;
use pleochroma::{
    geometry::GpuFacetPlane,
    optics::{
        absorption::AbsorptionTensor,
        zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneKernel, ZoneShape, ZonedAbsorption},
    },
    render_setup::measure_model_width,
};
use pleochroma_cut_core::{
    Design,
    material::pattern::{ColorPattern, PatternZone},
    rough_plan::{
        colour_fit::zones::{ZoneParameter, parameter_value},
        zoned_plan::resolve_relative,
    },
};
use std::f64::consts::{PI, TAU};

/// The picture shows a square this many stone widths wide, centered on the axis.
const VIEW_WIDTHS: f64 = 1.5;
/// The cut moves along this many stone widths between Slice 0 and 1.
const SLICE_TRAVEL_WIDTHS: f64 = 0.8;
/// The color around the stone, as the picture area's own background.
const BACKGROUND: Rgb = [0x12, 0x14, 0x1c];
/// How strongly the picture outside the stone is dimmed.
const DIM: f32 = 0.6;
/// How many cuts the Crystal view stacks.
const ROD_SLICES: usize = 5;
/// How far (pixels) from a handle a pointer still picks it.
pub(super) const PICK_RADIUS_PX: f64 = 12.0;
/// The radius, in stored length units, the wedge handles sit at.
const WEDGE_HANDLE_RADIUS: f64 = 0.35;
/// The selected zone's edge in the styled slice (`#38bdf8`).
const SELECTED_EDGE: Rgb = [0x38, 0xbd, 0xf8];
/// The stone's outline in the styled slice.
const OUTLINE: Rgb = [255, 255, 255];
/// Half the outline's stroke width, pixels (the line is 2 px wide).
const OUTLINE_HALF_WIDTH_PX: f64 = 1.0;
/// How much the zones other than the selected one are darkened (they keep 78 % of their light).
const OTHER_ZONE_DIM: f32 = 0.22;
/// How many segments the dashed stand-in circle is drawn with (every second one is stroked).
const CIRCLE_SEGMENTS: usize = 96;
/// The outline polygon starts as a square this many stone widths wide, which any stone fits in.
const OUTLINE_REACH_WIDTHS: f64 = 3.0;
/// The size of the stone, as a fraction of a thumbnail disc's edge.
const DISC_FILL: f64 = 0.96;

/// A pattern ready to be drawn: its zone kernel (none for a pattern without zones), its frame,
/// the color of each zone and the stone width.
pub(super) struct SliceScene {
    kernel: Option<ZoneKernel>,
    frame: ZoneFrame,
    swatches: Vec<Rgb>,
    width_mm: f64,
}

/// The zones of `pattern` as the kernels read them, in millimeters for a stone `width_mm` wide.
///
/// The colors are placeholders: only the geometry matters here, so a pattern with unsolved
/// colors can be drawn too. `None` for a pattern without zones or with geometry that does not
/// validate.
fn classification(pattern: &ColorPattern, width_mm: f64) -> Option<ZonedAbsorption> {
    if pattern.zones.is_empty() {
        return None;
    }
    let clear = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()));
    let mut zoned = ZonedAbsorption::new(clear.clone());
    zoned.frame = pattern.axis.frame();
    zoned.boundary_softness_mm = pattern.softness;
    zoned.zones = pattern
        .zones
        .iter()
        .map(|zone| Zone {
            shape: zone.shape.clone(),
            absorption: clear.clone(),
        })
        .collect();
    let zoned = if pattern.relative_to_stone {
        resolve_relative(&zoned, width_mm)?
    } else {
        zoned
    };
    zoned.validate().ok()?;
    Some(zoned)
}

/// Builds the scene of `pattern` for `stone` (its width is the stone width); the swatches are the
/// colors of the zones for that stone.
pub(super) fn scene(pattern: &ColorPattern, stone: &SwatchStone<'_>) -> SliceScene {
    let width_mm = stone.width_mm;
    let zoned = classification(pattern, width_mm);
    SliceScene {
        kernel: zoned.as_ref().map(ZoneKernel::new),
        frame: zoned.map_or(ZoneFrame::IDENTITY, |zoned| zoned.frame),
        swatches: pattern_swatches(pattern, stone)
            .into_iter()
            .map(|swatch| swatch.blend)
            .collect(),
        width_mm: width_mm.max(1e-6),
    }
}

impl SliceScene {
    /// The color of the zone that holds the pattern-frame point `(x, y, z)` (millimeters).
    fn color_at(&self, x: f64, y: f64, z: f64) -> Rgb {
        self.swatch_of(self.index_at(x, y, z))
    }

    /// The color of zone `index` (0 is the base).
    fn swatch_of(&self, index: usize) -> Rgb {
        self.swatches
            .get(index)
            .or_else(|| self.swatches.first())
            .copied()
            .unwrap_or(BACKGROUND)
    }

    /// The index of the zone that holds the pattern-frame point (0 is the base).
    fn index_at(&self, x: f64, y: f64, z: f64) -> usize {
        self.kernel.as_ref().map_or(0, |kernel| {
            kernel.zone_index_at(self.frame.point(DVec3::new(x, y, z)))
        })
    }
}

/// How the picture maps to millimeters in the cutting plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct View {
    width: f64,
    height: f64,
    /// Pixels per millimeter.
    scale: f64,
}

impl View {
    /// The view of a `width` x `height` picture of a stone `stone_width_mm` wide.
    pub(super) fn new(width: usize, height: usize, stone_width_mm: f64) -> Self {
        let (w, h) = (to_f64(width).max(1.0), to_f64(height).max(1.0));
        Self {
            width: w,
            height: h,
            scale: w.min(h) / (VIEW_WIDTHS * stone_width_mm.max(1e-6)),
        }
    }

    /// The millimeter position `(x, z)` of the center of pixel `(px, py)`.
    fn to_mm(self, px: usize, py: usize) -> (f64, f64) {
        (
            (to_f64(px) + 0.5 - self.width / 2.0) / self.scale,
            (to_f64(py) + 0.5 - self.height / 2.0) / self.scale,
        )
    }

    /// The position `(x, z)` in millimeters of the picture fractions `(fx, fy)` (0 to 1).
    pub(super) fn at_fraction(self, fx: f64, fy: f64) -> (f64, f64) {
        (
            (fx - 0.5) * self.width / self.scale,
            (fy - 0.5) * self.height / self.scale,
        )
    }

    /// The picture fractions of the position `(x, z)` in millimeters.
    pub(super) fn to_fraction(self, x_mm: f64, z_mm: f64) -> (f64, f64) {
        (
            0.5 + x_mm * self.scale / self.width,
            0.5 + z_mm * self.scale / self.height,
        )
    }

    /// The picture's size in pixels.
    pub(super) const fn size(self) -> (f64, f64) {
        (self.width, self.height)
    }
}

/// A pixel count as a float (the sizes are a few hundred).
#[expect(
    clippy::cast_precision_loss,
    reason = "pixel coordinates are far below f64's 53 bits"
)]
pub(super) const fn to_f64(value: usize) -> f64 {
    value as f64
}

/// A channel value in `0..=255` as a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded and clamped to 0..=255 before the cast"
)]
const fn to_byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Mixes `top` over `bottom` with weight `alpha` of `top`.
pub(super) fn mix(bottom: Rgb, top: Rgb, alpha: f32) -> Rgb {
    let a = alpha.clamp(0.0, 1.0);
    let channel = |b: u8, t: u8| to_byte(f32::from(t).mul_add(a, f32::from(b) * (1.0 - a)));
    [
        channel(bottom[0], top[0]),
        channel(bottom[1], top[1]),
        channel(bottom[2], top[2]),
    ]
}

/// The position of the cut along the pattern axis, millimeters, for `slice_pos` (0 to 1).
fn slice_y_mm(scene: &SliceScene, slice_pos: f64) -> f64 {
    (slice_pos.clamp(0.0, 1.0) - 0.5) * SLICE_TRAVEL_WIDTHS * scene.width_mm
}

/// The zone index of every pixel of a `width` x `height` cut at `y_mm`, row by row.
fn zone_grid(scene: &SliceScene, view: View, y_mm: f64, width: usize, height: usize) -> Vec<usize> {
    let mut zones = Vec::with_capacity(width * height);
    for py in 0..height {
        for px in 0..width {
            let (x, z) = view.to_mm(px, py);
            zones.push(scene.index_at(x, y_mm, z));
        }
    }
    zones
}

/// How the styled slice is drawn.
#[derive(Debug, Clone, Copy)]
pub(super) struct SliceStyle<'a> {
    /// The stored index of the selected zone (0 is the base): its edge is drawn in cyan and the
    /// other zones are dimmed. `None` leaves every zone as it is.
    pub selected: Option<usize>,
    /// The stone's outline in millimeters `(x, z)` of the cutting plane
    /// ([`stone_outline_mm`]). `None` draws a dashed circle one stone width across.
    pub outline: Option<&'a [(f64, f64)]>,
}

/// A pixel count or index as a pixel index no larger than `limit`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=limit before the cast"
)]
const fn clamp_index(value: f64, limit: usize) -> usize {
    value.clamp(0.0, to_f64(limit)) as usize
}

/// The distance from point `p` to the segment `a`-`b`.
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx.mul_add(dx, dy * dy);
    let t = if length_squared <= 1e-12 {
        0.0
    } else {
        ((p.0 - a.0).mul_add(dx, (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0)
    };
    (p.0 - t.mul_add(dx, a.0)).hypot(p.1 - t.mul_add(dy, a.1))
}

/// Raises `coverage` (one value per pixel, `width` x `height`) to the antialiased coverage of
/// the segment `a`-`b`.
///
/// The line is [`OUTLINE_HALF_WIDTH_PX`] * 2 pixels wide. Points are in pixel coordinates, where
/// the center of pixel `(px, py)` is `(px + 0.5, py + 0.5)`.
fn stroke(coverage: &mut [f32], width: usize, height: usize, a: (f64, f64), b: (f64, f64)) {
    let pad = OUTLINE_HALF_WIDTH_PX + 1.5;
    let x0 = clamp_index((a.0.min(b.0) - pad).floor(), width);
    let x1 = clamp_index((a.0.max(b.0) + pad).ceil(), width);
    let y0 = clamp_index((a.1.min(b.1) - pad).floor(), height);
    let y1 = clamp_index((a.1.max(b.1) + pad).ceil(), height);
    for py in y0..y1 {
        for px in x0..x1 {
            let center = (to_f64(px) + 0.5, to_f64(py) + 0.5);
            let distance = segment_distance(center, a, b);
            let value = f32_of((OUTLINE_HALF_WIDTH_PX + 0.5 - distance).clamp(0.0, 1.0));
            let at = py * width + px;
            if value > coverage[at] {
                coverage[at] = value;
            }
        }
    }
}

/// The outline in pixel coordinates, and whether it is drawn dashed.
///
/// The given polygon (millimeters) is drawn solid. Without one (or with fewer than three
/// points), a circle one stone width across is drawn dashed.
fn outline_pixels(
    outline: Option<&[(f64, f64)]>,
    view: View,
    width_mm: f64,
) -> (Vec<(f64, f64)>, bool) {
    let (cx, cy) = (view.width / 2.0, view.height / 2.0);
    outline.filter(|points| points.len() >= 3).map_or_else(
        || {
            let radius = 0.5 * width_mm * view.scale;
            let circle = (0..CIRCLE_SEGMENTS)
                .map(|i| {
                    let angle = TAU * to_f64(i) / to_f64(CIRCLE_SEGMENTS);
                    (
                        radius.mul_add(angle.cos(), cx),
                        radius.mul_add(angle.sin(), cy),
                    )
                })
                .collect();
            (circle, true)
        },
        |points| {
            (
                points
                    .iter()
                    .map(|&(x, z)| (x.mul_add(view.scale, cx), z.mul_add(view.scale, cy)))
                    .collect(),
                false,
            )
        },
    )
}

/// `1.0` for a polygon wound one way and `-1.0` for the other (its signed area's sign), so
/// [`inside_convex`] works for either winding.
fn winding(points: &[(f64, f64)]) -> f64 {
    let area: f64 = points
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = points[(i + 1) % points.len()];
            a.0.mul_add(b.1, -(b.0 * a.1))
        })
        .sum();
    if area < 0.0 { -1.0 } else { 1.0 }
}

/// Whether `(x, y)` lies inside the convex polygon `points`, wound as `wound` ([`winding`])
/// says.
fn inside_convex(points: &[(f64, f64)], wound: f64, x: f64, y: f64) -> bool {
    points.iter().enumerate().all(|(i, a)| {
        let b = points[(i + 1) % points.len()];
        let cross = (b.0 - a.0).mul_add(y - a.1, -((b.1 - a.1) * (x - a.0)));
        wound * cross >= 0.0
    })
}

/// The styled cut through the stone at `slice_pos` (0 to 1 along the pattern axis), for the
/// Slice view.
///
/// Every pixel takes its zone's color. The zones other than `style.selected` are dimmed to 78 %,
/// the edge of the selected zone is drawn in cyan, the picture outside the stone is dimmed and
/// the stone's outline is a 2 px white line (`style.outline`, or a dashed circle one stone width
/// across without one).
pub(super) fn render_slice_styled(
    scene: &SliceScene,
    slice_pos: f64,
    width: usize,
    height: usize,
    style: &SliceStyle<'_>,
) -> Rgba {
    let mut image = Rgba::filled(
        width,
        height,
        [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255],
    );
    if width == 0 || height == 0 {
        return image;
    }
    let view = View::new(width, height, scene.width_mm);
    let zones = zone_grid(scene, view, slice_y_mm(scene, slice_pos), width, height);
    let (outline, dashed) = outline_pixels(style.outline, view, scene.width_mm);
    let wound = winding(&outline);
    let mut coverage = vec![0.0_f32; width * height];
    for (i, &a) in outline.iter().enumerate() {
        if dashed && i % 2 == 1 {
            continue;
        }
        stroke(
            &mut coverage,
            width,
            height,
            a,
            outline[(i + 1) % outline.len()],
        );
    }
    for py in 0..height {
        for px in 0..width {
            let here = zones[py * width + px];
            let mut color = scene.swatch_of(here);
            if style.selected.is_some_and(|selected| selected != here) {
                color = mix(color, [0, 0, 0], OTHER_ZONE_DIM);
            }
            let neighbor = |dx: isize, dy: isize| {
                let nx = px.checked_add_signed(dx)?;
                let ny = py.checked_add_signed(dy)?;
                (nx < width && ny < height).then(|| zones[ny * width + nx])
            };
            let around = [
                neighbor(1, 0),
                neighbor(0, 1),
                neighbor(-1, 0),
                neighbor(0, -1),
            ];
            let differs = |other: Option<usize>| other.is_some_and(|other| other != here);
            if differs(around[0]) || differs(around[1]) {
                color = mix(color, [0, 0, 0], 0.4);
            }
            if !inside_convex(&outline, wound, to_f64(px) + 0.5, to_f64(py) + 0.5) {
                color = mix(color, BACKGROUND, DIM);
            }
            let on_selected_edge = style.selected.is_some_and(|selected| {
                around.iter().any(|other| {
                    other.is_some_and(|other| {
                        other != here && (other == selected || here == selected)
                    })
                })
            });
            if on_selected_edge {
                color = mix(color, SELECTED_EDGE, 0.95);
            }
            let line = coverage[py * width + px];
            if line > 0.0 {
                color = mix(color, OUTLINE, line * 0.95);
            }
            image.set(px, py, [color[0], color[1], color[2], 255]);
        }
    }
    image
}

/// The cut through the stone at `slice_pos` as a disc that fills a `size` x `size` picture, on a
/// transparent background: what a thumbnail of a radial pattern shows.
pub(super) fn render_slice_disc(scene: &SliceScene, slice_pos: f64, size: usize) -> Rgba {
    let mut image = Rgba::filled(size, size, [0, 0, 0, 0]);
    if size == 0 {
        return image;
    }
    let edge = to_f64(size);
    let scale = edge * DISC_FILL / scene.width_mm;
    let view = View {
        width: edge,
        height: edge,
        scale,
    };
    let zones = zone_grid(scene, view, slice_y_mm(scene, slice_pos), size, size);
    let radius_px = 0.5 * edge * DISC_FILL;
    for py in 0..size {
        for px in 0..size {
            let (x, z) = view.to_mm(px, py);
            let distance_px = x.hypot(z) * scale;
            let alpha = (radius_px - distance_px + 0.5).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let here = zones[py * size + px];
            let mut color = scene.swatch_of(here);
            let differs = |other: Option<&usize>| other.is_some_and(|other| *other != here);
            let right = (px + 1 < size).then(|| &zones[py * size + px + 1]);
            let below = (py + 1 < size).then(|| &zones[(py + 1) * size + px]);
            if differs(right) || differs(below) {
                color = mix(color, [0, 0, 0], 0.3);
            }
            image.set(
                px,
                py,
                [color[0], color[1], color[2], to_byte(f32_of(alpha * 255.0))],
            );
        }
    }
    image
}

/// The index of the zone that holds the picture point `(px, py)` (pixels) of the cut at
/// `slice_pos`, as a stored zone index (0 is the base); `None` outside the picture.
pub(super) fn zone_at(
    scene: &SliceScene,
    view: &View,
    slice_pos: f64,
    px: f64,
    py: f64,
) -> Option<usize> {
    let (width, height) = view.size();
    if !(0.0..width).contains(&px) || !(0.0..height).contains(&py) {
        return None;
    }
    let x = (px - width / 2.0) / view.scale;
    let z = (py - height / 2.0) / view.scale;
    Some(scene.index_at(x, slice_y_mm(scene, slice_pos), z))
}

// ---- the stone's outline ---------------------------------------------------------------------

/// `polygon` cut to the half-plane `nx * x + nz * z + offset <= 0` (Sutherland-Hodgman); `None`
/// when nothing is left.
fn clip(polygon: &[(f64, f64)], nx: f64, nz: f64, offset: f64) -> Option<Vec<(f64, f64)>> {
    if nx.hypot(nz) < 1e-9 {
        // A plane parallel to the cutting plane: it keeps the whole cut or none of it.
        return (offset <= 0.0).then(|| polygon.to_vec());
    }
    let side = |point: (f64, f64)| nx.mul_add(point.0, nz.mul_add(point.1, offset));
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (i, &from) in polygon.iter().enumerate() {
        let to = polygon[(i + 1) % polygon.len()];
        let (side_from, side_to) = (side(from), side(to));
        if side_from <= 0.0 {
            out.push(from);
        }
        if (side_from < 0.0 && side_to > 0.0) || (side_from > 0.0 && side_to < 0.0) {
            let along = side_from / (side_from - side_to);
            out.push((
                along.mul_add(to.0 - from.0, from.0),
                along.mul_add(to.1 - from.1, from.1),
            ));
        }
    }
    out.dedup_by(|first, second| {
        (first.0 - second.0).abs() < 1e-9 && (first.1 - second.1).abs() < 1e-9
    });
    (out.len() >= 3).then_some(out)
}

/// The section of the stone bounded by `planes` with the cutting plane `y = y_mm`.
///
/// `planes` are `n . x + d <= 0` inside, in model units. The result is in millimeters `(x, z)`
/// of the pattern frame's cutting plane. `frame` maps pattern-frame points to the stone frame
/// (`frame.point(p)`), and `mm_per_unit` is the millimeters of one model unit. The result is a
/// convex polygon with its corners in order, not repeated at the end. It is `None` when the
/// plane misses the stone. `reach_mm` bounds the polygon: the stone must fit in a square that
/// wide.
fn outline_from_planes(
    planes: &[GpuFacetPlane],
    frame: &ZoneFrame,
    mm_per_unit: f64,
    y_mm: f64,
    reach_mm: f64,
) -> Option<Vec<(f64, f64)>> {
    let mut polygon = vec![
        (-reach_mm, -reach_mm),
        (reach_mm, -reach_mm),
        (reach_mm, reach_mm),
        (-reach_mm, reach_mm),
    ];
    let inverse = frame.rotation.inverse();
    for plane in planes {
        let normal = DVec3::new(
            f64::from(plane.normal[0]),
            f64::from(plane.normal[1]),
            f64::from(plane.normal[2]),
        );
        // `n . (R p + t) + d = (R^-1 n) . p + n . t + d` with the offset in millimeters.
        let local = inverse * normal;
        let offset = f64::from(plane.d).mul_add(mm_per_unit, normal.dot(frame.translation));
        polygon = clip(&polygon, local.x, local.z, local.y.mul_add(y_mm, offset))?;
    }
    Some(polygon)
}

/// The outline of the stone of `design` at `slice_pos` (0 to 1 along the pattern axis).
///
/// It is in millimeters `(x, z)` of the cutting plane, in the scene's pattern frame. It is the
/// finished design's facet planes (`cut_geometry(design, None, None)`, the stone the Current
/// Design preview draws) intersected with the cut, scaled so the girdle is as wide as the stone
/// (the scale the zones get). Concave tools are not part of it.
///
/// `None` when the cut misses the stone or the design does not solve. This solves the design, so
/// call it off the UI thread or keep the result.
pub(super) fn stone_outline_mm(
    scene: &SliceScene,
    design: &Design,
    slice_pos: f64,
) -> Option<Vec<(f64, f64)>> {
    let geometry = cut_geometry(design, None, None);
    let model_width = measure_model_width(&geometry.planes).filter(|width| *width > 1e-9)?;
    outline_from_planes(
        &geometry.planes,
        &scene.frame,
        scene.width_mm / model_width,
        slice_y_mm(scene, slice_pos),
        OUTLINE_REACH_WIDTHS * scene.width_mm,
    )
}

/// The Crystal view, `width` x `height` pixels: [`ROD_SLICES`] cuts along the pattern axis drawn
/// as discs seen at a slant, the lowest first, so the higher ones cover the lower.
pub(super) fn render_rod(scene: &SliceScene, width: usize, height: usize) -> Rgba {
    render_rod_on(
        scene,
        width,
        height,
        [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255],
    )
}

/// [`render_rod`] on a picture that starts as `background` (RGBA): a transparent one makes a
/// thumbnail.
pub(super) fn render_rod_on(
    scene: &SliceScene,
    width: usize,
    height: usize,
    background: [u8; 4],
) -> Rgba {
    let mut image = Rgba::filled(width, height, background);
    if width == 0 || height == 0 {
        return image;
    }
    let (w, h) = (to_f64(width), to_f64(height));
    let mut rx = 0.34 * w;
    let mut ry = 0.42 * rx;
    let mut gap = 0.13 * h;
    let steps = to_f64(ROD_SLICES - 1);
    let total = ry.mul_add(2.0, steps * gap);
    if total > 0.9 * h {
        let shrink = 0.9 * h / total;
        rx *= shrink;
        ry *= shrink;
        gap *= shrink;
    }
    let lowest = h / 2.0 + steps * gap / 2.0;
    let radius_mm = 0.5 * scene.width_mm;
    for k in 0..ROD_SLICES {
        let along = to_f64(k) / steps - 0.5;
        let y_mm = along * SLICE_TRAVEL_WIDTHS * scene.width_mm;
        let cy = to_f64(k).mul_add(-gap, lowest);
        let (top, bottom) = (cy - ry, cy + ry);
        for py in 0..height {
            let fy = to_f64(py) + 0.5;
            if fy < top || fy > bottom {
                continue;
            }
            for px in 0..width {
                let nx = (to_f64(px) + 0.5 - w / 2.0) / rx;
                let ny = (fy - cy) / ry;
                if nx.mul_add(nx, ny * ny) > 1.0 {
                    continue;
                }
                let mut color = scene.color_at(nx * radius_mm, y_mm, ny * radius_mm);
                if nx.mul_add(nx, ny * ny) > 0.9 {
                    color = mix(color, [0, 0, 0], 0.35);
                }
                image.set(px, py, [color[0], color[1], color[2], 255]);
            }
        }
    }
    image
}

// ---- handles ---------------------------------------------------------------------------------

/// One handle over the Cross-Section: what it edits and where it sits (millimeters, `x` and
/// `z` of the cutting plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Handle {
    /// The zone it edits (1 and up).
    pub zone: usize,
    /// The parameter of that zone's shape it sets.
    pub parameter: ZoneParameter,
    /// Position `x`, millimeters.
    pub x_mm: f64,
    /// Position `z`, millimeters.
    pub z_mm: f64,
    /// 0 move, 1 size, 2 rotate: tells the window which shape to draw.
    pub kind: i32,
}

/// A direction that lies in the cutting plane (its `y` is zero).
fn in_plane(direction: DVec3) -> bool {
    direction.y.abs() < 1e-3
}

/// An axis that points along (or against) the pattern axis, so the shape shows as a round or
/// wedge-shaped cut. Returns `+1` or `-1`.
fn along_axis(direction: DVec3) -> Option<f64> {
    (direction.y.abs() > 0.999).then(|| direction.y.signum())
}

/// The in-plane direction at angle `theta` from the reference direction of an axis along `ay` (the
/// sign of its `y`), as `(x, z)`.
fn direction_at(theta: f64, ay: f64) -> (f64, f64) {
    (theta.cos(), -ay * theta.sin())
}

/// The handles of zone `index` in the cutting plane: millimeters in a stone `width_mm` wide.
pub(super) fn handles_for(
    index: usize,
    zone: &PatternZone,
    relative: bool,
    width_mm: f64,
) -> Vec<Handle> {
    let scale = if relative { width_mm } else { 1.0 };
    let handle = |parameter, x: f64, z: f64, kind| Handle {
        zone: index,
        parameter,
        x_mm: x * scale,
        z_mm: z * scale,
        kind,
    };
    match &zone.shape {
        ZoneShape::HalfSpace { normal, offset } if in_plane(*normal) => {
            vec![handle(
                ZoneParameter::Offset,
                normal.x * offset,
                normal.z * offset,
                0,
            )]
        }
        ZoneShape::Slab {
            normal,
            offset_min,
            offset_max,
        } if in_plane(*normal) => vec![
            handle(
                ZoneParameter::OffsetMin,
                normal.x * offset_min,
                normal.z * offset_min,
                0,
            ),
            handle(
                ZoneParameter::OffsetMax,
                normal.x * offset_max,
                normal.z * offset_max,
                0,
            ),
        ],
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_out,
            ..
        } => along_axis(*axis_dir).map_or_else(Vec::new, |ay| {
            let (dx, dz) = direction_at(0.0, ay);
            vec![handle(
                ZoneParameter::ROut,
                r_out.mul_add(dx, axis_point.x),
                r_out.mul_add(dz, axis_point.z),
                1,
            )]
        }),
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            r_out,
            phase,
            ..
        } => along_axis(*axis_dir).map_or_else(Vec::new, |ay| {
            let (dx, dz) = direction_at(*phase, ay);
            vec![handle(
                ZoneParameter::ROut,
                r_out.mul_add(dx, axis_point.x),
                r_out.mul_add(dz, axis_point.z),
                1,
            )]
        }),
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => along_axis(*axis_dir).map_or_else(Vec::new, |ay| {
            [
                (ZoneParameter::AngleFrom, *angle_from),
                (ZoneParameter::AngleTo, *angle_to),
            ]
            .into_iter()
            .map(|(parameter, angle)| {
                let (dx, dz) = direction_at(angle, ay);
                handle(
                    parameter,
                    WEDGE_HANDLE_RADIUS.mul_add(dx, axis_point.x),
                    WEDGE_HANDLE_RADIUS.mul_add(dz, axis_point.z),
                    2,
                )
            })
            .collect()
        }),
        _ => Vec::new(),
    }
}

/// The value to store for `handle` when the pointer is at `(x_mm, z_mm)` in the cutting plane;
/// `None` when the zone's shape has no such handle.
pub(super) fn dragged_value(
    handle: &Handle,
    zone: &PatternZone,
    x_mm: f64,
    z_mm: f64,
    relative: bool,
    width_mm: f64,
) -> Option<f64> {
    let scale = if relative { width_mm } else { 1.0 };
    let (x, z) = (x_mm / scale, z_mm / scale);
    match (&zone.shape, handle.parameter) {
        (ZoneShape::HalfSpace { normal, .. }, ZoneParameter::Offset)
        | (ZoneShape::Slab { normal, .. }, ZoneParameter::OffsetMin | ZoneParameter::OffsetMax) => {
            Some(normal.x.mul_add(x, normal.z * z))
        }
        (
            ZoneShape::CoaxialCylinder {
                axis_point,
                axis_dir,
                ..
            },
            ZoneParameter::ROut,
        ) => {
            let ay = along_axis(*axis_dir)?;
            let (dx, dz) = direction_at(0.0, ay);
            Some((x - axis_point.x).mul_add(dx, (z - axis_point.z) * dz))
        }
        (
            ZoneShape::CoaxialPrism {
                axis_point,
                axis_dir,
                phase,
                ..
            },
            ZoneParameter::ROut,
        ) => {
            let ay = along_axis(*axis_dir)?;
            let (dx, dz) = direction_at(*phase, ay);
            Some((x - axis_point.x).mul_add(dx, (z - axis_point.z) * dz))
        }
        (
            ZoneShape::Sector {
                axis_point,
                axis_dir,
                angle_from,
                angle_to,
            },
            ZoneParameter::AngleFrom | ZoneParameter::AngleTo,
        ) => {
            let ay = along_axis(*axis_dir)?;
            let current = if handle.parameter == ZoneParameter::AngleFrom {
                *angle_from
            } else {
                *angle_to
            };
            let angle = (-ay * (z - axis_point.z)).atan2(x - axis_point.x);
            // The representative of `angle` nearest `current`.
            let turn = (angle - current + PI).rem_euclid(TAU) - PI;
            Some(current + turn)
        }
        _ => None,
    }
}

/// The words the window shows beside `handle` of `zone` while it is hovered or dragged.
///
/// They name what the handle sets and its value now, as the Size slider and the parameter rows
/// show it ("Size 22 %", "Offset X 3 %", "Width 2.4 mm", "Start Angle 15°"). Empty when the
/// zone's shape no longer has that parameter.
///
/// A rod's radius reads "Size" in percent of the stone width and "Width" in millimeters. Both
/// kinds of length are already stored in the units shown, so `_width_mm` (the stone width) is not
/// needed to read them.
pub(super) fn handle_label(
    handle: &Handle,
    zone: &PatternZone,
    relative: bool,
    _width_mm: f64,
) -> String {
    let Some(stored) = parameter_value(&zone.shape, handle.parameter) else {
        return String::new();
    };
    let shown = to_display(handle.parameter, stored, relative);
    let length = |name: &str| {
        if relative {
            format!("{name} {shown:.0} %")
        } else {
            format!("{name} {shown:.1} mm")
        }
    };
    let plane_axis = match &zone.shape {
        ZoneShape::HalfSpace { normal, .. } | ZoneShape::Slab { normal, .. }
            if normal.z.abs() > normal.x.abs() =>
        {
            "Z"
        }
        _ => "X",
    };
    match handle.parameter {
        ZoneParameter::ROut => length(if relative { "Size" } else { "Width" }),
        ZoneParameter::Offset => length(&format!("Offset {plane_axis}")),
        ZoneParameter::OffsetMin => length(&format!("Lower Edge {plane_axis}")),
        ZoneParameter::OffsetMax => length(&format!("Upper Edge {plane_axis}")),
        ZoneParameter::Phase => format!("Rotation {shown:.0}\u{b0}"),
        ZoneParameter::AngleFrom => format!("Start Angle {shown:.0}\u{b0}"),
        ZoneParameter::AngleTo => format!("End Angle {shown:.0}\u{b0}"),
        _ => String::new(),
    }
}

/// The index of the handle nearest to the picture fractions `(fx, fy)` within [`PICK_RADIUS_PX`],
/// if any.
pub(super) fn pick_handle(handles: &[Handle], view: View, fx: f64, fy: f64) -> Option<usize> {
    let (w, h) = view.size();
    handles
        .iter()
        .enumerate()
        .filter_map(|(i, handle)| {
            let (hx, hy) = view.to_fraction(handle.x_mm, handle.z_mm);
            let distance = ((hx - fx) * w).hypot((hy - fy) * h);
            (distance <= PICK_RADIUS_PX).then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A handle's picture position as the `f32` fractions the window takes.
pub(super) fn handle_fractions(handle: &Handle, view: View) -> (f32, f32) {
    let (fx, fy) = view.to_fraction(handle.x_mm, handle.z_mm);
    (f32_of(fx), f32_of(fy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{
        PatternAxis, PatternColor, TemplateId, ZoneColor, template,
    };
    use std::f64::consts::FRAC_PI_2;

    /// A 7 mm stone with no material at hand: the light path is 10 mm.
    const STONE: SwatchStone<'static> = SwatchStone {
        path_mm: 10.0,
        width_mm: 7.0,
        host: None,
    };

    /// A watermelon with solved colors, so the swatches are not the sliders' fallback.
    fn solved_watermelon() -> ColorPattern {
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        pattern.base.color =
            PatternColor::Single(ZoneColor::solved([55.0, 35.0, 140.0], Vec::new()));
        pattern.zones[0].color = PatternColor::Single(ZoneColor::solved(
            [60.0, 35.0, 0.0],
            vec![[560.0, 40.0, 1.0]],
        ));
        pattern
    }

    #[test]
    fn the_center_of_a_watermelon_is_the_core_and_the_corner_is_the_rim() {
        let pattern = solved_watermelon();
        let scene = scene(&pattern, &STONE);
        let (core, rim) = (scene.swatches[1], scene.swatches[0]);
        assert_ne!(core, rim);
        assert_eq!(scene.color_at(0.0, 0.0, 0.0), core);
        assert_eq!(scene.color_at(3.0, 0.0, 3.0), rim);
    }

    #[test]
    fn the_slice_picture_has_the_core_in_the_middle() {
        let pattern = solved_watermelon();
        let scene = scene(&pattern, &STONE);
        let plain = SliceStyle {
            selected: None,
            outline: None,
        };
        let image = render_slice_styled(&scene, 0.5, 120, 90, &plain);
        let middle = image.pixel(60, 45);
        assert_eq!(&middle[..3], &scene.swatches[1][..]);
        assert_eq!(image.width, 120);
        assert_eq!(image.height, 90);
    }

    #[test]
    fn a_pattern_without_zones_is_one_color() {
        let mut pattern = template(TemplateId::Uniform, PatternAxis::TABLE_NORMAL);
        pattern.base.color =
            PatternColor::Single(ZoneColor::solved([50.0, 30.0, 60.0], Vec::new()));
        let scene = scene(&pattern, &STONE);
        assert_eq!(scene.color_at(1.0, 2.0, 3.0), scene.swatches[0]);
        let rod = render_rod(&scene, 100, 100);
        assert_eq!(rod.width, 100);
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "test arithmetic written as the formula"
    )]
    fn the_prism_handle_sits_on_its_apothem_and_a_drag_sets_the_radius() {
        let pattern = solved_watermelon();
        let handles = handles_for(1, &pattern.zones[0], true, 7.0);
        assert_eq!(handles.len(), 1);
        let handle = handles[0];
        assert_eq!(handle.parameter, ZoneParameter::ROut);
        // The watermelon core's apothem is 0.2 of the stone width (cut-core's
        // `WATERMELON_CORE_APOTHEM`).
        assert!((handle.x_mm - 0.2 * 7.0).abs() < 1e-9);
        assert!(handle.z_mm.abs() < 1e-9);
        let value = dragged_value(&handle, &pattern.zones[0], 0.5 * 7.0, 0.0, true, 7.0).unwrap();
        assert!((value - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_plane_in_the_cutting_plane_has_an_offset_handle_and_a_plane_across_it_has_none() {
        let split = template(TemplateId::Split, PatternAxis::TABLE_NORMAL);
        let handles = handles_for(1, &split.zones[0], true, 7.0);
        assert_eq!(handles.len(), 1);
        let moved = dragged_value(&handles[0], &split.zones[0], 1.4, 5.0, true, 7.0).unwrap();
        assert!((moved - 0.2).abs() < 1e-9);
        let bicolor = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
        assert_eq!(handles_for(1, &bicolor.zones[0], true, 7.0).len(), 0);
    }

    #[test]
    fn a_wedge_has_two_rotate_handles_and_the_angle_follows_the_pointer() {
        let sectors = template(TemplateId::Sectors, PatternAxis::TABLE_NORMAL);
        let handles = handles_for(1, &sectors.zones[0], true, 7.0);
        assert_eq!(handles.len(), 2);
        assert!(handles.iter().all(|h| h.kind == 2));
        // The pointer straight up the picture (negative z) is a quarter turn counter-clockwise.
        let angle = dragged_value(&handles[1], &sectors.zones[0], 0.0, -2.0, true, 7.0).unwrap();
        assert!((angle - FRAC_PI_2).abs() < 1e-9, "{angle}");
    }

    #[test]
    fn the_nearest_handle_within_reach_is_picked() {
        let pattern = solved_watermelon();
        let handles = handles_for(1, &pattern.zones[0], true, 7.0);
        let view = View::new(300, 300, 7.0);
        let (fx, fy) = view.to_fraction(handles[0].x_mm, handles[0].z_mm);
        assert_eq!(pick_handle(&handles, view, fx, fy), Some(0));
        assert_eq!(pick_handle(&handles, view, fx + 0.2, fy), None);
    }

    /// An octagonal prism, 2 model units across the flats and 2 high, as closed planes.
    fn octagonal_prism() -> Vec<GpuFacetPlane> {
        let mut planes: Vec<GpuFacetPlane> = (0..8_u8)
            .map(|k| {
                let angle = f32::from(k) * std::f32::consts::FRAC_PI_4;
                GpuFacetPlane::new(glam::Vec3::new(angle.cos(), 0.0, angle.sin()), -1.0)
            })
            .collect();
        planes.push(GpuFacetPlane::new(glam::Vec3::Y, -1.0));
        planes.push(GpuFacetPlane::new(glam::Vec3::NEG_Y, -1.0));
        planes
    }

    #[test]
    fn the_outline_of_an_octagon_is_a_closed_convex_polygon_of_the_stone_width() {
        // One model unit is 3.5 mm, so the stone is 7 mm across the flats.
        let outline = outline_from_planes(&octagonal_prism(), &ZoneFrame::IDENTITY, 3.5, 0.0, 20.0)
            .expect("the middle of the prism is cut");
        assert_eq!(outline.len(), 8);
        let signs: Vec<bool> = (0..outline.len())
            .map(|i| {
                let (first, second, third) =
                    (outline[i], outline[(i + 1) % 8], outline[(i + 2) % 8]);
                let cross = (second.0 - first.0).mul_add(
                    third.1 - second.1,
                    -((second.1 - first.1) * (third.0 - second.0)),
                );
                cross > 0.0
            })
            .collect();
        assert!(signs.iter().all(|s| *s == signs[0]), "turns one way only");
        let (lowest, highest) = outline.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.0), hi.max(p.0))
        });
        assert!((highest - lowest - 7.0).abs() < 1e-3, "{lowest} {highest}");
        // A frame that moves the pattern origin by 1 mm along x moves the stone the other way.
        let moved = ZoneFrame {
            translation: DVec3::X,
            ..ZoneFrame::IDENTITY
        };
        let shifted = outline_from_planes(&octagonal_prism(), &moved, 3.5, 0.0, 20.0).unwrap();
        let low = shifted.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        assert!((low - (lowest - 1.0)).abs() < 1e-3, "{low}");
    }

    #[test]
    fn a_cut_beyond_the_stone_has_no_outline() {
        // The prism is 7 mm high: a cut 4 mm from the middle misses it.
        assert!(
            outline_from_planes(&octagonal_prism(), &ZoneFrame::IDENTITY, 3.5, 4.0, 20.0).is_none()
        );
        assert!(
            outline_from_planes(&octagonal_prism(), &ZoneFrame::IDENTITY, 3.5, 3.0, 20.0).is_some()
        );
    }

    #[test]
    fn a_picture_point_names_its_zone_and_outside_the_picture_none() {
        let scene = scene(&solved_watermelon(), &STONE);
        let view = View::new(120, 90, 7.0);
        assert_eq!(zone_at(&scene, &view, 0.5, 60.0, 45.0), Some(1));
        assert_eq!(zone_at(&scene, &view, 0.5, 8.0, 6.0), Some(0));
        assert_eq!(zone_at(&scene, &view, 0.5, -1.0, 45.0), None);
        assert_eq!(zone_at(&scene, &view, 0.5, 120.0, 45.0), None);
        assert_eq!(zone_at(&scene, &view, 0.5, 60.0, 90.5), None);
    }

    #[test]
    fn the_styled_slice_keeps_the_selected_zone_and_dims_the_others() {
        let scene = scene(&solved_watermelon(), &STONE);
        let plain = SliceStyle {
            selected: None,
            outline: None,
        };
        let selected = SliceStyle {
            selected: Some(1),
            outline: None,
        };
        let before = render_slice_styled(&scene, 0.5, 120, 90, &plain);
        let after = render_slice_styled(&scene, 0.5, 120, 90, &selected);
        // The core in the middle is untouched; the rim 3 mm to the right got darker.
        assert_eq!(after.pixel(60, 45), before.pixel(60, 45));
        let sum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        assert!(sum(after.pixel(86, 45)) < sum(before.pixel(86, 45)));
        // The core's flat side stands 1.4 mm right of the middle (pixel 72): it is drawn in cyan.
        assert!(
            (70..=74).any(|x| {
                let p = after.pixel(x, 45);
                p[0] < 120 && p[1] > 150 && p[2] > 200
            }),
            "the core's edge shows in cyan"
        );
    }

    #[test]
    fn the_styled_slice_dims_outside_the_given_outline() {
        let scene = scene(&solved_watermelon(), &STONE);
        let square = [(-3.0, -3.0), (3.0, -3.0), (3.0, 3.0), (-3.0, 3.0)];
        let style = SliceStyle {
            selected: None,
            outline: Some(&square),
        };
        let image = render_slice_styled(&scene, 0.5, 120, 90, &style);
        let sum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        // Both points are in the rim, 2.3 mm above the middle row: x = 60 is inside the square,
        // x = 5 (6.4 mm left) is far outside it.
        assert!(sum(image.pixel(5, 25)) < sum(image.pixel(60, 25)));
        assert_eq!((image.width, image.height), (120, 90));
    }

    #[test]
    fn the_disc_is_transparent_outside_the_stone() {
        let scene = scene(&solved_watermelon(), &STONE);
        let disc = render_slice_disc(&scene, 0.5, 64);
        assert_eq!(disc.pixel(0, 0)[3], 0);
        assert_eq!(disc.pixel(32, 32)[3], 255);
    }

    #[test]
    fn a_handle_says_what_it_sets_and_its_value() {
        let pattern = solved_watermelon();
        let core = &pattern.zones[0];
        let handle = handles_for(1, core, true, 7.0)[0];
        assert_eq!(handle_label(&handle, core, true, 7.0), "Size 20 %");
        let mut fixed = core.clone();
        if let ZoneShape::CoaxialPrism { r_out, .. } = &mut fixed.shape {
            *r_out = 2.4;
        }
        assert_eq!(handle_label(&handle, &fixed, false, 7.0), "Width 2.4 mm");

        let mut split = template(TemplateId::Split, PatternAxis::TABLE_NORMAL)
            .zones
            .remove(0);
        if let ZoneShape::HalfSpace { offset, .. } = &mut split.shape {
            *offset = 0.03;
        }
        let plane = handles_for(1, &split, true, 7.0)[0];
        assert_eq!(handle_label(&plane, &split, true, 7.0), "Offset X 3 %");

        let sectors = template(TemplateId::Sectors, PatternAxis::TABLE_NORMAL);
        let wedge = handles_for(1, &sectors.zones[0], true, 7.0);
        assert_eq!(
            handle_label(&wedge[0], &sectors.zones[0], true, 7.0),
            "Start Angle 0\u{b0}"
        );
        assert_eq!(
            handle_label(&wedge[1], &sectors.zones[0], true, 7.0),
            "End Angle 120\u{b0}"
        );
    }

    #[test]
    fn the_view_maps_pixels_and_fractions_back_and_forth() {
        let view = View::new(200, 100, 6.0);
        let (fx, fy) = view.to_fraction(1.5, -0.5);
        let (x, z) = view.at_fraction(fx, fy);
        assert!((x - 1.5).abs() < 1e-9 && (z + 0.5).abs() < 1e-9);
    }
}
