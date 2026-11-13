//! Scenarios: the secondary windows (Help, Compare, Locate, Rig, Rough Color, Color Pattern) and
//! what closing the main window does to them. The Rough Planner has its own file.
//!
//! GAP: the Help window has no route to the glossary of its own (`open_glossary` is only handled
//! on the main window), so `help_window` opens the glossary through the main window's `HelpModel`
//! while the Help window is up. GAP: Locate is opened without a mesh rough (its gating button is
//! bypassed through the callback), so the photo and alignment states are not reached; they need a
//! mesh file the harness does not build.

use super::{Scenario, dialogs, existing_install};
use crate::support::{
    Ctx, HarnessApp, SecondaryWindow,
    app::{pump, wait_until},
    coverage,
};
use pleochroma_cut::{
    CompareModel, EditorModel, HelpModel, MainWindow, RoughPlanModel, harness_support,
};
use slint::{ComponentHandle, Model, platform::WindowEvent};
use std::time::Duration;

/// Every secondary window, for "is anything still open" checks.
const ALL_WINDOWS: [SecondaryWindow; 7] = [
    SecondaryWindow::Compare,
    SecondaryWindow::RoughPlanner,
    SecondaryWindow::Locate,
    SecondaryWindow::Rig,
    SecondaryWindow::RoughColor,
    SecondaryWindow::Help,
    SecondaryWindow::ColorPattern,
];

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "help_window",
            seed: existing_install,
            run: help_window,
        },
        Scenario {
            name: "compare_window",
            seed: existing_install,
            run: compare_window,
        },
        Scenario {
            name: "locate_and_rig",
            seed: existing_install,
            run: locate_and_rig,
        },
        Scenario {
            name: "closing_main_closes_all",
            seed: existing_install,
            run: closing_main_closes_all,
        },
        Scenario {
            name: "rough_color_window",
            seed: existing_install,
            run: rough_color_window,
        },
        Scenario {
            name: "color_pattern_window",
            seed: existing_install,
            run: color_pattern_window,
        },
    ]
}

/// Closes a secondary window the way its title-bar button does.
fn close_with_title_bar(window: &slint::Window) {
    window.dispatch_event(WindowEvent::CloseRequested);
    pump(200);
}

/// Waits until `which` is open or closed.
fn wait_open(which: SecondaryWindow, open: bool, seconds: u64) -> bool {
    wait_until(Duration::from_secs(seconds), || {
        harness_support::is_window_open(which) == open
    })
}

/// Opens the Help window with F1 and with `HelpModel.open_topic`; searches the manual and gets
/// hits; opens the glossary; closes the window with its own close callback and with the title
/// bar. The window is open exactly when expected each time.
fn help_window(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ctx.press("F1");
    let opened = wait_open(SecondaryWindow::Help, true, 10);
    ctx.expect(opened, "help-opens-f1", "F1 did not open the Help window");
    if !opened {
        return;
    }
    ctx.sweep_secondary(SecondaryWindow::Help, "opened with F1");
    search_manual(ctx);
    ui.global::<HelpModel>().invoke_open_glossary("".into());
    pump(300);
    ctx.expect(
        ui.global::<HelpModel>().get_glossary_open(),
        "help-glossary",
        "the glossary did not open while the Help window was up",
    );
    ui.global::<HelpModel>().set_glossary_open(false);
    pump(150);
    if let Some(window) = harness_support::help_window() {
        window.global::<HelpModel>().invoke_close_window();
    }
    pump(300);
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Help),
        "help-closes",
        "the Help window is still open after its close callback",
    );
    topic_then_title_bar(ctx, &ui);
    ctx.check_health("Help window");
}

/// Searches the open Help window for a term the manual certainly holds and sweeps the results.
fn search_manual(ctx: &mut Ctx<'_>) {
    let Some(window) = harness_support::help_window() else {
        return;
    };
    let model = window.global::<HelpModel>();
    model.set_search_text("facet".into());
    model.invoke_search("facet".into());
    let found = wait_until(Duration::from_secs(10), || {
        model.get_searching() && model.get_hits().row_count() > 0
    });
    ctx.expect(
        found,
        "help-search",
        format!(
            "searching the manual for \"facet\" gave no hits (status: {})",
            model.get_status()
        ),
    );
    if found {
        coverage::record_callback("HelpModel.search");
        ctx.sweep_secondary(SecondaryWindow::Help, "search results");
    }
    model.set_search_text("".into());
    model.invoke_search("".into());
    pump(200);
}

