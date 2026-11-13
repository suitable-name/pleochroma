//! Rasterisation of leaves and tile shading (beam guide §11.5).
//!
//! A leaf's image polygon is clipped against the pixel grid with two Sutherland–Hodgman passes:
//! strip by strip to `[y − 0.5, y + 0.5)`, then cell by cell to `[x − 0.5, x + 0.5)`. The pixel
//! rule is **half-open** (`geom::side`: `cell_side` / `cell_contains`): a vertex on a lower cell
//! edge is inside, one on an upper edge is outside, so an edge on a pixel boundary counts once.
//! The geometry kernel's "on counts as inside" rule is a different convention in other functions.
//!
//! [`shade_tile`] lights the fragments with `lighting::integrate_fragment_cancellable`, weights
//! them with the CIE table of the grid (the arithmetic of `integrate_nodes_to_xyz`) and sums in
//! `f64` in a fixed order (leaf order, then row-major pixels within a leaf).

use std::sync::atomic::{AtomicBool, Ordering};

use glam::{DVec2, DVec3, Vec3};

use super::{
    beam::{Fragment, GROUP, Leaf, TileBuffer},
    geom::{
        Tolerances,
        polygon::{ConvexPolygon, MAX_VERTICES},
        side::{Side, cell_side},
    },
    lighting::{FragmentInput, LightingScene, integrate_fragment_cancellable},
    primary::TileRect,
    stats::BeamStats,
};
use crate::optics::raytracer::cie_1931_cmf;

/// Fragments smaller than this many square pixels are dropped as numerical dust.
///
/// A fragment this small cannot carry measurable energy, and its Wachspress coordinates are not
/// well conditioned. The total over a leaf stays within `1e-15` of the polygon area per fragment.
pub const MIN_FRAGMENT_AREA_PX2: f64 = 1.0e-15;

/// A coordinate axis of the image plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// The `sx` axis (columns).
    X,
    /// The `sy` axis (rows).
    Y,
}

impl Axis {
    /// The coordinate of `p` along this axis.
    const fn coord(self, p: DVec2) -> f64 {
        match self {
            Self::X => p.x,
            Self::Y => p.y,
        }
    }

    /// `p` with its coordinate along this axis replaced by `value`.
    const fn with(self, p: DVec2, value: f64) -> DVec2 {
        match self {
            Self::X => DVec2::new(value, p.y),
            Self::Y => DVec2::new(p.x, value),
        }
    }
}

/// The crossing of the edge `a → b` with the line `axis = edge`, computed once from `a` and put
/// exactly on the line.
fn cut(a: DVec2, b: DVec2, axis: Axis, edge: f64) -> DVec2 {
    let (ca, cb) = (axis.coord(a), axis.coord(b));
    let span = cb - ca;
    let t = if span.abs() <= f64::MIN_POSITIVE {
        0.0
    } else {
        ((edge - ca) / span).clamp(0.0, 1.0)
    };
    axis.with((b - a).mul_add(DVec2::splat(t), a), edge)
}

/// One Sutherland–Hodgman pass: keeps the part of `points` whose `axis` coordinate satisfies
/// `inside`, with the crossings put on the line `axis = edge`.
fn clip_edge(points: &[DVec2], axis: Axis, edge: f64, inside: impl Fn(f64) -> bool) -> Vec<DVec2> {
    let n = points.len();
    let mut out = Vec::with_capacity(n + 2);
    for (i, &current) in points.iter().enumerate() {
        let next = points[(i + 1) % n];
        let (current_in, next_in) = (inside(axis.coord(current)), inside(axis.coord(next)));
        if current_in {
            out.push(current);
        }
        if current_in != next_in {
            out.push(cut(current, next, axis, edge));
        }
    }
    out
}

/// Clips the ring `points` to the half-open interval `[lo, hi)` along `axis`.
///
/// The pixel rule: a coordinate within `Tolerances::cell_eps` of `lo` (or above it) is inside, and
/// one within `cell_eps` of `hi` (or above it) is outside, so an edge on `hi` keeps the polygon
/// below it. Vertices the tolerance admits slightly below `lo` move onto `lo`, and every crossing
/// lies exactly on its line, so the result stays in the interval. Fewer than three vertices remain
/// when nothing of positive extent is left.
#[must_use]
pub fn clip_interval(points: &[DVec2], axis: Axis, lo: f64, hi: f64) -> Vec<DVec2> {
    let eps = Tolerances::DEFAULT.cell_eps;
    if points.is_empty() {
        return Vec::new();
    }
    let mut above = clip_edge(points, axis, lo, |c| c - lo > -eps);
    for p in &mut above {
        if axis.coord(*p) < lo {
            *p = axis.with(*p, lo);
        }
    }
    if above.is_empty() {
        return above;
    }
    let mut inside = clip_edge(&above, axis, hi, |c| cell_side(c, hi, eps) == Side::Inside);
    dedupe(&mut inside);
    inside
}

/// Removes consecutive duplicate vertices, including a last vertex equal to the first.
fn dedupe(points: &mut Vec<DVec2>) {
    points.dedup();
    while points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
}

