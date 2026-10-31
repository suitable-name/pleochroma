//! Lock: keep a stone and plan the rest of the rough around it.
//!
//! A locked stone's PIECE (the sawn box, clipped by the rough's own planes) becomes an inclusion
//! shell of the rough with margin 0: a closed solid inside the rough where no stone may go,
//! which still counts as material you hold (see the mesh `inclusion` module). A re-plan then
//!
//! - runs on the SHAPED path, also for a block rough (a rough with an inclusion is a mesh rough;
//!   [`rough_with_locked_pieces`] turns a convex rough into one first, moved so its bounding box
//!   starts at the origin as every imported rough is, which [`LockedRough::offset_mm`] reports);
//! - keeps the locked piece out;
//! - lists the locked stone first, outside the cut plan ([`LockedLayout`]): the guillotine cut
//!   plan of the rest cannot in general be cut around a piece in the interior, so the results
//!   say [`LOCKED_FIRST_NOTE`]. This limitation is stated in the manual too.
//!
//! The planner window keeps the locked stones and calls these functions; this module is the
//! geometry and the data, with no window state.

use std::fmt;

use glam::DVec3;
use pleochroma::geometry::stone_metrics::measure_solid_with_vertices;

use crate::rough_plan::{
    PlacedStone, RoughBase, RoughLayout, RoughModel,
    shape::{InclusionFault, InclusionSet, RoughMesh, ShapeError, hull},
    stone_preform::piece_box_halfspaces,
};

/// How far inside the piece box and the rough's planes a locked shell lies, in mm (a tenth of a
/// micrometre: far below anything a saw cuts).
const SHELL_INSET_MM: f64 = 1e-4;

/// What the results say next to a plan that has locked stones.
pub const LOCKED_FIRST_NOTE: &str =
    "The locked stone is cut first; the slabs below are for the remaining rough.";

/// Why a piece could not be locked into the rough.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The stone's piece box is not a usable solid.
    PieceUnusable,
    /// The rough could not be measured or turned into a mesh rough.
    Rough(String),
    /// The piece does not lie in the rough's material (it reaches the surface, or crosses
    /// another locked piece).
    Shell(String),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PieceUnusable => f.write_str("The locked stone's piece is not a usable solid."),
            Self::Rough(why) => write!(f, "The rough cannot take a locked piece: {why}"),
            Self::Shell(why) => write!(f, "The locked piece does not fit the rough: {why}"),
        }
    }
}

impl std::error::Error for LockError {}

/// The [`LockError::Shell`] of a failed check of a rough whose own `own` inclusions are followed
/// by the locked pieces: a failing piece is numbered as a locked stone, not as an inclusion the
/// user does not have.
fn shell_error(error: &ShapeError, own: usize) -> LockError {
    match *error {
        ShapeError::Inclusion { index, reason }
            if index >= own
                && !matches!(
                    reason,
                    InclusionFault::TooComplex | InclusionFault::Unusable
                ) =>
        {
            LockError::Shell(format!(
                "locked stone {}: {reason}. Move the stone or remove the lock.",
                index - own + 1
            ))
        }
        _ => LockError::Shell(error.to_string()),
    }
}

/// A rough with the locked pieces left out, and the move between its frame and the original.
#[derive(Debug, Clone, PartialEq)]
pub struct LockedRough {
    /// The rough to plan: a mesh rough with the pieces as inclusions (its orientation and cuts
    /// are the original's).
    pub model: RoughModel,
    /// The original frame is this far from the model's: `original = model + offset_mm`. Zero for
    /// a rough that was a mesh already. [`restore_frame`] undoes it on a plan's layouts.
    pub offset_mm: [f64; 3],
}

/// A layout with its locked stones: the locked ones are cut first and are not part of the
/// layout's cut plan, which is for the remaining rough.
#[derive(Debug, Clone, PartialEq)]
pub struct LockedLayout {
    /// The locked stones, in the original frame, listed first.
    pub locked: Vec<PlacedStone>,
    /// The layout of the remaining rough (its stones and cut plan, original frame).
    pub layout: RoughLayout,
}

