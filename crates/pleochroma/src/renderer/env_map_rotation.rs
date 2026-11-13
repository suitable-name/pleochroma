//! The yaw of an HDR environment map about the vertical axis.
//!
//! A panorama is level, so the only turn that makes sense is the one about `+Y`. The turn is
//! one `(cos, sin)` pair ([`EnvRotation`]), computed once on the CPU in `f64` and cast to
//! `f32`, and it is applied by one rule on every engine: a world direction is rotated into the
//! map's own frame at the entry of every lookup or pdf ([`EnvRotation::to_map`]), and a
//! direction drawn from the map is rotated back to the world ([`EnvRotation::to_world`]).
//! Texel colors, pdfs (a rigid rotation keeps solid angle), the importance distribution and
//! every GPU upload stay as they are.
//!
//! The identity is exactly `(cos, sin) == (1.0, 0.0)`. Both directions return their input
//! untouched there, so an unrotated map renders bit for bit as it did before the turn existed
//! (the same precedent as `hdr_rgb_at_exposure`, which skips the multiply at exactly `1.0`).
//! The WGSL twins are `hdr_world_to_map` and `hdr_map_to_world`
//! (`transport_physics/05_nee_env_sampling.wgsl`); they state the same operations in the same
//! order.
//!
//! [`HdrOrientation`] is the scene-level value the front ends store: a yaw in radians and
//! whether the map turns with the camera.

use std::f32::consts::TAU;

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// A turn of the HDR map about `+Y`, as the cosine and sine of its yaw.
///
/// The sense: a map yaw `psi` puts the map's `+Z` (`u = 0`) at the world azimuth `psi`, the
/// same `atan2(x, z)` azimuth the studio key light uses for its `light_yaw`. So turning the
/// map by `psi` moves its picture the way turning the key light by `psi` moves the light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvRotation {
    cos: f32,
    sin: f32,
}

impl EnvRotation {
    /// No turn: `(cos, sin) == (1.0, 0.0)` exactly. Every lookup returns its input untouched.
    pub const IDENTITY: Self = Self { cos: 1.0, sin: 0.0 };

    /// The turn by `yaw` radians about `+Y`.
    ///
    /// Computed in `f64` (the angle wrapped with `rem_euclid(TAU)`, then `sin_cos`) and cast to
    /// `f32`. A NaN or infinite yaw, `+-0.0`, a whole number of `f32` turns and any yaw whose
    /// `(cos, sin)` rounds to `(1.0, 0.0)` give exactly [`Self::IDENTITY`].
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the f64 cosine and sine are in -1..=1 and are meant to round to f32 once"
    )]
    pub fn from_yaw(yaw: f32) -> Self {
        if !yaw.is_finite() {
            return Self::IDENTITY;
        }
        let yaw = f64::from(yaw);
        if (yaw / f64::from(TAU)).fract() == 0.0 {
            return Self::IDENTITY;
        }
        let (sin, cos) = yaw.rem_euclid(std::f64::consts::TAU).sin_cos();
        let (cos, sin) = (cos as f32, sin as f32);
        if cos == 1.0 && sin == 0.0 {
            return Self::IDENTITY;
        }
        Self { cos, sin }
    }

    /// Whether this is exactly the identity.
    #[must_use]
    pub const fn is_identity(self) -> bool {
        self.cos == 1.0 && self.sin == 0.0
    }

    /// A world direction in the map's frame (a turn by `-yaw` about `+Y`); the identity
    /// returns `dir` untouched. WGSL twin: `hdr_world_to_map`.
    #[must_use]
    pub const fn to_map(self, dir: Vec3) -> Vec3 {
        if self.is_identity() {
            return dir;
        }
        Vec3::new(
            self.cos.mul_add(dir.x, -(self.sin * dir.z)),
            dir.y,
            self.cos.mul_add(dir.z, self.sin * dir.x),
        )
    }

    /// A map-frame direction (an importance draw) in the world (a turn by `+yaw` about `+Y`);
    /// the identity returns `dir` untouched. WGSL twin: `hdr_map_to_world`.
    #[must_use]
    pub const fn to_world(self, dir: Vec3) -> Vec3 {
        if self.is_identity() {
            return dir;
        }
        Vec3::new(
            self.cos.mul_add(dir.x, self.sin * dir.z),
            dir.y,
            self.cos.mul_add(dir.z, -(self.sin * dir.x)),
        )
    }

    /// `[cos, sin]`, what the GPU uniforms carry.
    #[must_use]
    pub const fn cos_sin(self) -> [f32; 2] {
        [self.cos, self.sin]
    }

    /// The bits of `[cos, sin]`, for cache keys and bit-for-bit comparisons.
    #[must_use]
    pub const fn bits(self) -> [u32; 2] {
        [self.cos.to_bits(), self.sin.to_bits()]
    }
}

