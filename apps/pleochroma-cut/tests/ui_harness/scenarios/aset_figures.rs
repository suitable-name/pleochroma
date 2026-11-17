//! Scenarios: the face-up Contrast figure and the Zones (ASET-style) block.
//!
//! `aset_figures_live_card` brings up the Live Render card in Path-Traced view, feeds it the figures
//! the render thread would push and checks what the card shows: the Contrast row and the zones block
//! with its split line in the Advanced interface, the `?` button that opens the "How the Face-Up
//! Figures Are Measured" popover and the popover's closing, the absence of both when the figures
//! are not measured, and the Simple interface (Contrast only, no zones block). It also pins the
//! wording: no label reads "ASET Zones", and the popover carries the attribution sentence.
//!
//! `aset_figures_edit_and_compare` checks the Edit card's Contrast cell and the Compare window's
//! Contrast row, its Zones (ASET-style) row and the lighting footnote, and that the zones row is
//! gone when the sides have no zones.

use super::{
    Scenario,
    editor::{Card, create_template, show_edit_tab},
    existing_install,
    main_window::{click, select_render_view, select_top_tab_model},
};
use crate::support::{
    Ctx, SecondaryWindow, WindowView,
    app::{pump, wait_until},
};
use pleochroma_cut::{
    BaselineCell, BaselineModel, CompareMetricRow, CompareModel, EditorModel, MainWindow,
    PreferencesModel, ViewportModel, harness_support,
};
use slint::{
    ComponentHandle, Model, ModelRc, VecModel,
    platform::{Key, WindowEvent},
};
use std::time::Duration;

/// The card's accessible label (the whole card is the copy button).
const CARD_LABEL: &str = "Copy the Face-Up Figures";
/// The `?` button's accessible label.
const HELP_LABEL: &str = "How these figures are measured";
/// The block title as the card prints it.
const ZONES_TITLE: &str = "ZONES (ASET-STYLE)";
/// The popover's title.
const POPOVER_TITLE: &str = "How the Face-Up Figures Are Measured";
/// The part of the popover's attribution line that is checked.
const ATTRIBUTION: &str = "registered trademark of GIA";
/// The zones as shares of the face: red, green, blue, dark (they sum to 100).
const SHARES: [f32; 4] = [44.0, 19.0, 16.0, 21.0];
/// The split line the card prints under the legend.
const SPLIT: &str = "Dark = Leak 9 % + Lost 12 %";
/// The legend cells' accessible labels: the cells print bare numbers, the labels keep the full
/// reading.
const LEGEND: [&str; 4] = ["Red 44 %", "Green 19 %", "Blue 16 %", "Dark 21 %"];
/// The block's note beside its title ("any lighting" moved into the popover).
const ZONES_NOTE: &str = "% of the face";
/// The popover's footer link.
const MANUAL_LINK: &str = "Open in the Manual";

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "aset_figures_live_card",
            seed: existing_install,
            run: live_card,
        },
        Scenario {
            name: "aset_figures_edit_and_compare",
            seed: existing_install,
            run: edit_and_compare,
        },
    ]
}

/// Writes the figures the render thread pushes: Contrast and the zone shares, or "not measured"
/// (`None`).
fn feed(ui: &MainWindow, figures: Option<(f32, &[f32], &str)>) {
    let viewport = ui.global::<ViewportModel>();
    viewport.set_brilliance_pct(62.0);
    viewport.set_fire_index(0.42);
    viewport.set_scintillation_pct(61.0);
    viewport.set_windowing_pct(9.0);
    viewport.set_extinction_pct(29.0);
    match figures {
        Some((contrast, shares, split)) => {
            viewport.set_contrast(contrast);
            viewport.set_aset_shares(ModelRc::new(VecModel::from(shares.to_vec())));
            viewport.set_aset_split_text(split.into());
        }
        None => {
            viewport.set_contrast(-1.0);
            viewport.set_aset_shares(ModelRc::default());
            viewport.set_aset_split_text("".into());
        }
    }
    pump(250);
}

