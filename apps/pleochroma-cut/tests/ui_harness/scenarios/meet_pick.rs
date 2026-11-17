//! Scenario: Meet Point pick mode on the Standard Round Brilliant.
//!
//! Lower Girdle is made to meet the named facets Pavilion Main and Girdle, which gives it corners to
//! pick from. The journey arms pick mode from the tier form's `Pick in View` button, waits for the
//! rings, picks one through `ManipulateModel.pick_click`, checks the row (state, chip, note) and the
//! History tab (one new step), takes the pick back with Undo, arms again from the toolbar's
//! `Pick Meet` pill and leaves with Escape.

use super::{
    Scenario,
    editor::{Card, create_template, show_edit_tab},
    existing_install,
    main_window::click,
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
};
use pleochroma_cut::{EditorModel, HistoryModel, MainWindow, ManipulateModel};
use slint::{
    ComponentHandle, Model,
    platform::{Key, WindowEvent},
};
use std::time::Duration;

/// `EditorModel.inspector_tab` of the Tier form.
const TIER_TAB: i32 = 0;
/// `EditorModel.inspector_tab` of the History tab.
const HISTORY_TAB: i32 = 4;
/// `EditorModel.meet_pick_state` of a tier without a pick.
const STATE_NONE: i32 = 0;
/// `EditorModel.meet_pick_state` of a pick that holds.
const STATE_MET: i32 = 1;

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![Scenario {
        name: "meet_pick_mode",
        seed: existing_install,
        run: meet_pick_mode,
    }]
}

/// Shows the inspector tab `tab`, opened and tall enough to hold its body.
fn show_inspector_tab(ui: &MainWindow, tab: i32) {
    let editor = ui.global::<EditorModel>();
    editor.set_inspector_collapsed(false);
    editor.set_inspector_tab(tab);
    editor.set_inspector_height(2400.0);
    pump(150);
}

/// The number of History rows once the History tab is showing (the list is filled only while the
/// tab is on screen), with the Tier form shown again afterwards.
fn history_rows(ui: &MainWindow) -> usize {
    let history = ui.global::<HistoryModel>();
    show_inspector_tab(ui, HISTORY_TAB);
    let _ = wait_until(Duration::from_secs(3), || {
        history.get_rows().row_count() >= 1
    });
    let rows = history.get_rows().row_count();
    show_inspector_tab(ui, TIER_TAB);
    rows
}

/// The centre of the first ring a click can pick (`kind` 0), in the viewport's logical pixels.
fn first_valid_ring(ui: &MainWindow) -> Option<(f32, f32)> {
    ui.global::<ManipulateModel>()
        .get_pick_rings()
        .iter()
        .find(|ring| ring.kind == 0)
        .map(|ring| (ring.x, ring.y))
}

/// Waits until pick mode shows at least one pickable ring.
fn wait_for_rings(ctx: &mut Ctx<'_>, ui: &MainWindow, label: &str) -> bool {
    ctx.wait_for(label, Duration::from_secs(30), || {
        first_valid_ring(ui).is_some()
    })
}

/// Presses Escape in the main window.
fn press_escape(ui: &MainWindow) {
    let window = ui.window();
    window.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    window.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    pump(150);
}

/// Clicks Undo, or fires the callback when the button cannot be found.
fn undo(app: &HarnessApp) {
    if !click(app, "Undo") {
        app.ui().global::<EditorModel>().invoke_undo();
        pump(100);
    }
}

/// Makes Lower Girdle meet the named facets Pavilion Main and Girdle and selects it; returns its
/// row.
fn prepare_lower_girdle(ctx: &mut Ctx<'_>, ui: &MainWindow) -> Option<i32> {
    let editor = ui.global::<EditorModel>();
    let tiers = editor.get_tiers();
    let found = (0..tiers.row_count())
        .filter_map(|i| tiers.row_data(i).map(|row| (i, row)))
        .find(|(_, row)| row.name.as_str() == "Lower Girdle");
    let Some((index, item)) = found else {
        ctx.expect(
            false,
            "pick-tier",
            "the Standard Round Brilliant has no tier named Lower Girdle",
        );
        return None;
    };
    let row = i32::try_from(index).ok()?;
    editor.set_selected_tier_index(row);
    show_inspector_tab(ui, TIER_TAB);
    editor.invoke_save_tier(
        row,
        item.angle_full,
        1,
        "Pavilion Main, Girdle".into(),
        item.name,
        item.indices_full,
    );
    pump(300);
    ctx.wait_for("the Facets edit solves", Duration::from_secs(30), || {
        !editor.get_solve_running()
    });
    Some(row)
}

