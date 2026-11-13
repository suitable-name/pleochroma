//! Scenarios: the Rough Planner window, against the sample library (four designs).
//!
//! The planner is driven through its own `RoughPlanModel` (the window's instance, reached with
//! `harness_support::rough_planner_window()`): typed fields, `plan`, `begin_save`, `open_saved`,
//! `export_saved`, `import_saved` -- the callbacks its buttons call -- and the results are read
//! back from the same model. File pickers get their answers queued first.
//!
//! The export suffix comes from the app (`gui::rough_plan::saved::naming`) and is re-exported
//! by `harness_support`.
//! Saving under a name a saved plan already has asks "Replace Saved Plan?"
//! (`confirm_overwrite_open`); `planner_save_and_reopen` answers Cancel, then Replace. The other
//! replace question is "Plan again over results that were not saved" (`confirm_replace_open`).

use super::{Piece, Scenario, dialogs, existing_install};
use crate::support::{
    Ctx, HarnessApp, Profile, SecondaryWindow, WindowView,
    app::{pump, wait_until},
    coverage, sample_designs, seed_library,
};
use pleochroma_cut::{
    EditorModel, LibraryModel, MainWindow, RoughPlanModel, RoughPlannerWindow, harness_support,
};
use slint::{ComponentHandle, Model};
use std::{fs, time::Duration};
use uiprobe::backend::wait_until_labeled;

use harness_support::ROUGH_PLAN_EXPORT_SUFFIX as EXPORT_SUFFIX;
/// The header line that marks a plan file; it is looked for in the first eight lines.
const PLAN_FORMAT_LINE: &str = "format = \"pleochroma-rough-plan\"";

/// Number of pieces the planner click-everything pass is split into, so each stays short. Each
/// piece presses only the controls whose key hashes into its part (`support::part`).
const CLICK_PARTS: usize = 3;

