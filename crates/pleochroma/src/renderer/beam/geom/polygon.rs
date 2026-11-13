//! `ConvexPolygon`: clipping, splitting, area, containment, Wachspress weights (beam guide §13.4).
//!
//! The polygon is a fixed array of at most [`MAX_VERTICES`] 3D vertices on one plane, stored
//! counter-clockwise seen along the plane normal, with the plane's [`Frame2`]. Image space is the
//! plane `z = 0` with [`Frame2::IMAGE`], so the same type serves both spaces.
//!
//! The two-sided clip follows `stone_metrics::concave::Poly::clip` and
//! `gcs::polytope::clip_polygon`. Each vertex's signed distance is computed once, `On` vertices
//! belong to both pieces, and every cut vertex comes from a canonical endpoint order, so both
//! pieces (and a clip with the negated plane) share it bit for bit.

use std::cmp::Ordering;

use glam::{DMat3, DVec2, DVec3};

use super::{
    frame::Frame2,
    polyline::{
        RingPoint, decompose_convex, dedupe_ring, nearest_edge, orient2, ring_area, split_rings,
    },
    side::{Side, classify},
};

/// The most vertices a polygon holds; a clip that would exceed it splits along a diagonal first.
pub const MAX_VERTICES: usize = 12;

/// Scratch size for one clip piece: a piece has at most `MAX_VERTICES + 1` vertices.
const SCRATCH: usize = MAX_VERTICES + 2;

/// Both sides of a clip: the inside piece and the outside piece (either may be empty).
pub type ClipPair = (Option<ConvexPolygon>, Option<ConvexPolygon>);

/// A convex polygon on a plane, counter-clockwise seen along the plane normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConvexPolygon {
    pts: [DVec3; MAX_VERTICES],
    len: u8,
    frame: Frame2,
}

/// The canonical cut point of the edge `a -> b` with signed distances `da`, `db` of opposite sign.
///
/// The endpoints are put in lexicographic order first, so the result does not depend on the walk
/// direction of the edge or on the plane's orientation (negated distances give the same `t`).
fn cut_point(a: DVec3, da: f64, b: DVec3, db: f64) -> DVec3 {
    let b_first =
        b.x.total_cmp(&a.x)
            .then(b.y.total_cmp(&a.y))
            .then(b.z.total_cmp(&a.z))
            == Ordering::Less;
    let (lo, d_lo, hi, d_hi) = if b_first {
        (b, db, a, da)
    } else {
        (a, da, b, db)
    };
    let t = d_lo / (d_lo - d_hi);
    (hi - lo).mul_add(DVec3::splat(t), lo)
}

/// Moves `vertex` onto the nearest of `points` within `eps`; `true` when it moved.
///
/// Ties go to the earliest point.
fn snap_vertex(vertex: &mut DVec3, points: &[DVec3], eps: f64) -> bool {
    let mut best: Option<(f64, DVec3)> = None;
    for &p in points {
        let d = vertex.distance(p);
        if d <= eps && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, p));
        }
    }
    match best {
        Some((_, p)) if p != *vertex => {
            *vertex = p;
            true
        }
        _ => false,
    }
}

/// A piece under construction: vertices and their count.
struct Piece {
    pts: [DVec3; SCRATCH],
    len: usize,
}

impl Piece {
    const fn new() -> Self {
        Self {
            pts: [DVec3::ZERO; SCRATCH],
            len: 0,
        }
    }

    const fn push(&mut self, p: DVec3) {
        if self.len < SCRATCH {
            self.pts[self.len] = p;
        }
        self.len += 1;
    }
}

impl ConvexPolygon {
    /// Copies `points` into a polygon on `frame`'s plane without any geometric validation.
    fn make(points: &[DVec3], frame: Frame2) -> Option<Self> {
        let n = points.len();
        if !(3..=MAX_VERTICES).contains(&n) {
            return None;
        }
        let len = u8::try_from(n).ok()?;
        let mut pts = [frame.origin; MAX_VERTICES];
        pts[..n].copy_from_slice(points);
        Some(Self { pts, len, frame })
    }