/// `HelpModel.open_topic` shows the window again with a chapter; the title-bar close hides it.
fn topic_then_title_bar(ctx: &mut Ctx<'_>, ui: &MainWindow) {
    ui.global::<HelpModel>()
        .invoke_open_topic("05-solving#what-solve-does".into());
    let opened = wait_open(SecondaryWindow::Help, true, 10);
    ctx.expect(
        opened,
        "help-opens-topic",
        "HelpModel.open_topic did not open the Help window",
    );
    if !opened {
        return;
    }
    coverage::record_callback("HelpModel.open_topic");
    let chapter = harness_support::help_window()
        .map_or(0, |w| w.global::<HelpModel>().get_blocks().row_count());
    ctx.expect(
        chapter > 0,
        "help-chapter",
        "the opened topic shows no content",
    );
    ctx.sweep_secondary(SecondaryWindow::Help, "opened on a topic");
    if let Some(window) = harness_support::help_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Help),
        "help-closes-title-bar",
        "the Help window is still open after the title-bar close",
    );
}

/// Brings a design and a snapshot of it up, edits the design, and opens the snapshot compare
/// window (what "Compare visually" does); returns once the window is open and its session built.
fn open_snapshot_compare(app: &HarnessApp) -> bool {
    let ui = app.ui().clone_strong();
    if !dialogs::make_template_design(&ui) {
        return false;
    }
    ui.global::<EditorModel>().invoke_snapshot_design();
    let _ = wait_until(Duration::from_secs(15), || {
        ui.global::<EditorModel>().get_has_snapshot()
    });
    if !dialogs::nudge_first_tier(&ui) {
        return false;
    }
    ui.global::<CompareModel>().invoke_open_snapshot();
    if !wait_open(SecondaryWindow::Compare, true, 10) {
        return false;
    }
    wait_until(Duration::from_secs(40), || {
        harness_support::compare_window()
            .is_some_and(|w| !w.global::<CompareModel>().get_status().contains("Solving"))
    })
}

/// A design, a snapshot, an edit; the snapshot compare window opens, solves both sides, sweeps
/// clean, survives a click-everything pass, and closes with its own Close callback.
///
/// No strong clone of the main window is held in this function: the smoke pass relaunches the
/// app, and a clone kept across that keeps the old main window, and every database handle its
/// callbacks hold, alive after its shutdown.
fn compare_window(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    {
        let editor = ctx.app.ui().global::<EditorModel>();
        editor.invoke_snapshot_design();
        let _ = wait_until(Duration::from_secs(15), || editor.get_has_snapshot());
    }
    if !dialogs::edit_design(ctx) {
        return;
    }
    ctx.app.ui().global::<CompareModel>().invoke_open_snapshot();
    let opened = wait_open(SecondaryWindow::Compare, true, 10);
    ctx.expect(
        opened,
        "compare-opens",
        "CompareModel.open_snapshot did not open the compare window (snapshot taken, design edited)",
    );
    if !opened {
        return;
    }
    coverage::record_callback("CompareModel.open_snapshot");
    ctx.wait_for(
        "the compare sides are solved",
        Duration::from_secs(40),
        || {
            harness_support::compare_window()
                .is_some_and(|w| !w.global::<CompareModel>().get_status().contains("Solving"))
        },
    );
    ctx.sweep_secondary(SecondaryWindow::Compare, "snapshot compare");
    ctx.check_health("compare window");
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::Compare,
        "snapshot compare",
        &mut |app: &mut HarnessApp| {
            let _ = open_snapshot_compare(app);
        },
    );
    // The smoke pass relaunched the app; bring the window back for the close check.
    if !harness_support::is_window_open(SecondaryWindow::Compare) && !open_snapshot_compare(ctx.app)
    {
        ctx.expect(
            false,
            "compare-reopens",
            "the compare window could not be reopened",
        );
        return;
    }
    if let Some(window) = harness_support::compare_window() {
        window.global::<CompareModel>().invoke_close();
    }
    pump(400);
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Compare),
        "compare-closes",
        "the compare window is still open after its Close callback",
    );
    coverage::record_callback("CompareModel.close");
    ctx.check_health("compare window closed");
}