/// The pieces of this file.
pub fn pieces() -> Vec<Piece> {
    let mut all: Vec<Piece> = vec![
        Scenario {
            name: "planner_empty_state",
            seed: existing_install,
            run: planner_empty_state,
        }
        .into(),
        Scenario {
            name: "planner_block_and_plan",
            seed: existing_install,
            run: planner_block_and_plan,
        }
        .into(),
        Scenario {
            name: "planner_stone_cap",
            seed: existing_install,
            run: planner_stone_cap,
        }
        .into(),
        Scenario {
            name: "planner_save_and_reopen",
            seed: existing_install,
            run: planner_save_and_reopen,
        }
        .into(),
        Scenario {
            name: "planner_export_import_psrd",
            seed: existing_install,
            run: planner_export_import_psrd,
        }
        .into(),
        Scenario {
            name: "planner_open_in_editor",
            seed: existing_install,
            run: planner_open_in_editor,
        }
        .into(),
        Scenario {
            name: "planner_steps_summary",
            seed: existing_install,
            run: planner_steps_summary,
        }
        .into(),
        Scenario {
            name: "planner_tabs_menus_legend",
            seed: existing_install,
            run: planner_tabs_menus_legend,
        }
        .into(),
        Scenario {
            name: "planner_scan_report",
            seed: existing_install,
            run: planner_scan_report,
        }
        .into(),
    ];
    for index in 0..CLICK_PARTS {
        all.push(
            Piece::new(
                format!("planner_click_everything:{}of{CLICK_PARTS}", index + 1),
                existing_install,
                planner_click_everything,
            )
            .part(index, CLICK_PARTS),
        );
    }
    all
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Opens the planner from the main window's menu route and waits until it is showing with its
/// material list; `None` when it did not open in time.
fn open_planner_raw(ui: &MainWindow) -> Option<RoughPlannerWindow> {
    ui.global::<RoughPlanModel>().invoke_open();
    let ready = wait_until(Duration::from_secs(10), || {
        harness_support::is_window_open(SecondaryWindow::RoughPlanner)
            && harness_support::rough_planner_window()
                .is_some_and(|w| w.global::<RoughPlanModel>().get_materials().row_count() > 0)
    });
    if ready {
        harness_support::rough_planner_window()
    } else {
        None
    }
}

/// [`open_planner_raw`] with a finding when it fails.
fn open_planner(ctx: &mut Ctx<'_>) -> Option<RoughPlannerWindow> {
    let ui = ctx.app.ui().clone_strong();
    let window = open_planner_raw(&ui);
    ctx.expect(
        window.is_some(),
        "planner-opens",
        "RoughPlanModel.open did not show the Rough Planner with its materials within 10 s",
    );
    window
}

/// The block most scenarios plan: 40 x 30 x 25 mm (x, y, z as typed into the size fields).
const BLOCK_MM: [&str; 3] = ["40", "30", "25"];

/// The block of [`planner_stone_cap`]: 60 x 8 x 8 mm, a long thin rough that holds several
/// stones in a row, so a cap of two stones binds.
const STONE_CAP_BLOCK_MM: [&str; 3] = ["60", "8", "8"];

/// Types a block of `size_mm` (x, y, z, in mm), asks all of the library, sets "Designs (Up To)"
/// to `max_designs` and "Stones (Up To)" to `max_stones` (0: no limit), plans, and waits (real
/// time, up to 60 s) for the layouts, each of at most that many different designs and stones.
/// `Err` carries what the window said.
fn plan_block_raw(
    window: &RoughPlannerWindow,
    size_mm: [&str; 3],
    max_designs: i32,
    max_stones: i32,
) -> Result<(), String> {
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x(size_mm[0].into());
    model.set_rough_y(size_mm[1].into());
    model.set_rough_z(size_mm[2].into());
    model.set_max_designs(max_designs);
    model.set_max_stones(max_stones);
    model.set_use_filter(false);
    model.invoke_base_field_committed();
    pump(250);
    model.invoke_plan();
    wait_until(Duration::from_secs(60), || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    if model.get_running() {
        return Err(format!("still running after 60 s: {}", model.get_stage()));
    }
    let error = model.get_error_text();
    if !error.is_empty() {
        return Err(error.to_string());
    }
    if model.get_results().row_count() == 0 {
        return Err(format!("no layout: {}", model.get_summary()));
    }
    Ok(())
}

/// [`plan_block_raw`] on the standard block, with at most three designs per layout and no stone
/// limit, with a finding when it fails.
fn plan_block(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    plan_with_finding(ctx, window, BLOCK_MM, 0)
}

/// [`plan_block_raw`] on `size_mm` with three designs and `max_stones` stones at most, with a
/// finding when it fails.
fn plan_with_finding(
    ctx: &mut Ctx<'_>,
    window: &RoughPlannerWindow,
    size_mm: [&str; 3],
    max_stones: i32,
) -> bool {
    match plan_block_raw(window, size_mm, 3, max_stones) {
        Ok(()) => true,
        Err(message) => {
            ctx.expect(
                false,
                "plan-runs",
                format!("planning a block failed: {message}"),
            );
            false
        }
    }
}

/// Stones in the best layout.
fn first_stone_count(window: &RoughPlannerWindow) -> i32 {
    window
        .global::<RoughPlanModel>()
        .get_results()
        .row_data(0)
        .map_or(0, |layout| layout.stone_count)
}

/// Saves every result under the suggested name and waits until the saved list grows.
fn save_all(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    let model = window.global::<RoughPlanModel>();
    let before = model.get_saved_plans().row_count();
    model.invoke_begin_save(false);
    pump(150);
    let named = model.get_save_name_open() && !model.get_save_name().is_empty();
    ctx.expect(
        named,
        "save-name-row",
        "Save All did not open the name row with a suggested name",
    );
    if !named {
        return false;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "save name row");
    model.invoke_confirm_save();
    // The suggested name may already be taken by a plan an earlier scenario saved in this profile
    // ("Replace Saved Plan?" then opens, after a name lookup on a worker): answer Replace, which
    // leaves the list the same length.
    let _ = wait_until(Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > before || model.get_confirm_overwrite_open()
    });
    let replaced = model.get_confirm_overwrite_open();
    if replaced {
        model.invoke_confirm_overwrite(true);
        coverage::record_callback("RoughPlanModel.confirm_overwrite");
    }
    let listed = ctx.wait_for("the saved plan is listed", Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > before
            || (replaced
                && !model.get_confirm_overwrite_open()
                && !model.get_save_name_open()
                && model.get_saved_plans().row_count() > 0)
    });
    if listed {
        coverage::record_callback("RoughPlanModel.confirm_save");
    } else {
        ctx.report.note(format!(
            "planner save: not listed; error line {:?}, name row open {}, replace question open {}",
            model.get_error_text().to_string(),
            model.get_save_name_open(),
            model.get_confirm_overwrite_open()
        ));
    }
    listed
}

/// The id of the first saved plan.
fn first_saved_id(window: &RoughPlannerWindow) -> Option<i32> {
    window
        .global::<RoughPlanModel>()
        .get_saved_plans()
        .row_data(0)
        .map(|row| row.id)
}

/// Closes the app, writes the sample library into the profile, starts again and waits for the
/// designs: what `Ctx::relaunch_with_library` does, for a `prepare` step that has no `Ctx`.
fn reload_with_library(app: &mut HarnessApp, profile: &Profile) -> bool {
    app.shutdown();
    if seed_library(profile, &sample_designs()).is_err() {
        return false;
    }
    match HarnessApp::launch() {
        Ok(fresh) => *app = fresh,
        Err(_) => return false,
    }
    let ui = app.ui().clone_strong();
    let wanted = sample_designs().len();
    wait_until_labeled("library-load", Duration::from_secs(10), || {
        ui.global::<LibraryModel>().get_diagram_list().row_count() >= wanted
    })
}

// ---------------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------------

/// The planner opens empty: no layouts, no typed size, not running, no error; sweeps clean at its
/// default and minimum size.
fn planner_empty_state(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    ctx.expect(
        model.get_results().row_count() == 0
            && model.get_rough_x().is_empty()
            && !model.get_running()
            && model.get_error_text().is_empty(),
        "planner-empty-state",
        "a fresh planner shows layouts, a size, a running plan or an error",
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "empty state");
    ctx.check_health("planner empty state");
}

