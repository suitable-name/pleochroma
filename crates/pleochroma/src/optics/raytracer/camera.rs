//! Camera/ray generation.
//!
//! The pinhole [`Camera`] model plus the [`Ray`] and [`HitRecord`] primitives every
//! intersection routine in this module tree operates on, and [`FacetFinish`] (a
//! facet's surface finish, looked up per-hit alongside its [`HitRecord::facet_idx`]).

use glam::Vec3;

/// A ray with an origin and a direction.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    /// Ray origin.
    pub origin: Vec3,
    /// Ray direction.
    pub dir: Vec3,
}

/// The entry hit of a ray against a faceted solid.
#[derive(Clone, Copy, Debug)]
pub struct HitRecord {
    /// Ray parameter of the hit.
    pub t: f32,
    /// Outward normal of the hit facet.
    pub normal: Vec3,
    /// Index of the hit facet in the plane slice.
    pub facet_idx: usize,
}

/// Girdle finish: a facet's surface finish.
///
/// Looked up per-hit via `HitRecord::facet_idx` into a `&[FacetFinish]` slice parallel to
/// `&[GpuFacetPlane]` (not a new field on `GpuFacetPlane` itself, to avoid disturbing that
/// struct's byte layout, its GPU echo test, or the Tier 2 `intersect_polyhedron` kernel
/// that reads it -- none of which need to know about finish at all). See
/// `apply_frosted_bounce`'s doc comment for what `Frosted` actually models, and
/// `trace_spectral_ray_with_finish`'s doc comment for why this is a distinct CPU entry
/// point from `trace_spectral_ray` rather than a new parameter on it.
///
/// The GPU megakernel has a full frosted-finish counterpart, not merely a Tier 2
/// self-test: `renderer::gpu::frame::TransportScene::facet_finishes` accepts a
/// `&[FacetFinish]` slice exactly like this one, `renderer::buffers::encode_facet_finishes`
/// packs it into the same `facet_finishes` GPU buffer `shaders/spectral_transport.wgsl`
/// reads (binding 8), and that kernel routes a `Frosted` hit through a bit-for-bit WGSL
/// translation of `apply_frosted_bounce` (in `shaders/transport_physics.wgsl`) instead of
/// the polished TIR/reflect/refract dispatch -- verified against this real CPU function by
/// `renderer::gpu::transport_check`'s Tier 2 `run_frosted_bounce` self-test. Unlike biaxial
/// materials (`GemMaterial::gpu_supported`), which remain a genuine CPU-only restriction,
/// `FacetFinish::Frosted` is not missing anything on the GPU side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FacetFinish {
    /// A smooth optical interface: the existing Dirac-delta specular Fresnel machinery,
    /// completely unchanged. Every scene rendered before girdle finish was added (and every scene calling
    /// the unmodified `trace_spectral_ray`) is implicitly all-`Polished`.
    #[default]
    Polished,
    /// A ground/lapped ("bruted") surface: diffusely scattering rather than mirror-like.
    Frosted,
}

/// The vertical field of view, in degrees, every renderer front end builds its
/// [`Camera`] with.
///
/// The CPU tracer, the GPU megakernel's camera uniform, the rasterised Solid view and
/// its orbit "Fit" distance must all agree on it, or the fitted pose would not match
/// what ends up on screen.
pub const DEFAULT_FOV_DEG: f32 = 42.0;

/// An orbit camera pose: `yaw` and `pitch` in radians, `distance` in scene units.
///
/// The three arguments [`Camera::new`] takes before the field of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitPose {
    /// Azimuth around the vertical axis, in radians.
    pub yaw: f32,
    /// Elevation above the horizon, in radians.
    pub pitch: f32,
    /// Distance from the orbit centre, in scene units.
    pub distance: f32,
}

/// The pose a fresh session and every "Reset" camera action start from.
///
/// Yaw 0.60 (about 35 degrees azimuth), pitch 0.45 (about 26 degrees elevation, showing
/// crown, table and pavilion sparkle in 3D) and distance 2.4.
pub const DEFAULT_POSE: OrbitPose = OrbitPose {
    yaw: 0.60,
    pitch: 0.45,
    distance: 2.4,
};

/// Orbit pinhole camera looking at the origin.
pub struct Camera {
    /// Eye position.
    pub origin: Vec3,
    /// Unit viewing direction.
    pub forward: Vec3,
    /// Unit right vector of the image plane.
    pub right: Vec3,
    /// Unit up vector of the image plane.
    pub up: Vec3,
    /// Tangent of the half field of view.
    pub fov_tan: f32,
}

