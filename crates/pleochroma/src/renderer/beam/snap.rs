//! Exact region boundaries: boundary snapping on exact rays (beam guide §7.4).
//!
//! The apex model decides the topology of a partition; this module moves its boundaries onto the
//! exact curves. Everything works in **image space** (the exact rays are per image point and
//! `ε_chord` is in pixels):
//!
//! 1. endpoints on a parent edge: 1-D bisection **along the edge** to `bisect_px`, keyed by the
//!    edge's canonical endpoints and shared through the [`SeamStore`] (kinds 0, 2, 3);
//! 2. junctions (the image of a stone vertex): a safeguarded 2-D Newton solve, stored once
//!    (kind 1);
//! 3. chord refinement: bisection along the chord normal, a crossing is inserted when the chord
//!    midpoint is more than `chord_px` away from it (kind 4).
//!
//! A bracket is searched outward from the model point by doubling a half-width, so a model that is
//! a few pixels off still finds the curve; when both ends of the bracket classify alike, or one of
//! them classifies as neither side, the model topology is wrong there and the caller gets a
//! [`SnapFailure`] (§7.4 step 8).

use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use glam::{DVec2, DVec3};

use super::{
    beam::{
        Beam, BoundaryKind, BranchPrefix, Classifier, Partition, PartitionPiece, SeamStore,
        SnapKey, SpectralNode,
    },
    geom::{
        Tolerances,
        homography::inverse,
        polygon::{ConvexPolygon, MAX_VERTICES},
        polyline::{RingPoint, decompose_convex, dedupe_ring, nearest_edge, orient2, ring_area},
    },
    interact::InteractKit,
    primary::TileRect,
    propagate::snapped_polygon,
    retrace::{RetraceKit, exact_ray_at},
    stats::BeamStats,
    stone::BeamStone,
};

/// Vertices closer than this (pixels) are one vertex of a partition's subdivision.
const MERGE_PX: f64 = 1e-7;
/// How far (pixels) a model polyline end may be from the parent boundary.
const ENDPOINT_RADIUS_PX: f64 = 1e-6;
/// A stored shared value farther than this (pixels) from the hint is another crossing.
const SHARE_RADIUS_PX: f64 = 4.0;
/// The most bisection steps of one root find (about 30 are needed for 1e-9 px).
const MAX_BISECT_STEPS: u32 = 80;
/// The most doublings of the bracket search half-width: from the smallest accepted chord tolerance
/// (1e-9 px) 64 doublings reach far beyond any image.
const MAX_EXPAND_STEPS: u32 = 64;
/// The deepest chord refinement.
const MAX_REFINE_DEPTH: u32 = 32;
/// The most points one refined polyline may hold; a chord that needs more is a failure.
const MAX_POLYLINE_POINTS: usize = 4096;
/// The relative tolerance of the area check after snapping.
const AREA_TOLERANCE: f64 = 1e-9;
/// A chord longer than this (pixels) is split at its exact midpoint crossing even when the model
/// midpoint is within `chord_px`.
const MAX_CHORD_PX: f64 = 32.0;
/// The farthest (pixels) a junction may move from its model position.
const JUNCTION_RADIUS_PX: f64 = 16.0;
/// The most Newton iterations of a junction solve.
const JUNCTION_MAX_ITER: u32 = 40;
/// The longest Newton step (pixels).
const JUNCTION_MAX_STEP_PX: f64 = 2.0;
/// The finite-difference step of the junction Jacobian (pixels).
const JUNCTION_FD_PX: f64 = 1e-5;

/// A bracket failure (§7.4 step 8): both ends of a bisection classify alike, or one of them
/// classifies as neither side. The model topology is wrong there; the caller subdivides and
/// partitions again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapFailure;

impl fmt::Display for SnapFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("boundary snapping found no bracket")
    }
}

impl std::error::Error for SnapFailure {}

/// One boundary to snap (§7.4).
pub struct SnapRequest<'c> {
    /// The parent polygon in image space.
    pub parent: ConvexPolygon,
    /// The model boundary polyline across the parent, in image space, from one parent edge to
    /// another. Interior vertices are snapped like chord midpoints.
    pub model: Vec<DVec2>,
    /// The exact-ray classifier.
    pub classifier: &'c dyn Classifier,
    /// The two class ids the boundary separates: `.0` left of the model's direction, `.1` right.
    pub sides: (u32, u32),
    /// What the boundary is (selects the key tag; `Seam` and `TileEdge` file the parent edge
    /// crossings under kinds 2 and 3).
    pub boundary: BoundaryKind,
    /// The tile; a parent edge lying on its rectangle is filed under kind 3.
    pub tile: TileRect,
    /// The wavelength of the exact rays, in nm.
    pub lambda_nm: f32,
    /// The branch prefix of the beam; mixed into the shared keys so two beams with different
    /// prefixes never share a crossing.
    pub prefix: BranchPrefix,
    /// The render's cancel token, polled between root finds; `None` never cancels.
    pub cancel: Option<&'c AtomicBool>,
}

/// The facet an exact ray reaches next, as a class id (§7.2, §7.4).
pub struct FacetClassifier<'k> {
    kit: RetraceKit<'k>,
    prefix: BranchPrefix,
    lambda_nm: f32,
}

impl<'k> FacetClassifier<'k> {
    /// A classifier of the branch `prefix` at `lambda_nm`.
    #[must_use]
    pub const fn new(kit: RetraceKit<'k>, prefix: BranchPrefix, lambda_nm: f32) -> Self {
        Self {
            kit,
            prefix,
            lambda_nm,
        }
    }
}