/// A block rough: shape cuts can be added and removed, then Plan over the whole sample library
/// finishes with at least one layout holding at least one placed stone and a summary line; the
/// result state sweeps clean.
fn planner_block_and_plan(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.invoke_base_field_committed();
    pump(250);
    model.invoke_add_cut(0);
    pump(250);
    ctx.expect(
        model.get_cuts().row_count() == 1,
        "planner-add-cut",
        format!(
            "Add edge cut left {} cut row(s), expected 1",
            model.get_cuts().row_count()
        ),
    );
    model.invoke_remove_cut(0);
    pump(250);
    ctx.expect(
        model.get_cuts().row_count() == 0,
        "planner-remove-cut",
        "Remove did not take the cut away",
    );
    if !plan_block(ctx, &window) {
        return;
    }
    coverage::record_callback("RoughPlanModel.plan");
    ctx.expect(
        first_stone_count(&window) >= 1,
        "planner-stones",
        "the best layout holds no placed stone",
    );
    ctx.expect(
        !model.get_summary().is_empty(),
        "planner-summary",
        "the finished plan shows no summary line",
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "plan result");
    ctx.check_health("planner block and plan");
}

/// The most stones the layouts of [`planner_stone_cap`] may hold.
const STONE_CAP: i32 = 2;

/// A 60 x 8 x 8 mm block planned with Designs (Up To) 3 and Stones (Up To) 2: every layout in
/// the list holds one or two stones, the best holds two (the cap binds, since the block has room
/// for more), and a saved plan brings both limits back when it is opened again.
fn planner_stone_cap(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_with_finding(ctx, &window, STONE_CAP_BLOCK_MM, STONE_CAP) {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let results = model.get_results();
    let counts: Vec<i32> = (0..results.row_count())
        .filter_map(|at| results.row_data(at))
        .map(|layout| layout.stone_count)
        .collect();
    ctx.expect(
        counts.iter().all(|stones| (1..=STONE_CAP).contains(stones)),
        "stone-cap-respected",
        format!("a layout holds no stone or more than {STONE_CAP}: stones per layout {counts:?}"),
    );
    ctx.expect(
        counts.first() == Some(&STONE_CAP),
        "stone-cap-binds",
        format!("the best layout should hold {STONE_CAP} stones: stones per layout {counts:?}"),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "stone cap result");

    if !save_all(ctx, &window) {
        return;
    }
    let Some(id) = first_saved_id(&window) else {
        return;
    };
    // Change both limits first, so that opening the plan has to bring them back.
    model.set_max_designs(1);
    model.set_max_stones(0);
    model.invoke_open_saved(id);
    let opened = ctx.wait_for("the saved plan opens", Duration::from_secs(15), || {
        !model.get_loaded_banner().is_empty()
    });
    if opened {
        coverage::record_callback("RoughPlanModel.open_saved");
    }
    ctx.expect(
        model.get_max_stones() == STONE_CAP,
        "stone-cap-reopened",
        format!(
            "the reopened plan shows Stones (Up To) {}, saved {STONE_CAP}",
            model.get_max_stones()
        ),
    );
    ctx.expect(
        model.get_max_designs() == 3,
        "design-cap-reopened",
        format!(
            "the reopened plan shows Designs (Up To) {}, saved 3",
            model.get_max_designs()
        ),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "stone cap plan reopened");
    ctx.check_health("planner stone cap");
}

/// Plan again over results that were not saved asks to replace them (Keep leaves them
/// untouched); Save All lists the plan; a second save under the same name asks to replace it
/// (Cancel keeps the row, Replace leaves one plan); opening the saved plan brings back the same
/// layouts and stone count.
fn planner_save_and_reopen(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let rows = model.get_results().row_count();
    let stones = first_stone_count(&window);
    replace_prompt(ctx, &window, rows);
    if !save_all(ctx, &window) {
        return;
    }
    if !overwrite_question(ctx, &window) {
        return;
    }
    let Some(id) = first_saved_id(&window) else {
        return;
    };
    model.invoke_open_saved(id);
    let opened = ctx.wait_for("the saved plan opens", Duration::from_secs(15), || {
        !model.get_loaded_banner().is_empty()
    });
    if opened {
        coverage::record_callback("RoughPlanModel.open_saved");
    }
    ctx.expect(
        model.get_results().row_count() == rows && first_stone_count(&window) == stones,
        "reopen-same-plan",
        format!(
            "reopened plan shows {} layout(s) / {} stone(s), saved {rows} / {stones}",
            model.get_results().row_count(),
            first_stone_count(&window)
        ),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "saved plan reopened");
    ctx.check_health("planner save and reopen");
}

/// Saving under a name a saved plan already has asks "Replace Saved Plan?". Cancel keeps the
/// name row open and the saved list as it was; saving again and answering Replace leaves exactly
/// ONE saved plan with that name. Returns whether the flow could be driven to its end.
fn overwrite_question(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    let model = window.global::<RoughPlanModel>();
    let saved = model.get_saved_plans();
    let Some(taken) = saved.row_data(0).map(|row| row.name.to_string()) else {
        ctx.expect(false, "overwrite-setup", "no saved plan to save over");
        return false;
    };
    let before = saved.row_count();
    model.invoke_begin_save(false);
    pump(150);
    model.set_save_name(taken.as_str().into());
    model.invoke_confirm_save();
    // The name lookup runs on a worker thread (real time); the question follows its answer.
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_confirm_overwrite_open()
    });
    let asked = model.get_confirm_overwrite_open();
    ctx.expect(
        asked && model.get_confirm_overwrite_name().as_str() == taken,
        "overwrite-question",
        format!("saving under the taken name {taken:?} did not ask to replace it"),
    );
    if !asked {
        return false;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "replace saved plan question");
    // Cancel: the question closes, the name row stays open, nothing was saved.
    model.invoke_confirm_overwrite(false);
    pump(300);
    ctx.expect(
        !model.get_confirm_overwrite_open()
            && model.get_save_name_open()
            && model.get_saved_plans().row_count() == before,
        "overwrite-cancel",
        "Cancel on the replace question did not keep the name row open with the list unchanged",
    );
    // Save again under the same name and Replace it.
    model.invoke_confirm_save();
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_confirm_overwrite_open()
    });
    if !ctx.expect(
        model.get_confirm_overwrite_open(),
        "overwrite-question-again",
        "the second save under the taken name did not ask again",
    ) {
        return false;
    }
    model.invoke_confirm_overwrite(true);
    let closed = ctx.wait_for("the replace is done", Duration::from_secs(15), || {
        !model.get_confirm_overwrite_open() && !model.get_save_name_open()
    });
    if closed {
        coverage::record_callback("RoughPlanModel.confirm_overwrite");
    }
    pump(500);
    let same_name = (0..model.get_saved_plans().row_count())
        .filter_map(|at| model.get_saved_plans().row_data(at))
        .filter(|row| row.name.as_str() == taken)
        .count();
    ctx.expect(
        same_name == 1 && model.get_saved_plans().row_count() == before,
        "overwrite-one-plan",
        format!(
            "after Replace {same_name} saved plan(s) are named {taken:?} and the list has {} (was {before})",
            model.get_saved_plans().row_count()
        ),
    );
    ctx.expect(
        model.get_error_text().is_empty(),
        "save-same-name",
        format!(
            "replacing the saved plan raised: {}",
            model.get_error_text()
        ),
    );
    true
}

