//! The design color combo's keyed entry list.
//!
//! The combo used to be a fixed index contract (0 = the material's own color, 1-9 = the nine
//! generic presets, last = Custom). A material now has its own color variants, so the list is
//! keyed: every entry is a [`ColorEntry`] (the material's default, a variant by its key, a preset
//! by its index, or Custom) with its label, swatch and tooltip ([`ColorEntryItem`]).
//!
//! Order ([`color_entries`]): Material Default, the material's built-in variants, the cutter's
//! saved variants for that material, the nine generic presets (ONLY for an isotropic host, where
//! one color is physically right) and Custom, which is always last. A design that carries a
//! legacy preset color on a pleochroic host (an old file) still shows it, as a
//! "Legacy: <label> (No Pleochroism)" entry before Custom ([`color_entries_for_design`]); it keeps
//! rendering as saved, and "Convert" opens the Custom editor seeded from it.
//!
//! A swatch is the ORIENTED face-up color of the entry (`variants::face_up_swatch`: the stone's
//! default crystal axis through the table, the unpolarised average for a one-color tensor) at the
//! stone width the entry's numbers are calibrated for (`ColorVariant::reference_path_mm`, 7 mm for
//! the Material Default and the presets); the tooltip names the color of each direction.
//!
//! [`entry_for_selection`] and [`selection_with_entry`] map between a design's
//! [`MaterialSelection`] and an entry, so the desktop, the web app and the mobile app can share
//! the list. No Slint here, so the rules are unit-testable without a window.

use pleochroma::{
    color::body_color::{BodyColor, BodyColors, Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionTensor, legacy_rgb_bands},
        materials::{
            AbsorptionUnit, GemMaterial,
            body_color::{BODY_COLOR_PRESETS, preset_index_for_rgb},
            variants::{
                ColorVariant, MATERIAL_DEFAULT_LABEL, MATERIAL_DEFAULT_REFERENCE_PATH_MM,
                TensorRank, VariantProvenance, built_in_variants, face_up_swatch,
                face_up_swatch_path, host_rank, tensor_rank,
            },
        },
    },
};
use pleochroma_cut_core::{
    MaterialSelection,
    material::{CUSTOM_VARIANT_KEY, VariantChoice},
};

use super::BODY_COLOR_CUSTOM_LABEL;
use crate::lch_color::{axis_rows, color_family, lch_of_body_color};

/// What one entry of the color combo stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorEntry {
    /// The material's own color: no color override at all.
    MaterialDefault,
    /// A color variant of the material, by its stable key (`"sapphire.pink"`, `"user.<uuid>"`).
    Variant {
        /// The variant's key.
        key: String,
    },
    /// One of the nine generic presets (index into `BODY_COLOR_PRESETS`): one isotropic color.
    Preset(usize),
    /// The Custom editor: "keep the design's current color", and open the editor.
    Custom,
}

/// One row of the color combo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorEntryItem {
    /// The label shown in the combo (Title Case).
    pub label: String,
    /// The entry's face-up color as `"#rrggbb"`; empty for Custom, which has no color of its own.
    pub swatch_hex: String,
    /// The tooltip: for a pleochroic entry the color of each direction ("Along the Axis: Blue /
    /// Across the Axis: Green"), else the face-up color; a note for an estimated or saved entry.
    pub tooltip: String,
    /// What picking the row does.
    pub entry: ColorEntry,
}

impl ColorEntryItem {
    /// [`Self::swatch_hex`] as bytes; `None` for Custom.
    #[must_use]
    pub fn swatch_rgb(&self) -> Option<[u8; 3]> {
        rgb_from_hex(&self.swatch_hex)
    }
}

