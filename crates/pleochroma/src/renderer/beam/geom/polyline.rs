//! Splitting a convex polygon by a snapped boundary polyline (beam guide §7.4, §13.4).
//!
//! Ring machinery behind `ConvexPolygon::split_by_polyline`: it locates the polyline's end points
//! on the boundary, builds the rings on either side, and decomposes a (possibly non-convex) ring
//! into convex pieces by diagonals between existing vertices. Every output vertex is a vertex of
//! the parent or of the polyline, so both sides tile the parent.

use glam::{DVec2, DVec3};

/// A ring vertex: its 2D coordinates in the carrier frame and the 3D point that goes with them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RingPoint {
    /// The 2D coordinates in the polygon's frame (image space for an image-space polygon).
    pub q: DVec2,
    /// The 3D point, kept bit for bit when it comes from the parent polygon.
    pub p: DVec3,
}

/// The nearest point of a polygon boundary to a query point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeHit {
    /// Index of the edge `ring[edge] -> ring[edge + 1]`.
    pub edge: usize,
    /// Parameter along that edge, in `[0, 1]`.
    pub t: f64,
    /// Distance from the query point to the edge.
    pub dist: f64,
}

/// Twice the signed area of the triangle `a b c` (positive when counter-clockwise).
#[must_use]
pub fn orient2(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

/// The signed area of a ring (positive when counter-clockwise).
#[must_use]
pub fn ring_area(ring: &[RingPoint]) -> f64 {
    let Some(first) = ring.first() else {
        return 0.0;
    };
    let mut twice = 0.0;
    for w in ring.windows(2).skip(1) {
        twice += orient2(first.q, w[0].q, w[1].q);
    }
    0.5 * twice
}

/// Finds the boundary edge of the polygon `poly` (counter-clockwise) nearest to `point`.
///
/// Ties go to the lowest edge index. Returns `None` for fewer than three vertices.
#[must_use]
pub fn nearest_edge(poly: &[DVec2], point: DVec2) -> Option<EdgeHit> {
    let count = poly.len();
    if count < 3 {
        return None;
    }
    let mut best: Option<EdgeHit> = None;
    for i in 0..count {
        let start = poly[i];
        let end = poly[(i + 1) % count];
        let along_edge = end - start;
        let len2 = along_edge.length_squared();
        let param = if len2 > 0.0 {
            ((point - start).dot(along_edge) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = along_edge
            .mul_add(DVec2::splat(param), start)
            .distance(point);
        if best.is_none_or(|prev| dist < prev.dist) {
            best = Some(EdgeHit {
                edge: i,
                t: param,
                dist,
            });
        }
    }
    best
}

/// Removes consecutive ring vertices closer than `eps` (including across the wrap-around).
pub fn dedupe_ring(ring: &mut Vec<RingPoint>, eps: f64) {
    let mut i = 0;
    while ring.len() >= 2 && i < ring.len() {
        let next = (i + 1) % ring.len();
        if next != i && ring[i].q.distance(ring[next].q) <= eps {
            ring.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Builds the rings on the left and on the right of a chord polyline that crosses a polygon.
///
/// `boundary` is the polygon ring (counter-clockwise), `chord` the polyline from its entry point
/// (on edge `entry.edge`) to its exit point (on edge `exit.edge`). The left ring runs along the
/// chord, then back along the boundary; the right ring is the reversed chord plus the other arc.
/// Both rings contain the chord's vertices unchanged.
#[must_use]
pub fn split_rings(
    boundary: &[RingPoint],
    chord: &[RingPoint],
    entry: EdgeHit,
    exit: EdgeHit,
) -> (Vec<RingPoint>, Vec<RingPoint>) {
    let n = boundary.len();
    // Left: chord, then the boundary vertices after the exit edge up to the entry edge.
    let mut count_left = (entry.edge + n - exit.edge) % n;
    if count_left == 0 && entry.t < exit.t {
        count_left = n;
    }
    let mut left: Vec<RingPoint> = chord.to_vec();
    for k in 0..count_left {
        left.push(boundary[(exit.edge + 1 + k) % n]);
    }
    // Right: chord reversed, then the boundary vertices after the entry edge up to the exit edge.
    let mut count_right = (exit.edge + n - entry.edge) % n;
    if count_right == 0 && exit.t < entry.t {
        count_right = n;
    }
    let mut right: Vec<RingPoint> = chord.iter().rev().copied().collect();
    for k in 0..count_right {
        right.push(boundary[(entry.edge + 1 + k) % n]);
    }
    (left, right)
}

/// Whether the ring turns clockwise (reflex) at vertex `i`, beyond rounding.
fn is_reflex(ring: &[RingPoint], i: usize) -> bool {
    let n = ring.len();
    let prev = ring[(i + n - 1) % n].q;
    let b = ring[i].q;
    let next = ring[(i + 1) % n].q;
    orient2(prev, b, next) < -1e-12 * (b - prev).length() * (next - b).length()
}

/// The number of reflex vertices of a ring.
fn reflex_count(ring: &[RingPoint]) -> usize {
    (0..ring.len()).filter(|&i| is_reflex(ring, i)).count()
}

fn left(a: DVec2, b: DVec2, c: DVec2) -> bool {
    orient2(a, b, c) > 0.0
}

fn left_on(a: DVec2, b: DVec2, c: DVec2) -> bool {
    orient2(a, b, c) >= 0.0
}

fn collinear(a: DVec2, b: DVec2, c: DVec2) -> bool {
    orient2(a, b, c).abs() <= 1e-14 * (b - a).length() * (c - a).length()
}

/// Whether `c` lies on the closed segment `a b` (collinear and between).
fn between(a: DVec2, b: DVec2, c: DVec2) -> bool {
    if !collinear(a, b, c) {
        return false;
    }
    if (a.x - b.x).abs() > (a.y - b.y).abs() {
        (a.x <= c.x && c.x <= b.x) || (a.x >= c.x && c.x >= b.x)
    } else {
        (a.y <= c.y && c.y <= b.y) || (a.y >= c.y && c.y >= b.y)
    }
}

/// Whether the closed segments `a b` and `c d` share a point.
fn segments_meet(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let proper = !collinear(a, b, c)
        && !collinear(a, b, d)
        && !collinear(c, d, a)
        && !collinear(c, d, b)
        && (left(a, b, c) != left(a, b, d))
        && (left(c, d, a) != left(c, d, b));
    proper || between(a, b, c) || between(a, b, d) || between(c, d, a) || between(c, d, b)
}

/// Whether the direction from vertex `i` towards vertex `j` points into the ring's interior.
fn in_cone(ring: &[RingPoint], i: usize, j: usize) -> bool {
    let n = ring.len();
    let from = ring[i].q;
    let to = ring[j].q;
    let prev = ring[(i + n - 1) % n].q;
    let next = ring[(i + 1) % n].q;
    if left_on(from, next, prev) {
        left(from, to, prev) && left(to, from, next)
    } else {
        !(left_on(from, to, next) && left_on(to, from, prev))
    }
}

/// Whether the segment between vertices `i` and `j` crosses no ring edge that avoids both.
fn crosses_no_edge(ring: &[RingPoint], i: usize, j: usize) -> bool {
    let n = ring.len();
    let from = ring[i].q;
    let to = ring[j].q;
    (0..n).all(|k| {
        let k1 = (k + 1) % n;
        k == i || k1 == i || k == j || k1 == j || !segments_meet(from, to, ring[k].q, ring[k1].q)
    })
}

/// Whether `i j` is a valid interior diagonal of the ring.
fn is_diagonal(ring: &[RingPoint], i: usize, j: usize) -> bool {
    in_cone(ring, i, j) && in_cone(ring, j, i) && crosses_no_edge(ring, i, j)
}

/// Splits a ring along the diagonal `i j` (`i < j`) into two rings that share both end vertices.
#[must_use]
pub fn split_ring_at(ring: &[RingPoint], i: usize, j: usize) -> (Vec<RingPoint>, Vec<RingPoint>) {
    let first = ring[i..=j].to_vec();
    let mut second = ring[j..].to_vec();
    second.extend_from_slice(&ring[..=i]);
    (first, second)
}

/// Picks the diagonal from `reflex` that leaves the fewest reflex vertices, or `None`.
fn best_diagonal(ring: &[RingPoint], reflex: usize) -> Option<(usize, usize)> {
    let n = ring.len();
    let mut best: Option<(usize, f64, (usize, usize))> = None;
    for s in 0..n {
        if s == reflex || s == (reflex + 1) % n || (s + 1) % n == reflex {
            continue;
        }
        if !is_diagonal(ring, reflex, s) {
            continue;
        }
        let (i, j) = if reflex < s { (reflex, s) } else { (s, reflex) };
        let (first, second) = split_ring_at(ring, i, j);
        let score = reflex_count(&first) + reflex_count(&second);
        let length = ring[i].q.distance(ring[j].q);
        let better = best.is_none_or(|(bs, bl, _)| score < bs || (score == bs && length < bl));
        if better {
            best = Some((score, length, (i, j)));
        }
    }
    best.map(|(_, _, ij)| ij)
}

/// The longest diagonal `(i, j)`, `i < j`, of a ring with at least four vertices.
fn longest_diagonal(ring: &[RingPoint]) -> Option<(usize, usize)> {
    let n = ring.len();
    let mut best: Option<(f64, (usize, usize))> = None;
    for i in 0..n {
        for j in (i + 2)..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let d = ring[i].q.distance_squared(ring[j].q);
            if best.is_none_or(|(bd, _)| d > bd) {
                best = Some((d, (i, j)));
            }
        }
    }
    best.map(|(_, ij)| ij)
}

/// Decomposes a counter-clockwise ring into convex rings of at most `max_vertices` vertices.
///
/// Reflex vertices are removed first, by diagonals between existing vertices (the diagonal that
/// leaves the fewest reflex vertices wins). A convex ring over the cap is cut along its longest
/// diagonal. The output order is deterministic. Rings with fewer than three vertices are dropped.
#[must_use]
pub fn decompose_convex(ring: Vec<RingPoint>, max_vertices: usize) -> Vec<Vec<RingPoint>> {
    let mut out = Vec::new();
    let mut stack = vec![ring];
    while let Some(r) = stack.pop() {
        let n = r.len();
        if n < 3 {
            continue;
        }
        let diagonal = match (0..n).find(|&i| is_reflex(&r, i)) {
            // A numerically unusable ring falls back to the opposite vertex: the split still
            // terminates and keeps shared vertices.
            Some(reflex) => best_diagonal(&r, reflex).or_else(|| {
                let other = (reflex + n / 2) % n;
                (other != reflex && n >= 4).then(|| (reflex.min(other), reflex.max(other)))
            }),
            None if n > max_vertices => longest_diagonal(&r),
            None => None,
        };
        match diagonal {
            Some((i, j)) if j > i + 1 && !(i == 0 && j == n - 1) => {
                let (first, second) = split_ring_at(&r, i, j);
                stack.push(second);
                stack.push(first);
            }
            _ => out.push(r),
        }
    }
    out
}