/// Locate opens from the planner, Rig from Locate; both sweep; closing Rig then Locate leaves
/// both closed.
fn locate_and_rig(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<RoughPlanModel>().invoke_open();
    let planner = wait_open(SecondaryWindow::RoughPlanner, true, 10);
    ctx.expect(planner, "planner-opens", "the Rough Planner did not open");
    let Some(planner_window) = harness_support::rough_planner_window().filter(|_| planner) else {
        return;
    };
    pump(300);
    planner_window
        .global::<RoughPlanModel>()
        .invoke_locate_inclusion();
    let locate = wait_open(SecondaryWindow::Locate, true, 10);
    ctx.expect(
        locate,
        "locate-opens",
        "locate_inclusion did not open the Locate window",
    );
    if !locate {
        return;
    }
    coverage::record_callback("RoughPlanModel.locate_inclusion");
    ctx.sweep_secondary(SecondaryWindow::Locate, "Locate window");
    if let Some(window) = harness_support::locate_window() {
        window.invoke_edit_rigs();
    }
    let rig = wait_open(SecondaryWindow::Rig, true, 10);
    ctx.expect(rig, "rig-opens", "Edit Rigs did not open the Rig window");
    if rig {
        ctx.sweep_secondary(SecondaryWindow::Rig, "Rig window");
        if let Some(window) = harness_support::rig_window() {
            close_with_title_bar(window.window());
        }
        ctx.expect(
            !harness_support::is_window_open(SecondaryWindow::Rig),
            "rig-closes",
            "the Rig window is still open after the title-bar close",
        );
    }
    if let Some(window) = harness_support::locate_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Locate),
        "locate-closes",
        "the Locate window is still open after the title-bar close",
    );
    ctx.check_health("Locate and Rig windows");
}

/// The Rough colour wizard opens from the planner, sweeps clean, and closes with the title bar.
fn rough_color_window(ctx: &mut Ctx<'_>) {
    use pleochroma_cut::Zoning;
    let ui = ctx.app.ui().clone_strong();
    ui.global::<RoughPlanModel>().invoke_open();
    let planner = wait_open(SecondaryWindow::RoughPlanner, true, 10);
    ctx.expect(planner, "planner-opens", "the Rough Planner did not open");
    let Some(planner_window) = harness_support::rough_planner_window().filter(|_| planner) else {
        return;
    };
    pump(300);
    planner_window.global::<Zoning>().invoke_open_rough_colour();
    let opened = wait_open(SecondaryWindow::RoughColor, true, 10);
    ctx.expect(
        opened,
        "colour-opens",
        "Zoning.open_rough_colour did not open the wizard",
    );
    if !opened {
        return;
    }
    coverage::record_callback("Zoning.open_rough_colour");
    ctx.sweep_secondary(SecondaryWindow::RoughColor, "Rough colour wizard");
    if let Some(window) = harness_support::rough_color_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::RoughColor),
        "colour-closes",
        "the Rough colour wizard is still open after the title-bar close",
    );
    ctx.check_health("Rough colour wizard");
}

/// The Color Pattern window opens on a seeded Watermelon pattern, sweeps clean at both sizes in
/// each view (Slice, Crystal, the swapped pair) and with each overlay up (the Start From gallery,
/// the Add and Layer menus, the color picker), takes edits and an undo, asks before discarding,
/// and closes.
fn color_pattern_window(ctx: &mut Ctx<'_>) {
    use pleochroma_cut::ColorPatternModel;
    let ui = ctx.app.ui().clone_strong();
    harness_support::open_color_pattern_window(&ui);
    let opened = wait_open(SecondaryWindow::ColorPattern, true, 10);
    ctx.expect(
        opened,
        "pattern-opens",
        "the Color Pattern window did not open",
    );
    let Some(window) = harness_support::color_pattern_window().filter(|_| opened) else {
        return;
    };
    pump(300);
    let model = window.global::<ColorPatternModel>();
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "opening view");
    // The view switch: Slice, then Crystal; Stone needs an open design and falls back to Slice
    // without one, so only the two always-present views are asserted.
    for (view, label) in [(1, "Slice view"), (2, "Crystal view")] {
        model.invoke_view_changed(view);
        pump(400);
        ctx.expect(
            model.get_view() == view,
            "pattern-view-switch",
            format!(
                "view_changed({view}) left the stage on view {}",
                model.get_view()
            ),
        );
        ctx.sweep_secondary(SecondaryWindow::ColorPattern, label);
    }
    model.invoke_view_changed(1);
    pump(200);
    // The inset's expand button swaps the main picture with the inset one.
    let before = model.get_view();
    model.invoke_swap_views();
    pump(300);
    ctx.expect(
        model.get_view() != before,
        "pattern-swap-views",
        "swap_views left the main picture unchanged",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "swapped views");
    model.invoke_view_changed(1);
    pump(200);
    pattern_gallery(ctx, &window);
    pattern_layers(ctx, &window);
    pattern_color_forms(ctx, &window);
    pattern_picker(ctx, &window);
    model.invoke_cancel();
    pump(200);
    ctx.expect(
        model.get_confirm_discard(),
        "pattern-discard-asks",
        "Cancel with edits did not ask before discarding",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "discard bar");
    model.invoke_discard_confirmed();
    pump(300);
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::ColorPattern),
        "pattern-closes",
        "the Color Pattern window is still open after Discard",
    );
    ctx.check_health("Color Pattern window");
}

