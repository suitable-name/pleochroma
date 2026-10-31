//! Interface optics at a sampled micro-normal: the unpolarized Fresnel reflectance and the
//! reflection and refraction directions.
//!
//! Directions follow the microfacet convention: the incident direction `i` points AWAY from the
//! surface (so it is minus the propagation direction), and so does the reflected direction. A
//! transmitted direction is returned as the propagation direction into the other medium, which
//! lies on the far side of the surface.

use glam::Vec3;

/// The unpolarized Fresnel reflectance of an interface from index `n1` to `n2` at incidence
/// cosine `cos_i`; `1` past the critical angle.
#[must_use]
pub fn fresnel_unpolarized(n1: f32, n2: f32, cos_i: f32) -> f32 {
    let eta = n1 / n2;
    let sin2_t = eta * eta * cos_i.mul_add(-cos_i, 1.0);
    if sin2_t >= 1.0 {
        return 1.0;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    let r_s = n1.mul_add(cos_i, -(n2 * cos_t)) / n1.mul_add(cos_i, n2 * cos_t);
    let r_p = n2.mul_add(cos_i, -(n1 * cos_t)) / n2.mul_add(cos_i, n1 * cos_t);
    0.5 * r_p.mul_add(r_p, r_s * r_s)
}

/// The mirror direction of `i` about the micro-normal `m`: `2 (i·m) m − i`.
#[must_use]
pub fn reflect_about(i: Vec3, m: Vec3) -> Vec3 {
    2.0 * i.dot(m) * m - i
}

/// The direction light arriving along `−i` takes after refracting through the micro-normal `m`
/// from index `n1` into `n2`, or `None` past the critical angle. `i·m` must be positive.
#[must_use]
pub fn refract_about(i: Vec3, m: Vec3, n1: f32, n2: f32) -> Option<Vec3> {
    let eta = n1 / n2;
    let c = i.dot(m);
    let k = eta * eta * c.mul_add(-c, 1.0);
    let k = 1.0 - k;
    if k < 0.0 {
        return None;
    }
    Some(eta.mul_add(c, -k.sqrt()) * m - eta * i)
}
