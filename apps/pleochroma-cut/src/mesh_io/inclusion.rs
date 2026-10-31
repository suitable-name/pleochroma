//! Reading a mesh file as an inclusion: the same parsers and the same unit choice as a
//! rough, and little else. The file's numbers come out in millimetres, in the file's own
//! coordinates; the planner then moves them into the rough's frame and checks them against
//! the rough (`RoughModel::with_inclusions_added` in the core), so this module has no window,
//! no file access and no rough, and its tests run with the rest of the workspace.
//!
//! An imported mesh rough shares its file's coordinates with an inclusion file. A block, a
//! cylinder or a pebble has no file, so an inclusion is placed by typed numbers instead (see
//! [`placement`]): a mesh file by the centre of its bounding box, a sphere, an ellipsoid or a
//! box by its size.

mod placement;
#[cfg(test)]
mod tests_placement;

use glam::DVec3;
use pleochroma_cut_core::rough_plan::{
    HullError, MAX_MESH_TRIANGLES, MeshError, RoughBase, RoughMesh, RoughModel,
    shape::hull::{self, SourceFrame},
};

use super::{MeshUnit, parse_mesh};
pub use placement::{InclusionKind, Placement, centre_text, parse_placement, primitive_in_rough};
use placement::{placed_centre, used_bbox_centre};

/// The margin an inclusion gets by default, in mm: real inclusion boundaries are uncertain,
/// so stones keep this far from the inclusion as it is drawn.
pub const DEFAULT_MARGIN_MM: f64 = 0.3;

/// The largest margin the field takes, in mm.
pub const MAX_MARGIN_MM: f64 = 10.0;

/// Shown when a tool that needs a mesh to trace on (an inclusion located from photos) is asked
/// for on a rough that is not an imported mesh. Typed inclusions and mesh files need no such
/// rough: they go into a block, a cylinder or a pebble as well.
pub const NOT_A_MESH_ROUGH: &str = "Locating an inclusion from photos needs a rough imported from \
     a mesh file. Choose the Mesh rough and import one first.";

/// The margin typed into the field ("0,3" and "0.3" both read as 0.3; an empty field is the
/// default, [`DEFAULT_MARGIN_MM`]).
///
/// # Errors
///
/// Returns the message for text that is not a number from 0 to [`MAX_MARGIN_MM`] mm.
pub fn parse_margin_mm(text: &str) -> Result<f64, String> {
    if text.trim().is_empty() {
        return Ok(DEFAULT_MARGIN_MM);
    }
    let value: f64 = text
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| "The inclusion margin must be a number in mm.".to_owned())?;
    if value.is_finite() && (0.0..=MAX_MARGIN_MM).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "The inclusion margin must be between 0 and {MAX_MARGIN_MM} mm."
        ))
    }
}

/// The text of a refused inclusion: the core's reason, and the likeliest cause of it.
///
/// For an imported mesh an inclusion outside the material usually comes from a file in other
/// coordinates. A block, a cylinder or a pebble has no file: the cause is the typed position
/// or size, and an inclusion that reaches the surface is not a notch to model in a mesh.
fn refusal_text(error: &HullError, base: &RoughBase) -> String {
    if matches!(base, RoughBase::Hull { .. }) {
        return match error {
            HullError::Inclusion(MeshError::InclusionOutside(_)) => format!(
                "{error}. The inclusion file is read in the same coordinates and unit as the \
                 rough's file; check that both come from the same scene."
            ),
            _ => error.to_string(),
        };
    }
    match error {
        HullError::Inclusion(MeshError::InclusionOutside(_)) => {
            "The inclusion is not inside the rough's material. Its position is the centre of the \
             inclusion in mm from the rough's corner: check the position and the size."
                .to_owned()
        }
        HullError::Inclusion(MeshError::InclusionReachesSurface(_)) => {
            "The inclusion reaches the surface of the rough. Move it inward or make it smaller."
                .to_owned()
        }
        HullError::Inclusion(MeshError::InclusionsOverlap) => {
            "The inclusion crosses another inclusion of the rough, or wraps it. Move it or make \
             it smaller."
                .to_owned()
        }
        HullError::InclusionTooClose => {
            "The inclusion is closer to the surface (or to another inclusion) than its margin \
             allows. Lower the margin, move it inward or make it smaller."
                .to_owned()
        }
        _ => error.to_string(),
    }
}

