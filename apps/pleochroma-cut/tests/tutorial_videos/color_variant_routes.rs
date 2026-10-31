//! Filmed routes of the two color variant lessons (`materials-color-variants` and
//! `materials-zone-colors-from-variants`).
//!
//! Every step of these lessons is routed here (`pure_tables::MODULE_SCRIPTED`), reading steps
//! included, so the match in `scripts.rs` stays a list of arms. Each route works the control the
//! step names with the real pointer and keys (the Stone tab's Body Color list and color editor,
//! the Material Editor's Color Variants section, the Color Pattern window of the stone) and then
//! reads one model value back, so a renamed button or a click that did nothing stops the video
//! with a sentence instead of filming something else. The goals of these lessons read the design,
//! so a route that changed it also waits for the guide to report the step done.
//!
//! This file is a child of `event_extras.rs`, so it can use the private helpers of the Material
//! Editor routes (`material_scroll_target`) and of the Stone tab routes.
//!
//! What the routes need at run time: the Rich Teaching Design on the bench (the lessons start from
//! it), a Sapphire with its built-in variants (Yellow is picked, any other variant of the list
//! would do) and a Tourmaline whose Color Pattern window lists variants (Rubellite and Verdelite
//! are picked when the material has them, else the first variant after the Material Default).

use crate::{
    director::Film,
    pacing, routes,
    support::WindowView,
    targets::{self, Spot},
};
use pleochroma_cut::{
    BodyColorEditorModel, ColorPatternModel, EditorModel, GuideModel, ViewportModel,
    harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::{AccessibleRole, screenshot};

/// The lessons this file routes.
const LESSONS: [&str; 2] = [
    "materials-color-variants",
    "materials-zone-colors-from-variants",
];

/// The variant the first lesson picks from the Body Color list.
const BUILT_IN_VARIANT: &str = "Yellow";

/// The name typed in the Save as Variant... row.
const SAVED_NAME: &str = "Sunset Sapphire";

/// How many wheel turns a control may need to scroll into view in the Material Editor.
const SCROLL_TRIES: usize = 24;

/// Whether this module routes the lesson `id`.
pub fn handles(id: &str) -> bool {
    LESSONS.contains(&id)
}

impl Film<'_> {
    /// Films step `step` (from 0) of the color variant lesson the film is of.
    pub(crate) fn film_color_variants(&mut self, step: usize) -> Result<(), String> {
        let id = self.id.clone();
        match (id.as_str(), step) {
            // The reading steps.
            ("materials-color-variants" | "materials-zone-colors-from-variants", 0)
            | ("materials-zone-colors-from-variants", 6) => self.hold(pacing::RESULT_HOLD),
            // Color Variants: the Body Color list, the color editor and the Material Editor.
            ("materials-color-variants", 1) => self.film_material("Sapphire"),
            ("materials-color-variants", 2) => self.pick_built_in_variant(),
            ("materials-color-variants", 3) => self.film_view_mode(1),
            ("materials-color-variants", 4) => self.set_the_along_color(),
            ("materials-color-variants", 5) => self.save_as_variant(),
            ("materials-color-variants", 6) => self.show_color_variants_section(),
            // Zone Colors From Variants: the stone's Color Pattern window.
            ("materials-zone-colors-from-variants", 1) => self.film_material("Tourmaline"),
            ("materials-zone-colors-from-variants", 2) => self.open_stone_color_pattern(),
            ("materials-zone-colors-from-variants", 3) => self.pick_pattern_template(),
            ("materials-zone-colors-from-variants", 4) => self.take_zone_colors_from_variants(),
            ("materials-zone-colors-from-variants", 5) => {
                self.close_color_pattern_window()?;
                self.film_view_mode(1)
            }
            _ => Err(format!(
                "no color variant route for step {} of {id}",
                step + 1
            )),
        }
    }

    /// Waits for the guide to report the current step done: the design was written and its goal
    /// holds.
    fn wait_color_step_done(&self, what: &str) -> Result<(), String> {
        let guide = self.ui.global::<GuideModel>();
        if pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
            Ok(())
        } else {
            Err(format!(
                "{what} did not reach the design before the timeout"
            ))
        }
    }

    // --- The Stone tab ------------------------------------------------------------------------

    /// Opens the Body Color list and moves to entry `wanted` with the keys, like a learner with the
    /// list open. Every arrow key of the open list applies its entry at once, so the list is
    /// followed one key at a time.
    fn choose_body_color_entry(&mut self, wanted: i32) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let combo = self.stone_control("the Body Color list", |view| {
            targets::role_label(
                view,
                "Body Color",
                AccessibleRole::Combobox,
                Some("ParametersPanel"),
            )
        })?;
        let start = editor.get_body_color_index();
        if start == wanted {
            return Err(format!(
                "the Body Color list is already on entry {wanted} before the lesson picks it"
            ));
        }
        self.click_and_invoke(&combo)?;
        let budget = 3 * (wanted - start).abs() + 4;
        for _ in 0..budget {
            let current = editor.get_body_color_index();
            if current == wanted {
                break;
            }
            let key = if wanted > current { "Down" } else { "Up" };
            self.rec
                .press_keys(self.ui.window(), key)
                .map_err(|e| format!("Body Color {key}: {e:?}"))?;
            if !pacing::wait_real(Duration::from_millis(600), || {
                editor.get_body_color_index() != current
            }) {
                break;
            }
        }
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|e| format!("Body Color Return: {e:?}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            editor.get_body_color_index() == wanted
        }) {
            return Err(format!(
                "the Body Color list did not select entry {wanted} (it shows entry {})",
                editor.get_body_color_index()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step 3: Yellow (or the first built-in variant) from the Body Color list.
    fn pick_built_in_variant(&mut self) -> Result<(), String> {
        let options = self.ui.global::<EditorModel>().get_body_color_options();
        let last =
            i32::try_from(options.row_count()).map_err(|_| "the Body Color list is too long")? - 1;
        let wanted = (0..options.row_count())
            .find(|&at| {
                options
                    .row_data(at)
                    .is_some_and(|label| label.as_str() == BUILT_IN_VARIANT)
            })
            .and_then(|at| i32::try_from(at).ok())
            .unwrap_or(1);
        if wanted < 1 || wanted >= last {
            return Err(format!(
                "the Body Color list ({} entries) has no color variant between Material Default and Custom...",
                last + 1
            ));
        }
        self.choose_body_color_entry(wanted)?;
        self.wait_color_step_done("the color variant")
    }

    /// Reads the number of rows of the color editor, one for each direction of the stone.
    fn color_editor_rows(&self) -> usize {
        self.ui
            .global::<BodyColorEditorModel>()
            .get_axis_labels()
            .row_count()
    }

    /// Waits until the color editor has solved the colors it was asked for.
    fn wait_color_editor_idle(&self, what: &str) -> Result<(), String> {
        let editor = self.ui.global::<BodyColorEditorModel>();
        if pacing::wait_real(Duration::from_secs(60), || !editor.get_busy()) {
            Ok(())
        } else {
            Err(format!("the color editor did not finish matching {what}"))
        }
    }

    /// Opens the color editor of the Stone tab with the Custom... entry of the Body Color list.
    fn open_color_editor_from_list(&mut self) -> Result<(), String> {
        let options = self.ui.global::<EditorModel>().get_body_color_options();
        let custom =
            i32::try_from(options.row_count()).map_err(|_| "the Body Color list is too long")? - 1;
        self.choose_body_color_entry(custom)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || self.color_editor_rows() > 0) {
            return Err("the Custom... entry did not open the color editor".into());
        }
        self.wait_color_editor_idle("the stored color")?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Clicks the chip of a direction row of the color editor, found by the start of its label
    /// ("Along the Axis: Blue").
    fn choose_direction_row(&mut self, direction: &str) -> Result<(), String> {
        let prefix = format!("{direction}:");
        let chip = self.stone_control(direction, |view| targets::run_label_start(view, &prefix))?;
        self.click_and_invoke(&chip)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Clicks the Stone tab button called `label`.
    fn press_stone_button(&mut self, label: &str) -> Result<(), String> {
        let button = self.stone_control(label, |view| {
            targets::role_label(view, label, AccessibleRole::Button, Some("ParametersPanel"))
        })?;
        self.click_and_invoke(&button)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step 5: Custom..., the Along the Axis row, a drag of Tone and Apply Color.
    fn set_the_along_color(&mut self) -> Result<(), String> {
        self.open_color_editor_from_list()?;
        if self.color_editor_rows() < 2 {
            return Err(format!(
                "the color editor has {} row(s); a sapphire shows a color along and across its axis",
                self.color_editor_rows()
            ));
        }
        self.choose_direction_row("Along the Axis")?;
        self.drag_stone_slider("Tone")?;
        self.wait_color_editor_idle("the new Along color")?;
        self.hold(pacing::RESULT_HOLD)?;
        self.press_stone_button("Apply Color")?;
        self.wait_color_step_done("the Custom Color")
    }

    /// Step 6: Choose Color..., a nudge of Tone, Save as Variant..., the name and Save.
    fn save_as_variant(&mut self) -> Result<(), String> {
        self.press_stone_button("Choose Color...")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || self.color_editor_rows() > 0) {
            return Err("Choose Color... did not open the color editor".into());
        }
        self.wait_color_editor_idle("the stored color")?;
        self.choose_direction_row("Along the Axis")?;
        self.drag_stone_slider("Tone")?;
        self.wait_color_editor_idle("the nudged color")?;
        self.press_stone_button("Save as Variant...")?;
        let field = self.stone_control("Variant Name", |view| {
            targets::text_input(view, "Variant Name")
        })?;
        self.click_and_type(&field, SAVED_NAME)?;
        self.hold(pacing::RESULT_HOLD)?;
        self.press_stone_button("Save")?;
        self.wait_color_step_done("the saved variant")
    }

    // --- The Material Editor ------------------------------------------------------------------

    /// Step 7: the Material Editor, scrolled to its Color Variants section, then Cancel.
    fn show_color_variants_section(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<ViewportModel>();
        let open = self.stone_control("New/Edit Material...", |view| {
            targets::role_label(
                view,
                "New/Edit Material...",
                AccessibleRole::Button,
                Some("ParametersPanel"),
            )
        })?;
        self.click_and_invoke(&open)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || editor.get_material_editor_open()) {
            return Err("Material Editor did not open".into());
        }
        // The section's Material list is the first control that is always there.
        let label = "Material of the Color Variants";
        self.scroll_material_editor_to(label)?;
        self.hold(pacing::RESULT_HOLD)?;
        // The rows of the material sit below the list: one more turn brings them into view.
        let (_, target) = self.material_scroll_target()?;
        self.wheel_over(&target, -120.0)?;
        self.hold(pacing::RESULT_HOLD)?;
        let cancel = self.scoped_button("MaterialEditorDialog", "Cancel")?;
        self.click_and_invoke(&cancel)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !editor.get_material_editor_open()) {
            return Err("Cancel did not close the Material Editor".into());
        }
        Ok(())
    }

    /// Turns the wheel over the Material Editor until the control labelled `label` is fully in
    /// view.
    fn scroll_material_editor_to(&mut self, label: &str) -> Result<(), String> {
        for _ in 0..SCROLL_TRIES {
            let (viewport, target) = self.material_scroll_target()?;
            let view = self.view();
            let found = targets::run_label(&view, label);
            if let Some(spot) = &found
                && let Some(clipped) = spot.clipped_to(&viewport, view.size)
                && (clipped.h - spot.h).abs() < 0.5
                && (clipped.w - spot.w).abs() < 0.5
            {
                return Ok(());
            }
            // Scroll down while the control is below the viewport or unknown, up when above it.
            let wheel = match &found {
                Some(spot) if spot.y < viewport.y => 90.0,
                _ => -90.0,
            };
            self.wheel_over(&target, wheel)?;
            self.hold(Duration::from_millis(100))?;
        }
        Err(format!(
            "Material Editor control {label:?} did not scroll into view"
        ))
    }

    // --- The Color Pattern window -------------------------------------------------------------

    /// Step 3 of the zone lesson: Color Pattern... in the Stone tab, and the window attached to
    /// the video.
    fn open_stone_color_pattern(&mut self) -> Result<(), String> {
        self.press_stone_button("Color Pattern...")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::color_pattern_window().is_some()
        }) {
            return Err("Color Pattern... did not open the Color Pattern window".into());
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
            .caption("Color the zones of the stone from its color variants.");
        self.hold(pacing::RESULT_HOLD)
    }

    /// The open Color Pattern window and the origin it is drawn at in the video.
    fn color_pattern_window(
        &self,
    ) -> Result<(pleochroma_cut::ColorPatternWindow, (f32, f32)), String> {
        let window = harness_support::color_pattern_window()
            .ok_or("the Color Pattern window is not open")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("the Color Pattern window is not attached to the video")?;
        Ok((window, origin))
    }

    /// A click on the control found by `find` in the Color Pattern window.
    fn click_pattern_spot(
        &mut self,
        what: &str,
        find: impl Fn(&WindowView) -> Option<Spot>,
    ) -> Result<(), String> {
        let (window, origin) = self.color_pattern_window()?;
        let view = WindowView::of("ColorPatternWindow", &window);
        let spot = find(&view)
            .ok_or_else(|| format!("Color Pattern window control {what:?} is missing"))?;
        self.click_secondary(window.window(), origin, &spot)?;
        self.hold(Duration::from_millis(200))
    }

    /// A click on the control labelled `label` in the Color Pattern window.
    fn click_pattern_label(&mut self, label: &str) -> Result<(), String> {
        self.click_pattern_spot(label, |view| targets::run_label(view, label))
    }

    /// Step 4 of the zone lesson: the Watermelon template.
    ///
    /// A tourmaline also has a Watermelon pill in the "Typical for Tourmaline" row above the
    /// Templates row; that one builds the species' pattern and raises no template event, so the
    /// lowest visible "Watermelon" (the Templates row) is the one clicked.
    fn pick_pattern_template(&mut self) -> Result<(), String> {
        let (window, _) = self.color_pattern_window()?;
        let model = window.global::<ColorPatternModel>();
        self.click_pattern_spot("Watermelon (Templates row)", |view| {
            view.by_label("Watermelon")
                .iter()
                .map(Spot::of)
                .filter(|spot| spot.usable(view.size))
                .max_by(|a, b| a.y.total_cmp(&b.y))
        })?;
        if !pacing::wait_real(Duration::from_secs(60), || {
            !model.get_solving_colors() && !model.get_preview_busy()
        }) {
            return Err("the Watermelon colors did not finish solving".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Selects the zone called `name` in the ZONES list (its row reads "<kind>: <name>").
    fn select_pattern_zone(&mut self, name: &str) -> Result<(), String> {
        let suffix = format!(": {name}");
        self.click_pattern_spot(name, |view| {
            let wanted = suffix.clone();
            view.root
                .query_descendants()
                .match_accessible_role(AccessibleRole::RadioButton)
                .match_predicate(move |element| {
                    element
                        .accessible_label()
                        .is_some_and(|label| label.ends_with(wanted.as_str()))
                })
                .find_all()
                .iter()
                .map(Spot::of)
                .find(|spot| spot.usable(view.size))
        })
    }

    /// Opens the combo called `label` of the Color Pattern window and moves its model index to
    /// `wanted` with the keys; `index` reads that index.
    fn choose_pattern_entry(
        &mut self,
        label: &str,
        wanted: i32,
        index: &dyn Fn() -> i32,
    ) -> Result<(), String> {
        let (window, origin) = self.color_pattern_window()?;
        let view = WindowView::of("ColorPatternWindow", &window);
        let combo = targets::role_label(&view, label, AccessibleRole::Combobox, None)
            .ok_or_else(|| format!("the {label} list is missing in the Color Pattern window"))?;
        self.click_secondary(window.window(), origin, &combo)?;
        let budget = 3 * (wanted - index()).abs() + 4;
        for _ in 0..budget {
            let current = index();
            if current == wanted {
                break;
            }
            let key = if wanted > current { "Down" } else { "Up" };
            self.rec
                .press_keys(window.window(), key)
                .map_err(|e| format!("{label} {key}: {e:?}"))?;
            if !pacing::wait_real(Duration::from_millis(600), || index() != current) {
                break;
            }
        }
        self.rec
            .press_keys(window.window(), "Return")
            .map_err(|e| format!("{label} Return: {e:?}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || index() == wanted) {
            return Err(format!(
                "the {label} list did not select entry {wanted} (it shows entry {})",
                index()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Gives the selected zone the color of the variant `name` (or the first variant after the
    /// Material Default when the material has no such variant): Source = Variant, then the list.
    fn zone_takes_variant(&mut self, zone: &str, name: &str) -> Result<(), String> {
        self.select_pattern_zone(zone)?;
        let (window, _) = self.color_pattern_window()?;
        let model = window.global::<ColorPatternModel>();
        if model.get_color_kind() != 3 {
            let source = || model.get_color_source_index();
            self.choose_pattern_entry("Color Source", 1, &source)?;
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_color_kind() == 3) {
            return Err(format!(
                "setting Source to Variant did not turn the {zone} zone into a variant color"
            ));
        }
        let labels = model.get_variant_labels();
        let wanted = (0..labels.row_count())
            .find(|&at| {
                labels
                    .row_data(at)
                    .is_some_and(|label| label.as_str().starts_with(name))
            })
            .and_then(|at| i32::try_from(at).ok())
            .or_else(|| (labels.row_count() > 1).then_some(1))
            .ok_or("the Variant list has no entry besides the Material Default")?;
        let variant = || model.get_variant_index();
        if variant() != wanted {
            self.choose_pattern_entry("Color Variant", wanted, &variant)?;
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Step 5 of the zone lesson: Core takes Rubellite, Rim takes Verdelite, Apply.
    fn take_zone_colors_from_variants(&mut self) -> Result<(), String> {
        self.zone_takes_variant("Core", "Rubellite")?;
        self.zone_takes_variant("Rim", "Verdelite")?;
        self.click_pattern_label("Apply")?;
        self.wait_color_step_done("the zone colors")
    }

    /// Done in the Color Pattern window, and the window leaves the video.
    fn close_color_pattern_window(&mut self) -> Result<(), String> {
        self.click_pattern_label("Done")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::color_pattern_window().is_none()
        }) {
            return Err("Done did not close the Color Pattern window".into());
        }
        self.rec.detach_window();
        self.attached_window = None;
        self.rec.caption("");
        Ok(())
    }
}