/// The color controls of the Color Pattern window on a sapphire host: the From variant
/// dropdown turns a picked color into a variant of the material (and says where it comes
/// from), and "Custom color" turns it back. Convert gives a one-color (legacy) zone the
/// host's Along and Across form. `ColorPatternModel` is on the coverage allow list
/// (`coverage_allow.rs`): this is where `variant_picked` and `convert_legacy` are driven.
fn pattern_color_forms(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    model.invoke_select_row(0);
    pump(200);
    let count = slint::Model::row_count(&model.get_variant_labels());
    ctx.expect(
        count > 1,
        "pattern-variants-offered",
        "the Color Pattern window of a sapphire offers no color variant besides Custom color",
    );
    // A one-color (legacy) zone shows the amber notice; Convert gives it the host's form. The zone
    // may not be legacy, in which case the callback is a no-op the window must survive.
    let legacy = model.get_legacy_notice();
    model.invoke_convert_legacy();
    pump(300);
    ctx.expect(
        !legacy || !model.get_legacy_notice(),
        "pattern-legacy-converted",
        "Convert left the legacy-color notice up",
    );
    if count < 2 {
        return;
    }
    // The last variant: the zone takes the variant's colors and says where they come from.
    let last = i32::try_from(count).unwrap_or(2) - 1;
    model.invoke_variant_picked(last);
    pump(300);
    ctx.expect(
        model.get_variant_index() == last && !model.get_variant_caption().is_empty(),
        "pattern-variant-picked",
        format!("the From variant entry {last} was not the zone's variant afterwards"),
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "From variant");
    // Custom color turns the variant back into picked colors.
    model.invoke_variant_picked(0);
    pump(300);
    ctx.expect(
        model.get_variant_index() == 0 && model.get_variant_caption().is_empty(),
        "pattern-variant-custom",
        "Custom color did not turn the variant back into picked colors",
    );
    // Every variant of the sapphire turns into picked colors, whether or not its row form carries
    // bands. A variant with a narrow energy band keeps its color and gets bands solved for it; a
    // colorless one takes the host's two-color form.
    for index in 1..count {
        let index = i32::try_from(index).unwrap_or(0);
        model.invoke_variant_picked(index);
        pump(150);
        model.invoke_variant_picked(0);
        pump(150);
        ctx.expect(
            model.get_variant_index() == 0,
            "pattern-variant-each",
            format!(
                "Custom color on variant {index} left the zone on variant {}",
                model.get_variant_index()
            ),
        );
    }
}

/// The names of the layer list, top layer first.
fn pattern_names(model: &pleochroma_cut::ColorPatternModel<'_>) -> Vec<String> {
    model
        .get_zone_rows()
        .iter()
        .map(|row| row.name.to_string())
        .collect()
}

