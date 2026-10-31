//! Defining the rough's crystal orientation.
//!
//! The orientation lives on `RoughModel::orientation` (`None` is Unknown) as a
//! [`CrystalOrientation`] in the ROUGH frame (mm): the tilt is measured from the rough's `+Y`
//! axis, the azimuth from `+X` toward `+Z`, the same convention as a stone's own axis. These
//! functions turn what the cutter points at into that orientation; the window stores it (one
//! undo step) with the rest of the model.
//!
//! - [`align_to_face`]: the normal of a face the cutter clicked.
//! - [`align_to_edge`]: the line through two points.
//! - [`fit_to_crystal_length`]: the principal axis of the rough's points (a crystal is usually
//!   longest along its axis), with how confident that is.
//! - [`orientation_from_color_zones`]: the axis of a coaxial color zone the rough color fit
//!   found (a cylinder or prism around the crystal axis).
//!
//! A line has no direction: the canonical axis keeps the upper hemisphere, so a face normal
//! and its opposite give the same orientation. The Second Axis turn, when given, is kept as it
//! is (a turn is relative to the material's own completion for the new axis).

use glam::DVec3;
use pleochroma::optics::zoning::{ZoneShape, ZonedAbsorption};

use crate::{
    crystal_axis::{CrystalAxis, CrystalOrientation},
    rough_plan::{RoughBase, RoughModel},
};

/// The orientation whose Crystal Axis runs along `direction`, `None` for a zero or non-finite
/// vector.
fn orientation_along(direction: DVec3, second_turn_deg: Option<f64>) -> Option<CrystalOrientation> {
    if !direction.is_finite() || direction.length_squared() <= 0.0 {
        return None;
    }
    let axis = CrystalAxis::from_direction(direction);
    Some(CrystalOrientation::from_angles(
        axis.tilt_deg(),
        axis.azimuth_deg(),
        second_turn_deg,
    ))
}

/// Align to Face: the Crystal Axis along the normal of a face (rough frame). `None` for a
/// normal that is zero or not finite.
#[must_use]
pub fn align_to_face(normal: [f64; 3], second_turn_deg: Option<f64>) -> Option<CrystalOrientation> {
    orientation_along(DVec3::from_array(normal), second_turn_deg)
}

/// Align to Edge: the Crystal Axis along the line from `from` to `to` (rough frame, mm). `None`
/// when the two points coincide or are not finite.
#[must_use]
pub fn align_to_edge(
    from: [f64; 3],
    to: [f64; 3],
    second_turn_deg: Option<f64>,
) -> Option<CrystalOrientation> {
    orientation_along(
        DVec3::from_array(to) - DVec3::from_array(from),
        second_turn_deg,
    )
}

/// The orientation a Fit to Crystal Length found and how far to trust it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrystalLengthFit {
    /// The orientation along the points' longest direction.
    pub orientation: CrystalOrientation,
    /// How much longer the longest direction is than the next, `0` (as long: no direction
    /// stands out) to `1` (a needle): `1 - second / first` of the variances.
    pub confidence: f64,
    /// The ratio of the spread (standard deviation) along the longest direction to the spread
    /// along the next, at least `1`.
    pub elongation: f64,
}

