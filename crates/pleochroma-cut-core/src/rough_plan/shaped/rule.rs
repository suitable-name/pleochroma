//! The shaped planner under an orientation rule.
//!
//! The rule needs no code of its own in the shaped DP: a design's resolved rule gives its `Norm`
//! the factor of each assignment, and the shaped valuations all read it through
//! `Norm::value_at` / `stone_value` (the size table and the shortlist through `best_pick`, the
//! clipped scaler `solve_partial_piece`, the uniform grids, the refinement). What lives here is
//! the one thing the alternatives of a Preferred rule need on top: a round of the shaped DP over
//! a pool of designs whose rules differ from the baseline's, reusing the baseline's piece
//! classification (the classes depend on the rough alone, not on the designs).

use super::dp::{ShapedAltParams, alternative_table, plan_shaped_for_order};
use crate::rough_plan::{CandidateDesign, CutOrder, PlanProgress, RoughLayout};

/// One shaped DP round of cut `order` over `pool`.
///
/// The pool holds the designs the round is planned with, each carrying its own resolved rule: the
/// clipped table of `params` is re-valued for `pool`, then the DP of the order runs.
/// `params.table` is the baseline whose classification is reused; the best layout
/// and the design list of `params` are not read.
///
/// `None` when `on_progress` cancels; the order's candidate layouts otherwise.
pub fn plan_shaped_round(
    params: &ShapedAltParams<'_>,
    pool: &[CandidateDesign],
    order: CutOrder,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let table = alternative_table(params, pool, on_progress)?;
    plan_shaped_for_order(
        params.grid,
        &table,
        params.ctx,
        pool,
        order,
        params.settings,
        on_progress,
    )
}