/// Plan over unsaved results asks "replace?"; Keep answers no and leaves the results as they
/// were.
fn replace_prompt(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow, rows: usize) {
    let model = window.global::<RoughPlanModel>();
    model.invoke_plan();
    pump(250);
    let asked = model.get_confirm_replace_open() && model.get_confirm_replace_count() > 0;
    ctx.expect(
        asked,
        "replace-prompt",
        "Plan over results that were not saved did not ask whether to replace them",
    );
    if !asked {
        return;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "replace question");
    model.invoke_confirm_replace(false);
    pump(250);
    ctx.expect(
        !model.get_confirm_replace_open()
            && !model.get_running()
            && model.get_results().row_count() == rows,
        "replace-keep",
        "Keep on the replace question did not leave the results alone",
    );
}

/// Export writes a `.psrd` file with the plan header; importing it adds a plan that opens with
/// the same stone count.
fn planner_export_import_psrd(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) || !save_all(ctx, &window) {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let stones = first_stone_count(&window);
    let Some(id) = first_saved_id(&window) else {
        return;
    };
    let path = ctx.scratch_file(&format!("p{EXPORT_SUFFIX}"));
    harness_support::queue_picker_path(&path);
    model.invoke_export_saved(id);
    let written = ctx.wait_for("the plan file is written", Duration::from_secs(15), || {
        path.exists()
    });
    if !written {
        return;
    }
    let text = fs::read_to_string(&path).unwrap_or_default();
    ctx.expect(
        text.lines()
            .take(8)
            .any(|line| line.trim() == PLAN_FORMAT_LINE),
        "export-header",
        format!("the exported file has no {PLAN_FORMAT_LINE} header line"),
    );
    coverage::record_callback("RoughPlanModel.export_saved");
    // Counted from what is stored now: another scenario's plan may already be in the profile.
    let stored = model.get_saved_plans().row_count();
    import_and_check(
        ctx,
        &window,
        &path,
        stored + 1,
        stones,
        "import of the .psrd file",
    );
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-queue-drained",
        "a queued picker answer was never consumed",
    );
    ctx.check_health("planner export and import");
}

