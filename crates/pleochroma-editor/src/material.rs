//! Material/RI and gear-tooth view-model helpers.
//!
//! The built-in and design material combo lists, the RI-source explanation text, and the
//! gear preset/remap-preview helpers the design settings panel's gear control uses.

use crate::loading::number_expr::eval_number;
use pleochroma::optics::materials::{
    GemMaterial,
    body_color::{BODY_COLOR_PRESETS, preset_index_for_rgb},
};
use pleochroma_cut_core::{Design, Edit, MaterialSelection, RemapRounding};

// The keyed color-entry list the desktop combo uses. The fixed-index functions below serve
// the web and mobile combos.
mod color_entry;
pub use color_entry::{
    ColorEntry, ColorEntryItem, color_entries, color_entries_for_design, entry_for_selection,
    hex_from_rgb, legacy_color_on_pleochroic_host, listed_variants, rgb_from_hex,
    selection_with_entry,
};

/// The gear combo's fixed pill choices, before the combo's own trailing "Custom"
/// entry -- shared by the design settings panel's gear control and the new-design
/// dialog so both present the same list.
pub const GEAR_PRESETS: [i32; 6] = [96, 80, 77, 72, 64, 120];

/// One row of the gear-remap confirmation panel -- see [`gear_remap_preview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GearRemapPreviewRow {
    /// The tier's name.
    pub name: String,
    /// The tier's current index list, `", "`-joined.
    pub old_indices: String,
    /// The index list confirming the remap would write, `", "`-joined.
    pub new_indices: String,
    /// Whether some position's ideal new value is not a whole tooth.
    pub non_integral: bool,
}

impl MaterialComboCache {
    /// [`design_material_options`], rebuilt only when `custom`'s own name list has
    /// changed since the last call -- see this type's own doc comment. A design
    /// settings refresh calls this every time; only a save/delete/rename in the
    /// material editor (which changes `custom`'s names) actually pays for a rebuild.
    pub fn options(&mut self, custom: &[GemMaterial]) -> Vec<String> {
        let signature: Vec<String> = custom.iter().map(|m| m.name.clone()).collect();
        if self.signature.as_ref() != Some(&signature) {
            self.options = design_material_options(custom);
            self.signature = Some(signature);
        }
        self.options.clone()
    }
}

/// Caches [`design_material_options`]'s last result, keyed by the custom-material name list it
/// was built from.
///
/// The combo is refreshed on every editor update, but its contents change only when a custom
/// material is saved, deleted or renamed, so the list is rebuilt only then. The key is names
/// only: `design_material_options` lists names alone, so RI and absorption edits do not
/// change it.
///
/// `signature` is `None` until the first build. An empty default would equal the signature of
/// an empty custom list, so the first call would return no options at all.
#[derive(Default)]
pub struct MaterialComboCache {
    signature: Option<Vec<String>>,
    options: Vec<String>,
}

/// The material `ComboBox`'s preset names, in exact index order.
///
/// Index 0 is `"(none)"`. Every following index is a built-in name from
/// [`pleochroma_cut_core::MaterialCatalogue`], sorted by [`compare_material_names`]. Every
/// consumer maps an index back through this function. The list covers every built-in species
/// the renderer supports, because it comes from the catalogue with no custom materials.
///
/// The New Design dialog's material combo lists these names through
/// `EditorModel.new_material_options` (`ui/models/editor.slint`).
#[must_use]
pub fn builtin_preset_names() -> Vec<String> {
    std::iter::once("(none)".to_string())
        .chain(built_in_names_alphabetical())
        .collect()
}

/// How two material names compare in a list the user reads: case-insensitively by the
/// displayed text, with the exact text as the tie-break so the order is deterministic.
///
/// This is the one comparison every material list in the apps uses (the design settings
/// combo, the render combo, the New Design lists, the Rough Planner list, the CLI's
/// "Available" line), so they cannot drift apart.
#[must_use]
pub fn compare_material_names(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Sorts `names` by [`compare_material_names`].
pub fn sort_material_names(names: &mut [String]) {
    names.sort_by(|a, b| compare_material_names(a, b));
}

/// Every built-in material name (the catalogue's own set), alphabetical.
fn built_in_names_alphabetical() -> Vec<String> {
    let mut names = pleochroma_cut_core::MaterialCatalogue::build(&[]).names();
    sort_material_names(&mut names);
    names
}

/// The render material combo's entries.
///
/// The combo sits in the viewport header on the desktop and the page header in the browser
/// app: every built-in plus every custom material whose name is not already listed (a custom
/// material sharing a built-in's name is not offered twice), all in one alphabetical list.
#[must_use]
pub fn render_material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut names = built_in_names_alphabetical();
    for material in custom {
        if !names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&material.name))
        {
            names.push(material.name.clone());
        }
    }
    sort_material_names(&mut names);
    names
}

