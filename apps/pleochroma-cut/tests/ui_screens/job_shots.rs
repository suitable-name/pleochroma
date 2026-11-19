//! The shots of the render job screens: the Render Jobs window at 640 x 480, the Edit Render Job
//! dialog (a still and a tilt video), Copy to Materials, the Load Job View banner and the four
//! 640 x 480 states of the row menu and the Compute menu.
//!
//! Every job is queued through the model call the export dialog's "Add to Queue" makes, so the
//! rows, the menus and the dialogs are the app's own; the harness only presses what a user
//! presses. Each shot function takes `small`: the `_640` shots shrink the window after the jobs
//! exist and before the window or the dialog opens.

use crate::{
    dialog_shots::{card_shot, wait_component},
    kit::{Rect, Sc, clickable, find_inherits, press_on, rect_of, sized_shots},
    library_shots::open_tilt,
    nav::FVS,
    support::app::pump,
};
use pleochroma_cut::{
    ExportModel, JobCopyModel, JobEditModel, RenderJobsModel, TiltModel, harness_support,
};
use slint::{ComponentHandle, Model};
use std::path::PathBuf;
use uiprobe::screenshot::Shot;

/// The folder of the profile's working directory the queue's folder dialog is answered with (it
/// is not shown).
fn queue_folder() -> Result<PathBuf, String> {
    let dir = std::env::current_dir()
        .map(|d| d.join("screens_queue"))
        .map_err(|error| format!("no working directory: {error}"))?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("no queue folder: {error}"))?;
    Ok(dir)
}

/// Queues two still jobs (1920 x 1080 and 1280 x 720); with `ray_second` the second one is a
/// Ray-Traced job. The design is open in the editor.
fn queue_stills(sc: &Sc<'_, '_>, ray_second: bool) -> Result<(), String> {
    let ui = sc.ui();
    let jobs = ui.global::<RenderJobsModel>();
    let export = ui.global::<ExportModel>();
    let dir = queue_folder()?;
    // The first answer is stored as the export folder; a second is queued in case it is not.
    harness_support::queue_picker_path(dir.clone());
    harness_support::queue_picker_path(dir);
    jobs.invoke_add_still_jobs(1920, 1080, 64, 0, 0, 2);
    let first = Sc::wait(30, || jobs.get_rows().row_count() >= 1);
    if ray_second {
        export.set_engine_index(1);
    }
    jobs.invoke_add_still_jobs(1280, 720, 32, 0, 0, 2);
    let second = Sc::wait(30, || jobs.get_rows().row_count() >= 2);
    export.set_engine_index(0);
    harness_support::clear_picker_answers();
    if first && second {
        Ok(())
    } else {
        Err("the render jobs were not queued".to_owned())
    }
}

/// The id of the job in row `row`.
fn job_id(sc: &Sc<'_, '_>, row: usize) -> Result<i32, String> {
    sc.ui()
        .global::<RenderJobsModel>()
        .get_rows()
        .row_data(row)
        .map(|job| job.id)
        .ok_or_else(|| format!("the queue has no row {row}"))
}

/// Opens the Render Jobs window and lets its rows draw.
fn open_panel(sc: &Sc<'_, '_>) {
    sc.ui().global::<RenderJobsModel>().set_panel_open(true);
    pump(1000);
    sc.ctx.app.clear_notice();
}

/// Presses the "More actions" button of row `row` with the pointer, which opens its menu.
fn open_row_menu(sc: &Sc<'_, '_>, row: usize) -> Result<(), String> {
    let buttons: Vec<_> = sc
        .view()
        .by_label("More actions")
        .into_iter()
        .filter(|element| clickable(element))
        .collect();
    let button = buttons
        .get(row)
        .ok_or_else(|| format!("row {row} has no More actions button"))?;
    press_on(sc.ui().window(), button);
    pump(600);
    Ok(())
}

