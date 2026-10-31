//! The material catalogue and the edits that choose a material: the rows the picker
//! lists, picking one, the refractive-index override, the body color and the custom
//! materials made in the compact form.
//!
//! A material pick is one undoable `Edit::SetMaterial` on the design, and it also records
//! the pick as the render material name in the settings (the render follows the design
//! while the link is on, see the parent module). Everything here reads and writes through
//! the shared editor helpers, so the rules (what an RI pick keeps, when a typed color
//! survives a species change) are the desktop's.
//!
//! # The body color picker
//!
//! The picker lists the shared keyed color entries of the editor crate
//! (`pleochroma_editor::material::color_entries_for_design`), read-only: the material's own color,
//! its built-in color variants, a variant the design file carries (saved on another device), the
//! nine generic presets on a single-color (isotropic) host only, and, only while the design has
//! one, its Custom Color. The phone has no color editor, so a Custom Color stays as saved and is
//! never offered as something to pick. A pick goes through `selection_with_entry`, so picking a
//! variant copies its whole per-axis tensor into the design (`color_variant_override`) and keeps
//! the material's pleochroism.

use std::sync::OnceLock;

use pleochroma::optics::materials::{
    GemMaterial,
    body_color::preset_label_for_rgb,
    variants::{
        ColorVariant, MATERIAL_DEFAULT_LABEL, TensorRank, VariantProvenance, built_in_variants,
        host_rank,
    },
};
use pleochroma_cut_core::{
    Edit, MaterialLookup, MaterialSelection, built_in_specific_gravity, critical_angle_deg,
    material::{CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice},
};
use pleochroma_editor::{
    loading::{eval_number, ri_override_for_material_pick},
    material::{
        ColorEntry, ColorEntryItem, body_color_options, color_entries_for_design,
        entry_for_selection, listed_variants, selection_with_entry,
    },
    material_lookup::{EditorMaterialLookup, traced_material_for},
};
use pleochroma_web_core::custom_material::{
    BuiltCustomMaterial, CustomMaterialForm, build_custom_material,
};

use super::trimmed;
use crate::app_state::AppState;

/// How many body colors the custom-material form offers: `Material Default` (clear) and the
/// nine generic presets. The form builds one isotropic color, so it keeps the fixed index
/// contract of `body_color_names`; the design's own picker is keyed ([`color_rows`]).
const PICKABLE_COLORS: usize = 10;

/// What the picker's Custom Color row says: the phone cannot edit it.
const CUSTOM_COLOR_DETAIL: &str =
    "The color saved in this design. It stays as it is; edit it on the desktop.";

/// One line of the material picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialRow {
    /// The material's name, exactly as the design stores it.
    pub name: String,
    /// The refractive index at the sodium D line, three decimals.
    pub ri_text: String,
    /// The dispersion (`n_F - n_C`), three decimals.
    pub dispersion_text: String,
    /// The specific gravity, two decimals; empty when none is on file.
    pub sg_text: String,
    /// Whether this is a custom material of the session.
    pub is_custom: bool,
    /// Whether this is the design's material.
    pub is_selected: bool,
}

impl MaterialRow {
    /// The supporting line of the row: `RI 2.417 · Dispersion 0.025 · SG 3.52` (no SG part
    /// when none is on file).
    #[must_use]
    pub fn detail_text(&self) -> String {
        let mut text = format!(
            "RI {} \u{b7} Dispersion {}",
            self.ri_text, self.dispersion_text
        );
        if !self.sg_text.is_empty() {
            text.push_str(" \u{b7} SG ");
            text.push_str(&self.sg_text);
        }
        text
    }
}

/// What the Stone tab's material row shows for the design's current material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialSummary {
    /// The material's name, or `Not Set`.
    pub title: String,
    /// Its optics in one line, or the sentence saying what to do when none is set.
    pub detail: String,
}

/// The catalogue facts of one built-in material.
struct BuiltIn {
    name: String,
    ri: f32,
    dispersion: f32,
    specific_gravity: Option<f64>,
}

