//! Editor and viewport states of the click-through passes that the 2026-10-28 run left uncovered
//! (lane H4): the Files & Downloads tab pill, the editor's File menu, the Path-Traced Live Render
//! view with its Optical Diagnostics button, metrics strip and panel, the tier table's orbit
//! column, the Tune tab's Jump to Best and Revert This Tier, the trial's Compare to Current, the
//! Export dialog's Ray-Traced Fire box, the Optimize comparison pane and the Material Editor's
//! Crystal Axis, Inclusion Families and Play-of-Color Size controls, and the Stone tab's Silk Turn.
//!
//! Proves: in each state every enabled button, tab, check box and combo box of the scope is pressed
//! once and every field touched (the engine of `overlays.rs`), after the state's `open` function
//! reached the page the way a user does: model flags first, then the opener or the typing.
//!
//! What kept the controls unreached:
//!
//! * the Live Render tab starts in Solid (`startup_settings` writes `live_view_mode` 0 whatever the
//!   settings remember), and the Optical Diagnostics button, the metrics strip and the panel
//!   exist only in Path-Traced (mode 1);
//! * the File menu items and the Files & Downloads pill were clicked by the scenarios' `click`
//!   helper, which does not record an activation, and the smoke engine denies the pill by its
//!   label ("download" matches the word in "Files & Downloads", a local tab, not a network call);
//! * the tier table's orbit button needs a column that fits: at the default 640 px dock the orbit
//!   column is the first one `data_limit` hides;
//! * the Tune tab's Jump to Best and Revert This Tier are enabled only for a tuned tier of a design
//!   that left its Baseline; the Material Editor's axis, family-direction and size controls only
//!   exist after their own switch is on; the Silk Turn slider after "Set Silk Turn".

