//! Targeted presses for controls that no click-through state shows enabled (`coverage` gate:
//! elements).
//!
//! Each piece builds the one state a control needs (a design open in Edit behind the Files tab, a
//! schedule longer than the step strip, a remote design, a paused job, a typed RI range ...),
//! presses or touches the control through its accessible label and records the activation. A
//! control that is not there, or not enabled, is a `coverage-reach` finding that names it, so a
//! renamed label shows up here and not only at the gate.
//!
//! Deviation from `Ctx::check_health`: several presses run a handler on a state the app is not
//! really in (a job that is not paused, a job view that was never loaded, a link that is no
//! link). The handler's warning or error toast is the correct answer there; [`quiet`] writes it as
//! a note and hides it. Panics and unanswered file dialogs stay findings.

use super::{
    super::{Piece, dialogs, existing_install, main_window},
    overlay_states::{
        Base, NONE, State, deny, edit_tab, inspector, jobs_with_rows, live_tab, open_cutting_mode,
        parameters, parameters_stone_open,
    },
    overlays,
    setups::wait_editor_design,
    state_pieces,
    states_dialogs::scroll_until_visible,
    states_editor::meet_point_row,
    states_gaps_editor::tune_buttons,
    states_more::variants_saved,
    states_reach::type_into,
};
use crate::support::{
    Control, Ctx, HarnessApp, SecondaryWindow, WindowView,
    app::{MINIMUM_SIZE, pump, wait_until},
    coverage,
    query::is_clickable_role,
    smoke::{DenyMatch, DenyRule},
};
use pleochroma_cut::{
    BaselineModel, CuttingModeModel, EditorModel, FacetingMachineModel, FileItem, JobCopyModel,
    JobEditModel, LibraryModel, ManipulateModel, ParametersModel, PreferencesModel,
    RemoteAvailabilityItem, RemoteWorkerModel, RenderJobsModel, RiChartModel, RiFieldModel,
    RoughPlanModel, TuneModel, VerdictModel, ViewportModel, harness_support,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel, platform::WindowEvent};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// A piece name and its body.
type Reach = (&'static str, fn(&mut Ctx<'_>));

