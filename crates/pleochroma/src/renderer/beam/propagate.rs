//! Partition by exit facet and projection to the next facet (beam guide §7.2–7.3), and the exact
//! state replay every new sample point goes through (§6.4, §7.3).
//!
//! # Conventions
//!
//! A beam lies on the plane of its carrier facet. A beam produced by an interaction at facet `F`
//! (an entry refraction, an internal reflection) is carried by `F` and its rays travel into the
//! stone; [`partition`] splits its polygon by the facet the rays reach next and [`project_to`]
//! moves a piece onto that facet `G`, where the next interaction happens. The sample states of a
//! beam are the rays **after** the last event of its prefix (origin at the last hit, direction S),
//! so the same state describes the ray on `F` and on `G`; [`retrace_state`] builds it.
//!
//! The pyramid planes of the partition are derived from the apex and the stone's canonical vertex
//! ids only (§13.3), and the projection of every stone vertex from the apex is computed from the
//! same inputs wherever it is needed, so neighbouring facets agree bit for bit.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

use glam::{DMat3, DVec2, DVec3, Vec3};

use super::{
    beam::{Beam, GROUP, Partition, PartitionPiece, SampleState, SampleStates, SpectralNode},
    geom::{
        Tolerances,
        homography::{PlaneBasis, apply, central_projection, compose, inverse},
        polygon::ConvexPolygon,
    },
    interact::{InteractKit, hero_node},
    retrace::{PrefixRay, camera_direction, exact_ray},
    snap::exit_facet,
    stats::BeamStats,
    stone::BeamStone,
};
use crate::{
    optics::{polarization::StokesVector, raytracer::Ray},
    renderer::path_cache::{
        bounce::{EventKind, ExplicitHit, PathState, SegmentDepths, expand_at},
        build::HeroContext,
    },
};

/// The relative tolerance of the conservation check (§13.6).
pub const CONSERVATION_TOLERANCE: f64 = 1e-9;

/// The hit check of [`project_to`] never asks for less than this many side epsilons.
const SNAP_FACTOR: f64 = 10.0;

/// The relative radius within which a polygon corner snaps to the projection of a stone vertex.
///
/// Well above the `2^-24` offset an `f32`-plane stone vertex has from the planes that did not
/// compute it, well below any facet size.
pub const VERTEX_SNAP_REL: f64 = 1e-6;

/// The slack of [`project_to`]'s hit check in chord tolerances: a sample point on a snapped
/// boundary is within `chord_px` of the true boundary, twice that is allowed.
const HIT_SLACK_CHORDS: f64 = 2.0;

/// The largest model-vs-exact hit distance seen by [`project_to`], as `f64` bits (positive floats
/// order like their bits).
static MAX_HIT_DISTANCE_BITS: AtomicU64 = AtomicU64::new(0);

/// The largest distance between an exact hit and the apex-model hit that [`project_to`] has seen
/// since the last reset, in model units (the model's distortion statistic, §7.3).
#[must_use]
pub fn max_model_distortion() -> f64 {
    f64::from_bits(MAX_HIT_DISTANCE_BITS.load(Ordering::Relaxed))
}

/// Resets [`max_model_distortion`] to zero.
pub fn reset_model_distortion() {
    MAX_HIT_DISTANCE_BITS.store(0, Ordering::Relaxed);
}

/// Records one hit distance in the running maximum.
fn record_distortion(distance: f64) {
    if distance.is_finite() && distance >= 0.0 {
        MAX_HIT_DISTANCE_BITS.fetch_max(distance.to_bits(), Ordering::Relaxed);
    }
}

/// An `f64` as `f32`, the precision of the tracer's states.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the sample states are f32 by design"
)]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

// ---- The partition (§7.2) ----