/// `render_jobs.png` and `render_jobs_640.png`: the Render Jobs window with two queued pictures.
fn render_jobs_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    if small {
        sc.small();
    }
    open_panel(sc);
    let shot = sc.snap_main()?;
    Ok(card_rect_of(sc, "RenderJobsPanel")?.crop(&shot))
}

/// The card of the dialog `component`.
fn card_rect_of(sc: &Sc<'_, '_>, component: &str) -> Result<Rect, String> {
    crate::kit::card_rect(&sc.view(), component)
}

sized_shots!(
    "The Render Jobs window with two queued pictures.",
    render_jobs,
    render_jobs_640,
    render_jobs_at
);

/// The Edit Render Job dialog of the first job.
fn edit_dialog(sc: &Sc<'_, '_>, id: i32) -> Result<(), String> {
    let ui = sc.ui();
    let edit = ui.global::<JobEditModel>();
    ui.global::<RenderJobsModel>().invoke_edit_job(id);
    if !Sc::wait(30, || edit.get_open()) {
        return Err("the Edit Render Job dialog did not open".to_owned());
    }
    wait_component(sc, "JobEditDialog")
}

/// `render_job_edit.png`: the Edit Render Job dialog of a queued Path-Traced picture.
fn render_job_edit_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    if small {
        sc.small();
    }
    edit_dialog(sc, job_id(sc, 0)?)?;
    card_shot(sc, "JobEditDialog")
}

sized_shots!(
    "The Edit Render Job dialog of a queued picture.",
    render_job_edit,
    render_job_edit_640,
    render_job_edit_at
);

/// Queues a tilt video through the Tilt Performance dialog's "Add to Queue" route. `Ok(true)`
/// when a video row exists afterwards.
fn queue_video(sc: &Sc<'_, '_>) -> Result<bool, String> {
    open_tilt(sc)?;
    let ui = sc.ui();
    let jobs = ui.global::<RenderJobsModel>();
    let dir = queue_folder()?;
    harness_support::queue_picker_path(dir.clone());
    harness_support::queue_picker_path(dir);
    jobs.invoke_add_tilt_video_job(0);
    let queued = Sc::wait(90, || jobs.get_rows().iter().any(|row| !row.is_still));
    harness_support::clear_picker_answers();
    let tilt = ui.global::<TiltModel>();
    if tilt.get_dialog_open() {
        tilt.invoke_toggle_dialog();
    }
    pump(500);
    Ok(queued)
}

/// `render_job_edit_video.png`: the Edit Render Job dialog of a tilt video job.
///
/// The video is queued the way a user queues it. When the queue does not take it, the dialog of
/// a picture is switched to its video form and filled with a typical video; the shot says so.
fn render_job_edit_video_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    let real = queue_video(sc)?;
    if !real {
        sc.note("render_job_edit_video: no video job was queued; the dialog of a picture is shown in its video form");
        queue_stills(sc, false)?;
    }
    if small {
        sc.small();
    }
    let jobs = sc.ui().global::<RenderJobsModel>().get_rows();
    let row = jobs.iter().position(|row| !row.is_still).unwrap_or(0);
    edit_dialog(sc, job_id(sc, row)?)?;
    if !real {
        let ui = sc.ui();
        let edit = ui.global::<JobEditModel>();
        edit.set_is_video(true);
        edit.set_video_name("Round Brilliant tilt".into());
        edit.set_start_deg(-90);
        edit.set_end_deg(90);
        edit.set_step_deg(1.0);
        edit.set_axis_0(true);
        edit.set_axis_2(true);
        edit.set_overlay_brilliance(true);
        edit.set_overlay_windowing(true);
        pump(600);
    }
    card_shot(sc, "JobEditDialog")
}

sized_shots!(
    "The Edit Render Job dialog of a tilt video job.",
    render_job_edit_video,
    render_job_edit_video_640,
    render_job_edit_video_at
);

