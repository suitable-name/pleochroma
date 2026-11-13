//! The cut-so-far meet of every flat tier ([`Design::cut_meets`]), and the snap that moves a
//! floating tier onto the nearest point it can meet ([`Design::snap_depths_to_meets`]).
//!
//! # Why the stone cut so far
//!
//! A cutter reads "Meet P1, P2" as "cut until this facet reaches the point where P1 and P2
//! meet", on the stone as it is when the tier is cut. The finished stone can share a vertex
//! between two tiers that the stone cut so far does not have yet (in the Pentalume design, P2
//! lands 0.12 mm from the nearest vertex of the stone cut before it), so a meet read against
//! the finished stone can leave the cutter nowhere to stop. This module therefore looks at the
//! stone as it is when each tier is cut.
//!
//! # The stone cut so far
//!
//! For each flat tier in [`Design::cutting_order`] (the concave tiers take no part), the stone
//! cut so far is the solid that the facets of the flat tiers cut before it bound, closed by the
//! design's preform planes (or, should the preform have none, by an axis-aligned box four times
//! the finished stone's extent). A corner of that solid is a *target*, a point a cutter can cut
//! to, only when every face incident at it is a facet of an earlier tier: a corner on the
//! preform is where the rough happens to end, not a point of the design. A tier meets when each
//! of its facet planes passes within [`MEET_TOLERANCE_FRACTION`] of the stone's size of a target.
//!
//! # Precedence
//!
//! A tier is, the first that applies:
//! 1. [`CutMeetAnchor::SetStoneSize`] when it is [`Design::sizing_tier`];
//! 2. [`CutMeetAnchor::CutToCenterpoint`] when it is the first tier in cutting order whose
//!    facets (at least three) all end in one point on the axis of the finished stone, unless it
//!    meets the stone cut so far: the twelve mains of Cam's Portuguese meet the points tiers 3
//!    and 4 left ("Meet 3"), and the centerpoint forms by itself;
//! 3. [`CutMeetAnchor::SetGirdleWidth`] when it is the first crown tier (the table excepted) in
//!    cutting order, or the table of a design with no other flat crown tier (pavilion mains,
//!    girdle, table), and its facets reach a girdle tier in the finished stone (or the design
//!    has no girdle tier, so the preform's walls are the girdle);
//! 4. [`CutMeetAnchor::SetTableSize`] when it is the table, it does not meet the stone cut so
//!    far, and that stone has no point a table could meet: every target is on the axis (the apex
//!    of crown mains meeting in one point) or a corner of a girdle or pavilion facet, none a
//!    crown vertex off the axis (the crown mains of the Rich Teaching Design meet only in the
//!    apex and on the girdle);
//! 5. when the stone cut so far has no target at all: [`CutMeetAnchor::SetGirdleLine`] for a
//!    pavilion tier whose facets reach, in the finished stone, a girdle tier cut before it,
//!    else [`CutMeetAnchor::NothingToMeet`];
//! 6. else [`CutMeet::Meets`], or [`CutMeet::Floats`] when a facet reaches no target.
//!
//! Rules 1 and 3, and rule 2 without its exception, are the fantasy-cut template's; the table
//! in rule 3 and rule 4 make the teaching templates meet-point designs.
//!
//! # Determinism
//!
//! Every decision reads sorted vectors (corners by position, tiers by cutting order) and takes
//! the first of equals; nothing iterates a hash map.

use self::geometry::{StoneContacts, closing_box, ring_bounds, weld};
use super::{ConstraintTier, Design, TierRef};
use crate::manufacturability::facet_plane_boundaries;
use glam::DVec3;
use pleochroma::geometry::{
    meet_solver::{MeetConstraint, SolvedTier},
    plane::tier_is_crown_side,
    stone_metrics::{SolidStatus, build_solid_mesh},
};

/// A vertex closer to a facet plane than this fraction of the stone's size is on it.
///
/// That is 0.5 µm at 10 mm. Loose enough for 5-decimal `GemCad` masts, which open meets by up to
/// 2e-5 of the stone.
pub const MEET_TOLERANCE_FRACTION: f64 = 5e-5;

