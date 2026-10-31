//! Debug screenshots: a window picture scaled to at most 1920 logical pixels wide, with the
//! pointer actions of a step drawn on it (a ring at a click, a cross at the end of a move, a
//! square at a wheel turn, a dot where the cursor stood when the step began) and a short label
//! (`C1`, `H1`, `W1`, `S`) that matches the step log.

use image::{
    Rgba, RgbaImage,
    imageops::{self, FilterType},
};
use std::{fs, path::Path};
use uiprobe::screenshot::Shot;

/// The widest picture written, in logical pixels (the video window's width).
const MAX_WIDTH: u32 = 1920;
/// The size of a mark's shape, in output pixels (its radius or half side).
const MARK_RADIUS: i32 = 14;
/// How many output pixels one font pixel covers.
const FONT_SCALE: i32 = 2;
/// The glyphs of the labels: five columns, seven rows, one bit per column (bit 4 is the left).
const GLYPHS: [(char, [u8; 7]); 14] = [
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    ('S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
];

/// What a mark looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A click.
    Ring,
    /// The end point of a move (a hover).
    Cross,
    /// A wheel turn.
    Square,
    /// Where the cursor stood before the step acted.
    Dot,
}

impl Shape {
    const fn color(self) -> Rgba<u8> {
        match self {
            Self::Ring => Rgba([230, 30, 30, 255]),
            Self::Cross => Rgba([30, 110, 240, 255]),
            Self::Square => Rgba([240, 150, 0, 255]),
            Self::Dot => Rgba([20, 170, 70, 255]),
        }
    }
}

/// One drawn mark, at a point of its window in logical pixels.
#[derive(Clone, Debug)]
pub struct Mark {
    /// The log label (`C1`).
    pub label: String,
    /// The window the point belongs to (`main`, `help`, ...).
    pub window: String,
    pub shape: Shape,
    pub x: f32,
    pub y: f32,
}

/// Writes `shot` as a PNG at `path`, scaled to at most 1920 logical pixels wide, with `marks`
/// (already filtered to this window) drawn on it.
///
/// # Errors
///
/// A picture that cannot be built or written. The message carries no path.
pub fn write(shot: &Shot, marks: &[Mark], path: &Path) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(shot.pixels().len() * 4);
    for pixel in shot.pixels() {
        bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b, 255]);
    }
    let full = RgbaImage::from_raw(shot.width(), shot.height(), bytes)
        .ok_or_else(|| "the captured picture has an unexpected size".to_owned())?;
    let scale = shot.scale().max(0.1);
    let logical_width = shot.width() as f32 / scale;
    let target_width = whole(logical_width.min(MAX_WIDTH as f32)).max(1);
    let mut image = if target_width < shot.width() {
        let target_height = whole(shot.height() as f32 * target_width as f32 / shot.width() as f32);
        imageops::resize(
            &full,
            target_width,
            target_height.max(1),
            FilterType::Triangle,
        )
    } else {
        full
    };
    // Output pixels per logical pixel.
    let factor = image.width() as f32 / logical_width.max(1.0);
    for mark in marks {
        draw_mark(&mut image, mark, factor);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "could not create the debug folder".to_owned())?;
    }
    image
        .save(path)
        .map_err(|_| "could not write a debug screenshot".to_owned())
}

/// A non-negative size as whole pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the picture sizes first"
)]
const fn whole(value: f32) -> u32 {
    value.round().clamp(0.0, 65_535.0) as u32
}

/// A coordinate as a signed pixel.
#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to a range far inside i32 first"
)]
const fn signed(value: f32) -> i32 {
    value.round().clamp(-100_000.0, 100_000.0) as i32
}

fn put(image: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>) {
    if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y))
        && x < image.width()
        && y < image.height()
    {
        image.put_pixel(x, y, color);
    }
}

fn draw_mark(image: &mut RgbaImage, mark: &Mark, factor: f32) {
    let (cx, cy) = (signed(mark.x * factor), signed(mark.y * factor));
    let white = Rgba([255, 255, 255, 255]);
    let color = mark.shape.color();
    let reach = MARK_RADIUS + 4;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let distance = (dx as f32).hypot(dy as f32);
            let radius = MARK_RADIUS as f32;
            let (inner, outer) = match mark.shape {
                Shape::Ring => (distance >= radius - 1.5 && distance <= radius + 1.5, true),
                Shape::Cross => (
                    (dx.abs() <= 1 && dy.abs() <= MARK_RADIUS)
                        || (dy.abs() <= 1 && dx.abs() <= MARK_RADIUS),
                    dx.abs() <= 2 || dy.abs() <= 2,
                ),
                Shape::Square => {
                    let edge = dx.abs().max(dy.abs());
                    (
                        (MARK_RADIUS - 1..=MARK_RADIUS + 1).contains(&edge),
                        edge <= MARK_RADIUS + 3,
                    )
                }
                Shape::Dot => (distance <= 5.0, distance <= 7.0),
            };
            let halo = match mark.shape {
                Shape::Ring => distance >= radius - 3.0 && distance <= radius + 3.0,
                Shape::Cross => {
                    outer
                        && ((dx.abs() <= 2 && dy.abs() <= MARK_RADIUS + 1)
                            || (dy.abs() <= 2 && dx.abs() <= MARK_RADIUS + 1))
                }
                Shape::Square | Shape::Dot => outer,
            };
            if inner {
                put(image, cx + dx, cy + dy, color);
            } else if halo {
                put(image, cx + dx, cy + dy, white);
            }
        }
    }
    draw_label(
        image,
        &mark.label,
        cx + MARK_RADIUS + 3,
        cy - MARK_RADIUS - 18,
    );
}

/// The label in white on a black box, kept inside the picture.
fn draw_label(image: &mut RgbaImage, text: &str, left: i32, top: i32) {
    let count = i32::try_from(text.chars().count()).unwrap_or(0);
    let width = count * 6 * FONT_SCALE + 4;
    let height = 7 * FONT_SCALE + 4;
    let max_left = i32::try_from(image.width()).unwrap_or(0) - width;
    let max_top = i32::try_from(image.height()).unwrap_or(0) - height;
    let left = left.clamp(0, max_left.max(0));
    let top = top.clamp(0, max_top.max(0));
    for y in 0..height {
        for x in 0..width {
            put(image, left + x, top + y, Rgba([0, 0, 0, 255]));
        }
    }
    for (index, character) in text.chars().enumerate() {
        let Some((_, rows)) = GLYPHS.iter().find(|(glyph, _)| *glyph == character) else {
            continue;
        };
        let origin = left + 2 + i32::try_from(index).unwrap_or(0) * 6 * FONT_SCALE;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..5 {
                if (bits >> (4 - column)) & 1 == 1 {
                    for sy in 0..FONT_SCALE {
                        for sx in 0..FONT_SCALE {
                            put(
                                image,
                                origin + column * FONT_SCALE + sx,
                                top + 2 + i32::try_from(row).unwrap_or(0) * FONT_SCALE + sy,
                                Rgba([255, 255, 255, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
}