/// `render_job_copy_materials.png`: Copy to Materials with two other materials ticked.
fn render_job_copy_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    if small {
        sc.small();
    }
    let ui = sc.ui();
    let copy = ui.global::<JobCopyModel>();
    ui.global::<RenderJobsModel>()
        .invoke_open_copy_dialog(job_id(sc, 0)?);
    if !Sc::wait(30, || copy.get_open()) {
        return Err("the Copy to Materials dialog did not open".to_owned());
    }
    wait_component(sc, "CopyMaterialsDialog")?;
    // Two materials other than the job's own: the dialog's ticks are owned by Rust.
    let source = copy.get_source_material();
    let names: Vec<String> = copy
        .get_choice_names()
        .iter()
        .map(String::from)
        .filter(|name| name.as_str() != source.as_str())
        .take(2)
        .collect();
    for name in names {
        copy.invoke_toggle(name.as_str().into());
        pump(300);
    }
    pump(600);
    card_shot(sc, "CopyMaterialsDialog")
}

sized_shots!(
    "Copy to Materials with two materials ticked.",
    render_job_copy_materials,
    render_job_copy_materials_640,
    render_job_copy_at
);

/// `render_job_view_banner.png`: the live view after Load Job View, with the banner.
fn render_job_view_banner_at(sc: &mut Sc<'_, '_>, small: bool) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    if small {
        sc.small();
    }
    let ui = sc.ui();
    let jobs = ui.global::<RenderJobsModel>();
    jobs.invoke_load_job_view(job_id(sc, 0)?);
    if !Sc::wait(90, || jobs.get_job_view_id() >= 0) {
        return Err("Load Job View did not show the job's view".to_owned());
    }
    pump(2500);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let area = find_inherits(&view, "GemViewportView")
        .map(|element| Rect::of(&element))
        .ok_or("the live view is not on screen")?;
    Ok(area.crop(&shot))
}

sized_shots!(
    "The live view after Load Job View, with its banner.",
    render_job_view_banner,
    render_job_view_banner_640,
    render_job_view_banner_at
);

/// Pauses the job in row `row` and lets the row redraw.
fn pause_row(sc: &Sc<'_, '_>, row: usize) -> Result<(), String> {
    let id = job_id(sc, row)?;
    sc.ui().global::<RenderJobsModel>().invoke_pause_job(id);
    pump(800);
    Ok(())
}

/// Sheet 4, 11a: the menu of row 2 at 640 x 480 (it opens upward).
///
/// # Errors
///
/// The jobs are not queued, or the menu does not open.
pub fn render_jobs_640_row_menu(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    sc.small();
    open_panel(sc);
    open_row_menu(sc, 1)?;
    sc.snap_main()
}

/// Sheet 4, 11b: the Compute menu of the header at 640 x 480.
///
/// # Errors
///
/// The jobs are not queued, or the menu does not open.
pub fn render_jobs_640_compute_menu(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    sc.small();
    open_panel(sc);
    let view = sc.view();
    let button = view
        .by_label("Compute")
        .into_iter()
        .find(|element| clickable(element))
        .ok_or("the Render Jobs header has no Compute button")?;
    press_on(sc.ui().window(), &button);
    pump(600);
    sc.snap_main()
}

/// Sheet 4, 11c: the longest menu, on the first row of a paused job, at 640 x 480.
///
/// # Errors
///
/// The jobs are not queued, or the menu does not open.
pub fn render_jobs_640_menu_first_row(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, false)?;
    sc.small();
    open_panel(sc);
    pause_row(sc, 0)?;
    open_row_menu(sc, 0)?;
    sc.snap_main()
}

/// Sheet 4, 11d: a paused row and a Ray-Traced row at 640 x 480.
///
/// # Errors
///
/// The jobs are not queued.
pub fn render_jobs_640_paused_ray_rows(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    queue_stills(sc, true)?;
    sc.small();
    open_panel(sc);
    pause_row(sc, 0)?;
    let shot = sc.snap_main()?;
    Ok(rect_of(&sc.view(), "RenderJobsPanel")?.crop(&shot))
}