/// How far [`Design::snap_depths_to_meets`] may move a tier by default.
///
/// A fraction of the stone's size, as [`MEET_TOLERANCE_FRACTION`] is: 0.2 mm at 10 mm.
pub const DEFAULT_SNAP_MAX_MOVE_FRACTION: f64 = 0.02;

/// Two facet corners closer than this fraction of the stone's size are one vertex. The solid
/// mesh already welds its vertices (faces that meet in a vertex carry the same position), so
/// this only absorbs rounding.
const SHARED_VERTEX_FRACTION: f64 = 1e-6;

/// A vertex closer to the stone's axis than this fraction of the stone's size lies on the
/// axis, for "Cut to centerpoint.". Looser than [`SHARED_VERTEX_FRACTION`]: the apex where
/// a tier's facets meet is solved from `f32` facet normals, so it sits a few `1e-7` of the
/// size off the axis; `1e-4` of a 7 mm stone is under a micrometre.
const ON_AXIS_FRACTION: f64 = 1e-4;

/// How a flat tier's depth is found at the moment it is cut, on the stone cut so far.
///
/// The preform closes the partial solid, but only vertices formed by facets of tiers cut
/// EARLIER count as points a cutter can cut to (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub enum CutMeet {
    /// The tier fixes a dimension instead of reaching a point.
    Anchor(CutMeetAnchor),
    /// Every facet of the tier, at its solved depth, passes (within the tolerance) through a
    /// vertex already on the stone.
    ///
    /// The tiers whose facets form those vertices, as indices into `Design::tiers`, in cutting
    /// order, without duplicates. Never empty.
    Meets(Vec<usize>),
    /// The stone cut so far has vertices, but at least one facet of the tier reaches none of
    /// them, so the cutter has no point to stop at.
    Floats {
        /// Distance from that facet's plane to the nearest such vertex, in design (mast) units.
        /// Larger than the tolerance.
        miss: f64,
        /// The tiers forming that nearest vertex, in cutting order.
        nearest: Vec<usize>,
    },
}

/// Which dimension an anchor tier ([`CutMeet::Anchor`]) fixes.
///
/// See the module docs for the order in which the anchors are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutMeetAnchor {
    /// [`Design::sizing_tier`].
    SetStoneSize,
    /// The first tier in cutting order whose facets (at least three) all end in one point on the
    /// axis, unless it meets points already on the stone. The rule of the cutting sheet's
    /// "Cut to centerpoint.", which is the same tier.
    CutToCenterpoint,
    /// The first crown tier in cutting order (the table excepted), or the table when the design
    /// has no other flat crown tier, when its facets reach the girdle: the cutting sheet's
    /// "Set girdle width.".
    SetGirdleWidth,
    /// The table, when the stone cut so far has no point a table could meet (every point is on
    /// the axis, where crown mains meet in one point, or on the girdle line) and it meets none:
    /// its depth sets the size of the table, "Set table size.".
    SetTableSize,
    /// A pavilion tier cut while the earlier tiers form no vertex yet, whose facets reach a
    /// girdle tier cut before it ("Set girdle line.", as in `GemCad` designs cut girdle-first).
    SetGirdleLine,
    /// Any other tier cut while the earlier tiers form no vertex yet (for example the first
    /// tier).
    NothingToMeet,
}

/// [`Design::cut_meets`]'s answer.
///
/// One [`CutMeet`] per flat tier, and the size the tolerance is measured against.
#[derive(Debug, Clone, PartialEq)]
pub struct CutMeets {
    /// One entry per `Design::tiers` (flat tiers), by tier index.
    pub tiers: Vec<CutMeet>,
    /// The finished stone's size (its largest extent), in design units. The tolerance is
    /// `MEET_TOLERANCE_FRACTION * size`.
    pub size: f64,
}

