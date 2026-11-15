//! Picked meet corners: the corner a tier is cut to, chosen by the cutter instead of by the
//! solver's shallowest-level rule.
//!
//! # The identity of a corner
//!
//! A pick ([`TierMeetPick`]) is three facets, each a tier by [`TierId`] and one of its
//! index-wheel positions. The point is never stored: the solver (and every check here)
//! recomputes it from the current planes of those facets, so the corner follows its facets
//! when angles or sizes change and stays the same corner. Tiers are named by id, so adding,
//! moving or renaming any tier cannot break a pick.
//!
//! A pick rides beside the tier's [`MeetConstraint::MeetNamed`], which keeps naming the tiers
//! at the corner: every reader of the constraint (the cutting order, the sheet, the `.asc`
//! export) reads the same text with or without a pick.
//!
//! # What this module offers
//!
//! - [`Design::pick_corners`]: the corners of the stone cut before a tier that the tier can be
//!   cut to, each with the facets that form it and the mast that puts the tier through it.
//! - [`Design::meet_pick_edit`]: the one undoable edit that cuts a tier to a corner.
//! - [`Design::meet_pick_states`]: how every stored pick stands after a solve ([`MeetPickState`]).
//! - [`Design::meet_pick_blocked`]: why a tier cannot take a pick at all ([`MeetPickBlock`]).
//! - `Design::solver_pick`: the pick the solver receives, only when it is usable.

use super::{CutWalk, Target};
use crate::{
    design::{ConstraintTier, Design, TierId},
    edit::Edit,
    manufacturability::facet_plane_boundaries,
};
use glam::DVec3;
use pleochroma::geometry::{
    cuts::StandardGemCuts,
    meet_solver::{Block, MeetConstraint, MeetPick, MeetPickPlane, SolvedTier, classify_blocks},
};
use std::collections::BTreeMap;

/// Three planes are independent when `|n1 . (n2 x n3)|` is at least this: the value the solver
/// and the meet-preserving re-solve use, so a pick this module accepts is one the solver can use.
const INDEPENDENT: f64 = 5e-3;

/// Two index-wheel positions this close (modulo the gear) are the same facet.
const INDEX_TOLERANCE: f64 = 1e-6;

/// A mast at or below this is no depth: the corner is not offered.
const MIN_MAST: f64 = 1e-9;

/// One facet of a picked corner: a tier by stable id and one of its index-wheel positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickedFacet {
    /// The tier the facet belongs to.
    pub tier: TierId,
    /// The facet's index-wheel position. `0.0` for a tier without indices (table, culet).
    pub index: f64,
}

/// The corner a tier is cut to ([`Design::tier_meet_picks`]): the point where three facet
/// planes meet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierMeetPick {
    /// The three facets, independent of each other, none of them a facet of the picking tier.
    pub facets: [PickedFacet; 3],
}

/// Every pick of a design, keyed by the picking tier's [`TierId`].
pub type TierMeetPickMap = BTreeMap<TierId, TierMeetPick>;

/// A corner of the stone cut before a tier that the tier can be cut to.
#[derive(Debug, Clone, PartialEq)]
pub struct PickCorner {
    /// Where the corner is, in the design's frame.
    pub point: DVec3,
    /// The tiers whose facets meet there, in cutting order, without duplicates.
    pub tiers: Vec<usize>,
    /// The first three independent facets there (cutting order, then facet order).
    pub pick: TierMeetPick,
    /// The mast that puts the picking tier through the corner.
    pub mast: f64,
    /// A tier at the corner that has no name, so [`MeetConstraint::MeetNamed`] cannot say it.
    /// Such a corner is shown but not pickable.
    pub unnamed_tier: Option<usize>,
}

