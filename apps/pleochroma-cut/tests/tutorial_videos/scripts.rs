//! Explicit presentation actions, keyed by stable tutorial identity and step index.
//! Goals observe these actions; they are never decomposed into substitute edits.
//!
//! The registry is a list of `("guide-id", step) => ...` arms, split by lesson family into the
//! `script_*` methods below. The arms are read line by line by `pure_tables::scripted_keys`, so
//! each one starts its own line with its key; a step whose action takes more than a few lines
//! calls a method of one of the route modules instead.
use crate::{debug_log, director::Film, pacing};
use pleochroma_cut::EditorModel;
use pleochroma_editor::guide::{CMD_ADOPT_ALL, CMD_SOLVE, CMD_UNDO, TierEntry};
use slint::{ComponentHandle, Model};

#[path = "scripts_actions.rs"]
mod actions;
#[path = "annotation_routes.rs"]
mod annotation_routes;
#[path = "batch_routes.rs"]
mod batch_routes;
#[path = "chip_routes.rs"]
mod chip_routes;
#[path = "crystal_axis_routes.rs"]
mod crystal_axis_routes;
#[path = "cutting_routes.rs"]
mod cutting_routes;
#[path = "key_routes.rs"]
mod key_routes;
#[path = "lesson_routes.rs"]
mod lesson_routes;
#[path = "manual_tier_routes.rs"]
mod manual_tier_routes;
#[path = "manual_view_routes.rs"]
mod manual_view_routes;
#[path = "open_routes.rs"]
mod open_routes;
#[path = "optical_routes.rs"]
mod optical_routes;
#[path = "optimizing_routes.rs"]
mod optimizing_routes;
#[path = "scripts_palette.rs"]
mod palette;
#[path = "pick_routes.rs"]
mod pick_routes;
#[path = "planner_routes.rs"]
mod planner_routes;
/// The pure tables that pin the arms below to the catalogue (`pure_tables::SCRIPTED`).
#[path = "pure_tables.rs"]
pub mod pure_tables;
#[path = "raw_text_routes.rs"]
mod raw_text_routes;
#[path = "render_effect_routes.rs"]
mod render_effect_routes;
#[path = "render_jobs_routes.rs"]
mod render_jobs_routes;
#[path = "setup_routes.rs"]
mod setup_routes;
#[path = "table_routes.rs"]
mod table_routes;
#[path = "tuning_routes.rs"]
mod tuning_routes;
#[path = "welcome_routes.rs"]
mod welcome_routes;