/// The area of the ring `points` (shoelace formula, positive when counter-clockwise).
fn ring_area(points: &[DVec2]) -> f64 {
    let n = points.len();
    let mut twice = 0.0;
    for (i, &p) in points.iter().enumerate() {
        twice += p.perp_dot(points[(i + 1) % n]);
    }
    0.5 * twice
}

/// The lowest and highest coordinates of the ring `points`.
fn bounds(points: &[DVec2]) -> (DVec2, DVec2) {
    points.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(lo, hi), &p| (lo.min(p), hi.max(p)),
    )
}

/// The index of the pixel containing `coord` (cell `[i − 0.5, i + 0.5)`), clamped to
/// `lo..hi_exclusive` (and to `lo` for an empty range).
fn pixel_index(coord: f64, lo: u32, hi_exclusive: u32) -> u32 {
    let low = f64::from(lo);
    let high = f64::from(hi_exclusive.saturating_sub(1).max(lo));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the tile's pixel range, a valid u32 range, just before the cast"
    )]
    let index = (coord + 0.5).floor().clamp(low, high) as u32;
    index
}

/// Convex polygons of a ring of at most any size: a ring above [`MAX_VERTICES`] is cut along a
/// diagonal (from vertex 0 to the middle one) until every piece fits.
fn convex_pieces(ring: &[DVec2]) -> Vec<ConvexPolygon> {
    if ring.len() <= MAX_VERTICES {
        return ConvexPolygon::from_image_points(ring).into_iter().collect();
    }
    let mid = ring.len() / 2;
    let mut second: Vec<DVec2> = ring[mid..].to_vec();
    second.push(ring[0]);
    let mut pieces = convex_pieces(&ring[..=mid]);
    pieces.extend(convex_pieces(&second));
    pieces
}

/// Calls `visit` with every pixel cell piece of `image` inside `tile`, strips ascending and cells
/// ascending within a strip (row-major pixel order). Pieces below [`MIN_FRAGMENT_AREA_PX2`] are
/// skipped; a cell whose clip needs more than [`MAX_VERTICES`] vertices gives several pieces.
fn for_each_fragment(
    image: &ConvexPolygon,
    tile: TileRect,
    visit: &mut impl FnMut((u32, u32), ConvexPolygon),
) {
    if tile.is_empty() {
        return;
    }
    let (points, count) = image.points_2d();
    let ring = &points[..count];
    let (lo, hi) = bounds(ring);
    let first_row = pixel_index(lo.y, tile.y0, tile.y1);
    let last_row = pixel_index(hi.y, tile.y0, tile.y1);
    for py in first_row..=last_row {
        let y = f64::from(py);
        let strip = clip_interval(ring, Axis::Y, y - 0.5, y + 0.5);
        if strip.len() < 3 {
            continue;
        }
        let (strip_lo, strip_hi) = bounds(&strip);
        let first_column = pixel_index(strip_lo.x, tile.x0, tile.x1);
        let last_column = pixel_index(strip_hi.x, tile.x0, tile.x1);
        for px in first_column..=last_column {
            let x = f64::from(px);
            let mut cell = clip_interval(&strip, Axis::X, x - 0.5, x + 0.5);
            dedupe(&mut cell);
            if cell.len() < 3 || ring_area(&cell) < MIN_FRAGMENT_AREA_PX2 {
                continue;
            }
            for piece in convex_pieces(&cell) {
                visit((px, py), piece);
            }
        }
    }
}

/// The pieces of `image` in the pixel cells of `tile`, with their pixels, in row-major order.
///
/// The polygon is clipped strip by strip and cell by cell with the half-open rule of
/// [`clip_interval`]; the pieces tile the part of the polygon inside the tile exactly.
#[must_use]
pub fn rasterise_polygon(
    image: &ConvexPolygon,
    tile: TileRect,
) -> Vec<((u32, u32), ConvexPolygon)> {
    let mut out = Vec::new();
    for_each_fragment(image, tile, &mut |pixel, piece| out.push((pixel, piece)));
    out
}

/// The fragments of `leaf` in `tile`: its image polygon cut at the pixel grid (beam guide §11.5).
///
/// Fragments come in row-major pixel order. The leaf here is a single one, so every fragment's
/// `leaf` index is 0; a caller with a list of leaves renumbers it.
#[must_use]
pub fn rasterise(leaf: &Leaf, tile: TileRect) -> Vec<Fragment> {
    let mut out = Vec::new();
    for_each_fragment(&leaf.image, tile, &mut |pixel, polygon| {
        out.push(Fragment {
            leaf: 0,
            pixel,
            polygon,
        });
    });
    out
}

/// The CIE observer at the node centres of one grid, evaluated once per `shade_tile` call.
///
/// The node wavelengths depend on the grid only, so the per-fragment weighting reads the table
/// instead of re-evaluating the observer.
pub struct CieTable {
    /// The node spacing in nm, as `integrate_nodes_to_xyz` computes it.
    delta: f32,
    /// The normalisation `delta / 106.856`.
    norm_factor: f32,
    /// `cie_1931_cmf` at the centre of node `i`, for `i < n_lambda`.
    cmf: Vec<Vec3>,
}