/// The pyramid plane through `apex` and the stone edge `(lo, hi)` (canonical ascending ids), as a
/// unit normal and offset, oriented so `centre` (inside the facet) satisfies `n . x <= m`.
///
/// `None` when the apex lies on the edge's line. The two facets of an edge build the same plane
/// with opposite signs, bit for bit (§13.3).
fn pyramid_plane(apex: DVec3, edge: (DVec3, DVec3), centre: DVec3) -> Option<(DVec3, f64)> {
    let n = (edge.0 - apex).cross(edge.1 - apex);
    let len = n.length();
    if !(len.is_finite() && len > 0.0) {
        return None;
    }
    let unit = n / len;
    let m = unit.dot(apex);
    if unit.dot(centre) - m > 0.0 {
        Some((-unit, -m))
    } else {
        Some((unit, m))
    }
}

/// The projection from `apex` of every stone vertex onto the plane of `polygon`, if inside it.
///
/// One target set for a whole partition: a vertex shared by several facets snaps to one point
/// whichever facet is being clipped. A target outside `polygon` could only pull a boundary vertex
/// off it, so it is dropped.
fn projected_vertices(
    polygon: &ConvexPolygon,
    apex: DVec3,
    stone: &BeamStone,
    eps: f64,
) -> Vec<DVec3> {
    let ids: BTreeSet<u32> = (0..u32::try_from(stone.facet_count()).unwrap_or(0))
        .flat_map(|facet| stone.ring(facet).iter().copied())
        .collect();
    let (n, m) = polygon.plane();
    ids.into_iter()
        .filter_map(|id| {
            let d = stone.vertex(id) - apex;
            let denom = n.dot(d);
            let s = (m - n.dot(apex)) / denom;
            (s.is_finite() && s > 0.0).then(|| d.mul_add(DVec3::splat(s), apex))
        })
        .filter(|target| polygon.contains(*target, eps))
        .collect()
}

/// The projection from `apex` of every ring vertex of `facet` onto the plane of `polygon`.
pub(super) fn projected_ring(
    polygon: &ConvexPolygon,
    apex: DVec3,
    stone: &BeamStone,
    facet: u32,
) -> Vec<DVec3> {
    let (n, m) = polygon.plane();
    stone
        .ring(facet)
        .iter()
        .filter_map(|&id| {
            let d = stone.vertex(id) - apex;
            let denom = n.dot(d);
            let s = (m - n.dot(apex)) / denom;
            (s.is_finite() && s > 0.0).then(|| d.mul_add(DVec3::splat(s), apex))
        })
        .collect()
}

/// `polygon` clipped by the pyramid of `facet` seen from `apex` (§7.2, §13.3).
///
/// The result is the part of the polygon whose rays from the apex reach `facet`: the polygon
/// clipped by the planes through the apex and each ring edge of the facet, oriented to contain the
/// facet. Interior vertices within `VERTEX_SNAP_REL` (scaled by the bounding radius) of a target
/// snap to it. A vertex on the polygon's boundary (within `eps`) stays, because the neighbouring
/// piece and the parent edge hold the same value bit for bit (section 13.3). There are several
/// polygons only when the vertex cap forced a split; the result is empty when the pyramid misses
/// the polygon. No sliver removal happens here. `targets` are the apex projections of the stone's
/// vertices onto the polygon's plane.
#[must_use]
pub fn clip_by_pyramid(
    polygon: &ConvexPolygon,
    apex: DVec3,
    stone: &BeamStone,
    facet: u32,
    eps: f64,
    targets: &[DVec3],
) -> Vec<ConvexPolygon> {
    let ring = stone.ring(facet);
    let count = f64::from(u16::try_from(ring.len()).unwrap_or(u16::MAX));
    let centre = ring
        .iter()
        .fold(DVec3::ZERO, |acc, &id| acc + stone.vertex(id))
        / count;
    let mut pieces = vec![*polygon];
    for edge in 0..ring.len() {
        let [lo, hi] = stone.edge_vertices(facet, edge);
        let Some((n, m)) = pyramid_plane(apex, (stone.vertex(lo), stone.vertex(hi)), centre) else {
            continue;
        };
        let mut next = Vec::with_capacity(pieces.len());
        for piece in &pieces {
            next.extend(piece.clip_capped(n, m, eps).0);
        }
        pieces = next;
        if pieces.is_empty() {
            return pieces;
        }
    }
    let reach = VERTEX_SNAP_REL * stone.bounding_radius();
    for piece in &mut pieces {
        piece.snap_interior_to(targets, reach, polygon, eps);
    }
    pieces
}