impl CutMeets {
    /// The floating tiers in cutting order, with their miss.
    ///
    /// Each entry is a tier (an index into `design.tiers`) whose meet is [`CutMeet::Floats`],
    /// with that miss in design units. `design` is the design `self` was worked out for; it
    /// gives the cutting order.
    #[must_use]
    pub fn floating(&self, design: &Design) -> Vec<(usize, f64)> {
        design
            .cutting_order()
            .into_iter()
            .filter_map(|tier_ref| {
                let TierRef::Flat(tier) = tier_ref else {
                    return None;
                };
                match self.tiers.get(tier)? {
                    CutMeet::Floats { miss, .. } => Some((tier, *miss)),
                    CutMeet::Anchor(_) | CutMeet::Meets(_) => None,
                }
            })
            .collect()
    }
}

/// One tier [`Design::snap_depths_to_meets`] moved.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapMove {
    /// The tier, as an index into `Design::tiers`.
    pub tier: usize,
    /// Its explicit depth before the snap (the `MeetConstraint::ScaleReference` mast).
    pub old_mast: f64,
    /// Its explicit depth after the snap, stored in its `MeetConstraint::ScaleReference`.
    pub new_mast: f64,
    /// The tiers whose facets form the points its facets pass through, in cutting order.
    /// Never empty.
    pub meets: Vec<usize>,
}

/// What [`Design::snap_depths_to_meets`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SnapReport {
    /// The moved tiers, in cutting order.
    pub moved: Vec<SnapMove>,
    /// Tiers that still float afterwards, in cutting order: the nearest point was farther than
    /// `max_move`, the tier is not an explicit-depth (`ScaleReference`) tier, or its facets
    /// cannot all reach one depth.
    pub still_floating: Vec<usize>,
}

impl Design {
    /// The cut-so-far meet of every flat tier.
    ///
    /// `None` when `solved` is not aligned with `self.tiers` or the finished stone does not
    /// close. See the module docs for the rules. Builds one solid per tier (the stone cut
    /// before it), so a caller with a large design runs it off the UI thread.
    #[must_use]
    pub fn cut_meets(&self, solved: &[SolvedTier]) -> Option<CutMeets> {
        let walk = CutWalk::new(self, solved)?;
        let mut tiers = vec![CutMeet::Anchor(CutMeetAnchor::NothingToMeet); self.tiers.len()];
        for (position, &tier) in walk.order.iter().enumerate() {
            if let Some(slot) = tiers.get_mut(tier) {
                *slot = walk.meet_at(position).0;
            }
        }
        Some(CutMeets {
            tiers,
            size: walk.size,
        })
    }

    /// Moves each floating explicit-depth tier onto the nearest meet point within `max_move`.
    ///
    /// Walks the cutting order. Each floating explicit-depth ([`MeetConstraint::ScaleReference`])
    /// tier gets the mast that puts its plane through the nearest vertex of the stone cut so far
    /// (with the earlier moves applied), when that is at most `max_move` design units away.
    /// Anchors are never moved. The caller re-solves afterwards. Returns `None` when `solved` is
    /// not aligned or the stone does not close.
    ///
    /// - A tier whose facets are not all the same distance from their nearest point (an
    ///   asymmetric tier) moves only to a mast that puts every facet on a point within the
    ///   tolerance; of the masts that do, the one closest to its own wins. Otherwise it stays in
    ///   [`SnapReport::still_floating`].
    /// - A tier with an authored target (`Design::tier_target`) is not moved: the target, not
    ///   the mast, sets its depth.
    /// - Moving one tier can change what the tiers cut after it meet, which the walk sees: a
    ///   tier that met before can float afterwards and be moved in turn, and one that floated can
    ///   come to meet without moving.
    ///
    /// # Undo
    ///
    /// This writes `self.tiers[..].constraint` directly, outside [`crate::edit::History`]. An
    /// editor runs it on a clone of its design and applies the report through its history as one
    /// [`crate::edit::Edit::Batch`] of [`crate::edit::Edit::SetConstraint`] edits, one per
    /// [`SnapMove`] (`constraint: MeetConstraint::ScaleReference(new_mast)`), so the snap is one
    /// undo step. The moved masts round-trip through the `.psdd` file like any other explicit
    /// depth (its tier records store `ScaleReference` masts as plain floats).
    #[must_use]
    pub fn snap_depths_to_meets(
        &mut self,
        solved: &[SolvedTier],
        max_move: f64,
    ) -> Option<SnapReport> {
        let mut walk = CutWalk::new(self, solved)?;
        let mut working = solved.to_vec();
        let mut report = SnapReport::default();
        // The walk's facets change as tiers move; the order does not.
        let order = walk.order.clone();
        for (position, &tier) in order.iter().enumerate() {
            let (meet, targets) = walk.meet_at(position);
            if !matches!(meet, CutMeet::Floats { .. }) {
                continue;
            }
            let moved = self.snap_tier(&mut walk, &mut working, tier, &targets, max_move);
            if moved.is_none() {
                report.still_floating.push(tier);
            }
            report.moved.extend(moved);
        }
        Some(report)
    }