impl Classifier for FacetClassifier<'_> {
    fn classify(&self, image_point: DVec2) -> Option<u32> {
        let ray = exact_ray_at(image_point, self.lambda_nm, &self.prefix, &self.kit).ok()?;
        if !ray.inside {
            return None;
        }
        exit_facet(self.kit.stone, ray.origin, ray.dir)
    }
}

/// The facet a ray from `origin` (on or inside the stone) along `dir` leaves through: the plane
/// with the smallest positive `t` among those it moves out through.
#[must_use]
pub fn exit_facet(stone: &BeamStone, origin: DVec3, dir: DVec3) -> Option<u32> {
    let mut best: Option<(f64, u32)> = None;
    for facet in 0..u32::try_from(stone.facet_count()).ok()? {
        let (n, m) = stone.plane(facet);
        let denom = n.dot(dir);
        if denom <= 0.0 {
            continue;
        }
        let t = (m - n.dot(origin)) / denom;
        if t > 0.0 && t.is_finite() && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, facet));
        }
    }
    best.map(|(_, facet)| facet)
}

// ---- keys ----

/// Mixes the word `w` into the hash `h` (a deterministic 64-bit finaliser).
const fn mix(h: u64, w: u64) -> u64 {
    let mut z = h.rotate_left(23) ^ w.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The key word of a point (its exact bit patterns).
const fn point_word(p: DVec2) -> u64 {
    mix(p.x.to_bits(), p.y.to_bits())
}

/// The two points in lexicographic order, so both neighbours build the same key.
fn canon(a: DVec2, b: DVec2) -> (DVec2, DVec2) {
    if b.x.total_cmp(&a.x).then(b.y.total_cmp(&a.y)).is_lt() {
        (b, a)
    } else {
        (a, b)
    }
}

/// The three words that identify a boundary kind.
fn boundary_words(kind: BoundaryKind) -> (u64, u64, u64) {
    match kind {
        BoundaryKind::Facet { a, b } => (1, u64::from(a), u64::from(b)),
        BoundaryKind::Tir { facet } => (2, u64::from(facet), 0),
        BoundaryKind::Mode => (3, 0, 0),
        BoundaryKind::Seam => (4, 0, 0),
        BoundaryKind::TileEdge => (5, 0, 0),
    }
}

/// The tag shared by the two sides of one boundary: all key words start with it.
#[derive(Clone, Copy)]
struct Pair {
    sides: (u32, u32),
    tag: u64,
    /// The key kind of a crossing on a parent edge: 0, or 2 for a seam.
    edge_kind: u8,
}

fn request_pair(req: &SnapRequest<'_>) -> Pair {
    let (k, a, b) = boundary_words(req.boundary);
    let mut tag = mix(u64::from(req.lambda_nm.to_bits()), k);
    for w in [a, b, u64::from(req.sides.0), u64::from(req.sides.1)] {
        tag = mix(tag, w);
    }
    for &w in req.prefix.steps() {
        tag = mix(tag, u64::from(w));
    }
    Pair {
        sides: req.sides,
        tag,
        edge_kind: match req.boundary {
            BoundaryKind::Seam => 2,
            BoundaryKind::TileEdge => 3,
            _ => 0,
        },
    }
}

/// The doubled signed area of a ring (positive counter-clockwise).
fn twice_area(ring: &[DVec2]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i].perp_dot(ring[(i + 1) % n])).sum()
}

/// `-1` for a clockwise ring, else `1`: it makes an edge cross product positive inside the ring.
fn ring_orientation(ring: &[DVec2]) -> f64 {
    if twice_area(ring) < 0.0 { -1.0 } else { 1.0 }
}

/// `true` when `p` lies inside the convex `ring`, within `eps` of every edge.
fn inside_ring(ring: &[DVec2], p: DVec2, eps: f64) -> bool {
    let n = ring.len();
    let sign = ring_orientation(ring);
    (0..n).all(|i| {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        sign * orient2(a, b, p) >= -eps * a.distance(b)
    })
}

// ---- the snapper ----

/// The root finders and the shared store.
struct Snapper<'a> {
    classifier: &'a dyn Classifier,
    tol: &'a Tolerances,
    seams: &'a mut SeamStore,
    tile: TileRect,
    /// The parent ring in image space (empty: no containment clamp).
    parent: &'a [DVec2],
    /// The render's cancel token.
    cancel: Option<&'a AtomicBool>,
    /// Freshly computed snapped values.
    snapped: u64,
}

