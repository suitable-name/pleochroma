//! Visible widget routes for `Perform` recipes; the harness never silently commits a step.

use crate::{
    debug_log::{self, debug_check, debug_check_with},
    director::{Film, snap},
    pacing,
    perform_inputs::FIELD_PAUSE,
    routes::trace_running,
    support::app::pump,
    targets::{self, Query, Spot},
};
use pleochroma_cut::{EditorModel, GuideModel, TemplateGalleryModel};
use pleochroma_cut_core::{FreshDesignSpec, PreformShape, design::ConcaveTier};
use pleochroma_editor::{
    guide::{Perform, TierEntry},
    loading::{ConcaveTierFormFields, concave_tier_form_fields},
    material::gear_index_from_teeth,
};
use row_helpers::tier_row_values;
use slint::{ComponentHandle, Model};
use std::time::{Duration, Instant};

#[path = "perform_route_rows.rs"]
mod row_helpers;

impl Film<'_> {
    /// Runs `perform` through visible widgets, waits for step `n`, and advances.
    pub(crate) fn perform_step(
        &mut self,
        n: usize,
        perform: Option<&Perform>,
        label: &str,
    ) -> Result<(), String> {
        self.perform_n = n;
        if let Some(recipe) = perform {
            self.drive_perform(recipe)?;
        } else {
            return Err(format!(
                "{label}: no authored filmed action; GuideModel.go_on is forbidden"
            ));
        }
        self.wait_after_perform(n, label)
    }

    fn wait_after_perform(&mut self, n: usize, label: &str) -> Result<(), String> {
        let ui = self.ui;
        pump(40);
        let mut gave_up_at: Option<Instant> = None;
        let mut question = false;
        let met = pacing::wait_lapse(&mut self.rec, pacing::STEP_TIMEOUT, || {
            let s = snap(ui);
            if !s.at(n) || s.done {
                return true;
            }
            if ui.global::<EditorModel>().get_unsaved_dialog_open() {
                question = true;
                return true;
            }
            if s.failed && !s.busy {
                let since = *gave_up_at.get_or_insert_with(Instant::now);
                return since.elapsed() >= pacing::GIVE_UP_GRACE;
            }
            false
        })?;
        if question {
            return Err(format!("{label}: the unsaved-changes question appeared"));
        }
        let end = snap(ui);
        if !met || (end.at(n) && !end.done) {
            let err = ui.global::<EditorModel>().get_tier_form_error();
            let tiers = ui.global::<EditorModel>().get_tiers();
            let count = tiers.row_count();
            let first = tiers.row_data(0).map(|r| {
                format!(
                    "name={:?},angle={:?},idx={:?},kind={},ctext={:?}",
                    r.name.as_str(),
                    r.angle_full.as_str(),
                    r.indices_full.as_str(),
                    r.constraint_kind,
                    r.constraint_text.as_str()
                )
            });
            let why = if gave_up_at.is_some() {
                "the recipe ran, gave up and the goal was never met"
            } else {
                "the goal was not met in time"
            };
            return Err(format!(
                "{label}: {why} (tiers={count}, row0={first:?}, form_err={err:?}, waiting={:?})",
                ui.global::<GuideModel>().get_current().waiting
            ));
        }
        let hold = if trace_running(ui) {
            let _ = pacing::wait_real(pacing::RENDER_CAP, || !trace_running(ui));
            pump(300);
            pacing::RENDER_HOLD
        } else {
            pacing::RESULT_HOLD
        };
        if snap(ui).at(n) {
            self.rec.hold(hold)?;
        }
        self.advance_step(n)
    }

    /// Dispatches one `Perform` variant to its filmed widget route.
    fn drive_perform(&mut self, perform: &Perform) -> Result<(), String> {
        debug_log::route_perform(perform);
        match perform {
            Perform::NewDesign {
                template_index,
                spec,
            } => self.film_new_design(*template_index, spec),
            Perform::Tier(entry) => self.film_tier(entry),
            Perform::ConcaveTier { edit, tier } => self.film_concave_tier(edit.as_deref(), tier),
            Perform::SelectTier(name) => self.film_select_tier(name).map(|_| ()),
            Perform::Steps(series) => self.film_steps(series),
            Perform::MirrorTier { tier, suffix } => self.film_mirror_tier(tier, suffix),
            Perform::ClearRelation(tier) => self.film_clear_relation(tier),
            Perform::TierNote { tier, text } => self.film_tier_note(tier, text),
            Perform::CheaterOffset { tier, text } => self.film_cheater_offset(tier, text),
            Perform::Material(name) => self.film_material(name),
            Perform::Yield { girdle_diameter_mm } => self.film_yield(*girdle_diameter_mm),
            Perform::ViewMode(mode) => self.film_view_mode(*mode),
            Perform::InspectorTab(tab) => self.film_inspector_tab(*tab),
            Perform::Command(id) => self.film_command(id),
            Perform::Sequence(parts) => {
                for part in parts {
                    self.drive_perform(part)?;
                    pump(30);
                }
                Ok(())
            }
        }
    }

    /// Films opening the New Design dialog, filling the Empty or Template form, and clicking Create.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear route through the New Design dialog"
    )]
    fn film_new_design(
        &mut self,
        template_index: i32,
        spec: &FreshDesignSpec,
    ) -> Result<(), String> {
        if !self.ui.global::<EditorModel>().get_new_dialog_open() {
            let file = targets::file_button(&self.view()).ok_or("File menu is missing")?;
            self.click_and_invoke(&file)?;
            pump(100);
            self.rec.hold(pacing::MENU_HOLD)?;
            let item = targets::menu_item(&self.view(), "New Design...")
                .ok_or("New Design menu item is missing")?;
            self.click_and_invoke(&item)?;
            pump(150);
            if !self.ui.global::<EditorModel>().get_new_dialog_open() {
                return Err("New Design menu click did not open the dialog".into());
            }
            self.rec.hold(pacing::STAGE_HOLD)?;
        }

        if template_index == 0 {
            let empty_card = targets::run_label_start(&self.view(), "Empty")
                .or_else(|| targets::run_label_start(&self.view(), "Blank"))
                .ok_or("Empty design card is missing")?;
            self.click_and_invoke(&empty_card)?;
            let picked = self.ui.global::<EditorModel>().get_new_template_index();
            debug_check("template index after the Empty card", &0, &picked);
            if picked != 0 {
                return Err("Empty card did not select the empty template".into());
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;

            let shape_index = match spec.preform.shape {
                PreformShape::Block | PreformShape::Planes(_) => 0,
                PreformShape::Cylinder { .. } => 1,
            };
            let current_shape_index = self
                .ui
                .global::<EditorModel>()
                .get_new_preform_shape_index();
            if current_shape_index != shape_index {
                let shape_combo = targets::run_label(&self.view(), "Preform Shape")
                    .ok_or("Preform Shape combo is missing")?;
                self.click_and_invoke(&shape_combo)?;
                let key = if shape_index > current_shape_index {
                    "Down"
                } else {
                    "Up"
                };
                self.rec
                    .press_keys(self.ui.window(), key)
                    .map_err(|e| format!("Preform Shape {key}: {e:?}"))?;
                self.rec
                    .press_keys(self.ui.window(), "Return")
                    .map_err(|e| format!("Preform Shape Return: {e:?}"))?;
            }
            pump(60);
            self.rec.hold(FIELD_PAUSE)?;

            let ed = self.ui.global::<EditorModel>();
            for (cur, val, label) in [
                (
                    ed.get_new_preform_half_width().to_string(),
                    spec.preform.half_width.to_string(),
                    "Half-Width of the Rough",
                ),
                (
                    ed.get_new_preform_length_over_width().to_string(),
                    spec.preform.length_over_width.to_string(),
                    "Length over Width of the Rough",
                ),
                (
                    ed.get_new_preform_depth().to_string(),
                    spec.preform.depth.to_string(),
                    "Depth of the Rough",
                ),
            ] {
                if cur != val {
                    let spot = targets::text_input(&self.view(), label)
                        .ok_or_else(|| format!("New Design field {label:?} is missing"))?;
                    self.click_and_type(&spot, &val)?;
                }
            }
            let gear_index = gear_index_from_teeth(spec.gear_teeth);
            if ed.get_new_gear_index() != gear_index {
                let combo = targets::run_label(&self.view(), "Index Gear")
                    .ok_or("Index Gear combo is missing")?;
                self.click_and_invoke(&combo)?;
                let current = ed.get_new_gear_index();
                let key = if gear_index >= current { "Down" } else { "Up" };
                for _ in 0..(gear_index - current).abs() {
                    self.rec
                        .press_keys(self.ui.window(), key)
                        .map_err(|e| format!("Index Gear {key}: {e:?}"))?;
                }
                self.rec
                    .press_keys(self.ui.window(), "Return")
                    .map_err(|e| format!("Index Gear Return: {e:?}"))?;
            }
            if gear_index == 6 {
                let custom = spec.gear_teeth.to_string();
                let spot = targets::text_input(&self.view(), "Teeth on the Custom Gear")
                    .ok_or("Custom Gear teeth input is missing")?;
                self.click_and_type(&spot, &custom)?;
            }
            let sym = spec.symmetry_order.to_string();
            if self
                .ui
                .global::<EditorModel>()
                .get_new_symmetry_order_text()
                .as_str()
                != sym
            {
                let spot = targets::text_input(&self.view(), "Symmetry Order")
                    .ok_or("Symmetry Order input is missing")?;
                self.click_and_type(&spot, &sym)?;
            }
            if ed.get_new_mirror() != spec.mirror {
                let mirror = targets::run_label(&self.view(), "Mirror")
                    .ok_or("Mirror checkbox is missing")?;
                self.click_and_invoke(&mirror)?;
            }
            pump(60);
        } else {
            let gallery = self.ui.global::<TemplateGalleryModel>();
            let card = gallery
                .get_shapes()
                .iter()
                .chain(gallery.get_variants().iter())
                .chain(gallery.get_blank().iter())
                .find(|card| card.template_index == template_index)
                .ok_or_else(|| format!("template card {template_index} is not in the gallery"))?;
            let label = if card.shape.is_empty() {
                card.name.to_string()
            } else {
                format!("{}, {}", card.name, card.shape)
            };
            let spot = targets::run_label(&self.view(), &label)
                .ok_or_else(|| format!("template card {label:?} is not visible"))?;
            self.click_and_invoke(&spot)?;
            let picked = self.ui.global::<EditorModel>().get_new_template_index();
            debug_check("template index after the card", &template_index, &picked);
            if picked != template_index {
                return Err(format!(
                    "template card click did not select {template_index}"
                ));
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        let create_btn =
            targets::run_label(&self.view(), "Create").ok_or("Create button is missing")?;
        self.click_and_invoke(&create_btn)?;
        pump(100);
        if self.ui.global::<EditorModel>().get_new_dialog_open() {
            return Err("Create click left the New Design dialog open".into());
        }
        Ok(())
    }

    /// Films adding or editing a flat tier through the Tier inspector.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(crate) fn film_tier(&mut self, entry: &TierEntry) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_collapsed() {
            let header = targets::run_label(&self.view(), "INSPECTOR")
                .ok_or("Inspector header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        if editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(60);
        self.scroll_inspector(3000.0)?;

        let mut edit_pos: Option<i32> = None;
        if let Some(target_name) = &entry.edit {
            let pos = self.film_select_tier(target_name)?;
            edit_pos = Some(pos);
            if let Some(discard_btn) = targets::run_label_start(&self.view(), "Discard Draft") {
                self.click_and_invoke(&discard_btn)?;
                pump(40);
            }
        } else {
            let add = targets::role_label(
                &self.view(),
                "+ Add Tier",
                uiprobe::AccessibleRole::Button,
                Some("TierTableToolbar"),
            )
            .ok_or_else(|| "+ Add Tier is not visible".to_owned())?;
            self.click_and_invoke(&add)?;
            let fresh = editor.get_selected_tier_index();
            debug_check("selected tier index after + Add Tier", &-1, &fresh);
            if fresh != -1 {
                return Err("+ Add Tier did not open a fresh form".into());
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        if let Some(angle) = &entry.angle {
            let spot = self
                .input_spot("Angle in Degrees")?
                .ok_or_else(|| "required Angle in Degrees field is missing".to_owned())?;
            self.click_and_type(&spot, angle)?;
        }

        if let Some((kind, text)) = &entry.meets {
            self.film_tier_meets(*kind, text)?;
        }

        if let Some(name) = &entry.name {
            let spot = self
                .input_spot("Facet Name")?
                .ok_or("Facet Name input is missing")?;
            self.click_and_type(&spot, name)?;
        }

        if let Some(indices) = &entry.indices {
            let spot = self
                .input_spot("Index Positions")?
                .ok_or_else(|| "required Index Positions field is missing".to_owned())?;
            self.click_and_type(&spot, indices)?;
        }

        let tiers_before = editor.get_tiers().row_count();
        let row_before = edit_pos.and_then(|at| tier_row_values(self.ui, at));
        let label = if edit_pos.is_some() {
            "Save Tier"
        } else {
            "Add Tier"
        };
        let before_commit = editor.get_tiers().row_count();
        let wanted_edit_index = edit_pos.unwrap_or(-1);
        let edited = editor.get_selected_tier_index();
        debug_check(
            "edited tier index before commit",
            &wanted_edit_index,
            &edited,
        );
        if edited != wanted_edit_index {
            return Err(format!(
                "{label}: wrong form selection before commit: expected edited_index={wanted_edit_index}, actual={}, before_count={before_commit}",
                editor.get_selected_tier_index()
            ));
        }
        row_helpers::commit_tier_form(self, label, edit_pos.is_none(), tiers_before)?;
        let wanted_count = tiers_before + usize::from(edit_pos.is_none());
        let counted = editor.get_tiers().row_count();
        debug_check("tier count after the commit", &wanted_count, &counted);
        if counted != wanted_count {
            return Err(format!(
                "{label}: unexpected tier count: before={before_commit}, after={}, edit_index={:?}, form_error={:?}, guide_waiting={:?}",
                editor.get_tiers().row_count(),
                editor.get_selected_tier_index(),
                editor.get_tier_form_error(),
                self.ui.global::<GuideModel>().get_current().waiting
            ));
        }
        if let Some(pos) = edit_pos {
            let after = tier_row_values(self.ui, pos)
                .ok_or_else(|| format!("{label}: edited row {pos} disappeared"))?;
            if let Some(before) = row_before
                && ((entry.name.is_none() && before.0 != after.0)
                    || (entry.angle.is_none() && before.1 != after.1)
                    || (entry.indices.is_none() && before.2 != after.2)
                    || (entry.meets.is_none() && (before.3 != after.3 || before.4 != after.4)))
            {
                return Err(format!(
                    "{label}: an omitted field changed: before={before:?}, after={after:?}"
                ));
            }
        }
        if !editor.get_tier_form_error().is_empty() {
            return Err(format!("{label}: {}", editor.get_tier_form_error()));
        }
        Ok(())
    }

    /// Films the second half of "Add Anchor": the Tier form it opened gets `scale` typed into its
    /// Exact Scale Value field, and the tier is saved.
    pub(crate) fn save_exact_scale(&mut self, scale: &str) -> Result<(), String> {
        row_helpers::type_scale_and_save(self, scale)
    }

    /// Films choosing the Meets kind `kind` in the Tier form and typing its value `text`.
    fn film_tier_meets(&mut self, kind: i32, text: &str) -> Result<(), String> {
        let meets_combo = self
            .inspector_form_control("Meets", uiprobe::AccessibleRole::Combobox)?
            .ok_or_else(|| "Meets selector is not visible".to_owned())?;
        self.click_and_invoke(&meets_combo)?;
        self.rec.hold(pacing::MENU_HOLD)?;
        let label = match kind {
            0 => "Unspecified Vertex",
            1 => "Named Facet(s)",
            2 => "Exact Scale Value",
            3 => "Cut to Depth (mm)",
            4 => "Girdle Thickness (mm)",
            5 => "Table Width (mm)",
            _ => return Err(format!("unknown Meets option {kind}")),
        };
        // Keyboard navigation is the visible user input for the open combo.
        for _ in 0..6 {
            self.rec
                .press_keys(self.ui.window(), "Up")
                .map_err(|e| format!("Meets Up: {e:?}"))?;
        }
        for _ in 0..kind {
            self.rec
                .press_keys(self.ui.window(), "Down")
                .map_err(|e| format!("Meets Down: {e:?}"))?;
        }
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|e| format!("Meets Return: {e:?}"))?;
        let value = meets_combo
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_value)
            .ok_or_else(|| {
                format!("Meets {label:?}: combo has no accessible selection readback")
            })?;
        debug_check_with("Meets selection", &label, &value, value.contains(label));
        if !value.contains(label) {
            return Err(format!(
                "Meets selection expected {label:?}, read back {value:?}"
            ));
        }
        pump(80);
        self.rec.hold(FIELD_PAUSE)?;

        if kind != 0 && !text.is_empty() {
            let value_label = match kind {
                1 => "Facet Names This Facet Meets",
                2 => "Exact Scale Value",
                3..=5 => "Value in Millimetres",
                _ => return Err(format!("unsupported Meets value kind {kind}")),
            };
            let spot = self
                .input_spot(value_label)?
                .ok_or_else(|| format!("Meets kind {kind}: {value_label} field is missing"))?;
            self.click_and_type(&spot, text)?;
            let after = meets_combo
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_value)
                .ok_or_else(|| {
                    format!("Meets {label:?}: combo lost its accessible readback after typing")
                })?;
            if !after.contains(label) {
                return Err(format!(
                    "Meets selection changed while typing {value_label}: expected {label:?}, read back {after:?}"
                ));
            }
        }
        Ok(())
    }

    /// Reveals a tier row's name and angle with filmed scrolling, without selecting the row.
    pub(crate) fn film_reveal_tier(&mut self, name: &str) -> Result<(), String> {
        row_helpers::reveal_tier_row(self, name).map(|_| ())
    }

    /// Films selecting a tier row by its fully visible name text center.
    pub(crate) fn film_select_tier(&mut self, name: &str) -> Result<i32, String> {
        row_helpers::click_tier_row(self, name)
    }

    /// Opens the Tool Shape combo of the concave tier form and picks the tool coded `tool`
    /// (CYL, CON, CIR, DSC or SPH) with the keys.
    fn choose_concave_tool(&mut self, tool: &str) -> Result<(), String> {
        const TOOLS: [&str; 5] = [
            "Cylinder (CYL)",
            "Cone (CON)",
            "Circle (CIR)",
            "Disc (DSC)",
            "Sphere (SPH)",
        ];
        let tool_index = match tool {
            "CYL" => 0,
            "CON" => 1,
            "CIR" => 2,
            "DSC" => 3,
            "SPH" => 4,
            _ => return Err(format!("unknown concave tool {tool:?}")),
        };
        // The combo sits under `if concave_loaded`, so it exists only once the concave form has
        // loaded after the click that chose it; wait for it to be there and enabled.
        let ready = pacing::wait_real(Duration::from_secs(3), || {
            self.inspector_label_state("Tool Shape") == Some(true)
        });
        let ed = self.ui.global::<EditorModel>();
        let missing = |film: &Self, why: &str| {
            format!(
                "concave Tool Shape combo is missing ({why}): label state {:?} (None = absent, \
                 Some(false) = disabled), inspector tab {}, concave mode {}, selected row {}, \
                 locked hint {:?}, inspector: [{}]",
                film.inspector_label_state("Tool Shape"),
                ed.get_inspector_tab(),
                ed.get_inspector_concave_mode(),
                ed.get_selected_tier_index(),
                film.ui.global::<GuideModel>().get_locked_hint(),
                film.inspector_dump()
            )
        };
        let Some(shape) =
            self.inspector_form_control("Tool Shape", uiprobe::AccessibleRole::Combobox)?
        else {
            let why = if ready {
                "present but not revealed by scrolling"
            } else {
                "never loaded in 3 s"
            };
            return Err(missing(self, why));
        };
        let read = |shape: &Spot| {
            shape
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_value)
                .map(|value| value.to_string())
        };
        let index_of = |value: &str| TOOLS.iter().position(|name| *name == value);
        self.click_and_invoke(&shape)?;
        let mut trace = vec![format!("opened: {:?}", read(&shape))];
        for pass in 0..3 {
            let before = read(&shape);
            let current = before.as_deref().and_then(index_of);
            let (key, count) = match current {
                Some(at) if at == tool_index => break,
                Some(at) if at < tool_index => ("Down", tool_index - at),
                Some(at) => ("Up", at - tool_index),
                None => ("Up", 4),
            };
            for _ in 0..count {
                self.rec
                    .press_keys(self.ui.window(), key)
                    .map_err(|e| format!("Tool Shape {key}: {e:?}"))?;
                trace.push(format!("{key}: {:?}", read(&shape)));
            }
            if current.is_none() {
                for _ in 0..tool_index {
                    self.rec
                        .press_keys(self.ui.window(), "Down")
                        .map_err(|e| format!("Tool Shape Down: {e:?}"))?;
                    trace.push(format!("Down: {:?}", read(&shape)));
                }
                break;
            }
            // A combo that only takes the highlighted entry on Return reads the same after the
            // keys; correcting again would overshoot.
            if pass == 0 && read(&shape) == before {
                break;
            }
        }
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|e| format!("Tool Shape Return: {e:?}"))?;
        pump(40);
        let selected =
            read(&shape).ok_or("Tool Shape combo has no accessible selection readback")?;
        let expected = TOOLS[tool_index];
        debug_check("Tool Shape selection", &expected, &selected);
        if selected != expected {
            return Err(format!(
                "Tool Shape expected {expected:?}, read back {selected:?}; trace {trace:?}"
            ));
        }
        Ok(())
    }

    /// Films adding or editing a concave tier in `ConcaveFormTab`.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    fn film_concave_tier(&mut self, edit: Option<&str>, tier: &ConcaveTier) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_collapsed() {
            let header = targets::run_label(&self.view(), "INSPECTOR")
                .ok_or("Inspector header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        if editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(60);

        if let Some(name) = edit {
            self.film_select_tier(name)?;
        } else {
            let btn = targets::role_label(
                &self.view(),
                "+ Add Concave Tier",
                uiprobe::AccessibleRole::Button,
                Some("TierTableToolbar"),
            )
            .ok_or("+ Add Concave Tier button is missing")?;
            self.click_and_invoke(&btn)?;
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        let ConcaveTierFormFields {
            name,
            angle_deg,
            indices,
            instructions,
            tool,
            tool_azimuth_deg,
            x,
            y,
            z,
            diameter_ratio,
            tool_angle_deg,
            reciprocating,
        } = concave_tier_form_fields(tier);

        self.choose_concave_tool(&tool)?;

        for (label, val) in [
            ("Facet Name", name.as_str()),
            ("Angle in Degrees", angle_deg.as_str()),
            ("Index Positions", indices.as_str()),
            ("Cutting Instructions", instructions.as_str()),
            ("Tool Azimuth in Degrees", tool_azimuth_deg.as_str()),
            ("Tool Displacement X", x.as_str()),
            ("Tool Displacement Y", y.as_str()),
            ("Tool Displacement Z", z.as_str()),
            ("Tool Diameter over Stone Width", diameter_ratio.as_str()),
            ("Tool Included Angle in Degrees", tool_angle_deg.as_str()),
        ] {
            if !val.is_empty() {
                let spot = self
                    .input_spot(label)?
                    .ok_or_else(|| format!("concave field {label:?} is missing"))?;
                self.click_and_type(&spot, val)?;
            }
        }

        let motion = self
            .inspector_form_control(
                "Reciprocating (Stroked Back and Forth)",
                uiprobe::AccessibleRole::Checkbox,
            )?
            .ok_or("Reciprocating checkbox is missing")?;
        let checked = motion
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_checked)
            .ok_or("Reciprocating checkbox has no checked-state readback")?;
        if checked != reciprocating {
            self.click_and_invoke(&motion)?;
        }
        let motion_now = motion
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_checked);
        debug_check("Reciprocating checked", &Some(reciprocating), &motion_now);
        if motion_now != Some(reciprocating) {
            return Err("Reciprocating click did not set the requested motion".into());
        }

        let commit = targets::run(
            &self.view(),
            &Query::Id("EditorInspector::tip-button-concave-loaded"),
        )
        .or_else(|| {
            targets::role_label(
                &self.view(),
                "Add Concave Tier",
                uiprobe::AccessibleRole::Button,
                None,
            )
        })
        .or_else(|| {
            targets::role_label(
                &self.view(),
                "Save Concave Tier",
                uiprobe::AccessibleRole::Button,
                None,
            )
        })
        .ok_or("concave form commit button is missing")?;

        self.click_pinned_commit(&commit)?;
        pump(80);
        // The picture after the step shows the true end of the form (its last line and the spacer
        // above the pinned button), not wherever the last field lookup left it. Cosmetic, so a
        // form that cannot scroll any more is not an error.
        if let Err(reason) = self.scroll_inspector(-3000.0) {
            debug_log::route(&format!(
                "no scroll to the end of the concave form: {reason}"
            ));
        }
        Ok(())
    }
}