/// The eigenvalues (descending) and unit eigenvectors of the symmetric matrix `m`, by Jacobi
/// rotations (fixed sweeps, no randomness: the same input gives the same answer).
#[expect(
    clippy::many_single_char_names,
    reason = "the Jacobi rotation's textbook names and fixed deterministic sweep order"
)]
fn eigen_symmetric(m: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let scale = (a[0][0].abs() + a[1][1].abs() + a[2][2].abs()).max(f64::MIN_POSITIVE);
    for _ in 0..64 {
        let (mut p, mut q, mut big) = (0, 1, a[0][1].abs());
        if a[0][2].abs() > big {
            (p, q, big) = (0, 2, a[0][2].abs());
        }
        if a[1][2].abs() > big {
            (p, q, big) = (1, 2, a[1][2].abs());
        }
        if big <= 1e-15 * scale {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + theta.mul_add(theta, 1.0).sqrt());
        let c = 1.0 / t.mul_add(t, 1.0).sqrt();
        let s = t * c;
        let (app, aqq, apq) = (a[p][p], a[q][q], a[p][q]);
        a[p][p] = t.mul_add(-apq, app);
        a[q][q] = t.mul_add(apq, aqq);
        a[p][q] = 0.0;
        a[q][p] = 0.0;
        let r = 3 - p - q;
        let (arp, arq) = (a[r][p], a[r][q]);
        a[r][p] = s.mul_add(-arq, c * arp);
        a[p][r] = a[r][p];
        a[r][q] = c.mul_add(arq, s * arp);
        a[q][r] = a[r][q];
        for row in &mut v {
            let (vp, vq) = (row[p], row[q]);
            row[p] = s.mul_add(-vq, c * vp);
            row[q] = c.mul_add(vq, s * vp);
        }
    }
    let mut pairs: Vec<(f64, DVec3)> = (0..3)
        .map(|i| (a[i][i], DVec3::new(v[0][i], v[1][i], v[2][i])))
        .collect();
    // Descending by eigenvalue; equal values keep their order (a stable sort).
    pairs.sort_by(|x, y| y.0.total_cmp(&x.0));
    (
        [pairs[0].0, pairs[1].0, pairs[2].0],
        [pairs[0].1, pairs[1].1, pairs[2].1],
    )
}

/// Fit to Crystal Length: the principal axis of `points`, with its confidence.
///
/// The points are the rough's corners or its mesh vertices, in the rough frame. `None` for fewer than two points, a point that
/// is not finite, or points with no spread.
#[must_use]
pub fn fit_to_crystal_length(
    points: &[DVec3],
    second_turn_deg: Option<f64>,
) -> Option<CrystalLengthFit> {
    if points.len() < 2 || points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let count = points.len() as f64;
    let centre = points.iter().copied().sum::<DVec3>() / count;
    let mut covariance = [[0.0_f64; 3]; 3];
    for point in points {
        let d = (*point - centre).to_array();
        for (i, row) in covariance.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = d[i].mul_add(d[j], *cell);
            }
        }
    }
    for row in &mut covariance {
        for cell in row {
            *cell /= count;
        }
    }
    let (values, vectors) = eigen_symmetric(covariance);
    if !(values[0].is_finite() && values[0] > 1e-18) {
        return None;
    }
    let second = values[1].max(0.0);
    let orientation = orientation_along(vectors[0], second_turn_deg)?;
    Some(CrystalLengthFit {
        orientation,
        confidence: (1.0 - second / values[0]).clamp(0.0, 1.0),
        elongation: if second > 0.0 {
            (values[0] / second).sqrt()
        } else {
            f64::INFINITY
        },
    })
}

/// [`fit_to_crystal_length`] for a rough model.
///
/// Uses the vertices of an imported mesh (the true outline, never the vertices of an inclusion
/// inside it), else the corners of the base before the cuts. A block, cylinder or pebble uses
/// its base corners with or without inclusions, so adding one does not move the fitted axis.
/// `None` when it has no usable points.
#[must_use]
pub fn fit_to_crystal_length_of(
    model: &RoughModel,
    second_turn_deg: Option<f64>,
) -> Option<CrystalLengthFit> {
    if let RoughBase::Hull { .. } = model.base
        && let Some(mesh) = model.mesh()
    {
        return fit_to_crystal_length(mesh.outer_vertices(), second_turn_deg);
    }
    let corners = model.base.vertices(false).ok()?;
    fit_to_crystal_length(&corners, second_turn_deg)
}

/// From Color Zones: the axis of the first coaxial cylinder or prism zone of `zoned`.
///
/// The zones are those of the plan's stored rough color, in the rough frame; the axis is the
/// direction a crystal's color zoning runs along. `None` when the zoning has no such zone.
#[must_use]
pub fn orientation_from_color_zones(
    zoned: &ZonedAbsorption,
    second_turn_deg: Option<f64>,
) -> Option<CrystalOrientation> {
    zoned
        .zones
        .iter()
        .find_map(|zone| match &zone.shape {
            ZoneShape::CoaxialCylinder { axis_dir, .. }
            | ZoneShape::CoaxialPrism { axis_dir, .. } => Some(*axis_dir),
            _ => None,
        })
        .and_then(|direction| orientation_along(direction, second_turn_deg))
}