/// `"#rrggbb"` for sRGB bytes.
#[must_use]
pub fn hex_from_rgb(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// The bytes of a `"#rrggbb"` string; `None` for anything else (including the empty string).
#[must_use]
pub fn rgb_from_hex(hex: &str) -> Option<[u8; 3]> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Whether two selections name the same material (case-insensitively; two nameless selections
/// count as the same), the rule of `parse_design_material_form`.
const fn same_material(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

/// The tooltip text of a tensor whose body `colors` are known: the color of each direction for a
/// pleochroic one, else the face-up color.
fn describe(tensor: &AbsorptionTensor, colors: &BodyColors) -> String {
    if !tensor.is_pleochroic {
        return format!(
            "Face-up color: {}.",
            color_family(lch_of_body_color(&colors.unpolarised))
        );
    }
    let mut rays: Vec<&BodyColor> = vec![&colors.o_ray];
    if let Some(beta) = &colors.beta_ray {
        rays.push(beta);
    }
    rays.push(&colors.e_ray);
    axis_rows(tensor_rank(tensor))
        .iter()
        .zip(rays)
        .map(|(row, color)| format!("{}: {}", row.label, color_family(lch_of_body_color(color))))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The face-up optical path of a stone of the default 7 mm width for a tensor in `unit`
/// (`face_up_swatch_path`): one model unit for a per-model-unit tensor, 9.905 mm for a per-mm one.
/// 7 mm is the stone width, not the path, so it is never passed to `body_colors` directly.
fn default_swatch_path(unit: AbsorptionUnit) -> f64 {
    face_up_swatch_path(unit, MATERIAL_DEFAULT_REFERENCE_PATH_MM)
}

/// The swatch and the tooltip of `tensor` (in `unit`) for a stone `stone_width_mm` wide on `host`,
/// with `lead` ahead of the tooltip.
///
/// The swatch is the oriented face-up color (`face_up_swatch`); the tooltip names the color of each
/// direction, read at the same optical path (`face_up_swatch_path`).
fn look_of(
    host: &GemMaterial,
    tensor: &AbsorptionTensor,
    unit: AbsorptionUnit,
    stone_width_mm: f32,
    lead: &str,
) -> (String, String) {
    let swatch = face_up_swatch(tensor, unit, f64::from(stone_width_mm), host);
    let colors = body_colors(
        tensor,
        face_up_swatch_path(unit, stone_width_mm),
        Illuminant::D65,
    );
    (
        hex_from_rgb(swatch.srgb),
        format!("{lead}{}", describe(tensor, &colors)),
    )
}

/// The Material Default row: the material's own tensor.
fn default_item(material: &GemMaterial) -> ColorEntryItem {
    let (swatch_hex, tooltip) = look_of(
        material,
        &material.absorption,
        material.absorption_unit,
        MATERIAL_DEFAULT_REFERENCE_PATH_MM,
        "The material's own color. ",
    );
    ColorEntryItem {
        label: MATERIAL_DEFAULT_LABEL.to_owned(),
        swatch_hex,
        tooltip,
        entry: ColorEntry::MaterialDefault,
    }
}

/// The note of an estimated built-in variant: `"Estimated: <first source line>"`, so the tooltip
/// names where the numbers come from and says they are not a fit to a measured spectrum. `None`
/// for a measured built-in, the material's own color and a saved variant.
#[must_use]
pub fn estimated_note(provenance: &VariantProvenance) -> Option<String> {
    let VariantProvenance::BuiltIn {
        sources,
        estimated: true,
        ..
    } = provenance
    else {
        return None;
    };
    let first = sources
        .iter()
        .flat_map(|source| source.lines())
        .map(str::trim)
        .find(|line| !line.is_empty());
    Some(first.map_or_else(
        || "Estimated: no published spectrum.".to_owned(),
        |line| format!("Estimated: {line}"),
    ))
}

/// A variant's row on `host`, shown for the stone width its numbers are calibrated for
/// (`ColorVariant::reference_path_mm`).
fn variant_item(host: &GemMaterial, variant: &ColorVariant) -> ColorEntryItem {
    let (swatch_hex, mut tooltip) = look_of(
        host,
        &variant.absorption,
        variant.unit,
        variant.reference_path_mm,
        "",
    );
    if let Some(note) = estimated_note(&variant.provenance) {
        tooltip.push(' ');
        tooltip.push_str(&note);
    } else if variant.provenance == VariantProvenance::User {
        tooltip.push_str(" Saved by you.");
    }
    ColorEntryItem {
        label: variant.label.clone(),
        swatch_hex,
        tooltip,
        entry: ColorEntry::Variant {
            key: variant.key.clone(),
        },
    }
}

/// The swatch and tooltip of generic preset `index`, shown for the 7 mm stone the presets are
/// calibrated for (they are per-model-unit colors).
///
/// A preset is one color in every direction, so its face-up color is the unpolarised color and
/// needs no host.
fn preset_look(index: usize, lead: &str) -> (String, String) {
    let rgb = BODY_COLOR_PRESETS
        .get(index)
        .map_or([0.0; 3], |preset| preset.absorption_rgb);
    let tensor = AbsorptionTensor::isotropic(legacy_rgb_bands(rgb));
    let colors = body_colors(
        &tensor,
        default_swatch_path(AbsorptionUnit::ModelUnit),
        Illuminant::D65,
    );
    (
        hex_from_rgb(colors.unpolarised.srgb),
        format!("{lead}{}", describe(&tensor, &colors)),
    )
}

/// A generic preset's row.
fn preset_item(index: usize) -> ColorEntryItem {
    let (swatch_hex, tooltip) = preset_look(
        index,
        "A generic one-color preset: the same color in every direction. ",
    );
    ColorEntryItem {
        label: BODY_COLOR_PRESETS
            .get(index)
            .map_or_else(String::new, |preset| preset.label.to_owned()),
        swatch_hex,
        tooltip,
        entry: ColorEntry::Preset(index),
    }
}

/// The row of a legacy preset color on a pleochroic host.
fn legacy_item(index: usize) -> ColorEntryItem {
    let (swatch_hex, _) = preset_look(index, "");
    let label = BODY_COLOR_PRESETS
        .get(index)
        .map_or("", |preset| preset.label);
    ColorEntryItem {
        label: format!("Legacy: {label} (No Pleochroism)"),
        swatch_hex,
        tooltip: "This design keeps the single color it was saved with, so the stone shows the \
                  same color in every direction. Convert it to set a color for each direction."
            .to_owned(),
        entry: ColorEntry::Preset(index),
    }
}

/// The Custom row, last in every list.
fn custom_item() -> ColorEntryItem {
    ColorEntryItem {
        label: BODY_COLOR_CUSTOM_LABEL.to_owned(),
        swatch_hex: String::new(),
        tooltip: "Open the color editor: one color for a stone that looks the same in every \
                  direction, a color along and across the axis for a two-color stone, and a color \
                  for each of alpha, beta and gamma for a three-color stone."
            .to_owned(),
        entry: ColorEntry::Custom,
    }
}

/// The variants the combo lists for `material`.
///
/// Its built-in variants (without the Material Default, which is the first row on its own), then
/// `user_variants` that belong to it, each only if the host accepts it
/// (`ColorVariant::validate_for`, so a pleochroic host never lists an isotropic color).
#[must_use]
pub fn listed_variants(
    material: &GemMaterial,
    user_variants: &[ColorVariant],
) -> Vec<ColorVariant> {
    built_in_variants(&material.name)
        .into_iter()
        .filter(|variant| !matches!(variant.provenance, VariantProvenance::MaterialDefault))
        .chain(
            user_variants
                .iter()
                .filter(|variant| variant.material.eq_ignore_ascii_case(&material.name))
                .cloned(),
        )
        .filter(|variant| variant.validate_for(material).is_ok())
        .collect()
}

/// The color combo's entries for `material`.
///
/// The order is Material Default, the material's built-in variants, `user_variants` (the cutter's
/// saved variants; ones of another material, or that the host does not accept, are skipped), the
/// nine generic presets (ONLY when the host is isotropic) and Custom.
///
/// Swatches are the oriented face-up D65 color for the stone width of each entry (its
/// `reference_path_mm`; 7 mm for the Material Default and the presets).
#[must_use]
pub fn color_entries(
    material: &GemMaterial,
    user_variants: &[ColorVariant],
) -> Vec<ColorEntryItem> {
    let mut items = vec![default_item(material)];
    items.extend(
        listed_variants(material, user_variants)
            .iter()
            .map(|variant| variant_item(material, variant)),
    );
    if host_rank(material) == TensorRank::Isotropic {
        items.extend((0..BODY_COLOR_PRESETS.len()).map(preset_item));
    }
    items.push(custom_item());
    items
}

/// The index of the generic preset a design's legacy triple stands for.
///
/// Only when the host is pleochroic and the design stores that triple alone (no variant, no
/// N-band form): the "Legacy" entry of [`color_entries_for_design`].
fn legacy_preset_index(current: &MaterialSelection, material: &GemMaterial) -> Option<usize> {
    if host_rank(material) == TensorRank::Isotropic
        || current.color_variant_override.is_some()
        || current
            .body_color_bands_override
            .as_ref()
            .is_some_and(|bands| !bands.is_empty())
    {
        return None;
    }
    current.body_color_override.and_then(preset_index_for_rgb)
}

/// Whether `current` carries a legacy isotropic color on the pleochroic host `material`.
///
/// A legacy color is a preset triple, a hand-picked triple or the old L*C*h bands: the stone then
/// renders without its pleochroism, exactly as saved, and the UI shows the "Legacy Color: No
/// Pleochroism" note with "Convert".
#[must_use]
pub fn legacy_color_on_pleochroic_host(
    current: &MaterialSelection,
    material: &GemMaterial,
) -> bool {
    host_rank(material) != TensorRank::Isotropic
        && current.color_variant_override.is_none()
        && (current.body_color_override.is_some()
            || current
                .body_color_bands_override
                .as_ref()
                .is_some_and(|bands| !bands.is_empty()))
}

/// [`color_entries`] plus the "Legacy" entry of a design with an old preset color.
///
/// For a design that carries a legacy preset color on a pleochroic host, a "Legacy: <label> (No
/// Pleochroism)" entry goes just before Custom (so Custom stays last). Picking it keeps the legacy
/// color; it is how [`entry_for_selection`] shows such a design.
#[must_use]
pub fn color_entries_for_design(
    material: &GemMaterial,
    user_variants: &[ColorVariant],
    current: &MaterialSelection,
) -> Vec<ColorEntryItem> {
    let mut items = color_entries(material, user_variants);
    if let Some(index) = legacy_preset_index(current, material) {
        items.insert(items.len().saturating_sub(1), legacy_item(index));
    }
    items
}

/// The position in `entries` of the row that shows `selection`'s color.
///
/// A color variant shows as its own row (by key), a Custom variant or a variant that is not
/// listed (a deleted saved variant, a variant of another host) as Custom, the legacy N-band color
/// as Custom, a legacy preset triple as its preset row (the plain one on an isotropic host, the
/// "Legacy" row on a pleochroic one), any other triple as Custom, and no color as Material
/// Default (position 0). Custom is the last row, so "the combo sits on Custom" always means
/// "keep the design's color as it is".
#[must_use]
pub fn entry_for_selection(selection: &MaterialSelection, entries: &[ColorEntryItem]) -> usize {
    let find = |wanted: &ColorEntry| entries.iter().position(|item| item.entry == *wanted);
    let custom = find(&ColorEntry::Custom).unwrap_or_else(|| entries.len().saturating_sub(1));
    if let Some(choice) = &selection.color_variant_override {
        if choice.key == CUSTOM_VARIANT_KEY {
            return custom;
        }
        return find(&ColorEntry::Variant {
            key: choice.key.clone(),
        })
        .unwrap_or(custom);
    }
    if selection
        .body_color_bands_override
        .as_ref()
        .is_some_and(|bands| !bands.is_empty())
    {
        return custom;
    }
    if let Some(rgb) = selection.body_color_override {
        return preset_index_for_rgb(rgb)
            .and_then(|index| find(&ColorEntry::Preset(index)))
            .unwrap_or(custom);
    }
    find(&ColorEntry::MaterialDefault).unwrap_or(0)
}

/// `material` with a plain color: the stone's pattern replaced (it would otherwise hide the
/// color) and `rgb` as the legacy triple (`None` = the material's own color).
fn plain(material: MaterialSelection, rgb: Option<[f32; 3]>) -> MaterialSelection {
    material.with_color_pattern(None).with_body_color(rgb)
}

/// `material` with the design's whole current color kept ("Custom": leave the color as it is) and
/// its color pattern.
///
/// A color variant belongs to its material, so it is kept only while the material stays; the
/// legacy colors carry through a species change as they always did (the combo is authoritative).
fn keep_current(material: MaterialSelection, current: &MaterialSelection) -> MaterialSelection {
    let pattern = current.color_pattern_override.clone();
    if let Some(choice) = &current.color_variant_override {
        let base = material.with_color_pattern(pattern);
        return if same_material(base.name.as_deref(), current.name.as_deref()) {
            base.with_color_variant(Some(choice.clone()))
        } else {
            base.with_body_color(None)
        };
    }
    material
        .with_body_color_bands(
            current.body_color_override,
            current.body_color_bands_override.clone(),
            current.absorption_path_scale_override,
        )
        .with_color_pattern(pattern)
}

/// Whether `current` stores the N-band form of the old L*C*h editor.
fn has_color_bands(current: &MaterialSelection) -> bool {
    current
        .body_color_bands_override
        .as_ref()
        .is_some_and(|bands| !bands.is_empty())
}

/// Whether `entry` is the row `current` already sits on, so an Apply with the combo left alone
/// changes nothing: the same material and
///
/// * a variant row whose key is the design's variant (its snapshot, however the library row has
///   moved since, and a color pattern stay);
/// * a preset row whose triple is the design's legacy triple (no variant, no N-band form);
/// * the Material Default row of a design with no color and no color pattern.
///
/// Custom is "keep the color" by itself and is not listed here.
fn is_current_entry(
    entry: &ColorEntry,
    material: &MaterialSelection,
    current: &MaterialSelection,
) -> bool {
    if !same_material(material.name.as_deref(), current.name.as_deref()) {
        return false;
    }
    match entry {
        ColorEntry::Variant { key } => current
            .color_variant_override
            .as_ref()
            .is_some_and(|choice| choice.key == *key),
        ColorEntry::Preset(index) => {
            current.color_variant_override.is_none()
                && !has_color_bands(current)
                && BODY_COLOR_PRESETS.get(*index).is_some_and(|preset| {
                    current.body_color_override == Some(preset.absorption_rgb)
                })
        }
        ColorEntry::MaterialDefault => {
            !current.has_body_color_override()
                && !has_color_bands(current)
                && current.color_pattern_override.is_none()
        }
        ColorEntry::Custom => false,
    }
}

/// `material` (the parsed material/RI row, see `parse_design_material_form`) with the color of
/// `entry` applied.
///
/// `current` is the design's selection and `variants` the variants the combo lists
/// ([`listed_variants`]).
///
/// * The row the design already sits on changes nothing ([`is_current_entry`]): the color and the
///   color pattern are kept as they are, so an Apply of the material row (an RI override, a
///   species) never refreshes a variant snapshot behind the cutter's back ("Variant Changed:
///   Update" is the way to refresh) and never drops a pattern.
/// * Material Default clears every color; a generic preset sets its legacy triple; both replace
///   the stone's color pattern (the desktop's rule: a plain color replaces the pattern).
/// * A variant is copied into the selection as a snapshot (`VariantChoice::from_variant`) and
///   also replaces the pattern. It applies only while the material stays the same and the design
///   names one: a variant belongs to its host, so on another material, or on a design with no
///   material name (the host would only be a refractive-index guess), the pick falls back to the
///   plain color of the material.
/// * Custom keeps the design's current color (variant, N-band form with its path scale, or
///   triple) and its pattern; the editor changes it afterwards. A variant key the list does not
///   hold also keeps the current color.
#[must_use]
pub fn selection_with_entry(
    material: MaterialSelection,
    entry: &ColorEntry,
    current: &MaterialSelection,
    variants: &[ColorVariant],
) -> MaterialSelection {
    if is_current_entry(entry, &material, current) {
        return keep_current(material, current);
    }
    match entry {
        ColorEntry::MaterialDefault => plain(material, None),
        ColorEntry::Preset(index) => plain(
            material,
            BODY_COLOR_PRESETS
                .get(*index)
                .map(|preset| preset.absorption_rgb),
        ),
        ColorEntry::Variant { key } => match variants.iter().find(|variant| variant.key == *key) {
            Some(variant)
                if material.name.is_some()
                    && same_material(material.name.as_deref(), current.name.as_deref()) =>
            {
                material
                    .with_color_pattern(None)
                    .with_color_variant(Some(VariantChoice::from_variant(variant)))
            }
            Some(_) => plain(material, None),
            None => keep_current(material, current),
        },
        ColorEntry::Custom => keep_current(material, current),
    }
}

#[cfg(test)]
mod tests;
