//! The glossy tray under the stone: a horizontal mirror plane with Schlick reflectance.
//!
//! The Tray Reflection turns the matte paper into a smooth dielectric-like surface. An
//! escaping downward ray (a camera ray past the stone or one leaving it) that meets the plane
//! at the culet is split by the Fresnel reflectance `F`: the share `1 - F` sees the matte tray
//! (the paper, the Stone Shadow included), the share `F` continues along the mirrored
//! direction and is traced for real, so the stone can appear in its own reflection and light
//! can reach the pavilion from below.
//!
//! - **Reflectance.** Schlick's approximation, `F = R0 + (1 - R0) (1 - cos(theta))^5` with
//!   `cos(theta) = -dir.y` for a unit direction, so `F` is `R0` looking straight down and
//!   rises to 1 at grazing incidence. `R0` is the head-on reflectance the user sets (about
//!   0.04 behaves like black acrylic or glass, 1 like a mirror).
//! - **Unpolarized.** The floor is wavelength-independent and depolarizes the beam; the
//!   transport code depolarizes and scales the Stokes vector by `F` in `depolarize_and_scale`.
//! - **Twins.** [`TrayMirror::hit`] uses plain multiplies and adds only, so the WGSL twins
//!   (`tm_floor` in the transport megakernel and `wt_floor` in the Ray-Traced shader) state
//!   the same operations in the same order.
//!
//! The plane height is the Stone Shadow's plane, see [`super::tray_plane::tray_plane_y`].

use glam::Vec3;

/// Distance, in model units, by which a reflected ray's start is lifted off the plane along the
/// reflected direction, so the restarted ray cannot re-hit the plane it just left.
const RESTART_LIFT: f32 = 1e-4;

/// The glossy tray under the stone: a horizontal plane at the culet with Schlick reflectance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayMirror {
    /// Height of the plane.
    plane_y: f32,
    /// Head-on reflectance `R0`, in `(0, 1]`.
    reflectance: f32,
}

/// Where a downward ray meets the tray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorHit {
    /// The hit point; `point.y == plane_y` exactly.
    pub point: Vec3,
    /// The mirrored direction `(dir.x, -dir.y, dir.z)`.
    pub reflected: Vec3,
    /// Schlick reflectance at this angle, in `[R0, 1]`.
    pub fresnel: f32,
}

impl TrayMirror {
    /// A mirror at height `plane_y` with head-on reflectance `reflectance`.
    ///
    /// `None` unless `plane_y` is finite and `reflectance > 0` (NaN is `None`); a reflectance
    /// above 1 clamps to 1.
    #[must_use]
    pub const fn new(plane_y: f32, reflectance: f32) -> Option<Self> {
        if !plane_y.is_finite() || reflectance.is_nan() || reflectance <= 0.0 {
            return None;
        }
        Some(Self {
            plane_y,
            reflectance: if reflectance > 1.0 { 1.0 } else { reflectance },
        })
    }

    /// Height of the plane (the Stone Shadow's plane).
    #[must_use]
    pub const fn plane_y(&self) -> f32 {
        self.plane_y
    }

    /// Head-on reflectance `R0`, in `(0, 1]`.
    #[must_use]
    pub const fn reflectance(&self) -> f32 {
        self.reflectance
    }

    /// The floor hit of the ray `(origin, dir)`, `dir` of unit length.
    ///
    /// `None` unless `dir.y < 0` and `origin.y > plane_y` (an upward ray, a horizontal one, or
    /// one that starts on or below the plane never meets it; a NaN `dir.y` or `origin.y` gives
    /// `None` too).
    /// The arithmetic is plain multiplies and adds, repeated operation for operation by the
    /// WGSL twins.
    #[must_use]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the WGSL twins state the same plain operations"
    )]
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<FloorHit> {
        let downward = dir.y < 0.0;
        let above = origin.y > self.plane_y;
        if !downward || !above {
            return None;
        }
        let t = (self.plane_y - origin.y) / dir.y;
        let point = Vec3::new(origin.x + dir.x * t, self.plane_y, origin.z + dir.z * t);
        let c = (-dir.y).clamp(0.0, 1.0);
        let m = 1.0 - c;
        let m2 = m * m;
        let m5 = m2 * m2 * m;
        let fresnel = self.reflectance + (1.0 - self.reflectance) * m5;
        Some(FloorHit {
            point,
            reflected: Vec3::new(dir.x, -dir.y, dir.z),
            fresnel,
        })
    }
}

