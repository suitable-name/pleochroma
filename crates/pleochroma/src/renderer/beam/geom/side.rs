//! The two side tests of the beam geometry (beam guide §13.2).
//!
//! Geometry uses [`side`]: points `On` a plane count as inside, so both pieces of a split share
//! the cut's vertices bit for bit. Pixel cells use [`cell_side`] and [`cell_contains`], which
//! follow the half-open rule. The two conventions stay in separate functions so that neither is
//! used for the other.

use glam::DVec3;

/// Where a point lies against a plane or a cell edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// At least `eps` on the inner side (signed distance `<= -eps`).
    Inside,
    /// Within `eps` of the plane or edge.
    On,
    /// At least `eps` on the outer side (signed distance `>= eps`).
    Outside,
}

/// Classifies a signed distance `d` against the tolerance `eps`.
///
/// `d <= -eps` is `Inside`, `d >= eps` is `Outside`, anything between is `On`. With `eps == 0.0` a
/// distance of exactly zero is `Inside`.
#[must_use]
pub fn classify(d: f64, eps: f64) -> Side {
    if d <= -eps {
        Side::Inside
    } else if d >= eps {
        Side::Outside
    } else {
        Side::On
    }
}

/// Classifies `p` against the plane `n . x = m` with the absolute tolerance `eps`.
///
/// This is the only geometric side test. `Inside` is the half space `n . x < m`. The caller scales
/// `eps` by the plane's magnitude (see `Tolerances::side_eps_rel`) and passes a plane whose normal
/// has the magnitude that `eps` was scaled for. `On` points count as inside when clipping.
#[must_use]
pub fn side(n: DVec3, m: f64, p: DVec3, eps: f64) -> Side {
    classify(n.dot(p) - m, eps)
}

/// Classifies a pixel-space `coord` against one cell edge `edge` with the tolerance `eps`.
///
/// `Inside` is below the edge by at least `eps`, `On` is within `eps`, `Outside` is above.
/// For the half-open cell `[lo, hi)` the right or bottom edge `hi` is an upper edge: `On` there
/// counts as outside the cell (matching the jitter range `[-0.5, 0.5)`). Use [`cell_contains`] for
/// the whole cell test.
#[must_use]
pub fn cell_side(coord: f64, edge: f64, eps: f64) -> Side {
    classify(coord - edge, eps)
}

/// Whether `coord` lies in the half-open cell interval `[lo, hi)` with the tolerance `eps`.
///
/// A coordinate `On` the lower edge `lo` is in the cell, a coordinate `On` the upper edge `hi` is
/// not, so a point on a boundary shared by two cells belongs to exactly one of them.
#[must_use]
pub fn cell_contains(coord: f64, lo: f64, hi: f64, eps: f64) -> bool {
    coord - lo > -eps && cell_side(coord, hi, eps) == Side::Inside
}