/// Queues `path`, imports it, expects `plans` stored plans and the same stone count on screen.
fn import_and_check(
    ctx: &mut Ctx<'_>,
    window: &RoughPlannerWindow,
    path: &std::path::Path,
    plans: usize,
    stones: i32,
    what: &str,
) {
    let model = window.global::<RoughPlanModel>();
    harness_support::queue_picker_path(path);
    model.invoke_import_saved();
    let listed = ctx.wait_for(what, Duration::from_secs(15), || {
        model.get_saved_plans().row_count() >= plans
    });
    if !listed {
        return;
    }
    coverage::record_callback("RoughPlanModel.import_saved");
    pump(1500);
    ctx.expect(
        first_stone_count(window) == stones,
        "import-same-plan",
        format!(
            "{what}: the plan on screen has {} stone(s) in its best layout, expected {stones}; error: {}",
            first_stone_count(window),
            model.get_error_text()
        ),
    );
}

/// "Open in Editor" on a result hands a piece of rough to the main window's editor: it ends up
/// with a design (tier count above zero).
fn planner_open_in_editor(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    ctx.expect(
        dialogs::tier_count(&ui) == 0,
        "editor-starts-empty",
        "the editor already holds a design before Open in Editor",
    );
    window
        .global::<RoughPlanModel>()
        .invoke_open_in_editor(0, 0);
    let opened = ctx.wait_for(
        "the editor receives the design",
        Duration::from_secs(20),
        || dialogs::tier_count(&ui) > 0,
    );
    if opened {
        coverage::record_callback("RoughPlanModel.open_in_editor");
        ctx.expect(
            ui.global::<EditorModel>().get_has_design(),
            "editor-has-design",
            "tiers arrived but the editor reports no design",
        );
    }
    ctx.check_health("planner open in editor");
}

// ---------------------------------------------------------------------------------------------
// The redesigned window: steps, tabs, menus, legend and the scan report
// ---------------------------------------------------------------------------------------------

