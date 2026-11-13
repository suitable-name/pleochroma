//! Deep Solve's "Cancel" button and its per-tier "Pin to verified mast" action.

use super::{DEEP_SOLVE_ACTIVITY_ID, LAST_DEEP_SOLVE_DELTAS};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve, deep_solve,
            state::EditorState,
            view::{SolidLastSolved, refresh_all_now},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::Edit;
use slint::{ComponentHandle, SharedString};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// "Cancel", shown only while a Deep Solve runs. [`deep_solve::DeepSolveHandle::cancel`] stops
/// the search itself. This callback also resets `editor_deep_solve_running` and the status on
/// the same click, without waiting for the worker's `on_done`. The completion handler is
/// otherwise the only place those are reset, so the strip would stay busy until the worker
/// reported back.
pub(in crate::gui::editor) fn setup_deep_solve_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_deep_solve_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if let Some(handle) = state.borrow().deep_solve.as_ref() {
            handle.cancel();
        }
        // Finish the activity now, not when the abandoned worker reports back (see
        // `DEEP_SOLVE_ACTIVITY_ID`).
        if let Some(id) = DEEP_SOLVE_ACTIVITY_ID.with(RefCell::take)
            && let Some(activity) = auto_solve::activity()
        {
            activity.finish(id);
        }
        ui.global::<EditorModel>().set_deep_solve_running(false);
        ui.global::<EditorModel>()
            .set_deep_solve_status("Deep Solve cancelled.".into());
        ui.global::<EditorModel>()
            .set_deep_solve_status_is_problem(false);
    });
}

/// The tier index encoded in a `"#N"` (1-based) row label -- the inverse of
/// `view::tier_name_for_row`'s callers, which all build that exact string via
/// `format!("#{}", tier_index + 1)` (see [`crate::DeepSolveTierRow::tier_number`]/
/// [`crate::OptimizeChangeRow::tier_number`]). `None` for anything that isn't
/// `#` followed by a positive integer -- in particular for `"#0"`, which this
/// format never produces (row numbers start at `#1`).
#[must_use]
fn tier_index_from_row_number(text: &str) -> Option<usize> {
    text.strip_prefix('#')?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

/// [`LAST_DEEP_SOLVE_DELTAS`]'s lookup half: the verified mast Deep Solve proposed
/// for `tier_index`, if that tier is still named in `deltas`. Pulled out of
/// [`setup_deep_solve_pin_callback`] so the lookup itself -- the one genuinely
/// testable piece of that callback -- has something a test can call directly.
#[must_use]
fn verified_mast_for(deltas: &[deep_solve::TierMastDelta], tier_index: usize) -> Option<f64> {
    deltas
        .iter()
        .find(|d| d.tier_index == tier_index)
        .map(|d| d.after_mast)
}

/// The per-tier "Pin to verified mast" action in Deep Solve's disagreement table
/// (`EditorModel.deep_solve_tier_rows`). It takes the `"#N"` tier number the table prints (see
/// [`tier_index_from_row_number`]), so the button calling `EditorModel.pin_verified_mast` needs
/// no extra field on `DeepSolveTierRow`.
///
/// It applies through [`EditorState::apply`] as `Edit::SetConstraint(ScaleReference(mast))`. It
/// never applies the whole verified configuration: the repair's other wins are vertex-level
/// picks with no `MeetConstraint` (see `deep_solve`'s module doc). If [`LAST_DEEP_SOLVE_DELTAS`]
/// no longer names the tier (a new run, a replaced design, or a tier-count change), it shows a
/// toast and does nothing.
pub(in crate::gui::editor) fn setup_deep_solve_pin_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_pin_verified_mast(move |tier_number: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(index) = tier_index_from_row_number(&tier_number) else {
                return;
            };
            let mast = LAST_DEEP_SOLVE_DELTAS.with(|cell| verified_mast_for(&cell.borrow(), index));
            let Some(mast) = mast else {
                show_toast(
                    &ui,
                    "No verified mast recorded for this tier any more -- re-run Deep \
                     Solve.",
                    "error",
                );
                return;
            };
            let mut st = state.borrow_mut();
            match st.apply(Edit::SetConstraint {
                index,
                constraint: MeetConstraint::ScaleReference(mast),
            }) {
                Ok(()) => {
                    // `refresh_all_now` takes the shared state, so release the borrow first.
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
                    show_toast(
                        &ui,
                        &format!("Pinned tier #{} to Deep Solve's verified mast.", index + 1),
                        "success",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use deep_solve::TierMastDelta;

    // --- tier_index_from_row_number ---

    #[test]
    fn tier_index_from_row_number_parses_the_1_based_label_back_to_a_0_based_index() {
        assert_eq!(tier_index_from_row_number("#1"), Some(0));
        assert_eq!(tier_index_from_row_number("#17"), Some(16));
    }

    #[test]
    fn tier_index_from_row_number_rejects_anything_that_is_not_hash_digits() {
        assert_eq!(tier_index_from_row_number("17"), None);
        assert_eq!(tier_index_from_row_number("#"), None);
        assert_eq!(tier_index_from_row_number("#abc"), None);
        assert_eq!(tier_index_from_row_number(""), None);
    }

    #[test]
    fn tier_index_from_row_number_rejects_hash_zero() {
        // This format never produces "#0" (rows are numbered from #1), so there is
        // no valid 0-based index to underflow to.
        assert_eq!(tier_index_from_row_number("#0"), None);
    }

    // --- verified_mast_for ---

    fn delta(tier_index: usize, before: f64, after: f64) -> TierMastDelta {
        TierMastDelta {
            tier_index,
            before_mast: before,
            after_mast: after,
        }
    }

    #[test]
    fn verified_mast_for_finds_the_matching_tiers_after_mast() {
        let deltas = vec![delta(0, 1.0, 1.1), delta(3, 2.0, 2.5)];
        assert_eq!(verified_mast_for(&deltas, 3), Some(2.5));
    }

    #[test]
    fn verified_mast_for_is_none_when_the_tier_is_not_in_the_list() {
        let deltas = vec![delta(0, 1.0, 1.1)];
        assert_eq!(verified_mast_for(&deltas, 5), None);
    }
}