/// Every built-in material, alphabetical (ignoring case). Built once: the table builders
/// allocate, and the picker reads it on every keystroke of the search field.
fn built_ins() -> &'static [BuiltIn] {
    static TABLE: OnceLock<Vec<BuiltIn>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<BuiltIn> = GemMaterial::all_materials()
            .iter()
            .map(|gem| BuiltIn {
                name: gem.name.clone(),
                ri: gem.dispersion.n_d(),
                dispersion: gem.dispersion.delta_f_c(),
                specific_gravity: built_in_specific_gravity(&gem.name)
                    .map(|specific_gravity| specific_gravity.representative),
            })
            .collect();
        table.sort_by_key(|material| material.name.to_ascii_lowercase());
        table
    })
}

/// The picker rows whose name contains `query` (ignoring case; a blank query lists
/// everything): the built-in materials alphabetically, then the session's custom ones.
#[must_use]
pub fn material_rows(state: &AppState, query: &str) -> Vec<MaterialRow> {
    let needle = query.trim().to_lowercase();
    let selected = state.session.design.material.name.as_deref();
    let is_selected = |name: &str| selected.is_some_and(|own| own.eq_ignore_ascii_case(name));
    let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);

    let built_in = built_ins()
        .iter()
        .filter(|material| matches(&material.name))
        .map(|material| MaterialRow {
            name: material.name.clone(),
            ri_text: format!("{:.3}", material.ri),
            dispersion_text: format!("{:.3}", material.dispersion),
            sg_text: material
                .specific_gravity
                .map_or_else(String::new, |value| format!("{value:.2}")),
            is_custom: false,
            is_selected: is_selected(&material.name),
        });
    let mut customs: Vec<&GemMaterial> = state
        .props
        .custom_materials
        .iter()
        .filter(|gem| matches(&gem.name))
        .collect();
    customs.sort_by_key(|gem| gem.name.to_ascii_lowercase());
    let custom = customs.into_iter().map(|gem| MaterialRow {
        name: gem.name.clone(),
        ri_text: format!("{:.3}", gem.dispersion.n_d()),
        dispersion_text: format!("{:.3}", gem.dispersion.delta_f_c()),
        sg_text: state
            .props
            .snapshot_for(&gem.name)
            .and_then(|snapshot| snapshot.specific_gravity)
            .map_or_else(String::new, |value| format!("{value:.2}")),
        is_custom: true,
        is_selected: is_selected(&gem.name),
    });
    built_in.chain(custom).collect()
}

/// The name of the built-in or custom material called `wanted` (ignoring case), spelled
/// the way the catalogue spells it.
fn known_material_name(state: &AppState, wanted: &str) -> Option<String> {
    built_ins()
        .iter()
        .map(|material| material.name.as_str())
        .chain(
            state
                .props
                .custom_materials
                .iter()
                .map(|gem| gem.name.as_str()),
        )
        .find(|name| name.eq_ignore_ascii_case(wanted))
        .map(str::to_owned)
}