impl Camera {
    /// Builds an orbit camera from yaw, pitch, distance to the origin and vertical field of view in degrees.
    #[must_use]
    pub fn new(yaw: f32, pitch: f32, distance: f32, fov_deg: f32) -> Self {
        let cos_p = pitch.cos();
        let sin_p = pitch.sin();
        let cos_y = yaw.cos();
        let sin_y = yaw.sin();

        // 3D camera position orbiting origin (0, 0, 0)
        let origin = Vec3::new(
            distance * cos_p * sin_y,
            distance * sin_p,
            distance * cos_p * cos_y,
        );

        let forward = (-origin).normalize();
        // Past +/-90 deg of pitch the camera is over the pole and `+Y` would flip
        // `right`; using `-Y` there keeps `right = (cos_yaw, 0, -sin_yaw)` continuous
        // through the pole, so a free orbit never mirrors the image. Exactly at the
        // pole the cross product degenerates, so `right` is that same yaw circle's
        // tangent (signed by the pole): the Top and Bottom views turn with the yaw like
        // every other view, which Turn 90 and a sideways drag rely on. At yaw 0 this is
        // the fixed orientation the batch previews' top view always had (`forward x -Z`).
        // `cos_p > 0` is bit-identical to before.
        let right = if cos_p.abs() < 1e-4 {
            Vec3::new(cos_y, 0.0, -sin_y) * sin_p.signum()
        } else {
            let world_up = if cos_p < 0.0 { Vec3::NEG_Y } else { Vec3::Y };
            forward.cross(world_up).normalize()
        };
        let up = right.cross(forward).normalize();
        let fov_tan = (fov_deg.to_radians() * 0.5).tan();

        Self {
            origin,
            forward,
            right,
            up,
            fov_tan,
        }
    }

    /// Rotates the image plane clockwise without changing the viewing direction,
    /// eye position, distance or field of view. Zero keeps the original basis exact.
    #[must_use]
    pub fn with_roll(mut self, radians: f32) -> Self {
        if radians != 0.0 {
            let (sin, cos) = radians.sin_cos();
            let right = self.right * cos + self.up * sin;
            self.up = self.up * cos - self.right * sin;
            self.right = right;
        }
        self
    }

    /// Generates the primary ray through a (jittered) pixel position.
    #[must_use]
    pub fn generate_ray(
        &self,
        screen_x: f32,
        screen_y: f32,
        width: f32,
        height: f32,
        jitter_x: f32,
        jitter_y: f32,
    ) -> Ray {
        let aspect = width / height;
        let u = ((screen_x + jitter_x) / width - 0.5) * 2.0 * aspect * self.fov_tan;
        let v = (0.5 - (screen_y + jitter_y) / height) * 2.0 * self.fov_tan;
        let dir = (self.forward + self.right * u + self.up * v).normalize();

        Ray {
            origin: self.origin,
            dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn image_roll_preserves_every_view_direction_and_zoom_including_the_poles() {
        for (yaw, pitch) in [
            (0.0, 0.0),
            (0.0, FRAC_PI_2),
            (0.0, -FRAC_PI_2),
            (-FRAC_PI_2, 0.0),
            (FRAC_PI_2, 0.0),
            (0.6, 0.45),
        ] {
            let original = Camera::new(yaw, pitch, 3.7, 42.0);
            let turned = Camera::new(yaw, pitch, 3.7, 42.0).with_roll(FRAC_PI_2);
            assert_eq!(turned.origin, original.origin);
            assert_eq!(turned.forward, original.forward);
            assert_eq!(turned.fov_tan, original.fov_tan);
            assert!(turned.right.abs_diff_eq(original.up, 1e-6));
            assert!(turned.up.abs_diff_eq(-original.right, 1e-6));
            let restored = Camera::new(yaw, pitch, 3.7, 42.0).with_roll(4.0 * FRAC_PI_2);
            assert!(restored.right.abs_diff_eq(original.right, 1e-6));
            assert!(restored.up.abs_diff_eq(original.up, 1e-6));
        }
    }

    /// A free orbit crosses the pole without the image mirroring: `right` on either
    /// side of +90 deg agrees, and the far side's `forward` still looks at the origin.
    #[test]
    fn right_vector_is_continuous_through_the_pole() {
        let yaw = 0.7;
        let before = Camera::new(yaw, FRAC_PI_2 - 0.01, 2.4, 42.0);
        let after = Camera::new(yaw, FRAC_PI_2 + 0.01, 2.4, 42.0);
        assert!(
            (before.right - after.right).length() < 1e-3,
            "right flipped through the pole: {:?} vs {:?}",
            before.right,
            after.right
        );
        assert!((after.forward + after.origin.normalize()).length() < 1e-6);
        assert!((after.up.dot(after.forward)).abs() < 1e-6);
    }

    /// Exactly at the pole the picture turns with the yaw (a quarter turn of yaw is a
    /// quarter turn of the image), matches the view just short of the pole, and keeps
    /// the yaw-0 orientation the top-view previews have always used.
    #[test]
    fn the_pole_views_turn_with_the_yaw() {
        for pitch in [FRAC_PI_2, -FRAC_PI_2] {
            let base = Camera::new(0.0, pitch, 2.4, 42.0);
            let expected_right = glam::Vec3::X * pitch.signum();
            assert!(
                base.right.abs_diff_eq(expected_right, 1e-6),
                "{:?}",
                base.right
            );
            let turned = Camera::new(FRAC_PI_2, pitch, 2.4, 42.0);
            assert!(
                turned.right.dot(base.right).abs() < 1e-6,
                "a quarter turn of yaw"
            );
            assert!(turned.forward.abs_diff_eq(base.forward, 1e-6));
            assert!((turned.right.dot(turned.forward)).abs() < 1e-6);
            assert!((turned.up.dot(turned.forward)).abs() < 1e-6);
        }
        let yaw = 0.7;
        let at_pole = Camera::new(yaw, FRAC_PI_2, 2.4, 42.0);
        let near_pole = Camera::new(yaw, FRAC_PI_2 - 0.01, 2.4, 42.0);
        assert!((at_pole.right - near_pole.right).length() < 1e-3);
    }
}