/// Whether some accessible label of the main window contains `part`.
fn any_label_contains(view: &WindowView, part: &str) -> bool {
    view.labels().iter().any(|label| label.contains(part))
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

/// Switches the interface mode through its header pill, or through the model when the pill cannot
/// be found.
fn set_interface(ctx: &mut Ctx<'_>, simple: bool) {
    let label = if simple {
        "Simple Interface"
    } else {
        "Advanced Interface"
    };
    if !click(ctx.app, label) {
        let prefs = ctx.app.ui().global::<PreferencesModel>();
        prefs.set_simple_mode(simple);
        prefs.invoke_simple_mode_changed(simple);
    }
    pump(250);
}

/// A design in the Live Render tab, Path-Traced, with the figures card open and no solve running.
fn open_live_card(ctx: &mut Ctx<'_>) -> bool {
    show_edit_tab(ctx.app);
    if !ctx.expect(
        create_template(ctx.app, Card::First),
        "template-design",
        "no design could be created from a template",
    ) {
        return false;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.wait_for(
        "the new design's first solve ends",
        Duration::from_secs(30),
        || !editor.get_solve_running(),
    );
    select_top_tab_model(ctx.app, 0);
    select_render_view(ctx.app, 0);
    ui.global::<ViewportModel>().set_live_view_mode(1);
    pump(400);
    true
}

/// The card, the Contrast row, the zones block, the popover and the Simple interface.
#[expect(
    clippy::too_many_lines,
    reason = "one sequential UI scenario: card, rows, popover and the Simple interface"
)]
fn live_card(ctx: &mut Ctx<'_>) {
    if !open_live_card(ctx) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let viewport = ui.global::<ViewportModel>();

    // Advanced: the Contrast row and the zones block with its split line.
    feed(&ui, Some((27.3, &SHARES, SPLIT)));
    let view = ctx.app.main_view();
    ctx.expect(
        view.has(CARD_LABEL),
        "aset-card",
        format!("the Live Render figures card ({CARD_LABEL:?}) is not showing"),
    );
    ctx.expect(
        view.has("Contrast"),
        "aset-contrast-row",
        "the card shows no Contrast row once a Contrast figure is measured",
    );
    ctx.expect(
        view.has(ZONES_TITLE),
        "aset-block-advanced",
        format!("the Advanced card shows no {ZONES_TITLE:?} block"),
    );
    ctx.expect(
        view.has(SPLIT),
        "aset-split-line",
        format!("the Advanced card shows no {SPLIT:?} line"),
    );
    ctx.expect(
        view.has(ZONES_NOTE),
        "aset-note",
        format!("the zones block does not say {ZONES_NOTE:?} in its header"),
    );
    ctx.expect(
        !view.has("any lighting"),
        "aset-note-moved",
        "the zones block still carries the \"any lighting\" note (it belongs in the popover)",
    );
    for label in LEGEND {
        ctx.expect(
            view.has(label),
            "aset-legend-cell",
            format!("the zones legend has no cell labeled {label:?}"),
        );
    }
    ctx.expect(
        !any_label_contains(&view, "ASET Zones"),
        "aset-name",
        "a label of the card reads \"ASET Zones\"",
    );
    ctx.check_health("Live Render card with Contrast and zones");
    ctx.sweep_main_at_sizes("Live Render, figures card with Contrast and zones");
    feed(&ui, Some((27.3, &SHARES, SPLIT)));

    // The ? button opens the popover; the title and the attribution line are in it.
    let pressed = click(ctx.app, HELP_LABEL);
    ctx.expect(
        pressed,
        "aset-help-button",
        format!("the card's {HELP_LABEL:?} button was not found"),
    );
    if !pressed {
        // GAP: the button is not reachable by label; the write it makes is driven.
        viewport.set_figures_help_open(true);
    }
    pump(300);
    ctx.expect(
        viewport.get_figures_help_open(),
        "aset-help-open",
        "the ? button did not open the popover (ViewportModel.figures_help_open is false)",
    );
    let view = ctx.app.main_view();
    ctx.expect(
        view.has(POPOVER_TITLE) || any_label_contains(&view, POPOVER_TITLE),
        "aset-popover-title",
        format!("the popover shows no {POPOVER_TITLE:?} title"),
    );
    ctx.expect(
        any_label_contains(&view, ATTRIBUTION),
        "aset-popover-attribution",
        format!("the popover carries no attribution line (no label contains {ATTRIBUTION:?})"),
    );
    ctx.expect(
        view.has(MANUAL_LINK),
        "aset-popover-manual",
        format!("the popover shows no {MANUAL_LINK:?} link in its footer"),
    );
    ctx.expect(
        !any_label_contains(&view, "ASET Zones"),
        "aset-name-popover",
        "a label of the popover reads \"ASET Zones\"",
    );
    ctx.check_health("figures popover open");
    ctx.sweep_main_at_sizes("Live Render, figures popover");
    if !viewport.get_figures_help_open() {
        // The size sweep closed it (a resize or an outside press); open it again to close it.
        viewport.set_figures_help_open(true);
        pump(200);
    }
    press_escape(&ui);
    if viewport.get_figures_help_open() {
        // GAP: Escape does not close it here; the property is the popover's own state.
        viewport.set_figures_help_open(false);
        pump(200);
        ctx.expect(
            false,
            "aset-popover-escape",
            "Escape did not close the figures popover",
        );
    }
    ctx.check_health("figures popover closed");

    // Not measured: neither the row nor the block.
    feed(&ui, None);
    let view = ctx.app.main_view();
    ctx.expect(
        !view.has("Contrast") && !view.has(ZONES_TITLE),
        "aset-absent-unmeasured",
        "the card shows Contrast or the zones block although nothing is measured",
    );

    // Simple: Contrast stays, the zones block goes.
    feed(&ui, Some((27.3, &SHARES, SPLIT)));
    set_interface(ctx, true);
    feed(&ui, Some((27.3, &SHARES, SPLIT)));
    let view = ctx.app.main_view();
    ctx.expect(
        view.has("Contrast"),
        "aset-contrast-simple",
        "the Simple interface lost the Contrast row",
    );
    ctx.expect(
        !view.has(ZONES_TITLE) && !view.has(SPLIT),
        "aset-block-simple",
        "the Simple interface shows the Zones (ASET-style) block",
    );
    ctx.check_health("Live Render card, Simple interface");
    ctx.sweep_main_at_sizes("Live Render, figures card, Simple interface");
    set_interface(ctx, false);
    feed(&ui, Some((27.3, &SHARES, SPLIT)));
    let view = ctx.app.main_view();
    ctx.expect(
        view.has(ZONES_TITLE),
        "aset-block-back",
        "the zones block did not return with the Advanced interface",
    );
    ctx.check_health("Live Render card, Advanced interface again");
}