impl FloorHit {
    /// The reflected ray's start, `point + reflected * 1e-4`: lifted off the plane so the new
    /// ray cannot re-hit it (the reflected direction points up).
    #[must_use]
    pub fn restart(&self) -> Vec3 {
        self.point + self.reflected * RESTART_LIFT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror(r0: f32) -> TrayMirror {
        TrayMirror::new(-1.0, r0).expect("valid mirror")
    }

    #[test]
    fn new_rejects_nan_and_non_positive_and_clamps_above_one() {
        assert!(TrayMirror::new(f32::NAN, 0.5).is_none());
        assert!(TrayMirror::new(f32::INFINITY, 0.5).is_none());
        assert!(TrayMirror::new(0.0, f32::NAN).is_none());
        assert!(TrayMirror::new(0.0, 0.0).is_none());
        assert!(TrayMirror::new(0.0, -0.1).is_none());
        let clamped = TrayMirror::new(0.25, 3.0).expect("clamps");
        assert_eq!(clamped.reflectance().to_bits(), 1.0_f32.to_bits());
        assert_eq!(clamped.plane_y().to_bits(), 0.25_f32.to_bits());
        let small = TrayMirror::new(0.0, 0.04).expect("small");
        assert_eq!(small.reflectance().to_bits(), 0.04_f32.to_bits());
    }

    #[test]
    fn upward_horizontal_and_below_plane_rays_miss() {
        let m = mirror(0.5);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let flat = Vec3::X;
        let down = Vec3::new(0.0, -1.0, 0.0);
        assert!(m.hit(Vec3::new(0.0, 2.0, 0.0), up).is_none());
        assert!(m.hit(Vec3::new(0.0, 2.0, 0.0), flat).is_none());
        // Starting below or exactly on the plane never hits it.
        assert!(m.hit(Vec3::new(0.0, -2.0, 0.0), down).is_none());
        assert!(m.hit(Vec3::new(0.0, -1.0, 0.0), down).is_none());
        assert!(m.hit(Vec3::new(f32::NAN, 2.0, 0.0), Vec3::NAN).is_none());
    }

    #[test]
    #[expect(clippy::suboptimal_flops, reason = "restates hit's plain operations")]
    fn hit_point_lies_exactly_on_the_plane() {
        let m = TrayMirror::new(-0.37, 0.04).expect("valid");
        let dir = Vec3::new(0.3, -0.8, 0.2).normalize();
        let hit = m
            .hit(Vec3::new(0.1, 1.3, -0.4), dir)
            .expect("a downward ray above the plane hits");
        assert_eq!(hit.point.y.to_bits(), (-0.37_f32).to_bits());
        assert_eq!(hit.reflected, Vec3::new(dir.x, -dir.y, dir.z));
        assert!(hit.reflected.y > 0.0);
        // The point is on the ray: it moved along dir by the parameter t.
        let t = (hit.point.y - 1.3) / dir.y;
        // `t` is `hit`'s own parameter: `hit.point.y` is `plane_y` exactly, so the same
        // operations give the same bits.
        assert_eq!(hit.point.x.to_bits(), (0.1 + dir.x * t).to_bits());
        assert_eq!(hit.point.z.to_bits(), (-0.4 + dir.z * t).to_bits());
    }

    #[test]
    fn fresnel_is_r0_straight_down_and_one_at_grazing() {
        let m = mirror(0.04);
        let down = m
            .hit(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0))
            .expect("hit");
        assert_eq!(down.fresnel.to_bits(), 0.04_f32.to_bits());
        let grazing = Vec3::new(1.0, -1e-4, 0.0).normalize();
        let hit = m.hit(Vec3::new(0.0, 1.0, 0.0), grazing).expect("hit");
        assert!(hit.fresnel > 0.999, "grazing F = {}", hit.fresnel);
        assert!(hit.fresnel <= 1.0 + 1e-6);
        // Monotone in between.
        let mid = Vec3::new(1.0, -1.0, 0.0).normalize();
        let f_mid = m.hit(Vec3::new(0.0, 1.0, 0.0), mid).expect("hit").fresnel;
        assert!(down.fresnel < f_mid && f_mid < hit.fresnel);
    }

    #[test]
    fn a_full_mirror_reflects_everything() {
        let m = mirror(1.0);
        let dir = Vec3::new(0.5, -0.5, 0.5).normalize();
        let hit = m.hit(Vec3::new(0.0, 1.0, 0.0), dir).expect("hit");
        assert_eq!(hit.fresnel.to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn restart_lifts_along_the_reflected_direction() {
        let m = mirror(0.5);
        let dir = Vec3::new(0.6, -0.8, 0.0);
        let hit = m.hit(Vec3::new(0.0, 1.0, 0.0), dir).expect("hit");
        let start = hit.restart();
        assert_eq!(start, hit.point + hit.reflected * 1e-4);
        assert!(start.y > m.plane_y());
    }
}
