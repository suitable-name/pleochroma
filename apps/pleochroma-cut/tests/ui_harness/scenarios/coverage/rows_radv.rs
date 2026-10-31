//! The direct-invoke rows of the RADV shader workaround's callbacks (`RadvWorkaroundModel`),
//! in a file of their own. Rows are skipped for callbacks another piece already drove (the
//! `radv-workaround` states and the `radv_workaround_question` scenario in `states_radv.rs`).
//!
//! In the harness build the two restart callbacks only RECORD the request
//! (`harness_support::take_radv_restart_request`): nothing closes and nothing is started.

use super::table::{Invoke, Row, Setup};
use pleochroma_cut::*;
use slint::ComponentHandle;

/// Startup setup, Main window, shared state.
const RADV_MAIN_SHARED: &[Row] = rows!(MainWindow, Main, Startup, false;
    RadvWorkaroundModel.invoke_dont_use();
    RadvWorkaroundModel.invoke_enabled_changed(Default::default());
    RadvWorkaroundModel.invoke_not_now();
    RadvWorkaroundModel.invoke_restart_now();
    RadvWorkaroundModel.invoke_restart_with_workaround();
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    RADV_MAIN_SHARED.iter().collect()
}