    /// Moves floating `tier` onto the mast [`snap_offset`] finds against `targets` (the stone cut
    /// before it), keeping `working` (the masts the walk builds the stone from) and `walk`'s
    /// facets in step. `None`, changing nothing, when the tier is not a movable explicit-depth
    /// tier or no mast within `max_move` reaches a point with every facet.
    fn snap_tier(
        &mut self,
        walk: &mut CutWalk,
        working: &mut [SolvedTier],
        tier: usize,
        targets: &[Target],
        max_move: f64,
    ) -> Option<SnapMove> {
        let MeetConstraint::ScaleReference(old_mast) = self.tiers.get(tier)?.constraint else {
            return None;
        };
        if self.tier_target(tier).is_some() {
            return None;
        }
        let offset = snap_offset(walk.facets.get(tier)?, targets, walk.tolerance(), max_move)?;
        // A plane's offset is the mast's magnitude (`StandardGemCuts::from_asc_schedule`), so the
        // mast keeps its sign.
        let new_mast = offset.copysign(old_mast);
        self.set_explicit_depth(working, tier, new_mast);
        walk.refresh_facets(self, working);
        if let Reach::Meets(meets) = walk.reach(tier, targets) {
            return Some(SnapMove {
                tier,
                old_mast,
                new_mast,
                meets,
            });
        }
        // The plane, rounded to the `f32` the solid is built from, missed after all: put the tier
        // back.
        self.set_explicit_depth(working, tier, old_mast);
        walk.refresh_facets(self, working);
        None
    }

    /// Sets `tier`'s explicit depth to `mast`, in its constraint and in `working`.
    fn set_explicit_depth(&mut self, working: &mut [SolvedTier], tier: usize, mast: f64) {
        if let Some(slot) = self.tiers.get_mut(tier) {
            slot.constraint = MeetConstraint::ScaleReference(mast);
        }
        if let Some(slot) = working.get_mut(tier) {
            slot.mast = mast;
        }
    }
}

/// The offset (a plane's distance from the origin along its normal) that puts every one of
/// `facets` within `tolerance` of a target, the closest such offset to the facets' own, when it
/// is positive and at most `max_move` away; else `None`.
///
/// Every facet of a tier has the same offset (the tier's mast), so the candidates are the
/// offsets that put some facet exactly through some target. They are tried closest first (the
/// larger offset, which removes less, first among equals), and the first that works for every
/// facet wins.
fn snap_offset(
    facets: &[(DVec3, f64)],
    targets: &[Target],
    tolerance: f64,
    max_move: f64,
) -> Option<f64> {
    let &(_, current) = facets.first()?;
    let projections: Vec<Vec<f64>> = facets
        .iter()
        .map(|&(normal, _)| {
            let mut along: Vec<f64> = targets
                .iter()
                .map(|target| normal.dot(target.point))
                .collect();
            along.sort_by(f64::total_cmp);
            along
        })
        .collect();
    let mut candidates: Vec<f64> = projections
        .iter()
        .flatten()
        .copied()
        .filter(|&offset| offset > 0.0 && (offset - current).abs() <= max_move)
        .collect();
    candidates.sort_by(|a, b| {
        (a - current)
            .abs()
            .total_cmp(&(b - current).abs())
            .then_with(|| b.total_cmp(a))
    });
    candidates.into_iter().find(|&offset| {
        projections.iter().all(|along| {
            let first = along.partition_point(|&value| value < offset - tolerance);
            along
                .get(first)
                .is_some_and(|&value| value <= offset + tolerance)
        })
    })
}

