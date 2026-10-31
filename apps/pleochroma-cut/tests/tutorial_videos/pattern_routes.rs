//! Visible routes of the Watermelon Tourmaline lesson: a custom material with a color made by
//! Pick..., then the Color Pattern window with the Watermelon template.
//!
//! This file is a child of `event_extras.rs`, so it can use the private helpers of the Material
//! Editor routes (`extra_click_panel`, `material_scroll_target`). Each route fails with a sentence
//! naming the control it could not find, so a changed label stops the video instead of filming
//! something else.

use crate::{
    director::Film,
    pacing, routes,
    support::WindowView,
    targets::{self, Spot},
};
use pleochroma_cut::{
    BodyColorEditorModel, ColorPatternModel, ParametersModel, ViewportModel, harness_support,
};
use slint::ComponentHandle;
use std::time::Duration;
use uiprobe::screenshot;

/// The name the lesson's material gets: no built-in material uses it.
const MATERIAL_NAME: &str = "Tutorial tourmaline";

/// How many wheel turns a control may need to scroll into view in the Material Editor.
const SCROLL_TRIES: usize = 24;

/// Whether `spot` overlaps the scroll viewport `viewport` vertically.
fn overlaps(spot: &Spot, viewport: &Spot) -> bool {
    spot.y < viewport.y + viewport.h && spot.y + spot.h > viewport.y
}

impl Film<'_> {
    /// Opens the Stone tab and the Material Editor from its New/Edit Material... button.
    fn open_pattern_material_editor(&mut self) -> Result<(), String> {
        let panel = self.ui.global::<ParametersModel>();
        if !panel.get_open() || panel.get_active_tab() != 0 {
            self.extra_click_dock("stone")?;
        }
        if !panel.get_open() || panel.get_active_tab() != 0 {
            return Err("Stone panel did not open".into());
        }
        self.extra_click_panel("New/Edit Material...")?;
        if !self.ui.global::<ViewportModel>().get_material_editor_open() {
            return Err("Material Editor did not open".into());
        }
        Ok(())
    }

    /// The control labelled `label` in the Material Editor, scrolled fully into view when it sits
    /// in the editor's scroll area (the header and the footer buttons do not scroll).
    fn material_editor_control(&mut self, label: &str) -> Result<Spot, String> {
        for _ in 0..SCROLL_TRIES {
            let (viewport, scroll_target) = self.material_scroll_target()?;
            let view = self.view();
            let found = targets::run_label(&view, label);
            if let Some(spot) = &found {
                if !overlaps(spot, &viewport) {
                    return Ok(spot.clone());
                }
                let clipped = spot.clipped_to(&viewport, view.size);
                if let Some(clipped) = clipped
                    && (clipped.h - spot.h).abs() < 0.5
                    && (clipped.w - spot.w).abs() < 0.5
                {
                    return Ok(clipped);
                }
            }
            // Scroll down while the control is below the middle of the viewport or unknown.
            let wheel = match &found {
                Some(spot) if spot.y < viewport.y => 90.0,
                _ => -90.0,
            };
            self.wheel_over(&scroll_target, wheel)?;
            self.hold(Duration::from_millis(100))?;
        }
        Err(format!(
            "Material Editor control {label:?} did not scroll into view"
        ))
    }

    /// A click on a Material Editor control, found with [`Self::material_editor_control`].
    fn click_material_editor(&mut self, label: &str) -> Result<(), String> {
        let spot = self.material_editor_control(label)?;
        self.click_and_invoke(&spot)?;
        Ok(())
    }

    /// Step "Make a material with a color": a named material whose color is made with Pick...,
    /// saved with Save & Apply.
    pub(crate) fn film_pattern_material(&mut self) -> Result<(), String> {
        self.open_pattern_material_editor()?;
        let field = targets::text_input(&self.view(), "Material Name")
            .ok_or("Material Name field is missing")?;
        self.click_and_type(&field, MATERIAL_NAME)?;
        self.click_material_editor("Pick\u{2026}")?;
        let editor = self.ui.global::<BodyColorEditorModel>();
        if !pacing::wait_real(Duration::from_secs(60), || {
            editor.get_has_solution() && !editor.get_busy()
        }) {
            return Err("the color editor did not finish matching the picked color".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.click_material_editor("Apply Color")?;
        self.hold(Duration::from_millis(300))?;
        self.click_material_editor("Save & Apply")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !self.ui.global::<ViewportModel>().get_material_editor_open()
        }) {
            return Err("Save & Apply did not close the Material Editor".into());
        }
        Ok(())
    }

    /// Step "Add the Watermelon pattern": the Material Editor again, Add Pattern..., the
    /// Watermelon template and Save in the Color Pattern window (a second captured window).
    pub(crate) fn film_pattern_watermelon(&mut self) -> Result<(), String> {
        self.open_pattern_material_editor()?;
        self.click_material_editor("Add Pattern\u{2026}")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::color_pattern_window().is_some()
        }) {
            return Err("Add Pattern... did not open the Color Pattern window".into());
        }
        let window =
            harness_support::color_pattern_window().ok_or("Color Pattern window disappeared")?;
        screenshot::set_window_size(window.window(), (1100, 780), crate::config::SCALE);
        for text in routes::visible_texts(&WindowView::of("ColorPatternWindow", &window)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in the Color Pattern window ({word:?}): {text:?}"
                ));
            }
        }
        if let Some(other) = self.attached_window {
            return Err(format!(
                "the {other} window is still attached to the video; the Color Pattern window cannot be attached too"
            ));
        }
        let origin = self.rec.place_window(window.window());
        self.rec.attach_window(&window, origin);
        self.attached_window = Some("color pattern");
        self.rec.chapter("Color Pattern");
        self.rec
            .caption("Give the material zones of different colors.");
        self.hold(pacing::RESULT_HOLD)?;

        let model = window.global::<ColorPatternModel>();
        self.click_pattern_control(&window, origin, "Watermelon")?;
        if !pacing::wait_real(Duration::from_secs(60), || {
            !model.get_solving_colors() && !model.get_preview_busy()
        }) {
            return Err("the Watermelon colors did not finish solving".into());
        }
        self.click_pattern_control(&window, origin, "Cross-Section")?;
        self.hold(pacing::RESULT_HOLD)?;
        self.click_pattern_control(&window, origin, "Save")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::color_pattern_window().is_none()
        }) {
            return Err("Save did not close the Color Pattern window".into());
        }
        self.rec.detach_window();
        self.attached_window = None;
        self.rec.caption("");
        // The step after this one looks at the stone, so the Material Editor must be closed.
        let cancel = self.scoped_button("MaterialEditorDialog", "Cancel")?;
        self.click_and_invoke(&cancel)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !self.ui.global::<ViewportModel>().get_material_editor_open()
        }) {
            return Err("Cancel did not close the Material Editor".into());
        }
        Ok(())
    }

    /// A click on the control labelled `label` in the Color Pattern window.
    fn click_pattern_control(
        &mut self,
        window: &pleochroma_cut::ColorPatternWindow,
        origin: (f32, f32),
        label: &str,
    ) -> Result<(), String> {
        let view = WindowView::of("ColorPatternWindow", window);
        let spot = targets::run_label(&view, label)
            .ok_or_else(|| format!("Color Pattern window control {label:?} is missing"))?;
        self.click_secondary(window.window(), origin, &spot)?;
        self.hold(Duration::from_millis(200))
    }
}
