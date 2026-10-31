//! The Tune view's answers to `TuneModel`'s callbacks.
//!
//! Every change of an angle goes through the edit path the tier table and the Angle Sweep dialog
//! use, so each is one undo step and the tiers that follow a relation follow in the same step:
//! the nudge buttons and the angle box through `EditorSession::set_tier_angle_from_text` (what
//! the tier table's angle cell does), a click on the chart, Jump to Best and Revert This Tier
//! through `pleochroma_editor::sweep::apply_sweep_angle` (what "Use This Angle" does).
//!
//! No handler holds the hub across an edit: it reads what it needs from the hub, lets go, edits,
//! then tells the hub what happened and shows it with [`tick_now`].

use super::{
    Deps, handles, is_tunable,
    mini_sweep::{ANGLE_EPSILON_DEG, FIGURES, jump_target},
    order::{baseline_angles, move_tier, next_untuned},
    present::{angle_field, format_angle, push_hover, readout, row_at},
    selection, tick_now, with_hub,
};
use crate::{
    MainWindow, TuneModel,
    gui::{
        editor::{
            callbacks::snapshot_for_compare,
            relation_ui::refresh_with_followers,
            sweep::open_two_tier_prefilled,
            trials::{self, TrialOrigin},
        },
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma_editor::{
    guide::solving_events as events,
    retarget::plan::tier_display_name,
    session::InlineAngle,
    sweep::{MAX_SWEEP_ANGLE_DEG, MIN_SWEEP_ANGLE_DEG, apply_sweep_angle},
};
use slint::ComponentHandle;

/// What an edit says when the editor is busy with another call.
const EDITOR_BUSY: &str = "The editor is busy. Try again in a moment.";

/// What a chart click, Jump to Best and Revert say when the chart is not the one for the design
/// now.
const CHART_NOT_READY: &str =
    "The chart is out of date or still being drawn. Wait for it, then try again.";

/// Registers the callbacks; each finds the window and the editor through the hub.
pub(super) fn wire(ui: &MainWindow) {
    let model = ui.global::<TuneModel>();
    model.on_mode_changed(|| {
        let _ = with_hub(|hub| {
            hub.dirty = true;
            hub.hover = None;
        });
        tick_now();
        selection::select_initial();
    });
    model.on_select_tier(selection::select_tier);
    model.on_move_tier(|tier, direction| act(|_ui, _deps| reorder(tier, direction)));
    model.on_nudge(|delta| act(|ui, deps| nudge(&ui, &deps, f64::from(delta))));
    model.on_angle_edited(|text| act(|ui, deps| angle_edited(&ui, &deps, text.as_str())));
    model.on_jump_to_best(|| act(|ui, deps| jump_to_best(&ui, &deps)));
    model.on_figure_changed(|| {
        act(|_ui, _deps| {
            let _ = with_hub(|hub| hub.dirty = true);
            tick_now();
        });
    });
    model.on_metric_toggled(|index| act(|ui, _deps| toggle_metric(&ui, index)));
    model.on_chart_hover(|fraction| act(|ui, _deps| chart_hover(&ui, fraction)));
    model.on_chart_clicked(|fraction| act(|ui, deps| chart_clicked(&ui, &deps, fraction)));
    model.on_pair_chosen(|index| act(|ui, _deps| pair_chosen(&ui, index)));
    model.on_redraw(|| act(|_ui, _deps| redraw()));
    model.on_accept(|| act(|ui, deps| accept(&ui, &deps)));
    model.on_revert_tier(|| act(|ui, deps| revert_tier(&ui, &deps)));
    model.on_next_tier(|| act(|_ui, _deps| next_tier()));
}

/// Runs `action` with the window and the editor's handles, when both are there.
fn act(action: impl FnOnce(MainWindow, std::rc::Rc<Deps>)) {
    if let Some((ui, deps)) = handles() {
        action(ui, deps);
    }
}

/// The picked tier, when there is one.
fn selected() -> Option<usize> {
    with_hub(|hub| hub.selected.filter(|&tier| is_tunable(&hub.infos, tier))).flatten()
}

/// Up or Down on a row: moves the tier one place in the order the session keeps.
fn reorder(tier: i32, direction: i32) {
    let Ok(tier) = usize::try_from(tier) else {
        return;
    };
    let _ = with_hub(|hub| {
        let mut order = hub.order.clone();
        if move_tier(&mut order, tier, direction < 0) {
            hub.custom_order = order;
            hub.dirty = true;
        }
    });
    tick_now();
}

/// A nudge button: the shown angle changes by `delta_deg`.
fn nudge(ui: &MainWindow, deps: &Deps, delta_deg: f64) {
    let Some(tier) = selected() else {
        return;
    };
    let Ok(state) = deps.state.try_borrow() else {
        show_toast(ui, EDITOR_BUSY, "info");
        return;
    };
    let Some(current) = state
        .design
        .tiers
        .get(tier)
        .map(|found| found.angle_deg.abs())
    else {
        return;
    };
    drop(state);
    let wanted = current + delta_deg;
    if !(MIN_SWEEP_ANGLE_DEG..=MAX_SWEEP_ANGLE_DEG).contains(&wanted) {
        show_toast(
            ui,
            &format!(
                "{} cannot go to {}: a tier stays between {MIN_SWEEP_ANGLE_DEG} and \
                 {MAX_SWEEP_ANGLE_DEG} degrees.",
                name_of(deps, tier),
                format_angle(wanted)
            ),
            "info",
        );
        return;
    }
    apply_text(ui, deps, tier, &angle_field(wanted));
}

/// The angle box was accepted with `text`.
fn angle_edited(ui: &MainWindow, deps: &Deps, text: &str) {
    let Some(tier) = selected() else {
        return;
    };
    apply_text(ui, deps, tier, text);
}

/// Sets tier `tier`'s angle from text the way the tier table's angle cell does: one undo step.
fn apply_text(ui: &MainWindow, deps: &Deps, tier: usize, text: &str) {
    let applied = {
        let Ok(mut state) = deps.state.try_borrow_mut() else {
            show_toast(ui, EDITOR_BUSY, "info");
            return;
        };
        match state.set_tier_angle_from_text(tier, text) {
            Ok(InlineAngle::Missing) => false,
            Ok(InlineAngle::NoChange) => {
                show_toast(ui, "No change.", "info");
                false
            }
            Ok(InlineAngle::Applied(_)) => {
                refresh_with_followers(
                    ui,
                    &deps.render_ctx,
                    &deps.preview_state,
                    &deps.solid_last_solved,
                    &state,
                    [tier],
                );
                true
            }
            Err(error) => {
                show_toast(ui, &error, "error");
                false
            }
        }
    };
    // The editor is free again. The box shows the tier's angle (a refused text goes back to it).
    let _ = with_hub(|hub| hub.dirty = true);
    tick_now();
    if applied {
        // The Tune lesson waits for a nudge.
        raise(ui, events::TUNE_NUDGED);
    }
}

/// Sets tier `tier`'s angle to `angle_deg` (signed, as the tier stores it) as one undo step.
/// `true` when the angle changed.
fn apply_signed(ui: &MainWindow, deps: &Deps, tier: usize, angle_deg: f64) -> bool {
    let Ok(mut state) = deps.state.try_borrow_mut() else {
        show_toast(ui, EDITOR_BUSY, "info");
        return false;
    };
    match apply_sweep_angle(&mut state, tier, angle_deg) {
        Ok(Some(_)) => {
            refresh_with_followers(
                ui,
                &deps.render_ctx,
                &deps.preview_state,
                &deps.solid_last_solved,
                &state,
                [tier],
            );
            drop(state);
            let _ = with_hub(|hub| hub.dirty = true);
            tick_now();
            true
        }
        Ok(None) => {
            drop(state);
            show_toast(ui, "No change.", "info");
            false
        }
        Err(error) => {
            drop(state);
            show_toast(ui, &error.to_string(), "error");
            false
        }
    }
}

/// The name a person reads for tier `tier` of the open design.
fn name_of(deps: &Deps, tier: usize) -> String {
    deps.state.try_borrow().map_or_else(
        |_| format!("Tier {}", tier + 1),
        |state| tier_display_name(&state.design, tier),
    )
}

/// The drawn sweep's rows and the figure shown, but only while the chart is the one for the
/// design now (not stale, not still drawing).
fn fresh_rows(ui: &MainWindow) -> Option<(Vec<pleochroma_editor::sweep::SweepRow>, usize)> {
    let model = ui.global::<TuneModel>();
    if model.get_sweep_state() != 2 || model.get_sweep_stale() {
        return None;
    }
    let figure = usize::try_from(model.get_figure_index())
        .unwrap_or(0)
        .min(FIGURES.len() - 1);
    with_hub(|hub| hub.shown.as_ref().map(|(_, outcome)| outcome.rows.clone()))
        .flatten()
        .map(|rows| (rows, figure))
}

/// Jump to Best: sets the best angle of the chart.
fn jump_to_best(ui: &MainWindow, deps: &Deps) {
    let Some(tier) = selected() else {
        return;
    };
    let Some((rows, figure)) = fresh_rows(ui) else {
        show_toast(ui, CHART_NOT_READY, "info");
        return;
    };
    let current = deps
        .state
        .try_borrow()
        .ok()
        .and_then(|state| state.design.tiers.get(tier).map(|found| found.angle_deg));
    let Some(current) = current else {
        return;
    };
    match jump_target(&rows, figure, current) {
        Some(angle) => {
            if apply_signed(ui, deps, tier, angle) {
                show_toast(
                    ui,
                    &format!(
                        "{} is now {}. Undo (Ctrl+Z) puts it back.",
                        name_of(deps, tier),
                        format_angle(angle)
                    ),
                    "success",
                );
            }
        }
        None => show_toast(ui, "The tier is at the best angle of this chart.", "info"),
    }
}

/// Re-present the completed rows only: no tick, worker, cache or design mutation.
fn toggle_metric(ui: &MainWindow, index: i32) {
    let _ = with_hub(|hub| {
        if !hub.visible_metrics.toggle(index) {
            return;
        }
        hub.dirty = true;
        let rows = hub
            .shown
            .as_ref()
            .map_or(&[][..], |(_, outcome)| outcome.rows.as_slice());
        super::metrics::push(ui, rows, hub.visible_metrics);
        let figure = usize::try_from(ui.global::<TuneModel>().get_figure_index())
            .unwrap_or(0)
            .min(FIGURES.len() - 1);
        ui.global::<TuneModel>()
            .set_readout_text(readout(rows, figure, hub.hover, hub.visible_metrics).into());
    });
}

/// The pointer moved over the chart (`fraction` across the plot, negative when it left).
fn chart_hover(ui: &MainWindow, fraction: f32) {
    let hover = if fraction >= 0.0 {
        with_hub(|hub| {
            hub.shown
                .as_ref()
                .and_then(|(_, outcome)| row_at(&outcome.rows, fraction))
        })
        .flatten()
    } else {
        None
    };
    let model = ui.global::<TuneModel>();
    let figure = usize::try_from(model.get_figure_index())
        .unwrap_or(0)
        .min(FIGURES.len() - 1);
    let text = with_hub(|hub| {
        hub.hover = hover;
        hub.shown.as_ref().map(|(_, outcome)| {
            push_hover(ui, &outcome.rows, hover);
            readout(&outcome.rows, figure, hover, hub.visible_metrics)
        })
    })
    .flatten();
    if let Some(text) = text {
        model.set_readout_text(text.into());
    }
}

/// A click on the chart: the angle under the pointer becomes the tier's angle (one undo step).
fn chart_clicked(ui: &MainWindow, deps: &Deps, fraction: f32) {
    let Some(tier) = selected() else {
        return;
    };
    let Some((rows, _figure)) = fresh_rows(ui) else {
        show_toast(ui, CHART_NOT_READY, "info");
        return;
    };
    let Some(row) = row_at(&rows, fraction).and_then(|index| rows.get(index)) else {
        return;
    };
    if !row.is_valid() {
        show_toast(ui, "That angle does not give a stone.", "info");
        return;
    }
    let angle = row.angle_deg;
    if apply_signed(ui, deps, tier, angle) {
        show_toast(
            ui,
            &format!(
                "{} is now {}. Undo (Ctrl+Z) puts it back.",
                name_of(deps, tier),
                format_angle(angle)
            ),
            "success",
        );
    }
}

/// Pair With: the Angle Sweep dialog in Two Tiers mode, this tier across and `index` (an entry
/// after the combo's heading) down.
fn pair_chosen(ui: &MainWindow, index: i32) {
    let partner = usize::try_from(index)
        .ok()
        .and_then(|index| with_hub(|hub| hub.partners.get(index).copied()).flatten());
    // The combo goes back to its heading, ready for the next choice.
    ui.global::<TuneModel>().set_pair_index(0);
    if let (Some(tier), Some(partner)) = (selected(), partner) {
        open_two_tier_prefilled(ui, tier, partner);
    }
}

/// Draw the mini sweep again: what was drawn is dropped, the timer starts a new one.
fn redraw() {
    let _ = with_hub(|hub| {
        hub.forget_sweeps();
    });
    tick_now();
}

/// Accept: keeps the design as it is now as a trial and ticks the tier.
fn accept(ui: &MainWindow, deps: &Deps) {
    let Some(tier) = selected() else {
        return;
    };
    {
        let Ok(state) = deps.state.try_borrow() else {
            show_toast(ui, EDITOR_BUSY, "info");
            return;
        };
        trials::record_after_apply(ui, &state, TrialOrigin::Tune);
    }
    let _ = with_hub(|hub| {
        hub.accepted.insert(tier);
        hub.dirty = true;
    });
    show_toast(
        ui,
        &format!("{} accepted and kept as a trial.", name_of(deps, tier)),
        "success",
    );
    tick_now();
    raise(ui, events::TUNE_ACCEPTED);
}

/// Revert This Tier: the tier goes back to the angle it has in the Baseline (one undo step).
fn revert_tier(ui: &MainWindow, deps: &Deps) {
    let Some(tier) = selected() else {
        return;
    };
    let Some((before, _label)) = snapshot_for_compare() else {
        show_toast(ui, "There is no Baseline to go back to.", "info");
        return;
    };
    let target = {
        let Ok(state) = deps.state.try_borrow() else {
            show_toast(ui, EDITOR_BUSY, "info");
            return;
        };
        baseline_angles(&before, &state.design)
            .get(tier)
            .copied()
            .flatten()
    };
    let Some(target) = target else {
        show_toast(ui, "The Baseline has no such tier.", "info");
        return;
    };
    let now = deps
        .state
        .try_borrow()
        .ok()
        .and_then(|state| state.design.tiers.get(tier).map(|found| found.angle_deg));
    if now.is_some_and(|now| (now - target).abs() <= ANGLE_EPSILON_DEG) {
        show_toast(ui, "The tier already has the Baseline's angle.", "info");
        return;
    }
    if apply_signed(ui, deps, tier, target) {
        let _ = with_hub(|hub| hub.accepted.remove(&tier));
        show_toast(
            ui,
            &format!(
                "{} is back to {}. Undo (Ctrl+Z) puts it forward again.",
                name_of(deps, tier),
                format_angle(target)
            ),
            "success",
        );
    }
}

/// Next Tier: picks the next tier that is not accepted yet.
fn next_tier() {
    let next = with_hub(|hub| {
        let infos = &hub.infos;
        next_untuned(
            &hub.order,
            hub.selected,
            &|tier| is_tunable(infos, tier),
            &hub.accepted,
        )
    })
    .flatten();
    if let Some(next) = next.and_then(|tier| i32::try_from(tier).ok()) {
        selection::select_tier(next);
    }
}
