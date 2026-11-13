//! Tone-map and PNG/ICC output: turning the finished accumulation buffer into RGBA
//! bytes (sRGB or wide-gamut) and writing it to disk.
//!
//! The tone-mapping ([`tonemap_accumulation`]) lives in `pleochroma::renderer::tonemap`, so a
//! server answering a final-picture request runs the same code and its PNG is byte-identical
//! to a local export. The PNG+ICC encoding ([`pleochroma::render_setup::encode_png_with_icc`])
//! is in-memory and also lives in `pleochroma`. [`save_png`] only writes the result to disk.

pub use pleochroma::renderer::tonemap::tonemap_accumulation;
use pleochroma::{color::ColorSpace, render_setup::encode_png_with_icc};
use std::path::Path;

/// Writes `rgba` (`width * height * 4` bytes) to `path` as PNG, embedding an ICC
/// profile for any non-`Srgb` `color_space` -- see
/// [`pleochroma::render_setup::encode_png_with_icc`] for the encoding itself.
pub(super) fn save_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
    color_space: ColorSpace,
) -> Result<(), String> {
    let bytes = encode_png_with_icc(rgba, width, height, color_space)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}
