//! What the cutter can change about one planned stone, and the single-piece re-fit.
//!
//! # A stone's choice
//!
//! [`StoneChoice`] is the planner's per-stone override, the generalisation of the pose the
//! color wizard stores (`pose`: which of the four box-symmetric poses, `0` the planner's own):
//!
//! - `pose`: one of the four poses of a box stone (a half turn about one of its own axes). The
//!   piece, the stone's box and the yield do not change; only the axis angle does.
//! - `free_pose`: a pose the cutter placed by hand for a single-stone exact fit ("Free
//!   Orientation"; `rescale_pose` re-scales the stone for it at once).
//! - `locked`: the stone is kept; a re-plan leaves its piece out (see `lock`).
//! - `reason`: why the cutter overrode the rule, shown as "overridden (reason)" and printed on
//!   the cutting sheet.
//!
//! # Re-fit This Piece
//!
//! Another table axis is not a pose of the same box (it changes the stone's box, so the piece
//! holds a different stone): [`refit_block_piece`] and [`refit_shaped_piece`] answer with the
//! best stone of a design in the SAME piece under the chosen assignment, for the layout builder
//! to replace the old stone with and to recompute its totals.

use crate::rough_plan::{
    CandidateDesign, PlacedStone, PlanSettings,
    fit::StonePose,
    piece::{Norm, best_pick, stone_value},
    placement::pose_variant,
    shaped::{ShapedCtx, tree::StoneFitter},
    tree::{Leaf, orient_of_pose, place_stone},
    types::RoughLayout,
};

/// The cutter's choice for one stone (all defaults: no choice).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoneChoice {
    /// The box-symmetric pose, `0..POSE_COUNT`; `0` is the planner's own.
    pub pose: u8,
    /// A pose placed by hand (an exact single-stone fit only); it replaces `pose`.
    pub free_pose: Option<StonePose>,
    /// The stone is locked: the next plan keeps its piece out.
    pub locked: bool,
    /// Why the cutter overrode the rule; empty when no reason was given.
    pub reason: String,
}

impl StoneChoice {
    /// Whether the choice changes nothing (and so is not stored).
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.pose == 0 && self.free_pose.is_none() && !self.locked && self.reason.is_empty()
    }

    /// Whether the stone is set up differently from the planner's pose (the rows then say
    /// "overridden"); a lock or a reason alone does not.
    #[must_use]
    pub const fn is_overridden(&self) -> bool {
        self.pose != 0 || self.free_pose.is_some()
    }

    /// The pose the stone actually has: the free pose, else the box-symmetric pose `pose` of
    /// the planner's `base` (the planner's own for an index that does not exist).
    #[must_use]
    pub fn pose_for(&self, base: &StonePose) -> StonePose {
        self.free_pose
            .or_else(|| pose_variant(base, self.pose))
            .unwrap_or(*base)
    }
}

/// The pose index that puts each stone of `layout` nearest its design's azimuth target.
///
/// It applies to the rules that have one (a Custom Angle with an azimuth); [`apply_suggested_poses`]
/// turns the layout's stones into it. `designs` are the candidates the layout was planned with (they carry the resolved
/// rules). `0` for every stone whose rule does not depend on the azimuth, whose design is not
/// in `designs`, and for an exact single-stone fit (its pose is unique).
#[must_use]
pub fn suggested_poses(layout: &RoughLayout, designs: &[CandidateDesign]) -> Vec<u8> {
    layout
        .stones
        .iter()
        .map(|stone| {
            if layout.exact_fit {
                return 0;
            }
            designs
                .iter()
                .find(|design| design.entry_id == stone.entry_id)
                .map_or(0, |design| suggested_pose_index(stone, design))
        })
        .collect()
}

/// The pose index that puts `stone` (in the planner's own form) nearest `design`'s azimuth
/// target; `0` for a rule that does not depend on the azimuth and for a stone already turned.
fn suggested_pose_index(stone: &PlacedStone, design: &CandidateDesign) -> u8 {
    match (design.orientation, orient_of_pose(&stone.pose)) {
        (Some(rule), Some(orient)) if rule.needs_azimuth() => rule.assignment_poses()[orient],
        _ => 0,
    }
}

/// `stone`, a fresh fit of `design`, turned into the box pose its rule credited, as
/// [`apply_suggested_poses`] does for a planned layout.
fn with_suggested_pose(mut stone: PlacedStone, design: &CandidateDesign) -> PlacedStone {
    let index = suggested_pose_index(&stone, design);
    if index != 0
        && let Some(turned) = pose_variant(&stone.pose, index)
    {
        stone.pose = turned;
    }
    stone
}

