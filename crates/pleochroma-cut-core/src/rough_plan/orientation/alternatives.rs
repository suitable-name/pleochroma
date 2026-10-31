//! The alternatives of a Preferred rule: best yield, best orientation, and the balance between.
//!
//! A Preferred rule trades yield against orientation, so one plan cannot show both ends. The
//! piece table stores ONE argmax per piece, so a different weight is a different table: the three
//! layouts are three table builds and three DPs of ONE cut order (the best layout's, the
//! `plan_alternatives_lanes` pattern), not three full plans.
//!
//! - **Best Yield**: the rule's weight is zero (no design carries a rule); the plan the rule
//!   would give if it did not exist.
//! - **Best Orientation**: every Preferred rule is held as Required at its tolerance, so every
//!   stone is within the tolerance (a Required rule or a design without a rule stays as it is).
//! - **Balanced**: the plan's own weight (the knee between the two).
//!
//! Cost: about 60 to 100 percent of the block stage again with a Preferred rule; nothing with no
//! rule or only Required rules (the function then returns no alternatives: the plan itself is the
//! only layout of its kind). The owner measures.
//!
//! The result is deterministic: the same inputs give the same layouts, whatever the lane count
//! (`lanes` only spreads the table builds over threads).

use crate::rough_plan::{
    CandidateDesign, CutOrder, LayoutCaps, PlanProgress, RoughBlock, RoughLayout,
    build_piece_table_lanes, choose_grid,
    pareto::{pareto_front, sanitize},
    plan::{PlanInput, PlanPath},
    plan_design_cap, plan_rough_for_order,
    rank::best_layout,
    shaped::{
        BuildClipParams, ShapedAltParams, ShapedCtx, build_clipped_table, plan_shaped_design_cap,
        plan_shaped_for_order, rule::plan_shaped_round,
    },
};

use super::{choices::apply_suggested_poses, resolved::RuleHold};

/// Which end of the trade a layout is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternativeKind {
    /// The most volume: the rule's weight is zero.
    BestYield,
    /// The best orientation: every Preferred rule held as Required at its tolerance.
    BestOrientation,
    /// The plan's own weight, between the two ends.
    Knee,
}

impl AlternativeKind {
    /// The three kinds in the order they are listed.
    pub const ALL: [Self; 3] = [Self::BestYield, Self::BestOrientation, Self::Knee];

    /// The label of the alternative (Title Case).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BestYield => "Best Yield",
            Self::BestOrientation => "Best Orientation",
            Self::Knee => "Balanced",
        }
    }
}

/// One alternative layout.
#[derive(Debug, Clone, PartialEq)]
pub struct OrientationAlternative {
    /// Which end of the trade it is.
    pub kind: AlternativeKind,
    /// The layout (the DP's best layout of the plan's cut order, not refined).
    pub layout: RoughLayout,
}

/// `designs` with the rules of the alternative `kind`.
///
/// None for [`AlternativeKind::BestYield`], every Preferred rule held as Required for
/// [`AlternativeKind::BestOrientation`], and the designs as they are for
/// [`AlternativeKind::Knee`].
#[must_use]
pub fn apply_alternative_rules(
    designs: &[CandidateDesign],
    kind: AlternativeKind,
) -> Vec<CandidateDesign> {
    designs
        .iter()
        .map(|design| {
            let orientation = match kind {
                AlternativeKind::BestYield => None,
                AlternativeKind::BestOrientation => {
                    design.orientation.map(|rule| match rule.hold {
                        RuleHold::Preferred { .. } => rule.as_required(),
                        RuleHold::Required => rule,
                    })
                }
                AlternativeKind::Knee => design.orientation,
            };
            CandidateDesign {
                orientation,
                ..*design
            }
        })
        .collect()
}

/// Whether any design carries a Preferred rule (the only case with alternatives).
fn has_preferred_rule(designs: &[CandidateDesign]) -> bool {
    designs
        .iter()
        .any(|design| design.orientation.is_some_and(|rule| rule.is_preferred()))
}

/// The best layout of `layouts` the caps admit, `None` when none does.
fn best_admitted(layouts: &[RoughLayout], caps: LayoutCaps) -> Option<RoughLayout> {
    let admitted: Vec<RoughLayout> = layouts
        .iter()
        .filter(|layout| caps.admits(layout))
        .cloned()
        .collect();
    best_layout(&admitted).cloned()
}

