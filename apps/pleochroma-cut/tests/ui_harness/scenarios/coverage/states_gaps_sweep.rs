//! The Angle Sweep dialog's Two Tiers mode for the elements run 8 left uncovered (lane H5 of the
//! 2026-10-28 findings): the Across and Down tier combos, the heat map's focus, the Figure combo,
//! "Outline the Best Cells", "Use These Angles" and "Refine Around This", and the tilt question's
//! "Run Anyway" and "Cancel".
//!
//! Why they were unreached: the one-tier states of `states_dialogs.rs` never leave One Tier, the
//! `dialogs_sweep_two_tiers` scenario switches the mode but runs no sweep (so there was no heat
//! map, no figure choice and nothing to use or refine), and the tilt question only appears for a
//! sweep of more than 100 angle pairs with the tilt averages on, which no state asked for.
//!
//! The two click-through states below open the dialog in Two Tiers with the smallest grid the form
//! accepts (the design's own angles and the step below them: two angles per tier, four pairs), so
//! the pass can leave "Run" enabled; the second one has the sweep finished and a usable pair
//! selected. The tilt question needs a form of 289 pairs, so it has a scenario of its own that
//! answers it both ways and stops the sweep the second answer starts.

use super::{
    super::{Piece, dialogs, existing_install},
    overlay_states::{Base, NONE, State, edit_tab, press},
    state_pieces,
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
};
use pleochroma_cut::SweepModel;
use slint::{ComponentHandle, SharedString};
use std::time::Duration;

/// How long the four-pair sweep and the stop of the tilt sweep may take, in seconds.
const SWEEP_WAIT_SECS: u64 = 90;

/// The pieces of this file: the two click-through states and the tilt question.
pub fn pieces() -> Vec<Piece> {
    let mut all = state_pieces("dialogs-sweep-two-tiers", existing_install, two_tier_states);
    all.push(Piece::new(
        "dialogs_sweep_tilt_question",
        existing_install,
        tilt_question,
    ));
    all
}

fn two_tier_states() -> Vec<State> {
    vec![
        State {
            name: "Angle sweep, Two Tiers form",
            base: Base::Design,
            scope: Some("SweepDialog"),
            deny: NONE,
            open: |a| {
                let _ = open_small_form(a);
            },
        },
        State {
            name: "Angle sweep, Two Tiers finished sweep",
            base: Base::Design,
            scope: Some("SweepDialog"),
            deny: NONE,
            open: finished_sweep,
        },
    ]
}

/// The range of one tier as the smallest grid: from one degree below the middle of the default
/// range (the tier's own angle) to the middle, `None` when the fields do not read as numbers.
fn one_degree_range(
    from: &SharedString,
    to: &SharedString,
) -> Option<(SharedString, SharedString)> {
    let low: f64 = from.trim().parse().ok()?;
    let high: f64 = to.trim().parse().ok()?;
    let centre = f64::midpoint(low, high);
    Some((
        format!("{:.2}", centre - 1.0).into(),
        format!("{centre:.2}").into(),
    ))
}

/// Opens the Angle Sweep dialog over the Edit view, switches it to Two Tiers and narrows both
/// tiers to two angles each (step 1). `false` when the dialog did not open, the mode did not
/// switch or the narrowed form cannot run.
fn open_small_form(a: &HarnessApp) -> bool {
    edit_tab(a);
    let sweep = a.ui().global::<SweepModel>();
    sweep.invoke_open_dialog();
    pump(300);
    if !sweep.get_dialog_open() {
        return false;
    }
    // The pill's own two steps: the mode, then the callback that fills the form.
    sweep.set_mode(1);
    sweep.invoke_mode_changed();
    pump(200);
    let across = one_degree_range(&sweep.get_from_text(), &sweep.get_to_text());
    let down = one_degree_range(&sweep.get_from_b_text(), &sweep.get_to_b_text());
    let (Some((from_a, to_a)), Some((from_b, to_b))) = (across, down) else {
        return false;
    };
    sweep.set_from_text(from_a);
    sweep.set_to_text(to_a);
    sweep.set_step_text("1".into());
    sweep.set_from_b_text(from_b);
    sweep.set_to_b_text(to_b);
    sweep.set_step_b_text("1".into());
    sweep.invoke_form_changed_2d();
    pump(100);
    sweep.get_mode() == 1 && sweep.get_form_ok_2d()
}