/// The inclusion given as `points` (mm, in the file's own coordinates) and `triangles`, as a
/// mesh in the rough's own frame.
///
/// For an imported mesh the file's coordinates are moved the way the import moved the rough
/// (and Fit to weight scaled it), and `placement` is not used. A rough read back from a saved
/// plan that did not keep that frame is its own. For a block, a cylinder or a pebble the
/// mesh is moved so that the centre of its bounding box is at `placement` (an empty axis is
/// the rough's centre).
///
/// # Errors
///
/// Returns the message for points and triangles that are not a closed mesh.
pub fn inclusion_in_rough_frame(
    base: &RoughBase,
    points: &[DVec3],
    triangles: &[[u32; 3]],
    placement: Placement,
) -> Result<RoughMesh, String> {
    let moved: Vec<DVec3> = if let RoughBase::Hull { id, .. } = *base {
        let frame = hull::source_frame(id).unwrap_or(SourceFrame::IDENTITY);
        points.iter().map(|&p| frame.apply(p)).collect()
    } else {
        let centre = used_bbox_centre(points, triangles)
            .ok_or_else(|| "The inclusion's mesh cannot be used: it has no faces.".to_owned())?;
        let shift = placed_centre(base, placement) - centre;
        points.iter().map(|&p| p + shift).collect()
    };
    RoughMesh::new(&moved, triangles)
        .map_err(|error| format!("The inclusion's mesh cannot be used: {error}."))
}

/// `model` with `inclusion` (a closed mesh in the rough's own frame, in mm) added, grown by
/// `margin_mm`. The planner window adds an inclusion through this, whether the mesh came from
/// a file, was typed, or came from a tool that located it.
///
/// An imported mesh gets a new base; a block, a cylinder or a pebble keeps its base and its
/// inclusion set grows. The model passed in is unchanged.
///
/// # Errors
///
/// Returns the message for an inclusion that does not lie in the rough's material, reaches its
/// surface, crosses another inclusion, cannot hold its margin, or is one too many.
pub fn add_inclusion(
    model: &RoughModel,
    inclusion: &RoughMesh,
    margin_mm: f64,
) -> Result<RoughModel, String> {
    model
        .with_inclusions_added(std::slice::from_ref(inclusion), margin_mm)
        .map_err(|error| refusal_text(&error, &model.base))
}

/// `model` without inclusion `index` (0-based, in the order added). The model passed in is
/// unchanged.
///
/// # Errors
///
/// Returns the message for an index the rough has no inclusion for.
pub fn remove_inclusion(model: &RoughModel, index: usize) -> Result<RoughModel, String> {
    model
        .with_inclusion_removed(index)
        .map_err(|error| error.to_string())
}

/// The line the window lists for inclusion number `index` (0-based): its size and volume.
#[must_use]
pub fn inclusion_row_text(index: usize, extents_mm: DVec3, volume_mm3: f64) -> String {
    format!(
        "Inclusion {}: {:.1} x {:.1} x {:.1} mm, {:.1} mm\u{b3}",
        index + 1,
        extents_mm.x,
        extents_mm.y,
        extents_mm.z,
        volume_mm3
    )
}

/// What the model readout adds for a rough with inclusions: "including 2 inclusions
/// (128 mm³)".
#[must_use]
pub fn including_text(count: usize, volume_mm3: f64) -> String {
    let noun = if count == 1 {
        "inclusion"
    } else {
        "inclusions"
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "display only: a stone's inclusion volume in mm3 is far below the i64 range (`as` saturates)"
    )]
    let rounded_mm3 = volume_mm3.round() as i64;
    format!("including {count} {noun} ({rounded_mm3} mm\u{b3})")
}

