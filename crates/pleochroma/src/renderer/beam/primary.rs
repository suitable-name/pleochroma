//! Primary beams: the front facets of the stone seen through the camera, clipped to a tile (beam
//! guide §6.2, §6.4, §7.2 first level).
//!
//! Every ray of a primary beam starts at the camera origin, so the apex is the camera and the
//! image mapping is `geom::homography::camera_to_plane` of the facet plane. The beam's polygon is
//! the facet ring intersected with the preimage of the tile's image rectangle; the clipping runs
//! in image space (the homography maps lines to lines) and the result is carried back to the
//! carrier plane.
//!
//! The sample states of a beam follow the vertex order of its **image polygon** (counter-clockwise
//! in image space): the vertices first, then the area centroid, then the midpoints of the edges
//! (beam guide §6.4). `Beam::polygon` is the same polygon on the carrier plane and may list its
//! vertices in the opposite order; [`image_polygon`] gives the image polygon back in the order of
//! the states.

use glam::{DMat3, DVec2, DVec3};

use super::{
    beam::{Beam, BranchPrefix, GROUP, SampleState, SampleStates, SpectralInterval},
    geom::{
        Tolerances,
        homography::{PlaneBasis, camera_direction_map, camera_to_plane, inverse},
        polygon::ConvexPolygon,
    },
    propagate::{VERTEX_SNAP_REL, projected_ring},
    stats::BeamStats,
    stone::BeamStone,
};
use crate::{
    optics::{
        polarization::StokesVector,
        raytracer::{Camera, Ray},
    },
    renderer::path_cache::signature::BRANCH_SIG_SEED,
};

/// A rectangle of whole pixels, half-open: pixels `x0..x1` by `y0..y1`.
///
/// Pixel `(x, y)` covers the image square `[x - 0.5, x + 0.5) x [y - 0.5, y + 0.5)` (beam guide
/// §5.2), so the rectangle's image extent runs from `image_min` to `image_max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRect {
    /// The first pixel column.
    pub x0: u32,
    /// The first pixel row.
    pub y0: u32,
    /// One past the last pixel column.
    pub x1: u32,
    /// One past the last pixel row.
    pub y1: u32,
}

impl TileRect {
    /// The width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// The height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    /// `true` when the rectangle holds no pixel.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// The number of pixels.
    #[must_use]
    pub const fn pixel_count(self) -> usize {
        self.width() as usize * self.height() as usize
    }

    /// `true` when pixel `(x, y)` lies in the rectangle.
    #[must_use]
    pub const fn contains(self, (x, y): (u32, u32)) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    /// The lower image corner `(x0 - 0.5, y0 - 0.5)`.
    #[must_use]
    pub fn image_min(self) -> DVec2 {
        DVec2::new(f64::from(self.x0) - 0.5, f64::from(self.y0) - 0.5)
    }

    /// The upper image corner `(x1 - 0.5, y1 - 0.5)`, excluded.
    #[must_use]
    pub fn image_max(self) -> DVec2 {
        DVec2::new(f64::from(self.x1) - 0.5, f64::from(self.y1) - 0.5)
    }

    /// The tile grid `(columns, rows)` of a frame of `size` pixels cut into tiles of `tile_size`.
    #[must_use]
    pub const fn grid(size: (u32, u32), tile_size: u32) -> (u32, u32) {
        let tile = if tile_size == 0 { 1 } else { tile_size };
        (size.0.div_ceil(tile), size.1.div_ceil(tile))
    }

    /// Tile `index` of the row-major grid, or `None` past the last tile.
    #[must_use]
    pub const fn of_index(size: (u32, u32), tile_size: u32, index: u32) -> Option<Self> {
        let tile = if tile_size == 0 { 1 } else { tile_size };
        let (columns, rows) = Self::grid(size, tile);
        if columns == 0 || rows == 0 || index >= columns.saturating_mul(rows) {
            return None;
        }
        let (column, row) = (index % columns, index / columns);
        let (x0, y0) = (column.saturating_mul(tile), row.saturating_mul(tile));
        Some(Self {
            x0,
            y0,
            x1: min_u32(x0.saturating_add(tile), size.0),
            y1: min_u32(y0.saturating_add(tile), size.1),
        })
    }
}

/// The smaller of two `u32`, usable in a `const fn`.
const fn min_u32(a: u32, b: u32) -> u32 {
    if a < b { a } else { b }
}

