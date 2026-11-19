//! Scenarios: the overlays and states of the render job screens and the cutter-feedback screens.
//!
//! Run from the editor and library overlay scenarios (`overlays.rs`), so each one starts from a
//! design in the editor or the sample library:
//!
//! - the RI chart, the Edit Render Job dialog and Copy to Materials open by their normal route
//!   over queued jobs, sweep clean at both sizes and close with Esc and with Cancel or Close;
//! - the Render Jobs window with its rows, the row menu, the Compute menu, pick mode and a
//!   paused row, the Load Job View banner, the "Render this job again?" question, the "Computing
//!   the RI range" wait, the Stone tab's two RI chips and Preferences' "Figures" section;
//! - the Compute RI Ranges dialog opens and closes. Its Compute button traces every design of
//!   the library, so no pass presses it: the dialog closes with Not Now or Cancel only.
//!
//! The Tilt Average Range is set to ±30° for one sweep and always back to ±90° (index 5).

use super::{
    click_center,
    overlays::{Overlay, click_backdrop, run_overlay, run_overlays, scroll_dialog_until_visible},
};
use crate::support::{
    Ctx,
    app::{pump, wait_until},
};
use pleochroma_cut::{
    JobCopyModel, JobEditModel, MainWindow, ParametersModel, PreferencesModel, RenderJobsModel,
    RiBatchModel, RiChartModel, RiFieldModel, RiRangeModel, TiltWindows, harness_support,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// The index of "±30°" among the Tilt Average Range choices.
const RANGE_30: i32 = 2;
/// The index of "±90° (all tilts)", the default.
const RANGE_90: i32 = 5;

/// The id of the first queued job.
fn first_job(ui: &MainWindow) -> Option<i32> {
    ui.global::<RenderJobsModel>()
        .get_rows()
        .row_data(0)
        .map(|row| row.id)
}

/// The RI chart: over a design in the editor.
fn ri_chart_overlays() -> Vec<Overlay> {
    vec![Overlay {
        name: "RI chart",
        open: |ui| ui.global::<RiChartModel>().invoke_open_chart(),
        is_open: |ui| ui.global::<RiChartModel>().get_open(),
        force_close: |ui| ui.global::<RiChartModel>().set_open(false),
        close_buttons: &["Close"],
    }]
}

/// The dialogs over a queued job: Edit Render Job and Copy to Materials.
fn job_dialog_overlays() -> Vec<Overlay> {
    vec![
        Overlay {
            name: "Edit Render Job",
            open: |ui| {
                if let Some(id) = first_job(ui) {
                    ui.global::<RenderJobsModel>().invoke_edit_job(id);
                }
            },
            is_open: |ui| ui.global::<JobEditModel>().get_open(),
            force_close: |ui| ui.global::<JobEditModel>().set_open(false),
            close_buttons: &["Cancel"],
        },
        Overlay {
            name: "Copy to Materials",
            open: |ui| {
                if let Some(id) = first_job(ui) {
                    ui.global::<RenderJobsModel>().invoke_open_copy_dialog(id);
                }
            },
            is_open: |ui| ui.global::<JobCopyModel>().get_open(),
            force_close: |ui| ui.global::<JobCopyModel>().set_open(false),
            // Not "Copy": it would queue the copies.
            close_buttons: &["Cancel"],
        },
    ]
}

/// The Compute RI Ranges dialog at its confirm step. Never "Compute" (it traces every design).
fn ri_batch_overlays() -> Vec<Overlay> {
    vec![Overlay {
        name: "Compute RI Ranges",
        open: |ui| ui.global::<RiBatchModel>().invoke_open_batch(),
        is_open: |ui| ui.global::<RiBatchModel>().get_visible(),
        force_close: |ui| ui.global::<RiBatchModel>().set_visible(false),
        close_buttons: &["Not Now", "Cancel"],
    }]
}

/// Queues two still jobs (nothing starts them) and leaves the Render Jobs window closed. `true`
/// when both rows exist; otherwise a finding.
fn queue_jobs(ctx: &mut Ctx<'_>) -> bool {
    let ui = ctx.app.ui().clone_strong();
    let jobs = ui.global::<RenderJobsModel>();
    if let Ok(dir) = std::env::current_dir().map(|d| d.join("harness_queue_exports")) {
        let _ = std::fs::create_dir_all(&dir);
        harness_support::queue_picker_path(dir.clone());
        harness_support::queue_picker_path(dir);
    }
    jobs.invoke_add_still_jobs(1920, 1080, 64, 0, 0, 2);
    let _ = wait_until(Duration::from_secs(30), || jobs.get_rows().row_count() >= 1);
    jobs.invoke_add_still_jobs(1280, 720, 32, 0, 0, 2);
    let _ = wait_until(Duration::from_secs(30), || jobs.get_rows().row_count() >= 2);
    harness_support::clear_picker_answers();
    jobs.set_panel_open(false);
    pump(250);
    ctx.expect(
        jobs.get_rows().row_count() >= 2,
        "jobs-queued",
        "two still jobs were not queued (setup of the job overlays failed)",
    )
}

/// With a design in the editor: the RI chart, then (over two queued jobs) the Edit Render Job
/// dialog, Copy to Materials, and the states of the Render Jobs window, the banner and the RI
/// field and chips.
pub(super) fn run_ri_chart_and_job_dialogs(ctx: &mut Ctx<'_>) {
    run_overlays(ctx, &ri_chart_overlays());
    if !queue_jobs(ctx) {
        return;
    }
    ctx.check_health("queued jobs for the job overlays");
    run_overlays(ctx, &job_dialog_overlays());
    jobs_window_states(ctx);
    job_view_banner(ctx);
    question_states(ctx);
    ri_chip_states(ctx);
    preferences_figures(ctx);
}

/// With the sample library behind: the Compute RI Ranges dialog.
pub(super) fn run_ri_batch(ctx: &mut Ctx<'_>) {
    for overlay in ri_batch_overlays() {
        run_overlay(ctx, &overlay);
    }
}

/// Presses the first enabled button labelled `label` with the pointer; whether there was one.
fn press_labelled(ctx: &Ctx<'_>, label: &str) -> bool {
    let ui = ctx.app.ui().clone_strong();
    let view = ctx.app.main_view();
    let found = view.controls().into_iter().find(|control| {
        control.role == AccessibleRole::Button && control.enabled && control.label == label
    });
    found.is_some_and(|control| {
        click_center(&ui, &control.handle);
        true
    })
}

/// The Render Jobs window with its rows, the row menu, the Compute menu, pick mode and a paused
/// row; each swept at both sizes.
fn jobs_window_states(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let jobs = ui.global::<RenderJobsModel>();
    jobs.set_panel_open(true);
    pump(500);
    ctx.sweep_main_at_sizes("Render Jobs with queued rows");

    // The row menu (a popup; its items are not in the element tree, the rest of the window is).
    let opened = press_labelled(ctx, "More actions");
    ctx.expect(
        opened,
        "job-row-menu",
        "a queued row has no enabled More actions button",
    );
    pump(400);
    ctx.sweep_main_at_sizes("Render Jobs, row menu open");
    click_backdrop(ctx.app);

    // The Compute menu of the header.
    let opened = press_labelled(ctx, "Compute");
    ctx.expect(
        opened,
        "job-compute-menu",
        "the Render Jobs header has no enabled Compute button",
    );
    pump(400);
    ctx.sweep_main_at_sizes("Render Jobs, Compute menu open");
    click_backdrop(ctx.app);

    // A paused first row: Resume instead of Pause.
    if let Some(id) = first_job(&ui) {
        jobs.invoke_pause_job(id);
        pump(600);
        ctx.sweep_main_at_sizes("Render Jobs, paused row");
    }

    // Pick mode of Load Job View: Esc ends it.
    jobs.set_pick_view_mode(true);
    pump(400);
    ctx.sweep_main_at_sizes("Render Jobs, pick mode");
    ctx.press("Escape");
    pump(300);
    let ended = !jobs.get_pick_view_mode();
    ctx.expect(
        ended,
        "pick-mode-escape",
        "Esc did not end the pick mode of Load Job View",
    );
    jobs.set_pick_view_mode(false);
    jobs.set_panel_open(false);
    pump(250);
    ctx.check_health("Render Jobs window states");
}

/// The Load Job View banner on the live view: a title, a subtitle, a note and Update switched
/// off with its reason. Only the model is filled; no job view is loaded and nothing is pressed.
fn job_view_banner(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.set_active_tab(0);
    ui.invoke_active_tab_changed(0);
    ui.set_render_view_tab(0);
    ui.invoke_render_view_tab_changed(0);
    pump(500);
    let jobs = ui.global::<RenderJobsModel>();
    jobs.set_job_view_title("Showing the view of job 1".into());
    jobs.set_job_view_subtitle("Round Brilliant · Sapphire".into());
    let notes = vec![SharedString::from(
        "The HDR map of this job is missing (studio_softbox.hdr). The view uses your current map.",
    )];
    jobs.set_job_view_notes(ModelRc::new(VecModel::from(notes)));
    jobs.set_job_view_can_update(false);
    jobs.set_job_view_update_reason("This job is rendering. Pause it first.".into());
    jobs.set_job_view_id(1);
    pump(500);
    ctx.sweep_main_at_sizes("Load Job View banner, Update off");
    jobs.set_job_view_can_update(true);
    jobs.set_job_view_update_reason("".into());
    pump(300);
    ctx.sweep_main_at_sizes("Load Job View banner, Update on");
    jobs.set_job_view_id(-1);
    jobs.set_job_view_notes(ModelRc::default());
    pump(300);
    ctx.check_health("Load Job View banner");
}

/// The two confirm questions of this run: "Render this job again?" and the Cancel-only "Computing
/// the RI range" wait. Each is shown by its flag, swept, and hidden again.
fn question_states(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let edit = ui.global::<JobEditModel>();
    edit.set_requeue_question_open(true);
    pump(300);
    ctx.sweep_main_at_sizes("Render this job again question");
    edit.set_requeue_question_open(false);
    pump(200);

    let field = ui.global::<RiFieldModel>();
    field.set_export_waiting(true);
    pump(300);
    ctx.sweep_main_at_sizes("Computing the RI range wait");
    field.set_export_waiting(false);
    pump(200);
    ctx.check_health("confirm questions of the job and RI screens");
}

/// The Stone tab's RI chip as the hint and as the traced range (the model is filled directly).
fn ri_chip_states(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<ParametersModel>().invoke_show_tab(0);
    pump(500);
    let chip = ui.global::<RiRangeModel>();
    chip.set_chip_text("Pavilion needs RI 1.50 or more (P2)".into());
    chip.set_chip_hover(
        "The shallowest pavilion tier, P2 at 40.50°, needs RI 1.50 or more.".into(),
    );
    chip.set_chip_state(1);
    pump(300);
    ctx.sweep_main_at_sizes("Stone tab, RI hint");
    chip.set_chip_text("RI 1.63 and up (estimate)".into());
    chip.set_chip_state(2);
    pump(300);
    ctx.sweep_main_at_sizes("Stone tab, traced RI chip");
    chip.set_chip_state(0);
    pump(200);
    ctx.check_health("Stone tab RI chip");
}

/// Preferences' "Figures" section: the row exists, ±30° changes the words every figure uses, and
/// the range goes back to ±90° before the pass ends.
fn preferences_figures(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let prefs = ui.global::<PreferencesModel>();
    prefs.set_dialog_open(true);
    pump(300);
    scroll_dialog_until_visible(ctx.app, "PreferencesDialog", "Tilt Average Range");
    let shown = ctx.app.main_view().has("Tilt Average Range");
    ctx.expect(
        shown,
        "preferences-figures",
        "the Preferences Figures section has no Tilt Average Range control",
    );
    ctx.sweep_main_at_sizes("Preferences, Figures section");
    let words = ui.global::<TiltWindows>();
    prefs.invoke_tilt_range_changed(RANGE_30);
    pump(500);
    ctx.expect(
        words.get_displayed().contains("30"),
        "tilt-range-words",
        format!(
            "after choosing ±30° the caption word reads {:?}",
            words.get_displayed().as_str()
        ),
    );
    ctx.sweep_main_at_sizes("Preferences, Figures section at ±30°");
    prefs.invoke_tilt_range_changed(RANGE_90);
    pump(500);
    ctx.expect(
        words.get_displayed().contains("90") && prefs.get_tilt_range_index() == RANGE_90,
        "tilt-range-back",
        "the Tilt Average Range did not return to ±90°",
    );
    prefs.set_dialog_open(false);
    pump(200);
    ctx.check_health("Preferences Figures section");
}