impl Default for EnvRotation {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// How a scene turns its HDR map: a yaw in radians and whether the map follows the camera.
///
/// The default (no yaw, not following) is the unrotated map. Ignored for a studio-lit scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HdrOrientation {
    /// The map's yaw about the vertical axis, in radians (see [`EnvRotation`] for the sense).
    pub yaw: f32,
    /// Whether the camera's yaw is added to [`Self::yaw`], which keeps the map fixed relative
    /// to the camera: orbiting then turns the stone under a fixed room.
    pub follows_camera: bool,
}

impl HdrOrientation {
    /// Whether this differs from the default: a yaw other than zero, or a map that follows the
    /// camera.
    #[must_use]
    pub const fn is_turned(self) -> bool {
        self.yaw != 0.0 || self.follows_camera
    }

    /// The rotation to render with at `camera_yaw` (radians): the map's yaw, plus the camera's
    /// when the map follows it.
    #[must_use]
    pub fn rotation_for(self, camera_yaw: f32) -> EnvRotation {
        let follow = if self.follows_camera { camera_yaw } else { 0.0 };
        EnvRotation::from_yaw(self.yaw + follow)
    }

    /// The same orientation with a finite yaw wrapped into `[0, TAU)`; a NaN or infinite yaw
    /// becomes `0.0`.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let wrapped = if self.yaw.is_finite() {
            self.yaw.rem_euclid(TAU)
        } else {
            0.0
        };
        Self {
            yaw: if wrapped < TAU { wrapped } else { 0.0 },
            follows_camera: self.follows_camera,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::StudioRig;

    /// Directions across the sphere, with both poles and the `u = 0 / 1` seam.
    fn directions() -> Vec<Vec3> {
        let mut dirs = vec![
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
            Vec3::X,
            Vec3::NEG_X,
        ];
        for i in 0..24u8 {
            for j in 0..11u8 {
                let phi = f32::from(i) * (TAU / 24.0);
                let theta = (f32::from(j) + 0.5) * (std::f32::consts::PI / 11.0);
                let (st, ct) = theta.sin_cos();
                let (sp, cp) = phi.sin_cos();
                dirs.push(Vec3::new(st * sp, ct, st * cp));
            }
        }
        dirs
    }

    fn bits3(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    #[test]
    fn degenerate_yaws_are_the_exact_identity() {
        for yaw in [
            0.0,
            -0.0,
            TAU,
            -TAU,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            let r = EnvRotation::from_yaw(yaw);
            assert_eq!(r.bits(), EnvRotation::IDENTITY.bits(), "yaw {yaw}");
            assert!(r.is_identity());
        }
        assert_eq!(EnvRotation::default(), EnvRotation::IDENTITY);
        assert_eq!(EnvRotation::IDENTITY.cos_sin(), [1.0, 0.0]);
    }

    #[test]
    fn identity_returns_every_direction_bit_for_bit() {
        for dir in directions() {
            let id = EnvRotation::IDENTITY;
            assert_eq!(bits3(id.to_map(dir)), bits3(dir));
            assert_eq!(bits3(id.to_world(dir)), bits3(dir));
        }
    }

    #[test]
    fn a_real_turn_is_not_the_identity() {
        let r = EnvRotation::from_yaw(0.7);
        assert!(!r.is_identity());
        let [c, s] = r.cos_sin();
        assert!((c - 0.7f32.cos()).abs() < 1e-7 && (s - 0.7f32.sin()).abs() < 1e-7);
    }

    #[test]
    fn round_trip_is_within_two_ulp_and_keeps_y_exact() {
        for yaw in [0.3f32, 1.0, 2.5, -1.2, 5.9, 100.0] {
            let r = EnvRotation::from_yaw(yaw);
            for dir in directions() {
                let mapped = r.to_map(dir);
                assert_eq!(mapped.y.to_bits(), dir.y.to_bits());
                let back = r.to_world(mapped);
                assert_eq!(back.y.to_bits(), dir.y.to_bits());
                assert!(
                    (back - dir).abs().max_element() <= 4.0 * f32::EPSILON,
                    "yaw {yaw}"
                );
            }
        }
    }

    #[test]
    fn map_yaw_puts_map_plus_z_at_the_key_light_azimuth() {
        for psi in [0.4f32, 1.3, 2.9, -0.8, 4.4] {
            let key = StudioRig::new(psi, 0.0).key_dir;
            let r = EnvRotation::from_yaw(psi);
            assert!(
                (r.to_world(Vec3::Z) - key).abs().max_element() < 1e-6,
                "psi {psi}"
            );
            assert!(
                (r.to_map(key) - Vec3::Z).abs().max_element() < 1e-6,
                "psi {psi}"
            );
        }
    }

    #[test]
    fn rotation_for_adds_the_camera_yaw_only_when_following() {
        let fixed = HdrOrientation {
            yaw: 0.5,
            follows_camera: false,
        };
        let follows = HdrOrientation {
            yaw: 0.5,
            follows_camera: true,
        };
        assert_eq!(fixed.rotation_for(1.25), EnvRotation::from_yaw(0.5));
        assert_eq!(
            follows.rotation_for(1.25),
            EnvRotation::from_yaw(0.5 + 1.25)
        );
        assert!(HdrOrientation::default().rotation_for(2.0).is_identity());
        assert!(!HdrOrientation::default().is_turned());
        assert!(fixed.is_turned() && follows.is_turned());
        assert!(
            HdrOrientation {
                yaw: 0.0,
                follows_camera: true
            }
            .is_turned()
        );
    }

    #[test]
    fn sanitized_wraps_and_cleans_the_yaw() {
        let wrap = |yaw: f32| {
            HdrOrientation {
                yaw,
                follows_camera: true,
            }
            .sanitized()
        };
        assert!((wrap(-1.0).yaw - (TAU - 1.0)).abs() < 1e-6);
        assert_eq!(wrap(f32::NAN).yaw.to_bits(), 0.0f32.to_bits());
        assert_eq!(wrap(f32::INFINITY).yaw.to_bits(), 0.0f32.to_bits());
        assert_eq!(wrap(TAU).yaw.to_bits(), 0.0f32.to_bits());
        assert!(wrap(2.0).follows_camera);
        assert_eq!(wrap(2.0).yaw.to_bits(), 2.0f32.to_bits());
    }

    #[test]
    fn orientation_serde_defaults_and_round_trips() {
        let empty: HdrOrientation = toml::from_str("").expect("an empty table is the default");
        assert_eq!(empty, HdrOrientation::default());
        let turned = HdrOrientation {
            yaw: 1.5,
            follows_camera: true,
        };
        let text = toml::to_string(&turned).expect("serializes");
        let back: HdrOrientation = toml::from_str(&text).expect("parses");
        assert_eq!(back.yaw.to_bits(), turned.yaw.to_bits());
        assert_eq!(back, turned);
    }
}
