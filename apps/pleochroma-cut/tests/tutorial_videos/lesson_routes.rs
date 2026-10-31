//! Filmed routes of single lesson steps that take more than one gesture, kept out of the
//! registry in `scripts.rs` so that its match stays a list of arms.

use crate::{
    director::Film,
    pacing,
    targets::{self, Query},
};
use pleochroma_cut::{EditorModel, GuideModel, ManipulateModel, VariantsModel};
use pleochroma_editor::guide::TierEntry;
use slint::{ComponentHandle, Model};
use std::time::Duration;

fn has_numeric_value(value: &str) -> bool {
    let token = value.split_whitespace().next().unwrap_or_default();
    token
        .strip_suffix('%')
        .unwrap_or(token)
        .parse::<f64>()
        .is_ok_and(f64::is_finite)
}

impl Film<'_> {
    /// Shows results for solving-yield-carat/3 and solving-raw-text/1–2, after their existing
    /// actions produce them. These read-only hooks never repeat the guide's Solve or edit.
    pub(crate) fn film_completed_step_result(&mut self, n: usize) -> Result<(), String> {
        let guide = self.ui.global::<GuideModel>();
        let id = guide.get_guide_id().to_string();
        match (id.as_str(), n) {
            ("solving-yield-carat", 3) => self.film_yield_readouts(),
            ("solving-raw-text", 1) => self.film_raw_text_angle(1, 36.0),
            ("solving-raw-text", 2) => self.film_raw_text_angle(2, 34.5),
            _ => Ok(()),
        }
    }

    fn require_completed_guide_step(&self, n: usize, guide_id: &str) -> Result<(), String> {
        let guide = self.ui.global::<GuideModel>();
        let expected = i32::try_from(n).map_err(|_| format!("invalid guide step {n}"))?;
        if guide.get_guide_id().as_str() != guide_id
            || guide.get_step_index() != expected
            || !guide.get_step_done()
        {
            return Err(format!(
                "{guide_id} step {} result cannot be shown: current guide={:?}, step={}, done={}",
                n + 1,
                guide.get_guide_id(),
                guide.get_step_index(),
                guide.get_step_done()
            ));
        }
        Ok(())
    }

    fn film_yield_readouts(&mut self) -> Result<(), String> {
        const GUIDE_ID: &str = "solving-yield-carat";
        self.require_completed_guide_step(3, GUIDE_ID)?;
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 1 {
            self.film_inspector_tab(1)?;
        }
        let before = (
            editor.get_volumetric_yield_text().to_string(),
            editor.get_carat_weight_text().to_string(),
            editor.get_specific_gravity_used_text().to_string(),
        );
        let readouts = [
            ("Volumetric Yield", before.0.as_str()),
            ("Est. Carat Weight", before.1.as_str()),
            ("Specific Gravity Used", before.2.as_str()),
        ];
        for (name, value) in readouts {
            if value.trim().is_empty() || value.trim() == "-" || !has_numeric_value(value) {
                return Err(format!(
                    "{name} has no solved numeric result to show: {value:?}"
                ));
            }
            let line = format!("{name}: {value}");
            let spot = self
                .inspector_form_control(&line, uiprobe::AccessibleRole::Text)?
                .ok_or_else(|| {
                    if name == "Specific Gravity Used" {
                        format!("{line} is hidden; the existing Advanced prelude is required")
                    } else {
                        format!("{line} is not visible in the Preform tab")
                    }
                })?;
            let actual = spot
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_label)
                .ok_or_else(|| format!("{line} has no accessible text readback"))?;
            if actual != line {
                return Err(format!(
                    "{name} visible text mismatch: expected {line:?}, observed {actual:?}"
                ));
            }
            self.glide_to(spot.center())?;
            self.hold(Duration::from_secs(2))?;
        }
        let after = (
            editor.get_volumetric_yield_text().to_string(),
            editor.get_carat_weight_text().to_string(),
            editor.get_specific_gravity_used_text().to_string(),
        );
        if after != before {
            return Err("yield readouts changed while the result was being shown".into());
        }
        Ok(())
    }

    fn film_raw_text_angle(&mut self, n: usize, expected: f64) -> Result<(), String> {
        const GUIDE_ID: &str = "solving-raw-text";
        self.require_completed_guide_step(n, GUIDE_ID)?;
        self.film_reveal_tier("Crown Main")?;
        let tiers = self.ui.global::<EditorModel>().get_tiers();
        let index = (0..tiers.row_count())
            .find(|&i| {
                tiers
                    .row_data(i)
                    .is_some_and(|row| row.name.as_str() == "Crown Main")
            })
            .and_then(|i| i32::try_from(i).ok())
            .ok_or("Crown Main disappeared from the tier table")?;
        let row = tiers
            .row_data(usize::try_from(index).map_err(|_| "invalid Crown Main row index")?)
            .ok_or("Crown Main row data disappeared")?;
        let actual = row
            .angle_full
            .parse::<f64>()
            .map_err(|_| format!("Crown Main angle is unreadable: {:?}", row.angle_full))?;
        if (actual - expected).abs() > 1.0e-6 {
            return Err(format!(
                "Crown Main angle mismatch: expected {expected}, observed {actual}"
            ));
        }
        let angle = self.tier_row_control(index, uiprobe::AccessibleRole::Button, "Angle ")?;
        let label = angle
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_label)
            .ok_or("Crown Main ANGLE cell has no accessible label")?;
        let displayed = label
            .strip_prefix("Angle ")
            .and_then(|value| value.strip_suffix(" degrees"))
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| format!("Crown Main ANGLE label is unexpected: {label:?}"))?;
        if (displayed - expected).abs() > 0.005 {
            return Err(format!(
                "Crown Main ANGLE label mismatch: expected {expected}, observed {label:?}"
            ));
        }
        self.glide_to(angle.center())?;
        self.hold(Duration::from_secs(2))?;
        let after = self.ui.global::<EditorModel>().get_tiers();
        let observed = after
            .row_data(usize::try_from(index).map_err(|_| "invalid Crown Main row index")?)
            .ok_or("Crown Main disappeared while its angle was shown")?;
        if observed.angle_full != row.angle_full {
            return Err("Crown Main angle changed while its result was being shown".into());
        }
        Ok(())
    }

    /// The Preform tab: types a half-width of 1.8 for the rough and clicks Apply Preform.
    pub(super) fn film_preform_width(&mut self) -> Result<(), String> {
        if self.ui.global::<EditorModel>().get_inspector_tab() != 1 {
            self.film_inspector_tab(1)?;
        }
        let editor = self.ui.global::<EditorModel>();
        let field = self
            .input_spot("Half-Width (Model Units, Girdle Half-Width = 1)")?
            .ok_or("Preform Half-Width input is missing")?;
        self.click_and_type(&field, "1.8")?;
        let apply_label = "Apply Preform, changes not applied yet";
        let apply = self
            .inspector_form_control(apply_label, uiprobe::AccessibleRole::Button)?
            .ok_or("Apply Preform button did not enter the Inspector viewport")?;
        self.click_and_invoke(&apply)?;
        let actual_width = editor.get_preform_half_width().to_string();
        let width_matches = actual_width
            .trim()
            .parse::<f64>()
            .is_ok_and(|width| width.is_finite() && (width - 1.8).abs() <= 1e-6);
        if !width_matches {
            return Err(format!(
                "Apply Preform did not keep Half-Width at 1.8; got {actual_width:?}"
            ));
        }
        let guide = self.ui.global::<GuideModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
            return Err(format!(
                "Apply Preform left the lesson goal false: {}",
                guide.get_current().waiting
            ));
        }
        Ok(())
    }

    /// Opens the status strip's Details popup, reads it on screen, then closes it with Escape.
    pub(super) fn film_read_status_details(&mut self) -> Result<(), String> {
        let details = targets::role_label(
            &self.view(),
            "Details",
            uiprobe::AccessibleRole::Button,
            Some("EditorStatusStrip"),
        )
        .ok_or("Details button is missing from the status strip")?;
        self.click_and_invoke(&details)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            targets::run_label(&self.view(), "Editor Details").is_some()
        }) {
            return Err("Details click did not show the Editor Details popup heading".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        // Debug mode: the popup is on no "during" table, so it is photographed here, open.
        crate::debug_log::page_shot("details", "main");
        self.rec
            .press_keys(self.ui.window(), "Escape")
            .map_err(|e| format!("close Editor Details: {e:?}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            targets::run_label(&self.view(), "Editor Details").is_none()
        }) {
            return Err("Escape did not close the Editor Details popup".into());
        }
        Ok(())
    }

    /// Crown Main: turns its index 12 into 12.5, a position between two teeth, in the Tier form.
    pub(super) fn film_index_between_teeth(&mut self) -> Result<(), String> {
        let index = self.film_select_tier("Crown Main")?;
        let row = self
            .ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(usize::try_from(index).map_err(|_| "Crown Main has no row")?)
            .ok_or_else(|| "Crown Main disappeared".to_owned())?;
        let mut changed = false;
        let indices = row
            .indices_full
            .split(',')
            .map(|part| {
                if part.trim() == "12" {
                    changed = true;
                    "12.5".to_owned()
                } else {
                    part.trim().to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !changed {
            return Err(format!(
                "Crown Main has no exact 12 token: {:?}",
                row.indices_full
            ));
        }
        self.film_tier(&TierEntry::of("Crown Main").indices(&indices))
    }

    /// The History tab: opens its Variants view (through the Steps view's tab when that one is
    /// showing) and waits for the step's goal to hold.
    pub(super) fn open_variants_view(&mut self) -> Result<(), String> {
        self.film_inspector_tab(4)?;
        if self.ui.global::<VariantsModel>().get_view() == 1 {
            let steps = targets::run(&self.view(), &Query::Tab("Steps"))
                .ok_or("History Steps tab is missing")?;
            self.click_and_invoke(&steps)?;
        }
        let variants = targets::run(&self.view(), &Query::Tab("Variants"))
            .ok_or("History Variants tab is missing")?;
        self.click_and_invoke(&variants)?;
        if self.ui.global::<VariantsModel>().get_view() != 1 {
            return Err("Variants view did not open".into());
        }
        if !self.wait_done(0, pacing::STEP_TIMEOUT)? {
            return Err(format!(
                "Variants tab click left the composite goal false: {}",
                self.ui.global::<GuideModel>().get_current().waiting
            ));
        }
        Ok(())
    }

    /// Slice mode: clicks Symmetric, holds the changed state on screen and clicks it back.
    pub(super) fn film_symmetric_toggle(&mut self) -> Result<(), String> {
        let before = self.ui.global::<ManipulateModel>().get_slice_symmetric();
        self.press_label("Symmetric")?;
        if self.ui.global::<ManipulateModel>().get_slice_symmetric() == before {
            return Err("Symmetric did not toggle".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.press_label("Symmetric")?;
        if self.ui.global::<ManipulateModel>().get_slice_symmetric() != before {
            return Err("Symmetric did not return to its original state".into());
        }
        Ok(())
    }

    /// The Diagram view: drags the angle handle until the step's goal holds.
    pub(super) fn drag_diagram_angle_handle(&mut self) -> Result<(), String> {
        for distance in [55.0, -110.0] {
            self.drag_handle(0, distance, false)?;
            if self.ui.global::<GuideModel>().get_step_done() {
                break;
            }
        }
        if !self.ui.global::<GuideModel>().get_step_done() {
            return Err(format!(
                "Diagram A-handle drag did not change a tier angle: {}",
                self.ui.global::<ManipulateModel>().get_hint_text()
            ));
        }
        Ok(())
    }
}