/// The points (mm) and triangles of the mesh file `bytes` (see [`parse_mesh`] for how the
/// format is chosen from `extension`), its numbers read in `unit`.
///
/// # Errors
///
/// Returns the message for a file that is truncated or malformed, or that gives no
/// triangles (no faces at all, or more than the planner takes): an inclusion must be a
/// closed mesh, and a point cloud has no inside.
pub fn inclusion_parts(
    bytes: &[u8],
    extension: Option<&str>,
    unit: MeshUnit,
) -> Result<(Vec<DVec3>, Vec<[u32; 3]>), String> {
    let (mut points, triangles, _note) = parse_mesh(bytes, extension)?;
    if triangles.is_empty() {
        return Err(format!(
            "the file has no usable faces. An inclusion must be a closed mesh of triangles, \
             at most {MAX_MESH_TRIANGLES} of them"
        ));
    }
    if unit != MeshUnit::Millimetre {
        let factor = unit.factor_to_mm();
        for point in &mut points {
            *point *= factor;
        }
    }
    Ok((points, triangles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_io::{
        fixtures::{CUBE_OBJ, ascii_stl_of, binary_stl_of, ply_of},
        parse_obj_mesh,
    };

    #[test]
    fn an_inclusion_is_read_in_the_chosen_unit() {
        let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        // The 20 mm cube written in centimetres (2 x 2 x 2) as each format.
        let cm: Vec<DVec3> = points.iter().map(|&p| p / 10.0).collect();
        let faces: Vec<Vec<u32>> = triangles.iter().map(|t| Vec::from(*t)).collect();
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("a.stl", ascii_stl_of(&cm, &triangles).into_bytes()),
            ("b.stl", binary_stl_of("solid", &cm, &triangles)),
            ("c.ply", ply_of("binary_little_endian", &cm, &faces)),
            ("d.ply", ply_of("ascii", &cm, &faces)),
        ];
        for (name, bytes) in files {
            let extension = name.rsplit('.').next();
            let (read, tris) =
                inclusion_parts(&bytes, extension, MeshUnit::Centimetre).expect(name);
            assert_eq!(tris.len(), 12, "{name}");
            let top = read.iter().fold(0.0_f64, |m, p| m.max(p.max_element()));
            assert!((top - 20.0).abs() < 1e-4, "{name}: {top}");
            // Read as millimetres the same file is ten times smaller.
            let (small, _) = inclusion_parts(&bytes, extension, MeshUnit::Millimetre).expect(name);
            let top = small.iter().fold(0.0_f64, |m, p| m.max(p.max_element()));
            assert!((top - 2.0).abs() < 1e-4, "{name}: {top}");
        }
    }

    #[test]
    fn millimetres_are_not_scaled_at_all() {
        let (read, _) =
            inclusion_parts(CUBE_OBJ.as_bytes(), Some("obj"), MeshUnit::Millimetre).expect("reads");
        let (points, _) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        assert_eq!(read, points);
    }

    #[test]
    fn a_file_without_triangles_is_not_an_inclusion() {
        let cloud = "v 0 0 0\nv 9 0 0\nv 0 9 0\nv 0 0 9\n";
        let message = inclusion_parts(cloud.as_bytes(), Some("obj"), MeshUnit::Millimetre)
            .expect_err("a point cloud has no inside");
        assert!(message.contains("closed mesh"), "{message}");
        let mut many = String::from("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n");
        many.push_str(&"f 1 2 3\n".repeat(MAX_MESH_TRIANGLES + 1));
        let message = inclusion_parts(many.as_bytes(), Some("obj"), MeshUnit::Millimetre)
            .expect_err("too many triangles");
        assert!(
            message.contains(&MAX_MESH_TRIANGLES.to_string()),
            "{message}"
        );
    }

    /// What the planner window does for a file of an imported mesh rough: the points in the
    /// rough's frame, then added.
    fn place(
        base: &RoughBase,
        points: &[DVec3],
        triangles: &[[u32; 3]],
        margin_mm: f64,
    ) -> Result<RoughModel, String> {
        let mesh = inclusion_in_rough_frame(base, points, triangles, [None; 3])?;
        add_inclusion(&RoughModel::new(*base, Vec::new()), &mesh, margin_mm)
    }

    /// The 20 mm cube as a convex rough imported from a file whose corner is at `shift`, and
    /// the cube `[lo, hi]` in the same file's coordinates.
    fn rough_and_inclusion(
        shift: DVec3,
        lo: f64,
        hi: f64,
    ) -> (RoughBase, Vec<DVec3>, Vec<[u32; 3]>) {
        let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        let rough: Vec<DVec3> = points.iter().map(|&p| p + shift).collect();
        let base = pleochroma_cut_core::rough_plan::import_hull(&rough).expect("a cube");
        let inner: Vec<DVec3> = points
            .iter()
            .map(|&p| p * ((hi - lo) / 20.0) + DVec3::splat(lo) + shift)
            .collect();
        (base, inner, triangles)
    }

    #[test]
    fn an_inclusion_in_the_files_coordinates_is_placed_in_the_rough() {
        for shift in [DVec3::ZERO, DVec3::new(100.0, -50.0, 7.5)] {
            let (base, inner, triangles) = rough_and_inclusion(shift, 8.0, 12.0);
            let placed = place(&base, &inner, &triangles, DEFAULT_MARGIN_MM)
                .expect("the inclusion is in the rough");
            assert!(
                matches!(placed.base, RoughBase::Hull { .. }),
                "a mesh rough keeps its inclusions in its base"
            );
            assert!(placed.inclusions.is_empty());
            let list = placed.inclusion_infos();
            assert_eq!(list.len(), 1);
            assert!(
                (list[0].centre_mm - DVec3::splat(10.0)).length() < 1e-6,
                "{shift}"
            );
        }
    }

    #[test]
    fn an_inclusion_in_other_coordinates_is_refused_with_the_likely_cause() {
        let away = DVec3::new(100.0, 0.0, 0.0);
        let (base, inner, triangles) = rough_and_inclusion(away, 8.0, 12.0);
        // The inclusion file still has the coordinates of a rough at the origin.
        let unshifted: Vec<DVec3> = inner.iter().map(|&p| p - away).collect();
        let message =
            place(&base, &unshifted, &triangles, 0.0).expect_err("it lies outside the rough");
        assert!(
            message.contains("not inside the rough's material"),
            "{message}"
        );
        assert!(message.contains("same coordinates and unit"), "{message}");
    }

    #[test]
    fn other_refusals_come_through_in_the_cores_words() {
        let (base, inner, triangles) = rough_and_inclusion(DVec3::ZERO, 15.0, 25.0);
        let message = place(&base, &inner, &triangles, 0.0).expect_err("it pokes out");
        assert!(message.contains("must be cut away"), "{message}");
    }

    #[test]
    fn the_photo_tool_refusal_names_the_photo_tool() {
        assert!(
            NOT_A_MESH_ROUGH.contains("from photos"),
            "{NOT_A_MESH_ROUGH}"
        );
    }

    #[test]
    fn the_margin_field_takes_numbers_from_zero_to_ten() {
        assert_eq!(parse_margin_mm("0.3"), Ok(0.3));
        assert_eq!(parse_margin_mm("  "), Ok(DEFAULT_MARGIN_MM));
        assert_eq!(parse_margin_mm(" 0,5 "), Ok(0.5));
        assert_eq!(parse_margin_mm("0"), Ok(0.0));
        assert_eq!(parse_margin_mm("10"), Ok(10.0));
        for bad in ["abc", "-0.1", "10.1", "inf", "NaN"] {
            assert!(parse_margin_mm(bad).is_err(), "{bad:?}");
        }
        assert!((DEFAULT_MARGIN_MM - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn the_readouts_name_the_inclusions() {
        assert_eq!(
            inclusion_row_text(1, DVec3::new(4.6, 4.6, 4.6), 97.336),
            "Inclusion 2: 4.6 x 4.6 x 4.6 mm, 97.3 mm\u{b3}"
        );
        assert_eq!(
            including_text(1, 64.2),
            "including 1 inclusion (64 mm\u{b3})"
        );
        assert_eq!(
            including_text(2, 128.0),
            "including 2 inclusions (128 mm\u{b3})"
        );
    }

    #[test]
    fn a_broken_file_names_its_problem() {
        let message = inclusion_parts(b"v 1 2\n", Some("obj"), MeshUnit::Millimetre)
            .expect_err("a short vertex line");
        assert!(message.contains("line 1"), "{message}");
    }
}