/// [`builtin_preset_names`]'s index for `name`. `0` for `None` or an unrecognized name, so the
/// result is never an out-of-range `ComboBox` index.
///
/// Test-only: production code uses [`design_material_index_from_name`], which also covers
/// custom catalogue materials. This built-ins-only form serves the round-trip test.
#[cfg(test)]
#[must_use]
pub fn material_index_from_name(name: Option<&str>) -> i32 {
    let names = builtin_preset_names();
    name.and_then(|n| names.iter().position(|p| p == n))
        .map_or(0, |i| i32::try_from(i).unwrap_or(i32::MAX))
}

/// The inverse of [`material_index_from_name`]: the name at `index` in [`builtin_preset_names`], or
/// `None` for index `0` ("(none)") or an out-of-range index.
///
/// The combo never produces an out-of-range index, so that case is defensive only.
#[must_use]
pub fn material_name_from_index(index: i32) -> Option<String> {
    let names = builtin_preset_names();
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i).cloned())
        .filter(|name| name != "(none)")
}

/// Parses the Yield form's three fields into
/// [`pleochroma_cut_core::Edit::SetGirdleDiameterMm`]/[`pleochroma_cut_core::Edit::SetMaterial`]'s
/// payloads.
///
/// A blank `girdle_diameter_mm` or `specific_gravity_override` parses to `None`, meaning the
/// preset's own figure is used. Anything else must be a positive, finite number.
///
/// The returned [`MaterialSelection`] is `current.with_specific_gravity_override(..)`, so the
/// name and RI override carry through from `current`. The Yield combo can name only a built-in
/// preset, so deriving the name from `_material_index` would rename a custom material to
/// "(none)". The parameter is unread and kept only so the call site's signature stays the same.
///
/// # Errors
///
/// A message when a non-blank girdle diameter or specific-gravity override is not a
/// positive, finite number.
pub fn parse_yield_form(
    girdle_diameter_mm: &str,
    _material_index: i32,
    specific_gravity_override: &str,
    current: &MaterialSelection,
) -> Result<(Option<f64>, MaterialSelection), String> {
    let girdle_diameter_mm = if girdle_diameter_mm.trim().is_empty() {
        None
    } else {
        let value = eval_number(girdle_diameter_mm, None)
            .map_err(|error| error.message("Girdle diameter", girdle_diameter_mm))?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Girdle diameter must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    let specific_gravity_override = if specific_gravity_override.trim().is_empty() {
        None
    } else {
        let value = eval_number(specific_gravity_override, None)
            .map_err(|error| error.message("Specific gravity", specific_gravity_override))?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Specific gravity override must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    Ok((
        girdle_diameter_mm,
        current.with_specific_gravity_override(specific_gravity_override),
    ))
}

/// Names the source of the design's effective refractive index, shown as "Eff.
///
/// RI" in the design settings panel, so a cutter can see whether the critical angle, MARGIN and
/// renders use the material's real index or a fallback.
///
/// Follows the precedence of `Design::effective_refractive_index_with` exactly (typed override,
/// custom catalogue material, built-in preset, then the legacy imported value), so the text
/// cannot disagree with the number.
#[must_use]
pub fn ri_source_text(material: &MaterialSelection, custom: &[GemMaterial]) -> String {
    if let Some(value) = material.refractive_index_override {
        return format!("typed override ({value:.4})");
    }
    if let Some(name) = material.name.as_deref() {
        if custom.iter().any(|gem| gem.name.eq_ignore_ascii_case(name)) {
            return format!("custom catalogue material '{name}'");
        }
        if pleochroma_cut_core::built_in_refractive_index(name).is_some() {
            return format!("built-in material '{name}'");
        }
        return format!(
            "'{name}' is not a recognized material -- falling back to this design's legacy imported value"
        );
    }
    "no material selected -- using this design's legacy imported value".to_string()
}

/// Appended to a custom material's name when it collides case-insensitively with a built-in, so
/// the combo lists both. The custom entry is the one rendered, since `EditorMaterialLookup`
/// gives custom materials precedence. [`design_material_name_from_index`] strips the suffix
/// before the name is stored.
const CUSTOM_BUILTIN_COLLISION_SUFFIX: &str = " (custom)";

/// The design settings panel's material combo, in the exact order it lists.
///
/// A leading `"(none)"`, then every built-in and custom name in one alphabetical list
/// ([`compare_material_names`]), then a final `"Custom RI…"` sentinel (see
/// [`design_material_name_from_index`]).
///
/// A custom name that collides case-insensitively with a built-in is labeled with
/// [`CUSTOM_BUILTIN_COLLISION_SUFFIX`] rather than skipped.
///
/// Callers that refresh often should use [`MaterialComboCache::options`], which returns the
/// cached list until the custom names change.
#[must_use]
pub fn design_material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut names: Vec<String> = built_in_names_alphabetical();
    for material in custom {
        if names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&material.name))
        {
            names.push(format!(
                "{}{CUSTOM_BUILTIN_COLLISION_SUFFIX}",
                material.name
            ));
        } else {
            names.push(material.name.clone());
        }
    }
    sort_material_names(&mut names);
    let mut options: Vec<String> = Vec::with_capacity(names.len() + 2);
    options.push("(none)".to_string());
    options.extend(names);
    options.push("Custom RI\u{2026}".to_string());
    options
}

