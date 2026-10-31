// Adapted from pleochroma geometry/stone_metrics; MIT, Copyright (c) 2026 Niklas Gussek.
// Full license: LICENSE-MIT in this directory.
//! Rotating-caliper extents over a 2D outline.
//!
//! Used by
//! production `measure_solid` as the cross-check against the
//! axis-aligned width/length convention the printed proportions actually use.

/// Rotating-caliper width and length of a 2D point set: the smallest directional
/// extent over all convex-outline edge directions, and the extent along the
/// perpendicular direction. Returns `None` when the outline is degenerate
/// (fewer than three distinct hull points). Widths equal within a relative
/// `1e-12` resolve to the edge with the smaller direction angle.
pub(super) fn caliper_extents(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let hull = convex_hull_2d(points);
    if hull.len() < 3 {
        return None;
    }
    best_edge(&hull).map(|m| {
        if m.width <= m.length {
            (m.width, m.length)
        } else {
            (m.length, m.width)
        }
    })
}

/// Relative width difference under which two candidate edges are a tie
/// (last-bit noise from differently rotated but geometrically equal extents).
const TIE_REL: f64 = 1e-12;

/// One hull edge's caliper measurement.
#[derive(Clone, Copy)]
struct EdgeMeasure {
    /// Extent across the edge direction.
    width: f64,
    /// Extent along the edge direction.
    length: f64,
    /// Unit edge direction `(dx, dz)`.
    dir: (f64, f64),
}

/// Measures the hull against the direction of edge `i`; `None` for a
/// zero-length edge. The edge length is `sqrt(fma(ez, ez, ex*ex))`, built only
/// from correctly rounded operations, so the result does not depend on the platform libm.
/// [`caliper_extents`] and production `caliper_frame` both go through here, which keeps
/// them bit-identical.
fn measure_edge(hull: &[(f64, f64)], i: usize) -> Option<EdgeMeasure> {
    let (px, pz) = hull[i];
    let (qx, qz) = hull[(i + 1) % hull.len()];
    let (ex, ez) = (qx - px, qz - pz);
    let ex_sq = ex * ex;
    let len = ez.mul_add(ez, ex_sq).sqrt();
    if len < 1e-12 {
        return None;
    }
    let (dx, dz) = (ex / len, ez / len);
    let mut along = (f64::INFINITY, f64::NEG_INFINITY);
    let mut across = (f64::INFINITY, f64::NEG_INFINITY);
    for &(x, z) in hull {
        let a = x.mul_add(dx, z * dz);
        let c = x.mul_add(-dz, z * dx);
        along = (along.0.min(a), along.1.max(a));
        across = (across.0.min(c), across.1.max(c));
    }
    Some(EdgeMeasure {
        width: across.1 - across.0,
        length: along.1 - along.0,
        dir: (dx, dz),
    })
}

/// True when direction `a` has a strictly smaller angle than `b` in `[0, 2*pi)`,
/// decided by half-plane and cross product (no trigonometry).
fn angle_less(a: (f64, f64), b: (f64, f64)) -> bool {
    let half = |d: (f64, f64)| u8::from(!(d.1 > 0.0 || (d.1 == 0.0 && d.0 > 0.0)));
    let (ha, hb) = (half(a), half(b));
    if ha == hb {
        a.0.mul_add(b.1, -(a.1 * b.0)) > 0.0
    } else {
        ha < hb
    }
}

/// Whether `cand` replaces `best`: a clearly smaller width wins; widths within
/// [`TIE_REL`] of each other are a tie that goes to the smaller edge angle.
fn edge_wins(cand: &EdgeMeasure, best: &EdgeMeasure) -> bool {
    let tol = TIE_REL * cand.width.max(best.width);
    if (cand.width - best.width).abs() <= tol {
        angle_less(cand.dir, best.dir)
    } else {
        cand.width < best.width
    }
}

/// The winning edge of the rotating-caliper sweep over a hull.
fn best_edge(hull: &[(f64, f64)]) -> Option<EdgeMeasure> {
    let mut best: Option<EdgeMeasure> = None;
    for i in 0..hull.len() {
        if let Some(m) = measure_edge(hull, i)
            && best.as_ref().is_none_or(|b| edge_wins(&m, b))
        {
            best = Some(m);
        }
    }
    best
}

/// Andrew's monotone-chain convex hull over 2D points, counterclockwise.
/// Deterministic: total-order lexicographic sort, no hashing.
fn convex_hull_2d(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = points.to_vec();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| -> f64 {
        (a.0 - o.0).mul_add(b.1 - o.1, -((a.1 - o.1) * (b.0 - o.0)))
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}