/// A corner of the stone cut so far that a cutter can cut to: every face incident at it is a
/// facet of a tier cut earlier.
struct Target {
    /// Where it is.
    point: DVec3,
    /// The tiers whose facets meet there, in cutting order, without duplicates.
    tiers: Vec<usize>,
    /// The facets that meet there, as `(tier, facet index within that tier's facet planes)`, in
    /// cutting order and then facet order, without duplicates.
    planes: Vec<(usize, usize)>,
}

/// How the facets of a tier reach the targets of the stone cut before it.
enum Reach {
    /// The stone cut so far has no target (or the tier has no facet plane).
    Nothing,
    /// Every facet passes through a target: the tiers forming those targets, in cutting order.
    Meets(Vec<usize>),
    /// At least one facet passes through none: the largest distance from a facet to its nearest
    /// target, and the tiers forming that target.
    Floats {
        /// The distance, in design units.
        miss: f64,
        /// The tiers, in cutting order.
        nearest: Vec<usize>,
    },
}

/// The tiers that fix a dimension, worked out once from the finished stone.
struct Anchors {
    /// [`Design::sizing_tier`].
    sizing_tier: Option<usize>,
    /// The first tier in cutting order whose facets all end in one point on the axis.
    centerpoint_tier: Option<usize>,
    /// The first crown tier in cutting order (the table excepted), or the table when the design
    /// has no other flat crown tier, when one of its facets shares a vertex with a girdle tier's
    /// (or the design has no girdle tier, so the preform's walls are the girdle).
    girdle_width_tier: Option<usize>,
    /// Per tier (index into `Design::tiers`): a pavilion tier whose facets share a vertex, in
    /// the finished stone, with a girdle tier cut before it.
    reaches_earlier_girdle: Vec<bool>,
    /// Per tier: the table ([`ConstraintTier::is_table`]).
    table: Vec<bool>,
    /// Per tier: a crown-side tier that is not a girdle (the table included), whose vertices
    /// off the axis a table could meet.
    crown: Vec<bool>,
}

impl Anchors {
    /// The anchors of `design` cut in `order` (its flat tiers in cutting order), from the
    /// finished stone's `contacts`, by the module docs' precedence rules.
    fn of(design: &Design, order: &[usize], contacts: &StoneContacts) -> Self {
        let tiers = &design.tiers;
        let centerpoint_tier = order
            .iter()
            .copied()
            .find(|&tier| contacts.ends_in_axis_point(tier));
        let girdles: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&tier| tiers[tier].is_girdle())
            .collect();
        // A design with no flat crown tier but the table (pavilion mains, girdle, table) sets the
        // girdle width with the table.
        let girdle_width_tier = order
            .iter()
            .copied()
            .find(|&tier| {
                let flat = &tiers[tier];
                !flat.is_table() && !flat.is_girdle() && tier_is_crown_side(flat.angle_deg)
            })
            .or_else(|| order.iter().copied().find(|&tier| tiers[tier].is_table()))
            .filter(|&crown| {
                girdles.is_empty() || girdles.iter().any(|&girdle| contacts.touch(crown, girdle))
            });
        let mut reaches_earlier_girdle = vec![false; tiers.len()];
        for (position, &tier) in order.iter().enumerate() {
            let flat = &tiers[tier];
            let pavilion = !tier_is_crown_side(flat.angle_deg) && !flat.is_girdle();
            reaches_earlier_girdle[tier] = pavilion
                && order[..position]
                    .iter()
                    .any(|&earlier| tiers[earlier].is_girdle() && contacts.touch(tier, earlier));
        }
        Self {
            sizing_tier: design.sizing_tier(),
            centerpoint_tier,
            girdle_width_tier,
            reaches_earlier_girdle,
            table: tiers.iter().map(ConstraintTier::is_table).collect(),
            crown: tiers
                .iter()
                .map(|flat| tier_is_crown_side(flat.angle_deg) && !flat.is_girdle())
                .collect(),
        }
    }
}