/// The shared inputs of one partition.
struct Flood<'a> {
    beam: &'a Beam,
    /// The beam polygon with its corners snapped to the apex rays through the carrier's vertices.
    polygon: ConvexPolygon,
    stone: &'a BeamStone,
    /// The apex projections of every stone vertex onto the polygon's plane: one target set for the
    /// parent and every piece, so a shared vertex snaps to one point whichever facet is clipped.
    targets: Vec<DVec3>,
    eps: f64,
    sliver_area: f64,
}

/// The pieces and the sliver area collected so far.
#[derive(Default)]
struct Collected {
    pieces: Vec<PartitionPiece>,
    sliver_area: f64,
}

impl Flood<'_> {
    /// `true` when the apex lies strictly outside the plane of `facet`: no ray from the apex can
    /// leave the stone through it.
    fn apex_outside(&self, facet: u32) -> bool {
        let (n, m) = self.stone.plane(facet);
        n.dot(self.beam.apex) - m > self.eps
    }

    /// The facet the model ray through the apex and the polygon centroid leaves through.
    fn seed(&self) -> Option<u32> {
        let d = self.polygon.centroid() - self.beam.apex;
        let mut best: Option<(f64, u32)> = None;
        for facet in 0..u32::try_from(self.stone.facet_count()).ok()? {
            if facet == self.beam.facet {
                continue;
            }
            let (n, m) = self.stone.plane(facet);
            let denom = n.dot(d);
            if denom <= 0.0 {
                continue;
            }
            let t = (m - n.dot(self.beam.apex)) / denom;
            if t.is_finite() && t > 0.0 && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, facet));
            }
        }
        best.map(|(_, facet)| facet)
    }

    /// Clips against `facet` and records the pieces; `false` when the raw clip is empty.
    fn clip_facet(&self, facet: u32, out: &mut Collected) -> bool {
        let raws = clip_by_pyramid(
            &self.polygon,
            self.beam.apex,
            self.stone,
            facet,
            self.eps,
            &self.targets,
        );
        if raws.is_empty() {
            return false;
        }
        for raw in raws {
            let area = raw.area();
            if let Some(clean) = raw.remove_degeneracies(self.eps, self.sliver_area) {
                out.sliver_area += (area - clean.area()).max(0.0);
                out.pieces.push(PartitionPiece {
                    to: facet,
                    polygon: clean,
                });
            } else {
                out.sliver_area += area;
            }
        }
        true
    }

    /// The flood of §7.2 from the seed facet.
    fn flood(&self) -> Collected {
        let mut out = Collected::default();
        let count = self.stone.facet_count();
        let mut queued = vec![false; count];
        let mut queue = VecDeque::new();
        if let Some(seed) = self.seed() {
            queued[seed as usize] = true;
            queue.push_back(seed);
        }
        while let Some(facet) = queue.pop_front() {
            if facet == self.beam.facet || self.apex_outside(facet) {
                continue;
            }
            if !self.clip_facet(facet, &mut out) {
                continue;
            }
            for edge in 0..self.stone.ring(facet).len() {
                if let Some(next) = self.stone.neighbour(facet, edge)
                    && !queued[next as usize]
                {
                    queued[next as usize] = true;
                    queue.push_back(next);
                }
            }
        }
        out
    }

    /// The retry: every facet the apex can see the polygon leave through, in ascending order.
    fn every_facet(&self) -> Collected {
        let mut out = Collected::default();
        for facet in 0..u32::try_from(self.stone.facet_count()).unwrap_or(0) {
            if facet != self.beam.facet && !self.apex_outside(facet) {
                self.clip_facet(facet, &mut out);
            }
        }
        out
    }

    /// The partition record of `collected`, with its conservation verdict.
    fn finish(&self, collected: Collected) -> Partition {
        let total = self.polygon.area();
        let sum = collected
            .pieces
            .iter()
            .fold(0.0, |acc, piece| acc + piece.polygon.area())
            + collected.sliver_area;
        Partition {
            conserved: (sum - total).abs() <= CONSERVATION_TOLERANCE * total,
            pieces: collected.pieces,
            sliver_area: collected.sliver_area,
        }
    }
}