impl CieTable {
    /// The table of an `n_lambda`-node grid.
    #[must_use]
    pub fn new(n_lambda: u32) -> Self {
        let delta = 400.0_f32 / f32::from(u16::try_from(n_lambda.max(1)).unwrap_or(u16::MAX));
        let cmf = (0..n_lambda)
            .map(|node| cie_1931_cmf(Self::lambda_nm(node, delta)))
            .collect();
        Self {
            delta,
            norm_factor: delta / 106.856,
            cmf,
        }
    }

    /// The centre wavelength of node `node`, with the arithmetic of `integrate_nodes_to_xyz`.
    #[must_use]
    fn lambda_nm(node: u32, delta: f32) -> f32 {
        let index = f32::from(u16::try_from(node).unwrap_or(u16::MAX));
        (index + 0.5).mul_add(delta, 380.0)
    }

    /// The XYZ of the node values `node_values`, the first being node `first_node`: bit-identical
    /// to `integrate_nodes_to_xyz`, with the observer taken from the table.
    #[must_use]
    pub fn integrate(&self, node_values: &[f32], first_node: u32) -> Vec3 {
        let mut xyz = Vec3::ZERO;
        for (i, &value) in node_values.iter().enumerate() {
            let node = first_node.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            let cmf = usize::try_from(node)
                .ok()
                .and_then(|n| self.cmf.get(n).copied())
                .unwrap_or_else(|| cie_1931_cmf(Self::lambda_nm(node, self.delta)));
            xyz += cmf * (value * self.norm_factor);
        }
        xyz
    }
}

/// The XYZ of one fragment: every 8-node group lit with `integrate_fragment` and weighted with the
/// CIE functions (`CieTable::integrate`, the arithmetic of `integrate_nodes_to_xyz`), summed in
/// `f64`.
fn fragment_xyz(
    scene: &LightingScene<'_>,
    cie: &CieTable,
    leaf: &Leaf,
    polygon: &ConvexPolygon,
    cancel: &AtomicBool,
) -> Option<DVec3> {
    let nodes = leaf.nodes();
    let mut xyz = DVec3::ZERO;
    for group in 0..leaf.spectrum.groups() {
        let values = integrate_fragment_cancellable(
            scene,
            &FragmentInput {
                leaf,
                polygon,
                group,
            },
            cancel,
        )?;
        let first = usize::from(leaf.spectrum.first) + group * GROUP;
        let count = (nodes - group * GROUP).min(GROUP);
        let first_node = u32::try_from(first).unwrap_or(u32::MAX);
        xyz += cie.integrate(&values[..count], first_node).as_dvec3();
    }
    Some(xyz)
}

/// Shades the leaves of one tile: the per-pixel XYZ of the beams (before white balance and the end
/// clamp), as the area-weighted mean over each pixel (beam guide §5.2).
///
/// Leaves are taken in the order given and their pixels row-major, so every pixel sums its
/// fragments in a fixed order. `stats.fragments` counts the fragments shaded. Leaves outside the
/// tile contribute only where they overlap it.
#[must_use]
pub fn shade_tile(
    leaves: &[Leaf],
    tile: TileRect,
    scene: &LightingScene<'_>,
    stats: &mut BeamStats,
) -> TileBuffer {
    let never = AtomicBool::new(false);
    shade_tile_cancellable(leaves, tile, scene, stats, &never)
        .unwrap_or_else(|| TileBuffer::new((tile.x0, tile.y0), (tile.width(), tile.height())))
}

/// [`shade_tile`] that polls `cancel` before every fragment, `None` once it is set.
///
/// A large leaf is the longest stretch of shading, so the poll is per fragment: a poll per leaf
/// could delay a cancel by a whole leaf. The summation order is unchanged, so an uncancelled
/// result is bit-identical to [`shade_tile`]'s.
#[must_use]
pub fn shade_tile_cancellable(
    leaves: &[Leaf],
    tile: TileRect,
    scene: &LightingScene<'_>,
    stats: &mut BeamStats,
    cancel: &AtomicBool,
) -> Option<TileBuffer> {
    let mut buffer = TileBuffer::new((tile.x0, tile.y0), (tile.width(), tile.height()));
    let width = tile.width() as usize;
    let cie = CieTable::new(scene.n_lambda);
    let mut stopped = false;
    for leaf in leaves {
        for_each_fragment(&leaf.image, tile, &mut |(px, py), polygon| {
            // The rest of the leaf's fragments are skipped (the clipping still runs, the lighting,
            // which is the cost, does not).
            stopped = stopped || cancel.load(Ordering::Relaxed);
            if stopped {
                return;
            }
            // `None` is a cancel seen inside the integrator.
            let Some(xyz) = fragment_xyz(scene, &cie, leaf, &polygon, cancel) else {
                stopped = true;
                return;
            };
            stats.fragments += 1;
            let local = (py - tile.y0) as usize * width + (px - tile.x0) as usize;
            buffer.xyz[local] += xyz;
        });
        if stopped {
            return None;
        }
    }
    Some(buffer)
}
