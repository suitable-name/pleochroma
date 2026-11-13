//! The pictures and numbers behind the color picker: a plane of lightness and chroma at one hue,
//! the hue strip, the gamut test and the color difference.
//!
//! The conversions are `colors::lch_to_rgb` (which clamps into sRGB) and
//! `pleochroma_editor::lch_color::lch_from_srgb`; this module only adds what the picker draws on
//! top of them.

use super::{
    colors::{Rgb, lch_to_rgb},
    cross_section::{mix, to_f64},
};
use crate::gui::rough_colour::wizard::raster::Rgba;
use pleochroma::color::body_color::{lch_to_lab, srgb_to_lab};
use pleochroma_editor::lch_color::lch_from_srgb;

/// The chroma at the right edge of the picker plane.
pub(super) const PLANE_MAX_CHROMA: f64 = 150.0;
/// The lightness of the hue strip.
const STRIP_LIGHTNESS: f64 = 62.0;
/// The chroma of the hue strip.
const STRIP_CHROMA: f64 = 48.0;
/// How far (ΔE) a color may sit from its own sRGB rendering and still count as inside sRGB: the
/// rounding of the three bytes.
const GAMUT_TOLERANCE: f64 = 1.5;
/// The color the out-of-gamut part of the plane fades toward (the window's picture background).
const DIMMED: Rgb = [0x12, 0x14, 0x1c];
/// How strongly the out-of-gamut part of the plane fades toward [`DIMMED`].
const DIM: f32 = 0.7;

/// The `[L*, C*, h]` of an sRGB color (D65), clamped to L* 0..100 and C* 0..130.
pub(super) fn rgb_to_lch(rgb: [u8; 3]) -> [f64; 3] {
    lch_from_srgb(rgb.map(|channel| f64::from(channel) / 255.0))
}

/// The CIE76 color difference between two `[L*, C*, h]` colors: the distance of their L*a*b*
/// points.
pub(super) fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    lab_distance(lch_to_lab(a), lch_to_lab(b))
}

/// The distance of two L*a*b* points.
fn lab_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

/// Whether `rgb`, the clamped rendering of `lch` (`colors::lch_to_rgb`), is still that color: it
/// is when `lch` lies inside sRGB.
fn rendering_matches(lch: [f64; 3], rgb: Rgb) -> bool {
    let back = srgb_to_lab(rgb.map(|channel| f64::from(channel) / 255.0));
    lab_distance(lch_to_lab(lch), back) <= GAMUT_TOLERANCE
}

/// Whether `[L*, C*, h]` lies inside the sRGB gamut (D65), within the rounding of a byte.
pub(super) fn in_srgb_gamut(lch: [f64; 3]) -> bool {
    rendering_matches(lch, lch_to_rgb(lch))
}

/// The picker plane at hue `hue_deg`, `width` x `height` pixels: chroma grows to the right from 0
/// to [`PLANE_MAX_CHROMA`], lightness falls from 100 at the top to 0 at the bottom. The part
/// outside sRGB shows the nearest sRGB color faded toward the background, so the reachable area
/// reads as the bright one.
pub(super) fn picker_plane(hue_deg: f64, width: usize, height: usize) -> Rgba {
    let mut image = Rgba::filled(width, height, [DIMMED[0], DIMMED[1], DIMMED[2], 255]);
    for py in 0..height {
        let lightness = 100.0 * (1.0 - (to_f64(py) + 0.5) / to_f64(height));
        for px in 0..width {
            let chroma = PLANE_MAX_CHROMA * (to_f64(px) + 0.5) / to_f64(width);
            let lch = [lightness, chroma, hue_deg];
            let rgb = lch_to_rgb(lch);
            let shown = if rendering_matches(lch, rgb) {
                rgb
            } else {
                mix(rgb, DIMMED, DIM)
            };
            image.set(px, py, [shown[0], shown[1], shown[2], 255]);
        }
    }
    image
}

/// The hue strip, `width` x `height` pixels: hue 0 to 360 degrees from left to right at a
/// lightness and chroma ([`STRIP_LIGHTNESS`], [`STRIP_CHROMA`]) every hue can show.
pub(super) fn hue_strip(width: usize, height: usize) -> Rgba {
    let mut image = Rgba::filled(width, height, [DIMMED[0], DIMMED[1], DIMMED[2], 255]);
    for px in 0..width {
        let hue = 360.0 * (to_f64(px) + 0.5) / to_f64(width);
        let rgb = lch_to_rgb([STRIP_LIGHTNESS, STRIP_CHROMA, hue]);
        for py in 0..height {
            image.set(px, py, [rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_color_has_no_difference_from_itself_and_a_known_one_from_white() {
        let color = [55.0, 80.0, 40.0];
        assert!(delta_e(color, color).abs() < 1e-12);
        // Same hue and chroma 0, so the difference is the lightness step.
        assert!((delta_e([60.0, 0.0, 0.0], [50.0, 0.0, 0.0]) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn the_gamut_test_separates_reachable_colors_from_impossible_ones() {
        assert!(in_srgb_gamut([50.0, 0.0, 0.0]));
        assert!(in_srgb_gamut([55.0, 80.0, 40.0]));
        assert!(in_srgb_gamut([100.0, 0.0, 0.0]));
        assert!(!in_srgb_gamut([50.0, 150.0, 40.0]));
        assert!(!in_srgb_gamut([100.0, 100.0, 200.0]));
    }

    #[test]
    fn an_srgb_color_comes_back_from_the_round_trip() {
        let lch = [55.0, 80.0, 40.0];
        let back = rgb_to_lch(lch_to_rgb(lch));
        assert!(delta_e(lch, back) < 2.0, "{back:?}");
    }

    #[test]
    fn the_plane_dims_what_the_screen_cannot_show() {
        let plane = picker_plane(354.0, 150, 100);
        assert_eq!((plane.width, plane.height), (150, 100));
        let brightest = |p: [u8; 4]| p[0].max(p[1]).max(p[2]);
        // Top left: white (L 100, chroma 0), in gamut. Top right: L 100 with chroma 150, far out.
        assert!(brightest(plane.pixel(0, 0)) > 240);
        assert!(brightest(plane.pixel(149, 0)) < 120);
        // Bottom left: black, in gamut and opaque.
        assert_eq!(plane.pixel(0, 99)[3], 255);
        assert!(brightest(plane.pixel(0, 99)) < 10);
    }

    #[test]
    fn the_hue_strip_runs_through_the_hues() {
        let strip = hue_strip(120, 8);
        assert_eq!((strip.width, strip.height), (120, 8));
        let red = strip.pixel(1, 0);
        // Pixel 46 is hue 139 degrees, pixel 80 hue 239.
        let green = strip.pixel(46, 0);
        let blue = strip.pixel(80, 0);
        assert!(red[0] > red[1] && red[0] > red[2], "{red:?}");
        assert!(green[1] > green[0] && green[1] > green[2], "{green:?}");
        assert!(blue[2] > blue[0], "{blue:?}");
        assert_eq!(strip.pixel(60, 7), strip.pixel(60, 0));
    }
}