/// The folded line of step `step` (0 Rough, 1 Crystal Axis, 2 Shape Cuts, 3 Plan Settings).
fn step_summary(model: &RoughPlanModel<'_>, step: usize) -> String {
    model
        .get_step_summary()
        .row_data(step)
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// Whether step `step` is complete (its number shows a check).
fn step_done(model: &RoughPlanModel<'_>, step: usize) -> bool {
    model.get_step_done().row_data(step).unwrap_or(false)
}

/// Presses the header of the step called `title` the way a click does (its default accessible
/// action), whatever number the window gives it (the numbers shift when the material has no
/// crystal axis); `false` when no such header is on screen.
fn press_step_header(window: &RoughPlannerWindow, title: &str) -> bool {
    let view = WindowView::of("RoughPlannerWindow", window);
    let header = (1..=4).find_map(|number| {
        view.by_label(&format!("Step {number}: {title}"))
            .into_iter()
            .next()
    });
    header.is_some_and(|element| {
        element.invoke_accessible_default_action();
        pump(250);
        true
    })
}

/// Whether a control with this accessible label is in the planner's tree (a closed step builds no
/// body, so the controls of a closed step are not).
fn on_screen(window: &RoughPlannerWindow, label: &str) -> bool {
    !WindowView::of("RoughPlannerWindow", window)
        .by_label(label)
        .is_empty()
}

/// The steps of the left column: their summary lines and check marks follow the model (a typed
/// block, a cut, the plan settings, a size that is no number), the header line follows the rough,
/// a click on a header opens that step and closes the others, and a closed step builds no
/// controls.
fn planner_steps_summary(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    ctx.expect(
        model.get_open_step() == 0
            && model.get_step_done().row_count() == 4
            && model.get_step_summary().row_count() == 4
            && model.get_step_error().row_count() == 4,
        "steps-start",
        format!(
            "a fresh planner should open step 0 with four entries per step list, not step {} / {} done / {} summaries / {} errors",
            model.get_open_step(),
            model.get_step_done().row_count(),
            model.get_step_summary().row_count(),
            model.get_step_error().row_count()
        ),
    );
    ctx.wait_for(
        "the Rough step asks for a shape and a size",
        Duration::from_secs(5),
        || !step_done(&model, 0) && step_summary(&model, 0).starts_with("Pick a shape"),
    );

    // A typed block: the Rough step is done and sums the block up, the header line names it.
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.invoke_base_field_committed();
    let sized = ctx.wait_for(
        "the Rough step sums up the block",
        Duration::from_secs(10),
        || step_done(&model, 0) && step_summary(&model, 0).starts_with("Block "),
    );
    if sized {
        ctx.expect(
            model.get_rough_subtitle().contains("block"),
            "steps-subtitle",
            format!(
                "the header line {:?} does not name the block",
                model.get_rough_subtitle()
            ),
        );
    }

    // A size that is no number takes the check back; the block comes back with the number.
    model.set_rough_x("abc".into());
    model.invoke_base_field_committed();
    ctx.wait_for(
        "the Rough step loses its check for a size that is no number",
        Duration::from_secs(10),
        || !step_done(&model, 0),
    );
    model.set_rough_x("40".into());
    model.invoke_base_field_committed();
    ctx.wait_for(
        "the Rough step is done again",
        Duration::from_secs(10),
        || step_done(&model, 0),
    );

    // Shape cuts: the count follows the model.
    model.invoke_add_cut(0);
    ctx.wait_for(
        "the Shape Cuts step counts the cut",
        Duration::from_secs(5),
        || step_summary(&model, 2) == "1 cut",
    );
    model.invoke_remove_cut(0);
    ctx.wait_for(
        "the Shape Cuts step has no cut again",
        Duration::from_secs(5),
        || step_summary(&model, 2) == "No cuts yet",
    );

    // Plan settings: the caps and the candidate set, and the step is never open to complaint.
    model.set_max_designs(2);
    model.set_max_stones(0);
    model.set_use_filter(false);
    ctx.wait_for(
        "the Plan Settings step names the caps and the whole library",
        Duration::from_secs(5),
        || {
            let line = step_summary(&model, 3);
            line.starts_with("Up to 2 designs") && line.contains("whole library")
        },
    );
    ctx.expect(
        step_done(&model, 3),
        "steps-settings-done",
        "the Plan Settings step is not done",
    );

    step_headers(ctx, &window, &model);
    ctx.check_health("planner steps");
}

/// Touches the text fields called `labels` (each set to the value it holds) so the coverage pass
/// counts them: the fields of a step exist only while its step is open.
fn touch_fields(window: &RoughPlannerWindow, labels: &[&str]) {
    let view = WindowView::of("RoughPlannerWindow", window);
    for label in labels {
        for field in view.by_label(label) {
            let current = field.accessible_value().unwrap_or_default();
            field.set_accessible_value(current);
            coverage::record_touch(&field);
            coverage::record_wrappers(&view.root, &field, true);
        }
    }
    pump(100);
}

/// The headers: Plan Settings opens (its controls exist now, the Rough step's do not), the same
/// header closes it, and the Rough header opens the first step again.
fn step_headers(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow, model: &RoughPlanModel<'_>) {
    ctx.expect(
        on_screen(window, "Rough X (mm)") && !on_screen(window, "Most Designs per Layout"),
        "steps-body-of-open-step",
        "step 0 should build its fields and the closed Plan Settings step none",
    );
    touch_fields(
        window,
        &["Rough X (mm)", "Rough Y (mm)", "Rough Z (mm)", "Carat (ct)"],
    );
    ctx.expect(
        press_step_header(window, "Plan Settings") && model.get_open_step() == 3,
        "steps-open-by-header",
        format!(
            "a press on the Plan Settings header left step {} open",
            model.get_open_step()
        ),
    );
    ctx.expect(
        on_screen(window, "Most Designs per Layout") && !on_screen(window, "Rough X (mm)"),
        "steps-body-follows-open-step",
        "the opened Plan Settings step should build its controls and the closed Rough step none",
    );
    // The four losses fields sit behind the Losses "Edit" link.
    model.set_losses_open(true);
    pump(200);
    touch_fields(
        window,
        &[
            "Saw Kerf (mm)",
            "Allowance / Side (mm)",
            "Rough Skin (mm)",
            "Min Stone Width (mm)",
        ],
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "plan settings step open");
    ctx.expect(
        press_step_header(window, "Plan Settings") && model.get_open_step() == -1,
        "steps-close-by-header",
        format!(
            "a second press on the open step's header left step {} open",
            model.get_open_step()
        ),
    );
    ctx.expect(
        press_step_header(window, "Rough") && model.get_open_step() == 0,
        "steps-reopen-rough",
        format!(
            "a press on the Rough header left step {} open",
            model.get_open_step()
        ),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "rough step open");
}

/// A planned and saved block: the legend picks a design and picking it again clears it, the
/// saved plan's menu exports (2), leaves rename to the window (1), opens (0, which also brings the
/// Layouts tab back) and deletes (3), and Plan Again with More Time doubles the time limit.
fn planner_tabs_menus_legend(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) {
        return;
    }
    let model = window.global::<RoughPlanModel>();

    // The legend: a pick lights a design, the same pick clears it.
    model.invoke_select_result(0);
    pump(400);
    let before = model.get_legend_selected();
    model.invoke_legend_picked(0);
    let picked = ctx.wait_for("the legend picks a design", Duration::from_secs(5), || {
        model.get_legend_selected() != before
    });
    if picked {
        coverage::record_callback("RoughPlanModel.legend_picked");
        ctx.expect(
            model.get_legend_selected() == model.get_selected_group(),
            "legend-matches-design-row",
            format!(
                "the legend shows design {} but the design row {}",
                model.get_legend_selected(),
                model.get_selected_group()
            ),
        );
        model.invoke_legend_picked(0);
        ctx.wait_for(
            "the second legend pick puts it back",
            Duration::from_secs(5),
            || model.get_legend_selected() == before,
        );
    }

    // The tabs and the saved plan's menu.
    if !save_all(ctx, &window) {
        return;
    }
    model.set_results_tab(1);
    pump(250);
    let plans = model.get_saved_plans().row_count();
    model.invoke_saved_menu(0, 1);
    pump(250);
    ctx.expect(
        model.get_saved_plans().row_count() == plans && model.get_error_text().is_empty(),
        "saved-menu-rename",
        "the Rename entry is the window's own inline rename: Rust should do nothing for action 1",
    );
    let path = ctx.scratch_file(&format!("menu{EXPORT_SUFFIX}"));
    harness_support::queue_picker_path(&path);
    model.invoke_saved_menu(0, 2);
    let written = ctx.wait_for(
        "the menu's Export writes the plan file",
        Duration::from_secs(15),
        || path.exists(),
    );
    if written {
        coverage::record_callback("RoughPlanModel.saved_menu");
        let text = fs::read_to_string(&path).unwrap_or_default();
        ctx.expect(
            text.lines()
                .take(8)
                .any(|line| line.trim() == PLAN_FORMAT_LINE),
            "saved-menu-export-header",
            format!("the file written by the menu has no {PLAN_FORMAT_LINE} header line"),
        );
    }
    model.invoke_saved_menu(0, 0);
    let opened = ctx.wait_for(
        "the menu's Open opens the plan",
        Duration::from_secs(15),
        || !model.get_loaded_banner().is_empty(),
    );
    if opened {
        ctx.expect(
            model.get_results_tab() == 0,
            "saved-menu-open-shows-layouts",
            format!(
                "an opened plan should bring the Layouts tab back, the window shows tab {}",
                model.get_results_tab()
            ),
        );
    }
    model.set_results_tab(1);
    pump(250);
    let before_delete = model.get_saved_plans().row_count();
    model.invoke_saved_menu(0, 3);
    ctx.wait_for(
        "the menu's Delete drops the plan",
        Duration::from_secs(15),
        || model.get_saved_plans().row_count() < before_delete,
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "saved plans tab");

    cancel_a_running_plan(ctx, &window);
    plan_again_with_more_time(ctx, &window);
    ctx.check_health("planner tabs, menus and legend");
}

