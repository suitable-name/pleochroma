//! Visible routes of the render jobs lessons (`output-render-jobs`, `output-render-jobs-queue`):
//! the export dialog's Add to Queue, the Render Jobs window, Start Queue, Pause Queue, a paused
//! job's Resume and the script export.
//!
//! The arms that call these routes are in `scripts.rs` (the harness ties them to the catalogue
//! there). Every route is a click on a visible control, and each one fails with a sentence naming
//! the control it could not find, so a renamed label stops the video instead of filming
//! something else.
//!
//! A video must not wait for a full render. The job is queued at the dialog's own size and
//! samples and started for a moment only: the wait for its first progress is a time-lapse
//! ([`pacing::wait_lapse`], cut once the video budget is used), and Pause Queue stops it at the
//! next sample batch, so nothing renders to the end. The job is left queued, never run to
//! completion, and the profile is thrown away with the scenario.

use crate::{director::Film, pacing, scripts::pure_tables::output_path, targets};
use pleochroma_cut::{ExportModel, RenderJobsModel, harness_support};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// How long the first job may take to show its first progress, real time. The wait is a
/// time-lapse of at most a few seconds of video; the rest is cut.
const FIRST_PROGRESS: Duration = Duration::from_secs(45);

/// How long a stopped job may take to reach the Paused state: the render stops at its next sample
/// batch.
const PAUSE_SETTLE: Duration = Duration::from_secs(90);