/// The inverse of [`design_material_name_from_index`]: `options`' index for `name`, matched
/// case-insensitively against the real name (a collision suffix is stripped first).
///
/// `0` ("(none)") when `name` is absent or not found.
#[must_use]
pub fn design_material_index_from_name(name: Option<&str>, options: &[String]) -> i32 {
    name.and_then(|n| {
        options.iter().position(|o| {
            o.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .unwrap_or(o)
                .eq_ignore_ascii_case(n)
        })
    })
    .map_or(0, |i| i32::try_from(i).unwrap_or(i32::MAX))
}

/// The name at `index` in `options`, or `None` for index `0` ("(none)"), the trailing
/// `"Custom RI…"` sentinel, or an out-of-range index.
///
/// Selecting `"Custom RI…"` clears `MaterialSelection::name`, so a species with no preset
/// (e.g. garnet) can take a typed refractive index. A collision-labeled entry is returned with
/// its suffix stripped: the stored name must stay the real name so it resolves through
/// `EditorMaterialLookup`.
#[must_use]
pub fn design_material_name_from_index(index: i32, options: &[String]) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.get(i))
        .filter(|&name| name != "(none)" && name != "Custom RI\u{2026}")
        .map(|name| {
            name.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .map_or_else(|| name.clone(), str::to_string)
        })
}

/// Parses the design settings panel's material combo index and RI override text into a new
/// [`MaterialSelection`].
///
/// The specific gravity override is carried through from `current`, since [`Edit::SetMaterial`]
/// replaces the whole selection. Color overrides and variants carry through only while the
/// material stays the same (case-insensitively): a color picked for one species does not
/// apply to another.
///
/// An empty override field means `None`, which uses the resolved material's own `n_D`. A
/// non-blank field must be a finite refractive index above 1.0.
///
/// # Errors
///
/// A message when a non-blank RI override is not a finite number greater than 1.0.
pub fn parse_design_material_form(
    combo_index: i32,
    ri_override_text: &str,
    options: &[String],
    current: &MaterialSelection,
) -> Result<MaterialSelection, String> {
    let refractive_index_override = if ri_override_text.trim().is_empty() {
        None
    } else {
        let value = eval_number(ri_override_text, None)
            .map_err(|error| error.message("Refractive index", ri_override_text))?;
        if !value.is_finite() || value <= 1.0 {
            return Err(
                "Refractive index override must be a finite number greater than 1.0.".to_string(),
            );
        }
        Some(value)
    };
    let name = design_material_name_from_index(combo_index, options);
    let same_material = match (name.as_deref(), current.name.as_deref()) {
        (Some(new), Some(old)) => new.eq_ignore_ascii_case(old),
        (None, None) => true,
        _ => false,
    };
    Ok(MaterialSelection {
        name,
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override,
        body_color_override: current.body_color_override.filter(|_| same_material),
        body_color_bands_override: current
            .body_color_bands_override
            .clone()
            .filter(|_| same_material),
        absorption_path_scale_override: current
            .absorption_path_scale_override
            .filter(|_| same_material),
        color_pattern_override: current
            .color_pattern_override
            .clone()
            .filter(|_| same_material),
        // A variant belongs to its material: kept while the material stays, dropped with it.
        color_variant_override: current
            .color_variant_override
            .clone()
            .filter(|_| same_material),
    })
}