/// The eleven Baseline cells of the Edit card, with Contrast after Extinction.
fn baseline_cells() -> Vec<BaselineCell> {
    let cell = |name: &str, short: &str, value: &str, delta: &str| BaselineCell {
        short_label: short.into(),
        text: format!("{value} {delta}").into(),
        tone: 0,
        tooltip: "".into(),
        name: name.into(),
        value: value.into(),
        delta: delta.into(),
        flag: false,
    };
    vec![
        cell("Brilliance", "Bril.", "62.1 %", "+1.8"),
        cell("Fire", "Fire", "0.42", "+0.01"),
        cell("Scintillation", "Scint.", "61.2 %", "-0.4"),
        cell("Windowing", "Wind.", "9.0 %", "-0.7"),
        cell("Extinction", "Ext.", "28.9 %", "-1.1"),
        cell("Contrast", "Contr.", "27.3", "+9"),
        cell("Yield", "Yield", "41 %", "0"),
        cell("Warnings", "Warn.", "0", "0"),
        cell("Brilliance, tilt", "Tilt B.", "55.0 %", "+1.0"),
        cell("Windowing, tilt", "Tilt W.", "12.0 %", "-0.5"),
        cell("Extinction, tilt", "Tilt E.", "30.0 %", "-0.9"),
    ]
}

/// Shows the Edit card's cells (the solve that built them has ended, so nothing rewrites them).
fn show_baseline_cells(ui: &MainWindow, cells: Vec<BaselineCell>) {
    let baseline = ui.global::<BaselineModel>();
    baseline.set_has_baseline(true);
    baseline.set_cells(ModelRc::new(VecModel::from(cells)));
    pump(300);
}

/// The Compare window's rows: a Contrast row and, with `zones`, the Zones (ASET-style) row.
fn feed_compare(window: &pleochroma_cut::CompareWindow, zones: bool) {
    let compare = window.global::<CompareModel>();
    let row = |label: &str, before: &str, after: &str, change: &str, verdict: &str, tone: i32| {
        CompareMetricRow {
            label: label.into(),
            before: before.into(),
            after: after.into(),
            change: change.into(),
            verdict: verdict.into(),
            tone,
        }
    };
    compare.set_metric_rows(ModelRc::new(VecModel::from(vec![
        row("Brilliance", "62.1", "60.4", "-1.7", "about the same", 0),
        row("Contrast", "27.3", "36.1", "+8.8", "better", 1),
    ])));
    compare.set_metric_summary("The after side has more contrast (+9).".into());
    compare.set_has_aset(zones);
    if zones {
        compare.set_aset_before(ModelRc::new(VecModel::from(vec![44.0, 19.0, 33.0, 4.0])));
        compare.set_aset_after(ModelRc::new(VecModel::from(vec![44.0, 19.0, 16.0, 21.0])));
        compare.set_aset_change("Blue -17, Dark +17".into());
    } else {
        compare.set_aset_before(ModelRc::default());
        compare.set_aset_after(ModelRc::default());
        compare.set_aset_change("".into());
    }
    pump(300);
}

