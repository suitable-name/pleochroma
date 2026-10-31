//! Visible Simple-mode material curve edits for the tutorial recording.

use crate::{director::Film, pacing, targets};
use std::time::Duration;
use uiprobe::{AccessibleRole, ElementHandle};

const ND: &str = "Refractive Index (nd):";
const DISPERSION: &str = "Dispersion (Fire ΔnF-C):";

impl Film<'_> {
    pub(super) fn film_simple_material_sliders(&mut self) -> Result<(), String> {
        if !self.material_slider_exists(ND) {
            let simple = self
                .material_control("Simple", AccessibleRole::Button)
                .ok_or("Simple material curve tab is not visible in the editor")?;
            self.click_and_invoke(&simple)?;
            if !pacing::wait_real(pacing::STEP_TIMEOUT, || self.material_slider_exists(ND)) {
                return Err(
                    "Simple material curve sliders did not appear after selecting Simple".into(),
                );
            }
        }
        if !self.material_slider_exists(DISPERSION) {
            return Err("Simple material dispersion slider is missing".into());
        }
        for (label, target, lower, upper, step) in [
            (ND, 1.800_f32, 1.300_f32, 3.200_f32, 0.001_f32),
            (DISPERSION, 0.0100_f32, 0.000_f32, 0.210_f32, 0.0005_f32),
        ] {
            self.scroll_material_slider_into_view(label)?;
            let (slider, thumb) = self.material_slider_and_thumb(label)?;
            let old = Self::slider_value(&slider, label)?;
            let track_left = slider.x + thumb.w / 2.0;
            let track_right = slider.x + slider.w - thumb.w / 2.0;
            if target < lower || target > upper || track_right <= track_left {
                return Err(format!("{label} drag target or track is invalid"));
            }
            let start = thumb.center();
            let end = (
                track_left + (track_right - track_left) * (target - lower) / (upper - lower),
                start.1,
            );
            self.gesture(start, end, None, false, false)?;
            let fresh = self
                .material_slider(label)
                .ok_or_else(|| format!("{label} slider disappeared after drag"))?;
            let value = Self::slider_value(&fresh, label)?;
            if (value - old).abs() < step / 2.0
                || value < lower
                || value > upper
                || (value - target).abs() > step
            {
                return Err(format!(
                    "{label} drag did not read back near {target}: before={old}, after={value}, allowed={lower}..{upper} by {step}"
                ));
            }
            self.hold(Duration::from_secs(1))?;
        }
        Ok(())
    }

    fn material_slider_exists(&self, label: &str) -> bool {
        !self
            .material_controls(label, AccessibleRole::Slider)
            .is_empty()
    }

    fn scroll_material_slider_into_view(&mut self, label: &str) -> Result<(), String> {
        for _ in 0..24 {
            if self.material_slider_and_thumb(label).is_ok() {
                return Ok(());
            }
            let (viewport, target) = self.material_scroll_target()?;
            let slider = self.material_slider_unclipped(label).ok_or_else(|| {
                format!("{label} slider is missing while revealing it in Material Editor")
            })?;
            let delta = (slider.center().1 - viewport.center().1).clamp(-160.0, 160.0);
            let wheel = if delta.abs() >= 1.0 {
                -delta
            } else if slider.y < viewport.y {
                80.0
            } else {
                -80.0
            };
            self.wheel_over(&target, wheel)?;
            self.hold(Duration::from_millis(100))?;
        }
        Err(
            "Simple material sliders are not fully visible in the Material Editor scroll view"
                .into(),
        )
    }

    pub(super) fn material_scroll_target(&self) -> Result<(targets::Spot, targets::Spot), String> {
        let view = self.view();
        let dialog = view
            .root
            .query_descendants()
            .match_inherits("MaterialEditorDialog")
            .find_first()
            .ok_or("Material Editor dialog is missing while scrolling")?;
        let scroll = dialog
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Material Editor scroll view is missing while scrolling")?;
        let viewport = targets::Spot::of(&scroll);
        let scrollbar = scroll
            .query_descendants()
            .match_inherits("ScrollBar")
            .find_all()
            .into_iter()
            .map(|element| targets::Spot::of(&element))
            .filter(|bar| bar.h > bar.w && bar.h >= 2.0)
            .filter_map(|bar| bar.clipped_to(&viewport, view.size))
            .max_by(|left, right| left.x.total_cmp(&right.x));
        let target = scrollbar.unwrap_or_else(|| targets::Spot {
            x: viewport.x + viewport.w - 7.0,
            y: viewport.y,
            w: 2.0,
            h: viewport.h,
            handle: None,
        });
        if !target.usable(view.size) || viewport.w < 16.0 {
            return Err("Material Editor has no safe ScrollBar or right-gutter target".into());
        }
        Ok((viewport, target))
    }

    fn material_controls(&self, label: &str, role: AccessibleRole) -> Vec<targets::Spot> {
        let view = self.view();
        let Some(dialog) = view
            .root
            .query_descendants()
            .match_inherits("MaterialEditorDialog")
            .find_first()
        else {
            return Vec::new();
        };
        let Some(container) = dialog
            .query_descendants()
            .match_inherits(if role == AccessibleRole::Button {
                "MaterialDispersionCoefficients"
            } else {
                "MaterialOpticsSliders"
            })
            .find_first()
        else {
            return Vec::new();
        };
        container
            .query_descendants()
            .find_all()
            .iter()
            .filter(|element| {
                element.accessible_role() == Some(role)
                    && element
                        .accessible_label()
                        .is_some_and(|actual| actual == label)
            })
            .map(targets::Spot::of)
            .collect()
    }

    fn material_slider_unclipped(&self, label: &str) -> Option<targets::Spot> {
        self.material_controls(label, AccessibleRole::Slider)
            .into_iter()
            .find(|spot| spot.w > 0.0 && spot.h > 0.0)
    }

    fn material_control(&self, label: &str, role: AccessibleRole) -> Option<targets::Spot> {
        let mut matches: Vec<_> = self
            .material_controls(label, role)
            .into_iter()
            .filter(|spot| spot.usable(self.view().size))
            .collect();
        (matches.len() == 1).then(|| matches.remove(0))
    }

    fn material_slider(&self, label: &str) -> Option<targets::Spot> {
        let mut matches: Vec<_> = self
            .material_controls(label, AccessibleRole::Slider)
            .into_iter()
            .filter(|spot| spot.usable(self.view().size))
            .collect();
        (matches.len() == 1).then(|| matches.remove(0))
    }

    fn material_slider_and_thumb(
        &self,
        label: &str,
    ) -> Result<(targets::Spot, targets::Spot), String> {
        let slider = self
            .material_slider(label)
            .ok_or_else(|| format!("{label} slider is not uniquely visible in Material Editor"))?;
        let handle = slider
            .handle
            .as_ref()
            .ok_or_else(|| format!("{label} slider has no accessible handle"))?;
        let view = self.view();
        let mut thumbs: Vec<_> = handle
            .query_descendants()
            .match_id("Slider::thumb")
            .find_all()
            .iter()
            .map(targets::Spot::of)
            .filter(|spot| spot.usable(view.size))
            .collect();
        if thumbs.len() != 1 || slider.w <= thumbs[0].w {
            return Err(format!(
                "{label} slider has {} usable rendered thumbs or no usable track",
                thumbs.len()
            ));
        }
        let (viewport, _) = self.material_scroll_target()?;
        let fully_visible = |spot: &targets::Spot| {
            spot.clipped_to(&viewport, view.size)
                .is_some_and(|clipped| {
                    (clipped.w - spot.w).abs() < 0.5 && (clipped.h - spot.h).abs() < 0.5
                })
        };
        if !fully_visible(&slider) || !fully_visible(&thumbs[0]) {
            return Err(format!(
                "{label} slider or thumb is clipped in the Material Editor"
            ));
        }
        let thumb = thumbs.remove(0);
        let track_left = slider.x + thumb.w / 2.0;
        let track_right = slider.x + slider.w - thumb.w / 2.0;
        let (thumb_x, thumb_y) = thumb.center();
        if thumb_x < track_left - 0.5
            || thumb_x > track_right + 0.5
            || thumb_y < slider.y - 0.5
            || thumb_y > slider.y + slider.h + 0.5
        {
            return Err(format!("{label} thumb is outside its slider track"));
        }
        Ok((slider, thumb))
    }

    fn slider_value(slider: &targets::Spot, label: &str) -> Result<f32, String> {
        let value = slider
            .handle
            .as_ref()
            .and_then(ElementHandle::accessible_value)
            .ok_or_else(|| format!("{label} slider has no accessible value readback"))?;
        let parsed: f32 = value
            .parse()
            .map_err(|_| format!("{label} slider has invalid accessible value {value:?}"))?;
        if !parsed.is_finite() {
            return Err(format!(
                "{label} slider has non-finite accessible value {value:?}"
            ));
        }
        Ok(parsed)
    }
}
