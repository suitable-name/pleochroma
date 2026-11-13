//! Disc and ellipse overlap areas, inscribed polygons, the sun-disc preimage (beam guide §11.3).
//!
//! Polygons are plain `&[DVec2]` slices (simple polygons, either orientation); the area functions
//! return the unsigned area of the intersection. The closed forms follow beam guide §11.4 step 3:
//! the circle case sums, over the polygon edges, the signed area of (triangle from the centre)
//! intersected with the disc, and the ellipse case normalises to the unit circle first.

use glam::{DMat2, DMat3, DVec2, DVec3};

/// The largest vertex count [`inscribed_polygon`] produces.
pub const MAX_INSCRIBED_VERTICES: u32 = 4096;

/// The signed area of the triangle `(0, a, b)` intersected with the disc of squared radius `r2`
/// centred on the origin.
///
/// The segment is cut at its crossings with the circle; a piece inside the disc contributes its
/// triangle, a piece outside contributes the circular sector it subtends.
fn segment_disc_area(a: DVec2, b: DVec2, r2: f64) -> f64 {
    let edge = b - a;
    let dd = edge.length_squared();
    let mut ts = [0.0_f64; 4];
    let mut count = 1;
    if dd > 0.0 {
        let ad = a.dot(edge);
        let c = a.length_squared() - r2;
        let disc = ad.mul_add(ad, -(dd * c));
        if disc > 0.0 {
            let root = disc.sqrt();
            for t in [(-ad - root) / dd, (-ad + root) / dd] {
                if t > 0.0 && t < 1.0 {
                    ts[count] = t;
                    count += 1;
                }
            }
        }
    }
    ts[count] = 1.0;
    count += 1;
    ts[..count]
        .windows(2)
        .map(|span| {
            let from = a + edge * span[0];
            let to = a + edge * span[1];
            let mid = (from + to) * 0.5;
            let cross = from.perp_dot(to);
            if mid.length_squared() <= r2 {
                0.5 * cross
            } else {
                0.5 * r2 * cross.atan2(from.dot(to))
            }
        })
        .sum()
}

/// The area of the intersection of the disc (`centre`, `r`) with the simple polygon `poly`.
///
/// Closed form: the sum over the edges of the segment-and-sector contributions. The polygon may
/// be wound either way and may be concave. Fewer than three vertices, or a radius that is not
/// positive and finite, give `0`.
#[must_use]
pub fn circle_polygon_area(centre: DVec2, r: f64, poly: &[DVec2]) -> f64 {
    if poly.len() < 3 || !r.is_finite() || r <= 0.0 {
        return 0.0;
    }
    let r2 = r * r;
    let signed: f64 = poly
        .iter()
        .zip(poly.iter().cycle().skip(1))
        .map(|(&p, &q)| segment_disc_area(p - centre, q - centre, r2))
        .sum();
    signed.abs()
}

/// The area of the intersection of the ellipse `{ centre + axes * u : |u| <= 1 }` with the simple
/// polygon `poly`.
///
/// The plane is mapped affinely to the unit circle, [`circle_polygon_area`] is evaluated there and
/// the result is scaled by `|det axes|`. A singular or non-finite `axes` gives `0`.
#[must_use]
pub fn ellipse_polygon_area(centre: DVec2, axes: DMat2, poly: &[DVec2]) -> f64 {
    let det = axes.determinant();
    if !det.is_finite() || det.abs() <= f64::MIN_POSITIVE {
        return 0.0;
    }
    let inv = axes.inverse();
    let unit: Vec<DVec2> = poly.iter().map(|&p| inv * (p - centre)).collect();
    circle_polygon_area(DVec2::ZERO, 1.0, &unit) * det.abs()
}

/// Counter-clockwise polygon inscribed in the circle, its sagitta at most `max_sagitta`.
///
/// The vertices lie on the circle at the angles `2 pi k / n` (the first at angle zero) with the
/// smallest `n >= 3` such that `r * (1 - cos(pi / n)) <= max_sagitta`, capped at
/// [`MAX_INSCRIBED_VERTICES`] (also the count for a sagitta that is not positive). The polygon is
/// inside the circle, so it under-covers the disc by at most the sagitta along every edge.
/// A radius that is not positive and finite, or a `NaN` sagitta, gives an empty polygon.
#[must_use]
pub fn inscribed_polygon(centre: DVec2, r: f64, max_sagitta: f64) -> Vec<DVec2> {
    if !r.is_finite() || r <= 0.0 || max_sagitta.is_nan() {
        return Vec::new();
    }
    let sagitta = |n: u32| {
        let half = std::f64::consts::PI / f64::from(n);
        2.0 * r * (0.5 * half).sin().powi(2)
    };
    // The sagitta decreases with n: binary search for the smallest n that meets the bound.
    let (mut lo, mut hi) = (3_u32, MAX_INSCRIBED_VERTICES);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if sagitta(mid) <= max_sagitta {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let n = lo;
    (0..n)
        .map(|k| {
            let angle = std::f64::consts::TAU * f64::from(k) / f64::from(n);
            centre + DVec2::new(angle.cos(), angle.sin()) * r
        })
        .collect()
}

/// An ellipse in image space: the points `centre + axes * u` with `|u| <= 1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageEllipse {
    /// The centre.
    pub centre: DVec2,
    /// The matrix taking the unit disc to the ellipse (columns are conjugate semi-axes); it is
    /// invertible.
    pub axes: DMat2,
}