/// The design settings panel's color combo options, in the fixed-index contract the web and
/// mobile combos still use.
///
/// The desktop combo lists a material's variants and uses the keyed [`color_entries`] instead.
///
/// `"Material Default"` (index 0, no override) followed by every
/// [`pleochroma::optics::materials::body_color::BODY_COLOR_PRESETS`] label, in
/// that table's own order -- so combo index `i >= 1` is preset `i - 1` -- and a last
/// entry, [`BODY_COLOR_CUSTOM_LABEL`], that opens the hue/saturation/brightness picker
/// ([`body_color_custom_index`]).
#[must_use]
pub fn body_color_options() -> Vec<String> {
    std::iter::once("Material Default".to_string())
        .chain(BODY_COLOR_PRESETS.iter().map(|p| p.label.to_string()))
        .chain(std::iter::once(BODY_COLOR_CUSTOM_LABEL.to_string()))
        .collect()
}

/// The label of the last [`body_color_options`] entry.
pub const BODY_COLOR_CUSTOM_LABEL: &str = "Custom\u{2026}";

/// The index of the trailing "Custom..." entry of [`body_color_options`]: one past the last
/// preset.
#[must_use]
pub fn body_color_custom_index() -> i32 {
    i32::try_from(BODY_COLOR_PRESETS.len() + 1).unwrap_or(i32::MAX)
}

/// [`body_color_options`]' index for `rgb`.
///
/// `0` for no override, `preset + 1` for a triple that matches a preset exactly, and
/// [`body_color_custom_index`] for a triple that matches none (a colour picked with the
/// picker, or typed into a design file by hand).
#[must_use]
pub fn body_color_index_for(rgb: Option<[f32; 3]>) -> i32 {
    rgb.map_or(0, |rgb| {
        preset_index_for_rgb(rgb).map_or_else(body_color_custom_index, |index| {
            i32::try_from(index).unwrap_or(i32::MAX).saturating_add(1)
        })
    })
}

/// The combo index of a design's whole colour.
///
/// It is [`body_color_custom_index`] while the
/// N-band form (the L*C*h editor's colour) is set, whatever its stored triple is, else
/// [`body_color_index_for`] the triple.
#[must_use]
pub fn body_color_index_for_material(material: &MaterialSelection) -> i32 {
    // A color variant is not in the preset table, so it reads as the custom entry, never as
    // "Material Default".
    if material.color_variant_override.is_some()
        || material
            .body_color_bands_override
            .as_ref()
            .is_some_and(|bands| !bands.is_empty())
    {
        body_color_custom_index()
    } else {
        body_color_index_for(material.body_color_override)
    }
}

/// The inverse of [`body_color_index_for`] for the fixed entries.
///
/// `None` ("Material Default") for index `0`, an out-of-range index, or the custom index. A
/// custom triple is in no table, so the caller keeps the design's current triple. Otherwise
/// preset `index - 1`'s absorption triple.
#[must_use]
pub fn body_color_from_index(index: i32) -> Option<[f32; 3]> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| BODY_COLOR_PRESETS.get(i))
        .map(|preset| preset.absorption_rgb)
}

/// The colour an Apply stores for the combo's selected `index`: the custom entry keeps the
/// design's `current` triple, every other index resolves through [`body_color_from_index`].
#[must_use]
pub fn body_color_for_apply(index: i32, current: Option<[f32; 3]>) -> Option<[f32; 3]> {
    if index == body_color_custom_index() {
        current
    } else {
        body_color_from_index(index)
    }
}

/// `material` with the colour the combo's selected `index` stands for, given the design's
/// `current` selection.
///
/// The custom entry keeps the design's whole current colour (the triple AND the N-band form
/// with its path scale, so a colour made in the L*C*h editor survives an Apply of the
/// material row) AND the design's current color pattern ("keep what the design has"); every
/// other index sets that preset's triple (or none) and DROPS the bands, which would otherwise
/// win over it. A preset replaces the stone's pattern too (the desktop's rule): an
/// intentional plain-color replacement clears the pattern override.
#[must_use]
pub fn with_body_color_choice(
    material: MaterialSelection,
    index: i32,
    current: &MaterialSelection,
) -> MaterialSelection {
    if index == body_color_custom_index() {
        if current.color_variant_override.is_some() {
            // The design's color is a variant: keep it (and the pattern) as it is.
            return material
                .with_color_variant(current.color_variant_override.clone())
                .with_color_pattern(current.color_pattern_override.clone());
        }
        material
            .with_body_color_bands(
                current.body_color_override,
                current.body_color_bands_override.clone(),
                current.absorption_path_scale_override,
            )
            .with_color_pattern(current.color_pattern_override.clone())
    } else {
        material
            .with_color_pattern(None)
            .with_body_color(body_color_from_index(index))
    }
}

