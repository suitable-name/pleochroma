//! Homographies: camera to plane, plane to plane projection, inverse, compose (beam guide §6.3).
//!
//! Everything here is `f64` and works on plain `glam` types. A homography is a [`DMat3`] acting on
//! homogeneous 2D points `(x, y, 1)`; [`apply`] divides by the third component.
//!
//! # Planes and plane frames
//!
//! A plane is a pair `(normal, offset)` ([`Plane`]) of the points `p` with `normal . p = offset`.
//! The normal need not be a unit vector. Carrier-plane coordinates are expressed in the plane
//! frame [`PlaneBasis`] of that plane, a pure function of the pair (so two callers that build the
//! frame from the same pair agree bit for bit):
//!
//! - the origin is the foot of the perpendicular from the origin: `normal * offset / |normal|^2`;
//! - `u` and `v` are orthonormal, tangent to the plane, and `u x v = normal / |normal|`;
//! - `u = normalize(helper x normal)` where `helper` is the world axis along which the unit normal
//!   has the smallest absolute component (the first of `x`, `y`, `z` on ties), `v = normal x u`
//!   (unit normal).
//!
//! This is exactly [`Frame2::from_normal`] with the foot of the perpendicular as the origin, so
//! [`PlaneBasis::frame`] gives the polygon kernel's frame bit for bit: polygons and homographies
//! of one carrier plane always share one 2D coordinate system.
//!
//! The plane coordinates of a world point `q` are `((q - origin) . u, (q - origin) . v)`.

use glam::{DMat3, DVec2, DVec3};

use super::frame::Frame2;
use crate::optics::raytracer::Camera;

/// A plane `(normal, offset)`: the points `p` with `normal . p = offset`.
pub type Plane = (DVec3, f64);

/// The 2D frame of a [`Plane`]: an origin and two orthonormal tangent axes (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneBasis {
    /// The foot of the perpendicular from the world origin to the plane.
    pub origin: DVec3,
    /// The first tangent axis (unit).
    pub u: DVec3,
    /// The second tangent axis (unit), with `u x v` the unit normal.
    pub v: DVec3,
    /// The unit normal.
    pub normal: DVec3,
}

