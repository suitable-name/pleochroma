//! Whether the girdle is level: whether the crown and pavilion facets meet the girdle on one line
//! each, all the way round, or cut into it.
//!
//! A cutter grinds the girdle and cuts the crown and pavilion facets to it. On a level girdle the
//! facets meet it on one girdle line above and one below, so the band between the crown and the
//! pavilion is equally thick at every girdle corner. A facet cut deeper than its neighbours there
//! (a small tier between two mains, say) cuts into the band: the girdle gets thinner where it
//! does, a setting no longer sits evenly on it, and the girdle line no longer tells the cutter
//! where to stop.
//!
//! [`Design::girdle_level`] reads it off the finished stone. The girdle faces are the facets of
//! the girdle tiers (`|angle| = 90`). A corner of a girdle face that lies on a crown facet is on
//! the girdle's top edge, one that lies on a pavilion facet on its bottom edge. Heights are
//! measured along the stone's axis, `y` (the line `x = z = 0`).
//!
//! # Line corners
//!
//! Only the corners that lie on the girdle line are judged:
//!
//! - a corner shared by two or more girdle faces;
//! - both ends of a level edge: an edge between a girdle face and a crown or pavilion facet that
//!   runs horizontally. The facet's plane meets the face's plane along a line whose slope is at
//!   most [`GIRDLE_LEVEL_TOLERANCE_FRACTION`] (a facet at the girdle face's own azimuth runs along
//!   it exactly). The slope is read off the two planes, not the edge's ends, so it does not depend
//!   on the edge's length. An edge no longer than the mesh's resolution is not an edge.
//!
//! Every other corner of a girdle edge is left out: the apex of the V in which two facets at other
//! azimuths meet on one girdle face. Each side of that V is the girdle face cut by a facet at
//! another azimuth, so it slopes, and its apex stands off the girdle line by a few tenths of a
//! percent of the width (above it for crown breaks, so the girdle is thicker there). Cutters take
//! that scallop as part of a correctly cut brilliant, not as an unlevel girdle.
//!
//! # The rule
//!
//! The outermost line level of an edge is the one the girdle-forming facets share: the highest top
//! line corner, the lowest bottom line corner. A crown tier through a top line corner more than
//! [`GIRDLE_LEVEL_TOLERANCE_FRACTION`] of the stone's width below the highest one cuts into the
//! girdle, and so does a pavilion tier through a bottom line corner that far above the lowest one.
//! The girdle is level when no tier does: every top line corner lies within the tolerance of the
//! highest, and every bottom line corner within it of the lowest.

use super::{
    ConstraintTier, Design, cutting_order::flat_cutting_order, girdle_band::corner_resolution,
    hinge::tier_plane_ranges,
};
use glam::DVec3;
use pleochroma::geometry::{
    meet_solver::SolvedTier,
    plane::tier_is_crown_side,
    stone_metrics::{SolidStatus, build_solid_mesh_and_measure},
};
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

/// How far, as a fraction of the stone's width, a girdle line may stray and still be level.
///
/// A girdle line corner more than this inside the outermost line level of its edge cuts into the
/// girdle: 0.025 mm on a stone 10 mm wide. It is also the steepest slope an edge between a girdle
/// face and a facet may have and still count as level (a rise of at most the tolerance across the
/// stone's width).
pub const GIRDLE_LEVEL_TOLERANCE_FRACTION: f64 = 0.0025;

/// A facet whose line on a girdle face has a horizontal direction shorter than this (a facet
/// nearly parallel to the face) has no slope to judge.
const MIN_LINE_HORIZONTAL: f64 = 1e-12;

/// The heights of a solved stone's girdle line, and the tiers that cut into the girdle. Built by
/// [`Design::girdle_level`]; see the module docs for the line corners and the rule.
#[derive(Debug, Clone, PartialEq)]
pub struct GirdleLevel {
    /// The lowest and the highest line corner of the girdle's top edge (where crown facets meet
    /// the girdle faces), as heights along the stone's axis (`y`), in design (mast) units.
    pub top: (f64, f64),
    /// The lowest and the highest line corner of the girdle's bottom edge (where pavilion facets
    /// meet the girdle faces), as heights along the stone's axis, in design units.
    pub bottom: (f64, f64),
    /// The stone's width, in design units: the
    /// [`pleochroma::geometry::stone_metrics::SolidMetrics::width_axis`] the proportions (and
    /// the millimetre scale, `crate::yield_metrics::mm_per_unit`) are read against.
    pub width: f64,
    /// The tiers (indices into [`Design::tiers`]), in cutting order, through a line corner more
    /// than [`Self::tolerance`] inside its edge's outermost line level: a crown tier through a top
    /// line corner below the highest one, a pavilion tier through a bottom line corner above the
    /// lowest one. Empty for a level girdle.
    pub cut_in: Vec<usize>,
}

