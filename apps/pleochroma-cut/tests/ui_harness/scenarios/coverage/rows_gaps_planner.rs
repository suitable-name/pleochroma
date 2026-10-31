//! The direct-invoke row of the Rough Planner callback run 8 left uncovered (lane H5 of the
//! 2026-10-28 findings): `RoughPlanModel.cut_plan_toggled`.
//!
//! The "Cut Plan" disclosure of a result card calls it with the card's row when the plan is folded
//! open and with -1 when it is folded shut; the click-through passes press that button, but a
//! pressed button is recorded as an element, not as the callback. The Rust handler only tells a
//! running tutorial (`ROUGH_CUT_PLAN_OPENED`, for a row of 0 or more), so nothing is observable in
//! the window: the row calls it both ways and the pass counts a panic or a closed window.

use super::table::{Invoke, Row, Setup};
use pleochroma_cut::*;
use slint::ComponentHandle;

/// Planner setup, Planner window, shared state.
const PLANNER_CUT_PLAN: &[Row] = &[Row {
    model: "RoughPlanModel",
    callback: "invoke_cut_plan_toggled",
    setup: Setup::Planner,
    fresh: false,
    invoke: Invoke::Planner(|w: &RoughPlannerWindow| {
        let model = w.global::<RoughPlanModel>();
        model.invoke_cut_plan_toggled(0);
        model.invoke_cut_plan_toggled(-1);
    }),
}];

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    PLANNER_CUT_PLAN.iter().collect()
}