/// The "Start From" gallery: it opens over the window, offers the typical zoning of the sapphire
/// and the templates. It takes a preset name, deletes the saved preset again and closes on a
/// pick.
/// It ends with a typical pattern in place, so the layer pass has layers to work on.
fn pattern_gallery(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    model.invoke_open_gallery();
    pump(400);
    ctx.expect(
        model.get_gallery_open(),
        "pattern-gallery-opens",
        "open_gallery did not open the gallery",
    );
    // The harness material is a sapphire: its typical patterns are offered in their own section.
    ctx.expect(
        slint::Model::row_count(&model.get_typical_tiles()) == 3
            && !model.get_typical_section().is_empty(),
        "pattern-typical-offered",
        "the gallery did not offer the three typical sapphire patterns",
    );
    ctx.expect(
        slint::Model::row_count(&model.get_template_tiles()) > 0,
        "pattern-templates-offered",
        "the gallery offered no template",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "Start From gallery");
    // The name field of "Save Current" suggests a name and goes away on cancel.
    model.invoke_save_preset();
    pump(200);
    ctx.expect(
        model.get_preset_naming() && !model.get_preset_name().is_empty(),
        "pattern-preset-naming",
        "Save Current did not open the name field with a suggested name",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "gallery preset name");
    model.invoke_preset_name_cancelled();
    pump(200);
    ctx.expect(
        !model.get_preset_naming(),
        "pattern-preset-cancelled",
        "the name field stayed open after it was cancelled",
    );
    pattern_gallery_picks(ctx, window);
}

/// The second half of the gallery pass: a saved preset's tile and its delete, then a pick from
/// each section and a close without a pick.
fn pattern_gallery_picks(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    // Save a preset, find its tile, delete it again.
    let name = "Harness Preset";
    model.invoke_save_preset();
    pump(100);
    model.invoke_preset_name_committed(name.into());
    pump(300);
    let tile = model
        .get_preset_tiles()
        .iter()
        .position(|tile| tile.label.as_str() == name);
    ctx.expect(
        tile.is_some(),
        "pattern-preset-saved",
        "a committed preset name did not add a preset tile",
    );
    if let Some(tile) = tile {
        model.invoke_preset_delete(i32::try_from(tile).unwrap_or(0));
        pump(300);
        ctx.expect(
            model
                .get_preset_tiles()
                .iter()
                .all(|tile| tile.label.as_str() != name),
            "pattern-preset-deleted",
            "preset_delete left the preset's tile in the gallery",
        );
    }
    // Picking a tile closes the gallery and is one undo step.
    model.invoke_template_picked(0);
    pump(300);
    ctx.expect(
        !model.get_gallery_open() && model.get_can_undo(),
        "pattern-template-picked",
        "a template pick did not close the gallery and offer Undo",
    );
    model.invoke_undo();
    pump(200);
    model.invoke_open_gallery();
    pump(200);
    if slint::Model::row_count(&model.get_preset_tiles()) > 0 {
        model.invoke_preset_picked(0);
        pump(300);
        ctx.expect(
            !model.get_gallery_open(),
            "pattern-preset-picked",
            "a preset pick did not close the gallery",
        );
        model.invoke_open_gallery();
        pump(200);
    }
    model.invoke_typical_picked(0);
    pump(300);
    ctx.expect(
        !model.get_gallery_open() && model.get_can_undo(),
        "pattern-typical-edits",
        "a typical pattern pick did not close the gallery and offer Undo",
    );
    // Close without a pick, too.
    model.invoke_open_gallery();
    pump(200);
    model.invoke_close_gallery();
    pump(200);
    ctx.expect(
        !model.get_gallery_open(),
        "pattern-gallery-closes",
        "close_gallery left the gallery open",
    );
}