impl GirdleLevel {
    /// The range the girdle band's thickness lies in at its line corners: from the lowest top line
    /// corner over the highest bottom one to the highest top line corner over the lowest bottom
    /// one. Both ends are the same on a level girdle cut to one exact line above and below.
    #[must_use]
    pub const fn thickness_range(&self) -> (f64, f64) {
        (self.top.0 - self.bottom.1, self.top.1 - self.bottom.0)
    }

    /// How far a line corner may stand inside its edge's outermost line level and still count as
    /// on it: [`GIRDLE_LEVEL_TOLERANCE_FRACTION`] of [`Self::width`], in design units.
    #[must_use]
    pub const fn tolerance(&self) -> f64 {
        GIRDLE_LEVEL_TOLERANCE_FRACTION * self.width
    }

    /// Whether the girdle is level: no tier cuts into it ([`Self::cut_in`] is empty). Then every
    /// top line corner lies within [`Self::tolerance`] of the highest one and every bottom line
    /// corner within it of the lowest one, since each such corner lies on a facet of some tier.
    #[must_use]
    pub const fn is_level(&self) -> bool {
        self.cut_in.is_empty()
    }
}

/// One plane of the stone: its outward normal and its offset (`normal . x = offset`).
type Plane = (DVec3, f64);

/// One crown or pavilion facet plane: the tier that cut it, and the plane.
type FacetPlane = (usize, Plane);

/// One line corner of a girdle edge: its height and the tiers whose facets pass through it.
type EdgeCorner = (f64, Vec<usize>);

/// The planes of a stone sorted by what they are, for the girdle: its girdle faces (with their
/// index into the stone's planes), its crown facets and its pavilion facets.
#[derive(Default)]
struct GirdlePlanes {
    /// The girdle faces' planes, each with its index into the stone's planes.
    walls: Vec<(usize, Plane)>,
    /// The crown facets' planes (the table's included), tier by tier.
    crown: Vec<FacetPlane>,
    /// The pavilion facets' planes (the culet's included), tier by tier.
    pavilion: Vec<FacetPlane>,
}

impl Design {
    /// How level the girdle of the solved stone is: the heights of its line corners above and
    /// below, and the tiers that cut into it (see the module docs).
    ///
    /// `solved` is the mast list of this design's solve. `None` when the design has no flat
    /// girdle tier (`|angle| = 90`), when `solved` is not aligned with [`Self::tiers`], when the
    /// stone does not close, or when the girdle has no line corner on a crown facet or none on a
    /// pavilion facet (there is no girdle line to judge then).
    #[must_use]
    pub fn girdle_level(&self, solved: &[SolvedTier]) -> Option<GirdleLevel> {
        if solved.len() != self.tiers.len() || !self.tiers.iter().any(ConstraintTier::is_girdle) {
            return None;
        }
        let planes = self.planes_from_solved(solved);
        let (SolidStatus::Closed(mesh), Some(metrics)) = build_solid_mesh_and_measure(&planes)
        else {
            return None;
        };
        let width = metrics.width_axis;
        if !(width.is_finite() && width > 0.0) {
            return None;
        }
        // The planes go through `f32`, so a corner where four planes meet lies on the fourth only
        // to within the mesh's resolution.
        let resolution = corner_resolution(&planes);
        let sorted = self.girdle_planes(solved, &planes);

        let mut top: Vec<EdgeCorner> = Vec::new();
        let mut bottom: Vec<EdgeCorner> = Vec::new();
        for (plane, ring) in &mesh.rings {
            let Some(&(_, wall)) = sorted.walls.iter().find(|(index, _)| index == plane) else {
                continue;
            };
            for corner in line_corners(ring, *plane, wall, &sorted, resolution) {
                let on_crown = tiers_through(&sorted.crown, corner, resolution);
                if !on_crown.is_empty() {
                    top.push((corner.y, on_crown));
                }
                let on_pavilion = tiers_through(&sorted.pavilion, corner, resolution);
                if !on_pavilion.is_empty() {
                    bottom.push((corner.y, on_pavilion));
                }
            }
        }
        let top_range = height_range(&top)?;
        let bottom_range = height_range(&bottom)?;

        let tolerance = GIRDLE_LEVEL_TOLERANCE_FRACTION * width;
        let mut cutting: BTreeSet<usize> = BTreeSet::new();
        for (height, tiers) in &top {
            if *height < top_range.1 - tolerance {
                cutting.extend(tiers.iter().copied());
            }
        }
        for (height, tiers) in &bottom {
            if *height > bottom_range.0 + tolerance {
                cutting.extend(tiers.iter().copied());
            }
        }
        let cut_in = flat_cutting_order(&self.tiers)
            .into_iter()
            .filter(|tier| cutting.contains(tier))
            .collect();
        Some(GirdleLevel {
            top: top_range,
            bottom: bottom_range,
            width,
            cut_in,
        })
    }