/// The gear combo's index for `gear_teeth`.
///
/// It is a position in [`GEAR_PRESETS`] when it
/// matches exactly, else `GEAR_PRESETS.len()` (the trailing "Custom" entry), matching
/// [`gear_choice_to_teeth`]'s inverse mapping.
#[must_use]
pub fn gear_index_from_teeth(gear_teeth: i32) -> i32 {
    GEAR_PRESETS
        .iter()
        .position(|&g| g == gear_teeth)
        .map_or_else(
            || i32::try_from(GEAR_PRESETS.len()).unwrap_or(i32::MAX),
            |i| i32::try_from(i).unwrap_or(i32::MAX),
        )
}

/// The inverse of [`gear_index_from_teeth`]: resolves the gear combo's selected index
/// (plus, only for the trailing "Custom" entry, the paired text field) to a real gear
/// tooth count.
///
/// `preset_index` outside `0..=GEAR_PRESETS.len()` is defensive-only and
/// falls back to reading `custom_text`, same as an explicit "Custom" pick.
///
/// # Errors
///
/// A message when the custom tooth count is not a positive whole number.
pub fn gear_choice_to_teeth(preset_index: i32, custom_text: &str) -> Result<i32, String> {
    if let Ok(i) = usize::try_from(preset_index)
        && let Some(&teeth) = GEAR_PRESETS.get(i)
    {
        return Ok(teeth);
    }
    let teeth: i32 = custom_text.trim().parse().map_err(|_| {
        format!(
            "Gear tooth count '{}' is not a whole number.",
            custom_text.trim()
        )
    })?;
    if teeth <= 0 {
        return Err("Gear tooth count must be a positive whole number.".to_string());
    }
    Ok(teeth)
}

/// A real dry run of [`Edit::RemapIndices`] against a scratch clone of `design`
/// (never `design` itself), turned into the [`GearRemapPreviewRow`]s the gear-remap
/// confirmation panel shows.
///
/// `new_indices` comes from a real `apply_edit` call
/// (staying byte-identical to what confirming would actually write), while
/// `non_integral` is computed from the UNROUNDED ratio so it flags a tier whose ideal
/// new position isn't a whole tooth regardless of which `rounding` this preview uses.
#[must_use]
pub fn gear_remap_preview(
    design: &Design,
    from_gear: i32,
    to_gear: i32,
    rounding: RemapRounding,
) -> Vec<GearRemapPreviewRow> {
    let format_indices = |v: &[f64]| v.iter().map(f64::to_string).collect::<Vec<_>>().join(", ");
    let original: Vec<String> = design
        .tiers
        .iter()
        .map(|t| format_indices(&t.indices))
        .collect();
    // The magnitude-only ratio `Edit::RemapIndices` applies, so the two agree for a negative
    // tooth count.
    let ratio = pleochroma_cut_core::remap_ratio(from_gear, to_gear);
    let non_integral: Vec<bool> = design
        .tiers
        .iter()
        .map(|t| t.indices.iter().any(|&i| (i * ratio).fract() != 0.0))
        .collect();

    let mut remapped = design.clone();
    if remapped
        .apply_edit(Edit::RemapIndices {
            from_gear,
            to_gear,
            rounding,
        })
        .is_err()
    {
        // `RemapIndices` names no tier index, so `apply_edit` cannot fail for it.
        // Defensive only: an empty preview reads as "nothing to remap" rather than
        // panicking on a future change to that contract.
        return Vec::new();
    }

    design
        .tiers
        .iter()
        .zip(&remapped.tiers)
        .enumerate()
        .map(|(i, (before, after))| GearRemapPreviewRow {
            name: before.name.clone(),
            old_indices: original[i].clone(),
            new_indices: format_indices(&after.indices),
            non_integral: non_integral[i],
        })
        .collect()
}