/// The polygon of `beam` with its corners snapped onto the apex rays through the carrier's stone
/// vertices.
///
/// Every pyramid plane through such a vertex then contains the corner, so no strip between the
/// polygon edge and a pyramid plane is lost. The partition's pieces tile this polygon, not the
/// stored one, so the boundary snapping of `snap` takes it as the parent.
#[must_use]
pub fn snapped_polygon(beam: &Beam, stone: &BeamStone) -> ConvexPolygon {
    let mut polygon = beam.polygon;
    let targets = projected_ring(&polygon, beam.apex, stone, beam.facet);
    polygon.snap_to(&targets, VERTEX_SNAP_REL * stone.bounding_radius());
    polygon
}

/// Splits `beam` by the facet its rays reach next (§7.2).
///
/// The polygon corners within `VERTEX_SNAP_REL` of the apex ray through a vertex of the beam's
/// facet are snapped onto that ray first, and the pieces and the conservation check refer to the
/// snapped polygon. The flood starts at the facet hit by the model ray through the apex and the
/// polygon centroid and visits neighbours FIFO in ring-edge order. A neighbour is queued before
/// the sliver test, so a strip that is a sliver on one facet still reaches the next. The
/// beam's own facet is skipped. The pieces plus `sliver_area` tile the polygon within 1e-9
/// relative
/// (`conserved`); if they do not, the retry clips against every facet the apex does not see from
/// outside. A result still not `conserved` tells the caller to subdivide the beam and retry.
/// Areas are in carrier-plane units; the sliver threshold is `tol.sliver_area_px2 · R²` for the
/// stone's bounding radius `R` and the side epsilon is `tol.side_eps_rel · R`. The caller counts
/// `partitions` and `repartitions`.
#[must_use]
pub fn partition(beam: &Beam, stone: &BeamStone, tol: &Tolerances) -> Partition {
    let radius = stone.bounding_radius();
    let polygon = snapped_polygon(beam, stone);
    let eps = tol.side_eps_rel * radius;
    let flood = Flood {
        beam,
        targets: projected_vertices(&polygon, beam.apex, stone, eps),
        polygon,
        stone,
        eps,
        sliver_area: tol.sliver_area_px2 * radius * radius,
    };
    let first = flood.finish(flood.flood());
    if first.conserved {
        return first;
    }
    flood.finish(flood.every_facet())
}

// ---- State replay (§6.4, §7.3) ----

/// The merged event code of a child kind in a branch step word: 1 reflect, 3 transmit.
const fn kind_code(kind: EventKind) -> u32 {
    match kind {
        EventKind::Reflect | EventKind::TotalReflect => 1,
        EventKind::Transmit => 3,
    }
}

/// One interior segment of a replayed prefix: its depth data and the `(k, is_ext)` wave it was
/// traced in.
type ReplaySegment = (SegmentDepths, Vec3, bool);

/// The outcome of replaying one group.
struct Replay {
    stokes: [StokesVector; GROUP],
    k: Vec3,
    s: Vec3,
    origin: Vec3,
    prev_plane: Option<Vec3>,
    is_ext: bool,
    path_length: f32,
    /// The interior segments in order (collected only when the kit tracks sub-node depths).
    segments: Vec<ReplaySegment>,
}

