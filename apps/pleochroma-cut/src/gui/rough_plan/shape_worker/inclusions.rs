//! The inclusion layer of the model view: the shells of a rough's inclusions as a mesh of
//! their own, in the frame of the rough's mesh.
//!
//! A block, cylinder or pebble is drawn from its planes (so Face from View and the hover pick
//! the right face) and an imported mesh from its clipped surface; neither draws an inclusion
//! as part of the rough. The view lays this layer over the opaque rough, see-through, and
//! never picks it.

use super::mesh_of_surface;
use glam::DVec3;
use pleochroma::geometry::stone_metrics::SolidMesh;
use pleochroma_cut_core::rough_plan::RoughModel;

/// The sharpest crease between two triangles of one shell that is still drawn without an
/// edge line (in degrees). A shell is faceted by construction (an 80-face sphere has about
/// 20 degrees between its faces), so this only hides the seams of a flat run: the diagonal
/// of a box face.
pub(super) const INCLUSION_CREASE_DEGREES: f64 = 12.0;

/// Appends `part` (a mesh built by `mesh_of_surface`, so it has per-ring normals and edge
/// flags) to `into`.
fn append(into: &mut SolidMesh, part: SolidMesh) {
    let Ok(offset) = u32::try_from(into.positions.len()) else {
        return;
    };
    into.positions.extend(part.positions);
    into.normals.extend(part.normals);
    into.facet_id.extend(part.facet_id);
    into.indices
        .extend(part.indices.iter().map(|&i| i + offset));
    into.rings.extend(part.rings);
    into.piece_normals
        .get_or_insert_with(Vec::new)
        .extend(part.piece_normals.unwrap_or_default());
    into.edge_visible
        .get_or_insert_with(Vec::new)
        .extend(part.edge_visible.unwrap_or_default());
}

/// The inclusion shells of `model`, clipped by its cuts, in the centred frame (the
/// bounding-box centre of the base at the origin, mm) of `centred_mesh`. Every triangle of
/// a shell is one ring with its own normal; the cut planes close a shell they slice. Empty
/// when the rough has no inclusions, or a cut takes them all away.
///
/// The facet ids of the layer mean nothing (the layer is never picked); `planes` are the
/// model's halfspaces, the base's first and then one per cut.
#[must_use]
pub(super) fn inclusion_layer(model: &RoughModel, planes: &[(DVec3, f64)]) -> SolidMesh {
    let Some(rough) = model.mesh().filter(|rough| rough.inclusion_count() > 0) else {
        return SolidMesh::default();
    };
    let Ok(base) = model.base.to_halfspaces(false) else {
        return SolidMesh::default();
    };
    let cuts = planes.get(base.len()..).unwrap_or_default();
    let centre = model.base.bounding_box_centre();
    let mut layer = SolidMesh::default();
    for shell in rough.inclusions() {
        // A shell that the cuts take away entirely has no surface left: nothing to draw.
        if let Ok(part) = mesh_of_surface(
            &shell.clipped_surface(cuts),
            base.len(),
            centre,
            INCLUSION_CREASE_DEGREES,
        ) {
            append(&mut layer, part);
        }
    }
    layer
}

#[cfg(test)]
mod tests;
