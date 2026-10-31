//! Explicit UI actions in the Command Palette and Live Render lessons.
use crate::{director::Film, pacing, targets};
use pleochroma_cut::{CommandPaletteModel, SolidPreviewModel};
use slint::{ComponentHandle, Model};
use uiprobe::AccessibleRole;

use crate::targets::Spot;

impl Film<'_> {
    /// Clicks the Edit pill after the Live Render lesson's final explanatory step.
    pub(crate) fn return_to_edit_from_live_render(&mut self) -> Result<(), String> {
        if self.ui.get_active_tab() != 0 || self.ui.get_render_view_tab() != 0 {
            return Err(format!(
                "Back to the Edit tab started on top_tab={} render_view={}",
                self.ui.get_active_tab(),
                self.ui.get_render_view_tab()
            ));
        }
        let edit = targets::edit_pill(&self.view())
            .ok_or("Edit view pill is not visible from Live Render")?;
        self.click_and_invoke(&edit)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            self.ui.get_active_tab() == 0 && self.ui.get_render_view_tab() == 1
        }) {
            return Err(format!(
                "clicking Edit left top_tab={} render_view={}",
                self.ui.get_active_tab(),
                self.ui.get_render_view_tab()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Demonstrates the disabled row's explanation while Diagram is active. The row activation
    /// goes through the production callback, which shows the reason and leaves the palette open.
    pub(crate) fn explain_disabled_palette_row(&mut self) -> Result<(), String> {
        let palette = self.ui.global::<CommandPaletteModel>();
        let preview = self.ui.global::<SolidPreviewModel>();
        if self.ui.get_active_tab() != 0 || preview.get_view_mode() != 3 {
            return Err(format!(
                "disabled palette example requires Edit/Diagram: top_tab={}, view_mode={}",
                self.ui.get_active_tab(),
                preview.get_view_mode()
            ));
        }
        self.key("Ctrl+K")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || palette.get_open()) {
            return Err("Ctrl+K did not open the palette for its disabled-row example".into());
        }
        let search = targets::text_input(&self.view(), "Search Commands")
            .ok_or("command palette search input is missing")?;
        self.click_and_type(&search, "slice")?;
        let row_loaded = pacing::wait_real(pacing::START_TIMEOUT, || {
            (0..palette.get_rows().row_count()).any(|index| {
                palette.get_rows().row_data(index).is_some_and(|row| {
                    row.title == "Toggle Slice Mode" && row.reason.contains("Diagram view")
                })
            })
        });
        if !row_loaded {
            return Err(format!(
                "searching slice did not show disabled Toggle Slice Mode: query={:?}, rows={}",
                palette.get_query(),
                palette.get_rows().row_count()
            ));
        }
        let displayed = palette
            .get_rows()
            .iter()
            .find(|row| row.title == "Toggle Slice Mode")
            .ok_or("disabled Toggle Slice Mode row disappeared")?;
        let expected_label = format!("Toggle Slice Mode. Not available: {}", displayed.reason);
        let rows = self
            .view()
            .root
            .query_descendants()
            .match_accessible_role(AccessibleRole::ListItem)
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label == expected_label)
            })
            .find_all();
        let visible: Vec<_> = rows
            .iter()
            .map(Spot::of)
            .filter(|spot| spot.usable(self.view().size))
            .collect();
        if visible.len() != 1 {
            return Err(format!(
                "disabled row with reason {:?} has {} visible list items",
                displayed.reason,
                visible.len()
            ));
        }
        self.click_and_invoke(&visible[0])?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            palette.get_open() && palette.get_query() == "slice" && preview.get_view_mode() == 3
        }) {
            return Err(format!(
                "choosing disabled row should show its reason and keep the palette open; open={}, view_mode={}",
                palette.get_open(),
                preview.get_view_mode()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Escape")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || !palette.get_open()) {
            return Err("Escape did not close the disabled-command palette".into());
        }
        self.key("1")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || preview.get_view_mode() == 0) {
            return Err(format!(
                "the final 1 key did not return to Solid view; mode={}",
                preview.get_view_mode()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }
}