/// Replays the prefix of `beam` for one group with `expand_at` at the exact hits `geo`.
fn replay_group(
    image: DVec2,
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    hero: &HeroContext<'_>,
    geo: &PrefixRay,
) -> Option<Replay> {
    let camera = kit.retrace.camera;
    let mut origin = camera.origin.as_dvec3();
    let mut dir = camera_direction(camera, kit.retrace.size, image).normalize();
    let mut path = PathState {
        k: dir.as_vec3(),
        inside: false,
        is_ext: false,
        stokes: [StokesVector::unpolarized(1.0); GROUP],
    };
    let (mut prev_plane, mut s, mut length) = (None, dir.as_vec3(), 0.0_f32);
    let mut segments = Vec::new();
    let steps = beam.prefix.steps();
    for (i, &word) in steps.iter().enumerate() {
        let original = (word >> 8) as usize;
        let hit = *geo.hits.get(i)?;
        if path.inside {
            length += to_f32(*geo.lengths.get(i)?);
        }
        let ray = Ray {
            origin: origin.as_vec3(),
            dir: dir.as_vec3(),
        };
        let at = ExplicitHit {
            point: hit.as_vec3(),
            facet: original,
            normal: Vec3::from(kit.planes.get(original)?.normal),
        };
        let expansion = expand_at(
            hero,
            kit.planes,
            path,
            ray,
            at,
            Some(!path.inside),
            prev_plane,
        );
        let (code, ext) = ((word >> 1) & 0x7F, word & 1 == 1);
        let child = expansion
            .children
            .iter()
            .find(|c| kind_code(c.kind) == code && c.state.is_ext == ext)?;
        if child.diverged {
            return None;
        }
        if path.inside && kit.subnodes.is_some() {
            segments.push((expansion.depths, path.k, path.is_ext));
        }
        path = child.state;
        prev_plane = Some(expansion.plane_normal);
        s = child.s;
        origin = hit;
        if i + 1 < steps.len() {
            dir = (*geo.hits.get(i + 1)? - hit).normalize();
        }
    }
    Some(Replay {
        stokes: path.stokes,
        k: path.k,
        s,
        origin: origin.as_vec3(),
        prev_plane,
        is_ext: path.is_ext,
        path_length: length,
        segments,
    })
}

/// [`retrace_state`] and the centre node's exact ray (for the hit check of [`project_to`]).
fn retrace(
    image_point: DVec2,
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
) -> Option<(SampleState, PrefixRay)> {
    let spectrum = beam.spectrum;
    let centre = spectrum.centre();
    let centre_ray = exact_ray(
        image_point,
        SpectralNode(centre),
        &beam.prefix,
        &kit.retrace,
    )
    .ok()?;
    let centre_group = usize::from(centre - spectrum.first) / GROUP;
    let heroes = kit.group_heroes(spectrum);
    let mut stokes = Vec::with_capacity(spectrum.groups());
    let mut shared: Option<Replay> = None;
    for g in 0..spectrum.groups() {
        let node = hero_node(spectrum, g);
        let geo = if node == centre {
            centre_ray
        } else {
            exact_ray(image_point, SpectralNode(node), &beam.prefix, &kit.retrace).ok()?
        };
        let replay = replay_group(image_point, beam, kit, heroes.get(g)?, &geo)?;
        stokes.push(replay.stokes);
        if g == centre_group {
            shared = Some(replay);
        }
    }
    let tail = shared?;
    let mut state = SampleState {
        image: image_point,
        ray: Ray {
            origin: tail.origin,
            dir: tail.s,
        },
        k: tail.k,
        segment_origin: tail.origin,
        prev_plane: tail.prev_plane,
        inside: centre_ray.inside,
        is_ext: tail.is_ext,
        stokes,
        depth: Vec::new(),
        path_length: tail.path_length,
    };
    for (depths, k, is_ext) in &tail.segments {
        if !kit.add_depth(&mut state, depths, (*k, *is_ext), spectrum) {
            return None;
        }
    }
    Some((state, centre_ray))
}

