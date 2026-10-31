//! What each control of the Trials view does: Keep as Trial, selecting (the preview on the
//! stone), Restore, Compare to Current, Set as Baseline, Save as Variant, Delete, the star and
//! the note, the sort, Record Every Change and Re-score Trials.
//!
//! Every function runs on the UI thread. None holds the hub's borrow across a call into the
//! editor or into Slint (they read what they need out of it first), so a call that ends up
//! back in this module's hooks cannot meet a held borrow.

use super::{
    Deps, PreviewMark, Snap, TrialOrigin, default_axis, record, refresh_with, rescore,
    show_real_design, with_hub,
};
use crate::{
    EditorModel, GuideModel, MainWindow, SweepModel, TrialsModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            baseline,
            callbacks::{HistoryMove, refresh_after_renumbering_move},
            compare::{VariantSide, open_variants_compare},
            variants,
            view::submit_design_ghost_preview,
        },
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma_editor::{
    guide::solving_events as events,
    optimize_view::default_optimize_material_ri,
    trials::{Trial, restore_edit},
};
use slint::{ComponentHandle, SharedString};
use std::{rc::Rc, sync::atomic::Ordering};

const EDITOR_BUSY_TEXT: &str = "The editor is busy. Try again in a moment.";
const NO_DESIGN_TEXT: &str = "Open or create a design first.";
const NO_SELECTION_TEXT: &str = "Click a trial first.";

/// The longest note a trial takes, in characters.
const NOTE_LIMIT: usize = 200;

/// Registers the view's callbacks.
pub(super) fn wire(ui: &MainWindow) {
    let model = ui.global::<TrialsModel>();
    model.on_keep_trial(unit(ui, keep_trial));
    model.on_select(number(ui, select));
    model.on_toggle_star(number(ui, toggle_star));
    model.on_restore(unit(ui, restore));
    model.on_compare(unit(ui, compare));
    model.on_set_as_baseline(unit(ui, set_as_baseline));
    model.on_save_as_variant(unit(ui, save_as_variant));
    model.on_delete(unit(ui, delete));
    model.on_rescore(unit(ui, rescore_trials));
    let weak = ui.as_weak();
    model.on_set_note(move |trial, text: SharedString| {
        if let (Some(ui), Ok(trial)) = (weak.upgrade(), u32::try_from(trial)) {
            set_note(&ui, trial, text.as_str());
        }
    });
    let weak = ui.as_weak();
    model.on_sort_changed(move |index| {
        let _ = with_hub(|hub| hub.sort_index = index);
        if let Some(ui) = weak.upgrade() {
            refresh_now(&ui);
        }
    });
    model.on_record_every_change_toggled(|on| {
        let _ = with_hub(|hub| {
            hub.every_change = on;
            hub.owed = false;
        });
    });
}

/// A callback without arguments that runs `action`.
fn unit(ui: &MainWindow, action: fn(&MainWindow)) -> impl Fn() + 'static {
    let weak = ui.as_weak();
    move || {
        if let Some(ui) = weak.upgrade() {
            action(&ui);
        }
    }
}

/// A callback with a trial's number that runs `action`.
fn number(ui: &MainWindow, action: fn(&MainWindow, u32)) -> impl Fn(i32) + 'static {
    let weak = ui.as_weak();
    move |number| {
        if let (Some(ui), Ok(number)) = (weak.upgrade(), u32::try_from(number)) {
            action(&ui, number);
        }
    }
}

// --- small helpers ---------------------------------------------------------------------------

fn deps() -> Option<Rc<Deps>> {
    with_hub(|hub| hub.deps.clone()).flatten()
}

fn trial_of(number: u32) -> Option<Trial> {
    with_hub(|hub| hub.list.get(number).cloned()).flatten()
}

/// The selected trial, or says that there is none.
fn selected_trial(ui: &MainWindow) -> Option<Trial> {
    let trial = with_hub(|hub| {
        hub.selected
            .and_then(|number| hub.list.get(number).cloned())
    })
    .flatten();
    if trial.is_none() {
        show_toast(ui, NO_SELECTION_TEXT, "info");
    }
    trial
}