    /// Builds a polygon from vertices on the plane of `frame`.
    ///
    /// The vertices are reversed when they run clockwise seen along the normal. Returns `None` for
    /// fewer than 3 or more than [`MAX_VERTICES`] vertices, non-finite coordinates or zero area.
    #[must_use]
    pub fn with_frame(points: &[DVec3], frame: Frame2) -> Option<Self> {
        if !points.iter().all(|p| p.is_finite()) {
            return None;
        }
        let mut poly = Self::make(points, frame)?;
        let area = poly.signed_area();
        if !area.is_finite() || area.abs() <= f64::MIN_POSITIVE {
            return None;
        }
        if area < 0.0 {
            let n = poly.count();
            poly.pts[..n].reverse();
        }
        Some(poly)
    }

    /// Builds a polygon on the plane with the given `normal` through the first vertex.
    ///
    /// See [`ConvexPolygon::with_frame`] for the validation.
    #[must_use]
    pub fn from_points(points: &[DVec3], normal: DVec3) -> Option<Self> {
        let frame = Frame2::from_normal(*points.first()?, normal)?;
        Self::with_frame(points, frame)
    }

    /// Builds a polygon from 2D coordinates in `frame`.
    ///
    /// See [`ConvexPolygon::with_frame`] for the validation.
    #[must_use]
    pub fn from_uv(points: &[DVec2], frame: Frame2) -> Option<Self> {
        let p3: Vec<DVec3> = points.iter().map(|&q| frame.to_3d(q)).collect();
        Self::with_frame(&p3, frame)
    }

    /// Builds an image-space polygon (the plane `z = 0`) from pixel coordinates.
    #[must_use]
    pub fn from_image_points(points: &[DVec2]) -> Option<Self> {
        Self::from_uv(points, Frame2::IMAGE)
    }

    fn count(&self) -> usize {
        usize::from(self.len)
    }