    /// `planes` (this design's [`Self::planes_from_solved`] of `solved`) sorted for the girdle:
    /// the girdle tiers' planes are its faces, the other tiers' planes its crown or pavilion
    /// facets by the side they face. The preform's planes take no part.
    fn girdle_planes(&self, solved: &[SolvedTier], planes: &[Plane]) -> GirdlePlanes {
        let mut sorted = GirdlePlanes::default();
        for (tier_index, (tier, range)) in self
            .tiers
            .iter()
            .zip(tier_plane_ranges(self, solved))
            .enumerate()
        {
            for index in range {
                let Some(&plane) = planes.get(index) else {
                    continue;
                };
                if tier.is_girdle() {
                    sorted.walls.push((index, plane));
                } else if tier_is_crown_side(tier.angle_deg) {
                    sorted.crown.push((tier_index, plane));
                } else {
                    sorted.pavilion.push((tier_index, plane));
                }
            }
        }
        sorted
    }
}

/// Whether `corner` lies on `plane`, to within `resolution`.
fn on_plane(plane: Plane, corner: DVec3, resolution: f64) -> bool {
    (plane.0.dot(corner) - plane.1).abs() <= resolution
}

/// Whether `facet` meets the girdle face `wall` along a level line: the line both planes share
/// rises at most [`GIRDLE_LEVEL_TOLERANCE_FRACTION`] of its horizontal run.
fn runs_level(facet: Plane, wall: Plane) -> bool {
    let line = facet.0.cross(wall.0);
    let run = line.x.hypot(line.z);
    run > MIN_LINE_HORIZONTAL && line.y.abs() <= GIRDLE_LEVEL_TOLERANCE_FRACTION * run
}

/// The line corners of the girdle face `wall` (plane `index` of the stone, its polygon `ring`):
/// the corners it shares with another girdle face, and both ends of each of its edges that a
/// crown or pavilion facet meets it on along a level line. See the module docs.
fn line_corners(
    ring: &[DVec3],
    index: usize,
    wall: Plane,
    sorted: &GirdlePlanes,
    resolution: f64,
) -> Vec<DVec3> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let mut on_line: Vec<bool> = ring
        .iter()
        .map(|&corner| {
            sorted
                .walls
                .iter()
                .any(|&(other, plane)| other != index && on_plane(plane, corner, resolution))
        })
        .collect();
    for (start, &a) in ring.iter().enumerate() {
        let end = (start + 1) % ring.len();
        let b = ring[end];
        if a.distance(b) <= resolution {
            continue;
        }
        let level = sorted
            .crown
            .iter()
            .chain(&sorted.pavilion)
            .any(|&(_, facet)| {
                on_plane(facet, a, resolution)
                    && on_plane(facet, b, resolution)
                    && runs_level(facet, wall)
            });
        if level {
            on_line[start] = true;
            on_line[end] = true;
        }
    }
    ring.iter()
        .zip(on_line)
        .filter(|&(_, line)| line)
        .map(|(&corner, _)| corner)
        .collect()
}

/// The tiers of `facets` whose plane passes within `resolution` of `corner`, ascending and
/// without duplicates (`facets` lists the planes tier by tier).
fn tiers_through(facets: &[FacetPlane], corner: DVec3, resolution: f64) -> Vec<usize> {
    let mut tiers: Vec<usize> = facets
        .iter()
        .filter(|&&(_, plane)| on_plane(plane, corner, resolution))
        .map(|&(tier, _)| tier)
        .collect();
    tiers.dedup();
    tiers
}

/// The lowest and the highest height of `corners`. `None` without a corner.
fn height_range(corners: &[EdgeCorner]) -> Option<(f64, f64)> {
    let low = corners
        .iter()
        .map(|&(height, _)| height)
        .fold(f64::INFINITY, f64::min);
    let high = corners
        .iter()
        .map(|&(height, _)| height)
        .fold(f64::NEG_INFINITY, f64::max);
    (low <= high).then_some((low, high))
}