/// Applies `next` as the design's material: one undoable edit, none when it equals the
/// current selection.
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub(super) fn apply_selection(
    state: &mut AppState,
    next: MaterialSelection,
) -> Result<bool, String> {
    if next == state.session.design.material {
        return Ok(false);
    }
    state
        .session
        .apply(Edit::SetMaterial {
            material: Box::new(next),
        })
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Makes `name` the design's material (one undoable edit) and the render material name of
/// the settings.
///
/// A plain pick keeps the design's specific-gravity override, keeps a typed color only
/// while the species stays the same, and clears the RI override -- except that a design
/// that had no material yet keeps its exported refractive index when the pick's own
/// differs (`ri_override_for_material_pick`), so a pick never silently rewrites the
/// schedule's `I` line.
///
/// # Errors
///
/// A sentence when `name` is not a built-in or session material, or the editor refuses.
pub fn select_material(state: &mut AppState, name: &str) -> Result<bool, String> {
    let wanted = name.trim();
    let canonical = known_material_name(state, wanted)
        .ok_or_else(|| format!("'{wanted}' is not a known material."))?;
    let current = state.session.design.material.clone();
    let legacy_ri = state.session.design.meta.refractive_index;
    let same = current
        .name
        .as_deref()
        .is_some_and(|own| own.eq_ignore_ascii_case(&canonical));
    let next = MaterialSelection {
        name: Some(canonical.clone()),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override: if same {
            current.refractive_index_override
        } else {
            ri_override_for_material_pick(Some(&canonical), current.name.as_deref(), legacy_ri)
        },
        body_color_override: current.body_color_override.filter(|_| same),
        body_color_bands_override: current.body_color_bands_override.filter(|_| same),
        absorption_path_scale_override: current.absorption_path_scale_override.filter(|_| same),
        color_pattern_override: current.color_pattern_override.filter(|_| same),
        color_variant_override: current.color_variant_override.filter(|_| same),
    };
    let changed = apply_selection(state, next)?;
    state.update_settings(|settings| settings.material_name = canonical);
    Ok(changed)
}

/// Sets the design's refractive-index override from `text`; blank clears it (the
/// material's own index applies). Arithmetic is allowed (`1.5+0.25`).
///
/// # Errors
///
/// A sentence when the text is not a finite number above 1.0, or the editor refuses.
pub fn set_refractive_index_override(state: &mut AppState, text: &str) -> Result<bool, String> {
    let value = if text.trim().is_empty() {
        None
    } else {
        let value =
            eval_number(text, None).map_err(|error| error.message("Refractive index", text))?;
        if !value.is_finite() || value <= 1.0 {
            return Err(
                "Refractive index override must be a finite number greater than 1.0.".to_owned(),
            );
        }
        Some(value)
    };
    let mut next = state.session.design.material.clone();
    next.refractive_index_override = value;
    apply_selection(state, next)
}

/// The names of the generic colors the custom-material form offers, `Material Default`
/// (clear) first; the index of a name is the form's [`CustomMaterialInput::color_index`].
///
/// This is not the design's color picker: that one is keyed and per material, see
/// [`color_rows`].
#[must_use]
pub fn body_color_names() -> Vec<String> {
    body_color_options()
        .into_iter()
        .take(PICKABLE_COLORS)
        .collect()
}

/// The design material as the tracer resolves it: a custom material first, then a
/// built-in; a material-less design is resolved by its refractive index when a built-in is
/// close enough. `None` when nothing resolves.
pub(super) fn resolved_gem(state: &AppState) -> Option<GemMaterial> {
    let design = &state.session.design;
    let name = design
        .material
        .name
        .clone()
        .unwrap_or_else(|| traced_material_for(design, &state.props.custom_materials).0);
    if name.is_empty() {
        return None;
    }
    EditorMaterialLookup::new(&state.props.custom_materials).lookup(&name)
}

/// The host the picker and the Stone tab's color label are worked out against.
///
/// A named design's own material (`None` when it resolves to nothing). A design with no material
/// name gets an isotropic stand-in, so it is offered the plain list (Material Default, the
/// presets, Custom Color) and never a variant: a refractive-index guess is not a host, and a
/// variant stored on a guessed one would be applied to another rank after the next index change.
fn color_host(state: &AppState) -> Option<GemMaterial> {
    let named = state
        .session
        .design
        .material
        .name
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty());
    if named {
        resolved_gem(state)
    } else {
        Some(GemMaterial::new_custom("", 1.5, 0.0, 0.0, [0.0; 3]))
    }
}

/// Whether a body color can be set: not for a material whose color comes from its zones,
/// unless the stone carries its own color pattern (the pattern is what a pick replaces).
#[must_use]
pub fn body_color_available(state: &AppState) -> bool {
    state
        .session
        .design
        .material
        .color_pattern_override
        .is_some()
        || resolved_gem(state).is_none_or(|gem| gem.zoning.is_none())
}