/// One design's stone, tier by tier as it is cut: what [`Design::cut_meets`] and
/// [`Design::snap_depths_to_meets`] share, built once by [`CutWalk::new`].
struct CutWalk {
    /// The flat tiers in cutting order, as indices into `Design::tiers`.
    order: Vec<usize>,
    /// Per tier (index into `Design::tiers`), its position in `order`.
    position: Vec<usize>,
    /// The planes that close the stone cut so far: the preform's, else a box around the stone.
    closing: Vec<(DVec3, f64)>,
    /// Per tier (index into `Design::tiers`), its facet planes as the finished stone has them
    /// (cheater offsets applied, duplicates of earlier planes dropped).
    facets: Vec<Vec<(DVec3, f64)>>,
    /// The finished stone's size, its largest extent.
    size: f64,
    /// The anchor tiers.
    anchors: Anchors,
}

impl CutWalk {
    /// The walk over `design` with its `solved` masts. `None` when `solved` is not aligned with
    /// `design.tiers` or the finished stone does not close.
    fn new(design: &Design, solved: &[SolvedTier]) -> Option<Self> {
        if solved.len() != design.tiers.len() {
            return None;
        }
        let tier_count = design.tiers.len();
        let (planes, owner) = owned_planes(design, solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return None;
        };
        let (lo, hi) = ring_bounds(&mesh);
        let extent = (hi - lo).max_element();
        let size = if extent.is_finite() && extent > 0.0 {
            extent
        } else {
            1.0
        };
        let contacts = StoneContacts::of(&mesh, &owner, tier_count, size);
        let (mut closing, facets) = split_planes(&planes, &owner, tier_count);
        if closing.is_empty() {
            closing = closing_box(lo, hi, size);
        }
        let order: Vec<usize> = design
            .cutting_order()
            .into_iter()
            .filter_map(|tier_ref| match tier_ref {
                TierRef::Flat(tier) => Some(tier),
                TierRef::Concave(_) => None,
            })
            .collect();
        let mut position = vec![usize::MAX; tier_count];
        for (at, &tier) in order.iter().enumerate() {
            if let Some(slot) = position.get_mut(tier) {
                *slot = at;
            }
        }
        let anchors = Anchors::of(design, &order, &contacts);
        Some(Self {
            order,
            position,
            closing,
            facets,
            size,
            anchors,
        })
    }

    /// The distance within which a facet plane passes through a vertex.
    const fn tolerance(&self) -> f64 {
        MEET_TOLERANCE_FRACTION * self.size
    }

    /// Where `tier` stands in the cutting order (`usize::MAX` for a tier that is not flat).
    fn cutting_position(&self, tier: usize) -> usize {
        self.position.get(tier).copied().unwrap_or(usize::MAX)
    }

    /// Sorts `tiers` in cutting order and drops duplicates.
    fn sort_in_cutting_order(&self, tiers: &mut Vec<usize>) {
        tiers.sort_by_key(|&tier| self.cutting_position(tier));
        tiers.dedup();
    }

    /// Whether `targets` hold a point a table could meet: a crown vertex above the girdle line,
    /// that is, off the axis (farther than [`ON_AXIS_FRACTION`] of the size) with only crown
    /// facets at it. The apex of crown mains meeting in one point is on the axis, and a corner
    /// with a girdle's or a pavilion tier's facet at it lies on or below the girdle line.
    fn has_table_point(&self, targets: &[Target]) -> bool {
        let on_axis = ON_AXIS_FRACTION * self.size;
        targets.iter().any(|target| {
            target.point.x.hypot(target.point.z) > on_axis
                && target
                    .tiers
                    .iter()
                    .all(|&tier| self.anchors.crown.get(tier).copied().unwrap_or(false))
        })
    }