impl Film<'_> {
    /// The state word of the first row of the Render Jobs window (`"queued"`, `"running"`,
    /// `"paused"`, ...), or `None` while the list is empty.
    fn first_job_state(&self) -> Option<String> {
        self.ui
            .global::<RenderJobsModel>()
            .get_rows()
            .row_data(0)
            .map(|row| row.state_word.to_string())
    }

    /// The control with role Button and label `label`, optionally inside the component `scope`.
    fn jobs_button(&self, label: &str, scope: Option<&str>) -> Result<targets::Spot, String> {
        targets::role_label(&self.view(), label, AccessibleRole::Button, scope)
            .ok_or_else(|| format!("render jobs: the {label:?} button is not visible and enabled"))
    }

    /// Removes every job the profile's library brought along, so the lesson's job is the first
    /// and only row. The queue lives in the library database, and a copied library may hold jobs
    /// of its own (finished ones, or one aimed at a remote coordinator this profile does not
    /// have); the routes below read the first row and Start Queue would run those first.
    /// Called through the model, before the Render Jobs window is ever shown.
    fn jobs_clear_inherited(&self) -> Result<(), String> {
        let jobs = self.ui.global::<RenderJobsModel>();
        jobs.invoke_refresh();
        let ids: Vec<i32> = jobs.get_rows().iter().map(|row| row.id).collect();
        for id in ids {
            jobs.invoke_cancel_job(id);
            jobs.invoke_delete_job(id);
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            jobs.invoke_refresh();
            jobs.get_rows().row_count() == 0
        }) {
            return Err(format!(
                "render jobs: the library's own {} job(s) could not be removed",
                jobs.get_rows().row_count()
            ));
        }
        Ok(())
    }

    /// Switches to Live Render with its pill, unless the view is already there.
    fn jobs_ensure_live_render(&mut self) -> Result<(), String> {
        if self.ui.get_active_tab() == 0 && self.ui.get_render_view_tab() == 0 {
            return Ok(());
        }
        self.film_command("view.live_render")?;
        if self.ui.get_active_tab() != 0 || self.ui.get_render_view_tab() != 0 {
            return Err("render jobs: the Live Render pill did not open Live Render".into());
        }
        Ok(())
    }

    /// Clicks the export dialog's Close button and waits for the dialog to go away.
    fn jobs_close_export_dialog(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ExportModel>();
        if !model.get_is_open() {
            return Ok(());
        }
        let close = self.jobs_button("Close", Some("ExportDialog"))?;
        self.click_and_invoke(&close)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !model.get_is_open()) {
            return Err("render jobs: Close did not close the export dialog".into());
        }
        Ok(())
    }

    /// Step "Open the export dialog": the Export button of the Live Render toolbar.
    pub(super) fn film_render_export_open(&mut self) -> Result<(), String> {
        self.jobs_clear_inherited()?;
        self.jobs_ensure_live_render()?;
        let model = self.ui.global::<ExportModel>();
        if model.get_is_open() {
            return Err("render jobs: the export dialog is already open".into());
        }
        let export = self.jobs_button("Export", None)?;
        self.click_and_invoke(&export)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_is_open()) {
            return Err("render jobs: Export did not open the export dialog".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step "Add it to the queue": Add to Queue in the open export dialog. A new export folder
    /// is answered with a private, relative folder of this guide, in case the profile has none.
    pub(super) fn film_render_job_add(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ExportModel>();
        if !model.get_is_open() {
            return Err("render jobs: Add to Queue needs the export dialog to be open".into());
        }
        let folder = output_path(&self.id, "renders");
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("render jobs: cannot create the renders folder: {error}"))?;
        harness_support::queue_picker_path(folder);
        let jobs = self.ui.global::<RenderJobsModel>();
        let before = jobs.get_rows().row_count();
        let add = self.jobs_button("Add to Queue", Some("ExportDialog"))?;
        self.click_and_invoke(&add)?;
        let added = pacing::wait_real(pacing::STEP_TIMEOUT, || {
            jobs.get_rows().row_count() > before
        });
        // An answer the app did not need (the export folder was already set) must not wait for
        // some later picker.
        harness_support::clear_picker_answers();
        if !added {
            return Err(format!(
                "render jobs: Add to Queue stored no job: {:?}",
                model.get_status_message()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step "Queue a picture" of the queue lesson: Live Render, the export dialog, Add to Queue,
    /// and Close, so the Render Jobs window can be opened next.
    pub(super) fn film_render_job_queue_picture(&mut self) -> Result<(), String> {
        self.film_render_export_open()?;
        self.film_render_job_add()?;
        self.jobs_close_export_dialog()
    }

    /// Step "Open the Render Jobs window": the header's Jobs button, with the export dialog
    /// closed first.
    pub(super) fn film_render_jobs_open(&mut self) -> Result<(), String> {
        self.jobs_close_export_dialog()?;
        let jobs = self.ui.global::<RenderJobsModel>();
        if jobs.get_panel_open() {
            return Err("render jobs: the Render Jobs window is already open".into());
        }
        let targets::Resolved::Found(chip) = targets::resolve(&self.view(), "render_jobs_chip")
        else {
            return Err("render jobs: the Jobs button is not in the header".into());
        };
        self.click_and_invoke(&chip)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            jobs.get_panel_open() && jobs.get_rows().row_count() > 0
        }) {
            return Err("render jobs: the Jobs button did not open a window with a job".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step "Start the queue": Start Queue, then a time-lapse of the job's first progress.
    pub(super) fn film_render_queue_start(&mut self) -> Result<(), String> {
        let jobs = self.ui.global::<RenderJobsModel>();
        let start = self.jobs_button("Start Queue", None)?;
        self.click_and_invoke(&start)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            jobs.get_queue_running() && self.first_job_state().as_deref() == Some("running")
        }) {
            return Err(format!(
                "render jobs: Start Queue did not start the job (queue running={}, first job {:?})",
                jobs.get_queue_running(),
                self.first_job_state()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        // The render may need a while to show its first batch; that wait is cut from the video.
        let model = self.ui.global::<RenderJobsModel>();
        let progressed = pacing::wait_lapse(&mut self.rec, FIRST_PROGRESS, || {
            model
                .get_rows()
                .row_data(0)
                .is_some_and(|row| row.progress > 0.0)
        })?;
        if !progressed {
            self.note_once(
                "render-jobs-first-progress",
                "The job showed no progress before the video paused it; the pause is filmed anyway."
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// Step "Pause the queue": Pause Queue, and a wait until the job reads Paused.
    pub(super) fn film_render_queue_pause(&mut self) -> Result<(), String> {
        let pause = self.jobs_button("Pause Queue", None)?;
        self.click_and_invoke(&pause)?;
        let model = self.ui.global::<RenderJobsModel>();
        let paused = pacing::wait_lapse(&mut self.rec, PAUSE_SETTLE, || {
            !model.get_queue_running()
                && model
                    .get_rows()
                    .row_data(0)
                    .is_some_and(|row| row.state_word.as_str() == "paused")
        })?;
        if !paused {
            return Err(format!(
                "render jobs: Pause Queue did not leave the job Paused (first job {:?})",
                self.first_job_state()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step "Resume the job": the Resume button of the paused row, and a wait until it is Queued.
    pub(super) fn film_render_job_resume(&mut self) -> Result<(), String> {
        let resume = self.jobs_button("Resume", None)?;
        self.click_and_invoke(&resume)?;
        let jobs = self.ui.global::<RenderJobsModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            jobs.get_rows()
                .row_data(0)
                .is_some_and(|row| row.state_word.as_str() == "queued")
        }) {
            return Err(format!(
                "render jobs: Resume did not put the job back in line (first job {:?})",
                self.first_job_state()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step "Export a script": Export Script..., then Choose Folder and Export with the folder
    /// picker answered by a private, relative folder of this guide. The new `render-jobs-` folder
    /// must exist afterwards.
    pub(super) fn film_render_script_export(&mut self) -> Result<(), String> {
        let jobs = self.ui.global::<RenderJobsModel>();
        let open = self.jobs_button("Export Script...", None)?;
        self.click_and_invoke(&open)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || jobs.get_script_open()) {
            return Err("render jobs: Export Script... did not open its section".into());
        }
        self.hold(pacing::RESULT_HOLD)?;

        let folder = output_path(&self.id, "scripts");
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("render jobs: cannot create the scripts folder: {error}"))?;
        harness_support::queue_picker_path(folder.clone());
        let export = self.jobs_button("Choose Folder and Export", None)?;
        self.click_and_invoke(&export)?;
        let written = pacing::wait_real(pacing::STEP_TIMEOUT, || !jobs.get_script_open());
        harness_support::clear_picker_answers();
        if !written {
            return Err("render jobs: Choose Folder and Export did not finish".into());
        }
        let made = std::fs::read_dir(&folder)
            .map_err(|error| format!("render jobs: cannot read the scripts folder: {error}"))?
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("render-jobs-")
            });
        if !made {
            return Err("render jobs: no render-jobs- folder was written".into());
        }
        // The privacy scan of the main window sees the toast and the list.
        for text in crate::routes::visible_texts(&self.view()) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in the Render Jobs window ({word:?}): {text:?}"
                ));
            }
        }
        self.hold(pacing::RESULT_HOLD)
    }
}
