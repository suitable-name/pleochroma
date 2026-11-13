//! The meet-preserving re-solve: [`Design::meet_targets`] records, for every tier that meets a
//! point of the stone cut so far, which three planes of earlier tiers form that point (and how
//! far the tier passed from it), and [`Design::reanchor_meets`] puts each such tier back on the
//! point of the same three planes, at the same signed distance, after the angles changed (a
//! Retarget, or a template created in another material).
//!
//! Snapping to the nearest point ([`Design::snap_depths_to_meets`]) can pick the wrong point
//! once the angles moved far (quartz to diamond); the recorded planes cannot. A tier meets with
//! all its facets, but its mast is one number, so the first facet stands for the tier: the
//! mirrored and rotated facets follow by symmetry.

use super::{CutMeet, CutWalk, SnapMove, Target};
use crate::design::Design;
use glam::DVec3;
use pleochroma::geometry::meet_solver::{MeetConstraint, SolvedTier};

/// Two planes, or three, are independent when `|n1 x n2|`, or `|n1 . (n2 x n3)|`, is at least
/// this. The point three such planes form then moves by about `2e-5` of the stone at most for
/// the `f32` rounding of the facet planes, well inside the meet tolerance; a triple below it is
/// degenerate (parallel or nearly so).
const INDEPENDENT: f64 = 5e-3;

/// A depth change smaller than this fraction of the stone's size is no change: far below the
/// `f32` resolution of a facet plane, so [`Design::reanchor_meets`] leaves the mast as it is
/// rather than rewrite it with rounding noise.
const NO_MOVE_FRACTION: f64 = 1e-9;

/// Which facets form the point a meeting tier reaches when it is cut.
///
/// Three planes of earlier tiers, each named by (tier index, facet index within that tier's
/// facet planes), and how far the tier's first facet passed from their point.
/// [`Design::meet_targets`] records them; [`Design::reanchor_meets`] puts the tier back at the
/// same distance from their point.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetTarget {
    /// The meeting tier, as an index into `Design::tiers`.
    pub tier: usize,
    /// The three independent planes through the point the tier's first facet reaches, as
    /// `(tier, facet)`: the first three among the facets at that point, in cutting order and
    /// then facet order, that are independent of the ones before.
    pub planes: [(usize, usize); 3],
    /// The signed distance of the tier's first facet plane from that point when it was
    /// recorded, `offset - normal . point` in design units: within the meet tolerance, and
    /// kept by [`Design::reanchor_meets`]. A pin-prick culet authored a hair above its point
    /// records that hair, so it stays a facet.
    pub miss: f64,
}

impl Design {
    /// For every tier whose cut-so-far meet is [`CutMeet::Meets`], the three independent planes
    /// of earlier tiers through the point its FIRST facet reaches. In cutting order.
    ///
    /// `None` when `solved` is misaligned or the stone does not close. A meeting tier whose
    /// point no three facets there form independently (no triple whose determinant reaches
    /// `5e-3`) has no entry.
    #[must_use]
    pub fn meet_targets(&self, solved: &[SolvedTier]) -> Option<Vec<MeetTarget>> {
        let walk = CutWalk::new(self, solved)?;
        let tolerance = walk.tolerance();
        let mut found = Vec::new();
        for (position, &tier) in walk.order.iter().enumerate() {
            let (CutMeet::Meets(_), targets) = walk.meet_at(position) else {
                continue;
            };
            let Some(&first) = walk.facets.get(tier).and_then(|facets| facets.first()) else {
                continue;
            };
            let Some(target) = nearest_within(first, &targets, tolerance) else {
                continue;
            };
            let Some(planes) = independent_triple(&walk, &target.planes) else {
                continue;
            };
            if let Some(point) = point_of(&walk, &planes) {
                let (normal, offset) = first;
                found.push(MeetTarget {
                    tier,
                    planes,
                    miss: offset - normal.dot(point),
                });
            }
        }
        Some(found)
    }

