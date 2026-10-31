//! Real-input actions that let the Welcome Tour show the window parts it names.

use crate::{debug_log::debug_check_with, director::Film, pacing, targets};
use pleochroma_cut::{
    EditorModel, LibraryModel, ManipulateModel, SolidPreviewModel, harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::AccessibleRole;

impl Film<'_> {
    pub(super) fn film_welcome_library(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        if library.get_panel_collapsed() {
            let show = targets::role_label(
                &self.view(),
                "Show the Library List",
                AccessibleRole::Button,
                None,
            )
            .ok_or("Show the Library List button is missing")?;
            self.click_and_invoke(&show)?;
            if !pacing::wait_real(pacing::START_TIMEOUT, || !library.get_panel_collapsed()) {
                return Err("Show the Library List did not expand the Library panel".into());
            }
        }
        if !pacing::wait_real(pacing::LIBRARY_LOAD, || {
            library.get_diagram_list().row_count() > 0
        }) {
            return Err("Welcome Tour needs at least one listed Library design".into());
        }
        let initial = library
            .get_diagram_list()
            .row_data(0)
            .ok_or("first Library design row disappeared")?;
        let query: String = initial.title.trim().chars().take(4).collect();
        if query.trim().is_empty() {
            return Err("first Library design has no searchable title text".into());
        }
        let search = targets::text_input(&self.view(), "Search the Library")
            .ok_or("Search the Library input is missing")?;
        self.click_and_type(&search, &query)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            library.get_search_text().as_str() == query
                && library.get_diagram_list().row_count() > 0
                && library.get_matched_count() > 0
        }) {
            return Err(format!(
                "Library search {query:?} did not produce matching designs"
            ));
        }
        let selected = library
            .get_diagram_list()
            .row_data(0)
            .ok_or("Library search produced no row to select")?;
        let title = selected.title.to_string();
        let id = selected.id;
        self.click_library_row(0, &title, id)?;
        self.ensure_edit_view()?;
        let file = targets::file_button(&self.view()).ok_or("File menu is missing")?;
        self.click_and_invoke(&file)?;
        self.hold(pacing::MENU_HOLD)?;
        let load = targets::menu_item(&self.view(), "Load Selected")
            .ok_or("Load Selected menu item is missing")?;
        self.click_and_invoke(&load)?;
        let editor = self.ui.global::<EditorModel>();
        if editor.get_unsaved_dialog_open() {
            return Err("Load Selected raised an unsaved-changes question".into());
        }
        if !pacing::wait_lapse(&mut self.rec, pacing::PREPARE_TIMEOUT, || {
            editor.get_has_design()
                && editor.get_tiers().row_count() > 0
                && !editor.get_solve_running()
        })? {
            return Err("Load Selected did not show a design with tiers in Edit".into());
        }
        self.hold(Duration::from_secs(1))
    }

    pub(super) fn film_welcome_tier(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let first = editor
            .get_tiers()
            .row_data(0)
            .ok_or("Welcome Tour design has no tier to select")?;
        let name = first.name.to_string();
        self.film_select_tier(&name)?;
        self.hold(Duration::from_secs(1))
    }

    pub(super) fn film_welcome_viewport(&mut self) -> Result<(), String> {
        if self.ui.global::<SolidPreviewModel>().get_view_mode() != 0 {
            self.press_view_key("1", 0)?;
        }
        let editor = self.ui.global::<EditorModel>();
        if let Some(row) = usize::try_from(editor.get_selected_tier_index())
            .ok()
            .and_then(|index| editor.get_tiers().row_data(index))
        {
            // The tier table handles Escape as the visible command for clearing its selection.
            self.film_select_tier(&row.name)?;
            self.key("Escape")?;
        }
        let manipulate = self.ui.global::<ManipulateModel>();
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor.get_selected_tier_index() == -1 && !manipulate.get_handles_visible()
        }) {
            return Err(format!(
                "Escape did not clear the selected tier and its handles: selected={}, handles_visible={}",
                editor.get_selected_tier_index(),
                self.ui.global::<ManipulateModel>().get_handles_visible()
            ));
        }
        let view = self.view();
        let viewport = view
            .root
            .query_descendants()
            .match_inherits("SolidViewportView")
            .find_first()
            .map(|element| targets::Spot::of(&element))
            .ok_or("Solid viewport is missing")?;
        let obstructions = targets::viewport_obstructions(&view);
        if !viewport.usable(view.size) {
            return Err("Solid viewport is outside the window".into());
        }
        let inside = |spot: &targets::Spot, point: (f32, f32)| {
            point.0 >= spot.x
                && point.0 <= spot.x + spot.w
                && point.1 >= spot.y
                && point.1 <= spot.y + spot.h
        };
        let clear = |point: (f32, f32)| {
            inside(&viewport, point) && !obstructions.iter().any(|spot| inside(spot, point))
        };
        let start = (0.25, 0.60);
        let start = [start, (0.35, 0.65), (0.22, 0.72), (0.42, 0.58)]
            .into_iter()
            .map(|(x, y)| {
                (
                    viewport.w.mul_add(x, viewport.x),
                    viewport.h.mul_add(y, viewport.y),
                )
            })
            .find(|point| clear(*point))
            .ok_or("no unobstructed Solid viewport area is available to drag")?;
        let end = (start.0 + 42.0, start.1 + 22.0);
        if !clear(end) {
            return Err("Welcome Tour viewport drag would cross an overlay or toolbar".into());
        }
        let before = harness_support::camera_pose();
        self.gesture(start, end, None, false, false)?;
        let after = harness_support::camera_pose();
        if (after.0 - before.0).abs() < 0.01 && (after.1 - before.1).abs() < 0.01 {
            return Err("Welcome Tour viewport drag did not change the camera pose".into());
        }
        self.hold(Duration::from_secs(1))?;
        self.press_view_key("4", 3)?;
        self.hold(Duration::from_secs(1))?;
        self.press_view_key("1", 0)?;
        self.hold(Duration::from_secs(1))
    }

    pub(super) fn film_welcome_help(&mut self) -> Result<(), String> {
        self.film_palette_introduction()?;
        self.hold(Duration::from_secs(1))
    }

    /// Whether a tooltip popup is open. Slint 1.18 lowers `Tooltip` to a `PopupWindow`, which is
    /// not part of the main window's element tree, so it is found through the active popups.
    /// Whether a tooltip is open: Slint shows a `Tooltip` as a popup of kind `ToolTip`, outside
    /// the window's element tree, so the window's own list of active popups is what tells.
    fn indices_tooltip_open(&self) -> bool {
        use slint::private_unstable_api::re_exports::{WindowInner, WindowKind};
        let open = WindowInner::from_pub(self.ui.window())
            .active_popups()
            .iter()
            .filter(|popup| matches!(popup.window_kind, WindowKind::ToolTip))
            .count();
        debug_check_with("open tooltip popups", &"at most one", &open, open <= 1);
        open > 0
    }

    pub(super) fn film_indices_tip_popup(&mut self) -> Result<(), String> {
        let label = self
            .inspector_form_control("Indices", AccessibleRole::Text)?
            .ok_or("Indices caption did not enter the Inspector viewport")?;
        let away = (self.view().size.0 - 10.0, self.view().size.1 - 10.0);
        self.glide_to(away)?;
        // Closed means no visible tooltip naming the index wheel.
        if !pacing::wait_real(pacing::START_TIMEOUT, || !self.indices_tooltip_open()) {
            return Err("Indices tooltip did not close before the demonstration".into());
        }
        self.glide_to(label.center())?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || self.indices_tooltip_open()) {
            return Err("Hovering the Indices FormRow did not show its tooltip popup".into());
        }
        self.hold(Duration::from_secs(2))?;
        self.glide_to(away)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || !self.indices_tooltip_open()) {
            return Err("Indices tooltip remained open after moving away".into());
        }
        Ok(())
    }
}