/// The layer list: select, hide, add (from the Add menu), rename, duplicate, move, reorder and
/// delete, with the Add menu and a card's menu opened by clicking their buttons.
fn pattern_layers(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use super::main_window::click_in;
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    let rows = |model: &pleochroma_cut::ColorPatternModel<'_>| {
        slint::Model::row_count(&model.get_zone_rows())
    };
    ctx.expect(
        rows(&model) >= 3,
        "pattern-layers-listed",
        "a typical pattern did not give at least two layers and the base",
    );
    if rows(&model) < 3 {
        return;
    }
    model.invoke_select_row(1);
    pump(200);
    ctx.expect(
        model.get_selected_row() == 1,
        "pattern-select-row",
        "select_row(1) did not select the second row",
    );
    // The eye hides a layer for this session only: no undo step.
    let undo = model.get_can_undo();
    model.invoke_toggle_hidden(0);
    pump(200);
    ctx.expect(
        model
            .get_zone_rows()
            .row_data(0)
            .is_some_and(|row| row.hidden)
            && model.get_can_undo() == undo,
        "pattern-hide-layer",
        "toggle_hidden did not hide the top layer without an undo step",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "hidden layer");
    model.invoke_toggle_hidden(0);
    pump(200);
    ctx.expect(
        model
            .get_zone_rows()
            .row_data(0)
            .is_some_and(|row| !row.hidden),
        "pattern-show-layer",
        "a second toggle_hidden did not show the top layer again",
    );
    // Add from the Add menu: click "Add", look at the menu, click its first shape.
    let before = rows(&model);
    if model.get_can_add_zone() {
        if let Some(view) = ctx.secondary_view(SecondaryWindow::ColorPattern) {
            let opened = click_in(&view, "Add");
            pump(200);
            ctx.expect(opened, "pattern-add-button", "the Add button was not found");
            ctx.sweep_secondary(SecondaryWindow::ColorPattern, "Add menu");
            let shape = model
                .get_shape_options()
                .row_data(0)
                .map(|shape| shape.to_string())
                .unwrap_or_default();
            if let Some(view) = ctx.secondary_view(SecondaryWindow::ColorPattern) {
                // The Advanced shape row carries the same names as radio buttons, ahead of the
                // menu in document order: take the menu entry, which is a button.
                let added = click_button_in(&view, &shape);
                pump(300);
                ctx.expect(
                    added && rows(&model) == before + 1 && model.get_selected_row() == 0,
                    "pattern-add-menu",
                    format!("the Add menu's '{shape}' did not add a selected top layer"),
                );
            }
        }
    } else {
        ctx.expect(
            !model.get_add_zone_hint().is_empty(),
            "pattern-add-hint",
            "a full pattern gave no reason why Add is off",
        );
    }
    model.invoke_add_zone(1);
    pump(200);
    // Rename, then duplicate when there is room.
    model.invoke_rename_row(0, "Harness Core".into());
    pump(200);
    ctx.expect(
        pattern_names(&model)
            .first()
            .is_some_and(|name| name == "Harness Core"),
        "pattern-rename",
        "rename_row did not rename the top layer",
    );
    let count = rows(&model);
    if model.get_can_add_zone() {
        model.invoke_duplicate_row(0);
        pump(200);
        ctx.expect(
            rows(&model) == count + 1,
            "pattern-duplicate",
            "duplicate_row did not add a layer",
        );
    }
    pattern_layer_moves(ctx, window);
}

/// Activates the first button (not a radio button or tab of the same name) labelled `label`.
fn click_button_in(view: &crate::support::WindowView, label: &str) -> bool {
    let target = view
        .by_label(label)
        .into_iter()
        .find(|e| e.accessible_role() == Some(uiprobe::AccessibleRole::Button));
    target.is_some_and(|element| {
        element.invoke_accessible_default_action();
        true
    })
}

/// The second half of the layer pass: the card menu's Move Down, then move, reorder and delete
/// through the model callbacks.
fn pattern_layer_moves(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use super::main_window::click_in;
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    let rows = |model: &pleochroma_cut::ColorPatternModel<'_>| {
        slint::Model::row_count(&model.get_zone_rows())
    };
    let names = pattern_names(&model);
    if let Some(view) = ctx.secondary_view(SecondaryWindow::ColorPattern) {
        let opened = click_in(&view, "Layer menu");
        pump(200);
        ctx.expect(
            opened,
            "pattern-card-menu",
            "a card's Layer menu button was not found",
        );
        ctx.sweep_secondary(SecondaryWindow::ColorPattern, "Layer menu");
        if let Some(view) = ctx.secondary_view(SecondaryWindow::ColorPattern) {
            click_in(&view, "Move Down");
            pump(300);
        }
        ctx.expect(
            pattern_names(&model).get(1) == names.first(),
            "pattern-move-menu",
            "the Layer menu's Move Down did not move the top layer below the second one",
        );
    }
    let names = pattern_names(&model);
    model.invoke_move_row(1, -1);
    pump(200);
    ctx.expect(
        pattern_names(&model).first() == names.get(1),
        "pattern-move-up",
        "move_row(1, -1) did not move the second layer to the top",
    );
    let names = pattern_names(&model);
    model.invoke_reorder_row(0, 1);
    pump(200);
    ctx.expect(
        pattern_names(&model).get(1) == names.first(),
        "pattern-reorder",
        "reorder_row(0, 1) did not drop the top layer on the second row",
    );
    // The base can neither be moved nor deleted.
    let last = i32::try_from(rows(&model)).unwrap_or(1) - 1;
    let base_rows = rows(&model);
    model.invoke_delete_row(last);
    pump(200);
    ctx.expect(
        rows(&model) == base_rows,
        "pattern-base-stays",
        "delete_row removed the base layer",
    );
    model.invoke_delete_row(0);
    pump(200);
    ctx.expect(
        rows(&model) == base_rows - 1,
        "pattern-delete",
        "delete_row did not remove the top layer",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "layers edited");
}

