//! The height of the tray under the stone, shared by the Stone Shadow and the Tray Reflection.
//!
//! The tray is a horizontal plane just below the culet: the lowest vertex of the true solid
//! (the polytope's own for a plane-only stone, the tool-aware mesh's with tools), lowered by
//! `1e-5` of the bounding-sphere radius so the stone never touches it. [`solid_frame`] is the
//! one place those numbers are computed; the shadow map's `StoneFrame` and [`tray_plane_y`]
//! both call it, so the shadow's receiving plane and the mirror's plane cannot drift by a bit.
//!
//! Nothing here depends on a Cargo feature: the B-rep and the solid mesh are compiled into
//! every build, so the mirror can be resolved on targets without the shadow precompute.

use crate::geometry::{
    brep::GemPolyhedron,
    stone_metrics::{SolidStatus, build_solid_mesh_geom},
    tool::StoneGeometry,
};
use glam::{DVec3, Vec3};

/// The vertices of the true solid: the polytope's own for a plane-only stone, and the
/// tool-aware solid mesh's with tools. Tools subtract material, so their bounding boxes
/// never enter this set. `None` when the geometry does not bound a closed solid.
pub(super) fn solid_vertices(geom: StoneGeometry<'_>) -> Option<Vec<Vec3>> {
    if geom.tools.is_empty() {
        let hull = GemPolyhedron::from_planes(geom.planes.to_vec()).ok()?;
        return Some(hull.vertices);
    }
    let planes: Vec<(DVec3, f64)> = geom
        .planes
        .iter()
        .map(|p| {
            (
                DVec3::new(
                    f64::from(p.normal[0]),
                    f64::from(p.normal[1]),
                    f64::from(p.normal[2]),
                ),
                -f64::from(p.d),
            )
        })
        .collect();
    match build_solid_mesh_geom(&planes, geom.tools) {
        SolidStatus::Closed(mesh) => Some(mesh.positions.iter().map(|p| p.as_vec3()).collect()),
        _ => None,
    }
}

/// Conservative bounds of the solid and the tray plane derived from them.
#[cfg_attr(
    not(feature = "stone-shadow"),
    expect(
        dead_code,
        reason = "without the shadow precompute only `plane_y` is read; the bounds are the shadow map's"
    )
)]
pub(super) struct SolidFrame {
    /// Centre of the vertices' bounding box (the emission sphere's centre).
    pub(super) sphere_center: Vec3,
    /// Largest distance from `sphere_center` to a vertex.
    pub(super) sphere_radius: f32,
    /// Horizontal (x, z) centre of the bounding box.
    pub(super) center: [f32; 2],
    /// Largest horizontal distance from `center` to a vertex.
    pub(super) radius_xz: f32,
    /// Height of the tray plane: the lowest vertex lowered by `1e-5` of `sphere_radius`.
    pub(super) plane_y: f32,
}

/// The bounds of `vertices` and the tray plane, `None` when the solid is degenerate (an empty
/// set, a nonfinite or zero radius).
///
/// The single computation behind the shadow map's frame and [`tray_plane_y`].
/// The operation order fixes the bits that both callers share.
pub(super) fn solid_frame(vertices: &[Vec3]) -> Option<SolidFrame> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &v in vertices {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let sphere_center = (lo + hi) * 0.5;
    let sphere_radius = vertices
        .iter()
        .map(|v| v.distance(sphere_center))
        .fold(0.0_f32, f32::max);
    let center = [sphere_center.x, sphere_center.z];
    let radius_xz = vertices
        .iter()
        .map(|v| (v.x - center[0]).hypot(v.z - center[1]))
        .fold(0.0_f32, f32::max);
    if !sphere_radius.is_finite()
        || sphere_radius <= 0.0
        || !radius_xz.is_finite()
        || radius_xz <= 0.0
    {
        return None;
    }
    Some(SolidFrame {
        sphere_center,
        sphere_radius,
        center,
        radius_xz,
        plane_y: sphere_radius.mul_add(-1e-5, lo.y),
    })
}

/// Height of the tray plane under this stone.
///
/// Bit for bit the Stone Shadow's `plane_y` (`sphere_radius.mul_add(-1e-5, lo.y)` over the
/// true solid's vertices). `None` when the geometry does not bound a closed, nondegenerate
/// solid.
#[must_use]
pub fn tray_plane_y(geom: StoneGeometry<'_>) -> Option<f32> {
    let vertices = solid_vertices(geom)?;
    solid_frame(&vertices).map(|frame| frame.plane_y)
}