/// Presses the button called `label` the way a click does and records the press for the coverage
/// pass; `false` when no such button is on screen.
fn press_button(window: &RoughPlannerWindow, label: &str) -> bool {
    let view = WindowView::of("RoughPlannerWindow", window);
    let Some(button) = view.by_label(label).into_iter().next() else {
        return false;
    };
    coverage::record_activation(&button);
    coverage::record_wrappers(&view.root, &button, false);
    button.invoke_accessible_default_action();
    pump(250);
    true
}

/// Starts a plan and presses "Cancel the Plan" while it runs: the run ends without an error and
/// the layouts of the plan before stay. Does nothing when the plan finishes before the button is
/// there (a plan of a small block is quick).
fn cancel_a_running_plan(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) {
    let model = window.global::<RoughPlanModel>();
    model.invoke_plan();
    pump(50);
    if model.get_confirm_replace_open() {
        model.invoke_confirm_replace(true);
    }
    let running = wait_until(Duration::from_secs(3), || model.get_running());
    if running && press_button(window, "Cancel the Plan") {
        ctx.wait_for(
            "Cancel the Plan ends the run",
            Duration::from_secs(30),
            || !model.get_running(),
        );
    }
    let _ = wait_until(Duration::from_secs(60), || !model.get_running());
}

/// Plan Again with More Time: the limit doubles and the plan runs again. A block rough hides the
/// limit field, so the button doubles the stored limit; the scenario reads that limit first and
/// expects twice it.
fn plan_again_with_more_time(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) {
    let model = window.global::<RoughPlanModel>();
    model.set_results_tab(0);
    model.set_stopped_at_limit(true);
    let current: u32 = model.get_time_limit_secs().trim().parse().unwrap_or(120);
    let expected = current.saturating_mul(2).min(86_400).to_string();
    pump(250);
    // The button element itself; the callback when the results column has no such button now.
    if !press_button(window, "Plan Again with More Time") {
        model.invoke_plan_more_time();
    }
    let doubled = ctx.wait_for(
        "Plan Again with More Time doubles the scan time limit",
        Duration::from_secs(15),
        || model.get_time_limit_secs().as_str() == expected,
    );
    if doubled {
        coverage::record_callback("RoughPlanModel.plan_more_time");
        let _ = wait_until(Duration::from_secs(3), || model.get_confirm_replace_open());
        if model.get_confirm_replace_open() {
            model.invoke_confirm_replace(true);
        }
        let _ = wait_until(Duration::from_secs(60), || {
            !model.get_running() && !model.get_stopped_at_limit()
        });
        ctx.expect(
            !model.get_running() && !model.get_stopped_at_limit(),
            "plan-more-time-replans",
            "the plan started by Plan Again with More Time did not finish or kept the limit mark",
        );
    }
}

/// The OBJ of [`planner_scan_report`]: a cube of 10 units with one of its twelve triangles left
/// out, a hole the import repairs and reports.
const CUBE_WITH_HOLE_OBJ: &str = "\
v 0 0 0
v 10 0 0
v 10 10 0
v 0 10 0
v 0 0 10
v 10 0 10
v 10 10 10
v 0 10 10
f 1 3 2
f 1 4 3
f 5 6 7
f 5 7 8
f 1 2 6
f 1 6 5
f 4 8 7
f 4 7 3
f 1 5 8
f 1 8 4
f 2 3 7
";

