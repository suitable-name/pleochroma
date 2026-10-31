//! Edit > Angle Sweep...: one tier's angle over a range, every angle solved and scored, as
//! a table, a chart and CSV (`ui/components/sweep_dialog.slint`, state in
//! `ui/models/sweep.slint`).
//!
//! The sweep itself is `pleochroma_editor::sweep` (shared with the web app); this group is
//! the desktop's glue around it:
//!
//! - [`form`] -- which tiers the combo offers, a tier's starting range, and what the three
//!   number fields add up to. Slint-free.
//! - [`state`] -- what the dialog remembers between callbacks (rows, selection, lines).
//!   Slint-free.
//! - [`run`] -- the worker thread and the handle the UI thread polls.
//! - [`present`] -- the table rows, chart lines and sentences, and the pushes into
//!   `SweepModel`.
//! - this file -- the [`Controller`] that answers the model's callbacks.
//!
//! The dialog's Two Tiers mode (two tiers' angles over a grid, shown as a heat map) is the
//! same arrangement in its own files: [`state2d`] (state and form), [`run2d`] (the worker
//! thread), [`heatmap`] (the pixels), [`present2d`] (the texts and the pushes) and
//! [`control2d`] (the [`Controller`]'s answers).
//!
//! A sweep solves every angle on a clone of the design, so the design is never touched while
//! it runs; "Use this angle" is the one edit, a single undo step through
//! `pleochroma_editor::sweep::apply_sweep_angle` (the tiers whose angles follow a relation
//! follow in the same step). It is refused once the design has changed since the sweep
//! started, because the rows no longer describe it.

mod control2d;
mod form;
mod heatmap;
mod present;
mod present2d;
mod run;
mod run2d;
mod state;
mod state2d;