/// The label of the design's body color: `Material Default`, a variant's or a preset's name,
/// or `Custom Color`.
///
/// A single color on a host with per-direction colors reads as the picker row that shows it,
/// `Legacy: <color> (No Pleochroism)`, and a variant the picker does not list for the host (its
/// rank no longer accepts it) reads as the `Custom Color` row that shows it.
///
/// Cheap (no spectra are worked out), because the Stone tab asks for it on every refresh.
#[must_use]
pub(super) fn body_color_label(state: &AppState) -> String {
    let selection = &state.session.design.material;
    if let Some(choice) = &selection.color_variant_override {
        return if variant_is_listed(state, choice) {
            choice.label.clone()
        } else {
            CUSTOM_VARIANT_LABEL.to_owned()
        };
    }
    if selection
        .body_color_bands_override
        .as_ref()
        .is_some_and(|bands| !bands.is_empty())
    {
        return CUSTOM_VARIANT_LABEL.to_owned();
    }
    let Some(rgb) = selection.body_color_override else {
        return MATERIAL_DEFAULT_LABEL.to_owned();
    };
    let Some(label) = preset_label_for_rgb(rgb) else {
        return CUSTOM_VARIANT_LABEL.to_owned();
    };
    let single_color_host = color_host(state)
        .as_ref()
        .is_none_or(|gem| host_rank(gem) == TensorRank::Isotropic);
    if single_color_host {
        label.to_owned()
    } else {
        format!("Legacy: {label} (No Pleochroism)")
    }
}

/// Whether the picker lists the design's variant `choice` under its own name: the Custom Color
/// and a variant the host does not accept are shown by the `Custom Color` row instead.
///
/// Checks only what the picker's list checks (`listed_variants`: the key, the material and
/// `validate_for`), no spectra.
fn variant_is_listed(state: &AppState, choice: &VariantChoice) -> bool {
    if choice.key == CUSTOM_VARIANT_KEY {
        return true;
    }
    let Some(host) = color_host(state) else {
        return true;
    };
    let carried = carried_variants(&state.session.design.material, &host);
    listed_variants(&host, &carried)
        .iter()
        .any(|variant| variant.key == choice.key)
}

/// One row of the body color picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorRow {
    /// The color's name (Title Case): `Material Default`, `Padparadscha`, `Red`, `Custom Color`.
    pub label: String,
    /// One line about the color: what each direction looks like for a variant with
    /// per-direction colors, whether it is estimated, and so on.
    pub detail: String,
    /// Whether this row shows the design's current color.
    pub is_selected: bool,
}

/// What the picker offers for the design as it stands.
struct ColorList {
    /// The variants a pick may copy in (the host's built-in ones and the design's own).
    variants: Vec<ColorVariant>,
    /// The rows, in picker order.
    items: Vec<ColorEntryItem>,
    /// The position of the row that shows the design's color.
    current: usize,
}

/// The design's own color variant as a listable one, when it is neither a Custom Color nor one
/// of the host's built-in variants (a variant saved on another device): the design file carries
/// the whole tensor, so the phone can list it under its own name. Empty otherwise.
fn carried_variants(selection: &MaterialSelection, host: &GemMaterial) -> Vec<ColorVariant> {
    let Some(choice) = &selection.color_variant_override else {
        return Vec::new();
    };
    if choice.key == CUSTOM_VARIANT_KEY
        || built_in_variants(&host.name)
            .iter()
            .any(|variant| variant.key == choice.key)
    {
        return Vec::new();
    }
    vec![ColorVariant {
        key: choice.key.clone(),
        label: choice.label.clone(),
        material: host.name.clone(),
        absorption: choice.absorption.clone(),
        unit: choice.unit,
        reference_path_mm: choice.reference_path_mm,
        provenance: VariantProvenance::User,
    }]
}

