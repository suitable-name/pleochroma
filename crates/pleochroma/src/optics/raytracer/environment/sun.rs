//! The direct sun of [`LightingModel::DaylightSun`](super::LightingModel::DaylightSun):
//! disc constants, radiance factor and the cone sampler / pdf for next-event estimation.

use super::rig::horizon_blend;
use glam::Vec3;

// ---------------------------------------------------------------------------------
// The direct sun of `LightingModel::DaylightSun` (GPU id 5).
//
// Geometry. A disc of angular radius 0.27 degrees about the key direction. `SUN_DISC_COS`
// is cos(0.27 deg) rounded to f32, which is `1 - 186 * 2^-24`, so the solid angle is derived
// from that same literal: `SUN_SOLID_ANGLE = 2 pi * SUN_ONE_MINUS_COS`. The consumers read
// the literals, so the 0.16 % gap to the exact pi r^2 is irrelevant.
//
// Radiance. The sun is set so that it delivers about 82 % of the horizontal irradiance at the
// default key elevation of 72 degrees. The sky there (`DaylightDome`'s sky plus aureole) gives
// E_sky = 0.577 in model units, and the sun gives E_sun = L * SUN_SOLID_ANGLE * sin(e) = 2.65
// at L = 40 000, a direct share of 82.1 %. The share follows sin(e): 74 % at 30 degrees and
// 83 % at 90 degrees.
//
// The head shadow does not dim the sun: it models the observer's body blocking the sky dome,
// and the NEE draw has no observer. The horizon blend at the key direction does fade it
// (`sun_radiance_factor`), so a key below the horizon switches the sun off.
//
// Spectrum: D65. The sun's own 5500-5800 K spectrum would need its own `spec_power` input in
// the WGSL twins.
// ---------------------------------------------------------------------------------

/// Cosine of the sun disc's angular radius (0.27 degrees), as an f32 literal.
pub const SUN_DISC_COS: f32 = 0.999_988_9;
/// `1 - SUN_DISC_COS`, exactly (`186 * 2^-24`), kept as its own literal so the cone sampler
/// never subtracts two nearly equal numbers.
pub(super) const SUN_ONE_MINUS_COS: f32 = 1.108_646_4e-5;
/// Solid angle of the sun disc in steradians: `2 pi * SUN_ONE_MINUS_COS`.
pub(super) const SUN_SOLID_ANGLE: f32 = 6.965_831e-5;
/// Radiance of the sun disc in the dome's radiance units (see the derivation above).
pub const SUN_RADIANCE: f32 = 40_000.0;

/// The sun's wavelength-independent radiance factor: [`SUN_RADIANCE`] faded by the horizon
/// blend at the key direction. The disc is uniform, so this one number is the sun's radiance
/// for every direction inside it, for the BSDF-sampled lookup and the NEE draw alike.
pub(super) fn sun_radiance_factor(key_dir: Vec3) -> f32 {
    SUN_RADIANCE * horizon_blend(key_dir)
}

/// A direction drawn uniformly over the sun disc about `key_dir` from two uniform `[0, 1)`
/// randoms (equal-area cone sampling: `1 - cos(theta) = u0 * (1 - cos(theta_max))`).
/// `sin(theta)` comes from `(1 - cos)(1 + cos)` with `1 - cos` kept exact, so the radial
/// resolution is not quantised by the f32 spacing of the cosine near 1. The WGSL twin
/// `daylight_sun_cone_direction` is operation for operation the same.
pub(super) fn sun_cone_direction(key_dir: Vec3, u0: f32, u1: f32) -> Vec3 {
    let one_minus_cos = u0 * SUN_ONE_MINUS_COS;
    let cos_t = 1.0 - one_minus_cos;
    let sin_t = (one_minus_cos * (2.0 - one_minus_cos)).max(0.0).sqrt();
    let phi = 2.0 * std::f32::consts::PI * u1;
    let (sin_p, cos_p) = phi.sin_cos();
    let a = if key_dir.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t = (a - key_dir * key_dir.dot(a)).normalize_or_zero();
    let b = key_dir.cross(t);
    (t * (sin_t * cos_p) + b * (sin_t * sin_p) + key_dir * cos_t).normalize_or_zero()
}

/// The solid-angle pdf of [`sun_cone_direction`] at `dir`: `1 / SUN_SOLID_ANGLE` inside the
/// disc, `0.0` outside. `dir` must be a unit vector.
pub(super) fn sun_nee_pdf(key_dir: Vec3, dir: Vec3) -> f32 {
    if dir.dot(key_dir) >= SUN_DISC_COS {
        1.0 / SUN_SOLID_ANGLE
    } else {
        0.0
    }
}