use self::{
    form::{Checked, Fields},
    run::{Job, Poll, RunHandle},
    state::Dialog,
    state2d::TwoTier,
};
use super::{relation_ui::refresh_with_followers, state::EditorState};
use crate::{
    EditorModel, MainWindow, SweepModel,
    bridge::render_thread::RenderContext,
    gui::{
        library::clipboard::copy_to_clipboard_with_toast,
        pickers::{PickerFilter, PickerKind, PickerRequest, pick},
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};
use pleochroma::optics::LightingPreset;
use pleochroma_cut_core::CANONICAL_LIGHTING_PRESET;
use pleochroma_editor::{
    optimize_view::default_optimize_material_ri,
    sweep::{
        SweepMetric, SweepOptions, SweepOutcome, SweepPlan, apply_sweep_angle,
        default_worker_count, sweep_csv,
    },
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

/// How often the UI thread looks at a running sweep.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the dialog says when the design has no tier a sweep can vary.
const NOTHING_TO_SWEEP: &str = "No tier of this design has an angle a sweep can vary. A table, \
    a culet, a girdle and a tier whose angle follows a relation cannot be swept.";

/// What "Use this angle" says once the design has moved on.
const DESIGN_MOVED_ON: &str = "The design has changed since this sweep ran, so its rows no \
    longer describe it. Run the sweep again.";

/// Answers `SweepModel`'s callbacks.
struct Controller {
    ui: Weak<MainWindow>,
    editor: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    dialog: RefCell<Dialog>,
    /// Polls a running sweep; stopped whenever none runs.
    timer: Timer,
    /// How many threads a sweep uses (the machine's, less one).
    workers: usize,
    /// The Two Tiers mode's state (see `control2d.rs`).
    two: RefCell<TwoTier>,
    /// Polls a running two-tier sweep; stopped whenever none runs.
    timer_2d: Timer,
}

thread_local! {
    /// The dialog's controller, for [`open_two_tier_prefilled`] (the Tune view's Pair With).
    static CONTROLLER: RefCell<Option<std::rc::Weak<Controller>>> = const { RefCell::new(None) };
}

/// Pair With (the Optimize tab's Tune view): opens the dialog in Two Tiers mode with `tier_a`
/// across and `tier_b` down, each over its own default range (two degrees either side of its
/// angle). Does nothing when a sweep is running, or when either tier is not one a sweep can
/// take.
pub(in crate::gui::editor) fn open_two_tier_prefilled(
    ui: &MainWindow,
    tier_a: usize,
    tier_b: usize,
) {
    let Some(controller) =
        CONTROLLER.with(|slot| slot.borrow().as_ref().and_then(std::rc::Weak::upgrade))
    else {
        return;
    };
    let model = ui.global::<SweepModel>();
    if model.get_running() {
        return;
    }
    if !model.get_dialog_open() {
        controller.open();
    }
    if !model.get_dialog_open() {
        return;
    }
    let (across, down) = {
        let dialog = controller.dialog.borrow();
        (
            dialog.tiers.iter().position(|&tier| tier == tier_a),
            dialog.tiers.iter().position(|&tier| tier == tier_b),
        )
    };
    let (Some(across), Some(down)) = (across, down) else {
        return;
    };
    if model.get_mode() != 1 {
        model.set_mode(1);
        controller.mode_changed();
    }
    model.set_tier_index(i32::try_from(across).unwrap_or(0));
    model.set_tier_b_index(i32::try_from(down).unwrap_or(0));
    controller.tier_a_changed();
    controller.tier_b_changed();
}

/// Registers `SweepModel`'s callbacks: the dialog's open and close, the form, Run and
/// Cancel, the table and chart, "Use this angle" and the two CSV buttons, and the Two Tiers
/// mode's own (the mode switch, its form, the heat map and its actions).
pub(super) fn setup_sweep_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let controller = Rc::new(Controller {
        ui: ui.as_weak(),
        editor: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        dialog: RefCell::new(Dialog::new()),
        timer: Timer::default(),
        workers: default_worker_count(),
        two: RefCell::new(TwoTier::new()),
        timer_2d: Timer::default(),
    });
    let model = ui.global::<SweepModel>();
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(Rc::downgrade(&controller)));

    let c = Rc::clone(&controller);
    model.on_open_dialog(move || c.open());
    let c = Rc::clone(&controller);
    model.on_close_dialog(move || c.close());
    let c = Rc::clone(&controller);
    model.on_form_changed(move || c.form_changed());
    let c = Rc::clone(&controller);
    model.on_tier_changed(move || c.tier_changed());
    let c = Rc::clone(&controller);
    model.on_run(move || c.run());
    let c = Rc::clone(&controller);
    model.on_cancel(move || c.cancel());
    let c = Rc::clone(&controller);
    model.on_select_row(move |row| c.select_row(row));
    let c = Rc::clone(&controller);
    model.on_toggle_series(move |series| c.toggle_series(series));
    let c = Rc::clone(&controller);
    model.on_chart_hover(move |fraction| c.chart_hover(fraction));
    let c = Rc::clone(&controller);
    model.on_use_selected(move || c.use_selected());
    let c = Rc::clone(&controller);
    model.on_copy_csv(move || c.copy_csv());
    let c = Rc::clone(&controller);
    model.on_save_csv(move || c.save_csv());
    setup_two_tier_callbacks(&model, &controller);
}

/// Registers the callbacks of the Two Tiers mode.
fn setup_two_tier_callbacks(model: &SweepModel<'_>, controller: &Rc<Controller>) {
    let c = Rc::clone(controller);
    model.on_mode_changed(move || c.mode_changed());
    let c = Rc::clone(controller);
    model.on_form_changed_2d(move || c.form_changed_2d());
    let c = Rc::clone(controller);
    model.on_tier_a_changed(move || c.tier_a_changed());
    let c = Rc::clone(controller);
    model.on_tier_b_changed(move || c.tier_b_changed());
    let c = Rc::clone(controller);
    model.on_run_2d(move || c.run_2d());
    let c = Rc::clone(controller);
    model.on_confirm_tilt(move |yes| c.confirm_tilt(yes));
    let c = Rc::clone(controller);
    model.on_heat_pointer(move |x, y, kind| c.heat_pointer(x, y, kind));
    let c = Rc::clone(controller);
    model.on_heat_key(move |key| c.heat_key(key));
    let c = Rc::clone(controller);
    model.on_heat_figure_changed(move || c.heat_figure_changed());
    let c = Rc::clone(controller);
    model.on_contour_changed(move || c.contour_changed());
    let c = Rc::clone(controller);
    model.on_use_cells(move || c.use_cells());
    let c = Rc::clone(controller);
    model.on_refine(move || c.refine());
    let c = Rc::clone(controller);
    model.on_copy_csv_2d(move || c.copy_csv_2d());
    let c = Rc::clone(controller);
    model.on_save_csv_2d(move || c.save_csv_2d());
}