/// The color picker popover: it opens on a tile of the selected layer, takes the L, C and H
/// fields, plane and hue-strip drags, a hex value and a variant chip, cancels without a change,
/// and applies as one undo step.
fn pattern_picker(ctx: &mut Ctx<'_>, window: &pleochroma_cut::ColorPatternWindow) {
    use pleochroma_cut::ColorPatternModel;
    let model = window.global::<ColorPatternModel>();
    model.invoke_select_row(0);
    pump(200);
    let Some(slot) = model.get_color_tiles().row_data(0).map(|tile| tile.slot) else {
        ctx.expect(
            false,
            "pattern-tiles",
            "the selected layer offered no color tile",
        );
        return;
    };
    model.invoke_open_picker(slot);
    pump(500);
    ctx.expect(
        model.get_picker_open() && !model.get_picker_title().is_empty(),
        "pattern-picker-opens",
        "open_picker did not open the picker with a title",
    );
    ctx.sweep_secondary(SecondaryWindow::ColorPattern, "color picker");
    model.invoke_picker_lch_changed(60.0, 40.0, 200.0);
    pump(300);
    ctx.expect(
        (model.get_picker_l() - 60.0).abs() < 1.0,
        "pattern-picker-lightness",
        format!(
            "picker_lch_changed left the lightness at {}",
            model.get_picker_l()
        ),
    );
    // A drag on the plane and on the hue strip: down, move, up.
    for phase in 0..3 {
        model.invoke_picker_plane_pointer(0.3, 0.4, phase);
    }
    for phase in 0..3 {
        model.invoke_picker_hue_pointer(0.5, phase);
    }
    pump(300);
    ctx.expect(
        model.get_picker_open(),
        "pattern-picker-drags",
        "a drag on the picker's plane or hue strip closed it",
    );
    model.invoke_picker_hex_committed("#3366aa".into());
    pump(300);
    ctx.expect(
        !model.get_picker_hex().is_empty(),
        "pattern-picker-hex",
        "a committed hex value left the picker's hex field empty",
    );
    if slint::Model::row_count(&model.get_picker_variants()) > 0 {
        model.invoke_picker_variant_picked(0);
        pump(300);
    }
    model.invoke_picker_cancelled();
    pump(200);
    ctx.expect(
        !model.get_picker_open(),
        "pattern-picker-cancels",
        "picker_cancelled left the picker open",
    );
    // Open again, change the color and apply once the solve is through.
    model.invoke_open_picker(slot);
    pump(500);
    model.invoke_picker_lch_changed(45.0, 55.0, 20.0);
    pump(800);
    model.invoke_picker_applied();
    pump(400);
    ctx.expect(
        !model.get_picker_open() && model.get_can_undo(),
        "pattern-picker-applies",
        "picker_applied did not close the picker with an undo step",
    );
}

/// Help and the Rough Planner are open; closing the main window (the title-bar request, with the
/// close guard answered Discard if it appears) leaves no secondary window open.
fn closing_main_closes_all(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<HelpModel>().invoke_open_context_help();
    ui.global::<RoughPlanModel>().invoke_open();
    let both = wait_until(Duration::from_secs(10), || {
        harness_support::is_window_open(SecondaryWindow::Help)
            && harness_support::is_window_open(SecondaryWindow::RoughPlanner)
    });
    ctx.expect(
        both,
        "windows-open",
        "Help and the Rough Planner did not both open",
    );
    dialogs::request_main_close(&ui);
    if ui.get_close_confirm_open() {
        ui.invoke_close_confirm_discard();
    }
    pump(400);
    ctx.expect(
        !ui.window().is_visible(),
        "main-closes",
        "the main window is still visible after the close request",
    );
    let open: Vec<String> = ALL_WINDOWS
        .iter()
        .filter(|w| harness_support::is_window_open(**w))
        .map(|w| format!("{w:?}"))
        .collect();
    ctx.expect(
        open.is_empty(),
        "secondary-windows-closed",
        format!("secondary windows still open after the main window closed: {open:?}"),
    );
    ctx.check_health("closing the main window");
}