/// The picker's rows for the design, `None` when its material resolves to nothing.
///
/// The editor's keyed list with the phone's rules applied: Custom is listed only while the
/// design's color is the Custom one (there is no editor to make another), under its
/// `Custom Color` name; a material whose color is its zones keeps `Material Default` as the one
/// choice, which restores the zones.
fn color_list(state: &AppState) -> Option<ColorList> {
    let host = color_host(state)?;
    let selection = &state.session.design.material;
    let carried = carried_variants(selection, &host);
    let mut items = color_entries_for_design(&host, &carried, selection);
    let shown = items
        .get(entry_for_selection(selection, &items))?
        .entry
        .clone();
    let zoned = host.zoning.is_some();
    items.retain(|item| match &item.entry {
        ColorEntry::MaterialDefault => true,
        ColorEntry::Custom => shown == ColorEntry::Custom,
        ColorEntry::Variant { .. } | ColorEntry::Preset(_) => !zoned,
    });
    if let Some(row) = items
        .iter_mut()
        .find(|item| item.entry == ColorEntry::Custom)
    {
        CUSTOM_VARIANT_LABEL.clone_into(&mut row.label);
        CUSTOM_COLOR_DETAIL.clone_into(&mut row.tooltip);
    }
    let current = items
        .iter()
        .position(|item| item.entry == shown)
        .unwrap_or(0);
    Some(ColorList {
        variants: listed_variants(&host, &carried),
        items,
        current,
    })
}

/// The body color picker's rows for the design.
///
/// `Material Default`, the material's built-in variants, a variant the design file carries, the
/// generic presets (single-color materials only) and, while the design has one, its
/// `Custom Color`. Empty when the material resolves to nothing. Compute it when the picker opens
/// or the design changes, not per frame: every row's detail is worked out from its spectrum.
#[must_use]
pub fn color_rows(state: &AppState) -> Vec<ColorRow> {
    let Some(list) = color_list(state) else {
        return Vec::new();
    };
    let current = list.current;
    list.items
        .into_iter()
        .enumerate()
        .map(|(position, item)| ColorRow {
            label: item.label,
            detail: item.tooltip,
            is_selected: position == current,
        })
        .collect()
}

/// Sets the design's body color to row `index` of [`color_rows`].
///
/// The pick goes through the editor's `selection_with_entry`: `Material Default` and a preset
/// replace the stone's color pattern as the desktop does (a plain color is an intentional
/// replacement; `Material Default` restores the material's own color, its zones for a zoned
/// material), a variant is copied into the design as a snapshot and keeps the material's
/// pleochroism, and the Custom Color row keeps the color the design has.
///
/// # Errors
///
/// A sentence when `index` names no row, the material's color comes from its zones (and the
/// stone has no pattern of its own to replace), or the editor refuses.
pub fn set_color_entry(state: &mut AppState, index: i32) -> Result<bool, String> {
    if !body_color_available(state) {
        return Err(
            "This material's color comes from its zones, so it cannot be recolored.".to_owned(),
        );
    }
    let unavailable = || "That color is not available.".to_owned();
    let list = color_list(state).ok_or_else(unavailable)?;
    let item = usize::try_from(index)
        .ok()
        .and_then(|position| list.items.get(position))
        .ok_or_else(unavailable)?;
    let current = &state.session.design.material;
    let next = selection_with_entry(current.clone(), &item.entry, current, &list.variants);
    apply_selection(state, next)
}

/// The Stone tab's material row for the design as it stands.
#[must_use]
pub fn material_summary(state: &AppState) -> MaterialSummary {
    let design = &state.session.design;
    let Some(name) = design.material.name.clone() else {
        let (guess, _) = traced_material_for(design, &state.props.custom_materials);
        let detail = if guess.is_empty() {
            "Pick a material to score and render this design.".to_owned()
        } else {
            format!(
                "Looks like {guess} (refractive index {}).",
                trimmed(design.effective_refractive_index(), 4)
            )
        };
        return MaterialSummary {
            title: "Not Set".to_owned(),
            detail,
        };
    };
    let custom_sg = state.props.custom_specific_gravity();
    let lookup =
        EditorMaterialLookup::new(&state.props.custom_materials).with_specific_gravity(&custom_sg);
    let specific_gravity = design.material.effective_specific_gravity_with(&lookup);
    let detail = lookup.lookup(&name).map_or_else(
        || "This material is not in the catalogue.".to_owned(),
        |gem| {
            let optics = format!(
                "RI {:.3} \u{b7} Dispersion {:.3}",
                gem.dispersion.n_d(),
                gem.dispersion.delta_f_c()
            );
            specific_gravity.map_or_else(
                || optics.clone(),
                |value| format!("{optics} \u{b7} SG {value:.2}"),
            )
        },
    );
    MaterialSummary {
        title: name,
        detail,
    }
}