/// Shows what the form comes to under it: the summary line, or why it cannot run.
fn push_check(ui: &MainWindow, checked: &Checked) {
    let model = ui.global::<SweepModel>();
    match checked {
        Checked::Ready { summary, .. } => {
            model.set_summary_text(summary.as_str().into());
            model.set_form_ok(true);
        }
        Checked::Refused(message) => {
            model.set_summary_text(message.as_str().into());
            model.set_form_ok(false);
        }
    }
}

/// Puts a tier's range into the three number fields.
fn push_fields(ui: &MainWindow, fields: &Fields) {
    let model = ui.global::<SweepModel>();
    model.set_from_text(fields.from.as_str().into());
    model.set_to_text(fields.to.as_str().into());
    model.set_step_text(fields.step.as_str().into());
}

impl Controller {
    /// Edit > Angle Sweep...: fills the form for the selected tier and shows the dialog.
    fn open(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let model = ui.global::<SweepModel>();
        if model.get_dialog_open() {
            return;
        }
        let selected = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok();
        let tilt_average = model.get_tilt_average();
        let opening = {
            let st = self.editor.borrow();
            form::open(&st.design, selected, tilt_average, self.workers)
        };
        let Some(opening) = opening else {
            show_toast(&ui, NOTHING_TO_SWEEP, "info");
            return;
        };
        {
            let mut dialog = self.dialog.borrow_mut();
            dialog.tiers = opening.choices.iter().map(|choice| choice.tier).collect();
            dialog.run = None;
            dialog.clear_results();
        }
        let labels = opening.choices.into_iter().map(|choice| choice.label);
        model.set_tier_names(present::tier_names(labels.collect()));
        model.set_tier_index(i32::try_from(opening.chosen).unwrap_or(0));
        push_fields(&ui, &opening.fields);
        model.set_notice_text(opening.notice.into());
        push_check(&ui, &opening.checked);
        model.set_running(false);
        model.set_progress(0.0);
        model.set_status_text("".into());
        present::push_results(&ui, &self.dialog.borrow());
        self.reset_two_tier(&ui);
        model.set_dialog_open(true);
    }

    /// Closes the dialog; a sweep still running is stopped and its rows are dropped.
    fn close(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        self.timer.stop();
        {
            // Dropping the run handle stops the sweep.
            let mut dialog = self.dialog.borrow_mut();
            dialog.run = None;
            dialog.clear_results();
        }
        present::push_results(&ui, &self.dialog.borrow());
        self.reset_two_tier(&ui);
        let model = ui.global::<SweepModel>();
        model.set_running(false);
        model.set_dialog_open(false);
    }

    /// The tier the combo has chosen and the form's three fields and tilt choice.
    fn read_form(&self, ui: &MainWindow) -> Option<(usize, Fields, bool)> {
        let model = ui.global::<SweepModel>();
        let index = usize::try_from(model.get_tier_index()).ok()?;
        let tier = *self.dialog.borrow().tiers.get(index)?;
        let fields = Fields {
            from: model.get_from_text().to_string(),
            to: model.get_to_text().to_string(),
            step: model.get_step_text().to_string(),
        };
        Some((tier, fields, model.get_tilt_average()))
    }