/// Shows the list as it is now.
fn refresh_now(ui: &MainWindow) {
    let snap = deps().and_then(|deps| deps.state.try_borrow().ok().map(|state| Snap::of(&state)));
    refresh_with(ui, snap.as_ref());
}

// --- keeping ---------------------------------------------------------------------------------

/// "Keep as Trial": the design as it is now.
fn keep_trial(ui: &MainWindow) {
    let Some(deps) = deps() else {
        return;
    };
    let Ok(state) = deps.state.try_borrow() else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    if !state.has_design {
        show_toast(ui, NO_DESIGN_TEXT, "info");
        return;
    }
    if let Some(number) = super::record_trial(ui, &state, TrialOrigin::Manual, None) {
        show_toast(ui, &format!("Kept as Trial {number}."), "success");
        raise(ui, events::TRIAL_KEPT);
    }
}

// --- selecting and previewing -----------------------------------------------------------------

/// A click on a row: selects the trial and previews its design on the stone; a click on the
/// selected one clears the selection and puts the real design back.
fn select(ui: &MainWindow, number: u32) {
    let Some(deps) = deps() else {
        return;
    };
    if with_hub(|hub| hub.selected == Some(number)).unwrap_or(false) {
        let was_previewing = with_hub(|hub| {
            hub.selected = None;
            hub.preview.take().is_some()
        })
        .unwrap_or(false);
        if was_previewing {
            show_real_design(ui, &deps);
        }
        refresh_now(ui);
        return;
    }
    let Some(trial) = trial_of(number) else {
        return;
    };
    let mark = {
        let Ok(state) = deps.state.try_borrow() else {
            show_toast(ui, EDITOR_BUSY_TEXT, "info");
            return;
        };
        PreviewMark {
            epoch: state.design_epoch.load(Ordering::Relaxed),
            generation: state.current_generation(),
        }
    };
    let queued =
        submit_design_ghost_preview(ui, &deps.render_ctx, &deps.preview_state, &trial.design);
    let _ = with_hub(|hub| {
        hub.selected = Some(number);
        hub.preview = queued.then_some(mark);
    });
    refresh_now(ui);
}

// --- the buttons under the table ------------------------------------------------------------

/// "Restore": the selected trial's design back, as one undo step worded "Restore Trial N". Not
/// recorded as a trial.
fn restore(ui: &MainWindow) {
    if !ui.global::<GuideModel>().invoke_allows_history() {
        return;
    }
    let Some(trial) = selected_trial(ui) else {
        return;
    };
    let Some(deps) = deps() else {
        return;
    };
    let Ok(mut state) = deps.state.try_borrow_mut() else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    let epoch = state.design_epoch.load(Ordering::Relaxed);
    let edit = match restore_edit(&state.design, &trial, epoch) {
        Ok(Some(edit)) => edit,
        Ok(None) => {
            show_toast(ui, "This trial is the current design.", "info");
            return;
        }
        Err(reason) => {
            show_toast(ui, &reason, "warning");
            return;
        }
    };
    let label = format!("Restore Trial {}", trial.number);
    match state.session.try_apply_mapped(edit, Some(label.as_str())) {
        Ok((change, renumbering)) => {
            // The replan below puts the real design on the stone; the edit watch must not see
            // the restore as an edit to record.
            let revisions = record::revisions_of(&state);
            let _ = with_hub(|hub| {
                hub.selected = None;
                hub.preview = None;
                hub.watch.absorb(epoch, &revisions);
            });
            refresh_after_renumbering_move(
                ui,
                &deps.render_ctx,
                &deps.preview_state,
                &deps.solid_last_solved,
                &state,
                &HistoryMove::new(change, renumbering),
            );
            drop(state);
            show_toast(
                ui,
                &format!(
                    "Restored Trial {}. Undo takes it back in one step.",
                    trial.number
                ),
                "success",
            );
            refresh_now(ui);
            raise(ui, events::TRIAL_RESTORED);
        }
        Err(error) => show_toast(
            ui,
            &format!("The design cannot take this trial. {error}"),
            "warning",
        ),
    }
}

