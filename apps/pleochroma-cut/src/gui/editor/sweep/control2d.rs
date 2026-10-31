//! The Angle Sweep dialog's Two Tiers mode: the [`Controller`]'s answers to the model's
//! callbacks for the mode switch, the form, Run and Cancel, the heat map's pointer and keys,
//! the figure choice, "Use These Angles", "Refine Around This" and the two CSV buttons.
//!
//! One Tier's answers are in `mod.rs`; the two modes share the dialog, the tier list, the
//! tilt option, the progress bar and the status line, and keep their own results.

use super::{
    Controller, DESIGN_MOVED_ON, POLL_INTERVAL,
    form::Fields,
    heatmap::cell_at,
    present2d,
    run2d::{Job2d, Poll2d, RunHandle2d, start_2d},
    state2d::{
        Checked2d, SavedForm, axis_fields, check_2d, default_b_choice, default_fields_2d,
        needs_tilt_confirm, refusal_text, tilt_question,
    },
};
use crate::{
    MainWindow, SweepModel,
    gui::{
        editor::{relation_ui::refresh_with_followers, state::EditorState},
        library::clipboard::copy_to_clipboard_with_toast,
        pickers::{PickerFilter, PickerKind, PickerRequest, pick},
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma_cut_core::CANONICAL_LIGHTING_PRESET;
use pleochroma_editor::{
    guide::solving_events as events,
    optimize_view::default_optimize_material_ri,
    solve_policy::SolveCostEstimate,
    sweep::{
        Sweep2dPlan, SweepOptions, apply_sweep_angles, estimate_seconds_2d, refine_around,
        sweep2d_csv,
    },
};
use slint::{ComponentHandle, TimerMode};
use std::{rc::Rc, sync::PoisonError};

/// How much finer "Refine Around This" makes the grid's step around the chosen pair.
const REFINE_FACTOR: f64 = 0.5;

/// Puts the two tiers' ranges into the six number fields.
fn push_fields_2d(ui: &MainWindow, across: &Fields, down: &Fields) {
    let model = ui.global::<SweepModel>();
    model.set_from_text(across.from.as_str().into());
    model.set_to_text(across.to.as_str().into());
    model.set_step_text(across.step.as_str().into());
    model.set_from_b_text(down.from.as_str().into());
    model.set_to_b_text(down.to.as_str().into());
    model.set_step_b_text(down.step.as_str().into());
}

/// What the form reads: the across tier and its fields, the down tier and its fields, and
/// the tilt choice.
type FormRead = (usize, Fields, usize, Fields, bool);

impl Controller {
    /// A new dialog session (or the end of one): Two Tiers starts from scratch, a running
    /// sweep is stopped and One Tier is shown.
    pub(super) fn reset_two_tier(&self, ui: &MainWindow) {
        self.timer_2d.stop();
        // Dropping the run handle stops the sweep.
        self.two.borrow_mut().reset();
        let model = ui.global::<SweepModel>();
        model.set_mode(0);
        model.set_confirm_tilt_text("".into());
        model.set_cells_summary("".into());
        model.set_form_ok_2d(false);
        present2d::push_results(ui, &self.two.borrow());
    }

    /// A mode pill was clicked (`SweepModel.mode` holds the new mode).
    pub(super) fn mode_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.global::<SweepModel>().get_mode() == 1 {
            self.enter_two_tier(&ui);
        } else {
            self.leave_two_tier(&ui);
        }
    }

    /// Going to Two Tiers: the form as it was left, else the starting form.
    fn enter_two_tier(&self, ui: &MainWindow) {
        let model = ui.global::<SweepModel>();
        let saved = self.two.borrow().saved.clone();
        if let Some(saved) = saved {
            model.set_tier_index(saved.a_index);
            model.set_tier_b_index(saved.b_index);
            push_fields_2d(ui, &saved.a, &saved.b);
        } else {
            let tiers = self.dialog.borrow().tiers.clone();
            let a_choice = usize::try_from(model.get_tier_index()).unwrap_or(0);
            let Some(&a_tier) = tiers.get(a_choice) else {
                return;
            };
            let (across, b_choice, down) = {
                let st = self.editor.borrow();
                let b_choice = default_b_choice(&st.design, &tiers, a_choice);
                let angle = |tier: usize| st.design.tiers.get(tier).map_or(0.0, |t| t.angle_deg);
                let b_tier = tiers.get(b_choice).copied().unwrap_or(a_tier);
                (
                    default_fields_2d(angle(a_tier)),
                    b_choice,
                    default_fields_2d(angle(b_tier)),
                )
            };
            model.set_tier_b_index(i32::try_from(b_choice).unwrap_or(0));
            push_fields_2d(ui, &across, &down);
        }
        self.check_two_tier(ui);
    }

    /// Going back to One Tier: the Two Tiers form is kept, and One Tier starts again from
    /// its tier's own range.
    fn leave_two_tier(&self, ui: &MainWindow) {
        let model = ui.global::<SweepModel>();
        self.two.borrow_mut().saved = Some(SavedForm {
            a_index: model.get_tier_index(),
            b_index: model.get_tier_b_index(),
            a: Fields {
                from: model.get_from_text().to_string(),
                to: model.get_to_text().to_string(),
                step: model.get_step_text().to_string(),
            },
            b: Fields {
                from: model.get_from_b_text().to_string(),
                to: model.get_to_b_text().to_string(),
                step: model.get_step_b_text().to_string(),
            },
        });
        self.tier_changed();
    }

    /// The combos, the six number fields and the tilt choice.
    fn read_form_2d(&self, ui: &MainWindow) -> Option<FormRead> {
        let model = ui.global::<SweepModel>();
        let dialog = self.dialog.borrow();
        let across = *dialog
            .tiers
            .get(usize::try_from(model.get_tier_index()).ok()?)?;
        let down = *dialog
            .tiers
            .get(usize::try_from(model.get_tier_b_index()).ok()?)?;
        let fields_a = Fields {
            from: model.get_from_text().to_string(),
            to: model.get_to_text().to_string(),
            step: model.get_step_text().to_string(),
        };
        let fields_b = Fields {
            from: model.get_from_b_text().to_string(),
            to: model.get_to_b_text().to_string(),
            step: model.get_step_b_text().to_string(),
        };
        Some((across, fields_a, down, fields_b, model.get_tilt_average()))
    }

    /// Says what the form comes to, and drops a question the form no longer matches.
    fn check_two_tier(&self, ui: &MainWindow) {
        let model = ui.global::<SweepModel>();
        model.set_confirm_tilt_text("".into());
        self.two.borrow_mut().pending = None;
        let Some((across, fields_a, down, fields_b, tilt_average)) = self.read_form_2d(ui) else {
            return;
        };
        let checked = {
            let st = self.editor.borrow();
            check_2d(
                &st.design,
                (across, &fields_a),
                (down, &fields_b),
                tilt_average,
                self.workers,
            )
        };
        match checked {
            Checked2d::Ready { summary, .. } => {
                model.set_cells_summary(summary.into());
                model.set_form_ok_2d(true);
            }
            Checked2d::Refused(message) => {
                model.set_cells_summary(message.into());
                model.set_form_ok_2d(false);
            }
        }
    }

    /// A number field or the tilt option changed: say what the form comes to.
    pub(super) fn form_changed_2d(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        self.check_two_tier(&ui);
    }

    /// The across tier was chosen: its own range goes into its fields.
    pub(super) fn tier_a_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((across, _, _, fields_b, _)) = self.read_form_2d(&ui) else {
            return;
        };
        let fields_a = self.starting_fields(across);
        push_fields_2d(&ui, &fields_a, &fields_b);
        self.check_two_tier(&ui);
    }

    /// The down tier was chosen: its own range goes into its fields.
    pub(super) fn tier_b_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((_, fields_a, down, _, _)) = self.read_form_2d(&ui) else {
            return;
        };
        let fields_b = self.starting_fields(down);
        push_fields_2d(&ui, &fields_a, &fields_b);
        self.check_two_tier(&ui);
    }

    /// The range `tier` starts with in Two Tiers.
    fn starting_fields(&self, tier: usize) -> Fields {
        let angle = self
            .editor
            .borrow()
            .design
            .tiers
            .get(tier)
            .map_or(0.0, |found| found.angle_deg);
        default_fields_2d(angle)
    }

    /// Everything a two-tier sweep of `plan` reads, taken from the design now.
    fn job_2d(&self, st: &EditorState, plan: Sweep2dPlan, tilt_average: bool) -> Job2d {
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
        Job2d {
            design: st.design.clone(),
            plan,
            material,
            custom_materials,
            // Rated under the grading tray like Optimize and Retarget, whatever lighting
            // the viewport shows, so a cell and an Optimize candidate compare directly.
            lighting: CANONICAL_LIGHTING_PRESET,
            options: SweepOptions {
                tilt_average,
                workers: self.workers,
            },
        }
    }

    /// Run: reads the form and starts the sweep, after the one question a long tilt sweep
    /// asks.
    pub(super) fn run_2d(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.two.borrow().run.is_some() {
            return;
        }
        let Some((across, fields_a, down, fields_b, tilt_average)) = self.read_form_2d(&ui) else {
            return;
        };
        let checked = {
            let st = self.editor.borrow();
            check_2d(
                &st.design,
                (across, &fields_a),
                (down, &fields_b),
                tilt_average,
                self.workers,
            )
        };
        match checked {
            Checked2d::Ready { plan, .. } => self.request_run(&ui, plan, tilt_average),
            Checked2d::Refused(message) => show_toast(&ui, &message, "error"),
        }
    }

    /// Starts the sweep of `plan`, or, for a long one with tilt averages that has not been
    /// confirmed in this dialog session, shows the question and waits for the answer.
    fn request_run(self: &Rc<Self>, ui: &MainWindow, plan: Sweep2dPlan, tilt_average: bool) {
        if needs_tilt_confirm(plan.len(), tilt_average) && !self.two.borrow().tilt_confirmed {
            let seconds = {
                let st = self.editor.borrow();
                estimate_seconds_2d(
                    plan.len(),
                    tilt_average,
                    self.workers,
                    SolveCostEstimate::of(&st.design),
                )
            };
            self.two.borrow_mut().pending = Some(plan);
            ui.global::<SweepModel>()
                .set_confirm_tilt_text(tilt_question(seconds).into());
            return;
        }
        self.begin_2d(ui, plan, tilt_average);
    }

    /// The answer to the tilt question: "Run Anyway" runs the waiting sweep (and the question
    /// is not asked again in this dialog session); "Cancel" drops it.
    pub(super) fn confirm_tilt(self: &Rc<Self>, yes: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let model = ui.global::<SweepModel>();
        model.set_confirm_tilt_text("".into());
        let pending = self.two.borrow_mut().pending.take();
        if !yes {
            return;
        }
        self.two.borrow_mut().tilt_confirmed = true;
        if let Some(plan) = pending {
            self.begin_2d(&ui, plan, model.get_tilt_average());
        }
    }

    /// Starts the sweep of `plan` and shows the dialog as running.
    fn begin_2d(self: &Rc<Self>, ui: &MainWindow, plan: Sweep2dPlan, tilt_average: bool) {
        let (job, generation) = {
            let st = self.editor.borrow();
            (
                self.job_2d(&st, plan, tilt_average),
                st.current_generation(),
            )
        };
        let total = job.plan.len();
        let handle = match start_2d(job) {
            Ok(handle) => handle,
            Err(message) => {
                show_toast(ui, &message, "error");
                return;
            }
        };
        {
            let mut two = self.two.borrow_mut();
            two.clear_results();
            two.generation = generation;
            two.pending = None;
            two.run = Some(handle);
        }
        let model = ui.global::<SweepModel>();
        model.set_confirm_tilt_text("".into());
        model.set_running(true);
        model.set_progress(0.0);
        model.set_status_text(present2d::running_status(0, total).into());
        present2d::push_results(ui, &self.two.borrow());
        let weak = Rc::downgrade(self);
        self.timer_2d
            .start(TimerMode::Repeated, POLL_INTERVAL, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.poll_2d();
                }
            });
    }

    /// The timer's tick: progress while the sweep runs, the cells when it is over.
    fn poll_2d(&self) {
        let Some(ui) = self.ui.upgrade() else {
            self.timer_2d.stop();
            return;
        };
        let polled = {
            let mut two = self.two.borrow_mut();
            two.run
                .as_mut()
                .map(|run: &mut RunHandle2d| (run.poll(), run.total()))
        };
        let model = ui.global::<SweepModel>();
        match polled {
            None => self.timer_2d.stop(),
            Some((Poll2d::Running { done }, total)) => {
                model.set_progress(done as f32 / total.max(1) as f32);
                model.set_status_text(present2d::running_status(done, total).into());
            }
            Some((Poll2d::Finished(outcome), _)) => {
                self.timer_2d.stop();
                let status = present2d::finished_status(&outcome);
                let has_cells = !outcome.cells.is_empty();
                {
                    let mut two = self.two.borrow_mut();
                    two.run = None;
                    two.clear_results();
                    // A sweep stopped before its first pair has nothing to show.
                    two.outcome = (!outcome.cells.is_empty()).then_some(*outcome);
                }
                model.set_running(false);
                model.set_progress(1.0);
                model.set_status_text(status.into());
                present2d::push_results(&ui, &self.two.borrow());
                // The Two Tiers lesson waits for a finished sweep.
                if has_cells {
                    raise(&ui, events::TWO_TIER_SWEEP_RUN);
                }
            }
            Some((Poll2d::Died, _)) => {
                self.timer_2d.stop();
                self.two.borrow_mut().run = None;
                model.set_running(false);
                model.set_status_text("The sweep stopped unexpectedly.".into());
                show_toast(&ui, "The sweep stopped unexpectedly.", "error");
            }
        }
    }

    /// Cancel (and Escape): asks a running two-tier sweep to stop; the cells finished so far
    /// still arrive.
    pub(super) fn cancel_two_tier(&self, ui: &MainWindow) {
        let stopping = self
            .two
            .borrow()
            .run
            .as_ref()
            .map(RunHandle2d::cancel)
            .is_some();
        if stopping {
            ui.global::<SweepModel>()
                .set_status_text("Stopping...".into());
        }
    }

    /// The pointer over the map (`kind` 0 a move, 1 a click, 2 it left) at `(x, y)`, each a
    /// fraction of the image.
    pub(super) fn heat_pointer(&self, x: f32, y: f32, kind: i32) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let changed = {
            let mut two = self.two.borrow_mut();
            let cell = two
                .outcome
                .as_ref()
                .map(|outcome| (outcome.plan.columns(), outcome.plan.rows()))
                .filter(|_| kind != 2)
                .and_then(|(columns, rows)| cell_at(columns, rows, x, y));
            if kind == 1 {
                let selected = cell.is_some_and(|(col, row)| two.select(col, row));
                let hovered = two.hover_at(cell);
                selected || hovered
            } else {
                two.hover_at(cell)
            }
        };
        if changed {
            present2d::push_marks(&ui, &self.two.borrow());
        }
    }

    /// A key on the map: the arrows move the selection, Enter uses the pair.
    pub(super) fn heat_key(&self, key: i32) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (dcol, drow) = match key {
            0 => (-1, 0),
            1 => (1, 0),
            2 => (0, -1),
            3 => (0, 1),
            4 => {
                self.use_cells();
                return;
            }
            _ => return,
        };
        let moved = self.two.borrow_mut().step_selection(dcol, drow);
        if moved {
            present2d::push_marks(&ui, &self.two.borrow());
        }
    }

    /// The figure combo changed: the map is drawn again for that figure.
    pub(super) fn heat_figure_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let index = usize::try_from(ui.global::<SweepModel>().get_heat_figure_index());
        let changed = index.is_ok_and(|index| self.two.borrow_mut().set_figure_index(index));
        if changed {
            present2d::push_results(&ui, &self.two.borrow());
        }
    }

    /// The outline checkbox changed: the map is drawn again.
    pub(super) fn contour_changed(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        self.two.borrow_mut().contour = ui.global::<SweepModel>().get_contour();
        present2d::push_results(&ui, &self.two.borrow());
    }

    /// "Use These Angles": the selected pair becomes the two tiers' angles, as one undo step.
    pub(super) fn use_cells(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (pick, generation) = {
            let two = self.two.borrow();
            if two.run.is_some() {
                return;
            }
            (two.pick(), two.generation)
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
        let name_a = pleochroma_editor::retarget::plan::tier_display_name(&st.design, pick.tier_a);
        let name_b = pleochroma_editor::retarget::plan::tier_display_name(&st.design, pick.tier_b);
        match apply_sweep_angles(
            &mut st,
            (pick.tier_a, pick.angle_a),
            (pick.tier_b, pick.angle_b),
        ) {
            Ok(Some(_)) => {
                refresh_with_followers(
                    &ui,
                    &self.render_ctx,
                    &self.preview_state,
                    &self.solid_last_solved,
                    &st,
                    [pick.tier_a, pick.tier_b],
                );
                crate::gui::editor::trials::record_after_apply(
                    &ui,
                    &st,
                    crate::gui::editor::trials::TrialOrigin::TwoTierSweep,
                );
                let after = st.current_generation();
                drop(st);
                {
                    let mut two = self.two.borrow_mut();
                    two.mark_applied(pick.col, pick.row, after);
                }
                present2d::push_marks(&ui, &self.two.borrow());
                show_toast(
                    &ui,
                    &format!(
                        "{name_a} is now {:.2}\u{b0} and {name_b} is now {:.2}\u{b0}. Undo \
                         (Ctrl+Z) puts them back.",
                        pick.angle_a.abs(),
                        pick.angle_b.abs()
                    ),
                    "success",
                );
                raise(&ui, events::TWO_TIER_APPLIED);
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

    /// "Refine Around This": a finer grid around the selected pair, run at once. The form
    /// shows the new ranges.
    pub(super) fn refine(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (plan, cell) = {
            let two = self.two.borrow();
            if two.run.is_some() {
                return;
            }
            let (Some(outcome), Some(cell)) = (&two.outcome, two.selected) else {
                return;
            };
            (outcome.plan.clone(), cell)
        };
        let refined = {
            let st = self.editor.borrow();
            refine_around(&plan, &st.design, cell, REFINE_FACTOR)
        };
        let refined = match refined {
            Ok(refined) => refined,
            Err(error) => {
                show_toast(&ui, &refusal_text(&error), "error");
                return;
            }
        };
        let model = ui.global::<SweepModel>();
        let across = axis_fields(&refined.angles_a, model.get_step_text().as_str());
        let down = axis_fields(&refined.angles_b, model.get_step_b_text().as_str());
        push_fields_2d(&ui, &across, &down);
        self.check_two_tier(&ui);
        self.request_run(&ui, refined, model.get_tilt_average());
    }

    /// The shown cells as CSV text.
    fn csv_text_2d(&self) -> Option<String> {
        self.two.borrow().outcome.as_ref().map(sweep2d_csv)
    }

    /// Copy CSV: the clipboard gets the cells.
    pub(super) fn copy_csv_2d(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if let Some(csv) = self.csv_text_2d() {
            copy_to_clipboard_with_toast(&ui, csv, "Copied the sweep as CSV.".to_owned());
        }
    }

    /// Save CSV...: asks for a file, then writes the cells to it.
    pub(super) fn save_csv_2d(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(csv) = self.csv_text_2d() else {
            return;
        };
        let (name_a, name_b) = self
            .two
            .borrow()
            .outcome
            .as_ref()
            .map(|outcome| {
                (
                    outcome.plan.tier_a_name.clone(),
                    outcome.plan.tier_b_name.clone(),
                )
            })
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
            default_file_name: Some(present2d::csv_file_name_2d(&design_title, &name_a, &name_b)),
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
