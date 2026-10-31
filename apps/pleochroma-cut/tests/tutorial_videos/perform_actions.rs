//! Secondary filmed widget routes for `Perform` recipes: steps ladder, mirroring, tier notes,
//! material selection, yield, solid view modes, inspector tabs, and toolbar commands.

use crate::{
    debug_log::{self, debug_check, debug_check_with},
    director::Film,
    pacing,
    scripts::pure_tables::PALETTE_COMMANDS,
    support::app::pump,
    targets::{self, Query, Resolved, Spot},
};
use pleochroma_cut::{
    CommandPaletteModel, EditorModel, GuideModel, ParametersModel, SolidPreviewModel,
};
use pleochroma_editor::guide::{
    CMD_ADOPT_ALL, CMD_COMPARE, CMD_DELETE, CMD_SOLVE, CMD_UNDO, StepSeries,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;

const FIELD_PAUSE: Duration = Duration::from_millis(200);

/// The component that holds the Steps / Mirror panel. The Tier form in the inspector carries
/// fields with the same labels ("Index Positions"), so every lookup is scoped to it.
const TOOLBAR_COMPONENT: &str = "TierTableToolbar";

/// What the accessible label of the trash button of a tier row starts with (`TierTableRow`, the
/// `CompactIcon` whose hint reads "Remove this tier (Delete).").
const REMOVE_BUTTON: &str = "Remove this tier";

/// Whether a typed value and its readback say the same: equal as numbers when both are numbers
/// (the app shows "0.50" for a typed "0.5"), else equal as trimmed text.
fn same_value(actual: &str, expected: &str) -> bool {
    match (actual.trim().parse::<f64>(), expected.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => (a - b).abs() <= 1.0e-9,
        _ => actual.trim() == expected.trim(),
    }
}
impl Film<'_> {
    /// The toolbar is above the row `ScrollView`, not inside it. Its clipping
    /// boundary is the whole editor dock (which scrolls on a short window).
    fn toolbar_viewport(&self) -> Option<Spot> {
        let view = self.view();
        let toolbar = view
            .root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()?;
        let dock = targets::run(&view, &Query::Id("EditorView::dock_scroll"))?;
        Spot::of(&toolbar).clipped_to(&dock, view.size)
    }

    /// The visible text input labelled `label` inside the Steps / Mirror toolbar panel.
    fn toolbar_input(&self, label: &str) -> Option<Spot> {
        let view = self.view();
        let toolbar = view
            .root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()?;
        let viewport = self.toolbar_viewport()?;
        toolbar
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_label().is_some_and(|name| name == label))
            .map(|element| Spot::of(&element))
            .find_map(|spot| spot.clipped_to(&viewport, view.size))
    }

    /// The visible element labelled exactly `label` inside the Steps / Mirror toolbar panel.
    fn toolbar_label(&self, label: &str) -> Option<Spot> {
        let view = self.view();
        let viewport = self.toolbar_viewport()?;
        targets::role_label(
            &view,
            label,
            uiprobe::AccessibleRole::Button,
            Some(TOOLBAR_COMPONENT),
        )
        .and_then(|spot| spot.clipped_to(&viewport, view.size))
    }

    /// What every text input of the Steps / Mirror panel reads right now, for failure messages.
    fn toolbar_values(&self) -> String {
        let view = self.view();
        let Some(toolbar) = view
            .root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()
        else {
            return "no tier table toolbar".to_owned();
        };
        let fields: Vec<String> = toolbar
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .map(|element| {
                format!(
                    "{:?}={:?}",
                    element.accessible_label(),
                    element.accessible_value()
                )
            })
            .collect();
        fields.join(", ")
    }

    /// Clicks the Steps / Mirror field `label` and types `text` into it, then reads the field
    /// again from a fresh lookup (the handle found before the click can go stale if the panel is
    /// rebuilt) and requires it to hold `text`. One retry; a failure names what every field of
    /// the panel read and whether the app thought a text field had the focus.
    fn type_toolbar_field(&mut self, label: &str, text: &str) -> Result<(), String> {
        let mut seen = Vec::new();
        for _attempt in 0..2 {
            let spot = self
                .toolbar_input(label)
                .ok_or_else(|| format!("Steps / Mirror: missing field {label:?} in the toolbar"))?;
            if !spot.usable(self.view().size) {
                return Err(format!(
                    "Steps / Mirror: field {label:?} is outside the window"
                ));
            }
            let window = self.ui.window();
            let (cx, cy) = spot.center();
            self.rec
                .click_at(window, (cx, cy), (0.0, 0.0))
                .map_err(|e| format!("Steps / Mirror {label:?} click: {e:?}"))?;
            self.cursor = (cx, cy);
            pump(30);
            let focused = self.ui.global::<EditorModel>().get_text_focus();
            self.rec
                .press_keys(window, "Ctrl+A")
                .map_err(|e| format!("Steps / Mirror {label:?} select all: {e:?}"))?;
            self.rec
                .press_keys(window, "Backspace")
                .map_err(|e| format!("Steps / Mirror {label:?} clear: {e:?}"))?;
            pump(20);
            self.rec
                .type_text(window, text, Duration::from_millis(55))
                .map_err(|e| format!("Steps / Mirror {label:?} typing: {e:?}"))?;
            pump(40);
            let fresh = self
                .toolbar_input(label)
                .and_then(|spot| spot.handle)
                .and_then(|handle| handle.accessible_value())
                .map(|value| value.to_string());
            debug_check("toolbar field value", &Some(text), &fresh.as_deref());
            if fresh.as_deref() == Some(text) {
                return self.rec.hold(FIELD_PAUSE);
            }
            seen.push(format!(
                "click at ({cx:.0}, {cy:.0}), text focus after click {focused}, field read {fresh:?}, panel reads [{}]",
                self.toolbar_values()
            ));
        }
        Err(format!(
            "Steps / Mirror: typed field {label:?} readback mismatch: expected {text:?}; {}",
            seen.join("; then ")
        ))
    }

    /// Makes the Keep Linked tick box of the Steps / Mirror panel read `linked`, clicking it only
    /// when it reads the other way.
    fn set_keep_linked(&mut self, linked: bool) -> Result<(), String> {
        let checkbox = self
            .view()
            .root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()
            .ok_or("Steps / Mirror: the tier table toolbar is missing")?
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Checkbox)
            .find_all()
            .into_iter()
            .find(|element| {
                element
                    .accessible_label()
                    .is_some_and(|name| name == "Keep Linked")
            })
            .ok_or_else(|| "Steps / Mirror: Keep Linked checkbox is missing".to_owned())?;
        let checked = checkbox
            .accessible_checked()
            .ok_or_else(|| "Keep Linked has no checked-state readback".to_owned())?;
        let viewport = self
            .toolbar_viewport()
            .ok_or("Steps / Mirror toolbar viewport is missing")?;
        let checkbox_spot = Spot::of(&checkbox)
            .clipped_to(&viewport, self.view().size)
            .ok_or("Keep Linked checkbox is clipped outside the toolbar")?;
        if checked != linked {
            self.click_and_invoke(&checkbox_spot)?;
        }
        debug_check(
            "Keep Linked checked",
            &Some(linked),
            &checkbox.accessible_checked(),
        );
        if checkbox.accessible_checked() != Some(linked) {
            return Err(format!(
                "Keep Linked readback: wanted {linked}, got {:?}",
                checkbox.accessible_checked()
            ));
        }
        Ok(())
    }

    /// Films opening the Steps / Mirror toolbar panel, typing each step series field, and clicking Generate.
    pub(crate) fn film_steps(&mut self, series: &StepSeries) -> Result<(), String> {
        if self.ui.global::<EditorModel>().get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(50);
        if self.toolbar_input("Ladder Name").is_none() {
            let toggle = targets::role_label(
                &self.view(),
                "Steps / Mirror",
                uiprobe::AccessibleRole::Button,
                Some(TOOLBAR_COMPONENT),
            )
            .ok_or("Steps / Mirror panel toggle is missing")?;
            self.click_and_invoke(&toggle)?;
            self.hold(FIELD_PAUSE)?;
        }
        let count_str = series.count.to_string();
        for (label, val) in [
            ("Ladder Name", series.name.as_str()),
            ("Start Angle in Degrees", series.start.as_str()),
            ("Step in Degrees", series.step.as_str()),
            ("Number of Tiers", count_str.as_str()),
            ("Index Positions", series.indices.as_str()),
            ("Anchor Scale Value", series.anchor.as_str()),
        ] {
            self.type_toolbar_field(label, val)?;
        }
        self.set_keep_linked(series.linked)?;
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let generate = self
            .toolbar_label("Generate")
            .ok_or_else(|| "Steps / Mirror: Generate is missing in the toolbar".to_owned())?;
        self.click_and_invoke(&generate)?;
        pump(80);
        let tiers = editor.get_tiers();
        if tiers.row_count()
            != before + usize::try_from(series.count).map_err(|_| "invalid series count")?
        {
            return Err("Generate did not add exactly the requested tiers".into());
        }
        let start = series
            .start
            .parse::<f64>()
            .map_err(|_| "series start is not numeric")?;
        let increment = series
            .step
            .parse::<f64>()
            .map_err(|_| "series increment is not numeric")?;
        for rung in 0..series.count {
            let name = format!("{}{}", series.name, rung + 1);
            let rows: Vec<_> = tiers
                .iter()
                .filter(|row| row.name.as_str() == name)
                .collect();
            if rows.len() != 1 {
                return Err(format!(
                    "Generate: expected unique {name:?}, got {} rows",
                    rows.len()
                ));
            }
            let row = &rows[0];
            let angle = row
                .angle_full
                .parse::<f64>()
                .map_err(|_| format!("{name}: invalid angle"))?;
            let expected = f64::from(rung).mul_add(increment, start);
            if (angle - expected).abs() > 1e-8
                || (rung == 0 && !row.relation_text.is_empty())
                || (rung > 0 && series.linked == row.relation_text.is_empty())
            {
                return Err(format!(
                    "{name}: angle={}, expected={expected}, relation={:?}, Keep Linked={}",
                    row.angle_full, row.relation_text, series.linked
                ));
            }
        }
        // The model readback above proves the generated values; reveal each row in the
        // table so the recording also shows its visible name and angle, even when the
        // ladder is longer than the viewport.
        for rung in 0..series.count {
            let name = format!("{}{}", series.name, rung + 1);
            self.film_reveal_tier(&name)?;
            self.hold(Duration::from_millis(1500))?;
        }
        Ok(())
    }

    /// Films mirroring a tier to the other block via the Steps / Mirror toolbar panel.
    pub(crate) fn film_mirror_tier(&mut self, tier: &str, suffix: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        if self
            .toolbar_input("Name Suffix for the Mirrored Tier")
            .is_none()
        {
            let toggle = targets::role_label(
                &self.view(),
                "Steps / Mirror",
                uiprobe::AccessibleRole::Button,
                Some(TOOLBAR_COMPONENT),
            )
            .ok_or("Steps / Mirror panel toggle is missing")?;
            self.click_and_invoke(&toggle)?;
            self.hold(FIELD_PAUSE)?;
        }
        let spot = self
            .toolbar_input("Name Suffix for the Mirrored Tier")
            .ok_or("Mirror suffix input is missing in the toolbar")?;
        self.click_and_type(&spot, suffix)?;
        let btn = self
            .toolbar_label("Mirror")
            .ok_or("Mirror button is missing in the toolbar")?;
        self.click_and_invoke(&btn)?;
        pump(80);
        Ok(())
    }

    /// Films clicking "Remove Relation" on a tier.
    pub(crate) fn film_clear_relation(&mut self, tier: &str) -> Result<(), String> {
        let row = self.film_select_tier(tier)?;
        let remove = self
            .inspector_form_control("Remove Relation", uiprobe::AccessibleRole::Button)?
            .ok_or("Remove Relation button is missing")?;
        self.click_and_invoke(&remove)?;
        pump(80);
        if self
            .ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(usize::try_from(row).map_err(|_| format!("{tier}: no row index"))?)
            .is_none_or(|row| !row.relation_text.is_empty())
        {
            return Err(format!("{tier}: Remove Relation did not free the tier"));
        }
        Ok(())
    }

    /// Films setting or clearing the selected tier's note with the matching visible button.
    pub(crate) fn film_tier_note(&mut self, tier: &str, text: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        let editor = self.ui.global::<EditorModel>();
        if text.is_empty() {
            let clear = self
                .inspector_form_control("Clear Note", uiprobe::AccessibleRole::Button)?
                .ok_or("Clear Note button is missing")?;
            self.click_and_invoke(&clear)?;
        } else {
            let spot = self
                .input_spot("Note for This Tier")?
                .ok_or("tier note input is missing")?;
            self.click_and_type(&spot, text)?;
            let btn = self
                .inspector_form_control("Set Note", uiprobe::AccessibleRole::Button)?
                .ok_or("Set Note button is missing")?;
            self.click_and_invoke(&btn)?;
        }
        pump(80);
        self.reselect_tier_to_verify_commit(tier)?;
        self.verify_tier_annotation(
            "Note for This Tier",
            text,
            editor.get_selected_tier_note_text().as_str(),
        )?;
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    /// Films setting or clearing the selected tier's cheater offset with its visible button.
    pub(crate) fn film_cheater_offset(&mut self, tier: &str, text: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        let editor = self.ui.global::<EditorModel>();
        if text.is_empty() {
            let clear = self
                .inspector_form_control("Clear Cheater Offset", uiprobe::AccessibleRole::Button)?
                .ok_or("Clear Cheater Offset button is missing")?;
            self.click_and_invoke(&clear)?;
        } else {
            let spot = self
                .input_spot("Cheater Offset in Degrees")?
                .ok_or("cheater offset input is missing")?;
            self.click_and_type(&spot, text)?;
            let btn = self
                .inspector_form_control("Set Cheater Offset", uiprobe::AccessibleRole::Button)?
                .ok_or("Set Cheater Offset button is missing")?;
            self.click_and_invoke(&btn)?;
        }
        pump(80);
        self.reselect_tier_to_verify_commit(tier)?;
        self.verify_tier_annotation(
            "Cheater Offset in Degrees",
            text,
            editor.get_selected_tier_cheater_offset_text().as_str(),
        )?;
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    fn reselect_tier_to_verify_commit(&mut self, tier: &str) -> Result<(), String> {
        let other = self
            .ui
            .global::<EditorModel>()
            .get_tiers()
            .iter()
            .find(|row| row.name.as_str() != tier)
            .map(|row| row.name.to_string())
            .ok_or("cannot verify annotation commit without another tier")?;
        self.film_select_tier(&other)?;
        self.film_select_tier(tier)?;
        Ok(())
    }

    pub(super) fn verify_tier_annotation(
        &mut self,
        label: &str,
        expected: &str,
        model_value: &str,
    ) -> Result<(), String> {
        // The same number in another spelling ("0.5" and "0.50") is the same value.
        debug_check_with(
            "annotation model value",
            &expected,
            &model_value,
            same_value(model_value, expected),
        );
        if !same_value(model_value, expected) {
            return Err(format!(
                "{label} model readback mismatch: expected {expected:?}, observed {model_value:?}"
            ));
        }
        let field = self
            .inspector_form_control(label, uiprobe::AccessibleRole::TextInput)?
            .ok_or_else(|| format!("{label} input is missing"))?;
        let actual = field
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_value)
            .ok_or_else(|| format!("{label} has no visible value readback"))?;
        debug_check_with(
            "annotation visible value",
            &expected,
            &actual,
            same_value(&actual, expected),
        );
        if !same_value(&actual, expected) {
            return Err(format!(
                "{label} visible readback mismatch: expected {expected:?}, observed {actual:?}"
            ));
        }
        Ok(())
    }

    /// Films choosing a material in the Stone tab of the Parameters dock.
    pub(crate) fn film_material(&mut self, name: &str) -> Result<(), String> {
        let params = self.ui.global::<ParametersModel>();
        if !params.get_open() || params.get_active_tab() != 0 {
            let btn = targets::dock_tab_button(&self.view(), "stone")
                .or_else(|| targets::run_label(&self.view(), "Edit in Stone Tab"))
                .ok_or("Stone tab button is missing")?;
            self.click_and_invoke(&btn)?;
            pump(100);
            if !params.get_open() || params.get_active_tab() != 0 {
                return Err("Stone tab click did not open the tab".into());
            }
            self.hold(pacing::STAGE_HOLD)?;
        }
        let combo = targets::role_label(
            &self.view(),
            "Material",
            uiprobe::AccessibleRole::Combobox,
            Some("StoneTab"),
        )
        .ok_or("Material combo is missing in the Stone tab")?;
        let editor = self.ui.global::<EditorModel>();
        let options = editor.get_material_combo_options();
        let wanted = name.trim();
        let index = (0..options.row_count())
            .find(|&at| {
                options
                    .row_data(at)
                    .is_some_and(|opt| opt.trim().eq_ignore_ascii_case(wanted))
            })
            .and_then(|at| i32::try_from(at).ok())
            .ok_or_else(|| format!("'{wanted}' is not in the Material list."))?;
        self.click_and_invoke(&combo)?;
        // Every arrow key of the open list applies the material at once (`selected` fires per
        // key), so each press waits for the combo to follow it. The indices seen after each key
        // are kept: if the app ever sends the combo back, the error shows where.
        let start = editor.get_material_combo_index();
        let mut trace = vec![start];
        let mut escaped = false;
        let budget = 3 * (index - start).abs() + 4;
        for _ in 0..budget {
            let current = editor.get_material_combo_index();
            if current == index {
                break;
            }
            let key = if index > current { "Down" } else { "Up" };
            self.rec
                .press_keys(self.ui.window(), key)
                .map_err(|e| format!("Material {key}: {e:?}"))?;
            let moved = pacing::wait_real(Duration::from_millis(600), || {
                editor.get_material_combo_index() != current
            });
            trace.push(editor.get_material_combo_index());
            if !moved {
                if escaped {
                    break;
                }
                // The list did not take the key: close it, which hands the focus back to the
                // combo itself, whose own arrow keys select the same way.
                escaped = true;
                self.rec
                    .press_keys(self.ui.window(), "Escape")
                    .map_err(|e| format!("Material Escape: {e:?}"))?;
                pump(60);
            }
        }
        if !escaped {
            self.rec
                .press_keys(self.ui.window(), "Return")
                .map_err(|e| format!("Material Return: {e:?}"))?;
        }
        let settled = pacing::wait_real(Duration::from_secs(2), || {
            editor.get_material_combo_index() == index
        });
        pump(150);
        debug_check(
            "Material combo index",
            &index,
            &editor.get_material_combo_index(),
        );
        if !settled || editor.get_material_combo_index() != index {
            let guide = self.ui.global::<GuideModel>();
            return Err(format!(
                "Material combo selected {}, expected {index} ({wanted}); start {start}, indices after each key {trace:?}, closed with Escape: {escaped}, combo shows {:?}, enabled {:?}, guide hint {:?}, waiting {:?}",
                editor.get_material_combo_index(),
                combo
                    .handle
                    .as_ref()
                    .and_then(uiprobe::ElementHandle::accessible_value),
                combo
                    .handle
                    .as_ref()
                    .and_then(uiprobe::ElementHandle::accessible_enabled),
                guide.get_locked_hint(),
                guide.get_current().waiting
            ));
        }
        self.hold(FIELD_PAUSE)
    }

    /// Films typing the Girdle Diameter in the Preform tab and clicking "Apply Yield Inputs".
    pub(crate) fn film_yield(&mut self, girdle_diameter_mm: f64) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 1 || editor.get_inspector_collapsed() {
            let pill =
                targets::inspector_tab_pill(&self.view(), 1).ok_or("Preform tab is missing")?;
            self.click_and_invoke(&pill)?;
            pump(80);
            if editor.get_inspector_tab() != 1 || editor.get_inspector_collapsed() {
                return Err("Preform tab click did not open the inspector".into());
            }
            self.hold(pacing::STAGE_HOLD)?;
        }
        let text = girdle_diameter_mm.to_string();
        let spot = self
            .input_spot("Girdle Diameter (mm)")?
            .ok_or("Girdle Diameter input is missing")?;
        self.click_and_type(&spot, &text)?;
        // Typing marks the tab "not applied yet", and the button's accessible label says so; the
        // plain label is the fallback for a commit that already cleared the mark.
        let pending = self.inspector_form_control(
            "Apply Yield Inputs, changes not applied yet",
            uiprobe::AccessibleRole::Button,
        )?;
        let apply_spot = match pending {
            Some(spot) => Some(spot),
            None => {
                self.inspector_form_control("Apply Yield Inputs", uiprobe::AccessibleRole::Button)?
            }
        };
        let spot = apply_spot.ok_or(
            "Apply Yield Inputs button is missing (neither the pending nor the plain label)",
        )?;
        self.click_and_invoke(&spot)?;
        pump(100);
        Ok(())
    }

    /// Films the real mode toggles, including Solid plus either tracing engine.
    pub(crate) fn film_view_mode(&mut self, mode: i32) -> Result<(), String> {
        if !(0..=5).contains(&mode) {
            return Err(format!("unknown view mode {mode}"));
        }
        for _ in 0..3 {
            let shown = self.ui.global::<SolidPreviewModel>().get_view_mode();
            if shown == mode {
                return Ok(());
            }
            // Selected tracer toggles off in a combined view; Solid toggles
            // its overlay. Never look for the removed Both button.
            let button = match (mode, shown) {
                (0, 2) => 1,
                (0, 5) => 4,
                (0, _) | (1, 2) | (2, 1) | (4, 5) | (5, 4) => 0,
                (1 | 2, _) => 1,
                (3, _) => 3,
                _ => 4,
            };
            let pill = targets::solid_view_mode_pill(&self.view(), button)
                .ok_or("view-mode pill is missing")?;
            self.click_and_invoke(&pill)?;
            if self.ui.global::<SolidPreviewModel>().get_view_mode() == shown {
                return Err(format!(
                    "view-mode toggle {button} left mode {shown} unchanged"
                ));
            }
        }
        Err(format!("view-mode toggles did not select {mode}"))
    }

    /// Selects the actual tab and requires its state to change through that input.
    pub(crate) fn film_inspector_tab(&mut self, tab: i32) -> Result<(), String> {
        let pill =
            targets::inspector_tab_pill(&self.view(), tab).ok_or("inspector tab is missing")?;
        self.click_and_invoke(&pill)?;
        let shown = self.ui.global::<EditorModel>().get_inspector_tab();
        debug_check("inspector tab after the pill", &tab, &shown);
        if shown != tab {
            return Err(format!("inspector tab did not select {tab}"));
        }
        Ok(())
    }

    /// Films clicking the visible toolbar/row button for a command and executing it.
    pub(crate) fn film_command(&mut self, id: &str) -> Result<(), String> {
        debug_log::route_command(id);
        let palette_title = PALETTE_COMMANDS
            .iter()
            .find(|(command, _)| *command == id)
            .map(|&(_, title)| title);
        if let Some(title) = palette_title {
            let palette_model = self.ui.global::<CommandPaletteModel>();
            if !palette_model.get_open() {
                self.rec
                    .press_keys(self.ui.window(), "Ctrl+K")
                    .map_err(|e| format!("open command palette: {e:?}"))?;
            }
            if !pacing::wait_real(pacing::START_TIMEOUT, || palette_model.get_open()) {
                return Err("Ctrl+K did not open the command palette".into());
            }
            let search = targets::text_input(&self.view(), "Search Commands")
                .ok_or("command palette search input is missing")?;
            self.click_and_type(&search, title)?;
            if !pacing::wait_real(pacing::START_TIMEOUT, || {
                palette_model.get_query() == title
                    && palette_model
                        .get_rows()
                        .iter()
                        .any(|row| row.title == title && row.reason.is_empty())
            }) {
                return Err(format!(
                    "palette command {title:?} did not become available"
                ));
            }
            // Scope the result to the palette: a title can also occur in a
            // menu or elsewhere behind its modal backdrop.
            let palette = self
                .view()
                .root
                .query_descendants()
                .match_inherits("CommandPalette")
                .find_first()
                .ok_or("command palette is not visible")?;
            let command = palette
                .query_descendants()
                .match_inherits("PaletteRowView")
                .find_all()
                .into_iter()
                .filter(|row| row.accessible_label().is_some_and(|label| label == title))
                .map(|row| Spot::of(&row))
                .find(|spot| spot.usable(self.view().size))
                .ok_or_else(|| format!("palette command {title:?} is not visible"))?;
            self.click_and_invoke(&command)?;
            if palette_model.get_open() {
                return Err(format!(
                    "clicking palette command {title:?} did not activate it"
                ));
            }
            self.hold(Duration::from_millis(500))?;
            if id == "help.manual" {
                self.attach_help_window()?;
            }
            return Ok(());
        }
        if id == CMD_SOLVE {
            let ui = self.ui;
            let _ = pacing::wait_real(Duration::from_secs(10), || {
                !ui.global::<EditorModel>().get_solve_running()
            });
        }
        if id == CMD_DELETE {
            return self.click_selected_row_remove();
        }
        let mut spot = self.command_spot(id);
        // The command bar scrolls with the editor dock: a tall tab (Optimize grows the dock)
        // can leave it above the dock's top edge, so scroll the dock back up, as a user would.
        let in_command_bar = matches!(
            id,
            CMD_SOLVE | CMD_UNDO | CMD_ADOPT_ALL | "solve.snapshot" | CMD_COMPARE
        );
        if spot.is_none() && in_command_bar {
            self.scroll_dock_to_top()?;
            spot = self.command_spot(id);
        }
        let spot =
            spot.ok_or_else(|| format!("command {id:?}: required visible control missing"))?;
        self.click_and_invoke(&spot)?;
        pump(60);
        self.hold(Duration::from_millis(500))?;
        Ok(())
    }

    /// The visible control a command route clicks, `None` while it is off screen.
    fn command_spot(&self, id: &str) -> Option<Spot> {
        let view = self.view();
        let resolved = |t: &str| match targets::resolve(&view, t) {
            Resolved::Found(s) => Some(s),
            _ => None,
        };
        match id {
            CMD_SOLVE => resolved("solve_button"),
            CMD_UNDO => targets::run_label(&view, "Undo"),
            CMD_ADOPT_ALL => targets::run_label(&view, "Adopt All"),
            "solve.snapshot" => resolved("snapshot_button"),
            CMD_COMPARE => resolved("compare_button"),
            "view.live_render" => targets::live_render_pill(&view),
            "view.snap" => resolved("snap_pill"),
            "view.slice" => resolved("slice_pill"),
            _ => None,
        }
    }

    /// Wheels the editor dock back to its top, where the command bar sits.
    fn scroll_dock_to_top(&mut self) -> Result<(), String> {
        let view = self.view();
        let dock = targets::run(&view, &Query::Id("EditorView::dock_scroll"))
            .ok_or("editor dock scroll area is missing")?;
        let target = crate::perform_inputs::dock_wheel_target(&dock, view.size);
        drop(view);
        self.wheel_over(&target, 3000.0)?;
        pump(100);
        self.hold(Duration::from_millis(180))
    }

    /// Films clicking the trash button of the selected tier's row, the last column of the table.
    ///
    /// The button sits in the pinned ACTIONS group at the right edge of the table, so it is on
    /// view without scrolling ([`Self::reveal_table_button`] turns the table sideways only if it
    /// is not), the button is clicked, and any turns are undone. The row must be on screen and the
    /// button must sit on it (within one row height, and wholly inside the table's scrolling
    /// area); anything else fails, so a click can never land on another tier's button or beside
    /// the table. The goal of the step (the tier is gone) is checked by the director afterwards.
    fn click_selected_row_remove(&mut self) -> Result<(), String> {
        let selected = self.ui.global::<EditorModel>().get_selected_tier_index();
        if selected < 0 {
            return Err(format!("{REMOVE_BUTTON:?}: no tier row is selected"));
        }
        let (button, turns) = self
            .reveal_table_button(selected, |film, _area| {
                film.tier_row_control(
                    selected,
                    uiprobe::AccessibleRole::Button,
                    "Remove this tier",
                )
                .ok()
            })
            .map_err(|why| format!("{REMOVE_BUTTON:?} on the selected tier row: {why}"))?;
        self.click_and_invoke(&button)?;
        pump(60);
        self.hold(Duration::from_millis(500))?;
        self.unturn_table(turns)
    }
}