/// The exact state of a ray through `image_point` along the branch of `beam` (§6.4, §7.3).
///
/// This is the one function that builds the state of a **new** sample point (a projected vertex,
/// a snapped vertex, a TIR piece's vertex, a leaf's vertex, a re-light refinement point).
///
/// - **Geometry.** `retrace::exact_ray` follows `beam.prefix` from the camera ray through
///   `image_point`. `ray` is the last segment: origin at the last hit, direction S of the last
///   child `expand_at` produced; `segment_origin` is the last hit; `inside` follows the prefix
///   parity (the centre node's). For an empty prefix it is the camera ray. The geometry of the
///   shared fields is the hero node of the group holding the interval's centre node (the shared
///   topology): the centre node itself off the group grid, and on the grid whenever the centre
///   sits in slot 3; an on-grid interval of several groups with another centre slot (all 48 nodes:
///   centre 23, hero 19) is built only for materials whose geometry is the same in every node.
/// - **Physics.** Per 8-node group the prefix is replayed with `expand_at` at the exact hits of
///   the group's hero node (slot 3, clamped to the interval), starting from unit unpolarized
///   Stokes. At each step the child chosen is the first whose kind (reflect and total reflect
///   merged, or transmit) and mode match the step word; its state feeds the next step and the
///   expansion's `plane_normal` is the next `prev_plane`. The `prev_plane`, `k` and mode of the
///   result are those of the centre node's group.
/// - **Depth.** When the kit tracks the absorption sub-nodes (`InteractKit::subnodes`), `depth`
///   holds the optical depth of every interior segment of the centre group's replay, added with
///   `InteractKit::add_depth` in the wave each segment was traced in; otherwise it stays empty.
///   `path_length` is the path length inside the stone up to the last hit.
///
/// `None` when the retracer refuses (a missed plane, total reflection on a transmit step), when no
/// child matches a step, or when the matching child has `diverged` set (the point straddles a
/// topological boundary, §8.5).
#[must_use]
pub fn retrace_state(
    image_point: DVec2,
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
) -> Option<SampleState> {
    retrace(image_point, beam, kit).map(|(state, _)| state)
}

// ---- Projection (§7.3) ----

