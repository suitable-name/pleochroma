//! Visible UI routes for the rendering, cutting, help, and rough-planning lessons.

#[path = "color_variant_routes.rs"]
mod color_variant_routes;
#[path = "material_routes.rs"]
mod material_routes;
#[path = "event_extras/panel_occlusion.rs"]
mod panel_occlusion;
#[path = "pattern_routes.rs"]
mod pattern_routes;
#[path = "rough_routes.rs"]
mod rough_routes;
#[path = "stone_effect_routes.rs"]
mod stone_effect_routes;

pub use color_variant_routes::handles as handles_color_variants;

use crate::{
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets,
};
use pleochroma_cut::{
    BatchModel, CuttingModeModel, DesignLightingModel, DispersionEditorModel, EditorModel,
    GuideModel, HelpModel, ParametersModel, PreferencesModel, ShortcutsModel, SolidPreviewModel,
    ViewportModel, harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::screenshot;

impl Film<'_> {
    pub(crate) fn attach_help_window(&mut self) -> Result<(), String> {
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::help_window().is_some()
        }) {
            return Err("User Manual did not open the help window".into());
        }
        let help = harness_support::help_window().ok_or("Help window disappeared")?;
        screenshot::set_window_size(help.window(), (1100, 780), crate::config::SCALE);
        for text in routes::visible_texts(&WindowView::of("HelpWindow", &help)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!("private text in Help window ({word:?}): {text:?}"));
            }
        }
        match self.attached_window {
            // The same manual window is already in the video (a second step opens it again).
            Some("help") => {}
            Some(other) => {
                return Err(format!(
                    "the {other} window is still attached to the video; the manual cannot be attached too"
                ));
            }
            None => {
                let origin = self.rec.place_window(help.window());
                self.rec.attach_window(&help, origin);
                self.attached_window = Some("help");
            }
        }
        Ok(())
    }
    pub(crate) fn extra_click(&mut self, label: &str) -> Result<(), String> {
        let spot = targets::run_label(&self.view(), label)
            .ok_or_else(|| format!("{label:?} is not visible"))?;
        self.click_and_invoke(&spot)?;
        Ok(())
    }

    /// Presses the Parameters dock control of `tab` (`"stone"`, `"lighting"`, `"render_output"`):
    /// the rail button while the panel is folded, the tab pill while it is open.
    pub(crate) fn extra_click_dock(&mut self, tab: &str) -> Result<(), String> {
        let spot = targets::dock_tab_button(&self.view(), tab)
            .ok_or_else(|| format!("the {tab} dock tab control is not visible"))?;
        self.click_and_invoke(&spot)
    }

    fn extra_click_panel(&mut self, label: &str) -> Result<(), String> {
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
        let viewport = targets::Spot::of(&scroll);
        let wheel_point = targets::Spot {
            x: viewport.center().0,
            y: viewport.center().1,
            w: 2.0,
            h: 2.0,
            handle: None,
        };
        panel_occlusion::ensure_guide_clear_of(self, &wheel_point)?;
        self.wheel_over(&wheel_point, 3000.0)?;
        self.hold(Duration::from_millis(140))?;
        for _ in 0..15 {
            let view = self.view();
            let spot = targets::role_label(
                &view,
                label,
                uiprobe::AccessibleRole::Button,
                Some("ParametersPanel"),
            );
            if let Some(spot) = spot
                && let Some(clipped) = spot.clipped_to(&viewport, view.size)
            {
                panel_occlusion::ensure_guide_clear_of(self, &clipped)?;
                // Re-resolve after any guide move so the click uses current screen bounds.
                let view = self.view();
                let panel = view
                    .root
                    .query_descendants()
                    .match_inherits("ParametersPanel")
                    .find_first()
                    .ok_or("Parameters panel disappeared after guide relocation")?;
                let scroll = panel
                    .query_descendants()
                    .match_inherits("ScrollView")
                    .find_first()
                    .ok_or("Parameters panel scroll view disappeared after guide relocation")?;
                let current_viewport = targets::Spot::of(&scroll);
                let spot = targets::role_label(
                    &view,
                    label,
                    uiprobe::AccessibleRole::Button,
                    Some("ParametersPanel"),
                )
                .ok_or_else(|| {
                    format!("Parameters control {label:?} disappeared after guide relocation")
                })?;
                let clipped = spot
                    .clipped_to(&current_viewport, view.size)
                    .ok_or_else(|| format!("Parameters control {label:?} is no longer visible"))?;
                panel_occlusion::ensure_guide_clear_of(self, &clipped)?;
                let (x, y) = clipped.center();
                self.rec.click_at(self.ui.window(), (x, y), (0.0, 0.0))?;
                pump(60);
                self.hold(Duration::from_millis(150))?;
                return Ok(());
            }
            self.wheel_over(&viewport, -250.0)?;
            self.hold(Duration::from_millis(120))?;
        }
        Err(format!(
            "Parameters control {label:?} is not visible after scrolling"
        ))
    }

    pub(crate) fn extra_menu(&mut self, menu: &str, item: &'static str) -> Result<(), String> {
        if (menu, item) == ("Library", "Plan Rough...") {
            self.extra_click("Library")?;
            self.hold(pacing::MENU_HOLD)?;
            // Library omits the three batch commands in Simple mode. Verify the actual popup
            // order and use keyboard selection, as native MenuBar popups do not reliably take
            // pointer activation on their rows.
            let advanced = !self.ui.global::<PreferencesModel>().get_simple_mode()
                || (self.ui.global::<GuideModel>().get_open()
                    && self.ui.global::<GuideModel>().get_current().allow.advanced);
            let batch = self.ui.global::<BatchModel>();
            let entries: Vec<(&str, bool)> = if advanced {
                vec![
                    (
                        "Regenerate Preview Images...",
                        !batch.get_preview_batch_running(),
                    ),
                    ("Regenerate Tilt Curves...", !batch.get_tilt_batch_running()),
                    (
                        "Regenerate Both...",
                        !batch.get_preview_batch_running() && !batch.get_tilt_batch_running(),
                    ),
                    ("Plan Rough...", true),
                ]
            } else {
                vec![("Plan Rough...", true)]
            };
            for (title, _) in &entries {
                if targets::run_label(&self.view(), title).is_none() {
                    return Err(format!("Library menu entry {title:?} is missing"));
                }
            }
            let target_index = entries
                .iter()
                .position(|(title, _)| *title == item)
                .ok_or_else(|| format!("Library > {item} is not present in this menu mode"))?;
            if !entries[target_index].1 {
                return Err(format!(
                    "Library > {item} is disabled while its batch is running"
                ));
            }
            let downs = entries[..target_index]
                .iter()
                .filter(|(_, enabled)| *enabled)
                .count()
                + 1;
            for _ in 0..downs {
                self.rec
                    .press_keys(self.ui.window(), "Down")
                    .map_err(|e| format!("Library menu Down: {e:?}"))?;
                self.hold(Duration::from_millis(180))?;
            }
            self.rec
                .press_keys(self.ui.window(), "Return")
                .map_err(|e| format!("Library menu Return: {e:?}"))?;
            pump(60);
            return Ok(());
        }
        if menu != "Help" {
            return Err(format!("no visible menu route for {menu} > {item}"));
        }
        // Help's real dropdown order in app.slint is Manual, F1 Help, Glossary,
        // Shortcuts. PopupMenuImpl opens with no highlight; each Down is visible input.
        let entries = [
            "User Manual",
            "Help for This Screen (F1)",
            "Glossary",
            "Keyboard Shortcuts",
        ];
        let target = match item {
            "User Manual" => 1,
            "Glossary" => 3,
            "Keyboard Shortcuts" => 4,
            _ => return Err(format!("no Help dropdown route for {item}")),
        };
        self.extra_click("Help")?;
        self.hold(pacing::MENU_HOLD)?;
        for title in entries.iter().take(target) {
            if targets::run_label(&self.view(), title).is_none() {
                return Err(format!("Help menu entry {title:?} is missing"));
            }
            self.rec
                .press_keys(self.ui.window(), "Down")
                .map_err(|e| format!("Help menu Down: {e:?}"))?;
            self.hold(Duration::from_millis(180))?;
        }
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|e| format!("Help menu Return: {e:?}"))?;
        pump(60);
        Ok(())
    }

    fn extra_combo_next(&mut self, label: &str, current: i32, count: usize) -> Result<(), String> {
        if count < 2 {
            return Err(format!("{label} has no alternate choice"));
        }
        let combo =
            targets::role_label(&self.view(), label, uiprobe::AccessibleRole::Combobox, None)
                .ok_or_else(|| format!("{label:?} combo is not visible"))?;
        self.click_and_invoke(&combo)?;
        let key = if i32::try_from(count).is_ok_and(|total| current + 1 < total) {
            "Down"
        } else {
            "Up"
        };
        self.rec
            .press_keys(self.ui.window(), key)
            .map_err(|e| format!("{label} {key}: {e:?}"))?;
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|e| format!("{label} Return: {e:?}"))?;
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per extra event, each a short filmed route"
    )]
    pub(crate) fn drive_extra_event(&mut self, n: usize, name: &str) -> Result<(), String> {
        match name {
            "lighting_preset_chosen" => {
                let model = self.ui.global::<ViewportModel>();
                let before = model.get_selected_lighting_index();
                self.extra_combo_next(
                    "Lighting",
                    before,
                    model.get_lighting_options().row_count(),
                )?;
                if model.get_selected_lighting_index() == before {
                    return Err("Lighting combo did not select a new rig".into());
                }
            }
            "design_lighting_saved" | "design_lighting_forgotten" => {
                let model = self.ui.global::<DesignLightingModel>();
                let editor = self.ui.global::<pleochroma_cut::EditorModel>();
                let panel = self.ui.global::<ParametersModel>();
                if !panel.get_open() || panel.get_active_tab() != 1 {
                    self.extra_click_dock("lighting")?;
                }
                if !panel.get_open() || panel.get_active_tab() != 1 {
                    return Err("Lighting and Environment panel did not open".into());
                }
                if !editor.get_has_design() || !model.get_available() {
                    return Err(format!(
                        "{name}: design lighting is unavailable (has_design={}, available={})",
                        editor.get_has_design(),
                        model.get_available()
                    ));
                }
                let expected = name == "design_lighting_saved";
                let label = if expected {
                    "Use This Lighting for This Design"
                } else {
                    "Forget for This Design"
                };
                if model.get_has_saved() == expected {
                    return Err(format!(
                        "{label}: design lighting already has_saved={}, so this event would not demonstrate a change",
                        model.get_has_saved()
                    ));
                }
                self.extra_click_panel(label)?;
                if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_has_saved() == expected) {
                    return Err(format!(
                        "{label} did not change design lighting state; available={}, has_saved={}",
                        model.get_available(),
                        model.get_has_saved()
                    ));
                }
            }
            "custom_material_saved" | "coefficient_material_saved" => {
                self.film_material_save(name == "coefficient_material_saved")?;
            }
            "cutting_step_marked" => {
                if !self.ui.global::<CuttingModeModel>().get_open() {
                    let spot = targets::resolve(&self.view(), "cutting_mode_button");
                    let targets::Resolved::Found(spot) = spot else {
                        return Err("Cut Mode button is missing".into());
                    };
                    self.click_and_invoke(&spot)?;
                }
                let model = self.ui.global::<CuttingModeModel>();
                if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                    model.get_open() && !model.get_busy()
                }) {
                    return Err("Cutting Mode did not become ready".into());
                }
                if model.get_step_state() == 1 {
                    return Err("cutting step is already marked done".into());
                }
                self.extra_click("Mark Step Done")?;
                if model.get_step_state() != 1 || model.get_done_count() < 1 {
                    return Err("Mark Step Done did not update cutting progress".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                self.rec
                    .press_keys(self.ui.window(), "Escape")
                    .map_err(|e| format!("close Cutting Mode: {e:?}"))?;
                if model.get_open() {
                    return Err("Escape did not close Cutting Mode after marking the step".into());
                }
            }
            "cut_slider_rough" | "cut_slider_step" | "cut_slider_finished" => {
                self.film_cut_slider(name)?;
            }
            "rough_planner_opened" => self.film_rough_open()?,
            "rough_plan_finished" => self.film_rough_finish()?,
            "glossary_opened" => {
                self.extra_menu("Help", "Glossary")?;
                if !self.ui.global::<HelpModel>().get_glossary_open() {
                    return Err("Help > Glossary did not open the glossary".into());
                }
                let field = targets::text_input(&self.view(), "Search the Terms")
                    .ok_or("glossary search field is missing")?;
                self.click_and_type(&field, "mast")?;
                if self.ui.global::<HelpModel>().get_glossary_query() != "mast" {
                    return Err("glossary search did not read back mast".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                self.rec
                    .press_keys(self.ui.window(), "Escape")
                    .map_err(|e| format!("close glossary: {e:?}"))?;
            }
            "shortcuts_opened" => {
                self.extra_menu("Help", "Keyboard Shortcuts")?;
                if !self.ui.global::<ShortcutsModel>().get_open() {
                    return Err("Help > Keyboard Shortcuts did not open".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                self.rec
                    .press_keys(self.ui.window(), "Escape")
                    .map_err(|e| format!("close shortcuts: {e:?}"))?;
            }
            "help_opened" => {
                if n == 1 {
                    self.extra_menu("Help", "User Manual")?;
                    self.attach_help_window()?;
                    let help = harness_support::help_window()
                        .ok_or("User Manual menu item did not leave a Help window open")?;
                    let model = help.global::<HelpModel>();
                    if model.get_chapter_title().as_str() != "Contents" || model.get_searching() {
                        return Err(format!(
                            "Help > User Manual did not open the Contents page: chapter={:?}, searching={}",
                            model.get_chapter_title(),
                            model.get_searching()
                        ));
                    }
                } else if n == 2 {
                    self.film_context_help()?;
                } else {
                    return Err(format!("Help opened at unexpected lesson step {n}"));
                }
            }
            _ => return Err(format!("unrecognized extra UI event {name:?}")),
        }
        Ok(())
    }

    fn film_material_save(&mut self, coefficients: bool) -> Result<(), String> {
        let panel = self.ui.global::<ParametersModel>();
        if !panel.get_open() || panel.get_active_tab() != 0 {
            self.extra_click_dock("stone")?;
        }
        if !panel.get_open() || panel.get_active_tab() != 0 {
            return Err("Stone panel did not open".into());
        }
        self.extra_click_panel("New/Edit Material...")?;
        let model = self.ui.global::<ViewportModel>();
        if !model.get_material_editor_open() {
            return Err("Material Editor did not open".into());
        }
        if coefficients {
            self.extra_click("Coefficients")?;
            let combo = targets::run_label(&self.view(), "Dispersion Model")
                .ok_or("Dispersion Model combo is missing")?;
            self.click_and_invoke(&combo)?;
            self.rec
                .press_keys(self.ui.window(), "Up")
                .map_err(|e| format!("Sellmeier choice: {e:?}"))?;
            self.rec
                .press_keys(self.ui.window(), "Return")
                .map_err(|e| format!("Sellmeier Return: {e:?}"))?;
            let options = self
                .ui
                .global::<DispersionEditorModel>()
                .invoke_copy_options(1);
            let glass = options
                .iter()
                .position(|name| name.as_str() == "Glass (N-BK7)")
                .ok_or("Glass (N-BK7) is not offered for Sellmeier, 3 Terms")?;
            let copy =
                targets::run_label(&self.view(), "Copy Coefficients from a Built-In Material")
                    .ok_or("coefficient built-in copy combo is missing")?;
            self.click_and_invoke(&copy)?;
            for _ in 0..glass {
                self.rec
                    .press_keys(self.ui.window(), "Down")
                    .map_err(|e| format!("copy glass choice: {e:?}"))?;
            }
            self.rec
                .press_keys(self.ui.window(), "Return")
                .map_err(|e| format!("copy glass Return: {e:?}"))?;
            let coefficient = targets::text_input_start(&self.view(), "Coefficient B1")
                .ok_or("Sellmeier B1 coefficient field is missing")?;
            let old = coefficient
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_value)
                .ok_or("Sellmeier B1 has no readable value")?;
            let old_number: f64 = old
                .parse()
                .map_err(|_| format!("invalid copied B1 value {old:?}"))?;
            self.click_and_type(&coefficient, &format!("{:.7}", old_number + 0.001))?;
        }
        let name = if coefficients {
            "Tutorial coefficients"
        } else {
            "Tutorial glass"
        };
        let field = targets::text_input(&self.view(), "Material Name")
            .ok_or("Material Name field is missing")?;
        self.click_and_type(&field, name)?;
        if !coefficients {
            self.film_simple_material_sliders()?;
        }
        self.extra_click("Save")?;
        if model.get_material_editor_open() {
            return Err("Material Save did not close the editor".into());
        }
        Ok(())
    }

    fn film_cut_slider(&mut self, name: &str) -> Result<(), String> {
        let model = self.ui.global::<SolidPreviewModel>();
        let steps = model.get_cut_step_count();
        if steps < 3 {
            return Err(format!(
                "Cut Step has only {steps} steps; the lesson needs at least three"
            ));
        }
        let targets: Vec<i32> = match name {
            "cut_slider_rough" => vec![0],
            "cut_slider_step" => vec![1, 2],
            "cut_slider_finished" => vec![steps],
            _ => return Err(format!("unrecognized Cut Step route {name:?}")),
        };
        for target in targets {
            let view = self.view();
            let slider =
                targets::role_label(&view, "Cut Step", uiprobe::AccessibleRole::Slider, None)
                    .ok_or("Cut Step slider is missing")?;
            let handle = slider
                .handle
                .as_ref()
                .ok_or("Cut Step slider has no accessible element handle")?;
            let thumbs: Vec<_> = handle
                .query_descendants()
                .match_id("Slider::thumb")
                .find_all()
                .iter()
                .map(targets::Spot::of)
                .filter(|spot| spot.usable(view.size))
                .collect();
            if thumbs.len() != 1 {
                return Err(format!(
                    "Cut Step slider has {} usable rendered thumbs, expected exactly one",
                    thumbs.len()
                ));
            }
            let thumb = &thumbs[0];
            if !slider.usable(view.size) || slider.w <= thumb.w {
                return Err(format!(
                    "Cut Step slider track is unusable: slider={}x{}, thumb={}x{}",
                    slider.w, slider.h, thumb.w, thumb.h
                ));
            }
            let start = thumb.center();
            let end = (
                slider.x + thumb.w / 2.0 + (slider.w - thumb.w) * target as f32 / steps as f32,
                start.1,
            );
            self.gesture(start, end, None, false, false)?;
            let expected_cutoff = if target == 0 {
                -2
            } else if target == steps {
                -1
            } else {
                target - 1
            };
            if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                model.get_cut_position() == target as f32
                    && model.get_tier_cutoff() == expected_cutoff
            }) {
                return Err(format!(
                    "Cut Step drag to position {target}/{steps} did not commit: position={}, cutoff={}, expected cutoff={expected_cutoff}",
                    model.get_cut_position(),
                    model.get_tier_cutoff()
                ));
            }
            self.hold(pacing::RESULT_HOLD)?;
        }
        Ok(())
    }

    pub(crate) fn film_context_help(&mut self) -> Result<(), String> {
        let overview = targets::run(
            &self.view(),
            &crate::targets::Query::Tab("3D Spectral Preview"),
        )
        .ok_or("3D Spectral Preview tab is missing before F1")?;
        self.click_and_invoke(&overview)?;
        if self.ui.get_active_tab() != 0 {
            return Err("3D Spectral Preview tab did not activate before F1".into());
        }
        self.rec
            .press_keys(self.ui.window(), "F1")
            .map_err(|e| format!("3D Spectral Preview F1: {e:?}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::help_window().is_some_and(|window| {
                let model = window.global::<HelpModel>();
                !model.get_searching()
                    && !model.get_scroll_pending()
                    && model.get_chapter_title().as_str()
                        == "2. Browsing the Catalogue and Viewing a Design"
                    && model.get_scroll_section() == 6
            })
        }) {
            return Err("F1 did not open context help for 3D Spectral Preview".into());
        }
        self.attach_help_window()?;
        let help = harness_support::help_window().ok_or("Help window disappeared")?;
        let help_model = help.global::<HelpModel>();
        let first_page = help_model.get_nav().iter().find(|row| row.current);
        if help_model.get_chapter_title().as_str()
            != "2. Browsing the Catalogue and Viewing a Design"
            || help_model.get_scroll_section() != 6
            || first_page
                .as_ref()
                .is_none_or(|row| row.label.as_str() != "The 3D Viewport")
        {
            return Err(format!(
                "F1 did not open the 3D Viewport section: chapter={:?}, section={}, nav={:?}",
                help_model.get_chapter_title(),
                help_model.get_scroll_section(),
                first_page.map(|row| row.label.as_str().to_owned())
            ));
        }
        self.hold(Duration::from_secs(3))?;

        let cutting = targets::run(
            &self.view(),
            &crate::targets::Query::Tab("Cutting Instructions"),
        )
        .ok_or("Cutting Instructions tab is missing")?;
        self.click_and_invoke(&cutting)?;
        if self.ui.get_active_tab() != 1 {
            return Err("Cutting Instructions tab did not activate".into());
        }
        self.hold(Duration::from_millis(350))?;
        self.rec
            .press_keys(self.ui.window(), "F1")
            .map_err(|e| format!("Cutting Instructions F1: {e:?}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let model = help.global::<HelpModel>();
            !model.get_searching()
                && !model.get_scroll_pending()
                && model.get_chapter_title().as_str()
                    == "2. Browsing the Catalogue and Viewing a Design"
                && model.get_scroll_section() == 5
                && model
                    .get_nav()
                    .iter()
                    .any(|row| row.current && row.label.as_str() == "The Cutting Instructions Tab")
        }) {
            return Err(format!(
                "F1 did not navigate to the Cutting Instructions section: chapter={:?}, section={}, searching={}, scroll_pending={}",
                help.global::<HelpModel>().get_chapter_title(),
                help.global::<HelpModel>().get_scroll_section(),
                help.global::<HelpModel>().get_searching(),
                help.global::<HelpModel>().get_scroll_pending()
            ));
        }
        self.hold(Duration::from_secs(3))?;
        self.attach_help_window()
    }

    pub(crate) fn film_manual_search(&mut self) -> Result<(), String> {
        self.attach_help_window()?;
        let help = harness_support::help_window().ok_or("Help window disappeared")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Help window is not attached to the recording")?;
        let view = WindowView::of("HelpWindow", &help);
        let field = targets::text_input(&view, "Search the Manual")
            .ok_or("Search the Manual field is missing")?;
        self.click_secondary(help.window(), origin, &field)?;
        self.rec
            .press_keys(help.window(), "Ctrl+A")
            .map_err(|e| format!("manual search Ctrl+A: {e:?}"))?;
        self.rec
            .press_keys(help.window(), "Backspace")
            .map_err(|e| format!("manual search Backspace: {e:?}"))?;
        self.rec
            .type_text(help.window(), "mast", Duration::from_millis(55))?;
        self.hold(Duration::from_millis(220))?;
        let model = help.global::<HelpModel>();
        if model.get_search_text() != "mast" {
            return Err(format!(
                "manual search readback mismatch: {:?}",
                model.get_search_text()
            ));
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            model.get_searching() && model.get_hits().row_count() > 0
        }) {
            return Err(format!(
                "manual search for mast produced no results: searching={}, count={}",
                model.get_searching(),
                model.get_hits().row_count()
            ));
        }
        let hit = model
            .get_hits()
            .row_data(0)
            .ok_or("manual search result disappeared")?;
        let expected_title = hit.chapter.to_string();
        let expected_topic = hit.topic.to_string();
        let label = if hit.section.is_empty() {
            hit.chapter.to_string()
        } else {
            format!("{}, {}", hit.chapter, hit.section)
        };
        self.hold(pacing::RESULT_HOLD)?;
        let hit = targets::role_label(
            &WindowView::of("HelpWindow", &help),
            &label,
            uiprobe::AccessibleRole::Button,
            None,
        )
        .ok_or_else(|| format!("manual result {label:?} is not visible"))?;
        self.click_secondary(help.window(), origin, &hit)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !model.get_searching()
                && model.get_chapter_title().as_str() == expected_title.as_str()
                && !model.get_scroll_pending()
                && (0..model.get_nav().row_count()).any(|index| {
                    model.get_nav().row_data(index).is_some_and(|row| {
                        row.current && row.topic.as_str() == expected_topic.as_str()
                    })
                })
        }) {
            return Err(format!(
                "clicking manual result {label:?} did not navigate to topic {expected_topic:?}; title={:?}, searching={}, scroll_pending={}",
                model.get_chapter_title(),
                model.get_searching(),
                model.get_scroll_pending(),
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Dismiss the filmed anchor explanation with its visible primary button before
    /// subsequent lesson actions can be blocked by the modal overlay.
    pub(crate) fn film_dismiss_anchor_explainer(&mut self) -> Result<(), String> {
        let model = self.ui.global::<EditorModel>();
        if !model.get_anchor_explainer_open() {
            return Ok(());
        }
        let got_it = targets::role_label(
            &self.view(),
            "Got It",
            uiprobe::AccessibleRole::Button,
            Some("AnchorExplainer"),
        )
        .ok_or("AnchorExplainer is open but its Got It button is not visible")?;
        self.click_and_invoke(&got_it)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !model.get_anchor_explainer_open()) {
            return Err("AnchorExplainer Got It activation did not close the modal".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }
}
