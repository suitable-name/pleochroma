//! Real-input demonstrations for tutorial steps whose instructions ask for a gesture rather
//! than a model edit. The registry lives in `scripts.rs`.

use crate::{director::Film, pacing, targets};
use pleochroma_cut::{CuttingModeModel, EditorModel};
use pleochroma_editor::guide::TierEntry;
use slint::{ComponentHandle, Model};
use std::time::Duration;

impl Film<'_> {
    /// Changes the Quick Add Table's scale in the Tier form, then adds a real Pavilion ring.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(super) fn film_quick_add_make_them_yours(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before_count = editor.get_tiers().row_count();
        let table = editor
            .get_tiers()
            .iter()
            .find(|row| row.name.as_str() == "Table")
            .ok_or("Quick Add Table row is missing")?;
        let initial_scale = table.constraint_text.parse::<f64>().map_err(|_| {
            format!(
                "Quick Add Table has unreadable scale {:?}",
                table.constraint_text
            )
        })?;
        if table.constraint_kind != 2
            || !initial_scale.is_finite()
            || (initial_scale - 0.32).abs() > 1e-8
        {
            return Err(format!(
                "Quick Add Table did not start pinned at scale 0.32: kind={}, value={:?}",
                table.constraint_kind, table.constraint_text
            ));
        }

        self.film_tier(&TierEntry::of("Table").meets(2, "0.30"))?;
        let table = editor
            .get_tiers()
            .iter()
            .find(|row| row.name.as_str() == "Table")
            .ok_or("Table disappeared after changing its scale")?;
        let table_scale = table.constraint_text.parse::<f64>().map_err(|_| {
            format!(
                "saved Table scale is unreadable: {:?}",
                table.constraint_text
            )
        })?;
        if editor.get_tiers().row_count() != before_count
            || table.constraint_kind != 2
            || !table_scale.is_finite()
            || (table_scale - 0.30).abs() > 1e-8
        {
            return Err(format!(
                "Table scale edit did not commit as 0.30 without changing row count: count={}, kind={}, value={:?}",
                editor.get_tiers().row_count(),
                table.constraint_kind,
                table.constraint_text
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;

        self.film_tier(&TierEntry::add("-40", 2, "0.7", "Pavilion Main", "0:12:96"))?;
        let rows = editor.get_tiers();
        let added = rows
            .iter()
            .find(|row| row.name.as_str() == "Pavilion Main")
            .ok_or("Pavilion Main ring was not added")?;
        let angle = added
            .angle_full
            .parse::<f64>()
            .map_err(|_| format!("Pavilion Main angle is unreadable: {:?}", added.angle_full))?;
        let scale = added.constraint_text.parse::<f64>().map_err(|_| {
            format!(
                "Pavilion Main scale is unreadable: {:?}",
                added.constraint_text
            )
        })?;
        let indices = added
            .indices_full
            .split(',')
            .map(str::trim)
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                format!(
                    "Pavilion Main indices are not readable: {:?}",
                    added.indices_full
                )
            })?;
        let expected_indices = (0..8).map(|n| f64::from(n * 12)).collect::<Vec<_>>();
        if rows.row_count() != before_count + 1
            || added.kind != 0
            || added.block.as_str() != "Pavilion"
            || added.constraint_kind != 2
            || !angle.is_finite()
            || (angle.abs() - 40.0).abs() > 1e-8
            || !scale.is_finite()
            || (scale - 0.7).abs() > 1e-8
            || indices.len() != expected_indices.len()
            || indices.iter().any(|index| !index.is_finite())
            || indices
                .iter()
                .zip(expected_indices)
                .any(|(actual, expected)| (actual - expected).abs() > 1e-8)
        {
            return Err(format!(
                "Pavilion Main ring readback mismatch: count={}, kind={}, block={}, angle={:?}, indices={:?}, constraint_kind={}, scale={:?}",
                rows.row_count(),
                added.kind,
                added.block,
                added.angle_full,
                added.indices_full,
                added.constraint_kind,
                added.constraint_text
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Starts a fresh tier draft with the actual pinned + Add Tier button.
    pub(super) fn film_add_tier_draft(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let add = targets::role_label(
            &self.view(),
            "+ Add Tier",
            uiprobe::AccessibleRole::Button,
            Some("TierTableToolbar"),
        )
        .ok_or("TierTableToolbar + Add Tier button is missing")?;
        self.click_and_invoke(&add)?;
        if editor.get_tiers().row_count() != before {
            return Err("+ Add Tier changed the saved tier count".into());
        }
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor.get_selected_tier_index() == -1
        }) {
            return Err(format!(
                "+ Add Tier did not clear the loaded row: index={}",
                editor.get_selected_tier_index()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Hovers the Indices caption of the real `FormRow` until its tooltip is visible.
    pub(super) fn film_indices_tip(&mut self) -> Result<(), String> {
        self.film_indices_tip_popup()
    }

    /// Opens Steps / Mirror with its toolbar toggle.
    pub(super) fn film_open_steps_panel(&mut self) -> Result<(), String> {
        if targets::role_label(
            &self.view(),
            "Ladder Name",
            uiprobe::AccessibleRole::TextInput,
            Some("TierTableToolbar"),
        )
        .is_some()
        {
            return Ok(());
        }
        let button = targets::role_label(
            &self.view(),
            "Steps / Mirror",
            uiprobe::AccessibleRole::Button,
            Some("TierTableToolbar"),
        )
        .ok_or("Steps / Mirror toolbar button is missing")?;
        self.click_and_invoke(&button)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            targets::role_label(
                &self.view(),
                "Ladder Name",
                uiprobe::AccessibleRole::TextInput,
                Some("TierTableToolbar"),
            )
            .is_some()
        }) {
            return Err("Steps / Mirror button did not open the panel".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Undoes and redoes existing lesson edits while the selected tier row has focus.
    /// Each key must change the rows; redo must restore the original row snapshot.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(super) fn film_undo_redo_history(
        &mut self,
        tier: &str,
        count: usize,
    ) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        self.film_select_tier(tier)?;
        let snapshot = editor
            .get_tiers()
            .iter()
            .map(|row| {
                (
                    row.name.to_string(),
                    row.angle_full.to_string(),
                    row.indices_full.to_string(),
                )
            })
            .collect::<Vec<_>>();
        let mut previous = snapshot.clone();
        for undone in 1..=count {
            self.key("Ctrl+Z")?;
            let changed = pacing::wait_real(pacing::START_TIMEOUT, || {
                let now = editor
                    .get_tiers()
                    .iter()
                    .map(|row| {
                        (
                            row.name.to_string(),
                            row.angle_full.to_string(),
                            row.indices_full.to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                now != previous
            });
            if !changed {
                return Err(format!("Ctrl+Z {undone}/{count} did not change tier rows"));
            }
            previous = editor
                .get_tiers()
                .iter()
                .map(|row| {
                    (
                        row.name.to_string(),
                        row.angle_full.to_string(),
                        row.indices_full.to_string(),
                    )
                })
                .collect();
            self.hold(Duration::from_millis(1200))?;
        }
        for redone in 1..=count {
            self.key("Ctrl+Y")?;
            let changed = pacing::wait_real(pacing::START_TIMEOUT, || {
                let now = editor
                    .get_tiers()
                    .iter()
                    .map(|row| {
                        (
                            row.name.to_string(),
                            row.angle_full.to_string(),
                            row.indices_full.to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                now != previous
            });
            if !changed {
                return Err(format!("Ctrl+Y {redone}/{count} did not change tier rows"));
            }
            previous = editor
                .get_tiers()
                .iter()
                .map(|row| {
                    (
                        row.name.to_string(),
                        row.angle_full.to_string(),
                        row.indices_full.to_string(),
                    )
                })
                .collect();
            self.hold(Duration::from_millis(1200))?;
        }
        let restored = editor
            .get_tiers()
            .iter()
            .map(|row| {
                (
                    row.name.to_string(),
                    row.angle_full.to_string(),
                    row.indices_full.to_string(),
                )
            })
            .collect::<Vec<_>>();
        if restored != snapshot {
            return Err("undo/redo left tier rows different from the captured state".into());
        }
        let visible_rows: Vec<String> = if tier == "Step2" && count == 2 {
            (1..=3).map(|rung| format!("Step{rung}")).collect()
        } else {
            vec![tier.to_owned()]
        };
        for name in visible_rows {
            self.film_reveal_tier(&name)?;
            self.hold(Duration::from_millis(1200))?;
        }
        let after_reveal = editor
            .get_tiers()
            .iter()
            .map(|row| {
                (
                    row.name.to_string(),
                    row.angle_full.to_string(),
                    row.indices_full.to_string(),
                )
            })
            .collect::<Vec<_>>();
        if after_reveal != snapshot {
            return Err("revealing restored rows changed the tier model".into());
        }
        Ok(())
    }

    /// Moves the latest mirrored crown copy up, undoes the move, then undoes the mirror.
    pub(super) fn film_move_and_undo_mirror_copy(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let source = editor
            .get_tiers()
            .iter()
            .find(|row| row.name.as_str() == "Crown Main")
            .ok_or("Crown Main source row is missing")?;
        let copy = editor
            .get_tiers()
            .iter()
            .find(|row| {
                row.name.as_str() == "Crown_Mainb"
                    && row.angle_full.as_str() == source.angle_full.as_str()
                    && row.indices_full.as_str() == source.indices_full.as_str()
            })
            .ok_or("the Crown_Mainb copy from the preceding lesson step is missing")?
            .name
            .to_string();
        let original_position = editor
            .get_tiers()
            .iter()
            .position(|row| row.name.as_str() == copy)
            .ok_or("mirrored copy row position is missing")?;
        self.film_select_tier(&copy)?;
        self.key("Alt+Up")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor
                .get_tiers()
                .iter()
                .position(|row| row.name.as_str() == copy)
                == original_position.checked_sub(1)
        }) {
            return Err("Alt+Up did not move the mirrored copy up one row".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Ctrl+Z")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor
                .get_tiers()
                .iter()
                .position(|row| row.name.as_str() == copy)
                == Some(original_position)
        }) {
            return Err("Ctrl+Z did not undo the copy's move and restore its original row".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Ctrl+Z")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            !editor
                .get_tiers()
                .iter()
                .any(|row| row.name.as_str() == copy)
        }) {
            return Err("second Ctrl+Z did not remove the Crown_Mainb mirrored copy".into());
        }
        Ok(())
    }

    /// Adds one degree to Groove through the concave form and saves the edited tier.
    pub(super) fn film_save_concave_groove_angle(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before_count = editor.get_tiers().row_count();
        let row = self.film_select_tier("Groove")?;
        let field = self
            .input_spot("Angle in Degrees")?
            .ok_or("concave Angle in Degrees field is missing")?;
        // Table rows show angle magnitudes; the concave form preserves the signed
        // authored angle. Read that field so editing a pavilion groove keeps its side.
        let old = field
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_value)
            .ok_or("Groove angle field has no readable value")?;
        let old_angle = old
            .parse::<f64>()
            .map_err(|_| format!("Groove has invalid angle {old:?}"))?;
        let next = format!("{}", old_angle + 1.0);
        self.click_and_type(&field, &next)?;
        // This commit button is pinned below the scrolling form, not inside it.
        let view = self.view();
        let dock = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll"))
            .ok_or("editor dock is missing")?;
        let save = targets::role_label(
            &view,
            "Save Concave Tier",
            uiprobe::AccessibleRole::Button,
            Some("EditorInspector"),
        )
        .and_then(|spot| spot.clipped_to(&dock, view.size))
        .ok_or("Save Concave Tier button is outside the editor dock")?;
        self.click_and_invoke(&save)?;
        let saved = editor
            .get_tiers()
            .row_data(usize::try_from(row).map_err(|_| "Groove row index is invalid")?)
            .ok_or("Groove disappeared after save")?;
        let actual = saved
            .angle_full
            .parse::<f64>()
            .map_err(|_| format!("saved Groove has invalid angle {:?}", saved.angle_full))?;
        if (actual - (old_angle + 1.0).abs()).abs() > 1e-6 {
            return Err(format!(
                "Save Concave Tier left Groove at {actual}, expected {}",
                old_angle + 1.0
            ));
        }
        if editor.get_tiers().row_count() != before_count {
            return Err(
                "Save Concave Tier changed the tier count instead of editing Groove".into(),
            );
        }
        Ok(())
    }

    /// Opens Cutting Mode, holds its tool instructions on screen, then closes with Escape.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(super) fn film_open_cutting_mode_briefly(&mut self) -> Result<(), String> {
        let model = self.ui.global::<CuttingModeModel>();
        let editor = self.ui.global::<EditorModel>();
        let tiers = editor.get_tiers();
        let mut concave_targets = Vec::new();
        for (name, tool_code) in [("Dimple", "SPH"), ("Wheel", "DSC")] {
            let matches = (0..tiers.row_count())
                .filter_map(|index| tiers.row_data(index))
                .filter(|row| row.kind == 1 && row.name.as_str() == name)
                .collect::<Vec<_>>();
            let row = match matches.as_slice() {
                [row] => row,
                [] => return Err(format!("the expected concave tier {name} is missing")),
                _ => return Err(format!("the expected concave tier {name} is ambiguous")),
            };
            concave_targets.push((name, tool_code, row.code.to_string()));
        }
        let command = targets::role_label(
            &self.view(),
            "Cut Mode",
            uiprobe::AccessibleRole::Button,
            Some("AnalysisGroup"),
        )
        .ok_or("Cutting Mode command is missing")?;
        self.click_and_invoke(&command)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            model.get_open() && !model.get_busy()
        }) {
            return Err(format!(
                "Cutting Mode did not open: open={}, busy={}, error={:?}",
                model.get_open(),
                model.get_busy(),
                model.get_error()
            ));
        }
        if !model.get_error().is_empty() {
            return Err(format!(
                "Cutting Mode could not prepare its schedule: {:?}",
                model.get_error()
            ));
        }
        let schedule = model.get_schedule();
        let mut target_positions = Vec::new();
        for (name, tool_code, expected_code) in concave_targets {
            let positions = (0..schedule.row_count())
                .filter(|index| {
                    schedule
                        .row_data(*index)
                        .is_some_and(|row| row.code.as_str() == expected_code)
                })
                .collect::<Vec<_>>();
            let position = match positions.as_slice() {
                [position] => *position,
                [] => {
                    return Err(format!(
                        "Cutting Mode schedule has no {name} row ({expected_code})"
                    ));
                }
                _ => {
                    return Err(format!(
                        "Cutting Mode schedule has duplicate code {expected_code}"
                    ));
                }
            };
            target_positions.push((name, tool_code, expected_code, position));
        }
        let mut current = usize::try_from(model.get_step_index()).map_err(|_| {
            format!(
                "Cutting Mode has invalid current index {}",
                model.get_step_index()
            )
        })?;
        let step_count = usize::try_from(model.get_step_count()).unwrap_or_default();
        if current >= step_count {
            return Err(format!(
                "Cutting Mode current index {current} is outside {step_count} steps"
            ));
        }
        let done_count = model.get_done_count();
        for (name, tool_code, expected_code, target) in target_positions {
            if target >= step_count {
                return Err(format!(
                    "Cutting Mode {name} position {target} is outside {step_count} steps"
                ));
            }
            while current != target {
                let next = if current < target {
                    current + 1
                } else {
                    current - 1
                };
                self.key(if current < target { "Right" } else { "Left" })?;
                if !pacing::wait_real(pacing::START_TIMEOUT, || {
                    usize::try_from(model.get_step_index()).ok() == Some(next)
                        && !model.get_busy()
                        && model.get_error().is_empty()
                        && model.get_done_count() == done_count
                }) {
                    return Err(format!(
                        "Cutting Mode navigation failed at {expected_code}: expected step {}, got {} (error: {:?})",
                        next + 1,
                        model.get_step_index() + 1,
                        model.get_error()
                    ));
                }
                current = next;
            }
            let tool = model.get_tool_text();
            let meet = model.get_meet_text();
            let meet_prefix = format!("{name}:");
            if model.get_tier_name().as_str() != expected_code
                || !meet.as_str().starts_with(meet_prefix.as_str())
                || !tool.as_str().contains(tool_code)
            {
                return Err(format!(
                    "Cutting Mode did not show the expected {name} concave tool line: code={:?}, instruction={:?}, tool={:?}",
                    model.get_tier_name(),
                    meet,
                    tool
                ));
            }
            self.hold(pacing::RESULT_HOLD)?;
        }
        self.key("Escape")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !model.get_open()) {
            return Err("Escape did not close Cutting Mode".into());
        }
        Ok(())
    }
}
