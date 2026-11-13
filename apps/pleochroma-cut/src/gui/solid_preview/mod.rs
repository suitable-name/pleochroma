//! Solid inspection preview: a flat-shaded, edge-outlined CPU render of the current
//! design's [`pleochroma::geometry::stone_metrics::SolidMesh`], independent of the
//! GPU path tracer.
//!
//! [`raster`], [`diagram2d`], [`facet_map`], [`mesh_cache`], [`edges_layer`] and
//! [`live_update`] are re-exported from the shared [`pleochroma_solid`] crate, which also
//! serves the wasm web app; see that crate for what each one does.
//!
//! [`to_pixel_buffer`] is the one bridge into Slint types. It stays here, not in the shared
//! crate, so that crate has no Slint dependency.
//!
//! [`preview_state`] is the editor-side controller: one dedicated worker thread
//! owning the mesh cache and rasterizer, coalescing redraw requests via
//! `RedrawGate`, handing finished frames to a [`preview_state::PreviewSink`] kept
//! generic over Slint rather than depending on `MainWindow` directly. It (plus
//! [`diagram_wiring`], the Slint callback wiring) stays in this app: it owns
//! threads and Slint types, neither of which the shared crate may depend on.
//!
//! [`cut_slider`] is the Cut slider's logic -- where it stands, its label, and the
//! truncated geometry every full redraw path draws for it.

pub use pleochroma_solid::{diagram2d, edges_layer, facet_map, live_update, mesh_cache, raster};

pub mod preview_state;

pub mod diagram_wiring;

pub mod facet_selection;

pub mod cut_slider;

/// Converts a finished [`raster::SolidRasterizer`] frame into a `Send`-safe pixel
/// buffer for [`preview_state::PreviewSink::apply`] to hand to the UI thread.
///
/// Returns a `SharedPixelBuffer` rather than a `slint::Image`, which is not `Send`: the
/// image is built with `slint::Image::from_rgba8` on the UI thread only.
#[must_use]
pub fn to_pixel_buffer(
    rasterizer: &raster::SolidRasterizer,
) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(rasterizer.width, rasterizer.height);
    let dst: &mut [slint::Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[slint::Rgba8Pixel] = bytemuck::cast_slice(&rasterizer.color);
    dst.copy_from_slice(src);
    buffer
}

/// The [`to_pixel_buffer`] counterpart for a [`diagram2d::DiagramFrame`].
#[must_use]
pub fn to_diagram_pixel_buffer(
    frame: &diagram2d::DiagramFrame,
) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(frame.width, frame.height);
    let dst: &mut [slint::Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[slint::Rgba8Pixel] = bytemuck::cast_slice(&frame.color);
    dst.copy_from_slice(src);
    buffer
}
