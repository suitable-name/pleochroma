//! Posed primitives for inclusions: a sphere, an ellipsoid or a box.
//!
//! A block, a cylinder or a pebble has no file to take an inclusion from, so the owner types a
//! shape, its size and where it sits. Each primitive is a closed [`RoughMesh`] in the frame of
//! the uncut base (see [`InclusionSet`](super::InclusionSet)), ready for
//! [`RoughModel::with_inclusions_added`](super::RoughModel::with_inclusions_added). They are
//! built on the shapes the located inclusion already uses ([`box_mesh`] and [`sphere_shell`]),
//! not on a tessellator of their own.
//!
//! A round primitive is a polyhedron that CONTAINS the shape: a keep-out shell must never be
//! smaller than the inclusion it stands for. The sphere shell is the icosahedron of
//! [`sphere_shell`] split once into 80 faces, scaled until every face lies at least a radius
//! from the centre. An ellipsoid is the same shell scaled per axis, which keeps every face
//! outside the ellipsoid (an affine map takes the tangent planes of a sphere to tangent
//! planes of its image).

use std::collections::BTreeMap;

use glam::{DQuat, DVec3, EulerRot};

use super::{MeshError, RoughMesh};
use crate::rough_plan::locate::{box_mesh, sphere_shell};

/// What an inclusion primitive is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrimitiveShape {
    /// A sphere of this radius in mm.
    Sphere {
        /// The radius in mm.
        radius_mm: f64,
    },
    /// An ellipsoid with these semi-axes in mm, along x, y and z before it is turned.
    Ellipsoid {
        /// The semi-axes in mm.
        semi_axes_mm: [f64; 3],
    },
    /// A box with these edge lengths in mm, along x, y and z before it is turned.
    Box {
        /// The edge lengths in mm.
        size_mm: [f64; 3],
    },
}

/// Where a primitive sits in the rough's frame, and how it is turned.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PrimitivePose {
    /// The centre in mm, in the frame of the uncut base (its corner at the origin).
    pub centre_mm: [f64; 3],
    /// Turns about x, then y, then z, in degrees (extrinsic, in this order); no turn when all
    /// are zero. A sphere ignores it.
    pub rotation_deg: [f64; 3],
}

impl PrimitivePose {
    /// A primitive centred at `centre_mm`, not turned.
    #[must_use]
    pub const fn at(centre_mm: [f64; 3]) -> Self {
        Self {
            centre_mm,
            rotation_deg: [0.0; 3],
        }
    }

    /// The turn as a rotation.
    #[must_use]
    pub fn rotation(&self) -> DQuat {
        let [x, y, z] = self.rotation_deg.map(f64::to_radians);
        DQuat::from_euler(EulerRot::XYZ, x, y, z)
    }
}

/// Whether every length of `v` is finite and positive.
fn positive(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite() && *c > 0.0)
}

/// The mesh of `points` and `tris` (wound outward) scaled per axis by `scale`, turned by
/// `rotation` and moved to `centre`.
fn posed(
    points: &[DVec3],
    tris: &[[u32; 3]],
    scale: DVec3,
    rotation: DQuat,
    centre: DVec3,
) -> Result<RoughMesh, MeshError> {
    let moved: Vec<DVec3> = points
        .iter()
        .map(|&p| rotation * (p * scale) + centre)
        .collect();
    RoughMesh::new(&moved, tris)
}

/// The index of the point halfway between `a` and `b`, on the unit sphere; made once.
fn midpoint(points: &mut Vec<DVec3>, made: &mut BTreeMap<(u32, u32), u32>, a: u32, b: u32) -> u32 {
    let key = (a.min(b), a.max(b));
    if let Some(&index) = made.get(&key) {
        return index;
    }
    let index = u32::try_from(points.len()).expect("a primitive has few vertices");
    points.push((points[a as usize] + points[b as usize]).normalize());
    made.insert(key, index);
    index
}

/// A polyhedron around the unit ball: 42 vertices and 80 triangles wound outward, with every
/// face at least 1 from the origin.
fn circumscribed_unit_shell() -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (corners, faces) = sphere_shell(DVec3::ZERO, 1.0);
    let mut points: Vec<DVec3> = corners.iter().map(|p| p.normalize()).collect();
    let mut made = BTreeMap::new();
    let mut tris = Vec::with_capacity(faces.len() * 4);
    for &[a, b, c] in &faces {
        let ab = midpoint(&mut points, &mut made, a, b);
        let bc = midpoint(&mut points, &mut made, b, c);
        let ca = midpoint(&mut points, &mut made, c, a);
        tris.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
    }
    // The vertices are on the unit sphere, so every face is at most 1 from the centre; moving
    // them all out until the nearest face is at 1 puts every face at 1 or more.
    let nearest = tris
        .iter()
        .map(|tri| {
            let [a, b, c] = tri.map(|i| points[i as usize]);
            (b - a).cross(c - a).normalize().dot(a)
        })
        .fold(f64::INFINITY, f64::min);
    let grow = 1.0 / nearest;
    for p in &mut points {
        *p *= grow;
    }
    (points, tris)
}

/// The closed mesh of `shape` at `pose`, in the frame of the uncut base.
///
/// A round shape is a polyhedron that contains it (see the module documentation), so the
/// keep-out volume is never smaller than the shape.
///
/// # Errors
///
/// [`MeshError::Degenerate`] when a size is not finite and positive or the pose is not finite;
/// otherwise as [`RoughMesh::new`].
pub fn primitive_mesh(shape: PrimitiveShape, pose: PrimitivePose) -> Result<RoughMesh, MeshError> {
    let centre = DVec3::from(pose.centre_mm);
    let rotation = pose.rotation();
    if !centre.is_finite() || !rotation.is_finite() {
        return Err(MeshError::Degenerate);
    }
    match shape {
        PrimitiveShape::Sphere { radius_mm } => round([radius_mm; 3], rotation, centre),
        PrimitiveShape::Ellipsoid { semi_axes_mm } => round(semi_axes_mm, rotation, centre),
        PrimitiveShape::Box { size_mm } => {
            if !positive(size_mm) {
                return Err(MeshError::Degenerate);
            }
            let local = box_mesh(DVec3::from(size_mm) * 0.5)?;
            posed(
                local.vertices(),
                local.triangles(),
                DVec3::ONE,
                rotation,
                centre,
            )
        }
    }
}

/// The sphere or ellipsoid shell with these semi-axes.
fn round(semi_axes: [f64; 3], rotation: DQuat, centre: DVec3) -> Result<RoughMesh, MeshError> {
    if !positive(semi_axes) {
        return Err(MeshError::Degenerate);
    }
    let (points, tris) = circumscribed_unit_shell();
    posed(&points, &tris, DVec3::from(semi_axes), rotation, centre)
}