/// Arms pick mode and waits for the rings. `press` is the label of the control clicked; when no
/// such control is found the model is driven the way the control would. Returns whether the mode
/// is armed with a pickable ring in view.
fn arm(ctx: &mut Ctx<'_>, ui: &MainWindow, press: &str) -> bool {
    let manipulate = ui.global::<ManipulateModel>();
    if !click(ctx.app, press) {
        // GAP: the control is not reachable by label; the two writes it makes are driven.
        manipulate.set_pick_mode(true);
        manipulate.invoke_pick_toggled();
        pump(150);
    }
    let armed = ctx.wait_for(
        &format!("{press} arms pick mode"),
        Duration::from_secs(5),
        || manipulate.get_pick_mode(),
    );
    armed && wait_for_rings(ctx, ui, &format!("{press} shows rings"))
}

/// Arm, rings, pick, row states, History, Undo, arm again, Escape.
#[expect(
    clippy::too_many_lines,
    reason = "one sequential UI scenario from arming the pick to Escape"
)]
fn meet_pick_mode(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    if !ctx.expect(
        create_template(ctx.app, Card::First),
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let manipulate = ui.global::<ManipulateModel>();
    ctx.wait_for(
        "the new design's first solve ends",
        Duration::from_secs(30),
        || !editor.get_solve_running(),
    );
    if prepare_lower_girdle(ctx, &ui).is_none() {
        return;
    }
    let available = ctx.wait_for(
        "Lower Girdle can take a pick",
        Duration::from_secs(10),
        || manipulate.get_pick_available(),
    );
    if !available {
        ctx.report.note(format!(
            "meet_pick_mode: pick_blocked_reason = {:?}",
            manipulate.get_pick_blocked_reason()
        ));
        return;
    }
    ctx.expect(
        editor.get_meet_pick_state() == STATE_NONE,
        "pick-row-none",
        format!(
            "a tier without a pick reports row state {}",
            editor.get_meet_pick_state()
        ),
    );
    let rows_before = history_rows(&ui);

    // Arm from the tier form's button.
    if !arm(ctx, &ui, "Pick in View") {
        ctx.check_health("pick mode armed");
        return;
    }
    pump(300);
    ctx.check_health("pick mode armed");
    ctx.sweep_main_at_sizes("Edit tab in pick mode");
    // The sweep resized the window: the rings were laid out again for the restored size.
    wait_for_rings(ctx, &ui, "rings after the size sweep");

    let Some((x, y)) = first_valid_ring(&ui) else {
        manipulate.set_pick_mode(false);
        manipulate.invoke_pick_toggled();
        return;
    };
    manipulate.invoke_pick_hover(x, y);
    ctx.wait_for(
        "the ring under the pointer lights",
        Duration::from_secs(5),
        || manipulate.get_pick_lit() >= 0,
    );

    // Pick the ring.
    manipulate.invoke_pick_click(x, y);
    ctx.wait_for("the pick ends pick mode", Duration::from_secs(10), || {
        !manipulate.get_pick_mode()
    });
    ctx.wait_for("the row holds the pick", Duration::from_secs(30), || {
        editor.get_meet_pick_state() == STATE_MET
    });
    ctx.expect(
        editor.get_meet_pick_chip().contains('\u{b7}') && !editor.get_meet_pick_note().is_empty(),
        "pick-row-names",
        format!(
            "chip {:?}, note {:?}",
            editor.get_meet_pick_chip(),
            editor.get_meet_pick_note()
        ),
    );
    ctx.check_health("corner picked");
    ctx.sweep_main_at_sizes("Tier form with a picked corner");
    let rows_after = history_rows(&ui);
    ctx.expect(
        rows_after == rows_before + 1,
        "pick-one-history-step",
        format!("History held {rows_before} row(s) before the pick and {rows_after} after"),
    );

    // Undo takes the pick back.
    undo(ctx.app);
    ctx.wait_for("Undo takes the pick back", Duration::from_secs(30), || {
        editor.get_meet_pick_state() == STATE_NONE
    });
    ctx.expect(
        editor.get_meet_pick_chip().is_empty(),
        "pick-undo-chip",
        format!(
            "the chip reads {:?} after Undo",
            editor.get_meet_pick_chip()
        ),
    );

    // Arm from the toolbar's pill, leave with Escape; no edit is made.
    if arm(ctx, &ui, "Pick Meet") {
        press_escape(&ui);
        let left = ctx.wait_for("Escape leaves pick mode", Duration::from_secs(3), || {
            !manipulate.get_pick_mode()
        });
        if !left {
            manipulate.set_pick_mode(false);
            manipulate.invoke_pick_toggled();
        }
        ctx.expect(
            editor.get_can_redo() && editor.get_meet_pick_state() == STATE_NONE,
            "pick-escape-no-edit",
            "leaving pick mode with Escape changed the design",
        );
    }
    ctx.check_health("pick mode left");
}