impl PlaneBasis {
    /// Builds the frame of `plane`.
    ///
    /// A zero or non-finite normal falls back to the unit `z` axis with the origin at the world
    /// origin, so the result is always finite.
    #[must_use]
    pub fn new(plane: Plane) -> Self {
        let (normal, offset) = plane;
        let len = normal.length();
        let (unit, origin) = if len.is_finite() && len > 0.0 && offset.is_finite() {
            (normal / len, normal * (offset / (len * len)))
        } else {
            (DVec3::Z, DVec3::ZERO)
        };
        let a = unit.abs();
        let helper = if a.x <= a.y && a.x <= a.z {
            DVec3::X
        } else if a.y <= a.z {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let u = helper.cross(unit).normalize();
        let v = unit.cross(u);
        Self {
            origin,
            u,
            v,
            normal: unit,
        }
    }

    /// The same frame as the polygon kernel's [`Frame2`], bit for bit.
    #[must_use]
    pub const fn frame(&self) -> Frame2 {
        Frame2 {
            origin: self.origin,
            u: self.u,
            v: self.v,
            normal: self.normal,
        }
    }

    /// The world point of the plane coordinates `p`.
    #[must_use]
    pub fn to_world(&self, p: DVec2) -> DVec3 {
        self.origin + self.u * p.x + self.v * p.y
    }

    /// The plane coordinates of the world point `q` (its component along the normal is dropped).
    #[must_use]
    pub fn from_world(&self, q: DVec3) -> DVec2 {
        let rel = q - self.origin;
        DVec2::new(rel.dot(self.u), rel.dot(self.v))
    }
}

/// The matrix of the central projection from `apex` onto `plane`, in the frame of `plane`,
/// of the rays whose directions are the affine family `x * dirs[0] + y * dirs[1] + dirs[2]`.
///
/// The ray `apex + s * D` meets the plane at `s = (offset - normal . apex) / (normal . D)`, so
/// the homogeneous point `apex * (normal . D) + D * (offset - normal . apex)` with weight
/// `normal . D` is linear in `D`, hence in `(x, y, 1)`.
fn projection_matrix(apex: DVec3, dirs: [DVec3; 3], plane: Plane, basis: &PlaneBasis) -> DMat3 {
    let (normal, offset) = plane;
    let k = offset - normal.dot(apex);
    let column = |c: DVec3| {
        let w = normal.dot(c);
        let rel = apex * w + c * k - basis.origin * w;
        DVec3::new(basis.u.dot(rel), basis.v.dot(rel), w)
    };
    DMat3::from_cols(column(dirs[0]), column(dirs[1]), column(dirs[2]))
}

/// The unnormalised camera direction as an affine function of the screen coordinates.
///
/// The returned matrix maps `(sx, sy, 1)` to `forward + right * u + up * v` with the exact
/// formulas of `Camera::generate_ray` (beam guide §5.1, evaluated in `f64`). Its columns are the
/// derivative along `sx`, the derivative along `sy` and the direction at `(0, 0)`. A mirror chain
/// applies a fixed linear map to it (guide §6.4), which is how
/// [`sun_disc_preimage`](super::circle::sun_disc_preimage) takes it. `width` and `height` must be
/// positive.
#[must_use]
pub fn camera_direction_map(camera: &Camera, width: u32, height: u32) -> DMat3 {
    let w = f64::from(width);
    let h = f64::from(height);
    let aspect = w / h;
    let tan = f64::from(camera.fov_tan);
    let forward = camera.forward.as_dvec3();
    let right = camera.right.as_dvec3();
    let up = camera.up.as_dvec3();
    // u = (sx / w - 0.5) * 2 * aspect * tan, v = (0.5 - sy / h) * 2 * tan.
    let d_screen_x = right * (2.0 * aspect * tan / w);
    let d_screen_y = up * (-2.0 * tan / h);
    let d_0 = forward - right * (aspect * tan) + up * tan;
    DMat3::from_cols(d_screen_x, d_screen_y, d_0)
}

/// The homography from image space to the carrier `plane`, in the frame of [`PlaneBasis::new`].
///
/// Image space is the screen coordinates `(sx, sy)` of beam guide §5.2: a pixel `(x, y)` covers
/// `[x - 0.5, x + 0.5) x [y - 0.5, y + 0.5)`. The matrix maps `(sx, sy, 1)` to the homogeneous
/// plane coordinates of the point where the camera ray through `(sx, sy)` meets `plane`, using
/// the formula of `Camera::generate_ray` (in `f64`, from the camera's `f32` basis). The third
/// component is `normal . D` for the unnormalised direction `D`, so it is zero for rays parallel
/// to the plane. `width` and `height` must be positive.
#[must_use]
pub fn camera_to_plane(camera: &Camera, width: u32, height: u32, plane: Plane) -> DMat3 {
    let map = camera_direction_map(camera, width, height);
    let basis = PlaneBasis::new(plane);
    projection_matrix(
        camera.origin.as_dvec3(),
        [map.x_axis, map.y_axis, map.z_axis],
        plane,
        &basis,
    )
}

/// The homography of the central projection from `apex` of the plane `from` onto the plane `to`.
///
/// Both planes are in their own frames ([`PlaneBasis::new`]): the matrix maps the homogeneous
/// `from`-plane coordinates of a point to the homogeneous `to`-plane coordinates of the point
/// where the ray from `apex` through it meets `to`. A beam's `image_to_plane` is propagated to the
/// next facet as `compose(central_projection(apex, carrier, next), image_to_plane)` (guide §6.3).
#[must_use]
pub fn central_projection(apex: DVec3, from: Plane, to: Plane) -> DMat3 {
    let from_basis = PlaneBasis::new(from);
    let to_basis = PlaneBasis::new(to);
    projection_matrix(
        apex,
        [from_basis.u, from_basis.v, from_basis.origin - apex],
        to,
        &to_basis,
    )
}

/// Applies the homography `h` to the point `p` and divides by the third component.
///
/// Returns `None` when the result is at infinity (third component zero) or not finite.
#[must_use]
pub fn apply(h: DMat3, p: DVec2) -> Option<DVec2> {
    let r = h * p.extend(1.0);
    if r.is_finite() && r.z.abs() > f64::MIN_POSITIVE {
        Some(DVec2::new(r.x / r.z, r.y / r.z))
    } else {
        None
    }
}

/// The inverse of the homography `h`, or `None` when it is singular or not finite.
#[must_use]
pub fn inverse(h: DMat3) -> Option<DMat3> {
    let det = h.determinant();
    if !det.is_finite() || det.abs() <= f64::MIN_POSITIVE {
        return None;
    }
    let inv = h.inverse();
    inv.is_finite().then_some(inv)
}

/// The composition `a . b`: the homography that applies `b` first, then `a`.
///
/// Fixed order `a * b` (`glam`'s column-vector product), so equal inputs give equal bits.
#[must_use]
pub fn compose(a: DMat3, b: DMat3) -> DMat3 {
    a * b
}