/// "Compare to Current": the trial and the design now, side by side in the Compare window.
fn compare(ui: &MainWindow) {
    let Some(trial) = selected_trial(ui) else {
        return;
    };
    let Some(deps) = deps() else {
        return;
    };
    let Some(current) = deps
        .state
        .try_borrow()
        .ok()
        .map(|state| state.design.clone())
    else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    open_variants_compare(
        ui,
        VariantSide {
            design: trial.design,
            label: format!("Trial {}", trial.number),
        },
        VariantSide {
            design: current,
            label: "Current design".to_owned(),
        },
    );
}

/// "Set as Baseline": the trial's design, with its masts, as the Baseline.
fn set_as_baseline(ui: &MainWindow) {
    let Some(trial) = selected_trial(ui) else {
        return;
    };
    baseline::set_baseline_to(
        ui,
        &trial.design,
        trial.solved.clone(),
        &format!("Trial {}", trial.number),
    );
}

/// "Save as Variant...": the trial's design (not the design now) as a variant in the library.
fn save_as_variant(ui: &MainWindow) {
    let Some(trial) = selected_trial(ui) else {
        return;
    };
    variants::save_design(
        ui,
        trial.design,
        &format!("Trial {}", trial.number),
        &trial.note,
        trial.design_epoch,
    );
}

/// "Delete": removes the selected trial.
fn delete(ui: &MainWindow) {
    let Some(deps) = deps() else {
        return;
    };
    let Some(was_previewing) = with_hub(|hub| {
        let number = hub.selected?;
        hub.list.remove(number);
        hub.pending.retain(|(pending, _)| *pending != number);
        hub.no_stone.remove(&number);
        hub.selected = None;
        Some(hub.preview.take().is_some())
    })
    .flatten() else {
        show_toast(ui, NO_SELECTION_TEXT, "info");
        return;
    };
    if was_previewing {
        show_real_design(ui, &deps);
    }
    refresh_now(ui);
}

// --- the table -----------------------------------------------------------------------------

/// The star of trial `number`.
fn toggle_star(ui: &MainWindow, number: u32) {
    let _ = with_hub(|hub| hub.list.toggle_star(number));
    refresh_now(ui);
}

/// The note of trial `number`: trimmed and cut to [`NOTE_LIMIT`] characters.
fn set_note(ui: &MainWindow, number: u32, text: &str) {
    let note: String = text.trim().chars().take(NOTE_LIMIT).collect();
    let _ = with_hub(|hub| hub.list.set_note(number, &note));
    refresh_now(ui);
}

// --- re-scoring ----------------------------------------------------------------------------

/// "Re-score Trials": scores the trials of another material, or not scored, under the design's
/// material and the Grading tray, on a thread of its own.
fn rescore_trials(ui: &MainWindow) {
    let Some(deps) = deps() else {
        return;
    };
    let editor = ui.global::<EditorModel>();
    if editor.get_optimize_running()
        || editor.get_deep_solve_running()
        || ui.global::<SweepModel>().get_running()
    {
        show_toast(
            ui,
            "An Optimize run or an Angle Sweep is using the processor. Try again when it is done.",
            "info",
        );
        return;
    }
    let custom_materials = RenderContext::lock(&deps.render_ctx)
        .custom_materials
        .as_ref()
        .clone();
    let (key, material, material_default) = {
        let Ok(state) = deps.state.try_borrow() else {
            show_toast(ui, EDITOR_BUSY_TEXT, "info");
            return;
        };
        // A design that names no material is scored at its own refractive index, not as
        // diamond: the same defaulting Optimize, the sweep and the strip do.
        let mut material = state.design.material.clone();
        let _ = default_optimize_material_ri(&state.design, &mut material, &custom_materials);
        let material_default = default_axis::of(&state.design.material);
        (
            record::trial_key(
                &state.design.material,
                &state.design.render_effects,
                material_default,
            ),
            material,
            material_default,
        )
    };
    let Some(items) =
        with_hub(|hub| hub.rescoring.is_none().then(|| hub.items_to_rescore(&key))).flatten()
    else {
        return;
    };
    if items.is_empty() {
        show_toast(ui, "Every trial is scored already.", "info");
        return;
    }
    let job = rescore::Job {
        items,
        material,
        custom_materials,
        material_default,
    };
    match rescore::start(job, key) {
        Ok(run) => {
            let _ = with_hub(|hub| hub.rescoring = Some(run));
            refresh_now(ui);
        }
        Err(reason) => show_toast(ui, &reason, "error"),
    }
}