impl LockedLayout {
    /// Every stone, the locked ones first.
    pub fn stones(&self) -> impl Iterator<Item = &PlacedStone> {
        self.locked.iter().chain(&self.layout.stones)
    }

    /// The finished weight of all stones in carats.
    #[must_use]
    pub fn total_carat(&self) -> f64 {
        self.locked.iter().map(|stone| stone.carat).sum::<f64>() + self.layout.total_carat
    }

    /// The finished volume of all stones in mm^3.
    #[must_use]
    pub fn total_volume_mm3(&self) -> f64 {
        self.locked
            .iter()
            .map(|stone| stone.volume_mm3)
            .sum::<f64>()
            + self.layout.total_volume_mm3
    }
}

/// The convex solid `n . p <= d` of `planes` as a closed mesh (vertices and fan-triangulated
/// faces, wound outward), `None` when the planes bound no volume. Coincident planes count
/// once; a plane that touches the solid in fewer than three corners is redundant and skipped.
fn convex_mesh(planes: &[(DVec3, f64)], shift: DVec3) -> Option<RoughMesh> {
    let mut distinct: Vec<(DVec3, f64)> = Vec::with_capacity(planes.len());
    let scale = planes
        .iter()
        .map(|(_, d)| d.abs())
        .fold(1e-12_f64, f64::max);
    for &(n, d) in planes {
        if !distinct
            .iter()
            .any(|&(m, e)| (m - n).length() < 1e-9 && (e - d).abs() <= 1e-9 * scale)
        {
            distinct.push((n, d));
        }
    }
    let (_, corners) = measure_solid_with_vertices(&distinct)?;
    if corners.len() < 4 {
        return None;
    }
    let eps = 1e-7 * scale;
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for &(n, d) in &distinct {
        let mut face: Vec<usize> = (0..corners.len())
            .filter(|&i| (n.dot(corners[i]) - d).abs() <= eps)
            .collect();
        if face.len() < 3 {
            continue;
        }
        let centre = face.iter().map(|&i| corners[i]).sum::<DVec3>() / face.len() as f64;
        let u = (corners[face[0]] - centre).normalize_or_zero();
        let w = n.cross(u);
        face.sort_by(|&a, &b| {
            let angle = |i: usize| {
                let p = corners[i] - centre;
                p.dot(w).atan2(p.dot(u))
            };
            angle(a).total_cmp(&angle(b))
        });
        for k in 1..face.len() - 1 {
            tris.push([
                u32::try_from(face[0]).ok()?,
                u32::try_from(face[k]).ok()?,
                u32::try_from(face[k + 1]).ok()?,
            ]);
        }
    }
    let moved: Vec<DVec3> = corners.iter().map(|&c| c - shift).collect();
    RoughMesh::new(&moved, &tris).ok()
}

/// The shell of `stone`'s piece: the sawn box clipped by `rough_halfspaces`.
///
/// The half-spaces are the rough's own planes (rough frame, mm); the shell is a closed mesh moved
/// by `-shift` (the frame of the model the shell goes into; zero for the original frame).
///
/// # Errors
///
/// [`LockError::PieceUnusable`] when the piece is not a solid.
pub fn locked_shell(
    stone: &PlacedStone,
    rough_halfspaces: &[(DVec3, f64)],
    shift: [f64; 3],
) -> Result<RoughMesh, LockError> {
    let faces = piece_box_halfspaces(stone).ok_or(LockError::PieceUnusable)?;
    // Every plane moves in by a hair, so a piece that rests on the rough's surface (or on a
    // cut) does not share a face with it: the shell then lies strictly in the material, where
    // the mesh accepts an inclusion.
    let planes: Vec<(DVec3, f64)> = faces
        .iter()
        .chain(rough_halfspaces)
        .map(|&(n, d)| (n, d - SHELL_INSET_MM))
        .collect();
    convex_mesh(&planes, DVec3::from_array(shift)).ok_or(LockError::PieceUnusable)
}