/// How a stored pick stands after a solve ([`Design::meet_pick_states`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetPickState {
    /// The tier's facets pass through the picked corner, and the corner is on the stone cut
    /// before the tier.
    Met,
    /// The pick is stored but not in use: the tier's constraint is not `MeetNamed`, or its names
    /// do not cover the three picked tiers.
    Dormant,
    /// The planes still meet, but that point is not a corner of the stone when the tier is cut,
    /// or the tier's facets do not pass through it.
    OffStone,
    /// The corner cannot be formed.
    Lost(MeetPickLoss),
}

/// Why a picked corner cannot be formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetPickLoss {
    /// A tier of the pick is no longer in the design.
    TierRemoved,
    /// A tier of the pick has no facet at the picked index any more.
    FacetRemoved,
    /// The three planes no longer meet in one point.
    PlanesParallel,
}

/// Why a tier cannot take a pick at all ([`Design::meet_pick_blocked`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetPickBlock {
    /// Nothing is cut before the tier, so there is no corner to pick.
    NothingCutBefore,
    /// The tier is the only one that sets the size of its block: cutting it to a corner would
    /// leave the block without an anchor.
    SoleBlockAnchor,
    /// The tier sets the stone's size ([`Design::sizing_tier`]).
    SizingTier,
}

/// The finished stone's facet planes with the schedule's plane provenance: what turns a
/// (tier, index-wheel position) into a plane and a facet of the cut walk into its position.
struct PickGeometry<'a> {
    /// The walk whose per-tier facet planes are looked up.
    walk: &'a CutWalk,
    /// Per tier and authored instance, the position in the facet-plane list of the plane that
    /// instance resolves to (a duplicate of an earlier tier's plane resolves to that plane).
    provenance: Vec<Vec<usize>>,
    /// Cumulative facet-plane count per tier.
    ends: Vec<usize>,
}

impl<'a> PickGeometry<'a> {
    /// The geometry of `design` with its `solved` masts. `solved` is aligned with `walk`.
    fn new(design: &Design, walk: &'a CutWalk, solved: &[SolvedTier]) -> Self {
        let schedule = design.to_asc_schedule_from_solved(solved);
        Self {
            walk,
            provenance: StandardGemCuts::asc_plane_provenance(&schedule),
            ends: facet_plane_boundaries(&schedule),
        }
    }

    /// The position in the facet-plane list where `tier`'s own planes start.
    fn start_of(&self, tier: usize) -> usize {
        tier.checked_sub(1)
            .and_then(|before| self.ends.get(before))
            .copied()
            .unwrap_or(0)
    }

    /// The plane at `global` in the facet-plane list, as the walk holds it.
    fn plane_at(&self, global: usize) -> Option<(DVec3, f64)> {
        let owner = self.ends.partition_point(|&end| end <= global);
        let local = global.checked_sub(self.start_of(owner))?;
        self.walk.facets.get(owner)?.get(local).copied()
    }

    /// The plane of `tier`'s authored instance `slot`.
    fn plane_of_slot(&self, tier: usize, slot: usize) -> Option<(DVec3, f64)> {
        self.plane_at(*self.provenance.get(tier)?.get(slot)?)
    }

    /// The index-wheel position of the facet that is the `facet`-th plane of `tier` in the
    /// walk. `0.0` for a tier without indices.
    fn position_of_facet(&self, design: &Design, tier: usize, facet: usize) -> Option<f64> {
        let global = self.start_of(tier) + facet;
        let slot = self
            .provenance
            .get(tier)?
            .iter()
            .position(|&plane| plane == global)?;
        let flat = design.tiers.get(tier)?;
        if flat.indices.is_empty() {
            Some(0.0)
        } else {
            flat.indices.get(slot).copied()
        }
    }

    /// The unit normals of every authored instance of `tier`.
    fn instance_normals(&self, design: &Design, tier: usize) -> Option<Vec<DVec3>> {
        let count = design.tiers.get(tier)?.indices.len().max(1);
        (0..count)
            .map(|slot| self.plane_of_slot(tier, slot).map(|(normal, _)| normal))
            .collect()
    }
}