/// The primary beams of one tile: every front facet of the stone, clipped to the tile.
///
/// A facet qualifies when the camera is strictly outside its plane (`BeamStone::front_facets`).
/// The beams come in canonical order: facet id ascending, and the pieces of one facet in a fixed
/// order. Facets with no area inside the tile, or behind the camera, give no beam. `spectrum` is
/// the interval the beams carry (all nodes for a reflection render).
#[must_use]
pub fn primary_beams(
    stone: &BeamStone,
    camera: &Camera,
    size: (u32, u32),
    tile: TileRect,
    spectrum: SpectralInterval,
) -> Vec<Beam> {
    primary_beams_counted(
        stone,
        camera,
        size,
        tile,
        spectrum,
        &mut BeamStats::default(),
    )
}

/// [`primary_beams`] that also counts the gaps it leaves to the residual.
///
/// A front facet with no image (a ring vertex at or behind the camera plane, a degenerate ring)
/// and a tile-clip sliver below `sliver_area_px2` each add one to `stats.silent_gaps`; the
/// slivers' image area is added to `stats.silent_gap_area`. They are gaps, not drops: the residual
/// covers them and no energy is known for them.
#[must_use]
pub fn primary_beams_counted(
    stone: &BeamStone,
    camera: &Camera,
    size: (u32, u32),
    tile: TileRect,
    spectrum: SpectralInterval,
    stats: &mut BeamStats,
) -> Vec<Beam> {
    if size.0 == 0 || size.1 == 0 || tile.is_empty() {
        return Vec::new();
    }
    let eye = camera.origin.as_dvec3();
    let mut beams = Vec::new();
    for facet in stone.front_facets(eye) {
        let Some(image) = facet_image(stone, camera, size, facet) else {
            stats.silent_gaps += 1;
            continue;
        };
        for piece in clip_to_tile(&image, tile, stats) {
            if let Some(beam) = beam_for_image(stone, camera, size, facet, &piece, spectrum) {
                beams.push(beam);
            }
        }
    }
    beams
}

/// The image of `facet` seen from the camera, or `None` for a facet with more than
/// `MAX_VERTICES` ring vertices, a degenerate ring, or a vertex at or behind the camera.
///
/// Every ring vertex is carried onto the facet's plane along its camera ray. A ring vertex can lie
/// off the plane (the stone's planes are `f32`, so a vertex where four or more facets meet is off
/// the planes that did not compute it by up to about `2^-24`), and dropping the normal
/// component instead would move the polygon corner off the ray through the stone vertex. The
/// polygon edges along a silhouette would then miss the pyramid planes of the neighbouring back
/// facets by that offset, and the partition of the beam would lose the strip between them.
fn facet_image(
    stone: &BeamStone,
    camera: &Camera,
    size: (u32, u32),
    facet: u32,
) -> Option<ConvexPolygon> {
    let plane = stone.plane(facet);
    let basis = PlaneBasis::new(plane);
    let eye = camera.origin.as_dvec3();
    let points: Vec<DVec3> = stone
        .ring(facet)
        .iter()
        .map(|&id| {
            let d = stone.vertex(id) - eye;
            let s = (plane.1 - plane.0.dot(eye)) / plane.0.dot(d);
            (s.is_finite() && s > 0.0).then(|| d.mul_add(DVec3::splat(s), eye))
        })
        .collect::<Option<Vec<_>>>()?;
    let carrier = ConvexPolygon::with_frame(&points, basis.frame())?;
    let image_to_plane = camera_to_plane(camera, size.0, size.1, plane);
    carrier.to_image(inverse(image_to_plane)?)
}

/// `image` clipped to the image rectangle of `tile`; slivers below `sliver_area_px2` are dropped
/// and counted as silent gaps in `stats`.
fn clip_to_tile(
    image: &ConvexPolygon,
    tile: TileRect,
    stats: &mut BeamStats,
) -> Vec<ConvexPolygon> {
    let (lo, hi) = (tile.image_min(), tile.image_max());
    let planes = [
        (DVec3::NEG_X, -lo.x),
        (DVec3::X, hi.x),
        (DVec3::NEG_Y, -lo.y),
        (DVec3::Y, hi.y),
    ];
    let mut pieces = vec![*image];
    for (normal, offset) in planes {
        pieces = pieces
            .iter()
            .flat_map(|piece| piece.clip_capped(normal, offset, 0.0).0)
            .collect();
    }
    pieces.retain(|piece| {
        let area = piece.area();
        let keep = area >= Tolerances::DEFAULT.sliver_area_px2;
        if !keep {
            stats.silent_gaps += 1;
            stats.silent_gap_area += area;
        }
        keep
    });
    pieces
}