    /// The number of vertices (3 to [`MAX_VERTICES`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.count()
    }

    /// Always `false`: a polygon has at least three vertices. Provided for `len` symmetry.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// The vertices, counter-clockwise seen along the normal.
    #[must_use]
    pub fn vertices(&self) -> &[DVec3] {
        &self.pts[..self.count()]
    }

    /// The frame of the carrier plane.
    #[must_use]
    pub const fn frame(&self) -> &Frame2 {
        &self.frame
    }

    /// The plane as `(unit normal, offset)` with `normal . x = offset`.
    #[must_use]
    pub fn plane(&self) -> (DVec3, f64) {
        (self.frame.normal, self.frame.offset())
    }

    /// The vertices in the frame's 2D coordinates and their count; the rest of the array is zero.
    #[must_use]
    pub fn points_2d(&self) -> ([DVec2; MAX_VERTICES], usize) {
        let n = self.count();
        let mut out = [DVec2::ZERO; MAX_VERTICES];
        for (o, p) in out.iter_mut().zip(&self.pts[..n]) {
            *o = self.frame.to_2d(*p);
        }
        (out, n)
    }

    fn signed_area(&self) -> f64 {
        let (v, n) = self.points_2d();
        let mut twice = 0.0;
        for i in 1..n - 1 {
            twice += orient2(v[0], v[i], v[i + 1]);
        }
        0.5 * twice
    }

    /// The area of the polygon (in the plane's units, positive).
    #[must_use]
    pub fn area(&self) -> f64 {
        self.signed_area()
    }

    /// The area centroid, on the plane.
    #[must_use]
    pub fn centroid(&self) -> DVec3 {
        let (v, n) = self.points_2d();
        let origin = v[0];
        let mut twice_area = 0.0;
        let mut weighted = DVec2::ZERO;
        for i in 1..n - 1 {
            let a = v[i] - origin;
            let b = v[i + 1] - origin;
            let cross = a.perp_dot(b);
            twice_area += cross;
            weighted += (a + b) * cross;
        }
        let centre = if twice_area > 0.0 {
            origin + weighted / (3.0 * twice_area)
        } else {
            v[..n].iter().fold(DVec2::ZERO, |acc, q| acc + *q) / n as f64
        };
        self.frame.to_3d(centre)
    }

    /// Whether `p` is on the plane (within `eps`) and inside every edge (within `eps`).
    #[must_use]
    pub fn contains(&self, p: DVec3, eps: f64) -> bool {
        if self.frame.signed_distance(p).abs() > eps {
            return false;
        }
        self.contains_2d(self.frame.to_2d(p), eps)
    }

    /// Whether the 2D point `q` (in the frame) is inside every edge, within `eps`.
    #[must_use]
    pub fn contains_2d(&self, q: DVec2, eps: f64) -> bool {
        let (v, n) = self.points_2d();
        (0..n).all(|i| {
            let j = (i + 1) % n;
            orient2(v[i], v[j], q) >= -eps * v[i].distance(v[j])
        })
    }

    /// Clips against the plane `n . x = m`, returning both sides.
    ///
    /// The first piece is the inside (`n . x <= m`), the second the outside. Each vertex's signed
    /// distance is computed once and classified with [`classify`]. `On` vertices go to both pieces
    /// and count as inside for the whole-polygon cases. Cut vertices are shared bit for bit.
    /// A piece with fewer than three vertices is `None`. The whole call is `None` when a piece
    /// would exceed [`MAX_VERTICES`]; [`ConvexPolygon::clip_capped`] handles that case.
    #[must_use]
    pub fn clip(&self, n: DVec3, m: f64, eps: f64) -> Option<ClipPair> {
        let count = self.count();
        let mut dist = [0.0_f64; MAX_VERTICES];
        let mut class = [Side::On; MAX_VERTICES];
        let (mut any_inside, mut any_outside) = (false, false);
        for i in 0..count {
            dist[i] = n.dot(self.pts[i]) - m;
            class[i] = classify(dist[i], eps);
            any_inside |= class[i] == Side::Inside;
            any_outside |= class[i] == Side::Outside;
        }
        if !any_outside {
            return Some((Some(*self), None));
        }
        if !any_inside {
            return Some((None, Some(*self)));
        }
        let mut inside = Piece::new();
        let mut outside = Piece::new();
        for i in 0..count {
            let j = if i + 1 == count { 0 } else { i + 1 };
            if class[i] != Side::Outside {
                inside.push(self.pts[i]);
            }
            if class[i] != Side::Inside {
                outside.push(self.pts[i]);
            }
            let crossing = (class[i] == Side::Inside && class[j] == Side::Outside)
                || (class[i] == Side::Outside && class[j] == Side::Inside);
            if crossing {
                let cut = cut_point(self.pts[i], dist[i], self.pts[j], dist[j]);
                inside.push(cut);
                outside.push(cut);
            }
        }
        if inside.len > MAX_VERTICES || outside.len > MAX_VERTICES {
            return None;
        }
        Some((
            Self::make(&inside.pts[..inside.len], self.frame),
            Self::make(&outside.pts[..outside.len], self.frame),
        ))
    }

    /// Clips like [`ConvexPolygon::clip`] but honours the vertex cap by splitting first.
    ///
    /// When a piece would exceed [`MAX_VERTICES`], the polygon is cut along its longest diagonal
    /// and each half is clipped, so a side may come back as several polygons.
    #[must_use]
    pub fn clip_capped(&self, n: DVec3, m: f64, eps: f64) -> (Vec<Self>, Vec<Self>) {
        if let Some((inside, outside)) = self.clip(n, m, eps) {
            return (inside.into_iter().collect(), outside.into_iter().collect());
        }
        let Some(halves) = self.split_longest_diagonal() else {
            return (Vec::new(), Vec::new());
        };
        let mut inside = Vec::new();
        let mut outside = Vec::new();
        for half in <[Self; 2]>::from(halves) {
            let (i, o) = half.clip_capped(n, m, eps);
            inside.extend(i);
            outside.extend(o);
        }
        (inside, outside)
    }

    /// Splits along the diagonal between vertices `i < j`, which must not be adjacent.
    ///
    /// Both halves keep the original vertices bit for bit.
    #[must_use]
    pub fn split_diagonal(&self, i: usize, j: usize) -> Option<(Self, Self)> {
        let n = self.count();
        if i >= j || j >= n || j == i + 1 || (i == 0 && j == n - 1) {
            return None;
        }
        let first = Self::make(&self.pts[i..=j], self.frame)?;
        let mut rest = self.pts[j..n].to_vec();
        rest.extend_from_slice(&self.pts[..=i]);
        Some((first, Self::make(&rest, self.frame)?))
    }

    /// Splits along the longest diagonal; `None` for a triangle.
    #[must_use]
    pub fn split_longest_diagonal(&self) -> Option<(Self, Self)> {
        let n = self.count();
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..n {
            for j in (i + 2)..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                let d = self.pts[i].distance_squared(self.pts[j]);
                if best.is_none_or(|(bd, _, _)| d > bd) {
                    best = Some((d, i, j));
                }
            }
        }
        let (_, i, j) = best?;
        self.split_diagonal(i, j)
    }

    /// Bisects through the midpoint of the longest edge, perpendicular to it.
    ///
    /// The seam is a straight line; both halves are convex and share the seam's two vertices bit
    /// for bit. Returns `None` for a degenerate polygon or when the cut does not give two pieces.
    #[must_use]
    pub fn split_longest_edge(&self) -> Option<(Self, Self)> {
        let n = self.count();
        let mut best: Option<(f64, usize)> = None;
        for i in 0..n {
            let d = self.pts[i].distance_squared(self.pts[(i + 1) % n]);
            if best.is_none_or(|(bd, _)| d > bd) {
                best = Some((d, i));
            }
        }
        let (len2, edge) = best?;
        let start = self.pts[edge];
        let end = self.pts[(edge + 1) % n];
        let len = len2.sqrt();
        if len.is_nan() || len <= 0.0 {
            return None;
        }
        let dir = (end - start) / len;
        let offset = dir.dot((start + end) * 0.5);
        let (inside, outside) = self.clip_capped(dir, offset, len * 1e-9);
        match (inside.as_slice(), outside.as_slice()) {
            ([inner], [outer]) => Some((*inner, *outer)),
            _ => None,
        }
    }

    /// Splits into up to four pieces by bisecting along the longest edge, then each half again.
    #[must_use]
    pub fn quadrisect(&self) -> Vec<Self> {
        let Some(halves) = self.split_longest_edge() else {
            return vec![*self];
        };
        let mut out = Vec::with_capacity(4);
        for half in <[Self; 2]>::from(halves) {
            match half.split_longest_edge() {
                Some((a, b)) => {
                    out.push(a);
                    out.push(b);
                }
                None => out.push(half),
            }
        }
        out
    }

    /// Removes duplicate and collinear vertices; `None` when the polygon is empty or a sliver.
    ///
    /// A vertex closer than `eps` to the next one is a duplicate. A vertex is collinear when twice
    /// the area of the triangle with its neighbours is below `eps` times the neighbours' distance.
    /// A polygon with fewer than three vertices or an area below `sliver_area` is empty.
    #[must_use]
    pub fn remove_degeneracies(&self, eps: f64, sliver_area: f64) -> Option<Self> {
        let mut pts: Vec<DVec3> = self.vertices().to_vec();
        'again: loop {
            let n = pts.len();
            if n < 3 {
                return None;
            }
            for i in 0..n {
                let next = (i + 1) % n;
                if pts[i].distance(pts[next]) < eps {
                    pts.remove(i);
                    continue 'again;
                }
            }
            for i in 0..n {
                let prev = pts[(i + n - 1) % n];
                let next = pts[(i + 1) % n];
                let twice_area = (pts[i] - prev).cross(next - pts[i]).length();
                if twice_area < eps * prev.distance(next) {
                    pts.remove(i);
                    continue 'again;
                }
            }
            break;
        }
        let cleaned = Self::make(&pts, self.frame)?;
        (cleaned.area() >= sliver_area).then_some(cleaned)
    }

    /// Moves every vertex within `eps` of one of `points` onto the nearest such point.
    ///
    /// Returns the number of vertices that moved. Ties go to the earliest point.
    pub fn snap_to(&mut self, points: &[DVec3], eps: f64) -> usize {
        let n = self.count();
        let mut moved = 0;
        for vertex in &mut self.pts[..n] {
            moved += usize::from(snap_vertex(vertex, points, eps));
        }
        moved
    }

    /// [`ConvexPolygon::snap_to`] for the vertices strictly inside `parent`: a vertex within
    /// `boundary_eps` of a parent edge stays where it is.
    ///
    /// A parent-boundary crossing is the same cut point in both pieces that share it (beam guide
    /// section 13.3), so it needs no snap. Moving it onto a target off the boundary would open a
    /// strip between the piece and the parent edge that no piece covers.
    /// Returns the number of vertices that moved.
    pub fn snap_interior_to(
        &mut self,
        points: &[DVec3],
        eps: f64,
        parent: &Self,
        boundary_eps: f64,
    ) -> usize {
        let n = self.count();
        let mut moved = 0;
        for vertex in &mut self.pts[..n] {
            if parent.on_boundary(*vertex, boundary_eps) {
                continue;
            }
            moved += usize::from(snap_vertex(vertex, points, eps));
        }
        moved
    }

    /// Whether `p` lies within `eps` of one of the polygon's edges, measured in the plane.
    #[must_use]
    pub fn on_boundary(&self, p: DVec3, eps: f64) -> bool {
        let (v, n) = self.points_2d();
        nearest_edge(&v[..n], self.frame.to_2d(p)).is_some_and(|hit| hit.dist <= eps)
    }

    /// Wachspress coordinates of the 2D `point` (in the polygon's frame).
    ///
    /// The first [`ConvexPolygon::len`] entries are used. They sum to one, are positive inside and
    /// reproduce affine fields exactly. On an edge they reduce to linear interpolation between its
    /// two end vertices. Use an image-space polygon to get image-space weights (beam guide §6.4).
    #[must_use]
    pub fn wachspress(&self, point: DVec2) -> [f64; MAX_VERTICES] {
        let (verts, count) = self.points_2d();
        let mut out = [0.0; MAX_VERTICES];
        let mut lo = verts[0];
        let mut hi = verts[0];
        for corner_pt in &verts[..count] {
            lo = lo.min(*corner_pt);
            hi = hi.max(*corner_pt);
        }
        let size = (hi - lo).max_element();
        for i in 0..count {
            let succ = (i + 1) % count;
            let edge = verts[succ] - verts[i];
            let len = edge.length();
            if len <= 0.0 {
                continue;
            }
            let rel = point - verts[i];
            let height = edge.perp_dot(rel).abs() / len;
            let along = rel.dot(edge) / (len * len);
            if height <= 1e-12 * size && (-1e-12..=1.0 + 1e-12).contains(&along) {
                let along = along.clamp(0.0, 1.0);
                out[i] = 1.0 - along;
                out[succ] = along;
                return out;
            }
        }
        let mut cap = [0.0; MAX_VERTICES];
        for i in 0..count {
            cap[i] = 0.5 * (verts[i] - point).perp_dot(verts[(i + 1) % count] - point);
        }
        let mut sum = 0.0;
        for i in 0..count {
            let prev = (i + count - 1) % count;
            let next = (i + 1) % count;
            let corner = 0.5 * orient2(verts[prev], verts[i], verts[next]);
            out[i] = corner / (cap[prev] * cap[i]);
            sum += out[i];
        }
        for weight in &mut out[..count] {
            *weight /= sum;
        }
        out
    }

    /// Maps the vertices through the homography `m` acting on `(x, y, 1)` of the frame's 2D
    /// coordinates; `None` when any vertex maps to infinity or the signs of `w` differ.
    fn project(&self, m: DMat3) -> Option<Vec<DVec2>> {
        let (verts, count) = self.points_2d();
        let mut out = Vec::with_capacity(count);
        let mut positive = true;
        for (i, vertex) in verts[..count].iter().enumerate() {
            let hom = m * DVec3::new(vertex.x, vertex.y, 1.0);
            if !hom.is_finite() || hom.z.abs() <= f64::MIN_POSITIVE {
                return None;
            }
            if i == 0 {
                positive = hom.z > 0.0;
            } else if (hom.z > 0.0) != positive {
                return None;
            }
            out.push(DVec2::new(hom.x / hom.z, hom.y / hom.z));
        }
        Some(out)
    }

    /// The image-space polygon `plane_to_image(polygon)`, on the plane `z = 0`.
    ///
    /// `plane_to_image` maps the frame's homogeneous 2D coordinates to pixels (the inverse of a
    /// beam's `image_to_plane`). `None` when a vertex is at or behind the projection centre.
    #[must_use]
    pub fn to_image(&self, plane_to_image: DMat3) -> Option<Self> {
        Self::from_uv(&self.project(plane_to_image)?, Frame2::IMAGE)
    }

    /// The carrier-plane polygon of this image-space polygon through `image_to_plane`, on `frame`.
    #[must_use]
    pub fn to_plane(&self, image_to_plane: DMat3, frame: Frame2) -> Option<Self> {
        Self::from_uv(&self.project(image_to_plane)?, frame)
    }

    /// Cuts the polygon along a polyline that crosses it edge to edge (beam guide §7.4 step 5).
    ///
    /// `chords` are 2D points in the polygon's frame (pixels in image space). The first
    /// and last must lie within `eps` of the boundary. Returns the pieces on the left of the
    /// polyline's direction and the pieces on its right, all convex and counter-clockwise. Both
    /// sides use the polyline's vertices and the parent's vertices unchanged, so they tile the
    /// parent. A side that is not convex is decomposed by diagonals between existing vertices,
    /// starting at the reflex (inflection) vertices; a convex side is a single piece unless it
    /// exceeds [`MAX_VERTICES`]. `None` for fewer than two points, a negative `eps`, or end points
    /// away from the boundary.
    #[must_use]
    pub fn split_by_polyline(&self, chords: &[DVec2], eps: f64) -> Option<(Vec<Self>, Vec<Self>)> {
        if chords.len() < 2 || eps.is_nan() || eps < 0.0 {
            return None;
        }
        let (uv, n) = self.points_2d();
        let entry = nearest_edge(&uv[..n], chords[0])?;
        let exit = nearest_edge(&uv[..n], chords[chords.len() - 1])?;
        if entry.dist > eps || exit.dist > eps {
            return None;
        }
        let boundary: Vec<RingPoint> = (0..n)
            .map(|i| RingPoint {
                q: uv[i],
                p: self.pts[i],
            })
            .collect();
        let chord: Vec<RingPoint> = chords
            .iter()
            .map(|&q| RingPoint {
                q,
                p: self.frame.to_3d(q),
            })
            .collect();
        let (left, right) = split_rings(&boundary, &chord, entry, exit);
        Some((self.pieces_of(left, eps), self.pieces_of(right, eps)))
    }

    /// Convex polygons of one ring on this polygon's frame.
    fn pieces_of(&self, mut ring: Vec<RingPoint>, eps: f64) -> Vec<Self> {
        dedupe_ring(&mut ring, eps);
        if ring.len() < 3 || ring_area(&ring) <= 0.0 {
            return Vec::new();
        }
        decompose_convex(ring, MAX_VERTICES)
            .into_iter()
            .filter_map(|r| {
                let pts: Vec<DVec3> = r.iter().map(|rp| rp.p).collect();
                Self::make(&pts, self.frame)
            })
            .collect()
    }
}
