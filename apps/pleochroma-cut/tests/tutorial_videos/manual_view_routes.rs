//! Recorded demonstrations for explanatory steps which also ask the learner to act.
//! Every activation is one pointer click or key chord; readbacks never change app state.

use crate::{director::Film, pacing, targets};
use pleochroma_cut::{CommandPaletteModel, EditorModel, SolidPreviewModel, harness_support};
use slint::{ComponentHandle, Model};
use uiprobe::AccessibleRole;

impl Film<'_> {
    pub(super) fn film_palette_introduction(&mut self) -> Result<(), String> {
        let palette = self.ui.global::<CommandPaletteModel>();
        self.key("Ctrl+K")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || palette.get_open()) {
            return Err("Ctrl+K did not open the introductory command search".into());
        }
        let search = targets::text_input(&self.view(), "Search Commands")
            .ok_or("introductory Search Commands input is missing")?;
        self.click_and_type(&search, "sv")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            palette.get_query() == "sv" && palette.get_rows().row_count() > 1
        }) {
            return Err("sv did not produce command search results".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let before = palette.get_selected();
        self.key("Down")?;
        if palette.get_selected() == before {
            return Err("Down did not move the palette highlight".into());
        }
        self.hold(pacing::MENU_HOLD)?;
        self.key("Up")?;
        if palette.get_selected() != before {
            return Err("Up did not return the palette highlight".into());
        }
        self.key("Escape")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || !palette.get_open()) {
            return Err("Escape did not close the introductory command search".into());
        }
        Ok(())
    }

    pub(super) fn film_restore_auto_solve(&mut self) -> Result<(), String> {
        let targets::Resolved::Found(combo) = targets::resolve(&self.view(), "auto_solve") else {
            return Err("Auto-Solve combo is missing".into());
        };
        self.click_and_invoke(&combo)?;
        // SolveGroup's list is Off, 150 ms, 300 ms, 1 s, 3 s. Reach its beginning
        // with bounded arrows, then visibly choose the recommended 300 ms setting.
        for key in ["Up", "Up", "Up", "Up", "Down", "Down", "Return"] {
            self.key(key)?;
        }
        if self.ui.global::<EditorModel>().get_auto_solve_budget_ms() != 300 {
            return Err("Auto-Solve did not read back as 300 ms".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    pub(super) fn film_diagram_return_and_undo(&mut self) -> Result<(), String> {
        let solid = self.ui.global::<SolidPreviewModel>();
        let label = match solid.get_diagram_enlarged_panel() {
            0 => "Crown",
            1 => "Pavilion",
            2 => "Profile",
            other => return Err(format!("no enlarged Diagram panel to restore: {other}")),
        };
        let pill = targets::role_label(
            &self.view(),
            label,
            AccessibleRole::Button,
            Some("SolidViewportView"),
        )
        .ok_or("lit Diagram panel pill is missing")?;
        self.click_and_invoke(&pill)?;
        if solid.get_diagram_enlarged_panel() != -1 {
            return Err("the lit Diagram pill did not restore all three panels".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        // Focus the table before the global undo chord; a focused form field would
        // consume Ctrl+Z as a text edit instead of undoing the handle drag.
        let row = usize::try_from(self.film_select_tier("Crown Main")?)
            .map_err(|_| "Crown Main has no row")?;
        let editor = self.ui.global::<EditorModel>();
        let before = editor
            .get_tiers()
            .row_data(row)
            .ok_or("Crown Main disappeared")?
            .angle_full;
        self.key("Ctrl+Z")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor
                .get_tiers()
                .row_data(row)
                .is_some_and(|r| r.angle_full != before)
        }) {
            return Err("Ctrl+Z did not undo the Diagram angle change".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    pub(super) fn film_live_mouse_controls(&mut self) -> Result<(), String> {
        let area = targets::run(
            &self.view(),
            &targets::Query::Id("GemViewportView::touch_area"),
        )
        .ok_or("Live Render mouse surface is missing")?;
        let center = area.center();
        let before = harness_support::camera_pose();
        self.gesture(
            (center.0 - 35.0, center.1),
            (center.0 + 35.0, center.1 + 20.0),
            None,
            false,
            false,
        )?;
        let orbited = harness_support::camera_pose();
        if (orbited.0 - before.0).abs() < 0.01 && (orbited.1 - before.1).abs() < 0.01 {
            return Err("Live Render drag did not turn the camera".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.rec.scroll_at(self.ui.window(), center, -120.0)?;
        if (harness_support::camera_pose().2 - orbited.2).abs() < 0.01 {
            return Err("Live Render wheel did not zoom the camera".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let camera_before_light = harness_support::camera_pose();
        let light_before = harness_support::light_pose();
        self.gesture(
            (center.0 - 25.0, center.1),
            (center.0 + 25.0, center.1 - 15.0),
            None,
            false,
            true,
        )?;
        let light_after = harness_support::light_pose();
        if (light_after.0 - light_before.0).abs() < 0.01
            || harness_support::camera_pose() != camera_before_light
        {
            return Err("Shift-drag did not move only the Live Render light".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        for (label, yaw, pitch, distance) in [
            ("Front", 0.0, 0.0, camera_before_light.2),
            (
                "Top",
                0.0,
                std::f32::consts::FRAC_PI_2,
                camera_before_light.2,
            ),
            ("Reset Camera", 0.60, 0.45, 2.4),
        ] {
            let button = targets::role_label(
                &self.view(),
                label,
                AccessibleRole::Button,
                Some("GemViewportView"),
            )
            .ok_or_else(|| format!("Live Render {label} button is missing"))?;
            self.click_and_invoke(&button)?;
            let pose = harness_support::camera_pose();
            if (pose.0 - yaw).abs() > 0.001
                || (pose.1 - pitch).abs() > 0.001
                || (pose.2 - distance).abs() > 0.001
            {
                return Err(format!("Live Render {label} left camera at {pose:?}"));
            }
            self.hold(pacing::RESULT_HOLD)?;
        }
        // Reset Camera also restores the light, whose defaults are exposed in the UI.
        let light = harness_support::light_pose();
        let settings = self.ui.global::<pleochroma_cut::SettingsModel>();
        if (light.0 - settings.get_light_yaw_deg().to_radians()).abs() > 0.001
            || (light.1 - settings.get_light_pitch_deg().to_radians()).abs() > 0.001
            || (light.0 - light_after.0).abs() < 0.01
        {
            return Err("Reset Camera did not restore the light direction".into());
        }
        Ok(())
    }
}