use super::{
    super::{Piece, existing_install},
    overlay_states::{Base, NONE, State, edit_tab, inspector, live_tab, parameters, press},
    overlays, state_pieces,
    states_axis::design_material,
    states_dialogs::{material_open, saved_material},
    states_reach::scroll_scope,
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
    coverage,
    query::is_clickable_role,
};
use pleochroma_cut::{
    CompareModel, CrystalAxisEditorModel, EditorModel, EditorTierItem, ExportModel,
    InclusionFamiliesModel, OpticalModel, RenderEffectsModel, StructureParamsModel, TrialsModel,
    TuneModel, VariantsModel, ViewportModel, harness_support,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::time::Duration;

/// The pieces of this file: one `overlay:gaps-editor-*:<state>` piece per state, and the File menu,
/// the Files & Downloads pill and the trial comparison.
pub fn pieces() -> Vec<Piece> {
    let groups: [super::StateGroup; 4] = [
        ("gaps-editor-viewport", viewport_states),
        ("gaps-editor-material", material_states),
        ("gaps-editor-tables", table_states),
        ("gaps-editor-panes", pane_states),
    ];
    let mut all = Vec::new();
    for (group, states) in groups {
        all.extend(state_pieces(group, existing_install, states));
    }
    all.push(Piece::new(
        "coverage_files_tab_pill",
        existing_install,
        files_tab_pill,
    ));
    all.push(Piece::new(
        "coverage_file_menu_items",
        existing_install,
        file_menu_items,
    ));
    all.push(Piece::new(
        "coverage_trials_compare_current",
        existing_install,
        trials_compare_current,
    ));
    all
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Waits (real time, up to 8 s) until no solve runs, so a late refresh does not overwrite what a
/// state sets on the models afterwards.
fn wait_settled(app: &HarnessApp) {
    let editor = app.ui().global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(400);
}

/// Presses the first clickable element labelled `label` of element type `kind` and records the
/// activation (the scenarios' `click` does not). `false` when there is none.
fn press_typed(app: &HarnessApp, label: &str, kind: &str) -> bool {
    let target = app.main_view().by_label(label).into_iter().find(|e| {
        e.accessible_role().is_some_and(is_clickable_role)
            && e.type_name().is_some_and(|t| t.as_str() == kind)
    });
    let Some(element) = target else {
        return false;
    };
    coverage::record_activation(&element);
    element.invoke_accessible_default_action();
    pump(300);
    true
}

// ---------------------------------------------------------------------------------------------
// Top tab pill
// ---------------------------------------------------------------------------------------------

/// The Files & Downloads pill, pressed through its accessible label (the pill's text carries the
/// "(3)" shortcut, its accessible name does not), then the 3D pill back. Proves each press selects
/// its tab. The smoke engine never presses the pill: its deny rule "download" (network) matches
/// the word in the label, but the pill only switches to the local Attachments view.
fn files_tab_pill(ctx: &mut Ctx<'_>) {
    for (label, tab) in [("Files & Downloads", 2), ("3D Spectral Preview", 0)] {
        let pressed = press(ctx.app, label, None);
        ctx.expect(
            pressed,
            "tab-pill",
            format!("the top tab pill {label:?} was not found"),
        );
        let active = ctx.app.ui().get_active_tab();
        ctx.expect(
            !pressed || active == tab,
            "tab-pill-selected",
            format!("pressing {label:?} left active_tab at {active}, expected {tab}"),
        );
        ctx.check_health(label);
    }
}

// ---------------------------------------------------------------------------------------------
// File menu
// ---------------------------------------------------------------------------------------------

/// Opens the editor's File menu and presses `label`'s item. `false` when the button or the item
/// is not there.
fn try_file_item(app: &HarnessApp, label: &str) -> bool {
    press_typed(app, "File", "Rectangle") && press_typed(app, label, "FileMenuItem")
}

/// [`try_file_item`] that records a finding when the item is missing.
fn file_item(ctx: &mut Ctx<'_>, label: &str) -> bool {
    let pressed = try_file_item(ctx.app, label);
    ctx.expect(
        pressed,
        "file-menu-item",
        format!("the File menu shows no item {label:?}"),
    );
    pressed
}

/// The File menu over the fixture library: Load Selected opens the selected design, New Design...
/// opens its dialog, Open... and Save get a cancelled or answered file dialog, Export .asc... gets
/// a path and writes it. Proves each item reaches its handler: the design is installed, the
/// dialog flag is up, the export file appears (a missing file is a note: the export may name its
/// own file beside the chosen one).
fn file_menu_items(ctx: &mut Ctx<'_>) {
    if let Err(message) =
        overlays::launch_base(&mut *ctx.app, ctx.profile, &ctx.seed, Base::Library)
    {
        ctx.report
            .add_simple("-", "launch", format!("file menu: cannot start: {message}"));
        return;
    }
    edit_tab(ctx.app);
    // Load Selected from the empty Edit view; when the File button is not shown without a design,
    // the callback it calls is driven and the item is pressed again further down.
    let loaded_by_menu = try_file_item(ctx.app, "Load Selected");
    if !loaded_by_menu {
        ctx.report
            .note("file menu: no File button over the empty Edit view; Load Selected was invoked");
        ctx.app.ui().global::<EditorModel>().invoke_load_selected();
    }
    let has_design = {
        let editor = ctx.app.ui().global::<EditorModel>();
        wait_until(Duration::from_secs(15), || editor.get_has_design())
    };
    ctx.expect(
        has_design,
        "file-menu-load-selected",
        "Load Selected did not install the selected library design in the editor",
    );
    pump(300);
    ctx.check_health("File > Load Selected");
    if !has_design {
        return;
    }

    if file_item(ctx, "New Design...") {
        let opened = ctx.app.ui().global::<EditorModel>().get_new_dialog_open();
        ctx.expect(
            opened,
            "file-menu-new-design",
            "New Design... did not open the New Design dialog",
        );
        ctx.app
            .ui()
            .global::<EditorModel>()
            .set_new_dialog_open(false);
        pump(200);
    }
    ctx.check_health("File > New Design...");

    // Open... shows the file dialog (answered with a cancel); a design with unsaved changes asks
    // first, and that question is dismissed.
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
    if file_item(ctx, "Open") {
        let editor = ctx.app.ui().global::<EditorModel>();
        if editor.get_unsaved_dialog_open() {
            editor.set_unsaved_dialog_open(false);
            pump(200);
        }
    }
    harness_support::clear_picker_answers();
    ctx.check_health("File > Open...");

    // Save writes to the design's own place; a design with none asks for a file (answered).
    let saved = ctx.scratch_file("harness_file_menu.psdd");
    let _ = std::fs::remove_file(&saved);
    harness_support::queue_picker_path(saved);
    harness_support::queue_picker_cancel();
    if file_item(ctx, "Save") {
        pump(500);
    }
    harness_support::clear_picker_answers();
    ctx.check_health("File > Save");

    let exported = ctx.scratch_file("harness_file_menu.asc");
    let _ = std::fs::remove_file(&exported);
    harness_support::queue_picker_path(exported.clone());
    harness_support::queue_picker_cancel();
    if file_item(ctx, "Export .asc") && !wait_until(Duration::from_secs(10), || exported.exists()) {
        ctx.report.note(format!(
            "coverage_file_menu_items: Export .asc wrote no file at {} (it may name its own file beside it)",
            exported.display()
        ));
    }
    harness_support::clear_picker_answers();
    ctx.check_health("File > Export .asc...");

    if !loaded_by_menu {
        let _ = try_file_item(ctx.app, "Load Selected");
        pump(300);
        ctx.check_health("File > Load Selected, again");
    }
}

// ---------------------------------------------------------------------------------------------
// Trial comparison
// ---------------------------------------------------------------------------------------------

/// The History tab's Trials page with one trial kept and selected, "Compare to Current" pressed:
/// the trial and the design now open side by side in the Compare window, which is closed again.
/// (The pass over the Trials page denies every "compare" label: the window renders both designs.)
fn trials_compare_current(ctx: &mut Ctx<'_>) {
    if let Err(message) = overlays::launch_base(&mut *ctx.app, ctx.profile, &ctx.seed, Base::Design)
    {
        ctx.report.add_simple(
            "-",
            "launch",
            format!("trial comparison: cannot start: {message}"),
        );
        return;
    }
    // The trial's buttons sit under the list, in the Inspector's scroll area: in the default
    // window only the strip above the list is on screen and the rest is not enumerated, so the
    // window is tall like the click-through passes' (`overlays::TALL`).
    ctx.app.set_size(overlays::TALL.0, overlays::TALL.1);
    inspector(ctx.app, 4);
    ctx.app.ui().global::<VariantsModel>().set_view(2);
    pump(300);
    let kept = {
        let trials = ctx.app.ui().global::<TrialsModel>();
        trials.invoke_keep_trial();
        let kept = wait_until(Duration::from_secs(10), || {
            trials.get_rows().row_count() >= 1
        });
        pump(250);
        if let Some(first) = trials.get_rows().row_data(0) {
            trials.invoke_select(first.number);
        }
        kept
    };
    if !ctx.expect(
        kept,
        "trials-keep",
        "Keep as Trial kept no trial for the comparison",
    ) {
        return;
    }
    pump(300);
    let pressed = press(ctx.app, "Compare to Current", None);
    if ctx.expect(
        pressed,
        "trials-compare-current",
        "the Trials page shows no enabled \"Compare to Current\" button",
    ) {
        let opened = wait_until(Duration::from_secs(15), || {
            harness_support::compare_window().is_some()
        });
        ctx.expect(
            opened,
            "trials-compare-current-effect",
            "Compare to Current did not open the Compare window",
        );
        ctx.app.ui().global::<CompareModel>().invoke_close();
        pump(300);
    }
    ctx.check_health("Compare to Current");
}

// ---------------------------------------------------------------------------------------------
// Path-Traced Live Render view
// ---------------------------------------------------------------------------------------------

/// The Live Render tab in Path-Traced mode, through the property the mode pill writes (its
/// `changed` handler does the rest).
fn path_traced(a: &HarnessApp) {
    live_tab(a);
    a.ui().global::<ViewportModel>().set_live_view_mode(1);
    pump(400);
}

/// The Optical Diagnostics button and the metrics strip's "Copy the Five Gemological Numbers"
/// (panel closed, so the button opens it), then the panel's own controls.
fn viewport_states() -> Vec<State> {
    vec![
        State {
            name: "Live Render, Path-Traced, toolbar and metrics",
            base: Base::Design,
            scope: Some("GemViewportView"),
            deny: NONE,
            open: path_traced,
        },
        State {
            name: "Live Render, Optical Diagnostics panel",
            base: Base::Design,
            scope: Some("OpticalPanel"),
            deny: NONE,
            open: |a| {
                path_traced(a);
                a.ui().global::<OpticalModel>().set_panel_open(true);
                pump(300);
            },
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Material Editor and the Stone tab
// ---------------------------------------------------------------------------------------------

/// The Material Editor over a saved biaxial material with the Crystal Axis set (and its Second
/// Axis), two inclusion families turned to the directions that have a Turn and a Tilt, and the
/// Play-of-Color section on with a custom size.
fn editor_axis_families_size(a: &HarnessApp) {
    saved_material(a, "Harness Gaps Biaxial", 4, 3, 0.01, 0.004, "");
    material_open(a);
    let axis = a.ui().global::<CrystalAxisEditorModel>();
    axis.set_axis_mode(1);
    axis.set_second_mode(1);
    // Direction 3 (Tilted) has a Turn and a Tilt, direction 1 (In the Basal Plane) a Turn.
    let families = a.ui().global::<InclusionFamiliesModel>();
    families.invoke_add_family();
    families.invoke_add_family();
    families.invoke_edit_family(0, 1, 3.0);
    families.invoke_edit_family(1, 1, 1.0);
    let structure = a.ui().global::<StructureParamsModel>();
    structure.set_enabled(true);
    structure.invoke_load_defaults(1);
    structure.set_use_size(true);
    pump(300);
}

/// The Stone tab of a sapphire with the first inclusion preset chosen and "Set Silk Turn" on,
/// scrolled by `dy` so the slider shows.
fn silk_turn(a: &HarnessApp, dy: f32) {
    edit_tab(a);
    let _ = design_material(a, "sapphire");
    parameters(a, 0);
    let effects = a.ui().global::<RenderEffectsModel>();
    effects.invoke_refresh();
    let _ = wait_until(Duration::from_secs(8), || {
        effects.get_inclusion_preset_names().row_count() > 0
    });
    if let Some(name) = effects.get_inclusion_preset_names().row_data(0) {
        effects.invoke_inclusion_preset_chosen(name);
    }
    effects.set_inclusion_turn_set(true);
    effects.invoke_inclusions_edited();
    pump(300);
    scroll_scope(a, "ParametersPanel", 0.0, dy);
}

fn material_states() -> Vec<State> {
    vec![
        State {
            name: "Material editor, crystal axis, family directions and play-of-color size",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: editor_axis_families_size,
        },
        State {
            name: "Parameters: Stone, silk turn of a sapphire",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| silk_turn(a, -2400.0),
        },
        State {
            name: "Parameters: Stone, silk turn of a sapphire, end",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| silk_turn(a, -4000.0),
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Tier table and Tune tab
// ---------------------------------------------------------------------------------------------

/// The tier table at a dock wide enough for every column, with tier 0 showing an incomplete orbit
/// that is not detached: its orbit cell is the "Complete orbit" button.
fn orbit_column(a: &HarnessApp) {
    edit_tab(a);
    wait_settled(a);
    let editor = a.ui().global::<EditorModel>();
    editor.set_dock_width(1000.0);
    pump(300);
    let rows: Vec<EditorTierItem> = editor
        .get_tiers()
        .iter()
        .enumerate()
        .map(|(index, mut row)| {
            if index == 0 {
                row.kind = 0;
                row.orbit_status = "3/4 orbit".into();
                row.orbit_incomplete = true;
                row.is_detached = false;
            }
            row
        })
        .collect();
    editor.set_tiers(ModelRc::new(VecModel::from(rows)));
    pump(400);
}

/// The Tune view on a design that left its Baseline (one nudge), tier 0 selected. Jump to Best
/// waits for the mini sweep behind the chart; when no result arrives in time the flag the sweep
/// would set is written so the button can be pressed.
fn tune_buttons(a: &HarnessApp) {
    inspector(a, 2);
    let tune = a.ui().global::<TuneModel>();
    tune.set_active(true);
    pump(600);
    tune.invoke_select_tier(0);
    pump(300);
    if !wait_until(Duration::from_secs(10), || tune.get_can_jump()) {
        println!("ui-harness:   tune: the mini sweep set no best angle; can_jump is written");
        tune.set_can_jump(true);
    }
    if !tune.get_can_revert() {
        println!("ui-harness:   tune: tier 0 reads as unchanged; can_revert is written");
        tune.set_can_revert(true);
    }
    pump(200);
}

fn table_states() -> Vec<State> {
    vec![
        State {
            name: "Tier table, orbit column",
            base: Base::Design,
            scope: Some("EditorTierTable"),
            deny: NONE,
            open: orbit_column,
        },
        State {
            name: "Inspector: Optimize, Tune, design off its Baseline",
            base: Base::Edited,
            scope: Some("EditorInspector"),
            deny: NONE,
            open: tune_buttons,
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Panes and dialogs
// ---------------------------------------------------------------------------------------------

/// Presses the Export dialog's own Ray-Traced renderer pill (the Live Render toolbar has a pill of
/// the same name that sets the viewport's mode, not the export's).
fn dialog_ray_traced(a: &HarnessApp) {
    let pill = overlays::find_scope(a, "ExportDialog").and_then(|view| {
        view.by_label("Ray-Traced")
            .into_iter()
            .find(|e| e.accessible_role().is_some_and(is_clickable_role))
    });
    if let Some(pill) = pill {
        coverage::record_activation(&pill);
        pill.invoke_accessible_default_action();
    }
    pump(300);
}

fn pane_states() -> Vec<State> {
    vec![
        State {
            name: "Edit view, Optimize comparison pane",
            base: Base::Design,
            scope: Some("OptimizerComparePane"),
            deny: NONE,
            // The pane is mounted while `CompareModel.optimizer_inline_open` is up and Apply
            // enabled while `can_keep` is; both are Rust-owned and normally set when a finished
            // Optimize candidate is opened, which needs a long search. The flags are set directly
            // and the handlers behind the buttons work on the (empty) session.
            open: |a| {
                edit_tab(a);
                let compare = a.ui().global::<CompareModel>();
                compare.set_optimizer_inline_open(true);
                compare.set_can_keep(true);
                pump(300);
            },
        },
        State {
            name: "Export dialog, Ray-Traced",
            base: Base::Design,
            scope: Some("ExportDialog"),
            deny: NONE,
            // The Fire box follows the Ray-Traced pill. Opening the dialog loads the renderer from
            // the saved output defaults (Path-Traced), so a write to `engine_index` before the
            // dialog shows is overwritten: the dialog's own pill is pressed, as a user does, and
            // the pass then lists the box.
            open: |a| {
                live_tab(a);
                a.ui().global::<ExportModel>().set_is_open(true);
                pump(300);
                dialog_ray_traced(a);
            },
        },
    ]
}