    /// After the angles changed, puts each target's tier back on the point of its three planes
    /// (at its recorded miss), in cutting order.
    ///
    /// The angles may have changed, but not the tiers, their facet counts or their indices:
    /// `targets` come from [`Self::meet_targets`] on the same tiers. Each point is recomputed
    /// from the current angles and the depths re-anchored so far, and the tier's first facet is
    /// put back at the signed distance from it the target recorded ([`MeetTarget::miss`]):
    /// through it for an exact meet, a hair above it for a pin-prick culet, which so stays a
    /// facet. Only explicit-depth (`ScaleReference`) tiers without an authored target move;
    /// anchors and other tiers keep their depth, and a tier already where it belongs (within a
    /// billionth of the size) is not rewritten.
    ///
    /// Returns the moves, in cutting order; the caller re-solves afterwards. `None`, with
    /// nothing changed, when `solved` is misaligned, the stone does not close, or a triple has
    /// become degenerate (parallel planes) or names a facet the design does not have.
    ///
    /// Like [`Self::snap_depths_to_meets`], this writes the constraints directly: an editor
    /// applies the moves through its history (`Edit::SetConstraint`, or the tier in an
    /// `Edit::ModifyTier`) in the same undo step as the angle change.
    pub fn reanchor_meets(
        &mut self,
        targets: &[MeetTarget],
        solved: &[SolvedTier],
    ) -> Option<Vec<SnapMove>> {
        let mut walk = CutWalk::new(self, solved)?;
        let mut ordered: Vec<&MeetTarget> = targets.iter().collect();
        ordered.sort_by_key(|target| walk.cutting_position(target.tier));
        // Every triple is checked before anything moves. A plane's normal follows its tier's
        // angle alone, so a depth moved on the way cannot make a checked triple degenerate.
        for target in &ordered {
            let has_facet = walk
                .facets
                .get(target.tier)
                .is_some_and(|facets| !facets.is_empty());
            if !has_facet || point_of(&walk, &target.planes).is_none() {
                return None;
            }
        }
        let no_move = NO_MOVE_FRACTION * walk.size;
        let mut working = solved.to_vec();
        let mut moves = Vec::new();
        for target in ordered {
            let Some(tier) = self.tiers.get(target.tier) else {
                continue;
            };
            let MeetConstraint::ScaleReference(old_mast) = tier.constraint else {
                continue;
            };
            if self.tier_target(target.tier).is_some() {
                continue;
            }
            let first = walk
                .facets
                .get(target.tier)
                .and_then(|facets| facets.first())
                .copied();
            let (Some(point), Some((normal, offset))) = (point_of(&walk, &target.planes), first)
            else {
                continue;
            };
            let new_offset = normal.dot(point) + target.miss;
            if !new_offset.is_finite()
                || new_offset <= 0.0
                || (new_offset - offset).abs() <= no_move
            {
                continue;
            }
            // A plane's offset is the mast's magnitude, so the mast keeps its sign.
            let new_mast = new_offset.copysign(old_mast);
            self.set_explicit_depth(&mut working, target.tier, new_mast);
            walk.refresh_facets(self, &working);
            moves.push(SnapMove {
                tier: target.tier,
                old_mast,
                new_mast,
                meets: tiers_of(&target.planes),
            });
        }
        Some(moves)
    }
}

/// The target nearest to `facet`'s plane among `targets` (the first of equals), when it lies
/// within `tolerance` of it.
fn nearest_within(facet: (DVec3, f64), targets: &[Target], tolerance: f64) -> Option<&Target> {
    let (normal, offset) = facet;
    let mut nearest: Option<(f64, &Target)> = None;
    for target in targets {
        let distance = (normal.dot(target.point) - offset).abs();
        if distance <= tolerance && nearest.is_none_or(|(best, _)| distance < best) {
            nearest = Some((distance, target));
        }
    }
    nearest.map(|(_, target)| target)
}

/// The first three of `planes` (in their order) that are independent of the ones chosen before
/// them: the second not parallel to the first, the third not in their pencil. `None` when there
/// are no three.
fn independent_triple(walk: &CutWalk, planes: &[(usize, usize)]) -> Option<[(usize, usize); 3]> {
    let mut chosen: Vec<((usize, usize), DVec3)> = Vec::with_capacity(3);
    for &plane in planes {
        let Some((normal, _)) = facet_plane(walk, plane) else {
            continue;
        };
        let independent = match chosen.as_slice() {
            [] => true,
            [(_, a)] => a.cross(normal).length() >= INDEPENDENT,
            [(_, a), (_, b)] => a.dot(b.cross(normal)).abs() >= INDEPENDENT,
            _ => false,
        };
        if independent {
            chosen.push((plane, normal));
        }
        if let [(first, _), (second, _), (third, _)] = chosen.as_slice() {
            return Some([*first, *second, *third]);
        }
    }
    None
}

/// The plane of facet `facet` of tier `tier`, as `walk` holds it.
fn facet_plane(walk: &CutWalk, (tier, facet): (usize, usize)) -> Option<(DVec3, f64)> {
    walk.facets.get(tier)?.get(facet).copied()
}

/// The point where the three `planes` meet, from their planes as `walk` holds them. `None`
/// when one is missing or the three are not independent ([`INDEPENDENT`]).
fn point_of(walk: &CutWalk, planes: &[(usize, usize); 3]) -> Option<DVec3> {
    let [first, second, third] = *planes;
    let (n1, d1) = facet_plane(walk, first)?;
    let (n2, d2) = facet_plane(walk, second)?;
    let (n3, d3) = facet_plane(walk, third)?;
    let det = n1.dot(n2.cross(n3));
    if det.is_nan() || det.abs() < INDEPENDENT {
        return None;
    }
    // Cramer's rule: the point `p` with `n_i . p = d_i` for all three.
    Some((n2.cross(n3) * d1 + n3.cross(n1) * d2 + n1.cross(n2) * d3) / det)
}

/// The tiers of `planes`, in their (cutting) order, without duplicates.
fn tiers_of(planes: &[(usize, usize); 3]) -> Vec<usize> {
    let mut tiers: Vec<usize> = planes.iter().map(|&(tier, _)| tier).collect();
    tiers.dedup();
    tiers
}

#[cfg(test)]
#[path = "reanchor_tests.rs"]
mod tests;