/// The form of [`open_small_form`], the sweep run to its end and a pair the design is not at
/// selected on the heat map (so "Use These Angles" and "Refine Around This" are enabled).
fn finished_sweep(a: &HarnessApp) {
    if !open_small_form(a) {
        return;
    }
    let sweep = a.ui().global::<SweepModel>();
    sweep.invoke_run_2d();
    let _ = wait_until(Duration::from_secs(SWEEP_WAIT_SECS), || {
        sweep.get_has_results_2d() && !sweep.get_running()
    });
    // A click on the map selects the pair under it; the pair the design is at, and a pair that
    // gives no stone, cannot be used, so the four quarters are tried in turn.
    for (x, y) in [(0.2, 0.2), (0.8, 0.2), (0.2, 0.8), (0.8, 0.8)] {
        sweep.invoke_heat_pointer(x, y, 1);
        pump(100);
        if sweep.get_cells_usable() {
            break;
        }
    }
    pump(250);
}

/// A form of 289 angle pairs with the tilt averages on: Run asks first. "Cancel" drops the
/// question; Run again and "Run Anyway" starts the sweep, which is stopped with the Run button's
/// own Cancel (the cells finished so far still arrive).
fn tilt_question(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    edit_tab(ctx.app);
    let ui = ctx.app.ui().clone_strong();
    let sweep = ui.global::<SweepModel>();
    sweep.invoke_open_dialog();
    pump(300);
    if !ctx.expect(
        sweep.get_dialog_open(),
        "overlay-opens",
        "Angle Sweep: its open flag is false after the normal open route",
    ) {
        return;
    }
    sweep.set_mode(1);
    sweep.invoke_mode_changed();
    pump(200);
    // The default ranges are four degrees wide; a quarter-degree step makes 17 x 17 pairs.
    sweep.set_step_text("0.25".into());
    sweep.set_step_b_text("0.25".into());
    sweep.set_tilt_average(true);
    sweep.invoke_form_changed_2d();
    pump(100);
    if !ctx.expect(
        sweep.get_mode() == 1 && sweep.get_form_ok_2d(),
        "sweep-tilt-question",
        format!(
            "the quarter-degree Two Tiers form with tilt averages cannot run: {}",
            sweep.get_cells_summary()
        ),
    ) {
        sweep.set_dialog_open(false);
        return;
    }

    // First answer: Cancel drops the question and starts nothing.
    sweep.invoke_run_2d();
    pump(200);
    let asked = !sweep.get_confirm_tilt_text().is_empty();
    ctx.expect(
        asked && !sweep.get_running(),
        "sweep-tilt-question",
        "Run on 289 pairs with tilt averages did not ask before starting",
    );
    let cancelled = asked && press(ctx.app, "Cancel", None);
    pump(200);
    ctx.expect(
        cancelled && sweep.get_confirm_tilt_text().is_empty() && !sweep.get_running(),
        "sweep-tilt-question",
        "the question's Cancel did not drop the question or it started the sweep",
    );

    // Second answer: Run Anyway starts the sweep; the Run button's Cancel stops it.
    sweep.invoke_run_2d();
    pump(200);
    let asked_again = !sweep.get_confirm_tilt_text().is_empty();
    let started = asked_again && press(ctx.app, "Run Anyway", None);
    pump(200);
    ctx.expect(
        started && sweep.get_confirm_tilt_text().is_empty() && sweep.get_running(),
        "sweep-tilt-question",
        "Run Anyway did not start the sweep with tilt averages",
    );
    if sweep.get_running() && !press(ctx.app, "Cancel", None) {
        sweep.invoke_cancel();
    }
    let stopped = wait_until(Duration::from_secs(SWEEP_WAIT_SECS), || {
        !sweep.get_running()
    });
    ctx.expect(
        stopped,
        "sweep-tilt-question",
        "the Two Tiers sweep did not stop after Cancel",
    );
    ctx.press("Escape");
    pump(200);
    if sweep.get_dialog_open() {
        sweep.set_dialog_open(false);
        pump(150);
    }
    ctx.check_health("Angle Sweep tilt question answered both ways");
}