/// The effective refractive index and critical angle line of the Stone tab, empty when the
/// design has no usable index.
#[must_use]
pub(super) fn effective_ri_text(state: &AppState) -> String {
    let index = state
        .session
        .design
        .effective_refractive_index_with(&state.props.custom_materials);
    if index.is_finite() && index > 1.0 {
        format!(
            "Effective RI {index:.4} \u{b7} Critical angle {:.2}\u{b0}",
            critical_angle_deg(index)
        )
    } else {
        String::new()
    }
}

/// The compact custom-material form as typed (all fields text, as on the web).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomMaterialInput {
    /// The material's name.
    pub name: String,
    /// Refractive index (sodium D).
    pub ri: String,
    /// Dispersion, `n_F - n_C`; blank reads as 0.
    pub dispersion: String,
    /// Birefringence (signed); blank reads as 0.
    pub birefringence: String,
    /// Specific gravity for the carat estimate; blank means not recorded.
    pub specific_gravity: String,
    /// The color: an index into [`body_color_names`] (`0` is clear).
    pub color_index: i32,
}

/// Builds the material (and the snapshot a design file stores for it) the form describes.
///
/// # Errors
///
/// A sentence for a blank name, a name of a built-in material, or a number that is missing,
/// not a number or outside its range (the refractive index must lie between 1.30 and 3.20).
pub fn custom_material_from_form(
    form: &CustomMaterialInput,
) -> Result<BuiltCustomMaterial, String> {
    build_custom_material(&CustomMaterialForm {
        name: &form.name,
        ri: &form.ri,
        dispersion: &form.dispersion,
        birefringence: &form.birefringence,
        specific_gravity: &form.specific_gravity,
        color_index: form.color_index,
    })
}

/// Builds the custom material from `form`, adds it to the session's catalogue and makes
/// it the design's material (one undoable edit, no RI override so the new index shows).
///
/// # Errors
///
/// A sentence when the form is refused (see [`custom_material_from_form`]) or the editor
/// refuses the edit; the catalogue is left as it was then.
pub fn add_custom_material(
    state: &mut AppState,
    form: &CustomMaterialInput,
) -> Result<bool, String> {
    let built = custom_material_from_form(form)?;
    let name = built.material.name.clone();
    let previous: Option<(GemMaterial, Option<_>)> = state
        .props
        .find_custom(&name)
        .cloned()
        .map(|material| (material, state.props.snapshot_for(&name).cloned()));
    state
        .props
        .register_custom(built.material, Some(built.snapshot))?;

    let mut selection = state.session.design.material.clone();
    selection.name = Some(name.clone());
    selection.refractive_index_override = None;
    match apply_selection(state, selection) {
        Ok(changed) => {
            state.update_settings(|settings| settings.material_name = name);
            Ok(changed)
        }
        Err(message) => {
            // Put the catalogue back the way it was.
            state
                .props
                .custom_materials
                .retain(|gem| !gem.name.eq_ignore_ascii_case(&name));
            state
                .props
                .custom_snapshots
                .retain(|(known, _)| !known.eq_ignore_ascii_case(&name));
            if let Some((material, snapshot)) = previous {
                // The name was a custom material's before; registering it again cannot be
                // refused (it passed once).
                let _ = state.props.register_custom(material, snapshot);
            }
            Err(message)
        }
    }
}