/// Presses the scan card's own "Choose File…" button before any scan is read; the picker is
/// cancelled.
///
/// The card shows for base kind 3 only. `set_base` cannot give that kind (only the import
/// does), so the kind is set on the model directly.
fn scan_card_choose_file(
    ctx: &mut Ctx<'_>,
    window: &RoughPlannerWindow,
    model: &RoughPlanModel<'_>,
) {
    model.set_base_kind(3);
    // The scan card sits in the Rough step: a closed step builds no controls.
    model.set_open_step(0);
    pump(300);
    let card_shown = on_screen(window, "Choose File\u{2026}");
    ctx.expect(
        card_shown,
        "scan-choose-file",
        "the scan card shows no Choose File\u{2026} button before a scan is read",
    );
    if card_shown {
        harness_support::queue_picker_cancel();
        press_button(window, "Choose File\u{2026}");
        let _ = wait_until(Duration::from_secs(2), || {
            harness_support::pending_picker_answers() == 0
        });
    }
}

/// A scan: importing a cube with a hole fills the scan card (file, size, verdict and an import
/// report), the Rough step sums it up, Show turns the view to a finding that has a place, the
/// layer switches redraw, and re-reading the file in another unit changes its size.
fn planner_scan_report(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    let path = ctx.scratch_file("cube_hole.obj");
    if fs::write(&path, CUBE_WITH_HOLE_OBJ).is_err() {
        ctx.expect(false, "scan-file", "the scratch OBJ could not be written");
        return;
    }
    scan_card_choose_file(ctx, &window, &model);
    harness_support::queue_picker_path(&path);
    model.invoke_import_obj();
    let read = ctx.wait_for("the scan is read", Duration::from_secs(30), || {
        !model.get_mesh_busy() && model.get_scan_verdict() >= 0
    });
    if !read {
        return;
    }
    ctx.expect(
        model.get_base_kind() == 3 && model.get_scan_file().as_str() == "cube_hole.obj",
        "scan-card",
        format!(
            "the scan card should show cube_hole.obj on base kind 3, not {:?} on {}",
            model.get_scan_file(),
            model.get_base_kind()
        ),
    );
    ctx.expect(
        model.get_scan_verdict() != 2 && !model.get_scan_size_text().is_empty(),
        "scan-verdict",
        format!(
            "the scan was refused or has no size: verdict {} ({:?})",
            model.get_scan_verdict(),
            model.get_scan_verdict_text()
        ),
    );
    ctx.expect(
        model.get_scan_findings().row_count() > 0,
        "scan-findings",
        "a cube with a hole should come with an import report that names the repair",
    );
    ctx.wait_for(
        "the Rough step sums up the scan",
        Duration::from_secs(5),
        || step_summary(&model, 0).starts_with("Scan cube_hole.obj"),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "scan report");

    // Show on a finding that has a place turns the view to it, with the problems layer on.
    let placed = (0..model.get_scan_findings().row_count())
        .find(|&at| {
            model
                .get_scan_findings()
                .row_data(at)
                .is_some_and(|finding| finding.has_place)
        })
        .and_then(|at| i32::try_from(at).ok());
    if let Some(index) = placed {
        model.set_show_problems(false);
        model.invoke_finding_show(index);
        pump(400);
        coverage::record_callback("RoughPlanModel.finding_show");
        ctx.expect(
            model.get_show_problems() && model.get_view_mode() == 0,
            "scan-finding-show",
            "Show on a finding should switch the problems layer on in the rough view",
        );
    }

    // The layer switches redraw the view without trouble.
    model.set_show_outline(true);
    model.invoke_view_toggles_changed();
    pump(300);
    model.set_show_problems(false);
    model.set_show_outline(false);
    model.invoke_view_toggles_changed();
    pump(300);
    coverage::record_callback("RoughPlanModel.view_toggles_changed");

    // The same file in centimetres is ten times as large.
    let before = model.get_scan_size_text().to_string();
    model.invoke_rescan_units(1);
    let reread = ctx.wait_for(
        "re-reading the file in centimetres changes the size",
        Duration::from_secs(30),
        || !model.get_mesh_busy() && model.get_scan_size_text().as_str() != before,
    );
    if reread {
        coverage::record_callback("RoughPlanModel.rescan_units");
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "scan re-read in centimetres");
    ctx.check_health("planner scan report");
}

/// Click-everything over the planner in its result state (block rough, a finished plan). The
/// engine denies delete, remove and replace controls.
fn planner_click_everything(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let mut prepare = |app: &mut HarnessApp| {
        if !reload_with_library(app, profile) {
            return;
        }
        let ui = app.ui().clone_strong();
        if let Some(window) = open_planner_raw(&ui) {
            let _ = plan_block_raw(&window, BLOCK_MM, 2, 0);
        }
    };
    // `reload_with_library` shuts down, seeds and launches by itself.
    let _stats = ctx.smoke_secondary_launching(
        SecondaryWindow::RoughPlanner,
        "block and plan result",
        &mut prepare,
    );
    ctx.check_health("planner click-everything");
}
