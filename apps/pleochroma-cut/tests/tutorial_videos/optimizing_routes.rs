//! Filmed routes of the Optimize, Retarget and Angle Sweep lessons, kept out of the registry in
//! `scripts.rs` so that its match stays a list of arms.

use crate::{director::Film, pacing, support::app::pump, targets};
use pleochroma_cut::{EditorModel, OptimizeModel, RetargetModel, SweepModel};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// How long the short Optimize run of the lesson may take. The panel's own estimate (about five
/// seconds) is a guess from the design size; the run is a multi-start search on several threads,
/// then three full-quality scorings (the starting stone and two candidates), so a loaded machine
/// (a debug run starts many scenarios at once) took 30 s and more.
const OPTIMIZE_TIMEOUT: Duration = Duration::from_secs(120);

impl Film<'_> {
    /// The Optimize tab: opens the Optimize Objective combo and chooses Low Windowing.
    pub(super) fn optimize_choose_objective(&mut self) -> Result<(), String> {
        // The previous step opened the Optimize tab; do not click it a second time.
        if self.ui.global::<EditorModel>().get_inspector_tab() != 2 {
            self.film_inspector_tab(2)?;
        }
        let combo = self
            .inspector_form_control("Optimize Objective", uiprobe::AccessibleRole::Combobox)?
            .ok_or("Optimize Objective combo is missing")?;
        let optimize = self.ui.global::<OptimizeModel>();
        let labels = optimize.get_preset_labels();
        let idx = labels
            .iter()
            .position(|l| l.as_str() == "Low Windowing")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Low Windowing objective is unavailable")?;
        let current = optimize.get_preset_index();
        self.click_and_invoke(&combo)?;
        self.select_open_combo(current, idx)?;
        self.verify_optimize_objective("Low Windowing")?;
        pump(60);
        self.hold(pacing::MENU_HOLD)
    }

    fn verify_optimize_objective(&mut self, expected: &str) -> Result<(), String> {
        let optimize = self.ui.global::<OptimizeModel>();
        let actual = usize::try_from(optimize.get_preset_index())
            .ok()
            .and_then(|index| optimize.get_preset_labels().row_data(index))
            .ok_or("Optimize Objective index has no preset")?;
        let value = self
            .inspector_form_control("Optimize Objective", uiprobe::AccessibleRole::Combobox)?
            .and_then(|spot| spot.handle)
            .and_then(|handle| handle.accessible_value())
            .ok_or("Optimize Objective combo has no accessible value")?;
        if actual.as_str() != expected || value.as_str() != expected {
            return Err(format!(
                "Optimize Objective readback mismatch: model={actual:?}, combo={value:?}, expected={expected:?}"
            ));
        }
        Ok(())
    }

    /// The Optimize tab: opens the Angle Ranges panel.
    pub(super) fn optimize_open_ranges(&mut self) -> Result<(), String> {
        let button = self
            .inspector_form_control("Angle Ranges...", uiprobe::AccessibleRole::Button)?
            .ok_or("Angle Ranges button is missing")?;
        self.click_and_invoke(&button)?;
        let optimize = self.ui.global::<OptimizeModel>();
        if !optimize.get_ranges_open() {
            return Err("Angle Ranges did not open".into());
        }
        let range_rows = optimize.get_range_rows();
        let row = range_rows
            .iter()
            .find(|row| !row.driven)
            .ok_or("Angle Ranges has no editable row to inspect")?;
        for (label, expected) in [
            (
                format!("Lowest Angle for Tier {}", row.tier_number),
                row.min_text.to_string(),
            ),
            (
                format!("Highest Angle for Tier {}", row.tier_number),
                row.max_text.to_string(),
            ),
        ] {
            let field = self
                .inspector_form_control(&label, uiprobe::AccessibleRole::TextInput)?
                .ok_or_else(|| format!("{label} is not visible in Angle Ranges"))?;
            let actual = field
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_value)
                .ok_or_else(|| format!("{label} has no accessible value"))?;
            if actual != expected {
                return Err(format!("{label} changed while being inspected: {actual:?}"));
            }
            self.glide_to(field.center())?;
            self.hold(pacing::TARGET_HOLD)?;
        }
        if !optimize.get_ranges_open() {
            return Err("Angle Ranges closed before its rows were shown".into());
        }
        let hide = self
            .inspector_form_control("Hide Angle Ranges", uiprobe::AccessibleRole::Button)?
            .ok_or("Hide Angle Ranges button is missing")?;
        self.click_and_invoke(&hide)?;
        if optimize.get_ranges_open() {
            return Err("Angle Ranges did not close after its rows were shown".into());
        }
        Ok(())
    }

    /// The Optimize tab: types a short budget and two candidates to keep, clicks Optimize and
    /// waits for the results.
    pub(super) fn optimize_run_search(&mut self) -> Result<(), String> {
        let budget_field = self
            .inspector_form_control("Search Budget", uiprobe::AccessibleRole::TextInput)?
            .ok_or("Search Budget field is not visible")?;
        self.click_and_type(&budget_field, "100")?;
        let editor = self.ui.global::<EditorModel>();
        if editor.get_optimize_budget_text().as_str() != "100" {
            return Err(format!(
                "Search Budget model readback mismatch: {:?}",
                editor.get_optimize_budget_text()
            ));
        }
        let optimize = self.ui.global::<OptimizeModel>();
        let spinbox = self
            .inspector_form_control(
                "Number of Candidates to Keep",
                uiprobe::AccessibleRole::Spinbox,
            )?
            .ok_or("Number of Candidates to Keep spin box is not visible")?;
        if !spinbox.usable(self.view().size) {
            return Err("Number of Candidates to Keep spin box is outside the viewport".into());
        }
        // Slint 1.18.1's SpinBox exposes one Spinbox accessibility node; its internal
        // TextInput is deliberately role=none. SpinBox forwards focus to that editor, so
        // click_and_type sends real pointer and key events through the visible root control.
        self.click_and_type(&spinbox, "2")?;
        let spinbox = self
            .inspector_form_control(
                "Number of Candidates to Keep",
                uiprobe::AccessibleRole::Spinbox,
            )?
            .ok_or("Number of Candidates to Keep spin box disappeared")?;
        let spinbox_value = spinbox
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_value)
            .ok_or("Number of Candidates to Keep spin box has no value readback")?;
        if optimize.get_candidates() != 2 || spinbox_value != "2" {
            return Err(format!(
                "Optimize candidates readback mismatch: model={}, spin box={spinbox_value:?}",
                optimize.get_candidates()
            ));
        }
        let button = self
            .inspector_form_control("Optimize", uiprobe::AccessibleRole::Button)?
            .ok_or("Optimize button is not visible")?;
        self.click_and_invoke(&button)?;
        if !pacing::wait_lapse(&mut self.rec, OPTIMIZE_TIMEOUT, || {
            !editor.get_optimize_running() && optimize.get_has_results()
        })? {
            return Err(format!(
                "Optimize search did not finish in time: running={}, status={:?}",
                editor.get_optimize_running(),
                editor.get_optimize_status()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// The Optimize tab: clicks the first candidate.
    pub(super) fn optimize_pick_candidate(&mut self) -> Result<(), String> {
        let optimize = self.ui.global::<OptimizeModel>();
        let row = optimize
            .get_candidate_rows()
            .row_data(0)
            .ok_or("Optimize search returned no first candidate")?;
        let label = format!("Candidate {}, score {}", row.rank, row.score);
        let candidate = self
            .inspector_form_control(&label, uiprobe::AccessibleRole::ListItem)?
            .ok_or_else(|| format!("{label} is not visible"))?;
        self.click_and_invoke(&candidate)?;
        pump(60);
        if optimize.get_selected_candidate() != 0 {
            return Err("Candidate 1 was not selected".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// The Optimize tab: clicks Apply This Candidate and waits for it to finish.
    pub(super) fn optimize_apply_candidate(&mut self) -> Result<(), String> {
        let apply = self
            .inspector_form_control("Apply This Candidate", uiprobe::AccessibleRole::Button)?
            .ok_or("Apply This Candidate button is not visible")?;
        self.click_and_invoke(&apply)?;
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !editor.get_optimize_running()) {
            return Err("Apply This Candidate did not finish".into());
        }
        Ok(())
    }

    /// Opens the Retarget dialog and chooses `material` as the target material.
    fn retarget_choose_material(&mut self, material: &str) -> Result<(), String> {
        self.press_label("Retarget...")?;
        let retarget = self.ui.global::<RetargetModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || retarget.get_is_open()) {
            return Err("Retarget dialog did not open".into());
        }
        let index = retarget
            .get_material_options()
            .iter()
            .position(|option| option.as_str() == material)
            .and_then(|at| i32::try_from(at).ok())
            .ok_or_else(|| format!("{material} is not in Retarget material options"))?;
        let combo = self.retarget_combo("Target Material")?;
        let current = retarget.get_target_material_index();
        self.click_and_invoke(&combo)?;
        let key = if index < current { "Up" } else { "Down" };
        for _ in 0..(index - current).abs() {
            self.key(key)?;
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            retarget.get_is_open()
                && self
                    .verify_retarget_combo("Target Material", material)
                    .is_ok()
        }) {
            return Err(format!(
                "Target Material did not select {material} before the combo was confirmed"
            ));
        }
        // Close the menu only after the real ComboBox and its resolved proposal agree.
        self.key("Return")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            retarget.get_is_open()
                && self
                    .verify_retarget_combo("Target Material", material)
                    .is_ok()
        }) {
            return Err(format!(
                "Target Material did not resolve to {material} after selection"
            ));
        }
        Ok(())
    }

    /// Clicks Apply in the Retarget dialog and waits for it to close.
    fn retarget_apply(&mut self, expected_material: &str) -> Result<(), String> {
        let retarget = self.ui.global::<RetargetModel>();
        if !retarget.get_is_open() {
            return Err("Retarget dialog closed before Apply".into());
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            retarget.get_validity_state() == 2
                && self
                    .verify_retarget_combo("Target Material", expected_material)
                    .is_ok()
        }) {
            return Err(format!(
                "Retarget proposal is not valid for {expected_material} before Apply"
            ));
        }
        let apply = self.scoped_button("RetargetDialog", "Apply")?;
        self.click_and_invoke(&apply)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !retarget.get_is_open()) {
            return Err("Retarget Apply did not close dialog".into());
        }
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let index = editor.get_material_combo_index();
            usize::try_from(index)
                .ok()
                .and_then(|at| editor.get_material_combo_options().row_data(at))
                .is_some_and(|name| name.as_str() == expected_material)
        }) {
            let index = editor.get_material_combo_index();
            let actual = usize::try_from(index)
                .ok()
                .and_then(|at| editor.get_material_combo_options().row_data(at));
            return Err(format!(
                "Retarget Apply material readback mismatch: expected {expected_material:?}, observed {actual:?}"
            ));
        }
        Ok(())
    }

    /// Retarget in Shift mode to topaz: waits for Valid, then applies.
    pub(super) fn retarget_shift_to_topaz(&mut self) -> Result<(), String> {
        self.retarget_choose_material("Topaz")?;
        let retarget = self.ui.global::<RetargetModel>();
        if retarget.get_mode_index() != 0 {
            let shift = self.scoped_button("RetargetDialog", "1. Shift")?;
            self.click_and_invoke(&shift)?;
        }
        if retarget.get_mode_index() != 0 {
            return Err("Retarget Shift mode is not active".into());
        }
        pump(60);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || retarget.get_validity_state() == 2) {
            return Err(format!(
                "Retarget to Topaz did not become Valid, state={}",
                retarget.get_validity_state()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.retarget_apply("Topaz")
    }

    /// Retarget in Optimize mode to sapphire: searches, picks the first candidate, applies.
    pub(super) fn retarget_optimize_to_sapphire(&mut self) -> Result<(), String> {
        self.retarget_choose_material("Sapphire")?;
        let retarget = self.ui.global::<RetargetModel>();
        if retarget.get_mode_index() != 1 {
            let optimize = self.scoped_button("RetargetDialog", "2. Optimize")?;
            self.click_and_invoke(&optimize)?;
        }
        if retarget.get_mode_index() != 1 {
            return Err("Retarget Optimize mode is not active".into());
        }
        let objective = retarget.get_objective_options();
        let range = retarget.get_range_options();
        let effort = retarget.get_effort_options();
        let balanced = objective
            .iter()
            .position(|option| option.as_str() == "Balanced")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Balanced objective is unavailable")?;
        let six_degrees = range
            .iter()
            .position(|option| option.as_str() == "6 Degrees Either Side")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("6 Degrees Either Side range is unavailable")?;
        let quick = effort
            .iter()
            .position(|option| option.as_str() == "Quick (100 Steps)")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Quick (100 Steps) effort is unavailable")?;
        for (label, expected, current) in [
            ("Objective", "Balanced", retarget.get_objective_index()),
            ("Range", "6 Degrees Either Side", retarget.get_range_index()),
        ] {
            let wanted = if label == "Objective" {
                balanced
            } else {
                six_degrees
            };
            if current != wanted {
                self.retarget_select_setting(label, wanted)?;
            }
            self.verify_retarget_combo(label, expected)?;
        }
        let effort_spot = self.retarget_combo("Effort")?;
        self.click_and_invoke(&effort_spot)?;
        self.select_open_combo(retarget.get_effort_index(), quick)?;
        self.verify_retarget_combo("Effort", "Quick (100 Steps)")?;
        pump(60);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || retarget.get_validity_state() != 1) {
            return Err("Retarget Shift geometry check did not finish before Search".into());
        }
        self.hold(Duration::from_secs(1))?;
        self.press_label("Search for Better Angles")?;
        if !pacing::wait_real(Duration::from_secs(30), || {
            !retarget.get_is_busy() && retarget.get_candidates().row_count() > 0
        }) {
            return Err("Retarget Optimize search did not complete with candidates".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let first = retarget
            .get_candidates()
            .row_data(0)
            .ok_or("Retarget Candidate 1 is missing")?;
        let prefix = format!("{}. {}, score ", first.rank, first.label);
        let candidate = targets::run_label_start(&self.view(), &prefix)
            .ok_or("Retarget Candidate 1 is not visible")?;
        self.click_and_invoke(&candidate)?;
        if retarget.get_selected_candidate() != 0 {
            return Err("Retarget Candidate 1 was not selected".into());
        }
        self.retarget_apply("Sapphire")
    }

    fn retarget_combo(&self, label: &str) -> Result<targets::Spot, String> {
        let mut matches: Vec<_> = self
            .scoped_spots("RetargetDialog", label)
            .into_iter()
            .filter(|spot| {
                spot.handle
                    .as_ref()
                    .and_then(crate::support::ElementHandle::accessible_role)
                    == Some(uiprobe::AccessibleRole::Combobox)
            })
            .collect();
        if matches.len() != 1 {
            return Err(format!(
                "Retarget {label} combo has {} accessible matches",
                matches.len()
            ));
        }
        let combo = matches.remove(0);
        let view = self.view();
        let dialog = view
            .root
            .query_descendants()
            .match_inherits("RetargetDialog")
            .find_first()
            .ok_or("Retarget dialog is missing")?;
        let viewport = dialog
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .map(|scroll| targets::Spot::of(&scroll))
            .ok_or("Retarget dialog body ScrollView is missing")?;
        combo
            .clipped_to(&viewport, view.size)
            .ok_or_else(|| format!("Retarget {label} combo is outside the visible dialog body"))
    }

    fn retarget_select_setting(&mut self, label: &str, wanted: i32) -> Result<(), String> {
        let retarget = self.ui.global::<RetargetModel>();
        let current = match label {
            "Objective" => retarget.get_objective_index(),
            "Range" => retarget.get_range_index(),
            _ => return Err(format!("unsupported Retarget setting {label}")),
        };
        let combo = self.retarget_combo(label)?;
        self.click_and_invoke(&combo)?;
        self.select_open_combo(current, wanted)
    }

    fn verify_retarget_combo(&self, label: &str, expected: &str) -> Result<(), String> {
        let retarget = self.ui.global::<RetargetModel>();
        let (index, option) = match label {
            "Target Material" => (
                retarget.get_target_material_index(),
                retarget.get_material_options(),
            ),
            "Objective" => (
                retarget.get_objective_index(),
                retarget.get_objective_options(),
            ),
            "Range" => (retarget.get_range_index(), retarget.get_range_options()),
            "Effort" => (retarget.get_effort_index(), retarget.get_effort_options()),
            _ => return Err(format!("unsupported Retarget setting {label}")),
        };
        let actual = usize::try_from(index)
            .ok()
            .and_then(|at| option.row_data(at))
            .ok_or_else(|| format!("Retarget {label} index {index} has no option"))?;
        let readback = self
            .retarget_combo(label)?
            .handle
            .and_then(|handle| handle.accessible_value())
            .ok_or_else(|| format!("Retarget {label} combo has no accessible value"))?;
        if actual.as_str() != expected || readback.as_str() != expected {
            return Err(format!(
                "Retarget {label} readback mismatch: model={actual:?}, combo={readback:?}, expected={expected:?}"
            ));
        }
        if label == "Target Material"
            && (retarget.get_target_material_name().as_str() != expected
                || !retarget.get_target_material_error().is_empty())
        {
            return Err(format!(
                "Retarget Target Material resolution mismatch: name={:?}, error={:?}, expected={expected:?}",
                retarget.get_target_material_name(),
                retarget.get_target_material_error()
            ));
        }
        Ok(())
    }

    /// The Angle Sweep dialog: sweeps Pavilion Main from 38 to 42 degrees in steps of 1, picks the
    /// 42 degree row, uses it and closes the dialog.
    pub(super) fn sweep_pavilion_and_use(&mut self) -> Result<(), String> {
        self.open_sweep()?;
        let sweep = self.ui.global::<SweepModel>();
        let tier_idx = sweep
            .get_tier_names()
            .iter()
            .position(|name| name.contains("Pavilion Main"))
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Pavilion Main is not offered for Angle Sweep")?;
        let tier_combo = targets::run_label(&self.view(), "Tier to Sweep")
            .ok_or("Tier to Sweep combo is missing")?;
        let current = sweep.get_tier_index();
        self.click_and_invoke(&tier_combo)?;
        self.select_open_combo(current, tier_idx)?;
        if sweep.get_tier_index() != tier_idx {
            return Err("Angle Sweep tier did not select Pavilion Main".into());
        }
        for (label, value) in [
            ("From (degrees)", "38"),
            ("To (degrees)", "42"),
            ("Step (degrees)", "1"),
        ] {
            let field = targets::text_input(&self.view(), label)
                .ok_or_else(|| format!("Angle Sweep {label} field is missing"))?;
            self.click_and_type(&field, value)?;
        }
        if sweep.get_from_text() != "38"
            || sweep.get_to_text() != "42"
            || sweep.get_step_text() != "1"
        {
            return Err("Angle Sweep range did not read back as 38-42 by 1".into());
        }
        pump(60);
        self.press_label("Run Sweep")?;
        if !pacing::wait_real(Duration::from_secs(30), || {
            !sweep.get_running() && sweep.get_has_results()
        }) {
            return Err("Angle sweep did not complete in time".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let row_idx = sweep
            .get_rows()
            .iter()
            .position(|r| r.angle.as_str() == "42.00")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Angle 42.00 row not found in sweep results")?;
        let selected = targets::run_label_start(&self.view(), "Angle 42.00")
            .ok_or("Angle 42.00 result row is not visible")?;
        self.click_and_invoke(&selected)?;
        if sweep.get_selected_row() != row_idx {
            return Err("Angle Sweep row click did not select 42.00".into());
        }
        let use_label = sweep.get_use_label().to_string();
        let use_btn = self.scoped_button("SweepDialog", &use_label)?;
        self.click_and_invoke(&use_btn)?;
        pump(60);
        let close = self.scoped_button("SweepDialog", "Close")?;
        self.click_and_invoke(&close)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !sweep.get_dialog_open()) {
            return Err("Angle Sweep Close click did not close the dialog".into());
        }
        Ok(())
    }
}