/// Builds the primary beam of `facet` over the image polygon `image` (counter-clockwise, in
/// `Frame2::IMAGE`).
///
/// The carrier polygon is `image_to_plane(image)`; the sample states are exact camera rays at the
/// image polygon's vertices, centroid and edge midpoints, in that order. Returns `None` when the
/// polygon cannot be carried to the plane.
#[must_use]
pub fn beam_for_image(
    stone: &BeamStone,
    camera: &Camera,
    size: (u32, u32),
    facet: u32,
    image: &ConvexPolygon,
    spectrum: SpectralInterval,
) -> Option<Beam> {
    let plane = stone.plane(facet);
    let basis = PlaneBasis::new(plane);
    let image_to_plane = camera_to_plane(camera, size.0, size.1, plane);
    let mut polygon = image.to_plane(image_to_plane, basis.frame())?;
    // The homography round trip leaves the corners a few 1e-8 off the exact camera rays; put
    // them on the rays so the stored and the partition's snapped polygons coincide.
    let targets = projected_ring(&polygon, camera.origin.as_dvec3(), stone, facet);
    polygon.snap_to(&targets, VERTEX_SNAP_REL * stone.bounding_radius());
    let map = camera_direction_map(camera, size.0, size.1);
    Some(Beam {
        facet,
        polygon,
        apex: camera.origin.as_dvec3(),
        image_to_plane,
        inside: false,
        sig: BRANCH_SIG_SEED,
        prefix: BranchPrefix::EMPTY,
        bounces: 0,
        spectrum,
        state: sample_states(camera, map, image, spectrum),
    })
}

/// The sample states of a primary beam over `image`: vertices, centroid, edge midpoints.
fn sample_states(
    camera: &Camera,
    map: DMat3,
    image: &ConvexPolygon,
    spectrum: SpectralInterval,
) -> SampleStates {
    let (points_2d, count) = image.points_2d();
    let vertices = &points_2d[..count];
    let mut points = Vec::with_capacity(2 * count + 1);
    for &vertex in vertices {
        points.push(camera_state(camera, map, vertex, spectrum));
    }
    points.push(camera_state(
        camera,
        map,
        image.centroid().truncate(),
        spectrum,
    ));
    for (i, &vertex) in vertices.iter().enumerate() {
        let next = vertices[(i + 1) % count];
        points.push(camera_state(camera, map, (vertex + next) * 0.5, spectrum));
    }
    SampleStates {
        points,
        vertices: count,
    }
}

/// The exact state of the camera ray through the image point `image` (beam guide §6.4).
///
/// The direction is the normalised value of the affine map `map` (`camera_direction_map`) in
/// `f64`, so it is the exact camera ray up to the rounding to `f32`. The state is outside the
/// stone, in the ordinary mode, with unpolarised unit intensity in every node of `spectrum`, and
/// its segment starts at the camera.
#[must_use]
pub fn camera_state(
    camera: &Camera,
    map: DMat3,
    image: DVec2,
    spectrum: SpectralInterval,
) -> SampleState {
    let direction = (map * image.extend(1.0)).normalize_or_zero().as_vec3();
    SampleState {
        image,
        ray: Ray {
            origin: camera.origin,
            dir: direction,
        },
        k: direction,
        segment_origin: camera.origin,
        prev_plane: None,
        inside: false,
        is_ext: false,
        stokes: vec![[StokesVector::unpolarized(1.0); GROUP]; spectrum.groups()],
        depth: Vec::new(),
        path_length: 0.0,
    }
}

/// The image polygon of `beam` in the vertex order of its sample states, or `None` when the states
/// do not form a polygon.
#[must_use]
pub fn image_polygon(beam: &Beam) -> Option<ConvexPolygon> {
    let points: Vec<DVec2> = (0..beam.state.vertices)
        .map(|i| beam.state.vertex(i).map(|state| state.image))
        .collect::<Option<_>>()?;
    let polygon = ConvexPolygon::from_image_points(&points)?;
    let (kept, count) = polygon.points_2d();
    (count == points.len() && kept[..count] == points[..]).then_some(polygon)
}

/// Splits a primary beam into two halves by bisecting its image polygon across the longest edge.
///
/// The halves are new primary beams over the two image pieces (fresh exact states); the branch
/// bookkeeping (`sig`, `prefix`, `bounces`) is copied from `beam`. `None` for a beam that is not a
/// primary one, or whose polygon cannot be split.
#[must_use]
pub fn split_beam(
    beam: &Beam,
    stone: &BeamStone,
    camera: &Camera,
    size: (u32, u32),
) -> Option<(Beam, Beam)> {
    if beam.inside || beam.bounces != 0 {
        return None;
    }
    let mut halves = <[_; 2]>::from(image_polygon(beam)?.split_longest_edge()?).map(|piece| {
        beam_for_image(stone, camera, size, beam.facet, &piece, beam.spectrum).map(|mut half| {
            half.sig = beam.sig;
            half.prefix = beam.prefix;
            half.bounces = beam.bounces;
            half
        })
    });
    let second = halves[1].take()?;
    let first = halves[0].take()?;
    Some((first, second))
}
