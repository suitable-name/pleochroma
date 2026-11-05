//! Space-aware gamut mapping.
//!
//! Compresses out-of-gamut CIE XYZ colors into a target [`ColorSpace`] by walking the
//! chromaticity radially toward that space's white point at constant luminance, rather
//! than clipping each RGB channel independently (which shifts hue as well as
//! saturation).
//!
//! Generalises `optics::raytracer::xyz_to_srgb_gamma`'s gamut-mapping step to any
//! [`ColorSpace`]; see `tests/color_tests.rs` for the cross-check.

use glam::Vec3;

use super::space::ColorSpace;

/// Radially compresses an out-of-gamut color toward `space`'s white point.
///
/// Walks the CIE xyY chromaticity of `xyz` toward `space`'s white point at constant
/// luminance until every RGB channel from `space`'s XYZ->RGB matrix
/// ([`ColorSpace::xyz_to_linear`]) is non-negative; in-gamut colors pass through
/// unchanged. Result is linear RGB -- apply [`ColorSpace::transfer_function`] or use
/// [`ColorSpace::encode`] for the full pipeline. Non-finite or near-zero `xyz`
/// (component sum `<= 1e-6`) maps to `Vec3::ZERO` rather than dividing by near-zero.
///
/// Thin wrapper around [`project_to_gamut_bounded`] with `max = f32::INFINITY`.
#[must_use]
pub fn project_to_gamut(xyz: Vec3, space: ColorSpace) -> Vec3 {
    project_to_gamut_bounded(xyz, space, f32::INFINITY)
}

/// As [`project_to_gamut`], but also caps every output channel at `max`.
///
/// `ColorSpace::encode`'s ACES tone mapping scales RGB by one luminance-derived
/// factor so a saturated color's hue survives (see
/// [`crate::color::ToneMap::AcesFilmic`]); hard-clamping a channel above `1.0`
/// per channel during quantization would reintroduce that hue shift. This
/// bounded walk desaturates an over-bright color toward white instead.
///
/// Reuses [`project_to_gamut`]'s walk with both a floor and ceiling per channel.
/// The white-point fallback step is not itself clamped to `max`: every channel
/// there is equal by construction, so a small overshoot (e.g. ACES's ~1.033
/// highlight asymptote) is hue-neutral and left for `encode`'s final clamp.
#[must_use]
pub fn project_to_gamut_bounded(xyz: Vec3, space: ColorSpace, max: f32) -> Vec3 {
    const STEPS: u32 = 32;

    let sum = xyz.x + xyz.y + xyz.z;
    if !sum.is_finite() || sum <= 1e-6 {
        return Vec3::ZERO;
    }

    let x = xyz.x / sum;
    let y = xyz.y / sum;
    let luminance = xyz.y.max(0.0);

    let linear = space.xyz_to_linear(xyz);
    let in_range =
        |v: Vec3| v.x >= 0.0 && v.y >= 0.0 && v.z >= 0.0 && v.x <= max && v.y <= max && v.z <= max;
    if in_range(linear) {
        return linear;
    }

    // Walk toward the white point, taking the smallest step in range.
    let (white_x, white_y) = space.white_point_xy();

    let mapped_xyz_for_t = |t: f32| -> Vec3 {
        let xp = (white_x - x).mul_add(t, x);
        let yp = (white_y - y).mul_add(t, y);
        if yp > 1e-6 {
            Vec3::new(
                (xp / yp) * luminance,
                luminance,
                ((1.0 - xp - yp) / yp) * luminance,
            )
        } else {
            Vec3::new(
                (white_x / white_y) * luminance,
                luminance,
                ((1.0 - white_x - white_y) / white_y) * luminance,
            )
        }
    };

    let mut resolved = space.xyz_to_linear(mapped_xyz_for_t(1.0)).max(Vec3::ZERO);
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let candidate = space.xyz_to_linear(mapped_xyz_for_t(t));
        if in_range(candidate) {
            resolved = candidate;
            break;
        }
    }
    resolved
}

/// Tone-mapped luminance from which an over-bright color starts to blend toward white under
/// [`fit_keeping_color`].
///
/// The ACES curve maps a scene luminance of 1.0 (diffuse white) to about 0.80, so only light
/// brighter than a lit white surface (specular highlights, the bright tray seen directly)
/// whitens; the body color a stone shows stays below it.
pub const KEEP_COLOR_KNEE: f32 = 0.8;

/// Tone-mapped luminance at and above which [`fit_keeping_color`] keeps the luminance and
/// whitens fully, as [`project_to_gamut_bounded`] does.
pub const KEEP_COLOR_WHITE: f32 = 1.0;

/// Brings a tone-mapped color whose largest channel exceeds `1.0` into `[0, 1]` while keeping
/// its hue and saturation.
///
/// An out-of-gamut chromaticity (a negative channel) is walked toward white first, at constant
/// luminance, exactly as [`project_to_gamut`] does: that is the space's gamut, not brightness.
/// A color that is then still too bright is dimmed until its largest channel is `1.0`, keeping
/// its chromaticity, so a saturated red stays red instead of turning pink. Above
/// [`KEEP_COLOR_KNEE`] the result blends smoothly (smoothstep) toward the luminance-keeping
/// rule, and from [`KEEP_COLOR_WHITE`] on the color keeps its luminance and mixes with the grey
/// of that luminance just enough to fit, the same straight path toward white as
/// [`project_to_gamut_bounded`] (solved exactly instead of in 32 steps).
///
/// The luminance is `xyz.y`. The grey of luminance `L` is `(L, L, L)`: every [`ColorSpace`]
/// maps its own white to linear `(1, 1, 1)` at `Y = 1`.
#[must_use]
pub fn fit_keeping_color(xyz: Vec3, space: ColorSpace) -> Vec3 {
    let rgb = project_to_gamut(xyz, space);
    let peak = rgb.max_element();
    if peak <= 1.0 {
        return rgb;
    }
    let luminance = xyz.y.max(0.0);
    let t = ((luminance - KEEP_COLOR_KNEE) / (KEEP_COLOR_WHITE - KEEP_COLOR_KNEE)).clamp(0.0, 1.0);
    let whiten = t * t * 2.0f32.mul_add(-t, 3.0);
    // 1 / peak keeps the color (the largest channel lands on 1.0); 1.0 keeps the luminance.
    let fit = 1.0 / peak;
    let scale = whiten.mul_add(1.0 - fit, fit);
    let dimmed = rgb * scale;
    let level = luminance * scale;
    let top = dimmed.max_element();
    if top <= 1.0 {
        return dimmed;
    }
    if level >= 1.0 {
        return Vec3::splat(level);
    }
    // The share of grey that brings the largest channel down to 1.0 exactly.
    let mix = (top - 1.0) / (top - level);
    dimmed + (Vec3::splat(level) - dimmed) * mix
}

/// sRGB-specialised convenience wrapper around [`project_to_gamut`].
///
/// Named entry point for callers that only need sRGB, rather than plumbing
/// `ColorSpace::Srgb` through everywhere.
#[must_use]
pub fn project_to_srgb(xyz: Vec3) -> Vec3 {
    project_to_gamut(xyz, ColorSpace::Srgb)
}