impl Snapper<'_> {
    /// `true` once the cancel token is set.
    fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// The part of `range` where the line `origin + s * dir` lies inside the parent ring (within
    /// `cell_eps`), by clipping against every edge; empty (`lo > hi`) when the line misses it
    /// Without a parent ring the range is unchanged.
    fn within_parent(&self, origin: DVec2, dir: DVec2, range: (f64, f64)) -> (f64, f64) {
        let ring = self.parent;
        let n = ring.len();
        if n < 3 {
            return range;
        }
        let sign = ring_orientation(ring);
        let eps = self.tol.cell_eps;
        let (mut lo, mut hi) = range;
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let edge = b - a;
            let len = edge.length();
            if !(len.is_finite() && len > 0.0) {
                continue;
            }
            let at_origin = sign * edge.perp_dot(origin - a) / len;
            let slope = sign * edge.perp_dot(dir) / len;
            if slope.abs() <= f64::MIN_POSITIVE {
                if at_origin < -eps {
                    return (1.0, 0.0);
                }
                continue;
            }
            let s = (-eps - at_origin) / slope;
            if slope > 0.0 {
                lo = lo.max(s);
            } else {
                hi = hi.min(s);
            }
        }
        (lo, hi)
    }

    /// Which side of the boundary the exact ray through `p` falls on; `None` for neither.
    fn side_at(&self, pair: Pair, p: DVec2) -> Option<bool> {
        match self.classifier.classify(p) {
            Some(c) if c == pair.sides.0 => Some(false),
            Some(c) if c == pair.sides.1 => Some(true),
            _ => None,
        }
    }

    /// The parameter `s` in `[range.0, range.1]` of the side flip on the line `origin + s * dir`,
    /// searched outward from `s0` (§7.4 steps 2 and 4).
    fn crossing_on_line(
        &self,
        pair: Pair,
        origin: DVec2,
        dir: DVec2,
        range: (f64, f64),
        s0: f64,
    ) -> Result<f64, SnapFailure> {
        let (s_min, s_max) = range;
        if s_min > s_max || self.cancelled() {
            return Err(SnapFailure);
        }
        let s0 = s0.clamp(s_min, s_max);
        let at = |s: f64| {
            self.side_at(pair, dir.mul_add(DVec2::splat(s), origin))
                .ok_or(SnapFailure)
        };
        // The expansion terminates on its own: it stops when the half-width stops growing (zero,
        // subnormal-rounded, infinite or NaN), when both probes stay where they were, and after
        // `MAX_EXPAND_STEPS` doublings. The cancel token is polled once per root find; the bounds
        // alone guarantee termination.
        let mut h = self.tol.chord_px * 0.5;
        let mut found = None;
        let mut previous = None;
        for _ in 0..MAX_EXPAND_STEPS {
            let lo = (s0 - h).max(s_min);
            let hi = (s0 + h).min(s_max);
            let a = at(lo)?;
            if a != at(hi)? {
                found = Some((lo, hi, a));
                break;
            }
            if lo <= s_min && hi >= s_max {
                return Err(SnapFailure);
            }
            if previous == Some((lo, hi)) {
                return Err(SnapFailure);
            }
            previous = Some((lo, hi));
            let next = h * 2.0;
            if !next.is_finite() || next <= h {
                return Err(SnapFailure);
            }
            h = next;
        }
        let Some((mut lo, mut hi, side_lo)) = found else {
            return Err(SnapFailure);
        };
        let mut steps = 0;
        while hi - lo > self.tol.bisect_px && steps < MAX_BISECT_STEPS {
            let mid = lo.midpoint(hi);
            if at(mid)? == side_lo {
                lo = mid;
            } else {
                hi = mid;
            }
            steps += 1;
        }
        Ok(lo.midpoint(hi))
    }

    /// `true` when the segment `a b` lies on the tile rectangle's boundary.
    fn on_tile_boundary(&self, a: DVec2, b: DVec2) -> bool {
        let (lo, hi) = (self.tile.image_min(), self.tile.image_max());
        let eps = self.tol.cell_eps;
        [lo.x, hi.x]
            .iter()
            .any(|&x| (a.x - x).abs() <= eps && (b.x - x).abs() <= eps)
            || [lo.y, hi.y]
                .iter()
                .any(|&y| (a.y - y).abs() <= eps && (b.y - y).abs() <= eps)
    }

    /// The exact crossing on the parent edge `a b` nearest to `hint` (§7.4 step 2), shared with
    /// the neighbour across the edge through the store.
    fn snap_on_edge(
        &mut self,
        pair: Pair,
        start: DVec2,
        end: DVec2,
        hint: DVec2,
    ) -> Result<DVec2, SnapFailure> {
        let (ca, cb) = canon(start, end);
        let kind = if self.on_tile_boundary(ca, cb) {
            3
        } else {
            pair.edge_kind
        };
        let key = SnapKey {
            kind,
            words: [pair.tag, point_word(ca), point_word(cb), 0],
        };
        let stored = self.seams.get(&key);
        if let Some(shared) = stored
            && shared.distance(hint) <= SHARE_RADIUS_PX
        {
            return Ok(shared);
        }
        let delta = cb - ca;
        let len = delta.length();
        if !len.is_finite() || len <= 0.0 {
            return Err(SnapFailure);
        }
        let dir = delta / len;
        let along = self.crossing_on_line(pair, ca, dir, (0.0, len), (hint - ca).dot(dir))?;
        let point = dir.mul_add(DVec2::splat(along), ca);
        self.snapped += 1;
        if stored.is_some() {
            return Ok(point);
        }
        Ok(self.seams.get_or_insert(key, point))
    }

    /// The exact crossing on the normal `n` through `m`, filed as a kind-4 value keyed by
    /// `key_pts` (the chord's end points, or the neighbours of a polyline vertex) and `extra`.
    fn crossing_at(
        &mut self,
        pair: Pair,
        key_pts: (DVec2, DVec2),
        extra: u64,
        m: DVec2,
        n: DVec2,
        limit: f64,
    ) -> Result<DVec2, SnapFailure> {
        let (ca, cb) = canon(key_pts.0, key_pts.1);
        let key = SnapKey {
            kind: 4,
            words: [pair.tag, point_word(ca), point_word(cb), extra],
        };
        if let Some(v) = self.seams.get(&key) {
            return Ok(v);
        }
        // The search stays on the part of the normal line inside the parent.
        let range = self.within_parent(m, n, (-limit, limit));
        let s = self.crossing_on_line(pair, m, n, range, 0.0)?;
        let p = n.mul_add(DVec2::splat(s), m);
        self.snapped += 1;
        Ok(self.seams.get_or_insert(key, p))
    }

    /// Refines the chord `from to`, appending the inserted crossings to `out` (§7.4 step 4).
    fn refine_into(
        &mut self,
        pair: Pair,
        from: DVec2,
        to: DVec2,
        depth: u32,
        out: &mut Vec<DVec2>,
    ) -> Result<(), SnapFailure> {
        let delta = to - from;
        let len = delta.length();
        if depth >= MAX_REFINE_DEPTH || !len.is_finite() || len <= self.tol.chord_px {
            return Ok(());
        }
        // A chord that needs more inserted points than the cap is a failure, never an endless
        // refinement.
        if out.len() >= MAX_POLYLINE_POINTS {
            return Err(SnapFailure);
        }
        let mid = (from + to) * 0.5;
        let normal = DVec2::new(-delta.y, delta.x) / len;
        let limit = len.mul_add(2.0, 16.0);
        let ms = self.crossing_at(pair, (from, to), 0, mid, normal, limit)?;
        if ms.distance(mid) <= self.tol.chord_px && len <= MAX_CHORD_PX {
            return Ok(());
        }
        self.refine_into(pair, from, ms, depth + 1, out)?;
        out.push(ms);
        self.refine_into(pair, ms, to, depth + 1, out)
    }

    /// The exact end of a model polyline on the parent ring.
    fn end_on_ring(&mut self, pair: Pair, ring: &[DVec2], p: DVec2) -> Result<DVec2, SnapFailure> {
        let hit = nearest_edge(ring, p).ok_or(SnapFailure)?;
        if hit.dist > ENDPOINT_RADIUS_PX {
            return Err(SnapFailure);
        }
        let n = ring.len();
        self.snap_on_edge(pair, ring[hit.edge], ring[(hit.edge + 1) % n], p)
    }

    /// The exact polyline for a model polyline across `ring`.
    fn polyline(
        &mut self,
        pair: Pair,
        ring: &[DVec2],
        model: &[DVec2],
    ) -> Result<Vec<DVec2>, SnapFailure> {
        let (Some(&first_m), Some(&last_m)) = (model.first(), model.last()) else {
            return Err(SnapFailure);
        };
        if model.len() < 2 || ring.len() < 3 {
            return Err(SnapFailure);
        }
        let first = self.end_on_ring(pair, ring, first_m)?;
        let last = self.end_on_ring(pair, ring, last_m)?;
        let mut pts = vec![first];
        for w in model.windows(3) {
            let d = w[2] - w[0];
            let len = d.length();
            if !len.is_finite() || len <= 0.0 {
                return Err(SnapFailure);
            }
            let n = DVec2::new(-d.y, d.x) / len;
            pts.push(self.crossing_at(pair, (w[0], w[2]), 1, w[1], n, len.mul_add(2.0, 16.0))?);
        }
        pts.push(last);
        let mut out = vec![first];
        for w in pts.windows(2) {
            self.refine_into(pair, w[0], w[1], 0, &mut out)?;
            out.push(w[1]);
        }
        Ok(out)
    }

    /// The junction where the exact ray hits the stone vertex `vertex_id`, solved once and shared.
    fn junction(
        &mut self,
        problem: &JunctionProblem<'_>,
        branch_tag: u64,
        vertex_id: u32,
    ) -> Result<DVec2, SnapFailure> {
        let key = SnapKey {
            kind: 1,
            words: [branch_tag, u64::from(vertex_id), 0, 0],
        };
        if let Some(v) = self.seams.get(&key) {
            return Ok(v);
        }
        let p = problem.solve(self.tol).ok_or(SnapFailure)?;
        self.snapped += 1;
        Ok(self.seams.get_or_insert(key, p))
    }
}

