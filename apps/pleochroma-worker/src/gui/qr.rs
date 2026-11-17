//! Draws a pairing link as a QR code image.
//!
//! Black modules on white with the four-module quiet zone the standard asks for, at a whole
//! number of pixels per module so every edge is crisp (the dialog shows the image with
//! nearest-neighbor scaling). Error correction level M: a link of about 217 characters fits
//! version 11 (61 modules a side), which a phone camera reads comfortably from a screen.
//!
//! The text is a pairing link and so a secret: nothing here logs it, and no type here
//! implements `Debug`.

use qrcode::{Color, EcLevel, QrCode};

/// The quiet zone around the code, in modules (the standard asks for four).
const QUIET_MODULES: usize = 4;

/// The size the picture aims for, in pixels. It is twice the size the dialog shows it at, so a
/// screen at a scale factor of two still maps one image pixel to one screen pixel.
const TARGET_PIXELS: usize = 552;

/// A QR code as a square RGBA bitmap.
pub struct QrBitmap {
    /// The side of the square, in pixels.
    pub side: u32,
    /// `side * side * 4` bytes, row by row: opaque black and opaque white.
    pub rgba: Vec<u8>,
    /// The number of modules per side, without the quiet zone.
    pub modules: u32,
}

/// Renders `text` as a QR code at error correction level M.
///
/// Returns `None` when the text does not fit the largest code.
#[must_use]
pub fn render(text: &str) -> Option<QrBitmap> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let total = modules + 2 * QUIET_MODULES;
    let scale = (TARGET_PIXELS / total).max(1);
    let side = total * scale;
    let mut rgba = vec![255_u8; side * side * 4];
    for my in 0..modules {
        for mx in 0..modules {
            if colors[my * modules + mx] != Color::Dark {
                continue;
            }
            let x0 = (mx + QUIET_MODULES) * scale;
            let y0 = (my + QUIET_MODULES) * scale;
            for y in y0..y0 + scale {
                let row = y * side * 4;
                for x in x0..x0 + scale {
                    let at = row + x * 4;
                    rgba[at..at + 3].fill(0);
                }
            }
        }
    }
    Some(QrBitmap {
        side: u32::try_from(side).ok()?,
        rgba,
        modules: u32::try_from(modules).ok()?,
    })
}

/// A Slint image of the QR code for `text`, or `None` when it does not fit.
#[must_use]
pub fn image(text: &str) -> Option<slint::Image> {
    let bitmap = render(text)?;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(bitmap.side, bitmap.side);
    buffer.make_mut_bytes().copy_from_slice(&bitmap.rgba);
    Some(slint::Image::from_rgba8(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link of the size the coordinator makes: three addresses and a 100-character code.
    ///
    /// The code is mixed case like a real token (`GW1-AbCdEf-...`). A run of capitals alone
    /// would let the encoder switch to its denser alphanumeric mode, a smaller code than any
    /// real link gets.
    fn sample_link() -> String {
        let mut link = String::from(
            "pleochroma://pair?v=1&r=viewer&n=Studio+PC&a=192.168.178.20:7878&a=10.0.0.5:7878&a=172.16.1.9:7878&e=7879&t=GW1-",
        );
        for c in "aB3x".chars().cycle() {
            if link.len() >= 217 {
                break;
            }
            link.push(c);
        }
        link
    }

    #[test]
    fn a_link_of_217_characters_is_version_11() {
        let link = sample_link();
        assert_eq!(link.len(), 217);
        let bitmap = render(&link).expect("fits");
        assert_eq!(bitmap.modules, 61);
    }

    #[test]
    fn the_bitmap_is_square_whole_scaled_with_a_white_quiet_zone() {
        let bitmap = render(&sample_link()).expect("fits");
        let side = bitmap.side as usize;
        assert_eq!(bitmap.rgba.len(), side * side * 4);
        let total = bitmap.modules as usize + 2 * QUIET_MODULES;
        assert_eq!(side % total, 0);
        let scale = side / total;
        assert!(scale >= 1);
        // The whole first four module rows and columns are white.
        let quiet = QUIET_MODULES * scale;
        for y in 0..side {
            for x in 0..side {
                if x < quiet || y < quiet || x >= side - quiet || y >= side - quiet {
                    assert_eq!(
                        &bitmap.rgba[(y * side + x) * 4..][..4],
                        &[255, 255, 255, 255]
                    );
                }
            }
        }
        // The top-left finder pattern starts with a dark module.
        let first = (quiet * side + quiet) * 4;
        assert_eq!(&bitmap.rgba[first..first + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn only_black_and_white_are_used() {
        let bitmap = render("pleochroma://pair?v=1").expect("fits");
        for pixel in bitmap.rgba.as_chunks::<4>().0 {
            assert!(*pixel == [0, 0, 0, 255] || *pixel == [255, 255, 255, 255]);
        }
    }

    #[test]
    fn text_past_the_largest_code_gives_none() {
        let too_long = "A".repeat(8000);
        assert!(render(&too_long).is_none());
    }
}
