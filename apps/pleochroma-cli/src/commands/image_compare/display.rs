//! A `PLRAW1` image as a viewer would see it: the renderer's own tone map, then CIE L*a*b*.
//!
//! The mean XYZ of a pixel is tone-mapped by
//! `pleochroma::renderer::tonemap::tonemap_accumulation`, the function the CLI and app share,
//! into the color space of the written picture. The 8-bit display color is then taken back to
//! linear light and XYZ and turned into L*a*b* (D65), where CIEDE2000 is measured.

use crate::raw_image::RawImage;
use glam::Vec3;
use pleochroma::{
    color::{ColorSpace, xyz_to_lab},
    renderer::tonemap::tonemap_accumulation,
};
use std::num::NonZero;

/// The D65 white the display colors are measured against.
const D65_WHITE: [f64; 3] = [0.950_47, 1.0, 1.088_83];

/// An image of display colors.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DisplayImage {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// CIE L*a*b* of each display color, row-major.
    pub lab: Vec<[f32; 3]>,
    /// The relative luminance (Y) of each display color, row-major.
    pub luminance: Vec<f32>,
}

impl DisplayImage {
    /// The display image of `image` tone-mapped for `space`.
    pub(super) fn from_raw(image: &RawImage, space: ColorSpace) -> Self {
        let width = usize::try_from(image.width).unwrap_or_default();
        let height = usize::try_from(image.height).unwrap_or_default();
        let colors: Vec<Vec3> = image
            .xyz
            .iter()
            .map(|&[x, y, z]| Vec3::new(x, y, z))
            .collect();
        // The values are means already, so the "sum of one sample" is the value itself.
        let rgba = tonemap_accumulation(image.width, image.height, 1, &colors, space);
        let decode = decode_table(space);
        let to_xyz = rgb_to_xyz_matrix(space);
        let pixels = parallel_map(width * height, |index| {
            let code = &rgba[index * 4..index * 4 + 3];
            let linear = [
                decode[usize::from(code[0])],
                decode[usize::from(code[1])],
                decode[usize::from(code[2])],
            ];
            let xyz = multiply(&to_xyz, linear);
            let lab = xyz_to_lab(xyz, D65_WHITE);
            (
                [narrow(lab[0]), narrow(lab[1]), narrow(lab[2])],
                narrow(xyz[1]),
            )
        });
        let (lab, luminance) = pixels.into_iter().unzip();
        Self {
            width,
            height,
            lab,
            luminance,
        }
    }
}

/// `value` in single precision, which is all a display color needs.
#[expect(
    clippy::cast_possible_truncation,
    reason = "display colors are stored in f32; L*a*b* and Y are small numbers"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

/// Linear light of each 8-bit code of `space`'s transfer curve.
fn decode_table(space: ColorSpace) -> [f64; 256] {
    let curve = space.transfer_function();
    std::array::from_fn(|index| {
        let code = u8::try_from(index).unwrap_or(u8::MAX);
        f64::from(curve.decode(f32::from(code) / 255.0))
    })
}

/// The matrix from `space`'s linear RGB to CIE XYZ: the inverse of the renderer's own XYZ to RGB
/// matrix.
fn rgb_to_xyz_matrix(space: ColorSpace) -> [[f64; 3]; 3] {
    let m = space.xyz_to_rgb_matrix().map(|row| row.map(f64::from));
    let cofactors = [
        [
            product_difference(m[1][1], m[2][2], m[1][2], m[2][1]),
            product_difference(m[0][2], m[2][1], m[0][1], m[2][2]),
            product_difference(m[0][1], m[1][2], m[0][2], m[1][1]),
        ],
        [
            product_difference(m[1][2], m[2][0], m[1][0], m[2][2]),
            product_difference(m[0][0], m[2][2], m[0][2], m[2][0]),
            product_difference(m[0][2], m[1][0], m[0][0], m[1][2]),
        ],
        [
            product_difference(m[1][0], m[2][1], m[1][1], m[2][0]),
            product_difference(m[0][1], m[2][0], m[0][0], m[2][1]),
            product_difference(m[0][0], m[1][1], m[0][1], m[1][0]),
        ],
    ];
    let determinant = dot(
        [m[0][0], m[0][1], m[0][2]],
        [cofactors[0][0], cofactors[1][0], cofactors[2][0]],
    );
    cofactors.map(|row| row.map(|value| value / determinant))
}

/// `a * b - c * d`.
fn product_difference(a: f64, b: f64, c: f64, d: f64) -> f64 {
    c.mul_add(-d, a * b)
}

/// The dot product of two 3-vectors.
fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[2].mul_add(right[2], left[1].mul_add(right[1], left[0] * right[0]))
}

/// `matrix` times `vector`.
fn multiply(matrix: &[[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| dot(row, vector))
}

/// `f(0)`, `f(1)`, ... `f(len - 1)`, in that order, computed on several threads.
///
/// `f` is a pure function of its index, so the result does not depend on the thread count.
pub(super) fn parallel_map<T, F>(len: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    let threads = std::thread::available_parallelism().map_or(1, NonZero::get);
    let chunk = len.div_ceil(threads).max(1);
    let mut out: Vec<T> = Vec::with_capacity(len);
    std::thread::scope(|scope| {
        let f = &f;
        let workers: Vec<_> = (0..len)
            .step_by(chunk)
            .map(|start| {
                let end = (start + chunk).min(len);
                scope.spawn(move || (start..end).map(f).collect::<Vec<T>>())
            })
            .collect();
        for worker in workers {
            match worker.join() {
                Ok(part) => out.extend(part),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    });
    out
}