    /// A number field or the tilt option changed: say what the form comes to.
    fn form_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((tier, fields, tilt_average)) = self.read_form(&ui) else {
            return;
        };
        let checked = {
            let st = self.editor.borrow();
            form::check(&st.design, tier, &fields, tilt_average, self.workers)
        };
        push_check(&ui, &checked);
    }

    /// Another tier was chosen: its own range goes into the fields.
    fn tier_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((tier, _, tilt_average)) = self.read_form(&ui) else {
            return;
        };
        let range = {
            let st = self.editor.borrow();
            form::retier(&st.design, tier, tilt_average, self.workers)
        };
        if let Some((fields, checked)) = range {
            push_fields(&ui, &fields);
            push_check(&ui, &checked);
        }
    }

    /// Everything a sweep of `plan` reads, taken from the design now.
    fn job(
        &self,
        st: &EditorState,
        plan: SweepPlan,
        tilt_average: bool,
        lighting: LightingPreset,
    ) -> Job {
        let custom_materials = self
            .render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_materials
            .as_ref()
            .clone();
        // A design that names no material is scored at its own refractive index, not as
        // diamond -- the same defaulting Optimize does.
        let mut material = st.design.material.clone();
        let _ = default_optimize_material_ri(&st.design, &mut material, &custom_materials);
        Job {
            design: st.design.clone(),
            plan,
            material,
            custom_materials,
            lighting,
            options: SweepOptions {
                tilt_average,
                workers: self.workers,
            },
        }
    }

    /// Run: starts the sweep on a worker thread and the timer that watches it.
    fn run(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.dialog.borrow().run.is_some() {
            return;
        }
        let Some((tier, fields, tilt_average)) = self.read_form(&ui) else {
            return;
        };
        // Rated under the grading tray like Optimize and Retarget, whatever lighting the
        // viewport shows, so a sweep row and an Optimize candidate compare directly.
        let lighting = CANONICAL_LIGHTING_PRESET;
        let prepared = {
            let st = self.editor.borrow();
            match form::check(&st.design, tier, &fields, tilt_average, self.workers) {
                Checked::Ready { plan, .. } => Ok((
                    self.job(&st, plan, tilt_average, lighting),
                    st.current_generation(),
                )),
                Checked::Refused(message) => Err(message),
            }
        };
        match prepared {
            Ok((job, generation)) => self.begin(&ui, job, generation),
            Err(message) => show_toast(&ui, &message, "error"),
        }
    }

    /// Starts `job` and shows the dialog as running.
    fn begin(self: &Rc<Self>, ui: &MainWindow, job: Job, generation: u64) {
        let total = job.plan.angles.len();
        let handle = match run::start(job) {
            Ok(handle) => handle,
            Err(message) => {
                show_toast(ui, &message, "error");
                return;
            }
        };
        {
            let mut dialog = self.dialog.borrow_mut();
            dialog.clear_results();
            dialog.generation = generation;
            dialog.run = Some(handle);
        }
        let model = ui.global::<SweepModel>();
        model.set_running(true);
        model.set_progress(0.0);
        model.set_status_text(present::running_status(0, total).into());
        present::push_results(ui, &self.dialog.borrow());
        let weak = Rc::downgrade(self);
        self.timer
            .start(TimerMode::Repeated, POLL_INTERVAL, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.poll();
                }
            });
    }

    /// The timer's tick: progress while the sweep runs, the rows when it is over.
    fn poll(&self) {
        let Some(ui) = self.ui.upgrade() else {
            self.timer.stop();
            return;
        };
        let polled = {
            let mut dialog = self.dialog.borrow_mut();
            dialog.run.as_mut().map(|run| (run.poll(), run.total()))
        };
        let model = ui.global::<SweepModel>();
        match polled {
            None => self.timer.stop(),
            Some((Poll::Running { done }, total)) => {
                model.set_progress(done as f32 / total.max(1) as f32);
                model.set_status_text(present::running_status(done, total).into());
            }
            Some((Poll::Finished(outcome), _)) => self.finish(&ui, outcome),
            Some((Poll::Died, _)) => {
                self.timer.stop();
                self.dialog.borrow_mut().run = None;
                model.set_running(false);
                model.set_status_text("The sweep stopped unexpectedly.".into());
                show_toast(&ui, "The sweep stopped unexpectedly.", "error");
            }
        }
    }

    /// A sweep is over (finished or stopped): shows its rows.
    fn finish(&self, ui: &MainWindow, outcome: SweepOutcome) {
        self.timer.stop();
        let status = present::finished_status(&outcome);
        {
            let mut dialog = self.dialog.borrow_mut();
            dialog.run = None;
            dialog.clear_results();
            // A sweep stopped before its first angle has no rows to show.
            dialog.outcome = (!outcome.rows.is_empty()).then_some(outcome);
        }
        let model = ui.global::<SweepModel>();
        model.set_running(false);
        model.set_progress(1.0);
        model.set_status_text(status.into());
        present::push_results(ui, &self.dialog.borrow());
    }

    /// Cancel: asks the sweep to stop; the rows finished so far still arrive.
    fn cancel(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let stopping = self
            .dialog
            .borrow()
            .run
            .as_ref()
            .map(RunHandle::cancel)
            .is_some();
        if stopping {
            ui.global::<SweepModel>()
                .set_status_text("Stopping...".into());
        }
        self.cancel_two_tier(&ui);
    }

    /// A table row or the chart was clicked.
    fn select_row(&self, row: i32) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Ok(index) = usize::try_from(row) else {
            return;
        };
        if self.dialog.borrow_mut().select(index) {
            present::push_pointer(&ui, &self.dialog.borrow());
        }
    }

    /// A figure's pill was clicked: its line goes on or off the chart.
    fn toggle_series(&self, series: i32) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(metric) = usize::try_from(series)
            .ok()
            .and_then(SweepMetric::from_index)
        else {
            return;
        };
        self.dialog.borrow_mut().toggle(metric);
        present::push_lines(&ui, &self.dialog.borrow());
    }

    /// The pointer moved over the chart (`fraction` across the plot, negative for left it).
    fn chart_hover(&self, fraction: f32) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let fraction = (fraction >= 0.0).then(|| f64::from(fraction));
        if self.dialog.borrow_mut().hover_at(fraction) {
            present::push_pointer(&ui, &self.dialog.borrow());
        }
    }

    /// "Use this angle": the selected row's angle becomes the tier's angle, as one undo step.
    fn use_selected(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (pick, generation) = {
            let dialog = self.dialog.borrow();
            (dialog.pick(), dialog.generation)
        };
        let Some(pick) = pick else {
            return;
        };
        let mut st = self.editor.borrow_mut();
        if st.current_generation() != generation {
            drop(st);
            show_toast(&ui, DESIGN_MOVED_ON, "error");
            return;
        }
        let name = pleochroma_editor::retarget::plan::tier_display_name(&st.design, pick.tier);
        match apply_sweep_angle(&mut st, pick.tier, pick.angle_deg) {
            Ok(Some(_)) => {
                refresh_with_followers(
                    &ui,
                    &self.render_ctx,
                    &self.preview_state,
                    &self.solid_last_solved,
                    &st,
                    [pick.tier],
                );
                crate::gui::editor::trials::record_after_apply(
                    &ui,
                    &st,
                    crate::gui::editor::trials::TrialOrigin::Sweep,
                );
                let after = st.current_generation();
                drop(st);
                self.dialog.borrow_mut().mark_applied(pick.row, after);
                present::push_results(&ui, &self.dialog.borrow());
                show_toast(
                    &ui,
                    &format!(
                        "{name} is now {:.2}\u{b0}. Undo (Ctrl+Z) puts it back.",
                        pick.angle_deg.abs()
                    ),
                    "success",
                );
            }
            Ok(None) => {
                drop(st);
                show_toast(&ui, "No change.", "info");
            }
            Err(error) => {
                drop(st);
                show_toast(&ui, &error.to_string(), "error");
            }
        }
    }

    /// The shown rows as CSV text.
    fn csv_text(&self) -> Option<String> {
        self.dialog.borrow().outcome.as_ref().map(sweep_csv)
    }

    /// Copy CSV: the clipboard gets the table.
    fn copy_csv(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if let Some(csv) = self.csv_text() {
            copy_to_clipboard_with_toast(&ui, csv, "Copied the sweep as CSV.".to_owned());
        }
    }

    /// Save CSV...: asks for a file, then writes the table to it.
    fn save_csv(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(csv) = self.csv_text() else {
            return;
        };
        let tier_name = self
            .dialog
            .borrow()
            .outcome
            .as_ref()
            .map(|outcome| outcome.tier_name.clone())
            .unwrap_or_default();
        let design_title = self
            .editor
            .borrow()
            .design
            .meta
            .headers
            .first()
            .cloned()
            .unwrap_or_default();
        let request = PickerRequest {
            kind: PickerKind::SaveFile,
            title: Some("Save the Angle Sweep as CSV".to_owned()),
            filters: vec![PickerFilter::single("csv")],
            default_file_name: Some(present::csv_file_name(&design_title, &tier_name)),
            starting_dir: None,
        };
        pick(&ui, request, move |ui, picked| {
            // A dismissed dialog is the cutter's own cancel: no toast.
            let Some(path) = picked else {
                return;
            };
            let ui_weak = ui.as_weak();
            std::thread::spawn(move || {
                let result = std::fs::write(&path, csv.as_bytes())
                    .map_err(|error| format!("Failed to write {}: {error}", path.display()));
                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(()) => show_toast(
                        &ui,
                        &format!("Saved the sweep to {}.", path.display()),
                        "success",
                    ),
                    Err(message) => show_toast(&ui, &message, "error"),
                });
            });
        });
    }
}