/// `model` with the pieces of `locked` left out of it as inclusions (margin 0), ready to plan on
/// the shaped path. With nothing locked the model is returned as it is.
///
/// A rough that is not a mesh rough (a block, cylinder, pebble or plain hull) first becomes the
/// hull of its cut solid, moved so its bounding box starts at the origin (see
/// [`LockedRough::offset_mm`]); a mesh rough keeps its own frame and cuts. So does a block,
/// cylinder or pebble that has inclusions of its own: the pieces join them, next to the
/// inclusions already there, and the offset is zero.
///
/// # Errors
///
/// [`LockError`] when the rough cannot be measured, a piece is not a solid, or a piece does not
/// lie in the rough's material.
pub fn rough_with_locked_pieces(
    model: &RoughModel,
    locked: &[PlacedStone],
) -> Result<LockedRough, LockError> {
    if locked.is_empty() {
        return Ok(LockedRough {
            model: model.clone(),
            offset_mm: [0.0; 3],
        });
    }
    let halfspaces = model
        .halfspaces()
        .map_err(|error| LockError::Rough(error.to_string()))?;
    // A block, cylinder or pebble that has inclusions of its own keeps its base, its cuts and
    // its frame: the locked pieces join its inclusions. (Turned into a hull, a cut that slices
    // one of its inclusions would leave the shell through the hull's surface.)
    if !matches!(model.base, RoughBase::Hull { .. }) && !model.inclusions.is_empty() {
        let shells = locked
            .iter()
            .map(|stone| locked_shell(stone, &halfspaces, [0.0; 3]))
            .collect::<Result<Vec<_>, _>>()?;
        let inclusions = model
            .inclusions
            .appended(shells)
            .map_err(|error| LockError::Shell(error.to_string()))?;
        let with_pieces = model.clone().with_inclusion_set(inclusions);
        with_pieces
            .check_inclusions()
            .map_err(|error| shell_error(&error, model.inclusions.len()))?;
        return Ok(LockedRough {
            model: with_pieces,
            offset_mm: [0.0; 3],
        });
    }
    let (base, cuts, offset) = if let RoughBase::Hull { .. } = model.base {
        (model.base, model.cuts.clone(), DVec3::ZERO)
    } else {
        let solid = model
            .solid()
            .map_err(|error| LockError::Rough(error.to_string()))?;
        let (_, corners) = measure_solid_with_vertices(&solid.planes)
            .ok_or_else(|| LockError::Rough("the rough has no volume".to_string()))?;
        let base =
            hull::import_hull(&corners).map_err(|error| LockError::Rough(format!("{error:?}")))?;
        (base, Vec::new(), solid.bbox_min)
    };
    let shells = locked
        .iter()
        .map(|stone| locked_shell(stone, &halfspaces, offset.to_array()))
        .collect::<Result<Vec<_>, _>>()?;
    let base = hull::add_inclusion_meshes(&base, &shells, 0.0)
        .map_err(|error| LockError::Shell(format!("{error:?}")))?;
    Ok(LockedRough {
        model: RoughModel {
            base,
            cuts,
            inclusions: InclusionSet::EMPTY,
            orientation: model.orientation,
        },
        offset_mm: offset.to_array(),
    })
}

/// Moves `layout` (planned on a [`LockedRough::model`]) back into the original frame: every
/// piece and pose centre by `offset_mm`.
pub fn restore_frame(layout: &mut RoughLayout, offset_mm: [f64; 3]) {
    for stone in &mut layout.stones {
        for (axis, shift) in offset_mm.iter().enumerate() {
            stone.piece_origin_mm[axis] += shift;
            stone.pose.center_mm[axis] += shift;
        }
    }
}
