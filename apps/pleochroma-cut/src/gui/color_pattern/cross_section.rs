//! The window-free pictures of a pattern and the handles over them.
//!
//! * [`render_slice`]: a flat cut through the stone, perpendicular to the pattern axis. Every pixel
//!   asks the zone kernel which zone holds that point (`ZoneKernel::zone_index_at`) and takes the
//!   zone's swatch.
//! * [`render_rod`]: the Crystal view, a stack of such cuts drawn one above the other like a rod.
//! * [`handles_for`], [`dragged_value`]: the in-plane handles of the selected zone (the outer
//!   radius of a rod, the edges of a plane or a stripe, the edges of a wedge) and what a drag of
//!   one of them sets.
//!
//! Everything is in the pattern frame, where the pattern axis is `+Y`: the cut is the plane
//! `y = const`, a picture pixel is `(x, z)`, `x` to the right and `z` down. The angle of a
//! rod's or wedge's reference direction runs from `+X`, counter-clockwise as seen on the picture
//! when the zone's axis is `+Y` (clockwise for `-Y`), as `ZoneShape` defines it.

use super::{
    colors::{Rgb, SwatchStone, pattern_swatches},
    model::f32_of,
};
use crate::gui::rough_colour::wizard::raster::Rgba;
use glam::DVec3;
use pleochroma::optics::{
    absorption::AbsorptionTensor,
    zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneKernel, ZoneShape, ZonedAbsorption},
};
use pleochroma_cut_core::{
    material::pattern::{ColorPattern, PatternZone},
    rough_plan::{colour_fit::zones::ZoneParameter, zoned_plan::resolve_relative},
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

/// A pattern ready to be drawn: its zone kernel (none for a pattern without zones), its frame,
/// the color of each zone and the stone width.
pub(super) struct SliceScene {
    kernel: Option<ZoneKernel>,
    frame: ZoneFrame,
    swatches: Vec<Rgb>,
    width_mm: f64,
}

/// The zones of `pattern` as the kernels read them, with the geometry in millimeters for a stone
/// `width_mm` wide. The colors are placeholders: only the geometry matters here, so a pattern with
/// unsolved colors can be drawn too. `None` for a pattern without zones or with geometry that does
/// not validate.
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
const fn to_f64(value: usize) -> f64 {
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
fn mix(bottom: Rgb, top: Rgb, alpha: f32) -> Rgb {
    let a = alpha.clamp(0.0, 1.0);
    let channel = |b: u8, t: u8| to_byte(f32::from(t).mul_add(a, f32::from(b) * (1.0 - a)));
    [
        channel(bottom[0], top[0]),
        channel(bottom[1], top[1]),
        channel(bottom[2], top[2]),
    ]
}

/// A flat cut through the stone at `slice_pos` (0 to 1 along the pattern axis), `width` x
/// `height` pixels: each pixel in its zone's color, the zone outlines darker, the stone's outline
/// a light ring and the picture outside it dimmed.
pub(super) fn render_slice(
    scene: &SliceScene,
    slice_pos: f64,
    width: usize,
    height: usize,
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
    let y_mm = (slice_pos.clamp(0.0, 1.0) - 0.5) * SLICE_TRAVEL_WIDTHS * scene.width_mm;
    let radius_mm = 0.5 * scene.width_mm;
    let mut zones = Vec::with_capacity(width * height);
    for py in 0..height {
        for px in 0..width {
            let (x, z) = view.to_mm(px, py);
            zones.push(scene.index_at(x, y_mm, z));
        }
    }
    for py in 0..height {
        for px in 0..width {
            let (x, z) = view.to_mm(px, py);
            let here = zones[py * width + px];
            let mut color = scene.swatch_of(here);
            let differs = |other: Option<&usize>| other.is_some_and(|other| *other != here);
            let right = (px + 1 < width).then(|| &zones[py * width + px + 1]);
            let below = (py + 1 < height).then(|| &zones[(py + 1) * width + px]);
            if differs(right) || differs(below) {
                color = mix(color, [0, 0, 0], 0.4);
            }
            let distance = x.hypot(z);
            if distance > radius_mm {
                color = mix(color, BACKGROUND, DIM);
            }
            if (distance - radius_mm).abs() * view.scale < 0.8 {
                color = mix(color, [235, 235, 235], 0.7);
            }
            image.set(px, py, [color[0], color[1], color[2], 255]);
        }
    }
    image
}

/// The Crystal view, `width` x `height` pixels: [`ROD_SLICES`] cuts along the pattern axis drawn
/// as discs seen at a slant, the lowest first, so the higher ones cover the lower.
pub(super) fn render_rod(scene: &SliceScene, width: usize, height: usize) -> Rgba {
    let mut image = Rgba::filled(
        width,
        height,
        [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255],
    );
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
        let image = render_slice(&scene, 0.5, 120, 90);
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
        assert!((handle.x_mm - 0.35 * 7.0).abs() < 1e-9);
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

    #[test]
    fn the_view_maps_pixels_and_fractions_back_and_forth() {
        let view = View::new(200, 100, 6.0);
        let (fx, fy) = view.to_fraction(1.5, -0.5);
        let (x, z) = view.at_fraction(fx, fy);
        assert!((x - 1.5).abs() < 1e-9 && (z + 0.5).abs() < 1e-9);
    }
}
