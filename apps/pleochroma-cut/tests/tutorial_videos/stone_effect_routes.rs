//! Visible routes of the Stone Effects lesson: the Stone tab's Inclusion Haze slider, Frosted
//! Girdle switch, Edge Rounding slider and Stone Width field, each worked with real pointer and
//! key input.
//!
//! This file is a child of `event_extras.rs`, so it can use the guide-occlusion helper of the
//! Parameters panel routes. Each route fails with a sentence naming the control it could not
//! find or the value that did not read back, so a changed label stops the video instead of
//! filming something else. The lesson's events are raised when the debounced commit has written
//! the design, so every route waits for the step to finish before it returns.

use crate::{
    director::Film,
    pacing,
    support::WindowView,
    targets::{self, Spot},
};
use pleochroma_cut::{EditorModel, GuideModel, ParametersModel, SettingsModel};
use slint::ComponentHandle;
use std::time::Duration;

/// How many wheel turns the Parameters panel may need to bring a control fully into view.
const SCROLL_TRIES: usize = 16;

/// How far along its track a slider is dragged: far enough to see, small enough to stay subtle.
const SLIDER_FRACTION: f32 = 0.35;

/// The Stone Width the lesson types, in millimeters.
const WIDTH_TEXT: &str = "8";

impl Film<'_> {
    /// The Stone tab control found by `find`, scrolled fully into the Parameters panel's view.
    pub(crate) fn stone_control(
        &mut self,
        what: &str,
        find: impl Fn(&WindowView) -> Option<Spot>,
    ) -> Result<Spot, String> {
        let panel = self.ui.global::<ParametersModel>();
        if !panel.get_open() || panel.get_active_tab() != 0 {
            self.extra_click_dock("stone")?;
        }
        if !panel.get_open() || panel.get_active_tab() != 0 {
            return Err("Stone panel did not open".into());
        }
        let (viewport, wheel_point) = self.stone_scroll_area()?;
        super::panel_occlusion::ensure_guide_clear_of(self, &wheel_point)?;
        self.wheel_over(&wheel_point, 3000.0)?;
        self.hold(Duration::from_millis(140))?;
        for _ in 0..SCROLL_TRIES {
            let view = self.view();
            if let Some(spot) = find(&view)
                && let Some(clipped) = spot.clipped_to(&viewport, view.size)
                && (clipped.h - spot.h).abs() < 0.5
            {
                super::panel_occlusion::ensure_guide_clear_of(self, &spot)?;
                // The guide may have moved: find the control again at its current place.
                let view = self.view();
                return find(&view)
                    .map(|spot| self.clear_of_panel_scroll_bar(spot))
                    .ok_or_else(|| format!("{what} disappeared after the guide moved"));
            }
            self.wheel_over(&wheel_point, -250.0)?;
            self.hold(Duration::from_millis(120))?;
        }
        Err(format!("{what} did not scroll into view in the Stone tab"))
    }

    /// `spot` narrowed to its part left of the Parameters panel's vertical scroll bar, when the
    /// bar lies over its centre.
    ///
    /// The bar is drawn over the right edge of the panel, and the right end of a row (the
    /// Material Menu button of the Stone tab) can reach under it: a click on the centre of such
    /// a control would land on the bar. A control that is not under the bar is returned as it is.
    pub(crate) fn clear_of_panel_scroll_bar(&self, spot: Spot) -> Spot {
        let bar = self
            .view()
            .root
            .query_descendants()
            .match_inherits("ParametersPanel")
            .find_first()
            .and_then(|panel| {
                panel
                    .query_descendants()
                    .match_inherits("ScrollBar")
                    .find_all()
                    .iter()
                    .map(Spot::of)
                    .find(|bar| bar.h > bar.w && bar.w >= 2.0)
            });
        let Some(bar) = bar else {
            return spot;
        };
        let (x, y) = spot.center();
        let under_bar =
            x >= bar.x - 1.0 && x <= bar.x + bar.w + 1.0 && y >= bar.y && y <= bar.y + bar.h;
        let room = bar.x - 2.0 - spot.x;
        if !under_bar || room < 6.0 {
            return spot;
        }
        Spot { w: room, ..spot }
    }

    /// Opens the Stone tab's fold card `label` when it is folded. The cards (Crystal Axis 1,
    /// Girdle & Edges 2, Clarity 4, Play-of-Color or Schiller 8: the bit is its place in
    /// `ParametersModel.stone_folds`) start folded on a fresh install, and the controls inside a
    /// folded card are not on screen, so every route that works one opens its card first, as a
    /// learner does with a click on the card's header.
    pub(crate) fn open_stone_fold(&mut self, label: &str, bit: i32) -> Result<(), String> {
        let panel = self.ui.global::<ParametersModel>();
        if panel.get_stone_folds() & bit != 0 {
            return Ok(());
        }
        let header = self.stone_control(label, |view| {
            targets::role_label(
                view,
                label,
                uiprobe::AccessibleRole::Button,
                Some("ParametersPanel"),
            )
        })?;
        self.click_and_invoke(&header)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || panel.get_stone_folds() & bit != 0) {
            return Err(format!(
                "the {label} card did not open (stone_folds {})",
                panel.get_stone_folds()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// The Parameters panel's scroll area and a small spot in its middle to turn the wheel over.
    fn stone_scroll_area(&self) -> Result<(Spot, Spot), String> {
        let panel = self
            .view()
            .root
            .query_descendants()
            .match_inherits("ParametersPanel")
            .find_first()
            .ok_or("Parameters panel is missing")?;
        let scroll = panel
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Parameters panel scroll view is missing")?;
        let viewport = Spot::of(&scroll);
        // The right-hand strip beside the rows (where the scroll bar sits), not the middle: a
        // slider that scrolls under the middle would take the wheel turns and stop the scroll.
        let (_, y) = viewport.center();
        let x = viewport.x + viewport.w - 5.0;
        let wheel_point = Spot {
            x,
            y,
            w: 2.0,
            h: 2.0,
            handle: None,
        };
        Ok((viewport, wheel_point))
    }

    /// Drags the thumb of the Stone tab slider labelled `label` a little along its track.
    pub(crate) fn drag_stone_slider(&mut self, label: &str) -> Result<(), String> {
        let slider = self.stone_control(label, |view| {
            targets::role_label(
                view,
                label,
                uiprobe::AccessibleRole::Slider,
                Some("ParametersPanel"),
            )
        })?;
        let view = self.view();
        let handle = slider
            .handle
            .as_ref()
            .ok_or_else(|| format!("{label} slider has no accessible element handle"))?;
        let thumbs: Vec<Spot> = handle
            .query_descendants()
            .match_id("Slider::thumb")
            .find_all()
            .iter()
            .map(Spot::of)
            .filter(|spot| spot.usable(view.size))
            .collect();
        let [thumb] = thumbs.as_slice() else {
            return Err(format!(
                "{label} slider has {} usable rendered thumbs, expected exactly one",
                thumbs.len()
            ));
        };
        if slider.w <= thumb.w {
            return Err(format!("{label} slider track is unusable"));
        }
        let start = thumb.center();
        let end = (
            (slider.w - thumb.w).mul_add(SLIDER_FRACTION, slider.x + thumb.w / 2.0),
            start.1,
        );
        self.gesture(start, end, None, false, false)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Waits for the lesson step to finish: the debounced commit wrote the design and the event
    /// reached the guide.
    fn wait_stone_step_done(&self, what: &str) -> Result<(), String> {
        let guide = self.ui.global::<GuideModel>();
        if pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
            Ok(())
        } else {
            Err(format!(
                "{what} did not reach the design before the timeout"
            ))
        }
    }

    /// The Stone Effects lesson's event `name`: the control it belongs to, worked in the Stone
    /// tab.
    pub(crate) fn film_stone_effect(&mut self, name: &str) -> Result<(), String> {
        let settings = self.ui.global::<SettingsModel>();
        match name {
            "stone_haze_changed" => {
                if settings.get_inclusion_sigma_s() > 0.0 {
                    return Err("Inclusion Haze is already on before the lesson drags it".into());
                }
                // The effect cards start folded: open Clarity first.
                self.open_stone_fold("Clarity", 4)?;
                self.drag_stone_slider("Inclusion Haze")?;
                if settings.get_inclusion_sigma_s() <= 0.0 {
                    return Err("dragging Inclusion Haze did not change its value".into());
                }
                self.wait_stone_step_done("Inclusion Haze")
            }
            "stone_girdle_frosted_changed" => {
                if settings.get_girdle_frosted() {
                    return Err("Frosted Girdle is already on before the lesson clicks it".into());
                }
                self.open_stone_fold("Girdle & Edges", 2)?;
                let switch = self.stone_control("Frosted Girdle", |view| {
                    targets::role_label(
                        view,
                        "Frosted Girdle",
                        uiprobe::AccessibleRole::Checkbox,
                        Some("ParametersPanel"),
                    )
                })?;
                self.click_and_invoke(&switch)?;
                if !settings.get_girdle_frosted() {
                    return Err("clicking Frosted Girdle did not switch it on".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                self.wait_stone_step_done("Frosted Girdle")
            }
            "stone_edge_rounding_changed" => {
                if settings.get_edge_rounding_radius() > 0.0 {
                    return Err("Edge Rounding is already on before the lesson drags it".into());
                }
                self.open_stone_fold("Girdle & Edges", 2)?;
                self.drag_stone_slider("Edge Rounding")?;
                if settings.get_edge_rounding_radius() <= 0.0 {
                    return Err("dragging Edge Rounding did not change its value".into());
                }
                self.wait_stone_step_done("Edge Rounding")
            }
            "stone_width_set" => {
                let field = self.stone_control("Stone Width field", |view| {
                    targets::text_input(view, "Stone Width in Millimetres")
                })?;
                self.click_and_type(&field, WIDTH_TEXT)?;
                self.rec
                    .press_keys(self.ui.window(), "Return")
                    .map_err(|e| format!("Stone Width Return: {e:?}"))?;
                let editor = self.ui.global::<EditorModel>();
                if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                    !editor.get_design_girdle_diameter_text().is_empty()
                }) {
                    return Err("Enter did not set the Stone Width in the design".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                self.wait_stone_step_done("Stone Width")
            }
            _ => Err(format!("unrecognized Stone Effects event {name:?}")),
        }
    }
}