/// The authored instance of `tier` at index-wheel position `index` (modulo `gear`). A tier
/// without indices has the single instance `0`.
fn facet_slot(tier: &ConstraintTier, gear: f64, index: f64) -> Option<usize> {
    if tier.indices.is_empty() {
        return Some(0);
    }
    if !index.is_finite() {
        return None;
    }
    tier.indices.iter().position(|&candidate| {
        let diff = (candidate - index).rem_euclid(gear);
        diff.min(gear - diff) <= INDEX_TOLERANCE
    })
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

/// The point where three planes meet, `None` when their normals are not independent.
fn meeting_point(planes: [(DVec3, f64); 3]) -> Option<DVec3> {
    let [(n1, d1), (n2, d2), (n3, d3)] = planes;
    let det = n1.dot(n2.cross(n3));
    if det.is_nan() || det.abs() < INDEPENDENT {
        return None;
    }
    // Cramer's rule: the point `p` with `n_i . p = d_i` for all three.
    Some((n2.cross(n3) * d1 + n3.cross(n1) * d2 + n1.cross(n2) * d3) / det)
}

/// The corner `target` of the stone cut before a tier, as an offer for that tier: `None` when
/// three independent facets do not form it or the tier's mast through it is not a depth.
/// `instances` are the unit normals of the picking tier's facets.
fn corner_of(
    design: &Design,
    walk: &CutWalk,
    geometry: &PickGeometry<'_>,
    instances: &[DVec3],
    target: &Target,
) -> Option<PickCorner> {
    let [first, second, third] = independent_triple(walk, &target.planes)?;
    let mast = instances
        .iter()
        .map(|normal| normal.dot(target.point))
        .reduce(f64::max)?;
    if !mast.is_finite() || mast <= MIN_MAST {
        return None;
    }
    let facet = |(tier, facet): (usize, usize)| -> Option<PickedFacet> {
        Some(PickedFacet {
            tier: design.tier_id_at(tier)?,
            index: geometry.position_of_facet(design, tier, facet)?,
        })
    };
    Some(PickCorner {
        point: target.point,
        tiers: target.tiers.clone(),
        pick: TierMeetPick {
            facets: [facet(first)?, facet(second)?, facet(third)?],
        },
        mast,
        unnamed_tier: target.tiers.iter().copied().find(|&tier| {
            design
                .tiers
                .get(tier)
                .is_some_and(|flat| flat.names().is_empty())
        }),
    })
}

impl Design {
    /// The pick of the tier CURRENTLY at `index`, `None` when it has none.
    #[must_use]
    pub fn tier_meet_pick(&self, index: usize) -> Option<&TierMeetPick> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_meet_picks.get(id))
    }

    /// [`PartialEq`]'s check for the picks: equal by tier position, so two designs that pick
    /// the same corners compare equal whatever their id numbers.
    pub(in crate::design) fn tier_meet_picks_eq(&self, other: &Self) -> bool {
        if self.tier_meet_picks.is_empty() && other.tier_meet_picks.is_empty() {
            return true;
        }
        self.tiers.len() == other.tiers.len()
            && (0..self.tiers.len())
                .all(|index| self.pick_by_position(index) == other.pick_by_position(index))
    }

    /// The pick of the tier at `index` with its facets' tiers as positions.
    fn pick_by_position(&self, index: usize) -> Option<[(Option<usize>, f64); 3]> {
        self.tier_meet_pick(index).map(|pick| {
            pick.facets
                .map(|facet| (self.index_of_tier_id(facet.tier), facet.index))
        })
    }

    /// The gear as the solver reads it: at least one tooth.
    fn pick_gear(&self) -> f64 {
        f64::from(self.meta.gear_teeth_abs().max(1))
    }

    /// The corners `tier` can be cut to, on the stone cut before it, in the stone's own corner
    /// order.
    ///
    /// A corner is offered when every face at it is a facet of an earlier flat tier (a corner on
    /// the preform is no point of the design), three of its facets are independent, and the mast
    /// that puts the tier through it is positive. `None` when `solved` is not aligned with the
    /// tiers, the stone does not close, or `tier` is not a flat tier of the design; empty when
    /// nothing is cut before the tier. Builds one solid, so a caller with a large design runs
    /// it off the UI thread.
    #[must_use]
    pub fn pick_corners(&self, solved: &[SolvedTier], tier: usize) -> Option<Vec<PickCorner>> {
        let walk = CutWalk::new(self, solved)?;
        let position = walk.cutting_position(tier);
        if position == usize::MAX {
            return None;
        }
        let geometry = PickGeometry::new(self, &walk, solved);
        let instances = geometry.instance_normals(self, tier)?;
        Some(
            walk.targets(position)
                .iter()
                .filter_map(|target| corner_of(self, &walk, &geometry, &instances, target))
                .collect(),
        )
    }

    /// The one edit that cuts `tier` to `corner`: a batch of clearing the tier's target (only
    /// when it has one, since a target would override the meet), setting its constraint to
    /// [`MeetConstraint::MeetNamed`] with the name of every tier at the corner, and storing the
    /// pick.
    ///
    /// `None` when the corner has an unnamed tier ([`PickCorner::unnamed_tier`]) or a tier of the
    /// corner is not in the design.
    #[must_use]
    pub fn meet_pick_edit(&self, tier: usize, corner: &PickCorner) -> Option<Edit> {
        if corner.unnamed_tier.is_some() {
            return None;
        }
        self.tiers.get(tier)?;
        let names: Vec<String> = corner
            .tiers
            .iter()
            .map(|&at| {
                self.tiers
                    .get(at)
                    .and_then(|flat| flat.names().first().map(|name| (*name).to_owned()))
            })
            .collect::<Option<_>>()?;
        let mut edits = Vec::with_capacity(3);
        if self.tier_target(tier).is_some() {
            edits.push(Edit::SetTierTarget {
                index: tier,
                target: None,
            });
        }
        edits.push(Edit::SetConstraint {
            index: tier,
            constraint: MeetConstraint::MeetNamed(names),
        });
        edits.push(Edit::SetTierMeetPick {
            index: tier,
            pick: Some(corner.pick),
        });
        Some(Edit::Batch(edits))
    }

    /// Why `tier` cannot take a pick at all, `None` when it can.
    ///
    /// Nothing cut before the tier leaves no corner; the sizing tier cannot meet a corner; and a
    /// tier that is the only one anchoring its block would leave the block without a size.
    #[must_use]
    pub fn meet_pick_blocked(&self, tier: usize) -> Option<MeetPickBlock> {
        let first_cut = self
            .cutting_order()
            .into_iter()
            .find_map(|tier_ref| match tier_ref {
                crate::design::TierRef::Flat(flat) => Some(flat),
                crate::design::TierRef::Concave(_) => None,
            });
        if first_cut.is_none_or(|first| first == tier) {
            return Some(MeetPickBlock::NothingCutBefore);
        }
        if self.sizing_tier() == Some(tier) {
            return Some(MeetPickBlock::SizingTier);
        }
        let anchors = |index: usize| {
            self.tiers
                .get(index)
                .is_some_and(|flat| matches!(flat.constraint, MeetConstraint::ScaleReference(_)))
                || self.tier_target(index).is_some()
        };
        if anchors(tier) {
            let blocks: Vec<Block> = classify_blocks(&self.meet_tier_inputs());
            let block = blocks.get(tier).copied()?;
            let another = (0..self.tiers.len())
                .any(|other| other != tier && blocks.get(other) == Some(&block) && anchors(other));
            if !another {
                return Some(MeetPickBlock::SoleBlockAnchor);
            }
        }
        None
    }

    /// How every stored pick stands, by tier index, in tier order.
    ///
    /// Empty when no tier has a pick, when `solved` is not aligned with the tiers or when the
    /// stone does not close. Builds one solid per picked tier.
    #[must_use]
    pub fn meet_pick_states(&self, solved: &[SolvedTier]) -> Vec<(usize, MeetPickState)> {
        if self.tier_meet_picks.is_empty() {
            return Vec::new();
        }
        let Some(walk) = CutWalk::new(self, solved) else {
            return Vec::new();
        };
        let geometry = PickGeometry::new(self, &walk, solved);
        (0..self.tiers.len())
            .filter_map(|index| {
                let pick = self.tier_meet_pick(index)?;
                Some((index, self.pick_state(index, pick, &walk, &geometry)))
            })
            .collect()
    }

    /// The state of the pick of the tier at `index`.
    fn pick_state(
        &self,
        index: usize,
        pick: &TierMeetPick,
        walk: &CutWalk,
        geometry: &PickGeometry<'_>,
    ) -> MeetPickState {
        if !self
            .tiers
            .get(index)
            .is_some_and(|flat| matches!(flat.constraint, MeetConstraint::MeetNamed(_)))
        {
            return MeetPickState::Dormant;
        }
        let tiers = pick.facets.map(|facet| self.index_of_tier_id(facet.tier));
        let [Some(a), Some(b), Some(c)] = tiers else {
            return MeetPickState::Lost(MeetPickLoss::TierRemoved);
        };
        let gear = self.pick_gear();
        let mut planes = Vec::with_capacity(3);
        for (&tier, facet) in [a, b, c].iter().zip(&pick.facets) {
            let plane = (tier != index)
                .then(|| self.tiers.get(tier))
                .flatten()
                .and_then(|flat| facet_slot(flat, gear, facet.index))
                .and_then(|slot| geometry.plane_of_slot(tier, slot));
            let Some(plane) = plane else {
                return MeetPickState::Lost(MeetPickLoss::FacetRemoved);
            };
            planes.push(plane);
        }
        let Ok(planes) = <[(DVec3, f64); 3]>::try_from(planes) else {
            return MeetPickState::Lost(MeetPickLoss::FacetRemoved);
        };
        let Some(point) = meeting_point(planes) else {
            return MeetPickState::Lost(MeetPickLoss::PlanesParallel);
        };
        let named = self.tiers[index].meet_target_indices(&self.tiers);
        if ![a, b, c].iter().all(|tier| named.contains(tier)) {
            return MeetPickState::Dormant;
        }
        let tolerance = walk.tolerance();
        let on_stone = walk
            .targets(walk.cutting_position(index))
            .iter()
            .any(|target| target.point.distance(point) <= tolerance);
        let reached = walk.facets.get(index).is_some_and(|facets| {
            let reach = |&(normal, offset): &(DVec3, f64)| normal.dot(point) - offset;
            facets.iter().any(|facet| reach(facet).abs() <= tolerance)
                && facets.iter().all(|facet| reach(facet) <= tolerance)
        });
        if on_stone && reached {
            MeetPickState::Met
        } else {
            MeetPickState::OffStone
        }
    }

    /// The pick the solver receives for the tier at `index`: `Some` only when it is usable, that
    /// is when the tier's constraint is `MeetNamed`, its names cover the three picked tiers,
    /// those tiers are other tiers of the design and each has a facet at the picked index. The
    /// solver then checks the planes' independence itself.
    pub(crate) fn solver_pick(&self, index: usize) -> Option<MeetPick> {
        let pick = self.tier_meet_pick(index)?;
        let flat = self.tiers.get(index)?;
        if !matches!(flat.constraint, MeetConstraint::MeetNamed(_)) {
            return None;
        }
        let named = flat.meet_target_indices(&self.tiers);
        let gear = self.pick_gear();
        let mut planes = [MeetPickPlane {
            tier: 0,
            index: 0.0,
        }; 3];
        for (plane, facet) in planes.iter_mut().zip(&pick.facets) {
            let tier = self.index_of_tier_id(facet.tier)?;
            if tier == index || !named.contains(&tier) {
                return None;
            }
            facet_slot(self.tiers.get(tier)?, gear, facet.index)?;
            *plane = MeetPickPlane {
                tier,
                index: facet.index,
            };
        }
        Some(MeetPick { planes })
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
