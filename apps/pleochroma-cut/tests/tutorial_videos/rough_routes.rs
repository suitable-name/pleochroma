//! Visible Rough Planner routes for the rough-modeling and planning lesson steps.

use crate::{
    debug_log::{self, debug_check},
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets,
};
use pleochroma_cut::{RoughPlanModel, harness_support};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::screenshot;

fn finite_dimension(value: &str, label: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|dimension| dimension.is_finite() && *dimension > 0.0)
        .ok_or_else(|| format!("{label} is not a finite positive dimension: {value:?}"))
}

impl Film<'_> {
    pub(crate) fn film_rough_open(&mut self) -> Result<(), String> {
        self.narrow_library_for_rough_plan()?;
        self.extra_menu("Library", "Plan Rough...")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::rough_planner_window().is_some()
        }) {
            return Err("Library > Plan Rough did not open its window".into());
        }
        let rough = harness_support::rough_planner_window().ok_or("Rough Planner disappeared")?;
        screenshot::set_window_size(rough.window(), (1180, 780), crate::config::SCALE);
        for text in routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Rough Planner ({word:?}): {text:?}"
                ));
            }
        }
        if let Some(other) = self.attached_window {
            return Err(format!(
                "the {other} window is still attached to the video; the Rough Planner cannot be attached too"
            ));
        }
        let origin = self.rec.place_window(rough.window());
        self.rec.attach_window(&rough, origin);
        self.attached_window = Some("rough planner");
        let rough_model = rough.global::<RoughPlanModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || rough_model.get_filter_count() >= 0) {
            return Err("Rough Planner did not finish counting the Current Filter designs".into());
        }
        self.hold(Duration::from_millis(220))?;
        self.rec.chapter("Rough Planner");
        self.rec.caption("Model the rough and plan a layout.");
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(crate) fn film_rough_model(&mut self) -> Result<(), String> {
        let rough =
            harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Rough Planner is not attached to the video")?;
        let model = rough.global::<RoughPlanModel>();
        let window = rough.window();

        let block = self.rough_control(
            window,
            origin,
            "Block",
            uiprobe::AccessibleRole::RadioButton,
        )?;
        self.click_secondary(window, origin, &block)?;
        self.hold(Duration::from_millis(180))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_base_kind() == 0) {
            return Err("Block base radio did not select the block model".into());
        }

        for (label, value) in [
            ("Rough X (mm)", "18.4"),
            ("Rough Y (mm)", "11.0"),
            ("Rough Z (mm)", "9.6"),
        ] {
            let read = || match label {
                "Rough X (mm)" => model.get_rough_x().to_string(),
                "Rough Y (mm)" => model.get_rough_y().to_string(),
                _ => model.get_rough_z().to_string(),
            };
            // The first click into the freshly attached window can be spent on focus alone, so
            // a field that did not take its text is clicked and typed once more before failing.
            let mut seen = Vec::new();
            for attempt in 1..=2 {
                let field =
                    self.rough_control(window, origin, label, uiprobe::AccessibleRole::TextInput)?;
                self.type_in_secondary(window, origin, &field, value)?;
                if pacing::wait_real(Duration::from_secs(2), || read() == value) {
                    seen.clear();
                    break;
                }
                seen.push(format!(
                    "attempt {attempt}: readback={:?}, field shows={:?} at ({:.0}, {:.0}) size {:.0} x {:.0}",
                    read(),
                    field
                        .handle
                        .as_ref()
                        .and_then(uiprobe::ElementHandle::accessible_value),
                    field.x,
                    field.y,
                    field.w,
                    field.h
                ));
            }
            debug_check(label, &value, &read());
            if !seen.is_empty() {
                return Err(format!(
                    "{label} readback mismatch: wanted {value:?}; {}; X={:?}, Y={:?}, Z={:?}, base_kind={}, running={}, model_error={:?}",
                    seen.join("; "),
                    model.get_rough_x(),
                    model.get_rough_y(),
                    model.get_rough_z(),
                    model.get_base_kind(),
                    model.get_running(),
                    model.get_model_error()
                ));
            }
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !model.get_model_text().is_empty() && model.get_model_error().is_empty()
        }) {
            return Err(format!(
                "rough model did not produce a valid live readout: {:?}",
                model.get_model_error()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;

        let carat = self.rough_control(
            window,
            origin,
            "Carat (ct)",
            uiprobe::AccessibleRole::TextInput,
        )?;
        self.type_in_secondary(window, origin, &carat, "8.00")?;
        debug_check("weighed Carat", &"8.00", &model.get_weighed_ct());
        if model.get_weighed_ct().as_str() != "8.00"
            || !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                model.get_weight_check_level() > 0 && !model.get_weight_check_text().is_empty()
            })
        {
            return Err(format!(
                "weighed Carat did not produce a weight check: weighed={:?}, level={}, text={:?}",
                model.get_weighed_ct(),
                model.get_weight_check_level(),
                model.get_weight_check_text()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;

        let before_fit = [
            finite_dimension(model.get_rough_x().as_str(), "Rough X")?,
            finite_dimension(model.get_rough_y().as_str(), "Rough Y")?,
            finite_dimension(model.get_rough_z().as_str(), "Rough Z")?,
        ];
        let fit = self.rough_control(
            window,
            origin,
            "Fit to Weight",
            uiprobe::AccessibleRole::Button,
        )?;
        self.click_secondary(window, origin, &fit)?;
        self.hold(Duration::from_millis(180))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let dimensions = [
                model.get_rough_x(),
                model.get_rough_y(),
                model.get_rough_z(),
            ];
            let changed = dimensions.iter().zip(before_fit).any(|(value, before)| {
                value
                    .parse::<f64>()
                    .is_ok_and(|after| (after - before).abs() > 1.0e-5)
            });
            changed
                && dimensions
                    .iter()
                    .all(|value| finite_dimension(value.as_str(), "rough").is_ok())
                && model.get_model_error().is_empty()
                && model
                    .get_weighed_ct()
                    .parse::<f64>()
                    .is_ok_and(|ct| (ct - 8.0).abs() <= 0.01)
        }) {
            debug_check("fitted weighed_ct", &"8.00", &model.get_weighed_ct());
            debug_check("fitted check level", &0, &model.get_weight_check_level());
            return Err(format!(
                "Fit to Weight did not change finite dimensions and keep the weighed Carat at 8.00: x={:?}, y={:?}, z={:?}, weighed={:?}, level={}, text={:?}, error={:?}",
                model.get_rough_x(),
                model.get_rough_y(),
                model.get_rough_z(),
                model.get_weighed_ct(),
                model.get_weight_check_level(),
                model.get_weight_check_text(),
                model.get_model_error()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;

        for (kind, label) in [(0, "+Edge"), (1, "+Corner"), (2, "+Face")] {
            let before = model.get_cuts().row_count();
            let button =
                self.rough_control(window, origin, label, uiprobe::AccessibleRole::Button)?;
            self.click_secondary(window, origin, &button)?;
            self.hold(Duration::from_millis(180))?;
            if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                model.get_cuts().row_count() == before + 1
                    && model
                        .get_cuts()
                        .row_data(before)
                        .is_some_and(|row| row.kind == kind && row.error.is_empty())
            }) {
                return Err(format!(
                    "{label} did not add a valid rough cut: count={}, error={:?}",
                    model.get_cuts().row_count(),
                    model.get_model_error()
                ));
            }
            self.hold(pacing::RESULT_HOLD)?;
        }
        Ok(())
    }

    /// Reacquires a rough form/cut control after scrolling the actual left-column viewport.
    pub(crate) fn rough_control(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        label: &str,
        role: uiprobe::AccessibleRole,
    ) -> Result<targets::Spot, String> {
        // Every call starts from the top of the actual RoughForm/CutList viewport. This makes
        // the bounded downward reveal deterministic even when a preceding control left the
        // scroll position near the bottom (where an offscreen role has no accessible bounds).
        let view = WindowView::of(
            "RoughPlannerWindow",
            &harness_support::rough_planner_window().ok_or("Rough Planner disappeared")?,
        );
        let viewport = targets::run(
            &view,
            &crate::targets::Query::Id("RoughPlannerWindow::left_scroll"),
        )
        .ok_or("Rough Planner rough/shape scroll viewport is missing")?;
        self.rec
            .scroll_at_in(window, viewport.center(), origin, 10_000.0)?;
        self.hold(Duration::from_millis(140))?;
        for _ in 0..24 {
            let view = WindowView::of(
                "RoughPlannerWindow",
                &harness_support::rough_planner_window().ok_or("Rough Planner disappeared")?,
            );
            let viewport = targets::run(
                &view,
                &crate::targets::Query::Id("RoughPlannerWindow::left_scroll"),
            )
            .ok_or("Rough Planner rough/shape scroll viewport is missing")?;
            let control = targets::role_label(&view, label, role, None);
            if let Some(clipped) = control
                .as_ref()
                .and_then(|spot| spot.clipped_to(&viewport, view.size))
            {
                return Ok(clipped);
            }
            let delta =
                control.as_ref().map_or(
                    -260.0,
                    |spot| {
                        if spot.y < viewport.y { 220.0 } else { -260.0 }
                    },
                );
            let scroll_at = viewport.center();
            self.rec.scroll_at_in(window, scroll_at, origin, delta)?;
            self.hold(Duration::from_millis(140))?;
        }
        Err(format!(
            "Rough Planner control {label:?} did not enter the left scroll viewport"
        ))
    }

    fn narrow_library_for_rough_plan(&mut self) -> Result<(), String> {
        let library = self.ui.global::<pleochroma_cut::LibraryModel>();
        if self.ui.get_active_tab() != 0 {
            let tab = targets::run(
                &self.view(),
                &crate::targets::Query::Tab("3D Spectral Preview"),
            )
            .ok_or("3D Spectral Preview tab is missing before library search")?;
            self.click_and_invoke(&tab)?;
        }
        if library.get_panel_collapsed() {
            let show = targets::show_library_button(&self.view())
                .ok_or("Show Library button is missing before rough planning")?;
            self.click_and_invoke(&show)?;
        }
        if !library.get_search_text().is_empty()
            && let Some(clear) = targets::role_label(
                &self.view(),
                "Clear Search",
                uiprobe::AccessibleRole::Button,
                None,
            )
        {
            self.click_and_invoke(&clear)?;
            if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                library.get_search_text().is_empty() && library.get_matched_count() > 0
            }) {
                return Err(
                    "Library search did not clear before narrowing for Rough Planner".into(),
                );
            }
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || library.get_matched_count() > 0) {
            return Err(
                "Library did not finish listing candidates before Rough Planner search".into(),
            );
        }
        let listed = library.get_diagram_list();
        let total = usize::try_from(library.get_matched_count()).unwrap_or_default();
        let first = listed
            .row_data(0)
            .ok_or("Library has no design to narrow for Rough Planner")?;
        let title = first.title.to_string();
        let mut query = None;
        for size in 3..=title.chars().count() {
            let prefix: String = title.chars().take(size).collect();
            let matches = listed
                .iter()
                .filter(|row| {
                    row.title
                        .to_string()
                        .to_lowercase()
                        .contains(&prefix.to_lowercase())
                })
                .count();
            if matches > 0 && matches < total {
                query = Some(prefix);
                break;
            }
        }
        let query = query.ok_or_else(|| {
            format!("Library search cannot narrow {total} listed designs with the first title")
        })?;
        let search = targets::text_input(&self.view(), "Search the Library")
            .ok_or("Search the Library input is missing")?;
        self.click_and_type(&search, &query)?;
        debug_check("Library search text", &query, &library.get_search_text());
        if library.get_search_text().as_str() != query {
            return Err(format!(
                "Library search did not read back {query:?}: {:?}",
                library.get_search_text()
            ));
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let count = library.get_matched_count();
            count > 0 && usize::try_from(count).is_ok_and(|count| count < total)
        }) {
            return Err(format!(
                "Library search {query:?} did not narrow the list from {total} designs"
            ));
        }
        self.hold(Duration::from_millis(220))
    }

    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(crate) fn film_rough_finish(&mut self) -> Result<(), String> {
        let rough =
            harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Rough Planner is not attached to video")?;
        let model = rough.global::<RoughPlanModel>();
        // These are PlanForm controls pinned below left_scroll. Set them in this planning step,
        // after the filmed library-search prerequisite and after rough modeling is complete.
        let view = WindowView::of("RoughPlannerWindow", &rough);
        let designs = targets::role_label(
            &view,
            "Most Designs per Layout",
            uiprobe::AccessibleRole::Spinbox,
            None,
        )
        .ok_or("Rough Planner Designs (Up To) spin box is missing")?;
        let window_viewport = targets::Spot {
            x: 0.0,
            y: 0.0,
            w: view.size.0,
            h: view.size.1,
            handle: None,
        };
        let designs_point = designs
            .visible_point_in(&window_viewport, view.size)
            .ok_or("Rough Planner Designs (Up To) spin box is outside the visible PlanForm")?;
        self.rec.click_at(rough.window(), designs_point, origin)?;
        self.rec
            .press_keys(rough.window(), "Ctrl+A")
            .map_err(|e| format!("Designs (Up To) Ctrl+A: {e:?}"))?;
        self.rec
            .press_keys(rough.window(), "Backspace")
            .map_err(|e| format!("Designs (Up To) Backspace: {e:?}"))?;
        self.rec
            .type_text(rough.window(), "1", Duration::from_millis(55))?;
        self.rec
            .press_keys(rough.window(), "Return")
            .map_err(|e| format!("Designs (Up To) Return: {e:?}"))?;
        self.hold(Duration::from_millis(180))?;
        debug_check("Designs (Up To)", &1, &model.get_max_designs());
        if model.get_max_designs() != 1 {
            return Err(format!(
                "Designs (Up To) did not read back as 1: {}",
                model.get_max_designs()
            ));
        }
        if !model.get_use_filter() {
            // The count is appended dynamically; target the label prefix and the exact
            // RadioButton role so its adjacent caption cannot receive the activation.
            let filter_choice = targets::role_label_start(
                &view,
                "Current Filter",
                uiprobe::AccessibleRole::RadioButton,
                None,
            )
            .ok_or("Rough Planner Current Filter radio choice is missing")?;
            let point = filter_choice
                .visible_point_in(&window_viewport, view.size)
                .ok_or("Rough Planner Current Filter choice is outside the visible PlanForm")?;
            self.rec.click_at(rough.window(), point, origin)?;
            self.hold(Duration::from_millis(180))?;
        }
        if model.get_max_designs() != 1 || !model.get_use_filter() || model.get_filter_count() <= 0
        {
            return Err(format!(
                "Rough Planner plan inputs changed before Plan: max_designs={}, use_filter={}, filter_count={}",
                model.get_max_designs(),
                model.get_use_filter(),
                model.get_filter_count()
            ));
        }
        if model.get_base_kind() != 0
            || model.get_cuts().row_count() != 3
            || model.get_model_text().is_empty()
            || !model.get_model_error().is_empty()
        {
            return Err(format!(
                "Plan requires the modeled block rough and its three filmed cuts: base={}, cuts={}, readout={:?}, error={:?}",
                model.get_base_kind(),
                model.get_cuts().row_count(),
                model.get_model_text(),
                model.get_model_error()
            ));
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            // The app writes the removed-volume note on one row only, so the wait is for: no cut
            // error, no model error, a model readout and an idle planner and mesh worker.
            (0..model.get_cuts().row_count()).all(|index| {
                model
                    .get_cuts()
                    .row_data(index)
                    .is_some_and(|row| row.error.is_empty())
            }) && model.get_model_error().is_empty()
                && !model.get_model_text().is_empty()
                && !model.get_running()
                && !model.get_mesh_busy()
        }) {
            return Err(
                "Rough Planner cut shapes did not finish their latest evaluation before Plan"
                    .into(),
            );
        }
        let plan = targets::role_label(&view, "Plan", uiprobe::AccessibleRole::Button, None)
            .ok_or("Rough Planner Plan button is missing")?;
        let window_viewport = targets::Spot {
            x: 0.0,
            y: 0.0,
            w: view.size.0,
            h: view.size.1,
            handle: None,
        };
        let plan_point = plan
            .visible_point_in(&window_viewport, view.size)
            .ok_or("Rough Planner Plan button is outside the visible pinned PlanForm")?;
        self.rec.click_at(rough.window(), plan_point, origin)?;
        pump(60);
        if !pacing::wait_lapse(&mut self.rec, Duration::from_secs(180), || {
            !model.get_running()
                && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
        })? {
            return Err("Rough Planner did not finish within 180 seconds".into());
        }
        if model.get_results().row_count() == 0 {
            return Err(format!(
                "Rough Planner returned no layouts: {}",
                model.get_error_text()
            ));
        }
        for text in routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Rough Planner results ({word:?}): {text:?}"
                ));
            }
        }
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    pub(crate) fn click_secondary(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &targets::Spot,
    ) -> Result<(), String> {
        let (x, y) = spot.center();
        debug_log::target(spot);
        self.rec.click_at(window, (x, y), origin)?;
        pump(60);
        Ok(())
    }

    pub(crate) fn type_in_secondary(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &targets::Spot,
        text: &str,
    ) -> Result<(), String> {
        self.click_secondary(window, origin, spot)?;
        self.rec
            .press_keys(window, "Ctrl+A")
            .map_err(|e| format!("rough field Ctrl+A: {e:?}"))?;
        self.rec
            .press_keys(window, "Backspace")
            .map_err(|e| format!("rough field Backspace: {e:?}"))?;
        self.rec
            .type_text(window, text, Duration::from_millis(55))?;
        self.rec
            .press_keys(window, "Return")
            .map_err(|e| format!("rough field Return: {e:?}"))?;
        self.hold(Duration::from_millis(160))?;
        Ok(())
    }
}