impl ImageEllipse {
    /// The area of the ellipse.
    #[must_use]
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.axes.determinant().abs()
    }

    /// Whether `p` is inside or on the ellipse.
    #[must_use]
    pub fn contains(&self, p: DVec2) -> bool {
        (self.axes.inverse() * (p - self.centre)).length_squared() <= 1.0
    }

    /// The area of the intersection with the simple polygon `poly`.
    #[must_use]
    pub fn polygon_area(&self, poly: &[DVec2]) -> f64 {
        ellipse_polygon_area(self.centre, self.axes, poly)
    }
}

/// Image-space preimage of the sun disc under a direction field that is projective in the image
/// (beam guide §11.4 step 3).
///
/// `dir_map` maps the homogeneous image point `(sx, sy, 1)` to an unnormalised exit direction
/// (only its direction matters): the columns are the derivative along `sx`, along `sy` and the
/// value at `(0, 0)`. For the primary camera see
/// [`camera_direction_map`](super::homography::camera_direction_map). A reflection chain
/// multiplies it on the left by the product of the mirror matrices, so the map is still exactly of
/// this form (a reflection leaf). `sun_dir` is the direction of the sun (any length) and
/// `tan_radius` the tangent of the disc's angular radius, whose cosine is `SUN_DISC_COS`.
///
/// The disc is `{ v : angle(v, sun_dir) <= r }` and its gnomonic image is a circle of radius
/// `tan r`, so the preimage is the conic `|v|^2 - (1 + tan^2 r) (v . sun_dir)^2 <= 0` with
/// `v = dir_map * (sx, sy, 1)`: a quadratic form in `(sx, sy, 1)`, computed exactly. The result is
/// the ellipse on the sun's side of the double cone.
///
/// Returns `None` when the preimage is not a bounded ellipse on the sun's side: the section of the
/// cone is a parabola or hyperbola (the disc touches the plane at infinity of the image), the
/// ellipse lies on the opposite nappe (the sun is behind), `dir_map` is singular, or an input is
/// not finite or `tan_radius` is not positive.
#[must_use]
pub fn sun_disc_preimage(dir_map: DMat3, sun_dir: DVec3, tan_radius: f64) -> Option<ImageEllipse> {
    let len = sun_dir.length();
    if !len.is_finite() || len <= 0.0 || !tan_radius.is_finite() || tan_radius <= 0.0 {
        return None;
    }
    let s = sun_dir / len;
    let sec2 = tan_radius.mul_add(tan_radius, 1.0);
    let outer = DMat3::from_cols(s * s.x, s * s.y, s * s.z);
    let form = DMat3::IDENTITY - outer * sec2;
    let q = dir_map.transpose() * form * dir_map;
    // q(x, y) = qxx x^2 + 2 qxy x y + qyy y^2 + 2 (qx x + qy y) + q0 <= 0.
    let (qxx, qxy, qyy) = (q.x_axis.x, q.y_axis.x, q.y_axis.y);
    let (qx, qy, q0) = (q.z_axis.x, q.z_axis.y, q.z_axis.z);
    let det = qxx.mul_add(qyy, -(qxy * qxy));
    if !(qxx > 0.0 && det > 0.0 && det.is_finite()) {
        return None;
    }
    let centre = DVec2::new(
        -qyy.mul_add(qx, -(qxy * qy)) / det,
        -qxx.mul_add(qy, -(qxy * qx)) / det,
    );
    let kappa = -centre.dot(DVec2::new(qx, qy)) - q0;
    if !(kappa > 0.0 && kappa.is_finite()) {
        return None;
    }
    if (dir_map * centre.extend(1.0)).dot(s) <= 0.0 {
        return None;
    }
    // A = L L^T (Cholesky); x - centre = sqrt(kappa) L^-T u maps the unit disc onto the ellipse.
    let l11 = qxx.sqrt();
    let l21 = qxy / l11;
    let l22 = (det / qxx).sqrt();
    let scale = kappa.sqrt();
    let axes = DMat2::from_cols(
        DVec2::new(1.0 / l11, 0.0) * scale,
        DVec2::new(-l21 / (l11 * l22), 1.0 / l22) * scale,
    );
    axes.is_finite().then_some(ImageEllipse { centre, axes })
}

/// First-order (Jacobian) image-space preimage of the sun disc for a refracted leaf
/// (beam guide §11.4 step 3).
///
/// The exit direction near the image point `x0` is linearised as
/// `omega(x) = omega0 + dw_d_screen_x * (sx - x0.x) + dw_d_screen_y * (sy - x0.y)`, with `omega0`
/// the exact exit direction at `x0`, and `dw_d_screen_x`, `dw_d_screen_y` its derivatives with
/// respect to the screen coordinates (as in ray differentials). The linearised field is affine in
/// the image, so the result is [`sun_disc_preimage`] of that field. The caller subdivides the
/// fragment until the Jacobian's variation over it is below `tau_L`. The arguments `sun_dir` and
/// `tan_radius` and the `None` cases are those of [`sun_disc_preimage`].
#[must_use]
pub fn sun_disc_preimage_jacobian(
    x0: DVec2,
    omega0: DVec3,
    dw_d_screen_x: DVec3,
    dw_d_screen_y: DVec3,
    sun_dir: DVec3,
    tan_radius: f64,
) -> Option<ImageEllipse> {
    let value_at_origin = omega0 - dw_d_screen_x * x0.x - dw_d_screen_y * x0.y;
    let dir_map = DMat3::from_cols(dw_d_screen_x, dw_d_screen_y, value_at_origin);
    sun_disc_preimage(dir_map, sun_dir, tan_radius)
}