/// The exact-ray hit condition on a stone vertex: two equations in the image point (§7.4 step 3).
pub struct JunctionProblem<'a> {
    kit: &'a RetraceKit<'a>,
    prefix: &'a BranchPrefix,
    lambda_nm: f32,
    target: DVec3,
    start: DVec2,
    e1: DVec3,
    e2: DVec3,
}

impl<'a> JunctionProblem<'a> {
    /// The problem "the exact ray of `prefix` at `lambda_nm` hits `target`", started at `start`.
    /// `None` when the exact ray through `start` does not exist.
    #[must_use]
    pub fn new(
        kit: &'a RetraceKit<'a>,
        prefix: &'a BranchPrefix,
        lambda_nm: f32,
        target: DVec3,
        start: DVec2,
    ) -> Option<Self> {
        let dir = exact_ray_at(start, lambda_nm, prefix, kit).ok()?.dir;
        let helper = if dir.x.abs() < 0.9 {
            DVec3::X
        } else {
            DVec3::Y
        };
        let e1 = dir.cross(helper).normalize();
        let e2 = dir.cross(e1);
        Some(Self {
            kit,
            prefix,
            lambda_nm,
            target,
            start,
            e1,
            e2,
        })
    }

    /// The perpendicular offset of the target from the exact ray through `x`, in the basis
    /// `(e1, e2)`; `None` when the ray does not exist or points away from the target.
    #[must_use]
    pub fn residual(&self, x: DVec2) -> Option<DVec2> {
        let ray = exact_ray_at(x, self.lambda_nm, self.prefix, self.kit).ok()?;
        let w = self.target - ray.origin;
        let along = w.dot(ray.dir);
        if along <= 0.0 {
            return None;
        }
        let r = w - ray.dir * along;
        Some(DVec2::new(r.dot(self.e1), r.dot(self.e2)))
    }