/// The pieces of this file: `coverage_reach:<name>` per targeted pass, and the header menu states
/// (`overlay:reach-header:<state>`).
pub fn pieces() -> Vec<Piece> {
    let reaches: [Reach; 20] = [
        ("files tab", files_tab),
        ("cutting step strip", cutting_strip),
        ("cutting table", cutting_table),
        ("edit pane switch", edit_pane_switch),
        ("command bar menus", command_bar),
        ("design settings", design_settings),
        ("optimize run settings", optimize_run),
        ("preform revert", preform_revert),
        ("tier form", tier_form),
        ("tune order", tune_order),
        ("variant menu", variant_menu),
        ("quick add tiers", quick_add),
        ("edit viewport banners", edit_banners),
        ("material editor delete", material_delete),
        ("stone tab", stone_tab),
        ("preferences path cache", preferences_path_cache),
        ("preferences faceting machine", preferences_machine),
        ("remote pairing link", remote_pairing),
        ("render jobs", render_jobs),
        ("planner inclusion unit", planner_unit),
    ];
    let mut all = state_pieces("reach-header", existing_install, header_states);
    for (name, run) in reaches {
        all.push(Piece::new(
            format!("coverage_reach:{name}"),
            existing_install,
            run,
        ));
    }
    all
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Relaunches the app into `base` at the tall size of the click-through passes (a control
/// scrolled out of a `ScrollView` is not enumerated). A failed launch is a finding.
fn launch(ctx: &mut Ctx<'_>, base: Base, what: &str) -> bool {
    match overlays::launch_base(&mut *ctx.app, ctx.profile, &ctx.seed, base) {
        Ok(()) => {
            ctx.app.set_size(overlays::TALL.0, overlays::TALL.1);
            pump(200);
            true
        }
        Err(message) => {
            ctx.report
                .add_simple("-", "launch", format!("{what}: cannot start: {message}"));
            false
        }
    }
}

/// Activates the first enabled clickable control of `view` that `wanted` accepts and records it
/// for the gate. `false` when there is none.
fn activate(view: &WindowView, wanted: impl Fn(&Control) -> bool) -> bool {
    let found = view.controls().into_iter().find(|control| {
        control.enabled
            && control.width > 0.0
            && control.height > 0.0
            && is_clickable_role(control.role)
            && wanted(control)
    });
    let Some(control) = found else {
        return false;
    };
    coverage::record_activation(&control.handle);
    coverage::record_wrappers(&view.root, &control.handle, false);
    control.handle.invoke_accessible_default_action();
    pump(250);
    true
}

/// Presses the enabled control labelled `label` of the main window; a missing one is a finding.
fn press(ctx: &mut Ctx<'_>, what: &str, label: &str) -> bool {
    let pressed = activate(&ctx.app.main_view(), |control| control.label == label);
    ctx.expect(
        pressed,
        "coverage-reach",
        format!("{what}: no enabled control labelled {label:?} is showing"),
    )
}

/// [`press`] limited to the element type `kind` (a label two kinds of control share).
fn press_kind(ctx: &mut Ctx<'_>, what: &str, label: &str, kind: &str) -> bool {
    let pressed = activate(&ctx.app.main_view(), |control| {
        control.label == label && control.type_name == kind
    });
    ctx.expect(
        pressed,
        "coverage-reach",
        format!("{what}: no enabled {kind} labelled {label:?} is showing"),
    )
}

/// [`press`] for a label that starts with `prefix` (the rest is data: a name, a description).
fn press_prefix(ctx: &mut Ctx<'_>, what: &str, prefix: &str) -> bool {
    let pressed = activate(&ctx.app.main_view(), |control| {
        control.label.starts_with(prefix)
    });
    ctx.expect(
        pressed,
        "coverage-reach",
        format!("{what}: no enabled control whose label starts with {prefix:?} is showing"),
    )
}

/// Touches the enabled field, slider or spin box labelled `label` in `view` the way the field pass
/// does (a field is set to the value it holds, a slider is stepped up and back) and records it.
fn touch_in(view: &WindowView, label: &str) -> bool {
    let found = view.controls().into_iter().find(|control| {
        control.enabled
            && control.label == label
            && matches!(
                control.role,
                AccessibleRole::TextInput | AccessibleRole::Slider | AccessibleRole::Spinbox
            )
    });
    let Some(control) = found else {
        return false;
    };
    let handle = &control.handle;
    if matches!(control.role, AccessibleRole::TextInput) {
        let current = handle.accessible_value().unwrap_or_default();
        handle.set_accessible_value(current);
    } else {
        handle.invoke_accessible_increment_action();
        handle.invoke_accessible_decrement_action();
    }
    coverage::record_touch(handle);
    coverage::record_wrappers(&view.root, handle, true);
    overlays::record_enclosing_sites(&view.root, handle);
    pump(150);
    true
}

/// [`touch_in`] on the main window; a missing field is a finding.
fn touch(ctx: &mut Ctx<'_>, what: &str, label: &str) -> bool {
    let touched = touch_in(&ctx.app.main_view(), label);
    ctx.expect(
        touched,
        "coverage-reach",
        format!("{what}: no enabled field, slider or spin box labelled {label:?} is showing"),
    )
}

/// The Preferences dialog's component name.
const PREFERENCES: &str = "PreferencesDialog";

/// Turns the mouse wheel by `delta` px over the scroll area inside the component `scope` of the
/// main window. The pointer is put on the area's right edge (the strip under the scroll bar), so
/// that no slider, spin box or combo box of the content takes the wheel for itself. `false` when
/// the component is not showing.
fn wheel_in(app: &HarnessApp, scope: &str, delta: f32) -> bool {
    let Some(view) = overlays::find_scope(app, scope) else {
        return false;
    };
    let area = view
        .root
        .query_descendants()
        .match_inherits("ScrollView")
        .find_first();
    let element = area.as_ref().unwrap_or(&view.root);
    let origin = element.absolute_position();
    let size = element.size();
    let at = slint::LogicalPosition::new(origin.x + size.width - 5.0, origin.y + size.height / 2.0);
    let window = app.ui().window();
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerScrolled {
        position: at,
        delta_x: 0.0,
        delta_y: delta,
    });
    pump(60);
    true
}

/// Scrolls the scroll area inside `scope` until a control whose label `wanted` accepts is
/// enumerable (a control scrolled out of a `ScrollView` is not): from the top, down in steps of
/// 100 px, so that a row in the middle of a long list is not jumped over, and then half a step
/// further to bring the row clear of the edge. `false` when no step showed it.
fn scroll_to_control(app: &HarnessApp, scope: &str, wanted: impl Fn(&str) -> bool) -> bool {
    let showing = || {
        app.main_view().controls().iter().any(|control| {
            control.width > 0.0 && control.height > 0.0 && wanted(control.label.as_str())
        })
    };
    if showing() {
        return true;
    }
    if !wheel_in(app, scope, 100_000.0) {
        return false;
    }
    for _ in 0..120 {
        if showing() {
            let _ = wheel_in(app, scope, -50.0);
            if !showing() {
                let _ = wheel_in(app, scope, 50.0);
            }
            pump(100);
            return showing();
        }
        if !wheel_in(app, scope, -100.0) {
            return false;
        }
    }
    false
}

/// Writes a warning or error toast as a note and hides it (see the module docs).
fn quiet(ctx: &mut Ctx<'_>, what: &str) {
    let ui = ctx.app.ui();
    if !ui.get_toast_visible() {
        return;
    }
    let kind = ui.get_toast_type().to_string();
    if kind == "error" || kind == "warning" {
        let message = ui.get_toast_message().to_string();
        ctx.report.note(format!(
            "coverage reach: {what} answered with a {kind} toast: {message}"
        ));
    }
    ctx.app.ui().set_toast_visible(false);
}

/// Queues two cancels for the file dialog the next press opens.
fn cancel_pickers() {
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
}

/// Drops unused picker answers and what the last presses copied or opened.
fn drop_captures() {
    harness_support::clear_picker_answers();
    let _ = harness_support::take_picker_failures();
    let _ = harness_support::take_clipboard_writes();
    let _ = harness_support::take_opened();
}

/// Waits (real time, up to 20 s) until the app has asked the file picker since `queued` answers
/// were queued. Print / PDF asks only after a background solve of the design, so a fixed pump can
/// end before the dialog: the answers would be dropped and the late dialog would be an unexpected
/// one.
fn wait_picker_asked(queued: usize) -> bool {
    let asked = wait_until(Duration::from_secs(20), || {
        harness_support::pending_picker_answers() < queued
    });
    pump(300);
    asked
}

/// Presses the cutting table's Print / PDF button of element type `kind` with the save dialog
/// cancelled, and waits for the dialog. A press that never asks for a file is a finding.
fn press_print(ctx: &mut Ctx<'_>, what: &str, kind: &str) {
    harness_support::clear_picker_answers();
    cancel_pickers();
    let queued = harness_support::pending_picker_answers();
    if press_kind(ctx, what, "Print / PDF\u{2026}", kind) {
        let asked = wait_picker_asked(queued);
        ctx.expect(
            asked,
            "coverage-reach",
            format!("{what}: Print / PDF ({kind}) asked for no file within 20 s"),
        );
    }
    quiet(ctx, "Print / PDF");
    drop_captures();
}

/// Waits (real time, up to 20 s) until the open design is solved and its cutting rows exist.
fn wait_cutting_rows(app: &HarnessApp) -> bool {
    let editor = app.ui().global::<EditorModel>();
    let ready = wait_until(Duration::from_secs(20), || {
        !editor.get_solve_running() && editor.get_cutting_rows().row_count() > 0
    });
    pump(300);
    ready
}

// ---------------------------------------------------------------------------------------------
// The header's Design Actions Menu
// ---------------------------------------------------------------------------------------------

/// The menu's rows that the plain menu state already presses.
const DENY_MENU_REST: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "build this design",
        "pressed by the plain Design Actions Menu state",
    ),
    deny(
        DenyMatch::Contains,
        "gem cut studio",
        "pressed by the plain Design Actions Menu state",
    ),
    deny(
        DenyMatch::Contains,
        "delete this design",
        "pressed by the plain Design Actions Menu state",
    ),
];

/// A scope name no component has: the menu is a popup, so the pass takes the controls the opener
/// added (`OverlayHost::restore`).
const MENU_SCOPE: Option<&str> = Some("DesignActionsMenuPopup");

/// Waits (real time, up to 10 s) until the selected library design's details are in the header.
/// They are read on a worker after the row is selected, and the header shows its actions
/// ("Export .asc", "More Actions") only for a design with a title.
pub(super) fn wait_design_detail(a: &HarnessApp) {
    let library = a.ui().global::<LibraryModel>();
    let shown = wait_until(Duration::from_secs(10), || {
        !library.get_current_detail().title.is_empty()
    });
    if !shown {
        println!("reach_presses: no design details arrived in the header");
    }
    pump(300);
}