/// How far `q` lies outside the convex counter-clockwise polygon `ring` (negative inside).
fn outside_distance(ring: &[DVec2], q: DVec2) -> f64 {
    let n = ring.len();
    let twice_area: f64 = (0..n).map(|i| ring[i].perp_dot(ring[(i + 1) % n])).sum();
    let sign = if twice_area < 0.0 { -1.0 } else { 1.0 };
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let len = a.distance(b);
            if len > 0.0 {
                -sign * (b - a).perp_dot(q - a) / len
            } else {
                f64::NEG_INFINITY
            }
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The world length of one image pixel on the target plane at `q`: the larger column of the
/// Jacobian of `image_to_plane`, by a central difference.
fn pixel_scale(image_to_plane: DMat3, q: DVec2) -> Option<f64> {
    const H: f64 = 0.25;
    let dx = apply(image_to_plane, q + DVec2::new(H, 0.0))?
        - apply(image_to_plane, q - DVec2::new(H, 0.0))?;
    let dy = apply(image_to_plane, q + DVec2::new(0.0, H))?
        - apply(image_to_plane, q - DVec2::new(0.0, H))?;
    let scale = dx.length().max(dy.length()) / (2.0 * H);
    scale.is_finite().then_some(scale)
}

/// The image points of the sample states of `image`: vertices, centroid, edge midpoints.
fn sample_points(image: &ConvexPolygon) -> (Vec<DVec2>, usize) {
    let (v, n) = image.points_2d();
    let mut points: Vec<DVec2> = v[..n].to_vec();
    points.push(image.centroid().truncate());
    points.extend((0..n).map(|i| (v[i] + v[(i + 1) % n]) * 0.5));
    (points, n)
}

/// Moves a piece of `beam` onto the facet `piece.to` its rays reach (§7.3).
///
/// The polygon is the central projection of `piece.polygon` from the apex onto the plane of `G`,
/// with vertices within `VERTEX_SNAP_REL` of a ring vertex of `G` snapped to it.
/// `image_to_plane` becomes `central_projection · beam.image_to_plane`; carrier, apex, signature,
/// prefix, bounces and spectrum carry over (`facet` is `G`). The sample states are retraced
/// exactly with [`retrace_state`] at the piece's image polygon, in the order vertices, centroid,
/// edge midpoints.
///
/// The exact ray through the centroid must leave through `G` (a hard topology check), and the
/// exact hit of every sample point on `G`'s plane must lie on `G`'s polygon within two chord
/// tolerances of image space (a point on a snapped boundary is that close to the true boundary;
/// a boundary that exists only in the apex model is not). Otherwise, or when a state cannot be
/// retraced, the result is `None`, the `bracket_failures` counter is incremented and the caller
/// subdivides and partitions again. The distance between the exact and the model hit is folded
/// into [`max_model_distortion`].
#[must_use]
pub fn project_to(
    beam: &Beam,
    piece: &PartitionPiece,
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> Option<Beam> {
    let child = project_checked(beam, piece, kit);
    if child.is_none() {
        stats.bracket_failures += 1;
    }
    child
}

/// The body of [`project_to`] without the counter.
fn project_checked(beam: &Beam, piece: &PartitionPiece, kit: &InteractKit<'_, '_>) -> Option<Beam> {
    let stone = kit.retrace.stone;
    let radius = stone.bounding_radius();
    let (carrier, target) = (stone.plane(beam.facet), stone.plane(piece.to));
    let basis = PlaneBasis::new(target);
    let ring_points: Vec<DVec3> = stone
        .ring(piece.to)
        .iter()
        .map(|&id| stone.vertex(id))
        .collect();
    let ring_2d: Vec<DVec2> = ring_points.iter().map(|&p| basis.from_world(p)).collect();

    let image = piece.polygon.to_image(inverse(beam.image_to_plane)?)?;
    let mut projected = Vec::with_capacity(piece.polygon.len());
    for &p in piece.polygon.vertices() {
        let d = p - beam.apex;
        let t = (target.1 - target.0.dot(beam.apex)) / target.0.dot(d);
        if !(t.is_finite() && t > 0.0) {
            return None;
        }
        projected.push(d.mul_add(DVec3::splat(t), beam.apex));
    }
    let mut polygon = ConvexPolygon::with_frame(&projected, basis.frame())?;
    polygon.snap_to(&ring_points, VERTEX_SNAP_REL * radius);
    let image_to_plane = compose(
        central_projection(beam.apex, carrier, target),
        beam.image_to_plane,
    );

    let chord = kit.settings.chord_tolerance_px;
    let floor = SNAP_FACTOR * Tolerances::DEFAULT.side_eps_rel * radius;
    let (points, vertices) = sample_points(&image);
    let mut states = Vec::with_capacity(points.len());
    // The centroid is strictly interior to a snapped piece: its exact ray must leave through the
    // piece's facet, or an exit facet the apex model never found crosses the piece. It is checked
    // first, so a piece with the wrong facet records no distortion.
    let mut centre = {
        let (state, ray) = retrace(*points.get(vertices)?, beam, kit)?;
        if exit_facet(stone, ray.origin, ray.dir) != Some(piece.to) {
            return None;
        }
        Some((state, ray))
    };
    for (index, &q) in points.iter().enumerate() {
        let (state, ray) = if index == vertices {
            centre.take()?
        } else {
            retrace(q, beam, kit)?
        };
        let t = (target.1 - target.0.dot(ray.origin)) / target.0.dot(ray.dir);
        if !(t.is_finite() && t > 0.0) {
            return None;
        }
        let exact = ray.dir.mul_add(DVec3::splat(t), ray.origin);
        let model = basis.to_world(apply(image_to_plane, q)?);
        record_distortion(exact.distance(model));
        // A point on a snapped boundary is within `chord` pixels of the true boundary, which lies
        // on `G`'s ring; a model-only boundary tens of pixels off does not pass.
        let allowed = (HIT_SLACK_CHORDS * chord).mul_add(pixel_scale(image_to_plane, q)?, floor);
        if outside_distance(&ring_2d, basis.from_world(exact)) > allowed {
            return None;
        }
        states.push(state);
    }
    Some(Beam {
        facet: piece.to,
        polygon,
        apex: beam.apex,
        image_to_plane,
        inside: beam.inside,
        sig: beam.sig,
        prefix: beam.prefix,
        bounces: beam.bounces,
        spectrum: beam.spectrum,
        state: SampleStates {
            points: states,
            vertices,
        },
    })
}
