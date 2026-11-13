//! The verdict's Fix buttons: plan on a worker, apply as ONE undo step, say so in a toast.
//!
//! The click goes: [`fix_reason`] checks the verdict on screen still describes the design and,
//! for a fix whose question is known up front (a removal), asks it with the heading and button
//! the fix names. [`start`] then hands the planning (`pleochroma_editor::verdict::plan_fix`,
//! which solves candidates and so takes a moment) to a thread. [`finish_fix`] runs back on the
//! UI thread. A plan that carries its own question (the snap to meet points, which names the
//! facets it would cut away: only planning finds them) asks it now, and nothing is applied
//! unless the cutter agrees. [`apply_plan`] applies the planned edit through the editor session
//! (one history step, so one Undo takes it back), refreshes the panel the way an Undo does,
//! selects the tier the plan names and toasts the sentence.
//!
//! Nothing is applied when the design changed meanwhile (the plan was made for another
//! design) or when the planner refused: the cutter gets the planner's plain sentence instead.

use super::{SLOT, ensure_current, pending_fix, present, services};
use crate::{
    EditorModel, MainWindow,
    gui::{
        editor::{
            callbacks::{HistoryMove, refresh_after_renumbering_move},
            native_io::ask_write_confirm,
            relation_ui::take_cleared_relations_sentence,
        },
        show_toast,
    },
};
use pleochroma_cut_core::ConstraintTier;
use pleochroma_editor::verdict::{FixAction, FixPlan, plan_fix};
use slint::ComponentHandle;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Said when the planner stopped unexpectedly.
const PLANNER_FAILED: &str = "The fix stopped unexpectedly. Nothing was changed.";

/// Said when the editor state was in use and the fix could not be applied.
const EDITOR_BUSY: &str = "The editor is busy. Try the fix again.";

/// `VerdictModel.fix_reason(index)`: the Fix button of reason `index` was pressed.
pub(super) fn fix_reason(ui: &MainWindow, index: i32) {
    let Some(services) = services() else {
        return;
    };
    let ready = usize::try_from(index)
        .map_err(|_| "There is no such reason.".to_string())
        .and_then(|index| pending_fix(index, &services.editor.borrow().design.tiers));
    match ready {
        // A fix whose plan asks its own question is planned first (see `finish_fix`).
        Ok((action, Some(question))) if !action.asks_after_planning() => {
            // The heading and the button name what the fix does (a removal, ...).
            let heading = action.confirm_heading();
            let button = action.confirm_button();
            ask_write_confirm(ui, heading, question, button, None, move |ui| {
                start(ui, action);
            });
        }
        Ok((action, _)) => start(ui, action),
        Err(message) => show_toast(ui, &message, "info"),
    }
}

/// Plans `action` against a copy of the design on a worker thread and applies the result.
fn start(ui: &MainWindow, action: FixAction) {
    let Some(services) = services() else {
        return;
    };
    // The confirmation dialog may have been open a while: check again.
    if let Err(message) = ensure_current(&services.editor.borrow().design.tiers) {
        show_toast(ui, &message, "info");
        return;
    }
    let design = services.editor.borrow().design.clone();
    let n_d = SLOT.with(|cell| cell.borrow().n_d);
    SLOT.with(|cell| cell.borrow_mut().busy = true);
    present::set_busy(ui, true);

    let reference = design.tiers.clone();
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let planned = catch_unwind(AssertUnwindSafe(|| plan_fix(&design, &action, n_d)))
            .unwrap_or_else(|_| Err(PLANNER_FAILED.to_string()));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            finish_fix(&ui, &action, reference, planned);
        });
    });
}

/// The planner's answer, back on the UI thread: a refusal is toasted; a plan that carries a
/// question asks it first (nothing is applied unless the cutter agrees); any other plan is
/// applied when it still fits the design.
fn finish_fix(
    ui: &MainWindow,
    action: &FixAction,
    reference: Vec<ConstraintTier>,
    planned: Result<FixPlan, String>,
) {
    SLOT.with(|cell| cell.borrow_mut().busy = false);
    present::set_busy(ui, false);
    let plan = match planned {
        Ok(plan) => plan,
        Err(message) => {
            show_toast(ui, &message, "error");
            return;
        }
    };
    if let Some(question) = plan.confirm.clone() {
        ask_write_confirm(
            ui,
            action.confirm_heading(),
            question,
            action.confirm_button(),
            None,
            move |ui| apply_plan(ui, &reference, plan),
        );
        return;
    }
    apply_plan(ui, &reference, plan);
}

/// Applies `plan`, made for a design with the tiers `reference`, when the design still has them.
fn apply_plan(ui: &MainWindow, reference: &[ConstraintTier], plan: FixPlan) {
    let Some(services) = services() else {
        return;
    };
    let Ok(mut st) = services.editor.try_borrow_mut() else {
        show_toast(ui, EDITOR_BUSY, "error");
        return;
    };
    // The plan was made for `reference`; any edit since (even a one-tier change) voids it,
    // including one made while its question was open.
    if st.design.tiers != reference {
        drop(st);
        show_toast(
            ui,
            "The design changed while the fix was being planned. Nothing was changed.",
            "info",
        );
        return;
    }
    let FixPlan {
        edit,
        message,
        select,
        ..
    } = plan;
    match st.try_apply_mapped(edit, None) {
        Ok((change, renumbering)) => {
            // The same refresh an Undo makes: the selected row first follows its tier (a fix
            // that removes tiers moves the rows below them), then a full re-solve, the form
            // re-seeded, the preview replanned. The solve's result reaches the verdict
            // through the usual hook, so the fix is re-validated by the next verdict.
            refresh_after_renumbering_move(
                ui,
                &services.render_ctx,
                &services.preview_state,
                &services.solid_last_solved,
                &st,
                &HistoryMove::new(change, renumbering),
            );
            // A fix that removes a tier other tiers' relations read frees those relations:
            // say so here, so the notice does not wait for an unrelated message.
            let freed = take_cleared_relations_sentence(&mut st);
            drop(st);
            if let Some(tier) = select.and_then(|tier| i32::try_from(tier).ok()) {
                ui.global::<EditorModel>().set_selected_tier_index(tier);
            }
            let mut text = format!("{message} Undo (Ctrl+Z) puts it back.");
            if let Some(freed) = freed {
                text.push(' ');
                text.push_str(&freed);
            }
            show_toast(ui, &text, "success");
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}