/// Opens the header's "..." menu once the selected design's details are showing.
pub(super) fn open_design_actions(a: &HarnessApp) {
    wait_design_detail(a);
    if !super::overlay_states::press(a, "More Actions", None) {
        println!("reach_presses: the More Actions button was not pressed");
    }
    pump(250);
}

fn header_states() -> Vec<State> {
    vec![
        State {
            // "Delete This Design" turns into the question inside the menu: Yes and No.
            name: "Header, Design Actions Menu, delete question",
            base: Base::Library,
            scope: MENU_SCOPE,
            deny: NONE,
            open: |a| {
                open_design_actions(a);
                if !super::overlay_states::press(a, "Delete This Design", None) {
                    println!("reach_presses: Delete This Design was not pressed");
                }
                pump(250);
            },
        },
        State {
            // A design from a web catalogue has a page: Open on the Web and Copy Link. The
            // fixture designs are local, so the detail is given a link the way a catalogue
            // design has one (the opener and the clipboard are captured).
            name: "Header, Design Actions Menu, design with a web page",
            base: Base::Library,
            scope: MENU_SCOPE,
            deny: DENY_MENU_REST,
            open: |a| {
                // The real details first: arriving later, they would replace the link.
                wait_design_detail(a);
                let library = a.ui().global::<LibraryModel>();
                let mut detail = library.get_current_detail();
                detail.url = "https://example.invalid/harness-design".into();
                detail.is_local = false;
                library.set_current_detail(detail);
                pump(250);
                open_design_actions(a);
            },
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Files tab and Cutting Instructions
// ---------------------------------------------------------------------------------------------

/// The Files tab with the selected design open in Edit (the "Make" exports need it) and a link on
/// the attached file (Open and Copy URL need one; an imported file has none).
fn files_tab(ctx: &mut Ctx<'_>) {
    let what = "files tab";
    if !launch(ctx, Base::Library, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    editor.invoke_load_selected();
    let loaded = wait_until(Duration::from_secs(20), || editor.get_has_design());
    if !ctx.expect(
        loaded,
        "coverage-reach",
        "files tab: Load Selected installed no design in the editor",
    ) {
        return;
    }
    wait_editor_design(&ui);
    main_window::select_top_tab_model(ctx.app, 2);
    let library = ui.global::<LibraryModel>();
    let _ = wait_until(Duration::from_secs(10), || {
        library.get_current_files().row_count() > 0
    });
    let mut files: Vec<FileItem> = library.get_current_files().iter().collect();
    if files.is_empty() {
        files.push(FileItem {
            name: "harness.asc".into(),
            url: "".into(),
            size_str: "1 kB".into(),
        });
    }
    for file in &mut files {
        if file.url.is_empty() {
            file.url = "https://example.invalid/harness.asc".into();
        }
    }
    library.set_current_files(ModelRc::new(VecModel::from(files)));
    pump(300);
    press_kind(ctx, what, "Open", "KitButton");
    press(ctx, what, "Copy URL");
    for label in [
        "Cutting Sheet (PDF)\u{2026}",
        "Edited .asc\u{2026}",
        ".gcs\u{2026}",
    ] {
        cancel_pickers();
        press_kind(ctx, what, label, "KitButton");
        pump(300);
        quiet(ctx, label);
    }
    drop_captures();
    ctx.check_health("Files tab with a design open in Edit");
}

/// Cutting mode on the fixture with the most tiers: above 20 steps the strip shows a window of
/// 18 with an "Earlier Steps" and a "Later Steps" cell.
fn cutting_strip(ctx: &mut Ctx<'_>) {
    let what = "cutting step strip";
    if !launch(ctx, Base::Library, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    let row = library
        .get_diagram_list()
        .iter()
        .find(|row| row.title.as_str().contains("For Fun"));
    if let Some(row) = row {
        library.invoke_select_diagram(row.id);
        pump(400);
    }
    let editor = ui.global::<EditorModel>();
    editor.invoke_load_selected();
    let loaded = wait_until(Duration::from_secs(20), || editor.get_has_design());
    if !ctx.expect(
        loaded,
        "coverage-reach",
        "cutting step strip: Load Selected installed no design in the editor",
    ) {
        return;
    }
    wait_editor_design(&ui);
    edit_tab(ctx.app);
    open_cutting_mode(ctx.app);
    let cutting = ui.global::<CuttingModeModel>();
    let steps = cutting.get_schedule().row_count();
    if !ctx.expect(
        steps > 20,
        "coverage-reach",
        format!(
            "cutting step strip: the schedule has {steps} step(s); the strip shows its window only above 20"
        ),
    ) {
        return;
    }
    for label in ["Earlier Steps", "Later Steps"] {
        // Step 11: the window starts after the first step and ends before the last.
        cutting.invoke_jump_to(10);
        pump(400);
        press(ctx, what, label);
    }
    ctx.check_health("Cutting mode step strip");
}

/// A window width at which the cutting table is at least 1000 px wide beside the open library
/// list, so that its step panel shows.
const STEP_PANEL_WIDTH: u32 = 1800;

/// The Cutting Instructions tab over a design open in Edit: Print / PDF, the step panel's two
/// buttons (the panel shows the row a pointer click picked), Start Cut Mode, and the icon buttons
/// of the narrow layout.
fn cutting_table(ctx: &mut Ctx<'_>) {
    let what = "cutting table";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let ready = wait_cutting_rows(ctx.app);
    ctx.expect(
        ready,
        "coverage-reach",
        "cutting table: the template design has no cutting rows",
    );
    main_window::select_top_tab_model(ctx.app, 1);
    pump(300);
    press_print(ctx, what, "KitButton");

    // The step panel ("Selected Step") shows from a table width of 1000 px; beside the open
    // library list the table is narrower than that in the 1366 px window.
    ctx.app.set_size(STEP_PANEL_WIDTH, overlays::TALL.1);
    pump(300);
    for label in ["Copy Step", "Open in Edit"] {
        main_window::select_top_tab_model(ctx.app, 1);
        pump(300);
        let row = ctx.app.main_view().elements().into_iter().find(|element| {
            element.accessible_role() == Some(AccessibleRole::ListItem)
                && element
                    .accessible_label()
                    .is_some_and(|text| text.starts_with("Step 1:"))
        });
        let found = row.is_some();
        if let Some(row) = row {
            dialogs::click_center(&ui, &row);
            pump(250);
        }
        if ctx.expect(
            found,
            "coverage-reach",
            "cutting table: no step row labelled \"Step 1: ...\" is showing",
        ) {
            press_kind(ctx, what, label, "KitButton");
            quiet(ctx, label);
        }
    }
    drop_captures();

    // The narrow layout: under 520 px the first two buttons are icon buttons.
    main_window::select_top_tab_model(ctx.app, 1);
    let mut narrow = false;
    for width in [900_u32, 720, 600, 480] {
        ctx.app.set_size(width, overlays::TALL.1);
        pump(300);
        narrow =
            ctx.app.main_view().controls().iter().any(|control| {
                control.type_name == "KitIconButton" && control.label == "Copy as Text"
            });
        if narrow {
            break;
        }
    }
    if ctx.expect(
        narrow,
        "coverage-reach",
        "cutting table: no window width down to 480 px showed the icon buttons of the narrow layout",
    ) {
        press_kind(ctx, what, "Copy as Text", "KitIconButton");
        press_print(ctx, what, "KitIconButton");
    }
    ctx.app.set_size(overlays::TALL.0, overlays::TALL.1);
    pump(300);

    if press_kind(ctx, what, "Start Cut Mode", "KitButton") {
        let cutting = ui.global::<CuttingModeModel>();
        let _ = wait_until(Duration::from_secs(10), || !cutting.get_busy());
        cutting.invoke_close();
        pump(300);
    }
    ctx.check_health("Cutting Instructions over a design open in Edit");
}

/// The Edit view in a window under 700 px high and too narrow for the side-by-side layout: it
/// shows one pane at a time, and the switch beside the status strip ("3D View", "Tier Editor")
/// picks which. Pressed at the minimum window size.
fn edit_pane_switch(ctx: &mut Ctx<'_>) {
    let what = "edit pane switch";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    wait_editor_design(&ui);
    edit_tab(ctx.app);
    ctx.app.set_size(MINIMUM_SIZE.0, MINIMUM_SIZE.1);
    pump(300);
    for label in ["3D View", "Tier Editor"] {
        press_kind(ctx, what, label, "KitSegment");
    }
    ctx.app.set_size(overlays::TALL.0, overlays::TALL.1);
    pump(300);
    ctx.check_health("Edit pane switch at the minimum window size");
}

// ---------------------------------------------------------------------------------------------
// The editor's command bar and sheets
// ---------------------------------------------------------------------------------------------

/// The File menu's Copy Cutting Schedule, the Solve chevron's Abandon Solve while a solve is
/// marked running, the Baseline menu's two Compare items with a Baseline and a snapshot held, and
/// the RI chart's Queue Material Check Set and close button.
fn command_bar(ctx: &mut Ctx<'_>) {
    let what = "command bar";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    edit_tab(ctx.app);
    let ready = wait_cutting_rows(ctx.app);
    ctx.expect(
        ready,
        "coverage-reach",
        "command bar: the template design has no cutting rows",
    );
    if press_kind(ctx, what, "File", "Rectangle") {
        press_kind(ctx, what, "Copy Cutting Schedule", "FileMenuItem");
    }
    drop_captures();
    ctx.check_health("File > Copy Cutting Schedule");

    // Abandon Solve is enabled only while a solve runs; auto-solve off, so no real solve ends
    // and clears the flag in between.
    let editor = ui.global::<EditorModel>();
    editor.set_auto_solve_budget_ms(0);
    pump(200);
    editor.set_solve_running(true);
    pump(250);
    if press(ctx, what, "Auto-Solve") {
        press_kind(ctx, what, "Abandon Solve", "BarMenuItem");
    }
    editor.set_solve_running(false);
    pump(250);
    quiet(ctx, "Abandon Solve");

    // The Baseline menu: "Compare..." needs a Baseline, "Compare" (with the snapshot) a snapshot.
    ui.global::<BaselineModel>().invoke_set_baseline();
    editor.invoke_snapshot_design();
    let held = wait_until(Duration::from_secs(20), || {
        ui.global::<BaselineModel>().get_has_baseline() && editor.get_has_snapshot()
    });
    ctx.expect(
        held,
        "coverage-reach",
        "command bar: Set as Baseline and Set Snapshot left no Baseline or no snapshot",
    );
    pump(300);
    for label in ["Compare...", "Compare"] {
        if press(ctx, what, "Baseline") {
            press_kind(ctx, what, label, "BarMenuItem");
            pump(400);
            quiet(ctx, label);
        }
    }
    ctx.check_health("Baseline menu with a Baseline and a snapshot");

    // The RI chart over the design. Queue Material Check Set asks for the export folder first.
    let chart = ui.global::<RiChartModel>();
    chart.invoke_open_chart();
    let _ = wait_until(Duration::from_secs(10), || chart.get_open());
    pump(300);
    chart.set_can_queue_check_set(true);
    pump(150);
    if let Ok(dir) = std::env::current_dir().map(|d| d.join("harness_queue_exports")) {
        let _ = std::fs::create_dir_all(&dir);
        harness_support::queue_picker_path(dir.clone());
        harness_support::queue_picker_path(dir);
    }
    press(ctx, what, "Queue Material Check Set");
    pump(500);
    quiet(ctx, "Queue Material Check Set");
    drop_captures();
    if !chart.get_open() {
        chart.invoke_open_chart();
        let _ = wait_until(Duration::from_secs(10), || chart.get_open());
        pump(300);
    }
    press(ctx, what, "Close the chart. Escape does the same.");
    ctx.check_health("RI chart buttons");
}

/// The Design Settings sheet with the RI range typed by hand (its two fields exist only then),
/// and the Gear Remap dialog's Rounding box (shown once an index was rounded).
fn design_settings(ctx: &mut Ctx<'_>) {
    let what = "design settings";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    edit_tab(ctx.app);
    let _ = wait_cutting_rows(ctx.app);
    if press(ctx, what, "Edit Design Settings") {
        pump(300);
        ui.global::<RiFieldModel>().set_source_index(3);
        pump(300);
        touch(ctx, what, "Lowest RI");
        touch(ctx, what, "Highest RI, optional");
        quiet(ctx, "typed RI range");
    }
    ctx.check_health("Design Settings, RI range typed by hand");

    let editor = ui.global::<EditorModel>();
    editor.set_gear_remap_rounded_count(1);
    editor.set_gear_remap_open(true);
    pump(400);
    press(ctx, what, "Rounding");
    editor.set_gear_remap_open(false);
    pump(200);
    ctx.check_health("Gear Remap, Rounding");
}

/// The Optimize tab with its "Run" fold open: Budget, Starts, Seed, Candidates and Polish.
fn optimize_run(ctx: &mut Ctx<'_>) {
    let what = "optimize run settings";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    inspector(ctx.app, 2);
    if press(ctx, what, "Run Settings") {
        for label in [
            "Search Budget",
            "Number of Starting Arrangements, 1 to 32",
            "Search Seed",
            "Number of Candidates to Keep",
        ] {
            touch(ctx, what, label);
        }
        press_kind(ctx, what, "Polish", "OptionRow");
    }
    quiet(ctx, "Optimize run settings");
    ctx.check_health("Optimize tab, Run settings open");
}

/// The Preform tab with a size typed and not applied: Revert is enabled only then.
fn preform_revert(ctx: &mut Ctx<'_>) {
    let what = "preform revert";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    inspector(ctx.app, 1);
    let typed = type_into(
        ctx.app,
        "Half-Width (Model Units, Girdle Half-Width = 1)",
        "1.3",
    );
    ctx.expect(
        typed,
        "coverage-reach",
        "preform revert: the Half-Width field was not found",
    );
    pump(300);
    press_kind(ctx, what, "Revert", "KitButton");
    quiet(ctx, "Revert");
    ctx.check_health("Preform tab, Revert");
}

/// The tier form's Meet Point row with a picked corner (Clear), in pick mode (Cancel Pick), and
/// the exact scale value's "Use Solved".
fn tier_form(ctx: &mut Ctx<'_>) {
    let what = "tier form";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    meet_point_row(
        ctx.app,
        1,
        "Pavilion Main \u{b7} Girdle \u{b7} Pavilion Main",
        "Lower Girdle is cut until it reaches the corner where Pavilion Main, Girdle and Pavilion Main meet.",
    );
    // A late solve result writes the row's state again; the state is written once more right
    // before the press.
    editor.set_meet_pick_state(1);
    pump(120);
    press(ctx, what, "Clear Picked Meet Point");
    quiet(ctx, "Clear Picked Meet Point");

    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_pick_mode(true);
    manipulate.invoke_pick_toggled();
    pump(300);
    if !manipulate.get_pick_mode() {
        manipulate.set_pick_mode(true);
        pump(120);
    }
    press(ctx, what, "Cancel Pick");
    manipulate.set_pick_mode(false);
    pump(200);
    quiet(ctx, "Cancel Pick");
    ctx.check_health("Tier form, Meet Point row");

    // "Use Solved" sits beside the exact scale value (constraint kind 2) of a solved tier.
    inspector(ctx.app, 0);
    editor.set_suggested_constraint_kind(2);
    pump(400);
    press(ctx, what, "Use Solved Mast");
    pump(400);
    quiet(ctx, "Use Solved Mast");
    ctx.check_health("Tier form, Use Solved");
}

/// The Tune view with the second tier of the tuning order picked: its "Up" is enabled (the first
/// tier's is not).
fn tune_order(ctx: &mut Ctx<'_>) {
    let what = "tune order";
    if !launch(ctx, Base::Edited, what) {
        return;
    }
    tune_buttons(ctx.app);
    let tune = ctx.app.ui().global::<TuneModel>();
    let second = tune.get_tiers().row_data(1).map(|row| row.tier);
    if let Some(tier) = second {
        tune.invoke_select_tier(tier);
    }
    pump(500);
    if ctx.expect(
        second.is_some(),
        "coverage-reach",
        "tune order: the Tune view lists fewer than two tiers",
    ) {
        press_kind(ctx, what, "Up", "CompactButton");
    }
    quiet(ctx, "Tune, Up");
    ctx.check_health("Tune view, Up");
}

/// A saved variant's "More" menu: Rename, Note and Delete (the menu is a popup that the
/// click-through pass closes with its first press).
fn variant_menu(ctx: &mut Ctx<'_>) {
    let what = "variant menu";
    if !launch(ctx, Base::Edited, what) {
        return;
    }
    variants_saved(ctx.app, 0);
    for prefix in ["Rename ", "Edit the Note of ", "Delete "] {
        if press_prefix(ctx, what, "More for ") {
            let pressed = activate(&ctx.app.main_view(), |control| {
                control.type_name == "VariantMenuItem" && control.label.starts_with(prefix)
            });
            ctx.expect(
                pressed,
                "coverage-reach",
                format!("variant menu: no enabled menu item whose label starts with {prefix:?}"),
            );
            pump(300);
            quiet(ctx, prefix);
        }
    }
    ctx.check_health("Variant More menu");
}

/// The Add Tier menu over an empty design: Table and Culet are offered only while the design has
/// no tier of that name (every template has both).
fn quick_add(ctx: &mut Ctx<'_>) {
    let what = "quick add tiers";
    if !launch(ctx, Base::Startup, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    editor.set_new_template_index(0);
    editor.set_new_dialog_open(true);
    pump(200);
    // The Empty form's create, as in the "Edit view, empty design" state.
    editor.invoke_new_design_create(
        1,
        "1.5".into(),
        "1.0".into(),
        "1.5".into(),
        0,
        "96".into(),
        "8".into(),
        true,
        0,
        0,
    );
    let created = wait_until(Duration::from_secs(15), || editor.get_has_design());
    if !ctx.expect(
        created,
        "coverage-reach",
        "quick add tiers: the empty design was not created",
    ) {
        return;
    }
    edit_tab(ctx.app);
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(400);
    for label in ["Table", "Culet"] {
        if press(ctx, what, "More Ways to Add a Tier") {
            press_kind(ctx, what, label, "TierMenuItem");
            let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
            pump(400);
            quiet(ctx, label);
        }
    }
    ctx.check_health("Add Tier menu over an empty design");
}

/// The Edit viewport's two banners over an edited design (their Undo buttons need an edit to take
/// back): the vanished-facet notice, then the problem banner, each shown by the properties the
/// verdict presenter writes.
fn edit_banners(ctx: &mut Ctx<'_>) {
    let what = "edit viewport banners";
    if !launch(ctx, Base::Edited, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let verdict = ui.global::<VerdictModel>();
    edit_tab(ctx.app);
    let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
    pump(400);
    verdict.set_vanish_notice_text("This cut away Pavilion Main.".into());
    verdict.set_vanish_notice_tier(0);
    verdict.set_vanish_notice_tier_label("Pavilion Main".into());
    verdict.set_vanish_notice_shown(true);
    editor.set_can_undo(true);
    pump(300);
    press_kind(ctx, what, "Undo", "KitButton");
    let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
    pump(400);
    quiet(ctx, "vanished-facet notice, Undo");

    // The problem banner takes the notice's place while the verdict is a Problem.
    verdict.set_vanish_notice_shown(false);
    verdict.set_level(2);
    editor.set_can_undo(true);
    pump(300);
    press_kind(ctx, what, "Undo Last Change", "KitButton");
    let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
    pump(400);
    quiet(ctx, "problem banner, Undo Last Change");
    ctx.check_health("Edit viewport banners");
}

/// The Material Editor's Delete: enabled only for the name of a saved custom material. The flag
/// Rust sets for such a name is written; the press opens the confirmation and deletes nothing.
fn material_delete(ctx: &mut Ctx<'_>) {
    let what = "material editor delete";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    edit_tab(ctx.app);
    let viewport = ctx.app.ui().global::<ViewportModel>();
    viewport.set_material_editor_open(true);
    pump(400);
    viewport.set_material_name_is_existing_custom(true);
    pump(250);
    scroll_until_visible(ctx.app, "MaterialEditorDialog", "Delete");
    press_kind(ctx, what, "Delete", "ActionButton");
    quiet(ctx, "Material Editor, Delete");
    ctx.check_health("Material Editor, Delete");
}

// ---------------------------------------------------------------------------------------------
// Parameters dock, Preferences, remote
// ---------------------------------------------------------------------------------------------

/// Hides the Material Reference window when a press opened it.
fn hide_reference_window() {
    let _ = wait_until(Duration::from_secs(5), || {
        harness_support::material_reference_window().is_some()
    });
    if let Some(window) = harness_support::material_reference_window() {
        let _ = window.hide();
    }
    pump(200);
}

/// Presses the Stone tab's "Material Menu" button at the top of the identity card.
///
/// The button sits at the right end of the card, and the card is as wide as the tab's widest
/// fixed-width row. A row wider than the 360 px dock makes the tab scroll sideways and pushes
/// the button out of the scroll area, where it is not enumerated (runs up to 2026-11-18: the
/// "Previewing ..." badge did that whenever a preview was on; scrolling down or mounting the
/// tab again cannot bring it back). So a miss writes a note with the widths: a Stone tab wider
/// than its panel is that overflow again, and a bug of the tab.
fn press_material_menu(ctx: &mut Ctx<'_>, what: &str) -> bool {
    let label = "Material Menu";
    if press(ctx, what, label) {
        return true;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let viewport = ui.global::<ViewportModel>();
    let dock = ui.global::<ParametersModel>();
    let view = ctx.app.main_view();
    let labelled = view.by_label(label).into_iter().count();
    let width_of = |component: &str| {
        view.root
            .query_descendants()
            .match_inherits(component)
            .find_first()
            .map_or_else(
                || "not in the window".to_owned(),
                |element| format!("{:.0} px wide", element.size().width),
            )
    };
    ctx.report.note(format!(
        "coverage reach: {what}: {label} is missing with EditorModel.enabled {}, has_design {},          ViewportModel.shows_editor_design {}, previewing {}, ParametersModel.open {},          active_tab {}; ParametersPanel {}, StoneTab {} (wider than the panel: the tab scrolls          sideways and the button is outside the view); {labelled} element(s) carry the label",
        editor.get_enabled(),
        editor.get_has_design(),
        viewport.get_shows_editor_design(),
        viewport.get_previewing(),
        dock.get_open(),
        dock.get_active_tab(),
        width_of("ParametersPanel"),
        width_of("StoneTab"),
    ));
    false
}

/// The Stone tab: the material guess (Set Material), the empty gem data link (Edit Material...),
/// Material Details... and the menu's Material Reference... (the click-through passes deny both:
/// they open a second window, hidden again here), the preview's Preview Color row and its color
/// control, and the Render tab's "This Machine Renders a Share Too" with a remote configured.
fn stone_tab(ctx: &mut Ctx<'_>) {
    let what = "stone tab";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let viewport = ui.global::<ViewportModel>();
    // The identity card with the menu button is the open design's (`design_shown`): the design
    // must own the viewport and no solve may be about to refresh the tab.
    wait_editor_design(&ui);
    let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
    parameters_stone_open(ctx.app);
    pump(400);

    // The menu comes first, on the untouched design: the steps further down write properties
    // and change the design's material, and a refresh after them can take the card away.
    if press_material_menu(ctx, what) && press(ctx, what, "Material Reference...") {
        hide_reference_window();
    }
    ctx.check_health("Stone tab, Material Reference");

    // A viewport material that is not the design's is a preview: the menu gains Preview Color,
    // which shows the preview's own color control under the menu.
    viewport.set_viewport_material_linked(false);
    pump(300);
    if press_material_menu(ctx, what) && press_kind(ctx, what, "Preview Color", "StoneMenuItem") {
        let pressed = activate(&ctx.app.main_view(), |control| {
            control
                .id
                .replace('_', "-")
                .ends_with("render-color-control-l344")
        });
        ctx.expect(
            pressed,
            "coverage-reach",
            "stone tab: the preview's color control (render-color-control-l344) is not showing",
        );
    }
    viewport.set_viewport_material_linked(true);
    pump(300);
    quiet(ctx, "Preview Color");
    ctx.check_health("Stone tab, preview color");
    // The tab is mounted again: that closes the menu and the color control's popup.
    parameters_stone_open(ctx.app);
    pump(300);

    // The inferred-material badge: the properties Rust writes for a design that names none.
    editor.set_material_guess_name("Quartz".into());
    editor.set_material_guess_text("Quartz?".into());
    editor.set_material_guess_applied(false);
    pump(300);
    press_kind(ctx, what, "Set Material", "KitButton");
    pump(400);
    quiet(ctx, "Set Material");

    // The gem facts of a material without gem data: one line with the link to the editor.
    editor.set_gem_name("Diamond".into());
    editor.set_gem_card_shown(true);
    editor.set_gem_empty(true);
    pump(300);
    if press(ctx, what, "Edit Material...") {
        viewport.set_material_editor_open(false);
        pump(250);
    }
    editor.set_gem_empty(false);
    editor.set_gem_card_shown(true);
    pump(300);
    if press_kind(ctx, what, "Material Details...", "KitButton") {
        hide_reference_window();
    }
    ctx.check_health("Stone tab, material links");

    // The Render tab's Compute card with a remote coordinator configured.
    let remote = ui.global::<RemoteWorkerModel>();
    remote.set_configured(true);
    remote.set_availability(RemoteAvailabilityItem {
        configured: true,
        reachable_render_capable: true,
    });
    parameters(ctx.app, 2);
    pump(300);
    let share = "Final-Picture Exports: This Machine Renders a Share Too";
    let shown = |app: &HarnessApp| {
        app.main_view()
            .controls()
            .iter()
            .any(|control| control.label.starts_with(share))
    };
    if !shown(&*ctx.app) {
        // The Compute card is folded.
        let _ = activate(&ctx.app.main_view(), |control| control.label == "Compute");
        pump(300);
    }
    press_prefix(ctx, what, share);
    quiet(ctx, share);
    ctx.check_health("Render tab, final-picture share");
}

/// Preferences with the path cache rows shown. The app's default estimator (exit collection) has
/// no path cache, so `seed_path_cache` hides the rows; the flag is set the way a Branching build
/// sets it, as the Export dialog's path cache states do.
fn preferences_path_cache(ctx: &mut Ctx<'_>) {
    let what = "preferences path cache";
    if !launch(ctx, Base::Startup, what) {
        return;
    }
    let prefs = ctx.app.ui().global::<PreferencesModel>();
    prefs.set_dialog_open(true);
    pump(250);
    prefs.set_path_cache_available(true);
    pump(300);
    // The rows sit in the middle of the dialog's list, which shows about 500 px at a time: each
    // is scrolled to before it is pressed.
    let switch = "Speed Up Still Views With a Path Cache";
    scroll_to_control(ctx.app, PREFERENCES, |label| label.starts_with(switch));
    press_prefix(ctx, what, switch);
    let limit = "Path Cache Disk Limit in Gigabytes";
    scroll_to_control(ctx.app, PREFERENCES, |label| label == limit);
    touch(ctx, what, limit);
    for label in ["Open Folder", "Clear Path Cache"] {
        scroll_to_control(ctx.app, PREFERENCES, |text| text == label);
        press(ctx, what, label);
    }
    quiet(ctx, "path cache rows");
    drop_captures();
    ctx.check_health("Preferences, path cache rows");
}

/// The Faceting Machine section of Preferences with the mast to the side and a custom sweep: the
/// Mast Position slider and the two sweep radius sliders show only then, and the section is
/// taller than the dialog's list, so each slider is scrolled to before it is touched.
fn preferences_machine(ctx: &mut Ctx<'_>) {
    let what = "preferences faceting machine";
    if !launch(ctx, Base::Startup, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    ui.global::<PreferencesModel>().set_dialog_open(true);
    pump(250);
    let machine = ui.global::<FacetingMachineModel>();
    machine.set_mast_distance_mm(30.0);
    machine.invoke_value_changed();
    machine.invoke_sweep_preset_chosen(3);
    pump(300);
    for label in [
        "Mast Position in Millimetres",
        "Sweep Inner Radius in Millimetres",
        "Sweep Outer Radius in Millimetres",
    ] {
        scroll_to_control(ctx.app, PREFERENCES, |text| text == label);
        touch(ctx, what, label);
    }
    quiet(ctx, "faceting machine sliders");
    ctx.check_health("Preferences, faceting machine sliders");
}

/// The Remote Coordinator dialog's pairing link: Review is enabled once the field holds text. The
/// text is no pairing link, so nothing is offered and nothing connects.
fn remote_pairing(ctx: &mut Ctx<'_>) {
    let what = "remote pairing link";
    if !launch(ctx, Base::Startup, what) {
        return;
    }
    let remote = ctx.app.ui().global::<RemoteWorkerModel>();
    remote.invoke_open_dialog();
    pump(300);
    let typed = type_into(ctx.app, "Pairing Link", "harness-not-a-pairing-link");
    ctx.expect(
        typed,
        "coverage-reach",
        "remote pairing link: the Pairing Link field was not found",
    );
    pump(200);
    press(ctx, what, "Review");
    pump(300);
    ctx.app
        .ui()
        .global::<RemoteWorkerModel>()
        .invoke_cancel_pairing();
    quiet(ctx, "Review");
    ctx.check_health("Remote Coordinator, pairing link");
}

// ---------------------------------------------------------------------------------------------
// Render jobs
// ---------------------------------------------------------------------------------------------

/// Gives the queued rows every action a row and its menu can offer (the queue itself holds two
/// waiting jobs): the first row shows Resume, the second Restart, and the Compute menu offers the
/// remote choices. Rust writes the rows again after every change of the queue, so this runs
/// before each press.
fn offer_job_actions(app: &HarnessApp) {
    let jobs = app.ui().global::<RenderJobsModel>();
    let rows: Vec<_> = jobs
        .get_rows()
        .iter()
        .enumerate()
        .map(|(index, mut row)| {
            row.can_resume = true;
            row.can_restart = true;
            row.can_cancel = true;
            row.can_delete = true;
            row.can_move_up = true;
            row.can_move_down = true;
            row.can_duplicate = true;
            row.can_copy = true;
            row.can_load_view = true;
            row.can_edit = true;
            row.has_menu = true;
            row.can_show = false;
            row.show_cancel = false;
            row.primary_action = if index == 0 { 2 } else { 4 };
            row
        })
        .collect();
    jobs.set_rows(ModelRc::new(VecModel::from(rows)));
    jobs.set_compute_remote_enabled(true);
    jobs.set_panel_open(true);
    pump(300);
}

/// Opens the Edit Render Job dialog on job `id` when it is not open.
fn open_job_edit(app: &HarnessApp, id: i32) -> bool {
    let edit = app.ui().global::<JobEditModel>();
    if !edit.get_open() {
        app.ui().global::<RenderJobsModel>().invoke_edit_job(id);
        let _ = wait_until(Duration::from_secs(10), || edit.get_open());
        pump(400);
    }
    edit.get_open()
}

/// The Render Jobs window over two queued jobs (nothing starts them; Run Next and Start Queue are
/// never pressed): the row's Resume and Restart, the row menu, the Compute menu, the Edit Render
/// Job and Copy to Materials dialogs, and the Load Job View banner.
fn render_jobs(ctx: &mut Ctx<'_>) {
    let what = "render jobs";
    if !launch(ctx, Base::Design, what) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let jobs = ui.global::<RenderJobsModel>();
    jobs_with_rows(ctx.app);
    let first = jobs.get_rows().row_data(0).map(|row| row.id);
    let Some(id) = first else {
        ctx.expect(
            false,
            "coverage-reach",
            "render jobs: no still job was queued",
        );
        return;
    };

    // The row's own buttons.
    for label in ["Resume", "Restart"] {
        offer_job_actions(ctx.app);
        press_kind(ctx, what, label, "RowButton");
        pump(300);
        quiet(ctx, label);
    }
    // The row menu of the first row. Copy to Materials opens its dialog (closed again); Load Job
    // View comes last, its banner is pressed below.
    for label in [
        "Up",
        "Down",
        "Restart",
        "Cancel",
        "Copy to Materials",
        "Load Job View",
    ] {
        offer_job_actions(ctx.app);
        if press(ctx, what, "More actions") {
            press_kind(ctx, what, label, "TierMenuItem");
            pump(400);
            quiet(ctx, label);
        }
        ui.global::<JobCopyModel>().set_open(false);
        pump(150);
    }
    ctx.check_health("Render Jobs, row buttons and row menu");

    // The banner of a loaded job view, on the Live Render view. When the job's view did not load
    // (the harness renders nothing), the banner is shown the way Rust shows it.
    jobs.set_panel_open(false);
    live_tab(ctx.app);
    pump(400);
    for label in ["Update Job from This View", "Back to my view"] {
        if jobs.get_job_view_id() < 0 {
            jobs.set_job_view_title("Showing the view of job 1".into());
            jobs.set_job_view_id(id);
        }
        jobs.set_job_view_can_update(true);
        pump(300);
        press_kind(ctx, what, label, "KitButton");
        pump(300);
        quiet(ctx, label);
    }
    jobs.set_job_view_id(-1);
    pump(200);
    ctx.check_health("Load Job View banner buttons");

    // The Compute menu of the window's header.
    for label in ["Local Only", "Remote Only", "Local + Remote"] {
        offer_job_actions(ctx.app);
        if press(ctx, what, "Compute") {
            press_kind(ctx, what, label, "TierMenuItem");
            pump(300);
            quiet(ctx, label);
        }
    }
    ctx.check_health("Render Jobs, Compute menu");

    // Edit Render Job. Each button that closes the dialog is followed by a fresh open.
    let edit = ui.global::<JobEditModel>();
    let scope = "JobEditDialog";
    let edit_open = open_job_edit(ctx.app, id);
    if ctx.expect(
        edit_open,
        "coverage-reach",
        "render jobs: Edit did not open the Edit Render Job dialog",
    ) {
        scroll_until_visible(ctx.app, scope, "Label");
        touch(ctx, what, "Label");
        scroll_until_visible(ctx.app, scope, "Samples per Pixel, as a Power of Two");
        touch(ctx, what, "Samples per Pixel, as a Power of Two");
        scroll_until_visible(ctx.app, scope, "Rename from Template");
        press(ctx, what, "Rename from Template");
        // A missing HDR map and a refused save are states of a job on disk; their callouts are
        // shown by the flags Rust sets for them.
        edit.set_hdr_missing(true);
        pump(300);
        scroll_until_visible(ctx.app, scope, "Locate HDR Map...");
        cancel_pickers();
        press(ctx, what, "Locate HDR Map...");
        drop_captures();
        quiet(ctx, "Locate HDR Map");
        edit.set_refused(true);
        pump(300);
        press(ctx, what, "Load the Job Again");
        pump(300);
        quiet(ctx, "Load the Job Again");
    }
    if open_job_edit(ctx.app, id) {
        press(ctx, what, "Save as New Job");
        pump(400);
        quiet(ctx, "Save as New Job");
    }
    if open_job_edit(ctx.app, id) {
        edit.set_can_save(true);
        pump(200);
        press(ctx, what, "Save Changes");
        pump(400);
        quiet(ctx, "Save Changes");
    }
    // "Render this job again?" may follow a save; the question is put away unanswered.
    if edit.get_requeue_question_open() {
        edit.set_requeue_question_open(false);
        pump(200);
    }
    if open_job_edit(ctx.app, id) {
        press(ctx, what, "Close this dialog and change nothing.");
    }
    edit.set_open(false);
    pump(200);
    ctx.check_health("Edit Render Job dialog");

    // Copy to Materials: the search field and the close button ("Copy" is never pressed).
    let copy = ui.global::<JobCopyModel>();
    jobs.invoke_open_copy_dialog(id);
    let opened = wait_until(Duration::from_secs(10), || copy.get_open());
    pump(300);
    if ctx.expect(
        opened,
        "coverage-reach",
        "render jobs: Copy to Materials did not open its dialog",
    ) {
        touch(ctx, what, "Search Materials");
        press(ctx, what, "Close this dialog and copy nothing.");
    }
    copy.set_open(false);
    pump(200);
    quiet(ctx, "Copy to Materials");
    drop_captures();
    ctx.check_health("Copy to Materials dialog");
}

// ---------------------------------------------------------------------------------------------
// Rough Planner
// ---------------------------------------------------------------------------------------------

/// The Rough Planner's inclusion section on a block rough with the kind "Mesh File": the unit box
/// of the inclusion's file shows only for that kind.
fn planner_unit(ctx: &mut Ctx<'_>) {
    let what = "planner inclusion unit";
    if !launch(ctx, Base::Startup, what) {
        return;
    }
    let opened = overlays::open_planner(ctx.app);
    let window = harness_support::rough_planner_window();
    let Some(window) = window.filter(|_| opened) else {
        ctx.expect(
            false,
            "coverage-reach",
            "planner inclusion unit: the Rough Planner window did not open",
        );
        return;
    };
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.invoke_base_field_committed();
    pump(300);
    model.set_inclusion_kind_index(0);
    overlays::grow(SecondaryWindow::RoughPlanner);
    pump(300);
    let label = "Unit of the Numbers in the Inclusion's File";
    let planner = || WindowView::of("RoughPlannerWindow", &window);
    let showing = |view: &WindowView| view.controls().iter().any(|c| c.label == label);
    if !showing(&planner()) {
        // The box is part of the form that "Add" opens in the Inclusions section.
        let opened = activate(&planner(), |control| control.label == "Add an Inclusion");
        ctx.expect(
            opened,
            "coverage-reach",
            "planner inclusion unit: no enabled button labelled \"Add an Inclusion\" is showing",
        );
        pump(300);
        model.set_inclusion_kind_index(0);
        pump(200);
    }
    let view = planner();
    let pressed = activate(&view, |control| control.label == label);
    ctx.expect(
        pressed,
        "coverage-reach",
        format!("planner inclusion unit: no enabled box labelled {label:?} is showing"),
    );
    quiet(ctx, label);
    ctx.check_health("Rough Planner, inclusion file unit");
}
