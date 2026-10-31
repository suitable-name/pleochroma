//! What an inclusion of a block, cylinder or pebble is, and where it sits.
//!
//! A mesh rough has a file whose coordinates an inclusion file shares. A block, a cylinder or a
//! pebble has none, so the owner types the position (the centre of the inclusion, in mm from the
//! corner of the uncut rough, an empty field meaning the rough's centre on that axis) and, for a
//! sphere, an ellipsoid or a box, its size. Everything here is plain functions on strings and
//! numbers, so the tests run with the rest of the workspace.

use glam::DVec3;
use pleochroma_cut_core::rough_plan::{
    PrimitivePose, PrimitiveShape, RoughBase, RoughMesh, primitive_mesh,
};

/// The largest size a typed inclusion may have, in mm: the planner's own limit for a rough.
pub const MAX_INCLUSION_SIZE_MM: f64 = 2000.0;

/// The largest position a typed inclusion may have, in mm, on either side of the corner.
const MAX_POSITION_MM: f64 = 100_000.0;

/// An inclusion's centre in mm in the frame of the uncut rough; `None` is the rough's centre on
/// that axis.
pub type Placement = [Option<f64>; 3];

/// What the Add Inclusion button builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InclusionKind {
    /// A mesh file (OBJ, STL or PLY); the default.
    #[default]
    MeshFile,
    /// A sphere of a typed diameter.
    Sphere,
    /// An ellipsoid of three typed axis lengths.
    Ellipsoid,
    /// A box of three typed edge lengths.
    Box,
}

impl InclusionKind {
    /// Every kind, in the order of the window's kind box (index 0 is the default).
    pub const ALL: [Self; 4] = [Self::MeshFile, Self::Sphere, Self::Ellipsoid, Self::Box];

    /// The kind at position `index` of the window's kind box; anything out of range is a mesh
    /// file.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .unwrap_or_default()
    }
}

/// `text` as a number (a comma is a decimal point), or `None` for anything else.
fn parse_mm(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// One typed coordinate: `None` for an empty field.
fn parse_coordinate(text: &str, axis: &str) -> Result<Option<f64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_mm(text)
        .filter(|value| value.abs() <= MAX_POSITION_MM)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "The inclusion's {axis} position must be a number in mm. Leave it empty for the \
                 rough's centre."
            )
        })
}

/// The position typed into the X, Y and Z fields: an empty field is the rough's centre on that
/// axis, and a comma is a decimal point.
///
/// # Errors
///
/// Returns the message for a field that is not a number.
pub fn parse_placement(x: &str, y: &str, z: &str) -> Result<Placement, String> {
    Ok([
        parse_coordinate(x, "X")?,
        parse_coordinate(y, "Y")?,
        parse_coordinate(z, "Z")?,
    ])
}

/// One typed size, which must be a number from 0 (exclusive) to [`MAX_INCLUSION_SIZE_MM`].
fn parse_size_mm(text: &str, name: &str) -> Result<f64, String> {
    if text.trim().is_empty() {
        return Err(format!("Type the inclusion's {name} in mm."));
    }
    parse_mm(text)
        .filter(|value| *value > 0.0 && *value <= MAX_INCLUSION_SIZE_MM)
        .ok_or_else(|| {
            format!(
                "The inclusion's {name} must be a number above 0 and up to \
                 {MAX_INCLUSION_SIZE_MM} mm."
            )
        })
}

/// The centre of the uncut `base` in mm, in its own frame (its corner at the origin).
#[must_use]
pub fn rough_centre_mm(base: &RoughBase) -> DVec3 {
    base.bounding_box_centre()
}

/// The centre of the uncut `base` as the window shows it: "10.0, 8.0, 6.0".
#[must_use]
pub fn centre_text(base: &RoughBase) -> String {
    let centre = rough_centre_mm(base);
    format!("{:.1}, {:.1}, {:.1}", centre.x, centre.y, centre.z)
}

/// The centre an inclusion gets for `placement` on `base`: the typed numbers, and the rough's
/// centre where a field was left empty.
#[must_use]
pub fn placed_centre(base: &RoughBase, placement: Placement) -> DVec3 {
    let centre = rough_centre_mm(base);
    DVec3::new(
        placement[0].unwrap_or(centre.x),
        placement[1].unwrap_or(centre.y),
        placement[2].unwrap_or(centre.z),
    )
}

/// The centre of the bounding box of the points the `triangles` use, or `None` when they use
/// none.
pub(super) fn used_bbox_centre(points: &[DVec3], triangles: &[[u32; 3]]) -> Option<DVec3> {
    let mut used = triangles
        .iter()
        .flatten()
        .filter_map(|&index| points.get(usize::try_from(index).ok()?).copied());
    let first = used.next()?;
    let (lo, hi) = used.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    Some((lo + hi) * 0.5)
}

/// The closed mesh of a typed primitive on `base`, in the rough's frame.
///
/// `sizes` are the three size fields as typed, in mm: a sphere reads the first as its diameter,
/// an ellipsoid its three axis lengths, a box its three edge lengths. The round shapes are
/// polyhedra that contain the shape (the core's `primitive_mesh`), so the keep-out volume is
/// never smaller than what was typed.
///
/// # Errors
///
/// Returns the message for a size that is not a number up to [`MAX_INCLUSION_SIZE_MM`], for a
/// kind that is not a primitive, or for a shape the core cannot build.
pub fn primitive_in_rough(
    kind: InclusionKind,
    sizes: [&str; 3],
    placement: Placement,
    base: &RoughBase,
) -> Result<RoughMesh, String> {
    let three = |names: [&str; 3]| -> Result<[f64; 3], String> {
        Ok([
            parse_size_mm(sizes[0], names[0])?,
            parse_size_mm(sizes[1], names[1])?,
            parse_size_mm(sizes[2], names[2])?,
        ])
    };
    let shape = match kind {
        InclusionKind::MeshFile => {
            return Err("Choose Sphere, Ellipsoid or Box to type an inclusion.".to_owned());
        }
        InclusionKind::Sphere => PrimitiveShape::Sphere {
            radius_mm: parse_size_mm(sizes[0], "diameter")? * 0.5,
        },
        InclusionKind::Ellipsoid => PrimitiveShape::Ellipsoid {
            semi_axes_mm: three(["X size", "Y size", "Z size"])?.map(|size| size * 0.5),
        },
        InclusionKind::Box => PrimitiveShape::Box {
            size_mm: three(["X size", "Y size", "Z size"])?,
        },
    };
    let pose = PrimitivePose::at(placed_centre(base, placement).to_array());
    primitive_mesh(shape, pose).map_err(|error| format!("The inclusion cannot be built: {error}."))
}