/// The three alternatives of `input` for the cut order `order` (the order of the plan's best
/// layout).
///
/// Each is a table build and a DP of that one order: the plain block on the unit grid, a shaped
/// rough on its clipped grid. The designs of `input` carry their resolved rules.
///
/// Returns no alternatives when no design has a Preferred rule. A kind whose plan holds no stone
/// is left out. Reports the table and DP events of every round as they come; `None` when
/// `on_progress` cancels.
pub fn plan_orientation_alternatives(
    input: &PlanInput<'_>,
    order: CutOrder,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<OrientationAlternative>> {
    let settings = input.settings;
    if settings.validate().is_err() {
        return Some(Vec::new());
    }
    let clean = sanitize(input.designs);
    if !has_preferred_rule(&clean) {
        return Some(Vec::new());
    }
    let slots = match input.path() {
        PlanPath::PlainBlock => plain_block_slots(input, &clean, order, lanes, on_progress)?,
        PlanPath::Shaped => shaped_slots(input, &clean, order, lanes, on_progress)?,
    };
    Some(
        AlternativeKind::ALL
            .into_iter()
            .zip(slots)
            .filter_map(|(kind, layout)| {
                layout.map(|mut layout| {
                    // Each slot took the pose its own rules credited.
                    apply_suggested_poses(&mut layout, &apply_alternative_rules(&clean, kind));
                    OrientationAlternative { kind, layout }
                })
            })
            .collect(),
    )
}

/// The planner was cancelled through its progress callback.
struct Cancelled;

/// The best layout within the design cap for a shaped round whose best layout `best` uses too
/// many designs: the shaped design-cap chain over `designs` (the round's own).
///
/// The error is the cancel flag, the `Ok(None)` is "no layout within the caps".
fn capped_shaped(
    params: &ShapedAltParams<'_>,
    designs: &[CandidateDesign],
    best: &RoughLayout,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Result<Option<RoughLayout>, Cancelled> {
    let chain = ShapedAltParams {
        all_designs: designs,
        best,
        ..params.clone()
    };
    let groups = plan_shaped_design_cap(&chain, on_progress).ok_or(Cancelled)?;
    let all: Vec<RoughLayout> = groups.into_iter().flat_map(|group| group.layouts).collect();
    Ok(best_admitted(&all, params.settings.caps()))
}

/// The three layout slots of the alternatives, in [`AlternativeKind::ALL`] order.
type Slots = [Option<RoughLayout>; 3];

/// The slots of a plain block: one table build and DP per kind. All empty when the block or the
/// designs give nothing; `None` when cancelled.
fn plain_block_slots(
    input: &PlanInput<'_>,
    clean: &[CandidateDesign],
    order: CutOrder,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Slots> {
    let settings = input.settings;
    let mut slots: Slots = [None, None, None];
    let extents = input.model.base.bounding_box_extents();
    let rough = RoughBlock {
        x_mm: extents[0],
        y_mm: extents[1],
        z_mm: extents[2],
    };
    if rough.validate().is_err() {
        return Some(slots);
    }
    let grid = choose_grid(&rough, settings);
    for (slot, kind) in slots.iter_mut().zip(AlternativeKind::ALL) {
        let designs = apply_alternative_rules(clean, kind);
        let front = pareto_front(&designs);
        if front.is_empty() {
            continue;
        }
        let table = build_piece_table_lanes(&grid, &front, lanes, on_progress)?;
        let layouts = plan_rough_for_order(&grid, &table, &front, order, on_progress)?;
        let Some(best) = best_layout(&layouts).cloned() else {
            continue;
        };
        *slot = if settings.caps().admits(&best) {
            Some(best)
        } else {
            let groups = plan_design_cap(
                &grid,
                &designs,
                &best,
                settings.max_designs_usize(),
                lanes,
                on_progress,
            )?;
            let all: Vec<RoughLayout> =
                groups.into_iter().flat_map(|group| group.layouts).collect();
            best_admitted(&all, settings.caps())
        };
    }
    Some(slots)
}

/// The slots of a shaped rough, on its clipped grid. All empty when the rough or the designs give
/// nothing; `None` when cancelled.
fn shaped_slots(
    input: &PlanInput<'_>,
    clean: &[CandidateDesign],
    order: CutOrder,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Slots> {
    let settings = input.settings;
    let mut slots: Slots = [None, None, None];
    let Ok(ctx) = ShapedCtx::new(input.model, settings) else {
        return Some(slots);
    };
    let grid = ctx.choose_grid(settings);
    // The Balanced plan builds its own tables; the other two reuse its classification.
    let knee_designs = apply_alternative_rules(clean, AlternativeKind::Knee);
    let knee_front = pareto_front(&knee_designs);
    if knee_front.is_empty() {
        return Some(slots);
    }
    let size_table = grid.size_table(settings, &knee_front, on_progress)?;
    let clip_params = BuildClipParams {
        grid: &grid,
        front: &knee_front,
        non_box_planes: &ctx.non_box,
        mesh: ctx.mesh.as_deref(),
        size_table: &size_table,
        settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    let clipped = build_clipped_table(&clip_params, on_progress)?;
    let knee_layouts = plan_shaped_for_order(
        &grid,
        &clipped,
        &ctx,
        &knee_front,
        order,
        settings,
        on_progress,
    )?;
    let Some(knee_best) = best_layout(&knee_layouts).cloned() else {
        return Some(slots);
    };
    let params = ShapedAltParams {
        grid: &grid,
        table: &clipped,
        ctx: &ctx,
        all_designs: clean,
        best: &knee_best,
        settings,
        lanes,
    };
    slots[2] = if settings.caps().admits(&knee_best) {
        Some(knee_best.clone())
    } else {
        capped_shaped(&params, &knee_designs, &knee_best, on_progress).ok()?
    };
    for (slot, kind) in slots.iter_mut().zip(AlternativeKind::ALL).take(2) {
        let designs = apply_alternative_rules(clean, kind);
        let front = pareto_front(&designs);
        if front.is_empty() {
            continue;
        }
        let layouts = plan_shaped_round(&params, &front, order, on_progress)?;
        let Some(best) = best_layout(&layouts).cloned() else {
            continue;
        };
        *slot = if settings.caps().admits(&best) {
            Some(best)
        } else {
            capped_shaped(&params, &designs, &best, on_progress).ok()?
        };
    }
    Some(slots)
}