/// The Edit card's Contrast cell, then the Compare window's Contrast and zones rows.
#[expect(
    clippy::too_many_lines,
    reason = "one sequential UI scenario: the Edit card, then the Compare window"
)]
fn edit_and_compare(ctx: &mut Ctx<'_>) {
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
    ctx.wait_for(
        "the new design's first solve ends",
        Duration::from_secs(30),
        || !editor.get_solve_running(),
    );

    // The Edit card: Contrast after Extinction.
    select_top_tab_model(ctx.app, 0);
    select_render_view(ctx.app, 1);
    show_baseline_cells(&ui, baseline_cells());
    let view = ctx.app.main_view();
    ctx.expect(
        view.has("Contrast"),
        "aset-edit-contrast",
        "the Edit figures card shows no Contrast cell",
    );
    ctx.expect(
        !view.has(ZONES_TITLE),
        "aset-edit-no-zones",
        "the Edit figures card shows the zones block (it holds Contrast only)",
    );
    ctx.expect(
        view.has(HELP_LABEL),
        "aset-edit-help",
        format!("the Edit figures card has no {HELP_LABEL:?} button"),
    );
    let viewport = ui.global::<ViewportModel>();
    if click(ctx.app, HELP_LABEL) {
        pump(300);
        ctx.expect(
            viewport.get_figures_help_open(),
            "aset-edit-help-open",
            "the Edit card's ? button did not open the popover",
        );
        let popover = ctx.app.main_view();
        ctx.expect(
            popover.has(MANUAL_LINK),
            "aset-edit-popover-manual",
            format!("the Edit card's popover shows no {MANUAL_LINK:?} link"),
        );
        viewport.set_figures_help_open(false);
        pump(200);
    }
    ctx.check_health("Edit card with Contrast");
    ctx.sweep_main_at_sizes("Edit view, figures card with Contrast");
    let without: Vec<BaselineCell> = baseline_cells()
        .into_iter()
        .filter(|cell| cell.name.as_str() != "Contrast")
        .collect();
    show_baseline_cells(&ui, without);
    ctx.expect(
        !ctx.app.main_view().has("Contrast"),
        "aset-edit-contrast-absent",
        "the Edit figures card shows Contrast although the design has none",
    );
    show_baseline_cells(&ui, baseline_cells());

    // Compare: a snapshot, then the window.
    editor.invoke_snapshot_design();
    ctx.wait_for("a Baseline is held", Duration::from_secs(15), || {
        editor.get_has_snapshot()
    });
    ui.global::<CompareModel>().invoke_open_snapshot();
    let opened = ctx.wait_for("the Compare window opens", Duration::from_secs(10), || {
        harness_support::compare_window().is_some()
    });
    let Some(window) = harness_support::compare_window().filter(|_| opened) else {
        return;
    };
    // The window's own solve lands later and rewrites the rows; they are written once it has.
    let _ = wait_until(Duration::from_secs(20), || {
        window
            .global::<CompareModel>()
            .get_metric_rows()
            .row_count()
            > 0
    });
    feed_compare(&window, true);
    let view = ctx.secondary_view(SecondaryWindow::Compare);
    let Some(view) = view else {
        ctx.expect(false, "aset-compare-view", "the Compare window has no view");
        return;
    };
    ctx.expect(
        any_label_contains(&view, "Contrast: 27.3 before"),
        "aset-compare-contrast",
        "the Compare table has no Contrast row",
    );
    ctx.expect(
        view.has("Zones (ASET-style): Blue -17, Dark +17 points"),
        "aset-compare-zones",
        "the Compare table has no Zones (ASET-style) row with its change",
    );
    ctx.expect(
        any_label_contains(
            &view,
            "The zones and Contrast do not depend on the lighting.",
        ),
        "aset-compare-footnote",
        "the Compare footnote does not say the zones and Contrast ignore the lighting",
    );
    ctx.expect(
        !any_label_contains(&view, "ASET Zones"),
        "aset-name-compare",
        "a label of the Compare window reads \"ASET Zones\"",
    );
    ctx.sweep_secondary(SecondaryWindow::Compare, "Compare, Contrast and zones rows");
    ctx.check_health("Compare with zones");

    feed_compare(&window, false);
    if let Some(view) = ctx.secondary_view(SecondaryWindow::Compare) {
        ctx.expect(
            !any_label_contains(&view, "Zones (ASET-style)"),
            "aset-compare-no-zones",
            "the Compare table shows a zones row although neither side has zones",
        );
    }
    window.global::<CompareModel>().invoke_close();
    pump(400);
    ctx.check_health("Compare closed");
}