    /// The Jacobian columns by central differences.
    fn jacobian(&self, x: DVec2) -> Option<[DVec2; 2]> {
        let h = JUNCTION_FD_PX;
        let col = |step: DVec2| -> Option<DVec2> {
            Some((self.residual(x + step)? - self.residual(x - step)?) / (2.0 * h))
        };
        Some([col(DVec2::new(h, 0.0))?, col(DVec2::new(0.0, h))?])
    }

    /// The safeguarded Newton solve: the step is capped, a step that does not reduce the residual
    /// is halved, and the iterate may not leave the radius around the start. Converged when a
    /// Newton step is below a tenth of `bisect_px`.
    #[must_use]
    pub fn solve(&self, tol: &Tolerances) -> Option<DVec2> {
        let mut x = self.start;
        let mut r = self.residual(x)?;
        for _ in 0..JUNCTION_MAX_ITER {
            let [jx, jy] = self.jacobian(x)?;
            let det = jx.x.mul_add(jy.y, -(jy.x * jx.y));
            if !det.is_finite() || det.abs() < 1e-30 {
                return None;
            }
            let mut d = DVec2::new(
                -jy.y.mul_add(r.x, -(jy.x * r.y)) / det,
                -jx.x.mul_add(r.y, -(jx.y * r.x)) / det,
            );
            let len = d.length();
            if !len.is_finite() {
                return None;
            }
            if len > JUNCTION_MAX_STEP_PX {
                d *= JUNCTION_MAX_STEP_PX / len;
            }
            if d.length() <= 0.1 * tol.bisect_px {
                return ((x + d - self.start).length() <= JUNCTION_RADIUS_PX).then_some(x + d);
            }
            let mut scale = 1.0;
            let mut accepted = None;
            for _ in 0..12 {
                let cand = d.mul_add(DVec2::splat(scale), x);
                if let Some(rc) = self.residual(cand)
                    && rc.length() < r.length()
                {
                    accepted = Some((cand, rc));
                    break;
                }
                scale *= 0.5;
            }
            let (cx, cr) = accepted?;
            if (cx - self.start).length() > JUNCTION_RADIUS_PX {
                return None;
            }
            x = cx;
            r = cr;
        }
        None
    }
}

// ---- public functions ----