    /// The meet of the tier at `position` of the cutting order, and the targets of the stone
    /// cut before it (empty for a tier whose anchor needs no stone: the sizing tier and the
    /// girdle-width tier).
    fn meet_at(&self, position: usize) -> (CutMeet, Vec<Target>) {
        let Some(&tier) = self.order.get(position) else {
            return (CutMeet::Anchor(CutMeetAnchor::NothingToMeet), Vec::new());
        };
        let anchors = &self.anchors;
        let fixed = anchors.sizing_tier == Some(tier)
            || (anchors.girdle_width_tier == Some(tier) && anchors.centerpoint_tier != Some(tier));
        if fixed {
            return (self.classify(tier, Reach::Nothing, false), Vec::new());
        }
        let targets = self.targets(position);
        let reach = self.reach(tier, &targets);
        let table_without_point =
            anchors.table.get(tier).copied().unwrap_or(false) && !self.has_table_point(&targets);
        (self.classify(tier, reach, table_without_point), targets)
    }

    /// The meet of `tier` from how its facets `reach` the stone cut so far, by the module
    /// docs' precedence. `table_without_point`: the tier is the table and that stone has no
    /// point a table could meet ([`Self::has_table_point`]).
    fn classify(&self, tier: usize, reach: Reach, table_without_point: bool) -> CutMeet {
        let anchors = &self.anchors;
        if anchors.sizing_tier == Some(tier) {
            return CutMeet::Anchor(CutMeetAnchor::SetStoneSize);
        }
        let meets = matches!(reach, Reach::Meets(_));
        if anchors.centerpoint_tier == Some(tier) && !meets {
            return CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint);
        }
        if anchors.girdle_width_tier == Some(tier) {
            return CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth);
        }
        if table_without_point && !meets {
            return CutMeet::Anchor(CutMeetAnchor::SetTableSize);
        }
        match reach {
            Reach::Nothing => {
                let girdle_line = anchors
                    .reaches_earlier_girdle
                    .get(tier)
                    .copied()
                    .unwrap_or(false);
                CutMeet::Anchor(if girdle_line {
                    CutMeetAnchor::SetGirdleLine
                } else {
                    CutMeetAnchor::NothingToMeet
                })
            }
            Reach::Meets(tiers) => CutMeet::Meets(tiers),
            Reach::Floats { miss, nearest } => CutMeet::Floats { miss, nearest },
        }
    }

    /// The targets of the stone cut before the tier at `position` of the cutting order: the
    /// solid of the earlier tiers' facets and the closing planes, one mesh. Empty for the first
    /// tier.
    fn targets(&self, position: usize) -> Vec<Target> {
        let earlier = self.order.get(..position).unwrap_or_default();
        if earlier.is_empty() {
            return Vec::new();
        }
        let mut planes = self.closing.clone();
        // Per plane: the facet it is, `(tier, facet index)`, or `None` for a closing plane.
        let mut owner: Vec<Option<(usize, usize)>> = vec![None; planes.len()];
        for &tier in earlier {
            if let Some(facets) = self.facets.get(tier) {
                planes.extend_from_slice(facets);
                owner.extend((0..facets.len()).map(|facet| Some((tier, facet))));
            }
        }
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return Vec::new();
        };
        let mut corners: Vec<(DVec3, Option<(usize, usize)>)> = mesh
            .rings
            .iter()
            .flat_map(|(plane, ring)| {
                let face = owner.get(*plane).copied().flatten();
                ring.iter().map(move |&point| (point, face))
            })
            .collect();
        weld(&mut corners, SHARED_VERTEX_FRACTION * self.size)
            .into_iter()
            .filter_map(|(point, owners)| {
                // A corner on a closing face is no target: `collect` into `Option` gives `None`.
                let mut planes = owners
                    .into_iter()
                    .collect::<Option<Vec<(usize, usize)>>>()?;
                planes.sort_by_key(|&(tier, facet)| (self.cutting_position(tier), facet));
                planes.dedup();
                // Sorted by cutting position, so a tier's facets stand together.
                let mut tiers: Vec<usize> = planes.iter().map(|&(tier, _)| tier).collect();
                tiers.dedup();
                Some(Target {
                    point,
                    tiers,
                    planes,
                })
            })
            .collect()
    }

    /// How the facets of `tier`, at their current depth, reach `targets`.
    ///
    /// A facet meets when a target lies within the tolerance of its plane; the meets are the
    /// tiers of every such target of every facet. A facet's miss is the distance to its nearest
    /// target (the first of equals); the tier floats when the largest miss (the first facet of
    /// equals) exceeds the tolerance.
    fn reach(&self, tier: usize, targets: &[Target]) -> Reach {
        let Some(facets) = self.facets.get(tier).filter(|facets| !facets.is_empty()) else {
            return Reach::Nothing;
        };
        if targets.is_empty() {
            return Reach::Nothing;
        }
        let tolerance = self.tolerance();
        let mut met: Vec<usize> = Vec::new();
        let mut worst: Option<(f64, &Target)> = None;
        for &(normal, offset) in facets {
            let mut nearest: Option<(f64, &Target)> = None;
            for target in targets {
                let distance = (normal.dot(target.point) - offset).abs();
                if distance <= tolerance {
                    met.extend_from_slice(&target.tiers);
                }
                if nearest.is_none_or(|(best, _)| distance < best) {
                    nearest = Some((distance, target));
                }
            }
            if let Some((distance, target)) = nearest
                && worst.is_none_or(|(most, _)| distance > most)
            {
                worst = Some((distance, target));
            }
        }
        if let Some((miss, target)) = worst.filter(|&(miss, _)| miss > tolerance) {
            return Reach::Floats {
                miss,
                nearest: target.tiers.clone(),
            };
        }
        self.sort_in_cutting_order(&mut met);
        Reach::Meets(met)
    }

    /// Rebuilds every tier's facet planes from `solved` (after a snap moved a mast): the
    /// planes of later tiers can depend on an earlier one through the schedule's duplicate
    /// dropping, so all of them are rebuilt.
    fn refresh_facets(&mut self, design: &Design, solved: &[SolvedTier]) {
        let (planes, owner) = owned_planes(design, solved);
        self.facets = split_planes(&planes, &owner, design.tiers.len()).1;
    }
}