impl Film<'_> {
    /// Returns false only when this step has no explicit script in this registry. Every arm of
    /// the `script_*` matches is listed, with its step title, in `pure_tables::SCRIPTED`, and the
    /// director checks both against the catalogue before it films: add an arm and its table row
    /// together.
    pub(crate) fn explicit_script(&mut self, n: usize) -> Result<bool, String> {
        debug_log::route_script(&self.id, n);
        if self.id == "viewing-optical-diagnostics" {
            optical_routes::drive(self, n)?;
            return Ok(true);
        }
        // The Rough Planner in-depth lessons: every step is routed by `planner_routes.rs`.
        if self.id.starts_with("library-planner-") {
            planner_routes::drive(self, n)?;
            return Ok(true);
        }
        // The Crystal Axis lessons: every step is routed by `crystal_axis_routes.rs`.
        if crystal_axis_routes::handles(&self.id) {
            crystal_axis_routes::drive(self, n)?;
            return Ok(true);
        }
        // The render-effects lessons: every step is routed by `render_effect_routes.rs`.
        if render_effect_routes::handles(&self.id) {
            render_effect_routes::drive(self, n)?;
            return Ok(true);
        }
        // The color variant lessons: every step is routed by `color_variant_routes.rs`.
        if crate::event_extras::handles_color_variants(&self.id) {
            self.film_color_variants(n)?;
            return Ok(true);
        }
        let found = self.script_tiers(n)?
            || self.script_optimizing(n)?
            || self.script_solving(n)?
            || self.script_tuning(n)?
            || self.script_viewing(n)?
            || self.script_render_jobs(n)?;
        if !found {
            debug_log::drop_route();
        }
        Ok(found)
    }

    /// The scripts of the tiers lessons, the command palette, the preform and the yield steps.
    #[expect(clippy::too_many_lines, reason = "one script arm per filmed step")]
    fn script_tiers(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("welcome-tour", 1) => self.film_welcome_library()?,
            ("welcome-tour", 3) => self.film_welcome_tier()?,
            ("welcome-tour", 5) => self.film_welcome_viewport()?,
            ("welcome-tour", 7) => self.film_welcome_help()?,
            ("app-command-palette", 0) => self.film_palette_introduction()?,
            ("tiers-edit-a-tier", 4) => self.film_add_tier_draft()?,
            ("tiers-tier-notes", 2) => annotation_routes::drive(self, n)?,
            ("tiers-index-shorthands", 0) => self.film_indices_tip()?,
            ("tiers-step-series", 0) => self.film_open_steps_panel()?,
            ("tiers-step-series", 3) => self.film_undo_redo_history("Step2", 2)?,
            ("tiers-inline-angle", 3) => {
                self.undo_redo_angle_nudges("Crown Main", 34.70, 34.50)?;
            }
            ("tiers-duplicate", 2) => self.film_undo_redo_history("Crown Main (2)", 2)?,
            ("tiers-mirror-to-other-block", 2) => self.film_move_and_undo_mirror_copy()?,
            ("tiers-concave-cylinder-cone", 3) => self.film_save_concave_groove_angle()?,
            ("tiers-concave-sphere-disc", 4) => self.film_open_cutting_mode_briefly()?,
            ("library-rough-planner", 1) => self.film_rough_model()?,
            ("app-command-palette", 1) => self.palette_choose_view("diag", "Diagram View", 3)?,
            ("app-command-palette", 2) => self.explain_disabled_palette_row()?,
            ("output-cutting-mode", 2) => self.film_cutting_mode_ticks_and_reset()?,
            ("new-design-walkthrough", 8) => self.film_yield(6.5)?,
            ("solving-yield-carat", 2) => self.film_yield(8.0)?,
            // The design to adopt from is loaded from the library, as the step's actions offer.
            ("tiers-adopt-imported-meets", 0) => self.open_imported_library_design()?,
            // Any one row's Adopt button that is on view in the table; it repeats the label on
            // every imported row, and the table is turned sideways until one is in view
            // (`open_routes.rs`).
            ("tiers-adopt-imported-meets", 1) => self.adopt_one_imported_tier()?,
            ("tiers-adopt-imported-meets", 2) => self.film_command(CMD_ADOPT_ALL)?,
            ("solving-preform", 2) => self.film_preform_width()?,
            ("tiers-pin-to-mast", 1) => self.pin_solved_depth("Upper Girdle")?,
            ("tiers-pin-to-mast", 2) => self.film_command(CMD_UNDO)?,
            ("tiers-linked-series", 1) => self.inline_angle("Step1", "32")?,
            ("tiers-linked-series", 3) => {
                self.film_tier(&TierEntry::of("Step3").angle("=Step1+2"))?;
                let row = self
                    .ui
                    .global::<EditorModel>()
                    .get_tiers()
                    .iter()
                    .find(|row| row.name.as_str() == "Step3")
                    .ok_or("Step3 disappeared after writing its relation")?;
                let angle = row.angle_full.parse::<f64>().map_err(|_| {
                    format!(
                        "Step3 has an unreadable committed angle: {:?}",
                        row.angle_full
                    )
                })?;
                if !angle.is_finite()
                    || (angle - 34.0).abs() > 1e-8
                    || row.relation_text.as_str().trim().is_empty()
                {
                    return Err(format!(
                        "Step3 relation did not commit as 34 degrees with a visible relation: angle={:?}, relation={:?}",
                        row.angle_full, row.relation_text
                    ));
                }
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("tiers-arithmetic", 1) => {
                // film_tier picks the Table row itself.
                self.film_tier(&TierEntry::of("Table").meets(2, "0.5-0.15"))?;
            }
            ("tiers-arithmetic", 2) => {
                self.film_select_tier("Crown Main")?;
                let field = self
                    .input_spot("Position of the Facet to Add")?
                    .ok_or_else(|| "facet-position input is not visible".to_owned())?;
                self.click_and_type(&field, "96/16")?;
                self.press_label("Add Facet")?;
            }
            ("tiers-arithmetic", 3) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            // The chips under Facets: the lesson teaches these controls, not a Tier form edit
            // that lands on the same indices (`chip_routes.rs`).
            ("tiers-facet-chips", 0) => self.add_chip_facet("Crown Main", 6.0, "6")?,
            ("tiers-facet-chips", 1) => self.remove_chip("Crown Main", 6.0)?,
            ("tiers-facet-chips", 2) => self.detach_chip("Crown Main", 84.0)?,
            ("tiers-facet-chips", 3) => self.remove_chip("Crown Main", 84.0)?,
            ("tiers-facet-chips", 4) => self.rotate_chips_up("Crown Main", "6")?,
            ("tiers-facet-chips", 5) => self.mirror_chips("Crown Main")?,
            // The Quick Add buttons of the bar above the table, not a Tier form that types the
            // same values (`table_routes.rs`).
            ("tiers-quick-add", 0) => self.quick_add_tier("Girdle")?,
            ("tiers-quick-add", 1) => self.quick_add_tier("Table")?,
            ("tiers-quick-add", 2) => self.quick_add_tier("Culet")?,
            ("tiers-quick-add", 3) => self.film_quick_add_make_them_yours()?,
            // The ANGLE cell of the table itself: F2 and typing, the Up key, Ctrl and the wheel.
            ("tiers-inline-angle", 0) => self.inline_angle("Pavilion Main", "41")?,
            ("tiers-inline-angle", 1) => self.nudge_angle_with_keys("Crown Main", 2)?,
            ("tiers-inline-angle", 2) => self.nudge_angle_with_wheel("Crown Main", -2)?,
            ("tiers-step-series", 2) => self.inline_angle("Step2", "35")?,
            ("tiers-duplicate", 1) => self.inline_angle("Crown Main (2)", "30")?,
            ("tiers-move", 2) => {
                self.key("Ctrl+Z")?;
                let editor = self.ui.global::<EditorModel>();
                if !pacing::wait_real(pacing::START_TIMEOUT, || {
                    let rows = editor.get_tiers();
                    rows.row_data(0)
                        .is_some_and(|row| row.name.as_str() == "Girdle")
                        && rows
                            .row_data(1)
                            .is_some_and(|row| row.name.as_str() == "Pavilion Main")
                }) {
                    return Err(format!(
                        "Ctrl+Z did not undo the latest tier move: row0={:?}, row1={:?}",
                        editor
                            .get_tiers()
                            .row_data(0)
                            .map(|row| row.name.to_string()),
                        editor
                            .get_tiers()
                            .row_data(1)
                            .map(|row| row.name.to_string())
                    ));
                }
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("tiers-relations", 2) => self.inline_angle("Crown Main", "36")?,
            // The Offset and Delete buttons of the bar above the table act on the whole ticked
            // group.
            ("tiers-multi-select", 1) => self.offset_ticked_tiers("1")?,
            ("tiers-multi-select", 2) => self.delete_ticked_tiers()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the Optimize, Retarget and Angle Sweep lessons.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_optimizing(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("optimizing-deep-solve", 3) => self.film_read_status_details()?,
            ("optimizing-optimize", 2) => self.optimize_choose_objective()?,
            ("optimizing-optimize", 3) => self.optimize_open_ranges()?,
            ("optimizing-optimize", 4) => self.optimize_run_search()?,
            ("optimizing-optimize", 5) => self.optimize_pick_candidate()?,
            ("optimizing-optimize", 6) => self.optimize_apply_candidate()?,
            ("optimizing-optimize", 7) => {
                self.film_command(CMD_SOLVE)?;
                self.hold(pacing::RESULT_HOLD)?;
                self.film_command(CMD_UNDO)?;
            }
            ("optimizing-retarget-shift", 1) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-shift", 2) => self.retarget_shift_to_topaz()?,
            ("optimizing-retarget-shift", 3) => self.film_command(CMD_UNDO)?,
            ("optimizing-retarget-shift", 4) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-optimize", 1) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-optimize", 2) => self.retarget_optimize_to_sapphire()?,
            ("optimizing-retarget-optimize", 3) => self.film_command(CMD_UNDO)?,
            ("optimizing-retarget-optimize", 4) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-sweep", 0) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-sweep", 1) => self.sweep_pavilion_and_use()?,
            ("optimizing-sweep", 2) => self.film_command(CMD_UNDO)?,
            ("optimizing-sweep", 3) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the history, variants, raw text, snapshot and verdict lessons.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_solving(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("solving-solve", 4) => self.film_read_status_details()?,
            ("solving-solve", 5) => self.film_restore_auto_solve()?,
            ("solving-status", 1) => self.film_read_status_details()?,
            ("solving-raw-text", 3) => {
                self.open_raw_text()?;
                self.hold(pacing::RESULT_HOLD)?;
                self.dialog_button("Close")?;
            }
            ("solving-snapshot-compare", 1) => self.inline_angle("Crown Main", "36")?,
            ("solving-snapshot-compare", 4) => self.film_compare_read_close()?,
            ("solving-history", 1) => self.inline_angle("Crown Main", "36")?,
            ("solving-history", 2) => self.inline_angle("Pavilion Main", "41")?,
            ("solving-history", 3) => self.history_jump(1)?,
            ("solving-history", 4) => self.history_jump(0)?,
            ("solving-history", 5) => self.history_jump(2)?,
            ("solving-history", 6) => self.film_command(CMD_UNDO)?,
            ("solving-history", 7) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            // The previous step solved; this one only opens the verdict.
            ("solving-verdict-fixes", 3) => self.verdict_fix("Add a Table", false)?,
            ("solving-verdict-fixes", 4) => self.film_index_between_teeth()?,
            // The step's first action is "Click Solve", so the verdict is current.
            ("solving-verdict-fixes", 5) => self.verdict_fix("Snap to Teeth", true)?,
            ("solving-verdict-fixes", 6) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("solving-variants", 0) => self.open_variants_view()?,
            ("solving-variants", 1) => self.save_variant("Variant 1")?,
            ("solving-variants", 2) => self.inline_angle("Crown Main", "38")?,
            ("solving-variants", 3) => self.save_variant("Steeper crown")?,
            ("solving-variants", 4) => self.open_variant("Variant 1")?,
            ("solving-variants", 5) => self.film_command(CMD_UNDO)?,
            ("solving-variants", 6) => self.compare_variants()?,
            ("solving-variants", 7) => {
                self.delete_variant("Steeper crown")?;
                self.delete_variant("Variant 1")?;
            }
            ("solving-raw-text", 0) => self.film_command("solve.snapshot")?,
            ("solving-raw-text", 1) => self.edit_raw_text()?,
            ("solving-raw-text", 2) => self.film_command(CMD_UNDO)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the Two Tiers sweep, Keep Trials and Tune by Hand lessons
    /// (`tuning_routes.rs`) and of the Watermelon Tourmaline lesson's windows.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_tuning(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("viewing-color-pattern", 1) => self.film_pattern_material()?,
            ("viewing-color-pattern", 2) => self.film_pattern_watermelon()?,
            ("optimizing-sweep-two-tiers", 1) => self.film_two_tier_sweep()?,
            ("optimizing-sweep-two-tiers", 2) => self.film_command(CMD_UNDO)?,
            ("solving-trials", 0) => self.film_keep_trial()?,
            ("solving-trials", 1) => self.inline_angle("Crown Main", "36")?,
            ("solving-trials", 2) => self.film_keep_trial()?,
            ("solving-trials", 3) => self.film_open_trials_view()?,
            ("solving-trials", 4) => self.film_restore_trial(1)?,
            ("solving-trials", 5) => self.film_trial_to_baseline(2)?,
            ("optimizing-tune-by-hand", 0) => self.film_open_tune_view()?,
            ("optimizing-tune-by-hand", 1) => {
                self.film_tune_pick("Pavilion Main")?;
                self.film_tune_nudge()?;
            }
            ("optimizing-tune-by-hand", 2) => self.film_tune_accept()?,
            ("optimizing-tune-by-hand", 3) => self.film_tune_next_and_nudge()?,
            ("optimizing-tune-by-hand", 4) => self.film_tune_accept()?,
            ("optimizing-tune-by-hand", 5) => self.film_tune_show_chart()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the keyboard, picking, diagram, drag handle and Slice lessons, and of
    /// opening a file.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_viewing(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("viewing-diagram", 5) => self.film_diagram_return_and_undo()?,
            ("viewing-live-render", 1) => self.film_live_mouse_controls()?,
            // The lesson teaches the keys themselves (`key_routes.rs`), not the pills, the palette
            // command or the Help menu that reach the same views.
            ("app-keyboard-shortcuts", 1) => self.press_view_key("4", 3)?,
            ("app-keyboard-shortcuts", 2) => self.press_view_key("1", 0)?,
            ("app-keyboard-shortcuts", 3) => self.swap_tabs_with_keys()?,
            ("app-keyboard-shortcuts", 4) => self.open_shortcuts_with_key()?,
            // The step says to click a facet of a different tier than the one step 2 picked (Crown
            // Main), which the event route's Crown Main facet would not be.
            ("viewing-solid-picking", 1) => self.hover_crown_facet()?,
            ("viewing-solid-picking", 4) => self.pick_other_tier_facet("Crown Main")?,
            // The step says to click a row of the tier list; a click on a facet raises the same
            // event, but it is the other route.
            ("viewing-solid-picking", 5) => {
                self.film_select_tier("Pavilion Main")?;
            }
            ("viewing-live-render", 5) => self.return_to_edit_from_live_render()?,
            // The wizard's pages the reading steps describe: one pill click per page of the strip
            // (`rough_color_routes.rs`).
            ("library-rough-color-wizard", 2) => self.film_rough_color_pages(&[0, 1, 2])?,
            ("library-rough-color-wizard", 3) => self.film_rough_color_pages(&[3, 4])?,
            ("library-rough-color-wizard", 4) => self.film_rough_color_pages(&[5])?,
            ("library-rough-color-wizard", 5) => self.film_rough_color_pages(&[6])?,
            ("library-rough-color-wizard", 6) => self.film_rough_color_pages(&[7])?,
            ("viewing-custom-material", 2) => self.film_material("Tutorial glass")?,
            ("output-cutting-sheet", 2) => self.film_external_follow_up(
                "cutting-sheet-printing",
                "Next: open and print outside this recording",
                "Next, open the exported HTML in a browser and print it, or keep it as a record of the cut. That browser step is outside this recording.",
            )?,
            ("output-export-gcs", 2) => self.film_external_follow_up(
                "gcs-validation",
                "Next: check in Gem Cut Studio before cutting",
                "Next, open the .gcs file in Gem Cut Studio and compare it with the faceting diagram. Check it in Gem Cut Studio before cutting; that validation is outside this recording.",
            )?,
            ("app-help-viewer", 1) => self.drive_extra_event(1, "help_opened")?,
            ("app-help-viewer", 2) => self.film_context_help()?,
            ("app-help-viewer", 3) => self.film_manual_search()?,
            // The step's own action: File > Open on the file the lesson saved, not Load Selected.
            ("output-open", 1) => self.open_saved_design()?,
            ("viewing-diagram", 3) => self.drag_diagram_angle_handle()?,
            ("viewing-drag-handles", 0) => self.pick_crown_facet()?,
            ("viewing-drag-handles", 1) => self.drag_handle(0, 70.0, false)?,
            ("viewing-drag-handles", 2) => self.film_command(CMD_UNDO)?,
            ("viewing-drag-handles", 3) => self.drag_handle(1, -70.0, false)?,
            ("viewing-drag-handles", 4) => self.drag_handle(2, 80.0, false)?,
            ("viewing-drag-handles", 5) => {
                self.film_command("view.snap")?;
                self.drag_handle_fine(0, 30.0)?;
            }
            ("viewing-drag-handles", 6) => self.film_command("view.snap")?,
            ("viewing-drag-handles", 7) => {
                self.drag_handle(0, 40.0, true)?;
                // The drag was cancelled with Escape inside `gesture`. This second Escape is the
                // lesson's "Escape with nothing being dragged clears the selection".
                self.key("Escape")?;
                for _ in 0..3 {
                    self.film_command(CMD_UNDO)?;
                }
            }
            ("viewing-slice", 0) => {
                self.film_view_mode(0)?;
                self.film_command("view.slice")?;
            }
            ("viewing-slice", 1) => self.draw_slice()?,
            ("viewing-slice", 2) => {
                self.press_label("Flip")?;
                self.hold(pacing::RESULT_HOLD)?;
                self.press_label("Flip")?;
            }
            ("viewing-slice", 3) => self.cut_slice_in()?,
            ("viewing-slice", 4) => self.film_symmetric_toggle()?,
            ("viewing-slice", 5) => self.keep_slice()?,
            ("viewing-slice", 6) => self.film_command(CMD_UNDO)?,
            ("viewing-slice", 7) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the render jobs lessons (`render_jobs_routes.rs`).
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_render_jobs(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("output-render-jobs", 2) => self.film_render_export_open()?,
            ("output-render-jobs", 3) => self.film_render_job_add()?,
            ("output-render-jobs", 4) => self.film_render_jobs_open()?,
            ("output-render-jobs-queue", 0) => self.film_render_job_queue_picture()?,
            ("output-render-jobs-queue", 1) => self.film_render_jobs_open()?,
            ("output-render-jobs-queue", 2) => self.film_render_queue_start()?,
            ("output-render-jobs-queue", 3) => self.film_render_queue_pause()?,
            ("output-render-jobs-queue", 4) => self.film_render_job_resume()?,
            ("output-render-jobs-queue", 6) => self.film_render_script_export()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn film_external_follow_up(
        &mut self,
        key: &str,
        chapter: &str,
        caption: &str,
    ) -> Result<(), String> {
        self.rec.chapter(chapter);
        self.note_once(
            key,
            format!("External follow-up is described but not recorded: {caption}"),
        );
        for cue in pacing::cues(chapter, &[caption.to_owned()]) {
            self.rec.caption(&cue);
            self.hold(pacing::reading_time(cue.split_whitespace().count()))?;
        }
        Ok(())
    }
}