/// The exact polyline of one boundary (§7.4): ends on the parent's edges by bisection along the
/// edge, interior model vertices and chord midpoints on the exact curve to `chord_px`.
///
/// The result runs from the entry edge to the exit edge in the model's direction. Counts
/// `snapped_vertices` for each freshly computed value and `bracket_failures` on failure.
///
/// # Errors
///
/// [`SnapFailure`] when a bracket is not found or the model ends are not on the parent boundary.
pub fn snap_boundary(
    req: &SnapRequest<'_>,
    tol: &Tolerances,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> Result<Vec<DVec2>, SnapFailure> {
    let (pts, n) = req.parent.points_2d();
    let mut snapper = Snapper {
        classifier: req.classifier,
        tol,
        seams,
        tile: req.tile,
        parent: &pts[..n],
        cancel: req.cancel,
        snapped: 0,
    };
    let result = snapper.polyline(request_pair(req), &pts[..n], &req.model);
    stats.snapped_vertices += snapper.snapped;
    if result.is_err() {
        stats.bracket_failures += 1;
    }
    result
}

/// A classifier that never classifies (the junction solve does not use one).
struct NoClassifier;

impl Classifier for NoClassifier {
    fn classify(&self, _image_point: DVec2) -> Option<u32> {
        None
    }
}

/// The junction of `problem` (the image point whose exact ray hits stone vertex `vertex_id`),
/// solved once per `(branch_tag, vertex_id)` and shared through `seams` (kind 1, §7.4 step 3).
///
/// # Errors
///
/// [`SnapFailure`] when the Newton solve does not converge inside its radius.
pub fn snap_junction(
    problem: &JunctionProblem<'_>,
    branch_tag: u64,
    vertex_id: u32,
    tol: &Tolerances,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> Result<DVec2, SnapFailure> {
    let mut snapper = Snapper {
        classifier: &NoClassifier,
        tol,
        seams,
        tile: TileRect {
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
        },
        parent: &[],
        cancel: None,
        snapped: 0,
    };
    let result = snapper.junction(problem, branch_tag, vertex_id);
    stats.snapped_vertices += snapper.snapped;
    if result.is_err() {
        stats.bracket_failures += 1;
    }
    result
}

/// Applies a snapped polyline to its parent with `ConvexPolygon::split_by_polyline`.
///
/// Both sides use the same vertex values, so the children tile the parent by construction. Returns
/// the pieces on the left and on the right of the polyline's direction; `None` when the ends are
/// off the boundary.
#[must_use]
pub fn split_snapped(
    parent: &ConvexPolygon,
    polyline: &[DVec2],
    tol: &Tolerances,
) -> Option<(Vec<ConvexPolygon>, Vec<ConvexPolygon>)> {
    parent.split_by_polyline(polyline, tol.cell_eps)
}

/// `true` when the prefix has a Transmit step: only then the apex-model boundaries are inexact.
fn has_transmit(prefix: &BranchPrefix) -> bool {
    prefix.steps().iter().any(|&w| (w >> 1) & 0x7F == 3)
}

/// The partition with every facet-to-facet boundary snapped (§7.4), on the carrier plane.
///
/// `None` on a bracket failure or a geometry failure: the caller re-partitions and then sends the
/// region to the residual.
///
/// A prefix without a Transmit step has exact model boundaries: the partition is returned
/// unchanged. The exact rays are taken at the centre node of the beam's spectrum.
#[must_use]
pub fn snap_partition(
    beam: &Beam,
    partition: &Partition,
    kit: &InteractKit<'_, '_>,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> Option<Vec<PartitionPiece>> {
    snap_partition_cancellable(
        beam,
        partition,
        (&kit.retrace, kit.tile, kit.cancel),
        seams,
        stats,
    )
}

/// [`snap_partition`] on the retracer and tile alone.
#[must_use]
pub fn snap_partition_with(
    beam: &Beam,
    partition: &Partition,
    retrace: &RetraceKit<'_>,
    tile: TileRect,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> Option<Vec<PartitionPiece>> {
    snap_partition_cancellable(beam, partition, (retrace, tile, None), seams, stats)
}

/// [`snap_partition_with`] that also polls a cancel token between root finds.
fn snap_partition_cancellable(
    beam: &Beam,
    partition: &Partition,
    (retrace, tile, cancel): (&RetraceKit<'_>, TileRect, Option<&AtomicBool>),
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> Option<Vec<PartitionPiece>> {
    if partition.pieces.len() < 2 || !has_transmit(&beam.prefix) {
        return Some(partition.pieces.clone());
    }
    let to_image = inverse(beam.image_to_plane)?;
    // The pieces tile the snapped polygon of the partition, not the stored one.
    let parent = snapped_polygon(beam, retrace.stone).to_image(to_image)?;
    let rings: Vec<(u32, Vec<DVec2>)> = partition
        .pieces
        .iter()
        .map(|piece| {
            piece
                .polygon
                .to_image(to_image)
                .map(|poly| (piece.to, ring_of(&poly)))
        })
        .collect::<Option<_>>()?;
    let mesh = build_mesh(&rings);
    let chords = find_chords(&mesh);
    if chords.is_empty() {
        return Some(partition.pieces.clone());
    }
    let lambda_nm = SpectralNode(beam.spectrum.centre()).centre_nm(retrace.n_lambda);
    let mut branch_tag = mix(u64::from(lambda_nm.to_bits()), u64::from(beam.facet));
    for &w in beam.prefix.steps() {
        branch_tag = mix(branch_tag, u64::from(w));
    }
    let tol = Tolerances::DEFAULT;
    let classifier = FacetClassifier::new(*retrace, beam.prefix, lambda_nm);
    let parent_ring = ring_of(&parent);
    let mut snapper = Snapper {
        classifier: &classifier,
        tol: &tol,
        seams,
        tile,
        parent: &parent_ring,
        cancel,
        snapped: 0,
    };
    let run = PartitionRun {
        beam,
        retrace,
        lambda_nm,
        parent: parent_ring.clone(),
        mesh,
        chords,
        branch_tag,
    };
    let result = run.snap(&mut snapper);
    stats.snapped_vertices += snapper.snapped;
    let Ok(snapped_rings) = result else {
        stats.bracket_failures += 1;
        return None;
    };
    to_pieces(beam, &snapped_rings, &tol)
}

// ---- the partition's subdivision ----

/// The vertices of an image polygon.
fn ring_of(poly: &ConvexPolygon) -> Vec<DVec2> {
    let (pts, n) = poly.points_2d();
    pts[..n].to_vec()
}

/// The partition's pieces as rings of shared vertex indices.
struct Mesh {
    verts: Vec<DVec2>,
    /// `(facet, ring of vertex indices)` per piece.
    pieces: Vec<(u32, Vec<usize>)>,
}

/// An interior edge between two pieces of different facets, `u < v`.
struct Chord {
    u: usize,
    v: usize,
    /// The two facets, ascending.
    sides: (u32, u32),
}

/// The index of the vertex within `MERGE_PX` of `p`, adding it when there is none.
fn vertex_index(verts: &mut Vec<DVec2>, p: DVec2) -> usize {
    verts
        .iter()
        .position(|q| q.distance(p) <= MERGE_PX)
        .unwrap_or_else(|| {
            verts.push(p);
            verts.len() - 1
        })
}

/// Clusters the vertices of all pieces and inserts into every edge the vertices lying on it, so
/// neighbouring pieces list the same edges.
fn build_mesh(rings: &[(u32, Vec<DVec2>)]) -> Mesh {
    let mut verts = Vec::new();
    let mut pieces: Vec<(u32, Vec<usize>)> = Vec::new();
    for (to, ring) in rings {
        let mut idx: Vec<usize> = Vec::new();
        for &p in ring {
            let i = vertex_index(&mut verts, p);
            if idx.last() != Some(&i) {
                idx.push(i);
            }
        }
        while idx.len() > 1 && idx.first() == idx.last() {
            idx.pop();
        }
        pieces.push((*to, idx));
    }
    for (_, ring) in &mut pieces {
        let count = ring.len();
        let mut out = Vec::with_capacity(count);
        for k in 0..count {
            let (start, end) = (ring[k], ring[(k + 1) % count]);
            out.push(start);
            let (start_pt, delta) = (verts[start], verts[end] - verts[start]);
            let len2 = delta.length_squared();
            if len2 <= 0.0 {
                continue;
            }
            let mut hits: Vec<(f64, usize)> = verts
                .iter()
                .enumerate()
                .filter_map(|(index, &pw)| {
                    if index == start || index == end {
                        return None;
                    }
                    let param = (pw - start_pt).dot(delta) / len2;
                    if param <= 0.0 || param >= 1.0 {
                        return None;
                    }
                    let foot = delta.mul_add(DVec2::splat(param), start_pt);
                    (foot.distance(pw) <= MERGE_PX).then_some((param, index))
                })
                .collect();
            hits.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            out.extend(hits.into_iter().map(|h| h.1));
        }
        *ring = out;
    }
    Mesh { verts, pieces }
}

/// The interior edges between pieces of different facets, sorted by `(u, v)`.
fn find_chords(mesh: &Mesh) -> Vec<Chord> {
    let mut directed: Vec<(usize, usize, usize)> = Vec::new();
    for (p, (_, ring)) in mesh.pieces.iter().enumerate() {
        let n = ring.len();
        for k in 0..n {
            directed.push((ring[k], ring[(k + 1) % n], p));
        }
    }
    directed.sort_unstable();
    let mut chords = Vec::new();
    for &(a, b, p) in &directed {
        if a >= b {
            continue;
        }
        let Ok(i) = directed.binary_search_by(|e| (e.0, e.1).cmp(&(b, a))) else {
            continue;
        };
        let (tp, tq) = (mesh.pieces[p].0, mesh.pieces[directed[i].2].0);
        if tp != tq {
            chords.push(Chord {
                u: a,
                v: b,
                sides: (tp.min(tq), tp.max(tq)),
            });
        }
    }
    chords.sort_by_key(|c| (c.u, c.v));
    chords.dedup_by_key(|c| (c.u, c.v));
    chords
}

/// Where a vertex lies relative to the parent polygon.
enum Place {
    Corner,
    Edge(usize),
    Interior,
}

fn locate(parent: &[DVec2], p: DVec2) -> Place {
    if parent.iter().any(|q| q.distance(p) <= MERGE_PX) {
        return Place::Corner;
    }
    match nearest_edge(parent, p) {
        Some(hit) if hit.dist <= MERGE_PX => Place::Edge(hit.edge),
        _ => Place::Interior,
    }
}

/// How a vertex of the subdivision is snapped.
enum Role {
    /// Stays where the model put it.
    Fixed,
    /// Slides along parent edge `.0`, between the facets `.1`.
    OnEdge(usize, (u32, u32)),
    /// A bend of one boundary: bisected along the normal `.1`.
    Bend((u32, u32), DVec2),
    /// The image of the stone vertex (`None` when the facets share no single vertex).
    Junction(Option<u32>),
}

/// Everything one partition snap reads.
struct PartitionRun<'a> {
    beam: &'a Beam,
    retrace: &'a RetraceKit<'a>,
    lambda_nm: f32,
    parent: Vec<DVec2>,
    mesh: Mesh,
    chords: Vec<Chord>,
    branch_tag: u64,
}

/// The only stone vertex on all of `facets`.
fn common_vertex(stone: &BeamStone, facets: &[u32]) -> Option<u32> {
    let count = u32::try_from(stone.facet_count()).ok()?;
    if facets.iter().any(|&f| f >= count) {
        return None;
    }
    let (first, rest) = facets.split_first()?;
    let mut found = None;
    for &v in stone.ring(*first) {
        if rest.iter().all(|&f| stone.ring(f).contains(&v)) {
            if found.is_some() {
                return None;
            }
            found = Some(v);
        }
    }
    found
}

impl PartitionRun<'_> {
    fn classes_at(&self, v: usize) -> Vec<u32> {
        let mut classes: Vec<u32> = self
            .mesh
            .pieces
            .iter()
            .filter(|(_, ring)| ring.contains(&v))
            .map(|(to, _)| *to)
            .collect();
        classes.sort_unstable();
        classes.dedup();
        classes
    }

    fn role(&self, v: usize) -> Role {
        let incident: Vec<&Chord> = self
            .chords
            .iter()
            .filter(|c| c.u == v || c.v == v)
            .collect();
        if incident.is_empty() {
            return Role::Fixed;
        }
        let classes = self.classes_at(v);
        match locate(&self.parent, self.mesh.verts[v]) {
            Place::Edge(e) if incident.len() == 1 && classes.len() == 2 => {
                Role::OnEdge(e, incident[0].sides)
            }
            Place::Interior if classes.len() >= 3 => {
                Role::Junction(common_vertex(self.retrace.stone, &classes))
            }
            Place::Interior if incident.len() == 2 => {
                let other = |c: &Chord| if c.u == v { c.v } else { c.u };
                let d = self.mesh.verts[other(incident[1])] - self.mesh.verts[other(incident[0])];
                let len = d.length();
                if len > 0.0 && len.is_finite() {
                    Role::Bend(incident[0].sides, DVec2::new(-d.y, d.x) / len)
                } else {
                    Role::Fixed
                }
            }
            Place::Corner | Place::Edge(_) | Place::Interior => Role::Fixed,
        }
    }

    fn pair(&self, sides: (u32, u32)) -> Pair {
        let (k, a, b) = boundary_words(BoundaryKind::Facet {
            a: sides.0,
            b: sides.1,
        });
        Pair {
            sides,
            tag: mix(mix(mix(self.branch_tag, k), a), b),
            edge_kind: 0,
        }
    }

    /// Snaps every vertex, refines every chord and returns the rebuilt rings per piece.
    fn snap(&self, sn: &mut Snapper<'_>) -> Result<Vec<(u32, Vec<DVec2>)>, SnapFailure> {
        let mut snapped = Vec::with_capacity(self.mesh.verts.len());
        for (v, &pos) in self.mesh.verts.iter().enumerate() {
            if sn.cancelled() {
                return Err(SnapFailure);
            }
            let p = match self.role(v) {
                Role::Fixed => pos,
                Role::OnEdge(e, sides) => {
                    let n = self.parent.len();
                    sn.snap_on_edge(
                        self.pair(sides),
                        self.parent[e],
                        self.parent[(e + 1) % n],
                        pos,
                    )?
                }
                Role::Bend(sides, normal) => {
                    sn.crossing_at(self.pair(sides), (pos, pos), 2, pos, normal, 16.0)?
                }
                Role::Junction(vertex) => {
                    let id = vertex.ok_or(SnapFailure)?;
                    let target = self.retrace.stone.vertex(id);
                    let problem = JunctionProblem::new(
                        self.retrace,
                        &self.beam.prefix,
                        self.lambda_nm,
                        target,
                        pos,
                    )
                    .ok_or(SnapFailure)?;
                    sn.junction(&problem, self.branch_tag, id)?
                }
            };
            snapped.push(p);
        }
        let mut polylines = Vec::with_capacity(self.chords.len());
        for c in &self.chords {
            let mut inner = Vec::new();
            sn.refine_into(
                self.pair(c.sides),
                snapped[c.u],
                snapped[c.v],
                0,
                &mut inner,
            )?;
            polylines.push(inner);
        }
        let rings = self.rebuild(&snapped, &polylines);
        self.check_rings(&rings, sn.tol)?;
        Ok(rings)
    }

    /// Rejects rings that left the parent, lost area or turned inside out: every point lies inside
    /// the parent within `cell_eps`, every ring keeps its orientation, and the areas add up to the
    /// model's.
    fn check_rings(
        &self,
        rings: &[(u32, Vec<DVec2>)],
        tol: &Tolerances,
    ) -> Result<(), SnapFailure> {
        let sign = ring_orientation(&self.parent);
        let mut snapped_total = 0.0;
        for (_, pts) in rings {
            if pts
                .iter()
                .any(|&q| !inside_ring(&self.parent, q, tol.cell_eps))
            {
                return Err(SnapFailure);
            }
            let area = sign * twice_area(pts);
            if !(area.is_finite() && area > 0.0) {
                return Err(SnapFailure);
            }
            snapped_total += area;
        }
        let model_total: f64 = self
            .mesh
            .pieces
            .iter()
            .map(|(_, ring)| {
                let pts: Vec<DVec2> = ring.iter().map(|&i| self.mesh.verts[i]).collect();
                sign * twice_area(&pts)
            })
            .sum();
        if (snapped_total - model_total).abs() <= AREA_TOLERANCE * model_total.abs() {
            Ok(())
        } else {
            Err(SnapFailure)
        }
    }

    /// Each piece's ring with the snapped vertices and the chord polylines in place of its edges.
    fn rebuild(&self, snapped: &[DVec2], polylines: &[Vec<DVec2>]) -> Vec<(u32, Vec<DVec2>)> {
        self.mesh
            .pieces
            .iter()
            .map(|(to, ring)| {
                let n = ring.len();
                let mut pts = Vec::with_capacity(n);
                for k in 0..n {
                    let (a, b) = (ring[k], ring[(k + 1) % n]);
                    pts.push(snapped[a]);
                    let key = (a.min(b), a.max(b));
                    if let Ok(ci) = self.chords.binary_search_by_key(&key, |c| (c.u, c.v)) {
                        if a < b {
                            pts.extend(polylines[ci].iter().copied());
                        } else {
                            pts.extend(polylines[ci].iter().rev().copied());
                        }
                    }
                }
                (*to, pts)
            })
            .collect()
    }
}

/// The snapped rings as convex carrier-plane pieces (non-convex rings are decomposed by diagonals
/// between existing vertices, so the pieces still tile).
fn to_pieces(
    beam: &Beam,
    rings: &[(u32, Vec<DVec2>)],
    tol: &Tolerances,
) -> Option<Vec<PartitionPiece>> {
    let mut out = Vec::new();
    for (to, pts) in rings {
        let mut ring: Vec<RingPoint> = pts
            .iter()
            .map(|&q| RingPoint {
                q,
                p: q.extend(0.0),
            })
            .collect();
        dedupe_ring(&mut ring, tol.bisect_px * 0.01);
        if ring.len() < 3 || ring_area(&ring) <= 0.0 {
            continue;
        }
        for convex in decompose_convex(ring, MAX_VERTICES) {
            let q: Vec<DVec2> = convex.iter().map(|rp| rp.q).collect();
            let image = ConvexPolygon::from_image_points(&q)?;
            let polygon = image.to_plane(beam.image_to_plane, *beam.polygon.frame())?;
            out.push(PartitionPiece { to: *to, polygon });
        }
    }
    Some(out)
}