/// The finished stone's planes ([`Design::planes_from_solved`]: the preform's first, then each
/// tier's facets) and the tier owning each one, `None` for a preform plane. Each face goes to
/// the tier that cut it the way [`Design::tier_for_plane_index`] gives it.
///
/// `solved` must be aligned with `design.tiers` (the caller checks it).
fn owned_planes(design: &Design, solved: &[SolvedTier]) -> (Vec<(DVec3, f64)>, Vec<Option<usize>>) {
    let planes = design.planes_from_solved(solved);
    let preform_len = design.preform.planes().len().min(planes.len());
    let boundaries = facet_plane_boundaries(&design.to_asc_schedule_from_solved(solved));
    let owner = (0..planes.len())
        .map(|plane| {
            let local = plane.checked_sub(preform_len)?;
            let tier = boundaries.partition_point(|&end| end <= local);
            (tier < boundaries.len()).then_some(tier)
        })
        .collect();
    (planes, owner)
}

/// Planes as `(outward normal, offset)` pairs: one tier's facets, or the planes that close the
/// stone cut so far.
type Planes = Vec<(DVec3, f64)>;

/// `planes` split by `owner` ([`owned_planes`]): the planes no tier owns (the preform's), and
/// each of `tier_count` tiers' facet planes, in plane order.
fn split_planes(
    planes: &[(DVec3, f64)],
    owner: &[Option<usize>],
    tier_count: usize,
) -> (Planes, Vec<Planes>) {
    let mut closing = Vec::new();
    let mut facets: Vec<Planes> = vec![Vec::new(); tier_count];
    for (&plane, &tier) in planes.iter().zip(owner) {
        tier.and_then(|tier| facets.get_mut(tier))
            .unwrap_or(&mut closing)
            .push(plane);
    }
    (closing, facets)
}

mod geometry;
mod reanchor;

pub use reanchor::MeetTarget;

#[cfg(test)]
mod tests;
