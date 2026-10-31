//! The solid-renderer catalogue preview: a flat-shaded, edge-outlined picture of a
//! design drawn by the software rasterizer in a few milliseconds, as a stand-in until
//! the traced preview is rendered.

use super::{PREVIEW_DISTANCE, PREVIEW_YAW, PreviewView, encode_png};
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};
use pleochroma_solid::{
    mesh_cache::MeshCache,
    preview::{
        StoneGeometryBuf,
        camera::{fit_distance_for_view, orbit_distance_bounds, planar_bounding_radius},
    },
    raster::{SolidRasterizer, SolidStyle},
};

/// The camera distance that fits the stone `planes` into the square catalogue preview, as
/// the viewport's Fit does: the stone's bounding sphere at the vertical field of view, with
/// Fit's margin. One distance per design, so its front and top views share a scale. The
/// fixed default pose distance for planes that do not close a solid. Concave tools only
/// remove material, so the planes alone bound the stone.
#[must_use]
pub fn preview_distance(planes: &[GpuFacetPlane]) -> f32 {
    planar_bounding_radius(&StoneGeometryBuf {
        planes: planes.to_vec(),
        ..StoneGeometryBuf::default()
    })
    .map_or(PREVIEW_DISTANCE, |radius| {
        let (min, _) = orbit_distance_bounds(radius);
        fit_distance_for_view(radius, 1.0).max(min)
    })
}

/// Draws `planes` from `view`'s pose into a `size` x `size` PNG, on an opaque black
/// background like the traced previews. `None` for a zero size, an arrangement that does
/// not close into a solid, or a failed PNG encode.
///
/// Uses the same camera as [`super::render_view`], so a solid picture and the traced one
/// that replaces it frame the stone identically. CPU only; it never touches the GPU.
///
/// Production draws every solid stand-in through [`render_view_solid_with_tools`] (the batch
/// preview passes the design's concave tools); this planar entry is the reference the tests
/// compare it against, so it exists only in test builds.
#[cfg(test)]
#[must_use]
pub fn render_view_solid(
    planes: &[GpuFacetPlane],
    size: u32,
    view: PreviewView,
) -> Option<Vec<u8>> {
    render_view_solid_with_tools(planes, &[], size, view)
}

/// The solid picture of the stone `planes` minus the concave `tools` (the grooves and
/// dimples the traced preview that replaces this picture shows). With no tools this is
/// the planar picture.
#[must_use]
pub fn render_view_solid_with_tools(
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    size: u32,
    view: PreviewView,
) -> Option<Vec<u8>> {
    if size == 0 {
        return None;
    }
    let mut cache = MeshCache::default();
    let mesh = if tools.is_empty() {
        // `GpuFacetPlane` is `n . x + d <= 0`; the mesh cache takes `n . x <= m`.
        let half_spaces: Vec<(Vec3, f32)> = planes
            .iter()
            .map(|plane| (Vec3::from(plane.normal), -plane.d))
            .collect();
        cache.get_or_build(&half_spaces)?
    } else {
        cache.get_or_build_geometry(&StoneGeometryBuf {
            planes: planes.to_vec(),
            tools: tools.to_vec(),
            // Bookkeeping for the facet map and the hover text; the picture needs neither.
            placements: Vec::new(),
            // The solid picture is flat shaded: no finish shows.
            frosted: Vec::new(),
        })?
    };
    let camera = Camera::new(
        PREVIEW_YAW,
        view.pitch(),
        preview_distance(planes),
        DEFAULT_FOV_DEG,
    );
    let style = SolidStyle {
        background: [0, 0, 0, 255],
        // A catalogue stand-in is shown small and replaced by the traced picture: no caption.
        show_orientation_marker: false,
        ..SolidStyle::default()
    };
    let mut raster = SolidRasterizer::new(size, size);
    raster.render_prepared(mesh, &camera, &style);
    encode_png(size, size, &raster.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube's six faces.
    fn cube() -> Vec<GpuFacetPlane> {
        [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|n| GpuFacetPlane::new(n, -0.5))
        .collect()
    }

    #[test]
    fn a_closed_stone_draws_a_png_of_the_requested_size_with_painted_pixels() {
        let png = render_view_solid(&cube(), 48, PreviewView::Top).expect("a cube closes");
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (48, 48));
        assert!(
            decoded.pixels().any(|p| p.0[..3] != [0, 0, 0]),
            "something other than the background was painted"
        );
    }

    /// With no tools the tool-aware entry is the planar picture, byte for byte.
    #[test]
    fn no_tools_draw_the_planar_picture() {
        assert_eq!(
            render_view_solid_with_tools(&cube(), &[], 40, PreviewView::Front),
            render_view_solid(&cube(), 40, PreviewView::Front)
        );
    }

    #[test]
    fn an_open_arrangement_and_a_zero_size_draw_nothing() {
        assert!(render_view_solid(&cube()[..3], 32, PreviewView::Front).is_none());
        assert!(render_view_solid(&cube(), 0, PreviewView::Front).is_none());
    }
}
