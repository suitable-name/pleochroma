//! The color of the delivered render's border, for `ViewportModel.render_backdrop_color`.
//!
//! The Live Render view paints its background in this color, so a letterboxed picture has no
//! bars beside it. Only a few dozen pixels along the four edges are read (never the whole
//! frame), once per delivered frame.

use crate::{MainWindow, ViewportModel};
use slint::ComponentHandle;

/// Samples taken along each of the four edges.
const SAMPLES_PER_EDGE: usize = 16;

/// The per-channel median of pixels sampled along the four edges of an RGBA8 frame
/// (`width * height * 4` bytes), or `None` for an empty frame or a short buffer. Alpha is
/// ignored.
#[must_use]
pub fn edge_color(rgba: &[u8], width: usize, height: usize) -> Option<[u8; 3]> {
    if width == 0 || height == 0 || rgba.len() < width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let mut channels: [Vec<u8>; 3] =
        std::array::from_fn(|_| Vec::with_capacity(4 * SAMPLES_PER_EDGE));
    let mut take = |x: usize, y: usize| {
        let at = (y * width + x) * 4;
        for (channel, value) in channels.iter_mut().zip(&rgba[at..at + 3]) {
            channel.push(*value);
        }
    };
    for i in 0..SAMPLES_PER_EDGE {
        // Evenly spread along each edge, ends included.
        let x = i * (width - 1) / (SAMPLES_PER_EDGE - 1);
        let y = i * (height - 1) / (SAMPLES_PER_EDGE - 1);
        take(x, 0);
        take(x, height - 1);
        take(0, y);
        take(width - 1, y);
    }
    Some(channels.map(|mut values| {
        values.sort_unstable();
        values[values.len() / 2]
    }))
}

/// Sets `ViewportModel.render_backdrop_color` from the edges of the frame just delivered.
/// Leaves the color as it was when the frame cannot be read.
pub fn push_backdrop(ui: &MainWindow, rgba: &[u8], width: u32, height: u32) {
    let (Ok(width), Ok(height)) = (usize::try_from(width), usize::try_from(height)) else {
        return;
    };
    if let Some([r, g, b]) = edge_color(rgba, width, height) {
        ui.global::<ViewportModel>()
            .set_render_backdrop_color(slint::Color::from_rgb_u8(r, g, b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: usize, height: usize, pixel: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = pixel(x, y);
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        out
    }

    #[test]
    fn a_uniform_border_gives_its_color() {
        let rgba = frame(40, 30, |x, y| {
            if x == 0 || y == 0 || x == 39 || y == 29 {
                [10, 20, 30]
            } else {
                [200, 200, 200]
            }
        });
        assert_eq!(edge_color(&rgba, 40, 30), Some([10, 20, 30]));
    }

    #[test]
    fn the_interior_is_not_read() {
        let rgba = frame(64, 64, |x, y| {
            if (1..63).contains(&x) && (1..63).contains(&y) {
                [255, 0, 0]
            } else {
                [8, 9, 10]
            }
        });
        assert_eq!(edge_color(&rgba, 64, 64), Some([8, 9, 10]));
    }

    #[test]
    fn a_few_stray_edge_pixels_do_not_move_the_median() {
        let rgba = frame(100, 100, |x, y| {
            if y == 0 && x < 5 {
                [255, 255, 255]
            } else {
                [30, 30, 30]
            }
        });
        assert_eq!(edge_color(&rgba, 100, 100), Some([30, 30, 30]));
    }

    #[test]
    fn tiny_and_broken_frames_are_handled() {
        assert_eq!(edge_color(&[1, 2, 3, 255], 1, 1), Some([1, 2, 3]));
        assert_eq!(edge_color(&[], 0, 0), None);
        assert_eq!(edge_color(&[0; 8], 4, 4), None);
    }
}