/// Turns every stone of `layout` into the box pose its rule credited ([`suggested_poses`]).
///
/// The layout takes the pose the search valued: the rows, the hand-off and the sheet then read
/// the stone's true orientation and nothing is recorded as a cutter's choice.
///
/// It touches box stones only (an exact single-stone fit has one pose) and is safe to call again:
/// a turned pose is no longer in the planner's own form (`orient_of_pose` is `None`), so its
/// suggested index is `0` and it stays as it is.
pub fn apply_suggested_poses(layout: &mut RoughLayout, designs: &[CandidateDesign]) {
    let suggested = suggested_poses(layout, designs);
    for (stone, index) in layout.stones.iter_mut().zip(suggested) {
        if index == 0 {
            continue;
        }
        if let Some(turned) = pose_variant(&stone.pose, index) {
            stone.pose = turned;
        }
    }
}

/// The best stone of `design` in the piece of `stone`, a sawn piece of a block rough.
///
/// It is under assignment `orient` (an index into the six assignments; the table faces rough axis
/// `ASSIGNMENTS[orient][2]`), or the best assignment its rule allows when `orient` is `None`.
///
/// The piece (origin and size) stays; the stone, its pose and its weight are new. An explicit
/// `orient` is the cutter's choice and ignores the design's rule. `None` when the stone would
/// be narrower than the minimum width, or `orient` is not one of the six.
#[must_use]
pub fn refit_block_piece(
    stone: &PlacedStone,
    design: &CandidateDesign,
    orient: Option<usize>,
    settings: &PlanSettings,
) -> Option<PlacedStone> {
    let a2 = 2.0 * settings.allowance_mm;
    let usable = stone.piece_size_mm.map(|s| s - a2);
    let automatic = orient.is_none();
    let orient = match orient {
        Some(orient) if orient < 6 => {
            let plain = Norm::plain(design);
            if stone_value(&plain, orient, usable, settings.min_width_mm) <= f64::NEG_INFINITY {
                return None;
            }
            orient
        }
        Some(_) => return None,
        None => best_pick(&[Norm::of(design)], usable, settings.min_width_mm)?.1,
    };
    let leaf = Leaf {
        len: 0.0,
        design: 0,
        orient,
    };
    let placed = place_stone(
        settings,
        std::slice::from_ref(design),
        &leaf,
        stone.piece_origin_mm,
        stone.piece_size_mm,
        usable,
    );
    // Under the rule (no explicit assignment) the stone takes the pose the rule credited, as
    // the planned layouts do; an explicit assignment is the cutter's own and stays as placed.
    Some(if automatic {
        with_suggested_pose(placed, design)
    } else {
        placed
    })
}

/// [`refit_block_piece`] for a shaped rough.
///
/// The piece of `stone` is clipped by the rough's planes (and its mesh), so the stone is solved by
/// the single-piece LP of the shaped planner (`StoneFitter`). `None` when no stone of at least the minimum width fits the piece under the
/// assignment (or under any assignment the rule allows, for `orient == None`).
#[must_use]
pub fn refit_shaped_piece(
    ctx: &ShapedCtx,
    settings: &PlanSettings,
    design: &CandidateDesign,
    stone: &PlacedStone,
    orient: Option<usize>,
) -> Option<PlacedStone> {
    let pool = std::slice::from_ref(design);
    let mut fitter = StoneFitter::new(settings, pool, &ctx.non_box).with_mesh(ctx.fit_mesh());
    let mut fit = |orient: usize| {
        fitter.fit(
            stone.piece_origin_mm,
            stone.piece_size_mm,
            &Leaf {
                len: 0.0,
                design: 0,
                orient,
            },
        )
    };
    match orient {
        Some(orient) if orient < 6 => fit(orient),
        Some(_) => None,
        None => {
            let norm = Norm::of(design);
            let mut best: Option<PlacedStone> = None;
            for orient in 0..6 {
                if norm.factors[orient] <= 0.0 {
                    continue;
                }
                if let Some(found) = fit(orient)
                    && best
                        .as_ref()
                        .is_none_or(|b| found.volume_mm3 > b.volume_mm3)
                {
                    best = Some(found);
                }
            }
            // Under the rule the stone takes the pose the rule credited (see
            // `refit_block_piece`).
            best.map(|found| with_suggested_pose(found, design))
        }
    }
}
