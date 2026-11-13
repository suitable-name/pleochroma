//! `Frame2`: an orthonormal in-plane basis for a polygon's carrier plane (beam guide §13.4).
//!
//! The idea follows `Frame` in `pleochroma-editor/src/retarget/refit/clip.rs`: express the 3D
//! points of a plane in 2D coordinates `(u, v)` so areas, centroids and inside tests are plain 2D
//! arithmetic. The basis is right-handed, `u x v = normal`, so a polygon that is counter-clockwise
//! seen along the normal has positive 2D area.

use glam::{DVec2, DVec3};

/// An orthonormal frame of a plane: an origin on the plane, two in-plane axes and the unit normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame2 {
    /// A point on the plane, the 2D origin.
    pub origin: DVec3,
    /// The first in-plane axis (unit).
    pub u: DVec3,
    /// The second in-plane axis (unit), `normal x u`.
    pub v: DVec3,
    /// The unit normal, `u x v`.
    pub normal: DVec3,
}

impl Frame2 {
    /// The identity frame of the plane `z = 0`: image space as a plane.
    pub const IMAGE: Self = Self {
        origin: DVec3::ZERO,
        u: DVec3::X,
        v: DVec3::Y,
        normal: DVec3::Z,
    };

    /// The frame of the plane through `origin` with the given normal, which need not be unit.
    ///
    /// The axis choice is deterministic: `u` is built from the coordinate axis least aligned with
    /// the normal. Returns `None` when the normal is zero or not finite.
    #[must_use]
    pub fn from_normal(origin: DVec3, normal: DVec3) -> Option<Self> {
        let len = normal.length();
        if !(len.is_finite() && len > 0.0 && origin.is_finite()) {
            return None;
        }
        let normal = normal / len;
        let a = normal.abs();
        let axis = if a.x <= a.y && a.x <= a.z {
            DVec3::X
        } else if a.y <= a.z {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let u = axis.cross(normal).normalize();
        let v = normal.cross(u);
        Some(Self {
            origin,
            u,
            v,
            normal,
        })
    }

    /// The plane offset `m` in `normal . x = m`.
    #[must_use]
    pub fn offset(&self) -> f64 {
        self.normal.dot(self.origin)
    }

    /// The signed distance of `p` from the plane, positive on the normal's side.
    #[must_use]
    pub fn signed_distance(&self, p: DVec3) -> f64 {
        self.normal.dot(p - self.origin)
    }

    /// The 2D coordinates of the projection of `p` onto the plane.
    #[must_use]
    pub fn to_2d(&self, p: DVec3) -> DVec2 {
        let d = p - self.origin;
        DVec2::new(d.dot(self.u), d.dot(self.v))
    }

    /// The 3D point on the plane with the 2D coordinates `q`.
    #[must_use]
    pub fn to_3d(&self, q: DVec2) -> DVec3 {
        self.v.mul_add(
            DVec3::splat(q.y),
            self.u.mul_add(DVec3::splat(q.x), self.origin),
        )
    }

    /// The orthogonal projection of `p` onto the plane.
    #[must_use]
    pub fn project(&self, p: DVec3) -> DVec3 {
        self.to_3d(self.to_2d(p))
    }
}
